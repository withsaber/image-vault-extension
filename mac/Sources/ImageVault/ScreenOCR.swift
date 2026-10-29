import AppKit
import ScreenCaptureKit

/// Shotr-style region picker shared by "capture text" (OCR → clipboard) and
/// screenshots (→ floating preview). Drag a box anywhere on any display; in
/// screenshot mode a plain click takes the whole display. Esc, right-click or
/// the shortcut again cancels.
///
/// One borderless overlay per display (so it works on every monitor and in
/// full-screen Spaces). Nothing is drawn until you drag: no guide lines, no dim
/// — just the crosshair cursor. While dragging, the outside is dimmed lightly
/// and the box shows its size.
@MainActor
final class ScreenOCRController {
    static let shared = ScreenOCRController()

    private var windows: [ScreenOCRWindow] = []
    private var previousApp: NSRunningApplication?
    private var keyMonitor: Any?
    private var onImage: ((CGImage, NSScreen) -> Void)?
    private var onCancel: (() -> Void)?
    private(set) var clickTakesDisplay = false
    var isActive: Bool { !windows.isEmpty }

    private init() {}

    func toggle() { isActive ? cancel() : begin() }

    /// Screenshot mode: the captured pixels go to `onImage` instead of OCR.
    func toggleScreenshot(_ onImage: @escaping (CGImage, NSScreen) -> Void) {
        isActive ? cancel() : pickImage(onImage: onImage)
    }

    func pickImage(onImage: @escaping (CGImage, NSScreen) -> Void, onCancel: (() -> Void)? = nil) {
        guard windows.isEmpty else { return }
        begin()
        self.onImage = onImage
        self.onCancel = onCancel
        clickTakesDisplay = true
    }

    func begin() {
        guard windows.isEmpty else { return }
        onImage = nil
        onCancel = nil
        clickTakesDisplay = false
        let front = NSWorkspace.shared.frontmostApplication
        previousApp = front?.processIdentifier == getpid() ? nil : front

        for screen in NSScreen.screens {
            let window = ScreenOCRWindow(screen: screen, controller: self)
            window.orderFrontRegardless()
            windows.append(window)
        }
        // The overlay must be key to receive Esc; that needs the app active.
        NSApp.activate(ignoringOtherApps: true)
        let mouse = NSEvent.mouseLocation
        (windows.first { $0.frame.contains(mouse) } ?? windows.first)?.makeKey()
        OCRCursor.shared.set()

        // Belt and braces: Esc cancels even if another window grabbed key status.
        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            if event.keyCode == 53 { self?.cancel(); return nil }
            return event
        }
    }

    func cancel() {
        let cancelled = onCancel
        onImage = nil
        onCancel = nil
        tearDown()
        if let cancelled { cancelled() } else { previousApp?.activate() }
        previousApp = nil
    }

    private func tearDown() {
        if let keyMonitor { NSEvent.removeMonitor(keyMonitor) }
        keyMonitor = nil
        windows.forEach { $0.orderOut(nil) }
        windows.removeAll()
        NSCursor.arrow.set()
    }

    /// `rect` is in global Cocoa coordinates (origin bottom-left of the main display).
    func finish(rect: NSRect, on screen: NSScreen) {
        let imageHandler = onImage
        let cancelHandler = onCancel
        onImage = nil
        onCancel = nil
        tearDown()
        let app = previousApp
        previousApp = nil
        // The preview never takes focus, so hand it straight back. (Add Capture
        // re-activates the editor itself.)
        app?.activate()
        guard rect.width >= 4, rect.height >= 4 else { cancelHandler?(); return }

        guard CGPreflightScreenCaptureAccess() else {
            CGRequestScreenCaptureAccess()
            IslandController.shared?.showActivity(image: nil, title: "Allow Screen Recording",
                                                  detail: "System Settings → Privacy → Screen Recording")
            cancelHandler?()
            return
        }

        if let imageHandler {
            Task { @MainActor in
                try? await Task.sleep(nanoseconds: 60_000_000)
                guard let image = await Self.capture(rect: rect, screen: screen) else {
                    IslandController.shared?.showActivity(image: nil, title: "Capture failed", detail: "")
                    cancelHandler?()
                    return
                }
                imageHandler(image, screen)
            }
            return
        }

        Task { @MainActor in
            // Let the overlay leave the screen before grabbing pixels.
            try? await Task.sleep(nanoseconds: 60_000_000)
            guard let image = await Self.capture(rect: rect, screen: screen) else {
                IslandController.shared?.showActivity(image: nil, title: "Capture failed", detail: "")
                return
            }
            let url = FileManager.default.temporaryDirectory.appendingPathComponent("ImageVault-ocr-\(UUID().uuidString).png")
            let rep = NSBitmapImageRep(cgImage: image)
            guard let png = rep.representation(using: .png, properties: [:]), (try? png.write(to: url)) != nil else { return }
            let thumb = NSImage(cgImage: image, size: NSSize(width: image.width, height: image.height))
            IslandController.shared?.showActivity(image: thumb, title: "Reading text…", detail: "")

            let text = await Task.detached(priority: .userInitiated) { OCR.recognize(at: url) }.value
            try? FileManager.default.removeItem(at: url)
            if let text, !text.isEmpty {
                Clipboard.copyText(text)
                IslandController.shared?.showActivity(image: thumb, title: "Text copied", detail: "\(text.count) characters")
            } else {
                IslandController.shared?.showActivity(image: thumb, title: "No text found", detail: "")
            }
        }
    }

    // MARK: Capture

    static func capture(rect: NSRect, screen: NSScreen) async -> CGImage? {
        let scale = screen.backingScaleFactor
        // Display-local rect, top-left origin (what both capture APIs expect),
        // snapped to whole device pixels so nothing is resampled or blurred.
        let x0 = ((rect.minX - screen.frame.minX) * scale).rounded()
        let x1 = ((rect.maxX - screen.frame.minX) * scale).rounded()
        let y0 = ((screen.frame.maxY - rect.maxY) * scale).rounded()
        let y1 = ((screen.frame.maxY - rect.minY) * scale).rounded()
        let pixelWidth = max(1, Int(x1 - x0)), pixelHeight = max(1, Int(y1 - y0))
        let local = CGRect(x: x0 / scale, y: y0 / scale, width: CGFloat(pixelWidth) / scale, height: CGFloat(pixelHeight) / scale)
        if #available(macOS 14.0, *) {
            do {
                let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
                let id = IslandController.id(of: screen)
                guard let display = content.displays.first(where: { $0.displayID == id }) else { return nil }
                let own = content.applications.filter { $0.processID == getpid() }
                let filter = SCContentFilter(display: display, excludingApplications: own, exceptingWindows: [])
                let config = SCStreamConfiguration()
                config.sourceRect = local
                // Exactly one output pixel per screen pixel, at the display's
                // native resolution and colours.
                config.captureResolution = .best
                config.width = pixelWidth
                config.height = pixelHeight
                config.scalesToFit = false
                config.showsCursor = false
                if let space = screen.colorSpace?.cgColorSpace?.name { config.colorSpaceName = space }
                return try await SCScreenshotManager.captureImage(contentFilter: filter, configuration: config)
            } catch {
                return nil
            }
        }
        // macOS 13: CoreGraphics global coordinates are top-left of the main display.
        let mainHeight = NSScreen.screens.first?.frame.height ?? screen.frame.height
        let cgRect = CGRect(x: screen.frame.minX + local.minX, y: mainHeight - screen.frame.maxY + local.minY,
                            width: local.width, height: local.height)
        return CGWindowListCreateImage(cgRect, .optionOnScreenOnly, kCGNullWindowID, [.bestResolution])
    }
}

