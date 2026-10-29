import AppKit
import ImageIO
import UniformTypeIdentifiers

enum Imaging {
    struct Portable {
        var data: Data
        var mime: String
        var ext: String
        var width: Int?
        var height: Int?
    }

    enum Failure: Error { case notAnImage }

    /// Formats every app accepts on paste/drop are kept byte-for-byte; anything
    /// else (AVIF, WebP, HEIC, TIFF, BMP...) is re-encoded: JPEG when opaque, PNG otherwise.
    static func portable(_ data: Data) throws -> Portable {
        if isSVG(data) {
            let size = NSImage(data: data)?.size
            return Portable(data: data, mime: "image/svg+xml", ext: "svg",
                            width: size.map { Int($0.width) }, height: size.map { Int($0.height) })
        }
        guard let src = CGImageSourceCreateWithData(data as CFData, nil),
              let typeID = CGImageSourceGetType(src) as String?,
              let type = UTType(typeID), type.conforms(to: .image) else { throw Failure.notAnImage }

        let props = CGImageSourceCopyPropertiesAtIndex(src, 0, nil) as? [CFString: Any]
        let width = props?[kCGImagePropertyPixelWidth] as? Int
        let height = props?[kCGImagePropertyPixelHeight] as? Int

        if type == .png { return Portable(data: data, mime: "image/png", ext: "png", width: width, height: height) }
        if type == .jpeg { return Portable(data: data, mime: "image/jpeg", ext: "jpg", width: width, height: height) }
        if type == .gif { return Portable(data: data, mime: "image/gif", ext: "gif", width: width, height: height) }

        guard let cg = CGImageSourceCreateImageAtIndex(src, 0, [kCGImageSourceCreateThumbnailWithTransform: true] as CFDictionary)
        else { throw Failure.notAnImage }
        let opaque = [.none, .noneSkipFirst, .noneSkipLast].contains(cg.alphaInfo)
        let out = NSMutableData()
        let outType = opaque ? UTType.jpeg : UTType.png
        guard let dest = CGImageDestinationCreateWithData(out, outType.identifier as CFString, 1, nil) else { throw Failure.notAnImage }
        CGImageDestinationAddImage(dest, cg, opaque ? [kCGImageDestinationLossyCompressionQuality: 0.94] as CFDictionary : nil)
        guard CGImageDestinationFinalize(dest) else { throw Failure.notAnImage }
        return Portable(data: out as Data, mime: opaque ? "image/jpeg" : "image/png", ext: opaque ? "jpg" : "png",
                        width: cg.width, height: cg.height)
    }

    /// SVG markup, also with a BOM, XML prolog, DOCTYPE or leading comments.
    static func isSVG(_ data: Data) -> Bool {
        guard let head = String(decoding: data.prefix(4096), as: UTF8.self).lowercased()
            .trimmingCharacters(in: CharacterSet.whitespacesAndNewlines.union(CharacterSet(charactersIn: "\u{FEFF}"))) as String?,
              head.hasPrefix("<") else { return false }
        return head.hasPrefix("<svg") || ((head.hasPrefix("<?xml") || head.hasPrefix("<!")) && head.contains("<svg"))
    }

    static func isSVG(text: String) -> Bool {
        let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard t.count < 2_000_000, t.lowercased().contains("</svg>") || t.hasSuffix("/>") else { return false }
        return isSVG(Data(t.utf8))
    }

    /// True for GIFs with more than one frame.
    static func isAnimated(_ url: URL) -> Bool {
        guard url.pathExtension.lowercased() == "gif", let src = CGImageSourceCreateWithURL(url as CFURL, nil) else { return false }
        return CGImageSourceGetCount(src) > 1
    }

