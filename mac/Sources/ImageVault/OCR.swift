import AppKit
import ImageIO
import Vision

/// On-device text recognition for vault images.
///
/// Built for screenshots: latest Vision revision, accurate recognition,
/// automatic language detection, full-resolution originals with EXIF
/// orientation, and a layout pass that keeps reading order, column gaps and
/// paragraph breaks. If the first pass finds almost nothing (tiny text), it
/// retries on a 2x upscale and keeps whichever pass read more.
enum OCR {
    private struct Row {
        var box: CGRect
        var parts: [(box: CGRect, text: String, confidence: Float)]
    }

    /// Synchronous — always run this off the main thread.
    /// Vision covers ~30 languages; scripts it can't read (Bangla, Hindi, Tamil…)
    /// go through the bundled Tesseract. Whichever engine actually read the
    /// non-Latin script wins.
    static func recognize(at url: URL) -> String? {
        let vision = recognizeWithVision(at: url)
        guard let extra = Tesseract.recognize(at: url), Tesseract.hasExtraScript(extra) else { return vision }
        return extra
    }

    /// Text for the search index: both engines, so either one can match.
    static func indexText(at url: URL) -> String {
        let vision = recognizeWithVision(at: url) ?? ""
        let extra = Tesseract.recognize(at: url) ?? ""
        if extra.isEmpty || !Tesseract.hasExtraScript(extra) { return vision }
        if vision.isEmpty { return extra }
        return vision + "\n" + extra
    }

    static func recognizeWithVision(at url: URL) -> String? {
        guard let source = load(url) else { return nil }
        let languages = preferredLanguages()
        let first = run(image: source.image, orientation: source.orientation, languages: languages)
        let firstText = first?.text ?? ""
        let firstScore = score(first)

        let needsRetry = firstText.count < 6 || (first?.confidence ?? 0) < 0.4
        let smallEnough = max(source.image.width, source.image.height) <= 1800
        if needsRetry, smallEnough, let upscaled = resized(source.image, scale: 2, maxDimension: 3600) {
            let second = run(image: upscaled, orientation: source.orientation, languages: languages)
            if score(second) > firstScore + 2 { return second?.text }
        }
        return firstText.isEmpty ? nil : firstText
    }

    /// Async convenience for UI code.
    static func text(at url: URL) async -> String? {
        await Task.detached(priority: .userInitiated) {
            recognize(at: url)
        }.value
    }

    // MARK: Vision

    private static func run(image: CGImage, orientation: CGImagePropertyOrientation,
                            languages: [String]) -> (text: String, confidence: Float)? {
        let request = VNRecognizeTextRequest()
        request.recognitionLevel = .accurate
        request.usesLanguageCorrection = true
        request.revision = VNRecognizeTextRequestRevision3
        request.automaticallyDetectsLanguage = true
        request.recognitionLanguages = languages
        request.minimumTextHeight = 0

        let handler = VNImageRequestHandler(cgImage: image, orientation: orientation, options: [:])
        do {
            try handler.perform([request])
        } catch {
            return nil
        }
        guard let observations = request.results, !observations.isEmpty else { return nil }
        return layout(observations)
    }

    /// Vision hands back lines in no useful order. Rebuild rows (top→bottom,
    /// left→right), use tabs for wide column gaps and blank lines for
    /// paragraph-sized vertical gaps.
    private static func layout(_ observations: [VNRecognizedTextObservation]) -> (text: String, confidence: Float)? {
        var rows: [Row] = []
        let sorted = observations.sorted { $0.boundingBox.maxY > $1.boundingBox.maxY }

        for observation in sorted {
            guard let candidate = observation.topCandidates(1).first,
                  !candidate.string.trimmingCharacters(in: .whitespaces).isEmpty else { continue }
            let box = observation.boundingBox
            var matched: Int?
            for index in stride(from: rows.count - 1, through: 0, by: -1) {
                let existing = rows[index].box
                let overlap = min(existing.maxY, box.maxY) - max(existing.minY, box.minY)
                if overlap > min(existing.height, box.height) * 0.45 {
                    matched = index
                    break
                }
            }
            if let index = matched {
                rows[index].parts.append((box, candidate.string, candidate.confidence))
                rows[index].box = rows[index].box.union(box)
            } else {
                rows.append(Row(box: box, parts: [(box, candidate.string, candidate.confidence)]))
            }
        }
        guard !rows.isEmpty else { return nil }
        rows.sort { $0.box.maxY > $1.box.maxY }

        var lines: [String] = []
        var previous: CGRect?
        var totalConfidence: Float = 0
        var pieces = 0

        for row in rows {
            if let last = previous {
                let gap = last.minY - row.box.maxY
                let lineHeight = max(row.box.height, last.height)
                if gap > max(0.02, lineHeight * 1.15) { lines.append("") }
            }
            let parts = row.parts.sorted { $0.box.minX < $1.box.minX }
            var line = ""
            var lastMaxX: CGFloat?
            for part in parts {
                if let trailing = lastMaxX {
                    let gap = part.box.minX - trailing
                    let threshold = max(0.012, row.box.height * 0.7)
                    line += gap > threshold ? "\t" : " "
                }
                line += part.text
                lastMaxX = part.box.maxX
                totalConfidence += part.confidence
                pieces += 1
            }
            let cleaned = line.trimmingCharacters(in: .whitespaces)
            if !cleaned.isEmpty { lines.append(cleaned) }
            previous = row.box
        }

        // Collapse runs of blank lines and trim the ends.
        var compact: [String] = []
        for line in lines {
            if line.isEmpty, compact.last?.isEmpty == true { continue }
            compact.append(line)
        }
        while compact.first?.isEmpty == true { compact.removeFirst() }
        while compact.last?.isEmpty == true { compact.removeLast() }
        let text = compact.joined(separator: "\n")
        guard !text.isEmpty else { return nil }
        return (text, pieces > 0 ? totalConfidence / Float(pieces) : 0)
    }

