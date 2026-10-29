import AppKit
import CoreImage
import ImageIO

// MARK: - Document

/// One screenshot: the captured pixels plus the annotations drawn on it.
/// Everything is laid out in image points (origin bottom-left); `scale` is
/// pixels per point, so exports keep the full Retina resolution.
@MainActor
final class CaptureDocument {
    let base: CGImage
    let scale: CGFloat
    let created = Date()
    var annotations: [Annotation] = [] {
        didSet { renderedCache = nil; changed() }
    }
    var onChange: [() -> Void] = []
    /// Set when editing an image that's already in the vault.
    var sourceItem: VaultItem?

    private var renderedCache: CGImage?
    private var blurredCache: CGImage?
    private var pixels: [UInt8]?

    init(image: CGImage, scale: CGFloat) {
        base = image
        self.scale = max(1, scale)
    }

    var size: CGSize { CGSize(width: CGFloat(base.width) / scale, height: CGFloat(base.height) / scale) }

    var name: String {
        if let source = sourceItem { return (source.name as NSString).deletingPathExtension + ".png" }
        let f = DateFormatter()
        f.dateFormat = "yyyy-MM-dd 'at' HH.mm.ss"
        return "Screenshot \(f.string(from: created)).png"
    }

    private func changed() { onChange.forEach { $0() } }

    /// Heavily blurred copy of the capture, used by Blur/Erase regions.
    var blurred: CGImage {
        if let blurredCache { return blurredCache }
        let input = CIImage(cgImage: base)
        let radius = 14 * scale
        let output = input.clampedToExtent()
            .applyingFilter("CIPixellate", parameters: [kCIInputScaleKey: 10 * scale])
            .applyingFilter("CIGaussianBlur", parameters: [kCIInputRadiusKey: radius])
            .cropped(to: input.extent)
        let image = Self.ci.createCGImage(output, from: input.extent) ?? base
        blurredCache = image
        return image
    }

    /// The capture with every annotation flattened in, at full resolution.
    var rendered: CGImage {
        if let renderedCache { return renderedCache }
        let image = render(annotations) ?? base
        renderedCache = image
        return image
    }

    func render(_ items: [Annotation]) -> CGImage? {
        guard !items.isEmpty else { return base }
        // Same size and colour space as the capture, so pixels map 1:1.
        let space = base.colorSpace.flatMap { $0.model == .rgb ? $0 : nil } ?? CGColorSpace(name: CGColorSpace.sRGB)!
        guard let ctx = CGContext(data: nil, width: base.width, height: base.height, bitsPerComponent: 8, bytesPerRow: 0,
                                  space: space, bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue
                                    | CGBitmapInfo.byteOrder32Little.rawValue) else { return nil }
        ctx.scaleBy(x: scale, y: scale)
        ctx.interpolationQuality = .high
        let previous = NSGraphicsContext.current
        NSGraphicsContext.current = NSGraphicsContext(cgContext: ctx, flipped: false)
        AnnotationRenderer.draw(document: self, annotations: items, in: ctx)
        NSGraphicsContext.current = previous
        return ctx.makeImage()
    }

    /// Full-resolution PNG. The point size is recorded (144 dpi for Retina,
    /// like macOS screenshots) so apps show it at the size seen on screen.
    func pngData() -> Data? { Self.png(rendered, pointSize: size) }

    static func png(_ image: CGImage, pointSize: CGSize) -> Data? {
        let rep = NSBitmapImageRep(cgImage: image)
        rep.size = pointSize
        return rep.representation(using: .png, properties: [:])
    }

