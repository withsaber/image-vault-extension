import AppKit
import QuartzCore

/// Pure black & white design tokens, shared with the Chrome extension's look.
enum Theme {
    static let ink = NSColor.black
    static let paper = NSColor.white
    static let soft = NSColor(white: 0.898, alpha: 1)   // #E5E5E5
    static let muted = NSColor(white: 0.55, alpha: 1)   // #8C8C8C
    static let dim = NSColor(white: 0.29, alpha: 1)     // #4A4A4A
    static let card = NSColor(white: 0.047, alpha: 1)   // #0C0C0C
    static let line = NSColor(white: 1, alpha: 0.12)
    static let line2 = NSColor(white: 1, alpha: 0.26)

    /// cubic-bezier(0.22, 1, 0.36, 1): fast out, long gentle settle.
    static let ease = CAMediaTimingFunction(controlPoints: 0.22, 1, 0.36, 1)

    static let panelWidth: CGFloat = 440
    static let tabWidth: CGFloat = 28
    static let tabHeight: CGFloat = 116

    static func sans(_ size: CGFloat, _ weight: NSFont.Weight = .regular) -> NSFont {
        .systemFont(ofSize: size, weight: weight)
    }

    static func mono(_ size: CGFloat, _ weight: NSFont.Weight = .regular) -> NSFont {
        .monospacedSystemFont(ofSize: size, weight: weight)
    }

    static func symbol(_ name: String, size: CGFloat = 13, weight: NSFont.Weight = .regular) -> NSImage? {
        NSImage(systemSymbolName: name, accessibilityDescription: nil)?
            .withSymbolConfiguration(.init(pointSize: size, weight: weight))
    }

    static func label(_ text: String, font: NSFont, color: NSColor = soft) -> NSTextField {
        let field = NSTextField(labelWithString: text)
        field.font = font
        field.textColor = color
        field.lineBreakMode = .byTruncatingTail
        field.translatesAutoresizingMaskIntoConstraints = false
        return field
    }

    /// Wide-tracked uppercase label (VAULT, DROP TO VAULT...).
    static func caps(_ text: String, size: CGFloat, color: NSColor = paper) -> NSAttributedString {
        NSAttributedString(string: text.uppercased(), attributes: [
            .font: sans(size, .bold), .kern: size * 0.22, .foregroundColor: color,
        ])
    }

    static func animate(_ duration: TimeInterval = 0.32, _ changes: () -> Void, completion: (() -> Void)? = nil) {
        NSAnimationContext.runAnimationGroup({ ctx in
            ctx.duration = duration
            ctx.timingFunction = ease
            ctx.allowsImplicitAnimation = true
            changes()
        }, completionHandler: completion)
    }

    /// Springy pop for micro-feedback (scale snap on copy/drop/select).
    static func pop(_ layer: CALayer, from: CGFloat = 0.85) {
        let spring = CASpringAnimation(keyPath: "transform.scale")
        spring.fromValue = from
        spring.toValue = 1
        spring.damping = 12
        spring.stiffness = 260
        spring.mass = 0.8
        spring.duration = spring.settlingDuration
        layer.add(spring, forKey: "pop")
    }
}

/// Borderless floating panel that sits above every app and Space and never
/// activates Image Vault unless it needs the keyboard (pinned gallery).
final class FloatingPanel: NSPanel {
    var acceptsKey = false

    init(frame: NSRect) {
        super.init(contentRect: frame, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        isFloatingPanel = true
        level = .statusBar
        collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]
        backgroundColor = .clear
        isOpaque = false
        hasShadow = false
        hidesOnDeactivate = false
        isMovable = false
        isReleasedWhenClosed = false
        animationBehavior = .none
    }

    override var canBecomeKey: Bool { acceptsKey }
    override var canBecomeMain: Bool { false }
}
