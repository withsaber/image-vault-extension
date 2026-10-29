import AppKit

// MARK: - Model

enum AnnotationKind: Equatable {
    case arrow, line, measure, rectangle, filledRect, oval, pencil, highlighter, blur, spotlight, counter, text, image
}

/// One mark on a capture, in image points (origin bottom-left). Two-point
/// kinds use points[0]…points[1]; pencil/highlighter use the whole path;
/// counter/text use points[0] (centre / top-left).
struct Annotation {
    var id = UUID()
    var kind: AnnotationKind
    var points: [CGPoint]
    var color: NSColor
    var width: CGFloat
    var text = ""
    var fontSize: CGFloat = 22
    var image: CGImage?

    var rect: CGRect {
        guard let a = points.first else { return .zero }
        let b = points.count > 1 ? points[1] : a
        return CGRect(x: min(a.x, b.x), y: min(a.y, b.y), width: abs(b.x - a.x), height: abs(b.y - a.y))
    }

    var counterRadius: CGFloat { max(12, width * 3) }

    var textAttributes: [NSAttributedString.Key: Any] {
        let shadow = NSShadow()
        shadow.shadowColor = NSColor(white: 0, alpha: 0.45)
        shadow.shadowBlurRadius = 3
        shadow.shadowOffset = NSSize(width: 0, height: -1)
        return [.font: NSFont.systemFont(ofSize: fontSize, weight: .bold), .foregroundColor: color, .shadow: shadow]
    }

    var textSize: CGSize {
        let s = (text.isEmpty ? " " : text as NSString).size(withAttributes: textAttributes)
        return CGSize(width: ceil(s.width), height: ceil(s.height))
    }

    /// Area used for hit-testing and the selection outline.
    var bounds: CGRect {
        switch kind {
        case .counter:
            let r = counterRadius, c = points.first ?? .zero
            return CGRect(x: c.x - r, y: c.y - r, width: r * 2, height: r * 2)
        case .text:
            let p = points.first ?? .zero, s = textSize
            return CGRect(x: p.x, y: p.y - s.height, width: s.width, height: s.height)
        case .pencil, .highlighter:
            guard let first = points.first else { return .zero }
            var r = CGRect(origin: first, size: .zero)
            for p in points { r = r.union(CGRect(origin: p, size: .zero)) }
            let pad = (kind == .highlighter ? highlighterWidth : width) / 2
            return r.insetBy(dx: -pad, dy: -pad)
        default:
            return rect
        }
    }

    var highlighterWidth: CGFloat { max(16, width * 4) }

    func hit(_ p: CGPoint, tolerance: CGFloat) -> Bool {
        switch kind {
        case .arrow, .line, .measure:
            guard points.count > 1 else { return false }
            return Self.distance(p, points[0], points[1]) <= width / 2 + tolerance
        case .pencil, .highlighter:
            let w = (kind == .highlighter ? highlighterWidth : width) / 2 + tolerance
            for i in 1..<max(1, points.count) where Self.distance(p, points[i - 1], points[i]) <= w { return true }
            return points.count == 1 && hypot(p.x - points[0].x, p.y - points[0].y) <= w
        case .rectangle, .oval:
            // Outlines are picked by their edge or anywhere inside.
            return bounds.insetBy(dx: -tolerance, dy: -tolerance).contains(p)
        default:
            return bounds.insetBy(dx: -tolerance, dy: -tolerance).contains(p)
        }
    }

    mutating func move(by d: CGVector) {
        points = points.map { CGPoint(x: $0.x + d.dx, y: $0.y + d.dy) }
    }

    static func distance(_ p: CGPoint, _ a: CGPoint, _ b: CGPoint) -> CGFloat {
        let dx = b.x - a.x, dy = b.y - a.y
        let len2 = dx * dx + dy * dy
        guard len2 > 0 else { return hypot(p.x - a.x, p.y - a.y) }
        let t = max(0, min(1, ((p.x - a.x) * dx + (p.y - a.y) * dy) / len2))
        return hypot(p.x - (a.x + t * dx), p.y - (a.y + t * dy))
    }
}

// MARK: - Rendering

/// Draws a capture and its annotations into a context already scaled to
/// image points. Used both on screen (editor) and for export.
@MainActor
enum AnnotationRenderer {
    static func draw(document: CaptureDocument, annotations: [Annotation], in ctx: CGContext) {
        let full = CGRect(origin: .zero, size: document.size)
        ctx.draw(document.base, in: full)

        // Blur/Erase first, so later marks sit on top of the blurred area.
        for a in annotations where a.kind == .blur && a.rect.width > 0 && a.rect.height > 0 {
            ctx.saveGState()
            ctx.clip(to: a.rect)
            ctx.draw(document.blurred, in: full)
            ctx.restoreGState()
        }

        // Spotlight: dim everything outside the union of spotlight boxes.
        let spots = annotations.filter { $0.kind == .spotlight && $0.rect.width > 1 && $0.rect.height > 1 }
        if !spots.isEmpty {
            let path = CGMutablePath()
            path.addRect(full)
            for s in spots { path.addPath(CGPath(roundedRect: s.rect, cornerWidth: 6, cornerHeight: 6, transform: nil)) }
            ctx.saveGState()
            ctx.addPath(path)
            ctx.setFillColor(NSColor(white: 0, alpha: 0.55).cgColor)
            ctx.fillPath(using: .evenOdd)
            ctx.restoreGState()
        }

        var counter = 0
        for a in annotations {
            if a.kind == .counter { counter += 1 }
            draw(a, number: counter, in: ctx)
        }
    }