    /// Frames + per-frame delays for a live GIF preview, downsampled.
    static func gifFrames(at url: URL, maxPixel: Int) -> (frames: [CGImage], durations: [Double])? {
        guard let src = CGImageSourceCreateWithURL(url as CFURL, nil) else { return nil }
        let count = CGImageSourceGetCount(src)
        guard count > 1 else { return nil }
        let opts: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: maxPixel,
        ]
        var frames: [CGImage] = []
        var durations: [Double] = []
        for i in 0..<min(count, 400) {
            guard let frame = CGImageSourceCreateThumbnailAtIndex(src, i, opts as CFDictionary) else { continue }
            let props = CGImageSourceCopyPropertiesAtIndex(src, i, nil) as? [CFString: Any]
            let gif = props?[kCGImagePropertyGIFDictionary] as? [CFString: Any]
            var delay = (gif?[kCGImagePropertyGIFUnclampedDelayTime] as? Double) ?? 0
            if delay <= 0.011 { delay = (gif?[kCGImagePropertyGIFDelayTime] as? Double) ?? 0.1 }
            if delay <= 0.011 { delay = 0.1 } // browsers treat ~0 as 100ms
            frames.append(frame)
            durations.append(delay)
        }
        return frames.count > 1 ? (frames, durations) : nil
    }

    /// A looping keyframe animation that plays GIF frames on a CALayer.
    static func gifAnimation(_ frames: [CGImage], durations: [Double]) -> CAKeyframeAnimation {
        let total = durations.reduce(0, +)
        var t = 0.0
        var times: [NSNumber] = []
        for d in durations { times.append(NSNumber(value: t / total)); t += d }
        let anim = CAKeyframeAnimation(keyPath: "contents")
        anim.values = frames
        anim.keyTimes = times
        anim.calculationMode = .discrete
        anim.duration = total
        anim.repeatCount = .infinity
        anim.isRemovedOnCompletion = false
        return anim
    }

    /// Downsampled CGImage for grid tiles (fast: decodes only what's needed).
    static func thumbnail(at url: URL, maxPixel: Int) -> CGImage? {
        if url.pathExtension.lowercased() == "svg" {
            guard let image = NSImage(contentsOf: url) else { return nil }
            return rasterize(image, maxPixel: maxPixel)
        }
        guard let src = CGImageSourceCreateWithURL(url as CFURL, nil) else { return nil }
        let opts: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: maxPixel,
        ]
        return CGImageSourceCreateThumbnailAtIndex(src, 0, opts as CFDictionary)
    }

    /// Draws a vector image (SVG) at up to maxPixel on its long side, keeping its aspect.
    static func rasterize(_ image: NSImage, maxPixel: Int) -> CGImage? {
        let s = image.size
        guard s.width > 0, s.height > 0 else { return nil }
        let k = CGFloat(maxPixel) / max(s.width, s.height)
        let w = max(1, Int(s.width * k)), h = max(1, Int(s.height * k))
        guard let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8, bytesPerRow: 0,
                                  space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
        let previous = NSGraphicsContext.current
        NSGraphicsContext.current = NSGraphicsContext(cgContext: ctx, flipped: false)
        image.draw(in: NSRect(x: 0, y: 0, width: w, height: h))
        NSGraphicsContext.current = previous
        return ctx.makeImage()
    }

    /// PNG bytes for the clipboard (keeps PNG files untouched).
    static func pngData(at url: URL) -> Data? {
        if url.pathExtension.lowercased() == "png" { return try? Data(contentsOf: url) }
        if url.pathExtension.lowercased() == "svg", let image = NSImage(contentsOf: url),
           let cg = rasterize(image, maxPixel: 2048) {
            return NSBitmapImageRep(cgImage: cg).representation(using: .png, properties: [:])
        }
        guard let image = NSImage(contentsOf: url), let tiff = image.tiffRepresentation,
              let rep = NSBitmapImageRep(data: tiff) else { return nil }
        return rep.representation(using: .png, properties: [:])
    }
}

/// Shared thumbnail cache: memory first, then small files in `thumbs/`, then
/// the original. Decoding runs off the main thread.
///
/// Grid tiles are ~96 pt, so 256 px covers them on Retina; decoding big
/// screenshots at that size (not 420 px) needs ~2.7× less memory per tile.
/// The memory cache is capped by bytes, not count. The disk copies make the
/// tray open instantly after a relaunch instead of decoding every original.
final class Thumbnails: @unchecked Sendable {
    static let shared = Thumbnails()
    static let gridPixel = 256
    private let cache = NSCache<NSString, CGImage>()
    private let queue = DispatchQueue(label: "imagevault.thumbs", qos: .userInitiated, attributes: .concurrent)
    private var dir: URL { VaultStore.shared.thumbsURL }

    init() {
        cache.totalCostLimit = 96 * 1024 * 1024
        cache.countLimit = 1000
    }

    /// Drops cached thumbnails of an image whose pixels changed or that's gone.
    func forget(_ id: String) {
        for size in [40, 88, 160, Self.gridPixel] { cache.removeObject(forKey: "\(id)@\(size)" as NSString) }
        let prefix = id + "@"
        for name in (try? FileManager.default.contentsOfDirectory(atPath: dir.path)) ?? [] where name.hasPrefix(prefix) {
            try? FileManager.default.removeItem(at: dir.appendingPathComponent(name))
        }
    }

    func load(_ item: VaultItem, maxPixel: Int = Thumbnails.gridPixel, completion: @escaping (CGImage?) -> Void) {
        let key = "\(item.id)@\(maxPixel)" as NSString
        if let hit = cache.object(forKey: key) { return completion(hit) }
        let url = VaultStore.shared.fileURL(for: item)
        // The content hash in the name means an edited image never shows a stale thumbnail.
        let file = dir.appendingPathComponent("\(item.id)@\(maxPixel)-\(item.hash?.prefix(8) ?? "x")")
        queue.async { [cache] in
            var image = Self.readCached(file)
            if image == nil {
                image = Imaging.thumbnail(at: url, maxPixel: maxPixel)
                if let image { Self.writeCached(image, to: file) }
            }
            if let image { cache.setObject(image, forKey: key, cost: image.bytesPerRow * image.height) }
            DispatchQueue.main.async { completion(image) }
        }
    }

    private static func readCached(_ base: URL) -> CGImage? {
        for ext in ["jpg", "png"] {
            let url = base.appendingPathExtension(ext)
            guard let src = CGImageSourceCreateWithURL(url as CFURL, nil),
                  let image = CGImageSourceCreateImageAtIndex(src, 0, [kCGImageSourceShouldCacheImmediately: true] as CFDictionary)
            else { continue }
            return image
        }
        return nil
    }

    /// JPEG when the thumbnail has no transparency (small), PNG otherwise.
    private static func writeCached(_ image: CGImage, to base: URL) {
        let opaque = [.none, .noneSkipFirst, .noneSkipLast].contains(image.alphaInfo)
        let url = base.appendingPathExtension(opaque ? "jpg" : "png")
        let type = (opaque ? UTType.jpeg : UTType.png).identifier as CFString
        guard let dest = CGImageDestinationCreateWithURL(url as CFURL, type, 1, nil) else { return }
        CGImageDestinationAddImage(dest, image, [kCGImageDestinationLossyCompressionQuality: 0.85] as CFDictionary)
        CGImageDestinationFinalize(dest)
    }
}
