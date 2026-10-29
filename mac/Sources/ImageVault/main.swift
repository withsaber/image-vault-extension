import AppKit

let app = NSApplication.shared

if let i = CommandLine.arguments.firstIndex(of: "--snapshot"), CommandLine.arguments.count > i + 1 {
    app.setActivationPolicy(.accessory)
    MainActor.assumeIsolated { Snapshot.run(to: CommandLine.arguments[i + 1]) }
    exit(0)
}
// Developer aid: run the OCR pipeline on an image and print the text.
if let i = CommandLine.arguments.firstIndex(of: "--ocr"), CommandLine.arguments.count > i + 1 {
    let url = URL(fileURLWithPath: CommandLine.arguments[i + 1])
    if let text = OCR.recognize(at: url) {
        print(text)
        exit(0)
    }
    print("NO TEXT")
    exit(1)
}
// Developer aid: annotate an image with every editor tool and render the
// result plus the preview, editor and pin windows to PNGs.
if let i = CommandLine.arguments.firstIndex(of: "--capture-demo"), CommandLine.arguments.count > i + 2 {
    app.setActivationPolicy(.accessory)
    MainActor.assumeIsolated { CaptureDemo.run(image: CommandLine.arguments[i + 1], out: CommandLine.arguments[i + 2]) }
    exit(0)
}
// Developer aid: capture a region and the whole main display with the real
// screenshot path and report pixel sizes (needs Screen Recording).
if let i = CommandLine.arguments.firstIndex(of: "--capture-test"), CommandLine.arguments.count > i + 1 {
    app.setActivationPolicy(.accessory)
    let out = URL(fileURLWithPath: CommandLine.arguments[i + 1])
    try? FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
    Task { @MainActor in
        let screen = NSScreen.screens[0]
        let f = screen.frame
        var lines: [String] = ["screen \(f.size) scale \(screen.backingScaleFactor) access \(CGPreflightScreenCaptureAccess())"]
        for (name, rect) in [("region", NSRect(x: f.minX + 100.4, y: f.minY + 200.6, width: 640.3, height: 360.7)), ("full", f)] {
            if let image = await ScreenOCRController.capture(rect: rect, screen: screen) {
                let doc = CaptureDocument(image: image, scale: screen.backingScaleFactor)
                try? doc.pngData()?.write(to: out.appendingPathComponent("\(name).png"))
                lines.append("\(name): requested \(rect.size) → \(image.width)×\(image.height) px, colorspace \(image.colorSpace?.name as String? ?? "?")")
            } else {
                lines.append("\(name): capture failed")
            }
        }
        try? lines.joined(separator: "\n").write(to: out.appendingPathComponent("report.txt"), atomically: true, encoding: .utf8)
        exit(0)
    }
    app.run()
}
// Developer aid: print what macOS currently reports as Now Playing.
if CommandLine.arguments.contains("--nowplaying") {
    app.setActivationPolicy(.accessory)
    MainActor.assumeIsolated {
        NowPlaying.shared.setActive(true)
        for _ in 0..<20 where NowPlaying.shared.track == nil {
            RunLoop.main.run(until: Date().addingTimeInterval(0.15))
        }
        if let track = NowPlaying.shared.track {
            print("now playing: \(track.title) — \(track.subtitle) · "
                + "\(track.isPlaying ? "playing" : "paused") · "
                + "\(Int(track.progress * 100))% · player=\(track.player) · art=\(track.artwork != nil) · "
                + "available=\(NowPlaying.shared.available)")
        } else {
            print("now playing: nothing (available=\(NowPlaying.shared.available), enabled=\(NowPlaying.shared.enabled))")
        }
        NowPlaying.shared.stop()
    }
    exit(0)
}
let controller = MainActor.assumeIsolated { AppController() }
app.delegate = controller
app.setActivationPolicy(.accessory)
app.run()