    /// A real file for drag-out and OCR, named like a macOS screenshot.
    func writeTemporaryFile() -> URL? {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("ImageVault-captures", isDirectory: true)
            .appendingPathComponent(UUID().uuidString, isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let url = dir.appendingPathComponent(name)
        guard let png = pngData(), (try? png.write(to: url)) != nil else { return nil }
        return url
    }

    /// sRGB colour of the original capture at a point (image points).
    func color(at p: CGPoint) -> NSColor? {
        let w = base.width, h = base.height
        if pixels == nil {
            var buffer = [UInt8](repeating: 0, count: w * h * 4)
            let space = CGColorSpace(name: CGColorSpace.sRGB)!
            let ok = buffer.withUnsafeMutableBytes { raw -> Bool in
                guard let ctx = CGContext(data: raw.baseAddress, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
                                          space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return false }
                ctx.draw(base, in: CGRect(x: 0, y: 0, width: w, height: h))
                return true
            }
            guard ok else { return nil }
            pixels = buffer
        }
        let x = Int(p.x * scale), y = h - 1 - Int(p.y * scale)
        guard x >= 0, y >= 0, x < w, y < h, let pixels else { return nil }
        let i = (y * w + x) * 4
        return NSColor(srgbRed: CGFloat(pixels[i]) / 255, green: CGFloat(pixels[i + 1]) / 255,
                       blue: CGFloat(pixels[i + 2]) / 255, alpha: 1)
    }

    static let ci = CIContext(options: [.useSoftwareRenderer: false])
}

extension NSColor {
    var hexString: String {
        let c = usingColorSpace(.sRGB) ?? self
        return String(format: "#%02X%02X%02X", Int(round(c.redComponent * 255)), Int(round(c.greenComponent * 255)),
                      Int(round(c.blueComponent * 255)))
    }
}

extension Clipboard {
    /// A rendered capture as real image data (PNG + TIFF), marked as ours so
    /// "Save Copied Images" doesn't store it a second time.
    static func copy(image: CGImage, pointSize: CGSize) {
        let entry = NSPasteboardItem()
        let rep = NSBitmapImageRep(cgImage: image)
        rep.size = pointSize
        if let png = rep.representation(using: .png, properties: [:]) { entry.setData(png, forType: .png) }
        if let tiff = rep.tiffRepresentation { entry.setData(tiff, forType: .tiff) }
        entry.setData(Data(), forType: marker)
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.writeObjects([entry])
    }
}

// MARK: - Actions shared by the preview, editor and pin

@MainActor
enum CaptureActions {
    /// Copy also keeps the shot in the vault (copied images always live there).
    static func copy(_ doc: CaptureDocument) {
        Clipboard.copy(image: doc.rendered, pointSize: doc.size)
        saveToVault(doc, title: "Copied")
    }

    /// A fresh capture goes into the vault; later edits of it then update
    /// that same item instead of adding copies.
    static func autoSave(_ doc: CaptureDocument) {
        guard let png = doc.pngData() else { return }
        let name = doc.name
        Task { @MainActor in
            let summary = await Ingest.saveAsync([Ingest.Candidate(data: png, name: name, url: nil)], source: "screenshot")
            if let item = summary.added.first ?? summary.duplicates.first, doc.sourceItem == nil { doc.sourceItem = item }
            IslandController.shared?.showActivity(image: summary.firstImage,
                                                  title: summary.isEmpty ? "Copied" : "Copied & saved",
                                                  detail: summary.isEmpty ? "Couldn't save to vault" : "\(VaultModel.shared.items.count) in vault")
        }
    }

    static func saveToVault(_ doc: CaptureDocument, title: String? = nil) {
        guard let png = doc.pngData() else { return }
        // Editing a vault image saves over it; GIFs and SVGs get a new copy so
        // the animation / vector original isn't flattened away.
        if let source = doc.sourceItem, source.mime != "image/gif", source.mime != "image/svg+xml" {
            Task { @MainActor in
                let updated = await Task.detached(priority: .userInitiated) {
                    try? VaultStore.shared.replaceData(of: source.id, with: png)
                }.value
                guard let updated else {
                    IslandController.shared?.showActivity(image: nil, title: "Save failed", detail: "")
                    return
                }
                doc.sourceItem = updated
                Thumbnails.shared.forget(updated.id)
                VaultModel.shared.reload()
                let thumb = Imaging.thumbnail(at: VaultStore.shared.fileURL(for: updated), maxPixel: 88)
                    .map { NSImage(cgImage: $0, size: NSSize(width: $0.width, height: $0.height)) }
                IslandController.shared?.showActivity(image: thumb, title: title ?? "Saved", detail: "Updated in vault")
            }
            return
        }
        let name = doc.name
        Task { @MainActor in
            let summary = await Ingest.saveAsync([Ingest.Candidate(data: png, name: name, url: nil)], source: "screenshot")
            let n = summary.added.count
            let saved = n == 0 ? (summary.isEmpty ? "Save failed" : "Already saved") : "Saved"
            IslandController.shared?.showActivity(image: summary.firstImage,
                                                  title: title ?? saved,
                                                  detail: title == nil ? "\(VaultModel.shared.items.count) in vault"
                                                                       : (summary.isEmpty ? "Couldn't save to vault" : "Saved to vault"))
        }
    }