    static func draw(_ a: Annotation, number: Int, in ctx: CGContext) {
        ctx.saveGState()
        defer { ctx.restoreGState() }
        let color = a.color.cgColor
        ctx.setLineCap(.round)
        ctx.setLineJoin(.round)
        let shadow = { ctx.setShadow(offset: CGSize(width: 0, height: -1), blur: 3, color: NSColor(white: 0, alpha: 0.35).cgColor) }

        switch a.kind {
        case .blur, .spotlight:
            break

        case .arrow:
            guard a.points.count > 1 else { return }
            let p0 = a.points[0], p1 = a.points[1]
            let len = hypot(p1.x - p0.x, p1.y - p0.y)
            guard len > 1 else { return }
            let angle = atan2(p1.y - p0.y, p1.x - p0.x)
            let head = min(max(a.width * 3.6, 12), len * 0.6)
            let base = CGPoint(x: p1.x - head * 0.8 * cos(angle), y: p1.y - head * 0.8 * sin(angle))
            shadow()
            ctx.beginTransparencyLayer(auxiliaryInfo: nil)
            ctx.setStrokeColor(color)
            ctx.setLineWidth(a.width)
            ctx.move(to: p0); ctx.addLine(to: base); ctx.strokePath()
            ctx.setFillColor(color)
            ctx.move(to: p1)
            ctx.addLine(to: CGPoint(x: p1.x - head * cos(angle - 0.48), y: p1.y - head * sin(angle - 0.48)))
            ctx.addLine(to: CGPoint(x: p1.x - head * cos(angle + 0.48), y: p1.y - head * sin(angle + 0.48)))
            ctx.closePath()
            ctx.setLineWidth(max(1, a.width * 0.5))
            ctx.drawPath(using: .fillStroke)
            ctx.endTransparencyLayer()

        case .line:
            guard a.points.count > 1 else { return }
            shadow()
            ctx.setStrokeColor(color)
            ctx.setLineWidth(a.width)
            ctx.move(to: a.points[0]); ctx.addLine(to: a.points[1]); ctx.strokePath()

        case .measure:
            guard a.points.count > 1 else { return }
            let p0 = a.points[0], p1 = a.points[1]
            let angle = atan2(p1.y - p0.y, p1.x - p0.x)
            let n = CGVector(dx: -sin(angle) * 6, dy: cos(angle) * 6)
            shadow()
            ctx.setStrokeColor(color)
            ctx.setLineWidth(max(1.5, a.width * 0.5))
            ctx.move(to: p0); ctx.addLine(to: p1)
            for p in [p0, p1] {
                ctx.move(to: CGPoint(x: p.x - n.dx, y: p.y - n.dy)); ctx.addLine(to: CGPoint(x: p.x + n.dx, y: p.y + n.dy))
            }
            ctx.strokePath()
            ctx.setShadow(offset: .zero, blur: 0, color: nil)
            let label = "\(Int(hypot(p1.x - p0.x, p1.y - p0.y).rounded())) pt" as NSString
            let attrs: [NSAttributedString.Key: Any] = [.font: NSFont.monospacedSystemFont(ofSize: 11, weight: .semibold),
                                                        .foregroundColor: NSColor.white]
            let s = label.size(withAttributes: attrs)
            let mid = CGPoint(x: (p0.x + p1.x) / 2, y: (p0.y + p1.y) / 2)
            let pill = CGRect(x: mid.x - s.width / 2 - 6, y: mid.y + 6, width: s.width + 12, height: s.height + 4)
            ctx.setFillColor(NSColor(white: 0, alpha: 0.78).cgColor)
            ctx.addPath(CGPath(roundedRect: pill, cornerWidth: pill.height / 2, cornerHeight: pill.height / 2, transform: nil))
            ctx.fillPath()
            label.draw(at: CGPoint(x: pill.minX + 6, y: pill.minY + 2), withAttributes: attrs)

        case .rectangle:
            shadow()
            ctx.setStrokeColor(color)
            ctx.setLineWidth(a.width)
            ctx.addPath(CGPath(roundedRect: a.rect, cornerWidth: min(4, a.rect.width / 2, a.rect.height / 2),
                               cornerHeight: min(4, a.rect.width / 2, a.rect.height / 2), transform: nil))
            ctx.strokePath()

        case .filledRect:
            ctx.setFillColor(color)
            ctx.addPath(CGPath(roundedRect: a.rect, cornerWidth: min(3, a.rect.width / 2, a.rect.height / 2),
                               cornerHeight: min(3, a.rect.width / 2, a.rect.height / 2), transform: nil))
            ctx.fillPath()

        case .oval:
            shadow()
            ctx.setStrokeColor(color)
            ctx.setLineWidth(a.width)
            ctx.strokeEllipse(in: a.rect)

        case .pencil:
            shadow()
            ctx.setStrokeColor(color)
            ctx.setLineWidth(a.width)
            ctx.addPath(smooth(a.points))
            ctx.strokePath()

        case .highlighter:
            // Plain translucency: multiply vanishes on dark screenshots.
            ctx.setStrokeColor(a.color.withAlphaComponent(0.4).cgColor)
            ctx.setLineWidth(a.highlighterWidth)
            ctx.setLineCap(.butt)
            ctx.addPath(smooth(a.points))
            ctx.strokePath()

        case .counter:
            guard let c = a.points.first else { return }
            let r = a.counterRadius
            shadow()
            ctx.setFillColor(color)
            ctx.fillEllipse(in: CGRect(x: c.x - r, y: c.y - r, width: r * 2, height: r * 2))
            ctx.setShadow(offset: .zero, blur: 0, color: nil)
            let light = (a.color.usingColorSpace(.sRGB)?.brightnessComponent ?? 0) > 0.85
                && (a.color.usingColorSpace(.sRGB)?.saturationComponent ?? 1) < 0.2
            let attrs: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: r * 1.05, weight: .bold),
                                                        .foregroundColor: light ? NSColor.black : NSColor.white]
            let label = "\(number)" as NSString
            let s = label.size(withAttributes: attrs)
            label.draw(at: CGPoint(x: c.x - s.width / 2, y: c.y - s.height / 2), withAttributes: attrs)

        case .text:
            guard let p = a.points.first, !a.text.isEmpty else { return }
            (a.text as NSString).draw(at: CGPoint(x: p.x, y: p.y - a.textSize.height), withAttributes: a.textAttributes)

        case .image:
            guard let image = a.image else { return }
            shadow()
            ctx.draw(image, in: a.rect)
            ctx.setShadow(offset: .zero, blur: 0, color: nil)
            ctx.setStrokeColor(NSColor(white: 1, alpha: 0.35).cgColor)
            ctx.setLineWidth(0.5)
            ctx.stroke(a.rect)
        }
    }

    /// Midpoint-quadratic smoothing for freehand strokes.
    static func smooth(_ pts: [CGPoint]) -> CGPath {
        let path = CGMutablePath()
        guard let first = pts.first else { return path }
        path.move(to: first)
        if pts.count == 1 { path.addLine(to: first); return path }
        for i in 1..<pts.count {
            let mid = CGPoint(x: (pts[i - 1].x + pts[i].x) / 2, y: (pts[i - 1].y + pts[i].y) / 2)
            path.addQuadCurve(to: mid, control: pts[i - 1])
        }
        path.addLine(to: pts[pts.count - 1])
        return path
    }
}

// MARK: - Tools

enum EditorTool: String, CaseIterable {
    case select, arrow, text, measure, rectangle, filledRect, pencil, blur, highlighter, spotlight, counter, oval, line

    var title: String {
        switch self {
        case .select: return "Select & Move"
        case .arrow: return "Arrow"
        case .text: return "Text"
        case .measure: return "Measure"
        case .rectangle: return "Rectangle"
        case .filledRect: return "Filled Rectangle"
        case .pencil: return "Pen"
        case .blur: return "Blur/Erase"
        case .highlighter: return "Highlighter"
        case .spotlight: return "Spotlight"
        case .counter: return "Counter"
        case .oval: return "Oval"
        case .line: return "Line"
        }
    }

