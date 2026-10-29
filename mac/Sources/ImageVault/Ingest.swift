import AppKit
import UniformTypeIdentifiers
import ObjCExceptionCatcher

/// Turns whatever arrives on a pasteboard (drop, clipboard) into saved images.
/// Order: image files → file promises (Chrome, Photos) → raw image data → web URLs.
enum Ingest {
    struct Candidate {
        var data: Data
        var name: String?
        var url: String?
    }

    struct Summary {
        var added: [VaultItem] = []
        var duplicates: [VaultItem] = []
        var failed = 0
        var firstImage: NSImage?
        var isEmpty: Bool { added.isEmpty && duplicates.isEmpty }
    }

    static let imageDataTypes: [NSPasteboard.PasteboardType] = [
        .png, .tiff, .init(UTType.jpeg.identifier), .init(UTType.heic.identifier),
        .init(UTType.webP.identifier), .init(UTType.gif.identifier), .init(UTType.svg.identifier),
    ]
    static let preferredDataOrder: [NSPasteboard.PasteboardType] =
        [.init(UTType.gif.identifier), .init(UTType.svg.identifier)] + imageDataTypes.filter {
            $0.rawValue != UTType.gif.identifier && $0.rawValue != UTType.svg.identifier
        }
    static let promiseTypes = NSFilePromiseReceiver.readableDraggedTypes.map { NSPasteboard.PasteboardType($0) }
    static let acceptedTypes: [NSPasteboard.PasteboardType] = [.fileURL, .URL] + imageDataTypes + promiseTypes