/// The crosshair from the design: white plus with a hairline outline and a
/// soft translucent dot in the middle. No guide lines across the screen.
@MainActor
final class OCRCursor {
    static let shared = OCRCursor()
    let cursor: NSCursor

    private init() {
        let size: CGFloat = 34
        let image = NSImage(size: NSSize(width: size, height: size), flipped: false) { rect in
            let c = NSPoint(x: rect.midX, y: rect.midY)
            // Soft dot
            let dot = NSBezierPath(ovalIn: NSRect(x: c.x - 9, y: c.y - 9, width: 18, height: 18))
            NSColor(white: 0.55, alpha: 0.45).setFill()
            dot.fill()
            NSColor(white: 1, alpha: 0.55).setStroke()
            dot.lineWidth = 1.2
            dot.stroke()
            // Plus: black outline under a white bar, for contrast on any background.
            let arm: CGFloat = 15
            for (width, color) in [(CGFloat(4.6), NSColor.black), (CGFloat(2.4), NSColor.white)] {
                let plus = NSBezierPath()
                plus.move(to: NSPoint(x: c.x - arm, y: c.y)); plus.line(to: NSPoint(x: c.x + arm, y: c.y))
                plus.move(to: NSPoint(x: c.x, y: c.y - arm)); plus.line(to: NSPoint(x: c.x, y: c.y + arm))
                plus.lineWidth = width
                plus.lineCapStyle = .butt
                color.setStroke()
                plus.stroke()
            }
            return true
        }
        cursor = NSCursor(image: image, hotSpot: NSPoint(x: size / 2, y: size / 2))
    }

    func set() { cursor.set() }
}

/// Borderless per-display overlay. Fully transparent until a drag starts.
final class ScreenOCRWindow: NSWindow {
    init(screen: NSScreen, controller: ScreenOCRController) {
        super.init(contentRect: screen.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        setFrame(screen.frame, display: false)
        level = NSWindow.Level(rawValue: Int(CGWindowLevelForKey(.screenSaverWindow)) + 1)
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]
        backgroundColor = .clear
        isOpaque = false
        hasShadow = false
        isReleasedWhenClosed = false
        animationBehavior = .none
        ignoresMouseEvents = false      // receive clicks even on transparent pixels
        acceptsMouseMovedEvents = true
        contentView = ScreenOCRView(frame: NSRect(origin: .zero, size: screen.frame.size), screen: screen, controller: controller)
    }

    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
}