    var key: String {
        switch self {
        case .select: return "v"
        case .arrow: return "a"
        case .text: return "t"
        case .measure: return "m"
        case .rectangle: return "r"
        case .filledRect: return "f"
        case .pencil: return "p"
        case .blur: return "b"
        case .highlighter: return "h"
        case .spotlight: return "s"
        case .counter: return "c"
        case .oval: return "o"
        case .line: return "l"
        }
    }

    var symbol: String {
        switch self {
        case .select: return "cursorarrow"
        case .arrow: return "arrow.up.right"
        case .text: return "textformat"
        case .measure: return "ruler"
        case .rectangle: return "rectangle"
        case .filledRect: return "rectangle.fill"
        case .pencil: return "pencil"
        case .blur: return "drop"
        case .highlighter: return "highlighter"
        case .spotlight: return "rectangle.dashed"
        case .counter: return "1.circle"
        case .oval: return "circle"
        case .line: return "line.diagonal"
        }
    }

    var kind: AnnotationKind? {
        switch self {
        case .select, .text: return nil
        case .arrow: return .arrow
        case .measure: return .measure
        case .rectangle: return .rectangle
        case .filledRect: return .filledRect
        case .pencil: return .pencil
        case .blur: return .blur
        case .highlighter: return .highlighter
        case .spotlight: return .spotlight
        case .counter: return .counter
        case .oval: return .oval
        case .line: return .line
        }
    }

    /// Tools on the bar; the rest live in the ··· menu (like the reference).
    static let bar: [EditorTool] = [.select, .arrow, .text, .measure, .rectangle, .filledRect, .pencil]
    static let menu: [EditorTool] = [.blur, .highlighter, .spotlight, .counter, .oval, .line]
}

// MARK: - Window

@MainActor
final class EditorWindowController: NSWindowController, NSWindowDelegate {
    private static var open: [EditorWindowController] = []
    let capture: CaptureDocument
    private let canvas: EditorCanvas
    private let toolbar: EditorToolbar

    static func open(_ doc: CaptureDocument) {
        if let existing = open.first(where: { $0.capture === doc }) {
            NSApp.activate(ignoringOtherApps: true)
            existing.window?.makeKeyAndOrderFront(nil)
            return
        }
        let controller = EditorWindowController(document: doc)
        open.append(controller)
        controller.present()
    }

    init(document: CaptureDocument) {
        self.capture = document
        canvas = EditorCanvas(document: document)
        toolbar = EditorToolbar()

        let screen = NSScreen.screens.first { $0.frame.contains(NSEvent.mouseLocation) } ?? NSScreen.main ?? NSScreen.screens[0]
        let v = screen.visibleFrame
        // Open at 100%: as big as the image needs, up to the screen; larger
        // captures scroll instead of being shrunk.
        let pad = EditorCanvas.pad
        let barHeight = EditorToolbar.height
        let s = document.size
        let width = min(v.width, max(EditorToolbar.minWidth, s.width + pad * 2))
        let height = min(v.height, max(300, s.height + pad * 2 + barHeight))

        let window = NSWindow(contentRect: NSRect(x: v.midX - width / 2, y: v.midY - height / 2, width: width, height: height),
                              styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
                              backing: .buffered, defer: false)
        window.titleVisibility = .hidden
        window.titlebarAppearsTransparent = true
        window.appearance = NSAppearance(named: .darkAqua)
        window.backgroundColor = NSColor(white: 0.07, alpha: 1)
        window.isReleasedWhenClosed = false
        window.minSize = NSSize(width: EditorToolbar.minWidth, height: 260)
        window.collectionBehavior = [.fullScreenAuxiliary, .moveToActiveSpace]
        window.title = document.sourceItem.map { "Edit \($0.name)" } ?? "Edit Screenshot"
        // An empty compact toolbar makes the title bar tall enough for ours,
        // with the traffic lights centred in it.
        let systemBar = NSToolbar(identifier: "ImageVaultEditor")
        systemBar.showsBaselineSeparator = false
        window.toolbar = systemBar
        window.toolbarStyle = .unifiedCompact

        super.init(window: window)
        window.delegate = self

        let root = NSView()
        window.contentView = root
        let scroll = NSScrollView()
        scroll.translatesAutoresizingMaskIntoConstraints = false
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = true
        scroll.autohidesScrollers = true
        scroll.scrollerStyle = .overlay
        scroll.drawsBackground = true
        scroll.backgroundColor = NSColor(white: 0.07, alpha: 1)
        scroll.documentView = canvas
        scroll.contentView.postsFrameChangedNotifications = true
        NotificationCenter.default.addObserver(forName: NSView.frameDidChangeNotification, object: scroll.contentView,
                                               queue: .main) { [weak canvas] _ in
            MainActor.assumeIsolated { canvas?.relayout() }
        }
        toolbar.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(scroll)
        root.addSubview(toolbar)
        NSLayoutConstraint.activate([
            toolbar.topAnchor.constraint(equalTo: root.topAnchor),
            toolbar.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            toolbar.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            toolbar.heightAnchor.constraint(equalToConstant: barHeight),
            scroll.topAnchor.constraint(equalTo: toolbar.bottomAnchor),
            scroll.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            scroll.bottomAnchor.constraint(equalTo: root.bottomAnchor),
        ])

        toolbar.build(document: document)
        toolbar.onTool = { [weak self] tool in self?.canvas.tool = tool }
        toolbar.onColor = { [weak self] color in self?.canvas.setColor(color) }
        toolbar.onWidth = { [weak self] width in self?.canvas.setWidth(width) }
        toolbar.onCommand = { [weak self] command in self?.run(command) }
        toolbar.select(tool: canvas.tool, color: canvas.color, width: canvas.strokeWidth)
        canvas.onToolChange = { [weak self] tool in
            guard let self else { return }
            self.toolbar.select(tool: tool, color: self.canvas.color, width: self.canvas.strokeWidth)
        }
        canvas.onHoverColor = { [weak self] color in self?.toolbar.showHover(color) }
        canvas.onCommand = { [weak self] command in self?.run(command) }
        canvas.onFlash = { [weak self] text in self?.toolbar.flash(text) }
        canvas.onZoom = { [weak self] zoom in self?.toolbar.showZoom(zoom) }
        toolbar.onZoom = { [weak self] action in self?.canvas.zoom(action) }
    }

    required init?(coder: NSCoder) { fatalError() }

    private func present() {
        NSApp.activate(ignoringOtherApps: true)
        window?.makeKeyAndOrderFront(nil)
        window?.makeFirstResponder(canvas)
        canvas.relayout()
    }

    enum Command { case copy, save, saveAs, pin, copyText, addCapture, paste, close }

    func run(_ command: Command) {
        canvas.commitText()
        switch command {
        case .copy:
            CaptureActions.copy(capture)
            toolbar.flash("Copied")
        case .save:
            CaptureActions.saveToVault(capture)
            toolbar.flash("Saved to Vault")
        case .saveAs:
            CaptureActions.saveAs(capture, from: window)
        case .copyText:
            CaptureActions.copyText(capture)
        case .pin:
            CaptureActions.pin(capture, near: window?.frame)
            window?.close()
        case .addCapture:
            addCapture()
        case .paste:
            canvas.pasteImage()
        case .close:
            window?.close()
        }
    }