    /// Quick synchronous check used while a drag is in flight.
    static func looksLikeImage(_ pb: NSPasteboard) -> Bool {
        let types = pb.types ?? []
        if types.contains(where: { imageDataTypes.contains($0) || promiseTypes.contains($0) }) { return true }
        if let urls = pb.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL],
           urls.contains(where: isImageFile) { return true }
        if let urls = pb.readObjects(forClasses: [NSURL.self]) as? [URL], urls.contains(where: { $0.scheme?.hasPrefix("http") == true }) {
            return true
        }
        return false
    }

    static func isImageFile(_ url: URL) -> Bool {
        guard url.isFileURL else { return false }
        if url.pathExtension.lowercased() == "svg" { return true }
        return UTType(filenameExtension: url.pathExtension)?.conforms(to: .image) == true
    }

    // MARK: Reading a drop

    /// Everything a drop can give us, captured while the drag is still alive.
    /// URLs and raw bytes are snapshots; promised files are requested straight
    /// away because AppKit only allows that during the dragging session.
    struct DropPayload {
        var files: [URL] = []
        var data: [Candidate] = []
        var urls: [URL] = []
        var promises: PromiseBatch?
    }

    /// Must be called synchronously from `performDragOperation`. Reading a
    /// pasteboard after the drag ends is unsupported, and asking a
    /// `NSFilePromiseReceiver` for its files then raises an ObjC exception,
    /// which terminates the app (the SIGABRT in the crash reports).
    static func readDrop(_ pb: NSPasteboard) -> DropPayload {
        var payload = DropPayload()

        // 1. Real image files (Finder, desktop, other apps' file drags).
        if let urls = pb.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL] {
            payload.files = urls.filter(isImageFile)
            if !payload.files.isEmpty { return payload }
        }

        // 2. File promises (dragging an image out of Chrome, Safari, Photos...).
        //    Start the copies now, while the dragging session is still alive.
        if let receivers = pb.readObjects(forClasses: [NSFilePromiseReceiver.self]) as? [NSFilePromiseReceiver], !receivers.isEmpty {
            payload.promises = PromiseBatch(receivers)
        }

        // 3. Raw image bytes (copied images, screenshots to clipboard).
        //    GIF and SVG first, so animation and vectors survive.
        for type in preferredDataOrder {
            if let data = pb.data(forType: type) {
                payload.data = [Candidate(data: data, name: nil, url: pb.string(forType: .URL))]
                break
            }
        }

        // 4. A web address pointing at an image.
        if let urls = pb.readObjects(forClasses: [NSURL.self]) as? [URL] {
            payload.urls = urls.filter { $0.scheme?.hasPrefix("http") == true }
        }
        return payload
    }

    /// Resolves a payload off the drag's lifetime (file reads, promise copies,
    /// downloads). Only `readDrop` has to happen during the drop.
    static func candidates(from payload: DropPayload) async -> [Candidate] {
        if !payload.files.isEmpty {
            let files = payload.files.compactMap { url -> Candidate? in
                guard let data = try? Data(contentsOf: url) else { return nil }
                return Candidate(data: data, name: url.lastPathComponent, url: nil)
            }
            if !files.isEmpty { return files }
        }

        if let promises = payload.promises {
            let promised = await promises.wait()
            if !promised.isEmpty { return promised }
        }

        if !payload.data.isEmpty { return payload.data }

        var out: [Candidate] = []
        for url in payload.urls {
            if let data = await download(url) {
                out.append(Candidate(data: data, name: url.lastPathComponent, url: url.absoluteString))
            }
        }
        return out
    }

    static func download(_ url: URL) async -> Data? {
        var request = URLRequest(url: url, timeoutInterval: 20)
        request.setValue("image/png,image/jpeg,image/gif;q=0.9,image/*;q=0.5,*/*;q=0.3", forHTTPHeaderField: "Accept")
        guard let (data, response) = try? await URLSession.shared.data(for: request),
              (response as? HTTPURLResponse).map({ (200..<300).contains($0.statusCode) }) ?? true else { return nil }
        return data
    }

    // MARK: Saving

    /// Store work only. Safe off the main thread: VaultStore serialises index
    /// writes with flock and every value here is a file/Data operation.
    static func persist(_ candidates: [Candidate], source: String) -> Summary {
        var summary = Summary()
        for c in candidates {
            let meta = VaultStore.Meta(name: c.name, url: c.url, pageTitle: nil, pageUrl: nil,
                                       domain: c.url.flatMap { URL(string: $0)?.host }, source: source)
            do {
                switch try VaultStore.shared.add(data: c.data, meta: meta) {
                case .added(let item): summary.added.append(item)
                case .duplicate(let item): summary.duplicates.append(item)
                }
            } catch {
                summary.failed += 1
            }
        }
        return summary
    }

    @MainActor
    static func finish(_ summary: Summary) -> Summary {
        var summary = summary
        // Decode the 22pt activity thumbnail, not the whole original image, on
        // the main thread (a 50MP screenshot would otherwise freeze the island).
        if summary.firstImage == nil, let item = summary.added.first ?? summary.duplicates.first,
           let cg = Imaging.thumbnail(at: VaultStore.shared.fileURL(for: item), maxPixel: 88) {
            summary.firstImage = NSImage(cgImage: cg, size: NSSize(width: cg.width, height: cg.height))
        }
        VaultModel.shared.reload(fresh: Set(summary.added.map(\.id)))
        return summary
    }

    /// Runs decode/encode off the main thread, then updates the UI on main.
    @MainActor
    static func saveAsync(_ candidates: [Candidate], source: String) async -> Summary {
        let persisted = await Task.detached(priority: .userInitiated) {
            persist(candidates, source: source)
        }.value
        return finish(persisted)
    }

    @MainActor
    static func importDrop(_ payload: DropPayload, source: String) async -> Summary {
        let found = await candidates(from: payload)
        return await saveAsync(found, source: source)
    }
}

/// One-shot flag used to balance a DispatchGroup.enter exactly once.
final class OnceFlag: @unchecked Sendable {
    private let lock = NSLock()
    private var done = false

    func claim() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        if done { return false }
        done = true
        return true
    }
}

/// Receives AppKit file promises. The request to copy the files has to be made
/// synchronously from the drag operation; only the copies themselves are async.
/// A promise source that never answers is cut off so the island can't stay
/// stuck on "Saving" forever.
final class PromiseBatch: @unchecked Sendable {
    private let group = DispatchGroup()
    private let lock = NSLock()
    private var received: [Ingest.Candidate] = []
    private let dir: URL