    // MARK: Image loading

    private static func load(_ url: URL) -> (image: CGImage, orientation: CGImagePropertyOrientation)? {
        if url.pathExtension.lowercased() == "svg" {
            guard let nsImage = NSImage(contentsOf: url) else { return nil }
            var rect = NSRect(x: 0, y: 0, width: max(nsImage.size.width, 512) * 2,
                              height: max(nsImage.size.height, 512) * 2)
            guard let image = nsImage.cgImage(forProposedRect: &rect, context: nil, hints: nil) else { return nil }
            return (image, .up)
        }
        guard let source = CGImageSourceCreateWithURL(url as CFURL, nil),
              let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else { return nil }
        var orientation: CGImagePropertyOrientation = .up
        if let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any],
           let raw = properties[kCGImagePropertyOrientation] as? UInt32,
           let parsed = CGImagePropertyOrientation(rawValue: raw) {
            orientation = parsed
        }
        return (image, orientation)
    }

    private static func resized(_ image: CGImage, scale: CGFloat, maxDimension: CGFloat) -> CGImage? {
        let width = CGFloat(image.width) * scale
        let height = CGFloat(image.height) * scale
        guard width <= maxDimension, height <= maxDimension else { return nil }
        guard let context = CGContext(data: nil, width: Int(width), height: Int(height),
                                      bitsPerComponent: 8, bytesPerRow: 0,
                                      space: CGColorSpaceCreateDeviceRGB(),
                                      bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
        context.interpolationQuality = .high
        context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
        return context.makeImage()
    }

    // MARK: Languages

    private static func preferredLanguages() -> [String] {
        let probe = VNRecognizeTextRequest()
        probe.recognitionLevel = .accurate
        probe.revision = VNRecognizeTextRequestRevision3
        let supported = (try? probe.supportedRecognitionLanguages()) ?? ["en-US"]
        var result: [String] = []
        for preferred in Locale.preferredLanguages {
            let code = Locale(identifier: preferred).language.languageCode?.identifier ?? "en"
            if let match = supported.first(where: { $0 == preferred || $0.hasPrefix(code + "-") }),
               !result.contains(match) {
                result.append(match)
            }
        }
        if !result.contains("en-US") { result.append("en-US") }
        return Array(result.prefix(4))
    }

    private static func score(_ result: (text: String, confidence: Float)?) -> Float {
        guard let result else { return 0 }
        return Float(result.text.count) + result.confidence * 40
    }
}

/// Bundled Tesseract (Contents/Resources/tesseract) for scripts Apple Vision
/// doesn't read. Languages are chosen in Settings → OCR Languages.
enum Tesseract {
    struct Language: Hashable {
        let code: String
        let name: String
    }

    static let languages: [Language] = [
        .init(code: "ben", name: "Bangla"), .init(code: "hin", name: "Hindi"), .init(code: "urd", name: "Urdu"),
        .init(code: "tam", name: "Tamil"), .init(code: "tel", name: "Telugu"), .init(code: "mar", name: "Marathi"),
        .init(code: "nep", name: "Nepali"), .init(code: "guj", name: "Gujarati"), .init(code: "pan", name: "Punjabi"),
        .init(code: "mal", name: "Malayalam"), .init(code: "kan", name: "Kannada"), .init(code: "sin", name: "Sinhala"),
    ]