    private func addCapture() {
        guard let window else { return }
        window.orderOut(nil)
        ScreenOCRController.shared.pickImage(onImage: { [weak self] image, screen in
            guard let self else { return }
            let size = CGSize(width: CGFloat(image.width) / screen.backingScaleFactor,
                              height: CGFloat(image.height) / screen.backingScaleFactor)
            self.canvas.insert(image: image, size: size)
            self.present()
        }, onCancel: { [weak self] in
            self?.present()
        })
    }

    func windowWillClose(_ notification: Notification) {
        canvas.commitText()
        Self.open.removeAll { $0 === self }
    }
}

// MARK: - Canvas

@MainActor
final class EditorCanvas: NSView, NSTextFieldDelegate {
    let document: CaptureDocument
    var tool: EditorTool = .arrow {
        didSet {
            if tool != oldValue { commitText(); if tool != .select { selectedID = nil }; window?.invalidateCursorRects(for: self) }
            onToolChange?(tool)
        }
    }
    private(set) var color = NSColor(srgbRed: 1, green: 0.23, blue: 0.19, alpha: 1)
    private(set) var strokeWidth: CGFloat = 4

    var onToolChange: ((EditorTool) -> Void)?
    var onHoverColor: ((NSColor?) -> Void)?
    var onCommand: ((EditorWindowController.Command) -> Void)?
    var onFlash: ((String) -> Void)?
    var onZoom: ((CGFloat) -> Void)?

    static let pad: CGFloat = 16
    /// nil = fit the window; otherwise a fixed zoom (1 = 100%, actual pixels).
    private var fixedZoom: CGFloat? = 1
    private(set) var zoomLevel: CGFloat = 1

    enum ZoomAction { case actual, fit, zoomIn, zoomOut }

    private var annotations: [Annotation]
    private var undoStack: [[Annotation]] = []
    private var redoStack: [[Annotation]] = []
    private var draft: Annotation?
    private var selectedID: UUID? { didSet { needsDisplay = true } }
    private var dragStart: CGPoint?
    private var dragMoved = false
    private var hoverColor: NSColor?
    private var textField: NSTextField?
    private var editingTextID: UUID?

    init(document: CaptureDocument) {
        self.document = document
        annotations = document.annotations
        super.init(frame: .zero)
        wantsLayer = true
    }

    required init?(coder: NSCoder) { fatalError() }

    override var acceptsFirstResponder: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override var mouseDownCanMoveWindow: Bool { false }

    // MARK: Geometry

    /// The image, centred, at the current zoom, aligned to device pixels so
    /// 100% shows exactly the captured pixels.
    private var imageRect: CGRect {
        let s = document.size
        let w = s.width * zoomLevel, h = s.height * zoomLevel
        return backingAlignedRect(CGRect(x: bounds.midX - w / 2, y: bounds.midY - h / 2, width: w, height: h),
                                  options: [.alignMinXNearest, .alignMinYNearest, .alignWidthNearest, .alignHeightNearest])
    }

    private var viewScale: CGFloat { zoomLevel }

    /// Sizes the canvas to the zoomed image (at least the visible area, so a
    /// small image stays centred) and keeps the view centred on the same spot.
    func relayout() {
        guard let scroll = enclosingScrollView else { return }
        let clip = scroll.contentView.bounds.size
        let s = document.size
        let pad = Self.pad
        let fit = max(0.05, min(1, (clip.width - pad * 2) / s.width, (clip.height - pad * 2) / s.height))
        let newZoom = fixedZoom ?? fit
        let visible = scroll.contentView.bounds
        let anchor = CGPoint(x: (visible.midX - imageRect.minX) / max(0.001, zoomLevel),
                             y: (visible.midY - imageRect.minY) / max(0.001, zoomLevel))
        let changed = abs(newZoom - zoomLevel) > 0.0001
        zoomLevel = newZoom
        let size = NSSize(width: max(clip.width, (s.width * zoomLevel + pad * 2).rounded(.up)),
                          height: max(clip.height, (s.height * zoomLevel + pad * 2).rounded(.up)))
        if frame.size != size { setFrameSize(size) }
        if changed {
            let r = imageRect
            let target = CGPoint(x: r.minX + anchor.x * zoomLevel - clip.width / 2, y: r.minY + anchor.y * zoomLevel - clip.height / 2)
            scroll.contentView.scroll(to: CGPoint(x: min(max(0, target.x), size.width - clip.width),
                                                  y: min(max(0, target.y), size.height - clip.height)))
            scroll.reflectScrolledClipView(scroll.contentView)
        }
        needsDisplay = true
        window?.invalidateCursorRects(for: self)
        onZoom?(zoomLevel)
    }

    func zoom(_ action: ZoomAction) {
        commitText()
        switch action {
        case .actual: fixedZoom = 1
        case .fit: fixedZoom = nil
        case .zoomIn: fixedZoom = Self.step(from: zoomLevel, up: true)
        case .zoomOut: fixedZoom = Self.step(from: zoomLevel, up: false)
        }
        relayout()
    }

    private static let steps: [CGFloat] = [0.1, 0.25, 0.33, 0.5, 0.67, 0.75, 1, 1.5, 2, 3, 4, 6, 8]

    private static func step(from z: CGFloat, up: Bool) -> CGFloat {
        up ? (steps.first { $0 > z + 0.001 } ?? 8) : (steps.last { $0 < z - 0.001 } ?? 0.1)
    }

    override func magnify(with event: NSEvent) {
        fixedZoom = min(8, max(0.1, zoomLevel * (1 + event.magnification)))
        relayout()
    }

    private func toImage(_ p: NSPoint) -> CGPoint {
        let r = imageRect
        return CGPoint(x: (p.x - r.minX) / viewScale, y: (p.y - r.minY) / viewScale)
    }

    private func toView(_ p: CGPoint) -> NSPoint {
        let r = imageRect
        return NSPoint(x: r.minX + p.x * viewScale, y: r.minY + p.y * viewScale)
    }

    private func toView(_ rect: CGRect) -> NSRect {
        let o = toView(rect.origin)
        return NSRect(x: o.x, y: o.y, width: rect.width * viewScale, height: rect.height * viewScale)
    }

    private func clamp(_ p: CGPoint) -> CGPoint {
        let s = document.size
        return CGPoint(x: min(max(p.x, 0), s.width), y: min(max(p.y, 0), s.height))
    }

    // MARK: Drawing

