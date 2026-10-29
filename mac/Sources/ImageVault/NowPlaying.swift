import AppKit
import SwiftUI

/// System-wide Now Playing, read and controlled through MediaRemote.
///
/// Since macOS 15.4 MediaRemote refuses direct calls from third-party apps
/// ("Operation not permitted", entitlements=0). Apple's own `/usr/bin/osascript`
/// is allowed, so we run a tiny AppleScript through it and talk over stdout.
/// That is the same approach open-source notch players use. Everything is
/// best-effort: if osascript or the private framework is unavailable, the bar
/// simply never appears.
@MainActor
final class NowPlaying: ObservableObject {
    struct Track: Equatable, Identifiable {
        var title: String
        var artist: String
        var album: String
        var artwork: NSImage?
        var player: String
        var isPlaying: Bool
        /// Position at `anchor`; MediaRemote reports a snapshot, not a live value.
        var elapsed: Double
        var duration: Double
        var anchor = Date()
        var rate: Double = 1

        var id: String { "\(title)|\(artist)|\(player)|\(duration)" }

        /// Live position, projected from the last snapshot.
        func elapsed(at date: Date) -> Double {
            guard isPlaying else { return min(elapsed, duration) }
            let value = elapsed + max(0, date.timeIntervalSince(anchor)) * max(rate, 0)
            return duration > 0 ? min(value, duration) : value
        }

        func progress(at date: Date) -> Double {
            guard duration > 0 else { return 0 }
            return min(1, max(0, elapsed(at: date) / duration))
        }

        var progress: Double { progress(at: Date()) }

        var subtitle: String {
            artist.isEmpty ? album : artist
        }
    }

    /// MRMediaRemoteCommand
    enum Command: Int32 {
        case play = 0, pause = 1, toggle = 2, next = 4, previous = 5
        case seek = 24  // kMRMediaRemoteCommandSeekToPlaybackPosition (+ kMRMediaRemoteOptionPlaybackPosition)
    }

    static let shared = NowPlaying()

    @Published private(set) var track: Track?
    @Published private(set) var available = false

    /// Opt-out lives in the settings menu; defaults to on.
    var enabled: Bool {
        get { UserDefaults.standard.object(forKey: "musicControls") as? Bool ?? true }
        set {
            UserDefaults.standard.set(newValue, forKey: "musicControls")
            if newValue {
                if active { startTimer(); refresh() }
            } else {
                stopTimer()
                track = nil
                lastArtKey = ""
                artwork = nil
            }
        }
    }

    private let artURL = FileManager.default.temporaryDirectory
        .appendingPathComponent("ImageVault-nowplaying-art.img")
    private var readScript: URL?
    private var commandScript: URL?
    private var seekScript: URL?
    private var timer: Timer?
    private var reading = false
    private var active = false
    private var failures = 0
    private var lastArtKey = ""
    private var artwork: NSImage?
    private var fetchingArtKey = ""
    private var seekHoldUntil = Date.distantPast

    private init() {
        readScript = writeScript(Self.readScriptSource, name: "ImageVault-nowplaying-read.applescript")
        commandScript = writeScript(Self.commandScriptSource, name: "ImageVault-nowplaying-command.applescript")
        seekScript = writeScript(Self.seekScriptSource, name: "ImageVault-nowplaying-seek.applescript")
        available = readScript != nil && commandScript != nil
            && FileManager.default.isExecutableFile(atPath: "/usr/bin/osascript")
    }

    // MARK: Lifecycle

    /// Polling only runs while the tray is open, so the app is idle when resting.
    func setActive(_ on: Bool) {
        active = on
        if on {
            startTimer()
            refresh()
        } else {
            stopTimer()
        }
    }

    func stop() {
        stopTimer()
        active = false
        reading = false
        failures = 0
        lastArtKey = ""
        artwork = nil
    }