    /// Enabled extra languages (default: Bangla).
    static var enabled: [String] {
        get { UserDefaults.standard.stringArray(forKey: "ocrExtraLanguages") ?? ["ben"] }
        set { UserDefaults.standard.set(newValue, forKey: "ocrExtraLanguages") }
    }

    private static var root: URL? {
        guard let res = Bundle.main.resourceURL?.appendingPathComponent("tesseract"),
              FileManager.default.isExecutableFile(atPath: res.appendingPathComponent("bin/tesseract").path) else { return nil }
        return res
    }

    static var available: Bool { root != nil }

    /// Runs `tesseract <image> stdout -l <langs>+eng`. Off the main thread only.
    static func recognize(at url: URL) -> String? {
        let langs = enabled.filter { code in languages.contains { $0.code == code } }
        guard !langs.isEmpty, let root else { return nil }

        // Tesseract can't read SVG; rasterise it first.
        var input = url
        var temp: URL?
        if url.pathExtension.lowercased() == "svg" {
            guard let image = NSImage(contentsOf: url), let tiff = image.tiffRepresentation,
                  let png = NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:]) else { return nil }
            let t = FileManager.default.temporaryDirectory.appendingPathComponent("iv-ocr-\(UUID().uuidString).png")
            guard (try? png.write(to: t)) != nil else { return nil }
            input = t
            temp = t
        }
        defer { if let temp { try? FileManager.default.removeItem(at: temp) } }

        let process = Process()
        process.executableURL = root.appendingPathComponent("bin/tesseract")
        process.arguments = [input.path, "stdout", "-l", (langs + ["eng"]).joined(separator: "+"), "--psm", "3"]
        process.environment = ["TESSDATA_PREFIX": root.appendingPathComponent("tessdata").path, "OMP_THREAD_LIMIT": "2"]
        let out = Pipe()
        process.standardOutput = out
        process.standardError = FileHandle.nullDevice
        do { try process.run() } catch { return nil }
        let watchdog = DispatchWorkItem { if process.isRunning { process.terminate() } }
        DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + 30, execute: watchdog)
        let data = out.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        watchdog.cancel()
        let text = String(decoding: data, as: UTF8.self)
            .replacingOccurrences(of: "\u{0C}", with: "")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return text.isEmpty ? nil : text
    }

    /// True when the text has a meaningful amount of a script Vision can't read
    /// (Bengali, Devanagari, Gurmukhi, Gujarati, Tamil, Telugu, Kannada,
    /// Malayalam, Sinhala, or Urdu's Arabic letters when Urdu is enabled).
    static func hasExtraScript(_ text: String) -> Bool {
        var extra = 0
        var letters = 0
        let urdu = enabled.contains("urd")
        for scalar in text.unicodeScalars where CharacterSet.letters.contains(scalar) {
            letters += 1
            let v = scalar.value
            if (0x0900...0x0DFF).contains(v) || (urdu && (0x0600...0x06FF).contains(v)) { extra += 1 }
        }
        return extra >= 3 && Double(extra) >= Double(letters) * 0.12
    }
}

/// Reads the text of every image in the background — new ones as they arrive,
/// old ones once — so search can find "siam" inside a screenshot without the
/// user ever pressing Copy Text.
@MainActor
final class OCRIndexer {
    static let shared = OCRIndexer()
    /// Bump when the pipeline changes so old results get refreshed.
    static let version = 2

    private var running = false
    private var languageKey: String { Tesseract.enabled.sorted().joined(separator: "+") }

    func start() {
        NotificationCenter.default.addObserver(forName: .vaultChanged, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.pump() }
        }
        pump()
    }

    /// Languages changed: the language key no longer matches, so every image
    /// gets re-read with the new set.
    func reindexAll() { pump() }

    /// Reads and saves one image at a time, entirely off the main thread: the
    /// result is one SQLite row update, and the grid doesn't reload for it.
    private func pump() {
        guard !running else { return }
        running = true
        let key = languageKey
        let version = Self.version
        Task.detached(priority: .utility) {
            let store = VaultStore.shared
            if let next = store.nextPendingOCR(version: version, languages: key) {
                let url = store.fileURL(for: next)
                let text = FileManager.default.fileExists(atPath: url.path) ? OCR.indexText(at: url) : ""
                store.setOCR(id: next.id, text: text.isEmpty ? nil : text, version: version, languages: key)
                await MainActor.run {
                    self.running = false
                    // Give the machine a breath between images.
                    DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) { self.pump() }
                }
            } else {
                await MainActor.run { self.running = false }
            }
        }
    }
}