    override func draw(_ dirtyRect: NSRect) {
        guard let ctx = NSGraphicsContext.current?.cgContext else { return }
        NSColor(white: 0.07, alpha: 1).setFill()
        bounds.fill()

        let r = imageRect
        ctx.saveGState()
        ctx.setShadow(offset: CGSize(width: 0, height: -4), blur: 18, color: NSColor(white: 0, alpha: 0.6).cgColor)
        ctx.setFillColor(NSColor.black.cgColor)
        ctx.fill(r)
        ctx.restoreGState()

        ctx.saveGState()
        ctx.clip(to: r)
        ctx.translateBy(x: r.minX, y: r.minY)
        ctx.scaleBy(x: viewScale, y: viewScale)
        // At 100% the capture lands pixel-for-pixel; never smooth it.
        ctx.interpolationQuality = abs(viewScale - 1) < 0.001 ? .none : .high
        var items = annotations.filter { $0.id != editingTextID }
        if let draft { items.append(draft) }
        AnnotationRenderer.draw(document: document, annotations: items, in: ctx)
        ctx.restoreGState()

        if let id = selectedID, let a = annotations.first(where: { $0.id == id }) {
            let box = toView(a.bounds).insetBy(dx: -4, dy: -4)
            let path = NSBezierPath(roundedRect: box, xRadius: 3, yRadius: 3)
            path.lineWidth = 1
            NSColor(white: 0, alpha: 0.6).setStroke()
            path.stroke()
            path.setLineDash([4, 3], count: 2, phase: 0)
            NSColor.white.setStroke()
            path.stroke()
        }
    }

    // MARK: Cursor

    override func resetCursorRects() {
        let cursor: NSCursor
        switch tool {
        case .select: cursor = .arrow
        case .text: cursor = .iBeam
        default: cursor = .crosshair
        }
        addCursorRect(imageRect, cursor: cursor)
    }