    static func saveAs(_ doc: CaptureDocument, from window: NSWindow?) {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = doc.name
        panel.allowedContentTypes = [.png]
        panel.canCreateDirectories = true
        NSApp.activate(ignoringOtherApps: true)
        let write: (NSApplication.ModalResponse) -> Void = { response in
            guard response == .OK, let url = panel.url, let png = doc.pngData() else { return }
            do { try png.write(to: url) } catch { NSSound.beep() }
        }
        if let window { panel.beginSheetModal(for: window, completionHandler: write) }
        else { write(panel.runModal()) }
    }

    static func copyText(_ doc: CaptureDocument) {
        guard let url = doc.writeTemporaryFile() else { return }
        let thumb = NSImage(cgImage: doc.rendered, size: doc.size)
        IslandController.shared?.showActivity(image: thumb, title: "Reading text…", detail: "")
        Task { @MainActor in
            let text = await Task.detached(priority: .userInitiated) { OCR.recognize(at: url) }.value
            try? FileManager.default.removeItem(at: url.deletingLastPathComponent())
            if let text, !text.isEmpty {
                Clipboard.copyText(text)
                IslandController.shared?.showActivity(image: thumb, title: "Text copied", detail: "\(text.count) characters")
            } else {
                IslandController.shared?.showActivity(image: thumb, title: "No text found", detail: "")
            }
        }
    }

    static func pin(_ doc: CaptureDocument, near frame: NSRect? = nil) {
        PinWindow.show(doc, near: frame)
    }

    static func edit(_ doc: CaptureDocument) {
        EditorWindowController.open(doc)
    }
}

/// Opens any vault image in the screenshot editor.
@MainActor
enum ImageEditing {
    static func open(_ item: VaultItem) {
        let url = VaultStore.shared.fileURL(for: item)
        let image: CGImage?
        var scale: CGFloat = 1
        if item.mime == "image/svg+xml" || url.pathExtension.lowercased() == "svg" {
            // Vectors: rasterise at 2× for a crisp canvas.
            let ns = NSImage(contentsOf: url)
            let side = Int(max(ns?.size.width ?? 512, ns?.size.height ?? 512) * 2)
            image = ns.flatMap { Imaging.rasterize($0, maxPixel: max(256, min(side, 4096))) }
            scale = 2
        } else if let src = CGImageSourceCreateWithURL(url as CFURL, nil) {
            image = CGImageSourceCreateImageAtIndex(src, 0, nil)
            // Respect the image's DPI (Retina screenshots are 144 dpi).
            let props = CGImageSourceCopyPropertiesAtIndex(src, 0, nil) as? [CFString: Any]
            if let dpi = props?[kCGImagePropertyDPIWidth] as? Double, dpi > 72 { scale = CGFloat(dpi / 72) }
        } else {
            image = nil
        }
        guard let image else {
            IslandController.shared?.showActivity(image: nil, title: "Can't edit this image", detail: "")
            return
        }
        let doc = CaptureDocument(image: image, scale: scale)
        doc.sourceItem = item
        EditorWindowController.open(doc)
    }
}

// MARK: - Center

/// Takes screenshots and keeps the stack of floating previews.
@MainActor
final class CaptureCenter {
    static let shared = CaptureCenter()
    private var previews: [CapturePreview] = []

    func takeScreenshot() {
        ScreenOCRController.shared.toggleScreenshot { [weak self] image, screen in
            let scale = screen.backingScaleFactor
            self?.present(CaptureDocument(image: image, scale: scale), on: screen)
        }
    }

    /// How long the card stays (seconds); 0 = until closed.
    static var previewSeconds: Int {
        get { UserDefaults.standard.object(forKey: "shotPreviewSeconds") as? Int ?? 5 }
        set { UserDefaults.standard.set(newValue, forKey: "shotPreviewSeconds") }
    }