    init(_ receivers: [NSFilePromiseReceiver]) {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("ImageVault-\(UUID().uuidString)", isDirectory: true)
        self.dir = dir
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let queue = OperationQueue()
        queue.maxConcurrentOperationCount = 4
        for receiver in receivers {
            group.enter()
            // Each group.enter must be balanced exactly once: the reader can
            // run after an exception was thrown, so guard it with `once`.
            let once = OnceFlag()
            // AppKit raises an NSException (uncatchable in Swift) if a provider
            // hands back an unusable promised URL. Swallow it per receiver.
            let started = IVCatchException {
                receiver.receivePromisedFiles(atDestination: dir, options: [:], operationQueue: queue) { [self] url, error in
                    if once.claim() { group.leave() }
                    guard error == nil, let data = try? Data(contentsOf: url) else { return }
                    lock.lock()
                    received.append(Ingest.Candidate(data: data, name: url.lastPathComponent, url: nil))
                    lock.unlock()
                }
            }
            if !started, once.claim() { group.leave() }
        }
    }

    /// Waits off the main thread. Returns whatever arrived before `timeout`.
    func wait(timeout: TimeInterval = 12) async -> [Ingest.Candidate] {
        await withCheckedContinuation { continuation in
            DispatchQueue.global(qos: .userInitiated).async { [self] in
                let finished = group.wait(timeout: .now() + timeout) == .success
                lock.lock()
                let result = received
                received = []
                lock.unlock()
                if finished {
                    try? FileManager.default.removeItem(at: dir)
                } else {
                    // A late provider may still be writing; clean up after it.
                    DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + 60) { [dir] in
                        try? FileManager.default.removeItem(at: dir)
                    }
                }
                continuation.resume(returning: result)
            }
        }
    }
}

/// Writes vault images to the system clipboard so ⌘V works in every app.
enum Clipboard {
    /// Our own writes carry this type so the clipboard watcher ignores them.
    static let marker = NSPasteboard.PasteboardType("com.imagevault.internal")

    /// Plain text on the clipboard (used by Copy Text and Copy Path).
    static func copyText(_ text: String) {
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.setString(text, forType: .string)
    }

    /// An SVG image's source code as plain text (for pasting into code / Figma).
    static func copySVGCode(_ item: VaultItem) -> Bool {
        guard let data = try? Data(contentsOf: VaultStore.shared.fileURL(for: item)),
              let code = String(data: data, encoding: .utf8) else { return false }
        let pb = NSPasteboard.general
        pb.clearContents()
        // Text only: it shows up (with a preview) in the Clipboard tab and
        // isn't saved to the vault a second time.
        pb.setString(code, forType: .string)
        return true
    }

    static func copy(_ items: [VaultItem]) {
        let pb = NSPasteboard.general
        pb.clearContents()
        let store = VaultStore.shared
        if items.count == 1, let item = items.first {
            // One image: real image data (what chat apps, Figma, Photoshop paste).
            let url = store.fileURL(for: item)
            let entry = NSPasteboardItem()
            if let png = Imaging.pngData(at: url) { entry.setData(png, forType: .png) }
            if let tiff = NSImage(contentsOf: url)?.tiffRepresentation { entry.setData(tiff, forType: .tiff) }
            if item.mime == "image/jpeg", let jpg = try? Data(contentsOf: url) { entry.setData(jpg, forType: .init(UTType.jpeg.identifier)) }
            // Keep the animation / the vector for apps that take them.
            if item.mime == "image/gif", let gif = try? Data(contentsOf: url) { entry.setData(gif, forType: .init(UTType.gif.identifier)) }
            if item.mime == "image/svg+xml", let svg = try? Data(contentsOf: url) {
                entry.setData(svg, forType: .init(UTType.svg.identifier))
            }
            entry.setData(Data(), forType: marker)
            pb.writeObjects([entry])
        } else {
            // Several: real files (Finder, Slack, WhatsApp, Mail paste them all).
            let urls = items.map { store.fileURL(for: $0) as NSURL }
            pb.writeObjects(urls)
            pb.setData(Data(), forType: marker)
        }
    }
}