    override func updateTrackingAreas() {
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.activeInKeyWindow, .mouseMoved, .mouseEnteredAndExited, .inVisibleRect],
                                       owner: self))
    }

    override func setFrameSize(_ newSize: NSSize) {
        commitText()
        super.setFrameSize(newSize)
        window?.invalidateCursorRects(for: self)
    }

    override func mouseMoved(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        hoverColor = imageRect.contains(p) ? document.color(at: toImage(p)) : nil
        onHoverColor?(hoverColor)
    }

    override func mouseExited(with event: NSEvent) {
        hoverColor = nil
        onHoverColor?(nil)
    }

    // MARK: Mouse

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        let v = convert(event.locationInWindow, from: nil)
        let p = toImage(v)
        let tolerance = 5 / viewScale

        if tool == .select {
            let hit = annotations.last { $0.hit(p, tolerance: tolerance) }
            selectedID = hit?.id
            if let hit, event.clickCount == 2, hit.kind == .text { beginText(editing: hit); return }
            dragStart = hit == nil ? nil : p
            dragMoved = false
            return
        }

        if tool == .text {
            if textField != nil { commitText(); return }
            if let hit = annotations.last(where: { $0.kind == .text && $0.hit(p, tolerance: tolerance) }) {
                beginText(editing: hit)
            } else {
                beginText(at: clamp(p))
            }
            return
        }

        guard let kind = tool.kind else { return }
        let start = clamp(p)
        if kind == .counter {
            commit(Annotation(kind: .counter, points: [start], color: color, width: strokeWidth))
            return
        }
        draft = Annotation(kind: kind, points: [start, start], color: kind == .blur || kind == .spotlight ? .clear : color,
                           width: strokeWidth)
        if kind == .pencil || kind == .highlighter { draft?.points = [start] }
        needsDisplay = true
    }

    override func mouseDragged(with event: NSEvent) {
        let p = clamp(toImage(convert(event.locationInWindow, from: nil)))

        if tool == .select, let start = dragStart, let id = selectedID, let i = annotations.firstIndex(where: { $0.id == id }) {
            if !dragMoved { pushUndo(); dragMoved = true }
            annotations[i].move(by: CGVector(dx: p.x - start.x, dy: p.y - start.y))
            dragStart = p
            needsDisplay = true
            return
        }

        guard var d = draft else { return }
        switch d.kind {
        case .pencil, .highlighter:
            if let last = d.points.last, hypot(p.x - last.x, p.y - last.y) * viewScale >= 1.5 { d.points.append(p) }
        default:
            d.points[1] = event.modifierFlags.contains(.shift) ? constrained(from: d.points[0], to: p, kind: d.kind) : p
        }
        draft = d
        needsDisplay = true
    }

    override func mouseUp(with event: NSEvent) {
        if tool == .select {
            if dragMoved { saveToDocument() }
            dragStart = nil
            dragMoved = false
            return
        }
        guard let d = draft else { return }
        draft = nil
        let big: Bool
        switch d.kind {
        case .pencil, .highlighter: big = d.points.count > 1
        case .arrow, .line, .measure: big = hypot(d.points[1].x - d.points[0].x, d.points[1].y - d.points[0].y) * viewScale > 4
        default: big = d.rect.width * viewScale > 3 && d.rect.height * viewScale > 3
        }
        if big { commit(d) } else { needsDisplay = true }
    }

    override func rightMouseDown(with event: NSEvent) {
        let p = toImage(convert(event.locationInWindow, from: nil))
        guard let hit = annotations.last(where: { $0.hit(p, tolerance: 5 / viewScale) }) else { return }
        selectedID = hit.id
        let menu = NSMenu()
        menu.addItem(ClosureMenuItem(title: "Delete") { [weak self] in self?.deleteSelection() })
        menu.addItem(ClosureMenuItem(title: "Bring to Front") { [weak self] in self?.reorder(front: true) })
        menu.addItem(ClosureMenuItem(title: "Send to Back") { [weak self] in self?.reorder(front: false) })
        NSMenu.popUpContextMenu(menu, with: event, for: self)
    }

    /// Shift: lines snap to 45°, boxes become squares.
    private func constrained(from a: CGPoint, to b: CGPoint, kind: AnnotationKind) -> CGPoint {
        let dx = b.x - a.x, dy = b.y - a.y
        switch kind {
        case .arrow, .line, .measure:
            let angle = (atan2(dy, dx) / (.pi / 4)).rounded() * (.pi / 4)
            let len = hypot(dx, dy)
            return CGPoint(x: a.x + len * cos(angle), y: a.y + len * sin(angle))
        default:
            let side = max(abs(dx), abs(dy))
            return CGPoint(x: a.x + (dx < 0 ? -side : side), y: a.y + (dy < 0 ? -side : side))
        }
    }

    // MARK: Editing model

    private func pushUndo() {
        undoStack.append(annotations)
        if undoStack.count > 200 { undoStack.removeFirst() }
        redoStack.removeAll()
    }

    private func commit(_ a: Annotation) {
        pushUndo()
        annotations.append(a)
        saveToDocument()
    }

    private func saveToDocument() {
        document.annotations = annotations
        needsDisplay = true
    }

    func undo() {
        commitText()
        guard let previous = undoStack.popLast() else { NSSound.beep(); return }
        redoStack.append(annotations)
        annotations = previous
        selectedID = nil
        saveToDocument()
    }

    func redo() {
        guard let next = redoStack.popLast() else { NSSound.beep(); return }
        undoStack.append(annotations)
        annotations = next
        selectedID = nil
        saveToDocument()
    }

    private func deleteSelection() {
        guard let id = selectedID else { return }
        pushUndo()
        annotations.removeAll { $0.id == id }
        selectedID = nil
        saveToDocument()
    }

    private func reorder(front: Bool) {
        guard let id = selectedID, let i = annotations.firstIndex(where: { $0.id == id }) else { return }
        pushUndo()
        let a = annotations.remove(at: i)
        if front { annotations.append(a) } else { annotations.insert(a, at: 0) }
        saveToDocument()
    }

    func setColor(_ c: NSColor) {
        color = c
        if let id = selectedID, let i = annotations.firstIndex(where: { $0.id == id }),
           ![.blur, .spotlight, .image].contains(annotations[i].kind) {
            pushUndo()
            annotations[i].color = c
            saveToDocument()
        }
        textField?.textColor = c
        onToolChange?(tool)
    }

    func setWidth(_ w: CGFloat) {
        strokeWidth = w
        if let id = selectedID, let i = annotations.firstIndex(where: { $0.id == id }) {
            pushUndo()
            annotations[i].width = w
            if annotations[i].kind == .text { annotations[i].fontSize = Self.fontSize(for: w) }
            saveToDocument()
        }
        onToolChange?(tool)
    }

    static func fontSize(for width: CGFloat) -> CGFloat { width <= 2 ? 16 : (width <= 4 ? 22 : 32) }

    /// Add Capture / Paste Image: a movable picture, centred, scaled to fit.
    func insert(image: CGImage, size: CGSize) {
        let s = document.size
        let k = min(1, s.width * 0.8 / max(1, size.width), s.height * 0.8 / max(1, size.height))
        let w = size.width * k, h = size.height * k
        let origin = CGPoint(x: (s.width - w) / 2, y: (s.height - h) / 2)
        var a = Annotation(kind: .image, points: [origin, CGPoint(x: origin.x + w, y: origin.y + h)], color: .white, width: 0)
        a.image = image
        commit(a)
        tool = .select
        selectedID = a.id
    }

    func pasteImage() {
        let pb = NSPasteboard.general
        guard let image = (pb.readObjects(forClasses: [NSImage.self]) as? [NSImage])?.first,
              let cg = image.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
            onFlash?("No image on the clipboard")
            NSSound.beep()
            return
        }
        insert(image: cg, size: image.size)
    }

    // MARK: Text

    private func beginText(at p: CGPoint) {
        var a = Annotation(kind: .text, points: [p], color: color, width: strokeWidth)
        a.fontSize = Self.fontSize(for: strokeWidth)
        showField(for: a, isNew: true)
    }

    private func beginText(editing a: Annotation) {
        selectedID = nil
        showField(for: a, isNew: false)
    }

    private var pendingText: Annotation?
    private var pendingIsNew = false

    private func showField(for a: Annotation, isNew: Bool) {
        commitText()
        pendingText = a
        pendingIsNew = isNew
        editingTextID = isNew ? nil : a.id
        let field = NSTextField(string: a.text)
        field.isBordered = false
        field.drawsBackground = false
        field.focusRingType = .none
        field.font = NSFont.systemFont(ofSize: a.fontSize * viewScale, weight: .bold)
        field.textColor = a.color
        field.delegate = self
        field.cell?.isScrollable = true
        field.placeholderString = "Type…"
        let origin = toView(a.points[0])
        let h = ceil(a.fontSize * viewScale * 1.3)
        field.frame = NSRect(x: origin.x - 2, y: origin.y - h, width: max(160, a.textSize.width * viewScale + 40), height: h)
        addSubview(field)
        textField = field
        window?.makeFirstResponder(field)
        needsDisplay = true
    }

    func controlTextDidChange(_ obj: Notification) {
        guard let field = textField else { return }
        let w = (field.stringValue as NSString).size(withAttributes: [.font: field.font as Any]).width
        field.frame.size.width = max(160, w + 40)
    }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
        if selector == #selector(NSResponder.insertNewline(_:)) || selector == #selector(NSResponder.cancelOperation(_:)) {
            commitText()
            window?.makeFirstResponder(self)
            return true
        }
        return false
    }

    func controlTextDidEndEditing(_ obj: Notification) { commitText() }

    func commitText() {
        guard let field = textField, var a = pendingText else { return }
        textField = nil
        pendingText = nil
        let text = field.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        field.delegate = nil
        field.removeFromSuperview()
        editingTextID = nil
        a.text = text
        if pendingIsNew {
            if !text.isEmpty { commit(a) }
        } else if let i = annotations.firstIndex(where: { $0.id == a.id }) {
            pushUndo()
            if text.isEmpty { annotations.remove(at: i) } else { annotations[i] = a }
            saveToDocument()
        }
        needsDisplay = true
        if window?.firstResponder == nil || window?.firstResponder === window { window?.makeFirstResponder(self) }
    }

    // MARK: Keyboard

    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        guard window?.isKeyWindow == true, window?.firstResponder === self,
              event.modifierFlags.contains(.command), let key = event.charactersIgnoringModifiers?.lowercased() else {
            return super.performKeyEquivalent(with: event)
        }
        let shift = event.modifierFlags.contains(.shift)
        switch key {
        case "z": shift ? redo() : undo()
        case "c": onCommand?(.copy)
        case "s": onCommand?(shift ? .saveAs : .save)
        case "v": onCommand?(.paste)
        case "a": onCommand?(.addCapture)
        case "w": onCommand?(.close)
        case "0": zoom(.actual)
        case "9": zoom(.fit)
        case "=", "+": zoom(.zoomIn)
        case "-": zoom(.zoomOut)
        default: return super.performKeyEquivalent(with: event)
        }
        return true
    }

    override func keyDown(with event: NSEvent) {
        let flags = event.modifierFlags.intersection([.command, .control, .option])
        switch event.keyCode {
        case 53: // Esc
            if selectedID != nil { selectedID = nil } else { onCommand?(.close) }
            return
        case 51, 117: // Delete / Forward delete
            deleteSelection()
            return
        case 48: // Tab copies the colour under the pointer
            if let hoverColor {
                Clipboard.copyText(hoverColor.hexString)
                onFlash?("Copied \(hoverColor.hexString)")
            } else {
                NSSound.beep()
            }
            return
        case 123, 124, 125, 126: // arrows nudge the selection
            guard let id = selectedID, let i = annotations.firstIndex(where: { $0.id == id }) else { break }
            let step: CGFloat = event.modifierFlags.contains(.shift) ? 10 : 1
            let d: CGVector = [123: CGVector(dx: -step, dy: 0), 124: CGVector(dx: step, dy: 0),
                               125: CGVector(dx: 0, dy: -step), 126: CGVector(dx: 0, dy: step)][Int(event.keyCode)]!
            pushUndo()
            annotations[i].move(by: d)
            saveToDocument()
            return
        default:
            break
        }
        if flags.isEmpty, let key = event.charactersIgnoringModifiers?.lowercased(),
           let t = EditorTool.allCases.first(where: { $0.key == key }) {
            tool = t
            return
        }
        super.keyDown(with: event)
    }
}