    func present(_ doc: CaptureDocument, on screen: NSScreen?) {
        // Every new screenshot lands on the clipboard and in the vault straight away.
        Clipboard.copy(image: doc.rendered, pointSize: doc.size)
        CaptureActions.autoSave(doc)
        let preview = CapturePreview(document: doc, screen: screen ?? NSScreen.main ?? NSScreen.screens[0])
        preview.onClose = { [weak self, weak preview] in
            guard let self, let preview else { return }
            self.previews.removeAll { $0 === preview }
            self.relayout(animated: true)
        }
        previews.append(preview)
        relayout(animated: false)
        preview.appear()
    }

    /// Newest at the bottom-right corner, older ones stacked above it.
    private func relayout(animated: Bool) {
        var offsets: [ObjectIdentifier: CGFloat] = [:]
        for preview in previews.reversed() {
            let key = ObjectIdentifier(preview.homeScreen)
            let y = offsets[key] ?? 0
            preview.place(stackOffset: y, animated: animated)
            offsets[key] = y + preview.frame.height + 10
        }
    }
}

// MARK: - Floating preview

/// The small window that appears after a capture: close and drag handle on top,
/// the screenshot, then Edit · Copy · Save · Copy Text · Pin.
@MainActor
final class CapturePreview: NSPanel {
    let document: CaptureDocument
    let homeScreen: NSScreen
    var onClose: (() -> Void)?
    private let imageView = PreviewImageView()
    private var buttons: [HUDButton] = []
    private static let width: CGFloat = 220

    init(document: CaptureDocument, screen: NSScreen) {
        self.document = document
        self.homeScreen = screen
        let s = document.size
        let imageW = Self.width - 16
        let imageH = min(150, max(60, imageW * s.height / max(1, s.width)))
        let height = 26 + imageH + 8 + 34
        super.init(contentRect: NSRect(x: 0, y: 0, width: Self.width, height: height),
                   styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        isFloatingPanel = true
        level = .floating
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        backgroundColor = .clear
        isOpaque = false
        hasShadow = true
        hidesOnDeactivate = false
        isReleasedWhenClosed = false
        animationBehavior = .none

        let root = HoverView(frame: NSRect(x: 0, y: 0, width: Self.width, height: height))
        root.onHover = { [weak self] on in self?.setHovering(on) }
        root.wantsLayer = true
        root.layer?.backgroundColor = NSColor(white: 0.11, alpha: 0.97).cgColor
        root.layer?.cornerRadius = 12
        root.layer?.cornerCurve = .continuous
        root.layer?.borderColor = NSColor(white: 1, alpha: 0.16).cgColor
        root.layer?.borderWidth = 0.5
        contentView = root

        // Top bar: close (left), drag handle (centre).
        let close = HUDButton(symbol: "xmark", size: 9.5, help: "Close") { [weak self] in self?.dismiss() }
        close.frame = NSRect(x: 6, y: height - 24, width: 22, height: 22)
        root.addSubview(close)
        let grip = DragGrip(frame: NSRect(x: Self.width / 2 - 20, y: height - 24, width: 40, height: 22))
        grip.document = document
        root.addSubview(grip)

        imageView.frame = NSRect(x: 8, y: 38, width: imageW, height: imageH)
        imageView.document = document
        imageView.onDoubleClick = { [weak self] in self?.edit() }
        root.addSubview(imageView)

        // Bottom bar.
        let actions: [(String, String, () -> Void)] = [
            ("pencil", "Edit", { [weak self] in self?.edit() }),
            ("doc.on.doc", "Copy  ⌘C", { [weak self] in self?.copy() }),
            ("square.and.arrow.down", "Save to Vault", { [weak self] in self?.save() }),
            ("text.viewfinder", "Copy Text", { [weak self] in self?.copyText() }),
            ("pin", "Pin to Screen", { [weak self] in self?.pin() }),
        ]
        let slot = (Self.width - 16) / CGFloat(actions.count)
        for (i, a) in actions.enumerated() {
            let b = HUDButton(symbol: a.0, size: 12, help: a.1, action: a.2)
            b.frame = NSRect(x: 8 + slot * CGFloat(i) + (slot - 28) / 2, y: 5, width: 28, height: 26)
            root.addSubview(b)
            buttons.append(b)
        }

        document.onChange.append { [weak self] in self?.imageView.refreshSoon() }
    }

    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }

    func place(stackOffset: CGFloat, animated: Bool) {
        let v = homeScreen.visibleFrame
        let target = NSRect(x: v.maxX - frame.width - 16, y: v.minY + 16 + stackOffset, width: frame.width, height: frame.height)
        if animated && isVisible {
            NSAnimationContext.runAnimationGroup { ctx in
                ctx.duration = 0.25
                ctx.timingFunction = Theme.ease
                animator().setFrame(target, display: true)
            }
        } else {
            setFrame(target, display: false)
        }
    }

    private var dismissWork: DispatchWorkItem?
    private var hovering = false
    private var closing = false

    /// Starts (or restarts) the auto-hide countdown; paused while hovered.
    private func scheduleDismiss() {
        dismissWork?.cancel()
        let seconds = CaptureCenter.previewSeconds
        guard seconds > 0, !hovering else { return }
        let work = DispatchWorkItem { [weak self] in self?.dismiss() }
        dismissWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + .seconds(seconds), execute: work)
    }

    func setHovering(_ on: Bool) {
        hovering = on
        if on { dismissWork?.cancel() } else { scheduleDismiss() }
    }

    /// Any finished interaction (copy, save, text, drag, edit) closes the card.
    func finishInteraction(after delay: TimeInterval = 0.55) {
        dismissWork?.cancel()
        DispatchQueue.main.asyncAfter(deadline: .now() + delay) { [weak self] in self?.dismiss() }
    }

    func appear() {
        scheduleDismiss()
        let target = frame
        setFrame(target.offsetBy(dx: 24, dy: 0), display: false)
        alphaValue = 0
        orderFrontRegardless()
        NSAnimationContext.runAnimationGroup { ctx in
            ctx.duration = 0.28
            ctx.timingFunction = Theme.ease
            animator().setFrame(target, display: true)
            animator().alphaValue = 1
        }
    }

    func dismiss() {
        guard !closing else { return }
        closing = true
        dismissWork?.cancel()
        NSAnimationContext.runAnimationGroup({ ctx in
            ctx.duration = 0.18
            animator().alphaValue = 0
        }, completionHandler: { [weak self] in
            MainActor.assumeIsolated {
                self?.orderOut(nil)
                self?.onClose?()
            }
        })
    }

    private func edit() {
        CaptureActions.edit(document)
        finishInteraction(after: 0.1)
    }

    private func copy() {
        CaptureActions.copy(document)
        buttons[1].confirm()
        finishInteraction()
    }

    private func save() {
        CaptureActions.saveToVault(document)
        buttons[2].confirm()
        finishInteraction()
    }

    private func copyText() {
        CaptureActions.copyText(document)
        finishInteraction(after: 0.3)
    }

    private func pin() {
        CaptureActions.pin(document, near: nil)
        dismiss()
    }
}

/// Thumbnail of the capture; drags out as a real PNG file, double-click edits.
@MainActor
final class PreviewImageView: NSView, NSDraggingSource {
    weak var document: CaptureDocument? { didSet { refresh() } }
    var onDoubleClick: (() -> Void)?
    private let imageLayer = CALayer()
    private var pending = false
    private var downPoint: NSPoint?

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.cornerRadius = 7
        layer?.cornerCurve = .continuous
        layer?.masksToBounds = true
        layer?.backgroundColor = NSColor(white: 0, alpha: 0.5).cgColor
        layer?.borderColor = NSColor(white: 1, alpha: 0.12).cgColor
        layer?.borderWidth = 0.5
        imageLayer.contentsGravity = .resizeAspect
        layer?.addSublayer(imageLayer)
        toolTip = "Drag into any app · double-click to edit"
    }

    required init?(coder: NSCoder) { fatalError() }

    override func layout() {
        super.layout()
        imageLayer.frame = bounds
    }

    func refreshSoon() {
        guard !pending else { return }
        pending = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) { [weak self] in
            self?.pending = false
            self?.refresh()
        }
    }

    func refresh() {
        imageLayer.contents = document?.rendered
    }

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func mouseDown(with event: NSEvent) {
        if event.clickCount == 2 { onDoubleClick?(); return }
        downPoint = event.locationInWindow
    }

    override func mouseDragged(with event: NSEvent) {
        guard let start = downPoint, hypot(event.locationInWindow.x - start.x, event.locationInWindow.y - start.y) > 3,
              let document else { return }
        downPoint = nil
        CaptureDrag.begin(document, from: self, event: event, source: self)
    }

    nonisolated func draggingSession(_ session: NSDraggingSession, sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation {
        context == .outsideApplication ? .copy : []
    }

    nonisolated func draggingSession(_ session: NSDraggingSession, endedAt screenPoint: NSPoint, operation: NSDragOperation) {
        guard !operation.isEmpty else { return }
        MainActor.assumeIsolated { (window as? CapturePreview)?.finishInteraction(after: 0.1) }
    }
}