    private func startTimer() {
        guard timer == nil else { return }
        let t = Timer(timeInterval: 1.0, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.refresh() }
        }
        RunLoop.main.add(t, forMode: .common)
        timer = t
    }

    private func stopTimer() {
        timer?.invalidate()
        timer = nil
    }

    // MARK: Reading

    private func refresh() {
        guard enabled, active, !reading, let script = readScript else { return }
        reading = true
        let artPath = artURL.path
        DispatchQueue.global(qos: .utility).async {
            let line = Self.runScript(script, arguments: [artPath])
            DispatchQueue.main.async { [weak self] in
                MainActor.assumeIsolated {
                    guard let self else { return }
                    self.reading = false
                    self.consume(line)
                }
            }
        }
    }

    private func consume(_ line: String) {
        if line.hasPrefix("!none") {
            track = nil
            lastArtKey = ""
            artwork = nil
            return
        }
        if line.hasPrefix("!error") {
            failures += 1
            if failures >= 3 { track = nil }
            return
        }
        failures = 0
        let parts = line.split(separator: "\t", omittingEmptySubsequences: false).map(String.init)
        guard parts.count >= 9 else { return }

        func text(_ index: Int) -> String {
            guard index < parts.count,
                  let data = Data(base64Encoded: parts[index]),
                  let value = String(data: data, encoding: .utf8) else { return "" }
            return value
        }

        let title = text(0)
        guard !title.isEmpty else {
            track = nil
            return
        }
        let artist = text(1)
        let album = text(2)
        let rate = Double(text(3)) ?? 0
        let duration = Double(text(4)) ?? 0
        let elapsed = Double(text(5)) ?? 0
        let artID = text(6)
        let hasArt = parts[7] == "1"
        let player = text(8)
        let age = parts.count > 9 ? (Double(text(9)) ?? 0) : 0
        let artAvailable = parts.count > 10 ? parts[10] == "1" : false
        let artKey = artID.isEmpty ? "\(title)|\(artist)" : artID

        if hasArt {
            if artKey != lastArtKey {
                lastArtKey = artKey
                artwork = NSImage(contentsOf: artURL)
            }
        } else if artKey != lastArtKey {
            // Browsers advertise artwork without sending the bytes; ask the helper.
            lastArtKey = artKey
            artwork = nil
            if artAvailable || !artID.isEmpty { fetchArtwork(key: artKey) }
        }

        var next = Track(
            title: title,
            artist: artist,
            album: album,
            artwork: artwork,
            player: player,
            isPlaying: rate > 0.01,
            elapsed: elapsed,
            duration: duration,
            anchor: Date().addingTimeInterval(-max(0, age)),
            rate: rate > 0.01 ? rate : 1)

        // Right after a seek MediaRemote can still report the old position;
        // keep the requested one for a moment instead of snapping back.
        if Date() < seekHoldUntil, let current = track, current.id == next.id {
            next.elapsed = current.elapsed(at: Date())
            next.anchor = Date()
        }
        if next != track { track = next }
    }

    private func fetchArtwork(key: String) {
        guard fetchingArtKey != key,
              let lib = Bundle.main.resourceURL?.appendingPathComponent("libivmr.dylib"),
              FileManager.default.fileExists(atPath: lib.path) else { return }
        fetchingArtKey = key
        let out = artURL.path
        DispatchQueue.global(qos: .utility).async {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/usr/bin/perl")
            process.arguments = ["-e", #"use DynaLoader; my $l = DynaLoader::dl_load_file($ENV{IV_LIB}, 0) or exit 1; my $s = DynaLoader::dl_find_symbol($l, "iv_artwork") or exit 1; DynaLoader::dl_install_xsub("main::art", $s); art();"#]
            process.environment = ["IV_LIB": lib.path, "IV_ART_OUT": out]
            let pipe = Pipe()
            process.standardOutput = pipe
            process.standardError = FileHandle.nullDevice
            var ok = false
            if (try? process.run()) != nil {
                let watchdog = DispatchWorkItem { if process.isRunning { process.terminate() } }
                DispatchQueue.global().asyncAfter(deadline: .now() + 4, execute: watchdog)
                let data = pipe.fileHandleForReading.readDataToEndOfFile()
                process.waitUntilExit()
                watchdog.cancel()
                ok = String(decoding: data, as: UTF8.self).hasPrefix("ok")
            }
            let image = ok ? NSImage(contentsOfFile: out) : nil
            DispatchQueue.main.async { [weak self] in
                MainActor.assumeIsolated {
                    guard let self else { return }
                    self.fetchingArtKey = ""
                    guard self.lastArtKey == key, let image else { return }
                    self.artwork = image
                    if var current = self.track { current.artwork = image; self.track = current }
                }
            }
        }
    }

    // MARK: Controls

    func toggle() {
        guard var current = track else { return }
        current.elapsed = current.elapsed(at: Date())   // freeze / resume from here
        current.anchor = Date()
        current.isPlaying.toggle()
        track = current            // instant feedback; the next poll corrects it
        send(.toggle)
    }

    func next() { send(.next) }
    func previous() { send(.previous) }
    /// Sends the system toggle without needing a cached track (menu bar).
    func playPause() { send(.toggle) }

    /// Seek to a position (0.0 to 1.0)
    func seek(to progress: Double) {
        guard enabled, let script = seekScript, var current = track else { return }
        let targetTime = current.duration * progress
        current.elapsed = targetTime
        current.anchor = Date()
        track = current  // instant feedback
        seekHoldUntil = Date().addingTimeInterval(1.5)
        let argument = String(format: "%.2f", targetTime)
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            _ = Self.runScript(script, arguments: [argument])
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { [weak self] in
                self?.refresh()
            }
        }
    }

    private func send(_ command: Command) {
        guard enabled, let script = commandScript else { return }
        let argument = String(command.rawValue)
        DispatchQueue.global(qos: .userInitiated).async {
            _ = Self.runScript(script, arguments: [argument])
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { [weak self] in
                self?.refresh()
            }
        }
    }

    // MARK: Sample data for the off-screen design snapshots

    func previewSample() {
        let art = NSImage(size: NSSize(width: 60, height: 60), flipped: false) { rect in
            NSColor(white: 0.18, alpha: 1).setFill()
            rect.fill()
            NSColor(white: 1, alpha: 0.75).setFill()
            NSBezierPath(ovalIn: rect.insetBy(dx: 18, dy: 18)).fill()
            return true
        }
        track = Track(title: "Midnight City", artist: "M83", album: "Hurry Up, We're Dreaming",
                      artwork: art, player: "com.apple.Music",
                      isPlaying: true, elapsed: 68, duration: 244)
    }

    // MARK: osascript

    private func writeScript(_ text: String, name: String) -> URL? {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(name)
        do {
            try text.write(to: url, atomically: true, encoding: .utf8)
            return url
        } catch {
            return nil
        }
    }

    /// Blocks until the script finishes. Always run off the main thread.
    nonisolated private static func runScript(_ script: URL, arguments: [String]) -> String {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/osascript")
        process.arguments = [script.path] + arguments
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = pipe
        do {
            try process.run()
        } catch {
            return "!error:spawn"
        }
        // Never let a stuck osascript stall the poll forever.
        let watchdog = DispatchWorkItem {
            if process.isRunning { process.terminate() }
        }
        DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + 4, execute: watchdog)
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        watchdog.cancel()
        return String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private static let readScriptSource = #"""
    use framework "Foundation"

    on encodeText(value)
        if value is missing value then return ""
        set s to current application's NSString's stringWithString:(value as text)
        set d to s's dataUsingEncoding:(current application's NSUTF8StringEncoding)
        if d is missing value then return ""
        return (d's base64EncodedStringWithOptions:0) as text
    end encodeText

    on run argv
        set artPath to item 1 of argv
        try
            set mr to current application's NSBundle's bundleWithPath:"/System/Library/PrivateFrameworks/MediaRemote.framework/"
            mr's load()
            set cls to current application's NSClassFromString("MRNowPlayingRequest")
            set npItem to cls's localNowPlayingItem()
            if npItem is missing value then return "!none"
            set info to npItem's nowPlayingInfo()
            if info is missing value then return "!none"
            set title to info's valueForKey:"kMRMediaRemoteNowPlayingInfoTitle"
            if title is missing value then return "!none"
            set artist to info's valueForKey:"kMRMediaRemoteNowPlayingInfoArtist"
            set album to info's valueForKey:"kMRMediaRemoteNowPlayingInfoAlbum"
            set rate to info's valueForKey:"kMRMediaRemoteNowPlayingInfoPlaybackRate"
            set duration to info's valueForKey:"kMRMediaRemoteNowPlayingInfoDuration"
            set elapsed to info's valueForKey:"kMRMediaRemoteNowPlayingInfoElapsedTime"
            set artID to info's valueForKey:"kMRMediaRemoteNowPlayingInfoArtworkIdentifier"
            set artFlag to "0"
            set artData to info's valueForKey:"kMRMediaRemoteNowPlayingInfoArtworkData"
            if artData is missing value then
                try
                    set artObj to npItem's artwork()
                    if artObj is not missing value then set artData to artObj's imageData()
                end try
            end if
            if artData is not missing value then
                set wrote to artData's writeToFile:artPath atomically:true
                if wrote then set artFlag to "1"
            end if
            set player to ""
            try
                set player to (cls's localNowPlayingPlayerPath()'s client()'s bundleIdentifier()) as text
            end try
            set age to 0
            try
                set ts to info's valueForKey:"kMRMediaRemoteNowPlayingInfoTimestamp"
                if ts is not missing value then set age to -((ts's timeIntervalSinceNow()) as real)
            end try
            set artAvail to "0"
            try
                if (npItem's metadata()'s artworkAvailable()) as boolean then set artAvail to "1"
            end try
            set sep to tab
            return (encodeText(title)) & sep & (encodeText(artist)) & sep & (encodeText(album)) & sep & (encodeText(rate)) & sep & (encodeText(duration)) & sep & (encodeText(elapsed)) & sep & (encodeText(artID)) & sep & artFlag & sep & (encodeText(player)) & sep & (encodeText(age)) & sep & artAvail
        on error msg number n
            return "!error:" & msg
        end try
    end run
    """#

    private static let commandScriptSource = #"""
    use framework "Foundation"
    on run argv
        set cmd to (item 1 of argv) as integer
        try
            set mr to current application's NSBundle's bundleWithPath:"/System/Library/PrivateFrameworks/MediaRemote.framework/"
            mr's load()
            set cls to current application's NSClassFromString("MRNowPlayingRequest")
            set p to cls's localNowPlayingPlayerPath()
            set req to cls's alloc()'s initWithPlayerPath:p
            req's sendCommand:cmd options:(missing value) queue:(missing value) completion:(missing value)
            return "OK"
        on error msg number n
            return "!error:" & msg
        end try
    end run
    """#

    private static let seekScriptSource = #"""
    use framework "Foundation"
    on run argv
        set targetTime to (item 1 of argv) as double
        try
            set mr to current application's NSBundle's bundleWithPath:"/System/Library/PrivateFrameworks/MediaRemote.framework/"
            mr's load()
            set cls to current application's NSClassFromString("MRNowPlayingRequest")
            set p to cls's localNowPlayingPlayerPath()
            set req to cls's alloc()'s initWithPlayerPath:p
            set opts to current application's NSDictionary's dictionaryWithObject:targetTime forKey:"kMRMediaRemoteOptionPlaybackPosition"
            req's sendCommand:24 options:opts queue:(missing value) completion:(missing value)
            return "OK"
        on error msg number n
            return "!error:" & msg
        end try
    end run
    """#
}

/// Grayscale app icon for the app that is currently playing (browser, Music...).
@MainActor
enum PlayerIcon {
    private static var cache: [String: NSImage] = [:]
    private static var names: [String: String] = [:]

    static func icon(for bundleID: String) -> NSImage? {
        guard !bundleID.isEmpty else { return nil }
        if let hit = cache[bundleID] { return hit }
        guard let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundleID) else { return nil }
        let icon = NSWorkspace.shared.icon(forFile: url.path)
        let gray = grayscale(icon)
        cache[bundleID] = gray
        return gray
    }

    static func name(for bundleID: String) -> String {
        if let hit = names[bundleID] { return hit }
        guard !bundleID.isEmpty else { return "" }
        var name = bundleID
        if let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundleID),
           let bundle = Bundle(url: url) {
            name = bundle.object(forInfoDictionaryKey: "CFBundleDisplayName") as? String
                ?? bundle.object(forInfoDictionaryKey: "CFBundleName") as? String
                ?? name
        }
        names[bundleID] = name
        return name
    }

    static func open(_ bundleID: String) {
        guard !bundleID.isEmpty,
              let url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundleID) else { return }
        NSWorkspace.shared.openApplication(at: url,
                                           configuration: NSWorkspace.OpenConfiguration(),
                                           completionHandler: nil)
    }

    /// Tiny 64px grayscale conversion. Deliberately not Core Image: creating a
    /// CI context just for a 28pt icon costs tens of megabytes.
    private static func grayscale(_ image: NSImage) -> NSImage {
        let side = 64
        guard let rep = NSBitmapImageRep(bitmapDataPlanes: nil,
                                         pixelsWide: side, pixelsHigh: side,
                                         bitsPerSample: 8, samplesPerPixel: 4,
                                         hasAlpha: true, isPlanar: false,
                                         colorSpaceName: .deviceRGB,
                                         bytesPerRow: 0, bitsPerPixel: 0),
              let context = NSGraphicsContext(bitmapImageRep: rep) else { return image }
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = context
        image.draw(in: NSRect(x: 0, y: 0, width: CGFloat(side), height: CGFloat(side)))
        NSGraphicsContext.restoreGraphicsState()

        if let data = rep.bitmapData {
            for y in 0..<side {
                for x in 0..<side {
                    let pixel = data + y * rep.bytesPerRow + x * 4
                    let value = UInt8(0.299 * Double(pixel[0])
                                      + 0.587 * Double(pixel[1])
                                      + 0.114 * Double(pixel[2]))
                    pixel[0] = value
                    pixel[1] = value
                    pixel[2] = value
                }
            }
        }
        let result = NSImage(size: NSSize(width: side, height: side))
        result.addRepresentation(rep)
        return result
    }
}