// MARK: - Toolbar

@MainActor
final class EditorToolbar: NSView {
    static let height: CGFloat = 40
    static let minWidth: CGFloat = 820

    var onTool: ((EditorTool) -> Void)?
    var onColor: ((NSColor) -> Void)?
    var onWidth: ((CGFloat) -> Void)?
    var onCommand: ((EditorWindowController.Command) -> Void)?
    var onZoom: ((EditorCanvas.ZoomAction) -> Void)?
    private var zoom: CGFloat = 1

    private var toolButtons: [EditorTool: HUDButton] = [:]
    private var moreButton: HUDButton!
    private var styleButton: StyleButton!
    private let swatch = NSView()
    private let hexLabel = Theme.label("—", font: Theme.mono(11, .semibold), color: .white)
    private let hexHint = Theme.label("Tab to copy", font: Theme.sans(9.5), color: Theme.muted)
    private let sizeLabel = Theme.label("", font: Theme.mono(11, .semibold), color: .white)
    private let sizeHint = Theme.label("Image size", font: Theme.sans(9.5), color: Theme.muted)
    private var flashWork: DispatchWorkItem?
    private var currentTool: EditorTool = .arrow

    static let colors: [(String, NSColor)] = [
        ("Red", NSColor(srgbRed: 1, green: 0.23, blue: 0.19, alpha: 1)),
        ("Orange", NSColor(srgbRed: 1, green: 0.58, blue: 0, alpha: 1)),
        ("Yellow", NSColor(srgbRed: 1, green: 0.8, blue: 0, alpha: 1)),
        ("Green", NSColor(srgbRed: 0.2, green: 0.78, blue: 0.35, alpha: 1)),
        ("Blue", NSColor(srgbRed: 0, green: 0.48, blue: 1, alpha: 1)),
        ("Purple", NSColor(srgbRed: 0.69, green: 0.32, blue: 0.87, alpha: 1)),
        ("White", .white),
        ("Black", .black),
    ]

    override var mouseDownCanMoveWindow: Bool { true }