/// Reports the pointer entering / leaving (pauses the card's countdown).
final class HoverView: NSView {
    var onHover: ((Bool) -> Void)?
    override func updateTrackingAreas() {
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.activeAlways, .mouseEnteredAndExited, .inVisibleRect], owner: self))
    }
    override func mouseEntered(with event: NSEvent) { onHover?(true) }
    override func mouseExited(with event: NSEvent) { onHover?(false) }
}

/// The "• • • •" handle at the top of the preview: drag it into any app.
@MainActor
final class DragGrip: NSView, NSDraggingSource {
    weak var document: CaptureDocument?

    override init(frame: NSRect) {
        super.init(frame: frame)
        toolTip = "Drag into any app"
    }

    required init?(coder: NSCoder) { fatalError() }

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func draw(_ dirtyRect: NSRect) {
        NSColor(white: 1, alpha: 0.45).setFill()
        for row in 0..<2 {
            for i in 0..<4 {
                let x = bounds.midX - 9 + CGFloat(i) * 6 - 1
                let y = bounds.midY - 2.5 + CGFloat(row) * 4
                NSBezierPath(ovalIn: NSRect(x: x, y: y, width: 2, height: 2)).fill()
            }
        }
    }

    override func resetCursorRects() { addCursorRect(bounds, cursor: .openHand) }

    override func mouseDragged(with event: NSEvent) {
        guard let document else { return }
        CaptureDrag.begin(document, from: self, event: event, source: self)
    }

    nonisolated func draggingSession(_ session: NSDraggingSession, sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation {
        context == .outsideApplication ? .copy : []
    }

    nonisolated func draggingSession(_ session: NSDraggingSession, endedAt screenPoint: NSPoint, operation: NSDragOperation) {
        guard !operation.isEmpty else { return }
        MainActor.assumeIsolated { (window as? CapturePreview)?.finishInteraction(after: 0.1) }
    }
}

@MainActor
enum CaptureDrag {
    static func begin(_ doc: CaptureDocument, from view: NSView, event: NSEvent, source: NSDraggingSource) {
        guard let url = doc.writeTemporaryFile() else { return }
        let item = NSDraggingItem(pasteboardWriter: url as NSURL)
        let s = doc.size
        let w = min(160, s.width), h = w * s.height / max(1, s.width)
        let p = view.convert(event.locationInWindow, from: nil)
        item.setDraggingFrame(NSRect(x: p.x - w / 2, y: p.y - h / 2, width: w, height: h),
                              contents: NSImage(cgImage: doc.rendered, size: s))
        view.beginDraggingSession(with: [item], event: event, source: source)
    }
}

/// Small round icon button for the dark HUD windows.
@MainActor
final class HUDButton: NSView {
    private let symbol: String
    private let size: CGFloat
    private let action: () -> Void
    private let icon = NSImageView()
    private var hovering = false { didSet { updateLook() } }
    var isOn = false { didSet { updateLook() } }

    init(symbol: String, size: CGFloat, help: String, action: @escaping () -> Void) {
        self.symbol = symbol
        self.size = size
        self.action = action
        super.init(frame: .zero)
        wantsLayer = true
        layer?.cornerRadius = 6
        layer?.cornerCurve = .continuous
        toolTip = help
        icon.imageScaling = .scaleNone
        addSubview(icon)
        setSymbol(symbol)
        updateLook()
    }

    required init?(coder: NSCoder) { fatalError() }

    func setSymbol(_ name: String) {
        icon.image = Theme.symbol(name, size: size, weight: .medium) ?? Theme.symbol("questionmark", size: size)
    }