final class ScreenOCRView: NSView {
    private weak var controller: ScreenOCRController?
    private let screenRef: NSScreen
    private var start: NSPoint?
    private var current: NSPoint?

    init(frame: NSRect, screen: NSScreen, controller: ScreenOCRController) {
        self.screenRef = screen
        self.controller = controller
        super.init(frame: frame)
    }

    required init?(coder: NSCoder) { fatalError() }

    override var acceptsFirstResponder: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    private var selection: NSRect? {
        guard let start, let current else { return nil }
        return NSRect(x: min(start.x, current.x), y: min(start.y, current.y),
                      width: abs(current.x - start.x), height: abs(current.y - start.y))
    }

    // MARK: Cursor

    override func resetCursorRects() {
        addCursorRect(bounds, cursor: MainActor.assumeIsolated { OCRCursor.shared.cursor })
    }

    override func updateTrackingAreas() {
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.activeAlways, .cursorUpdate, .mouseMoved, .inVisibleRect], owner: self))
    }

    override func cursorUpdate(with event: NSEvent) { MainActor.assumeIsolated { OCRCursor.shared.set() } }
    override func mouseMoved(with event: NSEvent) { MainActor.assumeIsolated { OCRCursor.shared.set() } }

    // MARK: Mouse & keys

    override func mouseDown(with event: NSEvent) {
        window?.makeKey()
        start = convert(event.locationInWindow, from: nil)
        current = start
        needsDisplay = true
    }

    override func mouseDragged(with event: NSEvent) {
        guard start != nil else { return }
        var p = convert(event.locationInWindow, from: nil)
        p.x = min(max(p.x, 0), bounds.width)
        p.y = min(max(p.y, 0), bounds.height)
        current = p
        needsDisplay = true
        MainActor.assumeIsolated { OCRCursor.shared.set() }
    }

    override func mouseUp(with event: NSEvent) {
        guard let rect = selection, let window else { start = nil; current = nil; return }
        start = nil
        current = nil
        needsDisplay = true
        let screen = screenRef
        // A plain click keeps OCR mode open; in screenshot mode it takes the display.
        guard rect.width >= 4, rect.height >= 4 else {
            if MainActor.assumeIsolated({ controller?.clickTakesDisplay == true }) {
                MainActor.assumeIsolated { controller?.finish(rect: screen.frame, on: screen) }
            }
            return
        }
        let global = window.convertToScreen(convert(rect, to: nil))
        MainActor.assumeIsolated { controller?.finish(rect: global, on: screen) }
    }

    override func rightMouseDown(with event: NSEvent) {
        MainActor.assumeIsolated { controller?.cancel() }
    }

    override func keyDown(with event: NSEvent) {
        if event.keyCode == 53 { MainActor.assumeIsolated { controller?.cancel() } }
        else { super.keyDown(with: event) }
    }

    override func cancelOperation(_ sender: Any?) {
        MainActor.assumeIsolated { controller?.cancel() }
    }

    // MARK: Drawing (only while dragging)

    override func draw(_ dirtyRect: NSRect) {
        guard let rect = selection, rect.width > 0 || rect.height > 0 else { return }
        // Light dim outside the box so the selection reads clearly.
        let outside = NSBezierPath(rect: bounds)
        outside.append(NSBezierPath(rect: rect))
        outside.windingRule = .evenOdd
        NSColor(white: 0, alpha: 0.22).setFill()
        outside.fill()

        // Box: white hairline over a soft dark outline.
        NSColor(white: 0, alpha: 0.35).setStroke()
        let shadow = NSBezierPath(rect: rect.insetBy(dx: -1, dy: -1))
        shadow.lineWidth = 1
        shadow.stroke()
        NSColor.white.setStroke()
        let box = NSBezierPath(rect: rect.insetBy(dx: 0.5, dy: 0.5))
        box.lineWidth = 1
        box.stroke()

        // Size label under the box (or inside if there's no room).
        let label = "\(Int(rect.width)) × \(Int(rect.height))" as NSString
        let attrs: [NSAttributedString.Key: Any] = [
            .font: NSFont.monospacedSystemFont(ofSize: 11, weight: .medium), .foregroundColor: NSColor.white,
        ]
        let size = label.size(withAttributes: attrs)
        var pill = NSRect(x: rect.maxX - size.width - 12, y: rect.minY - size.height - 12,
                          width: size.width + 12, height: size.height + 6)
        if pill.minY < 4 { pill.origin.y = rect.minY + 6 }
        if pill.minX < 4 { pill.origin.x = rect.minX }
        NSColor(white: 0, alpha: 0.75).setFill()
        NSBezierPath(roundedRect: pill, xRadius: pill.height / 2, yRadius: pill.height / 2).fill()
        label.draw(at: NSPoint(x: pill.minX + 6, y: pill.minY + 3), withAttributes: attrs)
    }
}