    func build(document: CaptureDocument) {
        wantsLayer = true
        layer?.backgroundColor = NSColor(white: 0.12, alpha: 1).cgColor
        let line = NSView()
        line.wantsLayer = true
        line.layer?.backgroundColor = NSColor(white: 1, alpha: 0.08).cgColor
        line.translatesAutoresizingMaskIntoConstraints = false
        addSubview(line)
        NSLayoutConstraint.activate([
            line.leadingAnchor.constraint(equalTo: leadingAnchor), line.trailingAnchor.constraint(equalTo: trailingAnchor),
            line.bottomAnchor.constraint(equalTo: bottomAnchor), line.heightAnchor.constraint(equalToConstant: 1),
        ])

        var x: CGFloat = 80   // clear of the traffic lights
        func place(_ v: NSView, width: CGFloat = 30) {
            v.frame = NSRect(x: x, y: (Self.height - 28) / 2, width: width, height: 28)
            v.autoresizingMask = [.maxXMargin]
            addSubview(v)
            x += width + 2
        }
        func separator() {
            let s = NSView(frame: NSRect(x: x + 4, y: 11, width: 1, height: Self.height - 22))
            s.wantsLayer = true
            s.layer?.backgroundColor = NSColor(white: 1, alpha: 0.14).cgColor
            addSubview(s)
            x += 11
        }

        place(HUDButton(symbol: "doc.on.doc", size: 13, help: "Copy  ⌘C") { [weak self] in self?.onCommand?(.copy) })
        place(HUDButton(symbol: "square.and.arrow.down", size: 13, help: "Save to Vault  ⌘S") { [weak self] in self?.onCommand?(.save) })
        place(HUDButton(symbol: "pin", size: 13, help: "Pin to Screen") { [weak self] in self?.onCommand?(.pin) })
        separator()
        let grip = DragGrip(frame: .zero)
        grip.document = document
        place(grip, width: 34)
        separator()

        for tool in EditorTool.bar {
            let b = HUDButton(symbol: tool.symbol, size: 13, help: "\(tool.title)  \(tool.key.uppercased())") { [weak self] in
                self?.onTool?(tool)
            }
            toolButtons[tool] = b
            place(b)
        }
        styleButton = StyleButton { [weak self] in self?.showStyleMenu() }
        styleButton.toolTip = "Colour & thickness"
        place(styleButton)
        place(HUDButton(symbol: "plus.viewfinder", size: 13, help: "Add Capture  ⌘A") { [weak self] in self?.onCommand?(.addCapture) })
        let blur = HUDButton(symbol: EditorTool.blur.symbol, size: 13, help: "Blur/Erase  B") { [weak self] in self?.onTool?(.blur) }
        toolButtons[.blur] = blur
        place(blur)
        moreButton = HUDButton(symbol: "ellipsis", size: 13, help: "More tools") { [weak self] in self?.showMoreMenu() }
        place(moreButton)

        // Right side: colour under the pointer, then the image size.
        let info = NSStackView()
        info.orientation = .horizontal
        info.spacing = 10
        info.alignment = .centerY
        info.translatesAutoresizingMaskIntoConstraints = false
        swatch.wantsLayer = true
        swatch.layer?.cornerRadius = 8
        swatch.layer?.borderWidth = 1
        swatch.layer?.borderColor = NSColor(white: 1, alpha: 0.25).cgColor
        swatch.layer?.backgroundColor = NSColor(white: 0.2, alpha: 1).cgColor
        swatch.translatesAutoresizingMaskIntoConstraints = false
        swatch.widthAnchor.constraint(equalToConstant: 16).isActive = true
        swatch.heightAnchor.constraint(equalToConstant: 16).isActive = true
        let hexStack = NSStackView(views: [hexLabel, hexHint])
        hexStack.orientation = .vertical
        hexStack.alignment = .leading
        hexStack.spacing = 0
        hexLabel.widthAnchor.constraint(greaterThanOrEqualToConstant: 66).isActive = true
        let divider = NSView()
        divider.wantsLayer = true
        divider.layer?.backgroundColor = NSColor(white: 1, alpha: 0.14).cgColor
        divider.translatesAutoresizingMaskIntoConstraints = false
        divider.widthAnchor.constraint(equalToConstant: 1).isActive = true
        divider.heightAnchor.constraint(equalToConstant: 22).isActive = true
        let s = document.size
        sizeLabel.stringValue = "\(Int(s.width.rounded()))×\(Int(s.height.rounded()))pt"
        let sizeStack = NSStackView(views: [sizeLabel, sizeHint])
        sizeStack.orientation = .vertical
        sizeStack.alignment = .leading
        sizeStack.spacing = 0
        sizeStack.toolTip = "Click: toggle 100% / fit  ·  ⌘0 actual size, ⌘9 fit, ⌘+ ⌘− zoom"
        sizeStack.addGestureRecognizer(NSClickGestureRecognizer(target: self, action: #selector(toggleZoom)))
        [swatch, hexStack, divider, sizeStack].forEach(info.addArrangedSubview)
        addSubview(info)
        NSLayoutConstraint.activate([
            info.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -14),
            info.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
        showHover(nil)
    }

    func select(tool: EditorTool, color: NSColor, width: CGFloat) {
        currentTool = tool
        for (t, b) in toolButtons { b.isOn = t == tool }
        let inMenu = EditorTool.menu.contains(tool) && tool != .blur
        moreButton.isOn = inMenu
        moreButton.setSymbol(inMenu ? tool.symbol : "ellipsis")
        styleButton.color = color
        styleButton.lineWidth = width
    }

    func showZoom(_ z: CGFloat) {
        zoom = z
        sizeHint.stringValue = "Image size · \(Int((z * 100).rounded()))%"
    }

    @objc private func toggleZoom() {
        onZoom?(abs(zoom - 1) < 0.001 ? .fit : .actual)
    }

    func showHover(_ color: NSColor?) {
        if flashWork != nil { return }
        if let color {
            swatch.layer?.backgroundColor = color.cgColor
            hexLabel.stringValue = color.hexString
            hexHint.stringValue = "Tab to copy"
        } else {
            swatch.layer?.backgroundColor = NSColor(white: 0.2, alpha: 1).cgColor
            hexLabel.stringValue = "—"
            hexHint.stringValue = "Hover the image"
        }
    }

    func flash(_ text: String) {
        flashWork?.cancel()
        hexLabel.stringValue = text
        hexHint.stringValue = " "
        let work = DispatchWorkItem { [weak self] in
            self?.flashWork = nil
            self?.showHover(nil)
        }
        flashWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.4, execute: work)
    }

    private func showMoreMenu() {
        let menu = NSMenu()
        menu.appearance = NSAppearance(named: .darkAqua)
        menu.autoenablesItems = false
        func item(_ title: String, _ symbol: String, key: String, mods: NSEvent.ModifierFlags, on: Bool = false,
                  _ run: @escaping () -> Void) {
            let i = ClosureMenuItem(title: title, key: key, modifiers: mods, run: run)
            i.image = Theme.symbol(symbol, size: 13)
            i.state = on ? .on : .off
            menu.addItem(i)
        }
        item("Add Capture", "plus.viewfinder", key: "a", mods: .command) { [weak self] in self?.onCommand?(.addCapture) }
        for tool in EditorTool.menu {
            item(tool.title, tool.symbol, key: tool.key, mods: [], on: currentTool == tool) { [weak self] in self?.onTool?(tool) }
        }
        item("Paste Image", "photo.on.rectangle", key: "v", mods: .command) { [weak self] in self?.onCommand?(.paste) }
        menu.addItem(.separator())
        item("Actual Size (100%)", "1.magnifyingglass", key: "0", mods: .command) { [weak self] in self?.onZoom?(.actual) }
        item("Zoom to Fit", "arrow.up.left.and.arrow.down.right", key: "9", mods: .command) { [weak self] in self?.onZoom?(.fit) }
        item("Zoom In", "plus.magnifyingglass", key: "=", mods: .command) { [weak self] in self?.onZoom?(.zoomIn) }
        item("Zoom Out", "minus.magnifyingglass", key: "-", mods: .command) { [weak self] in self?.onZoom?(.zoomOut) }
        menu.addItem(.separator())
        item("Copy Text", "text.viewfinder", key: "", mods: []) { [weak self] in self?.onCommand?(.copyText) }
        item("Save As…", "square.and.arrow.down.on.square", key: "s", mods: [.command, .shift]) { [weak self] in
            self?.onCommand?(.saveAs)
        }
        menu.popUp(positioning: nil, at: NSPoint(x: moreButton.frame.minX, y: moreButton.frame.minY - 4), in: self)
    }

    private func showStyleMenu() {
        let menu = NSMenu()
        menu.appearance = NSAppearance(named: .darkAqua)
        for (name, color) in Self.colors {
            let i = ClosureMenuItem(title: name) { [weak self] in self?.onColor?(color) }
            i.image = NSImage(size: NSSize(width: 14, height: 14), flipped: false) { r in
                color.setFill()
                NSBezierPath(ovalIn: r.insetBy(dx: 1, dy: 1)).fill()
                NSColor(white: 1, alpha: 0.3).setStroke()
                NSBezierPath(ovalIn: r.insetBy(dx: 1, dy: 1)).stroke()
                return true
            }
            i.state = styleButton.color.hexString == color.hexString ? .on : .off
            menu.addItem(i)
        }
        menu.addItem(.separator())
        for (name, w) in [("Thin", CGFloat(2)), ("Medium", 4), ("Thick", 7)] {
            let i = ClosureMenuItem(title: name) { [weak self] in self?.onWidth?(w) }
            i.state = styleButton.lineWidth == w ? .on : .off
            menu.addItem(i)
        }
        menu.popUp(positioning: nil, at: NSPoint(x: styleButton.frame.minX, y: styleButton.frame.minY - 4), in: self)
    }
}

/// The colour & thickness button: a filled dot in the current colour.
@MainActor
final class StyleButton: NSView {
    var color: NSColor = .red { didSet { needsDisplay = true } }
    var lineWidth: CGFloat = 4 { didSet { needsDisplay = true } }
    private let action: () -> Void
    private var hovering = false { didSet { needsDisplay = true } }

    init(action: @escaping () -> Void) {
        self.action = action
        super.init(frame: .zero)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func updateTrackingAreas() {
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.activeAlways, .mouseEnteredAndExited, .inVisibleRect], owner: self))
    }

    override func mouseEntered(with event: NSEvent) { hovering = true }
    override func mouseExited(with event: NSEvent) { hovering = false }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override func mouseUp(with event: NSEvent) {
        if bounds.contains(convert(event.locationInWindow, from: nil)) { action() }
    }

    override func draw(_ dirtyRect: NSRect) {
        if hovering {
            NSColor(white: 1, alpha: 0.1).setFill()
            NSBezierPath(roundedRect: bounds, xRadius: 6, yRadius: 6).fill()
        }
        let d: CGFloat = 8 + lineWidth * 1.2
        let r = NSRect(x: bounds.midX - d / 2, y: bounds.midY - d / 2, width: d, height: d)
        color.setFill()
        NSBezierPath(ovalIn: r).fill()
        NSColor(white: 1, alpha: 0.45).setStroke()
        let ring = NSBezierPath(ovalIn: r.insetBy(dx: -2.5, dy: -2.5))
        ring.lineWidth = 1
        ring.stroke()
    }
}