    override func layout() {
        super.layout()
        icon.frame = bounds
    }

    override func updateTrackingAreas() {
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.activeAlways, .mouseEnteredAndExited, .inVisibleRect], owner: self))
    }

    override func mouseEntered(with event: NSEvent) { hovering = true }
    override func mouseExited(with event: NSEvent) { hovering = false }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func mouseDown(with event: NSEvent) {
        layer?.setAffineTransform(CGAffineTransform(scaleX: 0.9, y: 0.9))
    }

    override func mouseUp(with event: NSEvent) {
        layer?.setAffineTransform(.identity)
        if bounds.contains(convert(event.locationInWindow, from: nil)) { action() }
    }

    private func updateLook() {
        layer?.backgroundColor = NSColor(white: 1, alpha: isOn ? 0.2 : (hovering ? 0.1 : 0)).cgColor
        icon.contentTintColor = (hovering || isOn) ? .white : NSColor(white: 1, alpha: 0.72)
    }

    /// Brief checkmark after Copy / Save.
    func confirm() {
        setSymbol("checkmark")
        icon.contentTintColor = .white
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.2) { [weak self] in
            guard let self else { return }
            self.setSymbol(self.symbol)
            self.updateLook()
        }
    }
}

// MARK: - Pin

/// A capture pinned above every window. Drag to move, scroll or pinch to
/// resize, double-click to edit, right-click for more, Esc or × to close.
@MainActor
final class PinWindow: NSPanel {
    private static var open: [PinWindow] = []
    let document: CaptureDocument
    private let imageLayer = CALayer()

    static func show(_ doc: CaptureDocument, near: NSRect?) {
        let pin = PinWindow(document: doc)
        open.append(pin)
        let screen = NSScreen.screens.first { $0.frame.contains(NSEvent.mouseLocation) } ?? NSScreen.main ?? NSScreen.screens[0]
        let v = screen.visibleFrame
        var size = doc.size
        // Actual size (100%) unless it can't fit on screen.
        let fit = min(1, (v.width * 0.9) / size.width, (v.height * 0.9) / size.height)
        size = CGSize(width: (size.width * fit).rounded(), height: (size.height * fit).rounded())
        let origin = near.map { NSPoint(x: $0.midX - size.width / 2, y: $0.midY - size.height / 2) }
            ?? NSPoint(x: v.midX - size.width / 2, y: v.midY - size.height / 2)
        pin.setFrame(NSRect(origin: NSPoint(x: origin.x.rounded(), y: origin.y.rounded()), size: size), display: true)
        pin.alphaValue = 0
        pin.orderFrontRegardless()
        pin.makeKey()
        NSAnimationContext.runAnimationGroup { ctx in
            ctx.duration = 0.2
            pin.animator().alphaValue = 1
        }
    }

    init(document: CaptureDocument) {
        self.document = document
        super.init(contentRect: NSRect(x: 0, y: 0, width: 200, height: 200),
                   styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        isFloatingPanel = true
        level = .floating
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        backgroundColor = .clear
        isOpaque = false
        hasShadow = true
        isMovableByWindowBackground = true
        hidesOnDeactivate = false
        isReleasedWhenClosed = false

        let root = PinView()
        root.window_ = self
        root.wantsLayer = true
        root.layer?.cornerRadius = 6
        root.layer?.cornerCurve = .continuous
        root.layer?.masksToBounds = true
        root.layer?.borderColor = NSColor(white: 1, alpha: 0.2).cgColor
        root.layer?.borderWidth = 0.5
        imageLayer.contentsGravity = .resizeAspect
        imageLayer.contentsScale = document.scale
        imageLayer.minificationFilter = .trilinear
        imageLayer.contents = document.rendered
        root.layer?.addSublayer(imageLayer)
        contentView = root

        let closeButton = HUDButton(symbol: "xmark", size: 9, help: "Close  Esc") { [weak self] in self?.dismiss() }
        closeButton.wantsLayer = true
        closeButton.layer?.backgroundColor = NSColor(white: 0, alpha: 0.6).cgColor
        closeButton.layer?.cornerRadius = 10
        closeButton.isHidden = true
        root.addSubview(closeButton)
        root.closeButton = closeButton

        document.onChange.append { [weak self] in self?.imageLayer.contents = self?.document.rendered }
    }

    override var canBecomeKey: Bool { true }

    func layoutContent() {
        guard let root = contentView as? PinView else { return }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        imageLayer.frame = root.bounds
        CATransaction.commit()
        root.closeButton?.frame = NSRect(x: 6, y: root.bounds.height - 26, width: 20, height: 20)
    }

    func dismiss() {
        orderOut(nil)
        Self.open.removeAll { $0 === self }
    }

    func resize(by factor: CGFloat) {
        let s = document.size
        var w = frame.width * factor
        w = min(max(w, 60), s.width * 3)
        let h = w * s.height / max(1, s.width)
        let f = frame
        setFrame(NSRect(x: f.midX - w / 2, y: f.midY - h / 2, width: w, height: h), display: true)
    }

    func showMenu(_ event: NSEvent, in view: NSView) {
        let menu = NSMenu()
        menu.appearance = NSAppearance(named: .darkAqua)
        menu.addItem(ClosureMenuItem(title: "Copy") { [weak self] in if let d = self?.document { CaptureActions.copy(d) } })
        menu.addItem(ClosureMenuItem(title: "Save to Vault") { [weak self] in if let d = self?.document { CaptureActions.saveToVault(d) } })
        menu.addItem(ClosureMenuItem(title: "Save As…") { [weak self] in if let d = self?.document { CaptureActions.saveAs(d, from: nil) } })
        menu.addItem(ClosureMenuItem(title: "Copy Text") { [weak self] in if let d = self?.document { CaptureActions.copyText(d) } })
        menu.addItem(ClosureMenuItem(title: "Edit") { [weak self] in if let d = self?.document { CaptureActions.edit(d) } })
        menu.addItem(.separator())
        let opacity = NSMenu()
        for value in [1.0, 0.75, 0.5, 0.25] {
            let item = ClosureMenuItem(title: "\(Int(value * 100))%") { [weak self] in self?.alphaValue = value }
            item.state = abs(alphaValue - value) < 0.01 ? .on : .off
            opacity.addItem(item)
        }
        let opacityItem = NSMenuItem(title: "Opacity", action: nil, keyEquivalent: "")
        opacityItem.submenu = opacity
        menu.addItem(opacityItem)
        menu.addItem(ClosureMenuItem(title: "Actual Size") { [weak self] in
            guard let self else { return }
            self.resize(by: self.document.size.width / max(1, self.frame.width))
        })
        menu.addItem(.separator())
        menu.addItem(ClosureMenuItem(title: "Close") { [weak self] in self?.dismiss() })
        NSMenu.popUpContextMenu(menu, with: event, for: view)
    }

    override func keyDown(with event: NSEvent) {
        if event.keyCode == 53 { dismiss() }
        else if event.modifierFlags.contains(.command), event.charactersIgnoringModifiers == "c" { CaptureActions.copy(document) }
        else { super.keyDown(with: event) }
    }

    override func cancelOperation(_ sender: Any?) { dismiss() }
}

@MainActor
final class PinView: NSView {
    weak var window_: PinWindow?
    weak var closeButton: HUDButton?

    override var mouseDownCanMoveWindow: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func layout() {
        super.layout()
        window_?.layoutContent()
    }

    override func updateTrackingAreas() {
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.activeAlways, .mouseEnteredAndExited, .inVisibleRect], owner: self))
    }

    override func mouseEntered(with event: NSEvent) { closeButton?.isHidden = false }
    override func mouseExited(with event: NSEvent) { closeButton?.isHidden = true }

    override func mouseDown(with event: NSEvent) {
        window?.makeKey()
        if event.clickCount == 2, let doc = window_?.document { CaptureActions.edit(doc); return }
        super.mouseDown(with: event)
    }

    override func rightMouseDown(with event: NSEvent) { window_?.showMenu(event, in: self) }

    override func scrollWheel(with event: NSEvent) {
        let delta = event.hasPreciseScrollingDeltas ? event.scrollingDeltaY / 200 : event.scrollingDeltaY / 20
        window_?.resize(by: 1 + delta)
    }

    override func magnify(with event: NSEvent) { window_?.resize(by: 1 + event.magnification) }
}
