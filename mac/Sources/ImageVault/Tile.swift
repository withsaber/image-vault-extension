import AppKit
import QuartzCore

extension NSImage {
    /// SF Symbol rendered in a flat colour (for CALayer contents).
    static func symbol(_ name: String, size: CGFloat, weight: NSFont.Weight = .medium, color: NSColor) -> NSImage? {
        guard let base = Theme.symbol(name, size: size, weight: weight) else { return nil }
        return NSImage(size: base.size, flipped: false) { rect in
            base.draw(in: rect)
            color.set()
            rect.fill(using: .sourceAtop)
            return true
        }
    }
}

/// Decoded GIF frames for live tiles, shared between cells.
final class GIFFrames {
    static let shared = GIFFrames()
    private let cache = NSCache<NSString, Box>()
    private let queue = DispatchQueue(label: "imagevault.gif", qos: .userInitiated)
    final class Box { let frames: [CGImage]; let durations: [Double]; init(_ f: [CGImage], _ d: [Double]) { frames = f; durations = d } }

    init() { cache.countLimit = 40 }

    func load(_ item: VaultItem, maxPixel: Int = 240, completion: @escaping (Box?) -> Void) {
        let key = "\(item.id)@\(maxPixel)" as NSString
        if let hit = cache.object(forKey: key) { return completion(hit) }
        let url = VaultStore.shared.fileURL(for: item)
        queue.async { [cache] in
            let box = Imaging.gifFrames(at: url, maxPixel: maxPixel).map { Box($0.frames, $0.durations) }
            if let box { cache.setObject(box, forKey: key) }
            DispatchQueue.main.async { completion(box) }
        }
    }
}

/// Round hover button drawn with layers (copy, edit, text, code).
final class TileButton {
    let layer = CALayer()
    let icon = CALayer()
    var symbol: String { didSet { redraw() } }
    let size: CGFloat
    var inverted = false { didSet { redraw() } }

    init(_ symbol: String, size: CGFloat) {
        self.symbol = symbol
        self.size = size
        layer.cornerRadius = size / 2
        layer.borderColor = Theme.line2.cgColor
        layer.borderWidth = 1
        layer.opacity = 0
        icon.contentsGravity = .center
        layer.addSublayer(icon)
        redraw()
    }

    private func redraw() {
        layer.backgroundColor = (inverted ? Theme.paper : NSColor(white: 0, alpha: 0.72)).cgColor
        icon.contents = NSImage.symbol(symbol, size: size * 0.43, weight: inverted ? .bold : .medium,
                                       color: inverted ? .black : .white)
    }

    func place(_ rect: CGRect) {
        layer.frame = rect
        icon.frame = layer.bounds
    }
}

/// One grid tile. All moving parts are CALayers so hover / select / copy
/// micro-interactions animate on the GPU. Everything sits in `content`, which
/// is what flies in and out of a folder.
final class TileView: NSView {
    private let content = CALayer()
    private let card = CALayer()
    private let checker = CALayer()
    private let imageLayer = CALayer()
    private let ring = CALayer()
    private let badge = CALayer()
    private let badgeCheck = CALayer()
    private let kindBadge = CATextLayer()
    // Hover: a 2×2 action grid (your sketch). Select · Copy / Edit · Text (or SVG code).
    private let hoverVeil = CALayer()
    private let crossH = CALayer()
    private let crossV = CALayer()
    private let selectButton = TileButton("circle", size: 21)
    private let copyButton = TileButton("doc.on.doc", size: 21)
    private let editButton = TileButton("pencil", size: 21)
    private let extraButton = TileButton("text.alignleft", size: 21)
    private var buttons: [TileButton] { [selectButton, copyButton, editButton, extraButton] }
    private var hotButton: Int? { didSet { if hotButton != oldValue { refreshHot() } } }
    var dimmed = false { didSet { if dimmed != oldValue { refresh() } } }
    private let metaShade = CAGradientLayer()
    private let metaName = CATextLayer()
    private let metaDims = CATextLayer()
    private let copiedOverlay = CALayer()
    private let copiedDisc = CALayer()
    private let copiedCheck = CAShapeLayer()
    private let copiedLabel = CATextLayer()

    var onCopy: (() -> Void)?
    var onCopyText: (() -> Void)?
    var onCopyCode: (() -> Void)?
    var onEdit: (() -> Void)?
    var onToggleSelect: (() -> Void)?

    var hovered = false { didSet { if hovered != oldValue { refresh() } } }
    var selected = false { didSet { if selected != oldValue { refresh(); if selected { Theme.pop(badge, from: 0.6) } } } }
    var previewSelected = false { didSet { if previewSelected != oldValue { refresh() } } }
    /// Another tile is being dragged onto this one (drop = new folder).
    var dropTarget = false { didSet { if dropTarget != oldValue { refresh(); if dropTarget { Theme.pop(card, from: 0.94) } } } }
    private var isSVG = false

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.masksToBounds = false
        layer?.addSublayer(content)

        card.cornerRadius = 10
        card.cornerCurve = .continuous
        card.masksToBounds = true
        card.backgroundColor = Theme.card.cgColor
        content.addSublayer(card)

        checker.backgroundColor = NSColor(patternImage: Self.checkerImage).cgColor
        card.addSublayer(checker)

        imageLayer.contentsGravity = .resizeAspectFill
        imageLayer.masksToBounds = true
        imageLayer.opacity = 0
        card.addSublayer(imageLayer)

        metaShade.colors = [NSColor(white: 0, alpha: 0.88).cgColor, NSColor.clear.cgColor]
        metaShade.startPoint = CGPoint(x: 0.5, y: 0)
        metaShade.endPoint = CGPoint(x: 0.5, y: 1)
        metaShade.opacity = 0
        card.addSublayer(metaShade)
        for (t, size, color) in [(metaName, 11.0, Theme.paper), (metaDims, 9.5, Theme.muted)] {
            t.fontSize = size
            t.foregroundColor = color.cgColor
            t.truncationMode = .end
            metaShade.addSublayer(t)
        }
        metaName.font = Theme.sans(11)
        metaDims.font = Theme.mono(9.5)
        metaDims.alignmentMode = .right

        kindBadge.font = Theme.sans(7.5, .heavy)
        kindBadge.fontSize = 7.5
        kindBadge.alignmentMode = .center
        kindBadge.foregroundColor = NSColor.black.cgColor
        kindBadge.backgroundColor = NSColor(white: 1, alpha: 0.9).cgColor
        kindBadge.cornerRadius = 3
        kindBadge.isHidden = true
        card.addSublayer(kindBadge)

        ring.cornerRadius = 10
        ring.cornerCurve = .continuous
        ring.borderColor = Theme.line.cgColor
        ring.borderWidth = 1
        card.addSublayer(ring)

        // Copy feedback: a dark veil, a white disc that springs in and a
        // check that draws itself.
        copiedOverlay.backgroundColor = NSColor(white: 0, alpha: 0.62).cgColor
        copiedOverlay.opacity = 0
        card.addSublayer(copiedOverlay)
        copiedDisc.backgroundColor = Theme.paper.cgColor
        copiedOverlay.addSublayer(copiedDisc)
        copiedCheck.strokeColor = NSColor.black.cgColor
        copiedCheck.fillColor = nil
        copiedCheck.lineWidth = 2.2
        copiedCheck.lineCap = .round
        copiedCheck.lineJoin = .round
        copiedCheck.strokeEnd = 0
        copiedDisc.addSublayer(copiedCheck)
        copiedLabel.string = "Copied"
        copiedLabel.font = Theme.sans(9, .semibold)
        copiedLabel.fontSize = 9
        copiedLabel.alignmentMode = .center
        copiedLabel.foregroundColor = Theme.paper.cgColor
        copiedOverlay.addSublayer(copiedLabel)

        hoverVeil.backgroundColor = NSColor(white: 0, alpha: 0.58).cgColor
        hoverVeil.opacity = 0
        card.addSublayer(hoverVeil)
        for line in [crossH, crossV] {
            line.backgroundColor = NSColor(white: 1, alpha: 0.22).cgColor
            line.opacity = 0
            hoverVeil.addSublayer(line)
        }

        badge.cornerRadius = 8
        badge.borderWidth = 1.5
        badge.borderColor = NSColor(white: 1, alpha: 0.85).cgColor
        badge.backgroundColor = NSColor(white: 0, alpha: 0.45).cgColor
        badge.opacity = 0
        badgeCheck.contents = NSImage.symbol("checkmark", size: 8, weight: .heavy, color: .black)
        badgeCheck.contentsGravity = .center
        badgeCheck.opacity = 0
        badge.addSublayer(badgeCheck)
        content.addSublayer(badge)

        buttons.forEach {
            $0.layer.borderColor = NSColor(white: 1, alpha: 0.18).cgColor
            content.addSublayer($0.layer)
        }
    }

    required init?(coder: NSCoder) { fatalError() }

    private static let checkerImage: NSImage = NSImage(size: NSSize(width: 14, height: 14), flipped: false) { r in
        Theme.card.setFill(); r.fill()
        NSColor(white: 0.07, alpha: 1).setFill()
        NSRect(x: 0, y: 0, width: 7, height: 7).fill()
        NSRect(x: 7, y: 7, width: 7, height: 7).fill()
        return true
    }

    override func viewDidChangeBackingProperties() {
        let scale = window?.backingScaleFactor ?? 2
        [metaName, metaDims, badgeCheck, kindBadge, copiedLabel, copiedCheck,
         selectButton.icon, copyButton.icon, editButton.icon, extraButton.icon].forEach { $0.contentsScale = scale }
    }

    /// Centre of each quadrant: 0 top-left, 1 top-right, 2 bottom-left, 3 bottom-right.
    private func buttonRect(_ i: Int) -> CGRect {
        let side: CGFloat = 21
        let x = i % 2 == 0 ? bounds.width * 0.25 : bounds.width * 0.75
        let y = i < 2 ? bounds.height * 0.75 : bounds.height * 0.25
        return CGRect(x: x - side / 2, y: y - side / 2, width: side, height: side)
    }

    /// Which action the point is over (the button plus a small margin).
    private func action(at p: CGPoint) -> Int? {
        (0..<4).first { buttonRect($0).insetBy(dx: -5, dy: -5).contains(p) }
    }
    var badgeRect: CGRect { CGRect(x: 7, y: bounds.maxY - 23, width: 16, height: 16) }

    override func layout() {
        super.layout()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        content.bounds = bounds
        content.position = CGPoint(x: bounds.midX, y: bounds.midY)
        card.bounds = bounds
        card.position = CGPoint(x: bounds.midX, y: bounds.midY)
        checker.frame = card.bounds
        imageLayer.bounds = card.bounds
        imageLayer.position = CGPoint(x: bounds.midX, y: bounds.midY)
        ring.frame = card.bounds
        copiedOverlay.frame = card.bounds
        let disc: CGFloat = 26
        let showLabel = bounds.height >= 64
        copiedDisc.frame = CGRect(x: bounds.midX - disc / 2, y: bounds.midY - disc / 2 + (showLabel ? 6 : 0), width: disc, height: disc)
        copiedDisc.cornerRadius = disc / 2
        let check = CGMutablePath()
        check.move(to: CGPoint(x: 7.5, y: 13.5))
        check.addLine(to: CGPoint(x: 11.5, y: 9.5))
        check.addLine(to: CGPoint(x: 18.5, y: 17))
        copiedCheck.path = check
        copiedCheck.frame = copiedDisc.bounds
        copiedLabel.frame = CGRect(x: 0, y: copiedDisc.frame.minY - 16, width: bounds.width, height: 12)
        copiedLabel.isHidden = !showLabel
        metaShade.frame = CGRect(x: 0, y: 0, width: bounds.width, height: 38)
        let roomy = bounds.width >= 120
        metaDims.frame = roomy ? CGRect(x: bounds.width - 78, y: 6, width: 70, height: 13)
                               : CGRect(x: 6, y: 5, width: bounds.width - 12, height: 13)
        metaName.frame = CGRect(x: 8, y: 6, width: max(0, bounds.width - 90), height: 14)
        metaName.isHidden = !roomy
        kindBadge.frame = CGRect(x: 6, y: 6, width: 22, height: 11)
        badge.frame = badgeRect
        badgeCheck.frame = badge.bounds
        hoverVeil.frame = card.bounds
        let inset: CGFloat = 9
        crossH.frame = CGRect(x: inset, y: bounds.midY - 0.25, width: bounds.width - inset * 2, height: 0.5)
        crossV.frame = CGRect(x: bounds.midX - 0.25, y: inset, width: 0.5, height: bounds.height - inset * 2)
        for (i, b) in buttons.enumerated() { b.place(buttonRect(i)) }
        CATransaction.commit()
    }

    func configure(_ item: VaultItem, fresh: Bool) {
        metaName.string = item.name
        metaDims.string = item.width.flatMap { w in item.height.map { "\(w)×\($0)" } } ?? ""
        toolTip = [item.name, item.domain, item.source].compactMap { $0 }.joined(separator: "  ·  ")
        imageLayer.removeAnimation(forKey: "gif")
        imageLayer.contents = nil
        imageLayer.opacity = 0
        isSVG = item.mime == "image/svg+xml" || item.name.lowercased().hasSuffix(".svg")
        let isGIF = item.mime == "image/gif"
        kindBadge.string = isSVG ? "SVG" : (isGIF ? "GIF" : nil)
        kindBadge.isHidden = !(isSVG || isGIF)
        // SVGs are logos and icons: show them whole, not cropped.
        imageLayer.contentsGravity = isSVG ? .resizeAspect : .resizeAspectFill
        extraButton.symbol = isSVG ? "chevron.left.forwardslash.chevron.right" : "text.alignleft"
        let id = item.id
        // Must be set before loading: a cached thumbnail calls back immediately.
        currentID = id
        Thumbnails.shared.load(item) { [weak self] image in
            guard let self, self.currentID == id else { return }
            if self.imageLayer.animation(forKey: "gif") == nil { self.imageLayer.contents = image }
            CATransaction.begin()
            CATransaction.setAnimationDuration(0.28)
            CATransaction.setAnimationTimingFunction(Theme.ease)
            self.imageLayer.opacity = 1
            CATransaction.commit()
        }
        if isGIF {
            // Live preview: the GIF plays right in the grid.
            GIFFrames.shared.load(item) { [weak self] box in
                guard let self, self.currentID == id, let box else { return }
                let anim = Imaging.gifAnimation(box.frames, durations: box.durations)
                self.imageLayer.add(anim, forKey: "gif")
                self.imageLayer.opacity = 1
            }
        }
        if fresh { playEntrance() }
    }

    private(set) var currentID: String?

    private func playEntrance() {
        Theme.pop(card, from: 0.6)
        let flash = CABasicAnimation(keyPath: "borderWidth")
        flash.fromValue = 3
        flash.toValue = 1
        flash.duration = 0.9
        flash.timingFunction = Theme.ease
        ring.add(flash, forKey: "enter")
    }

    func refresh() {
        let on = selected || previewSelected
        CATransaction.begin()
        CATransaction.setAnimationDuration(0.24)
        CATransaction.setAnimationTimingFunction(Theme.ease)
        card.setAffineTransform(dropTarget ? CGAffineTransform(scaleX: 0.9, y: 0.9)
                                : (hovered ? CGAffineTransform(scaleX: 1.03, y: 1.03) : .identity))
        imageLayer.setAffineTransform(on ? CGAffineTransform(scaleX: 0.88, y: 0.88) : .identity)
        ring.borderWidth = on || dropTarget ? 2 : 1
        ring.borderColor = (on || dropTarget ? Theme.paper : (hovered ? Theme.line2 : Theme.line)).cgColor
        card.backgroundColor = (on ? NSColor(white: 0.1, alpha: 1) : Theme.card).cgColor
        let showButtons = hovered && !copied && !dropTarget
        // The corner check marks a selection while the hover grid is away.
        badge.opacity = on && !showButtons ? 1 : 0
        badge.backgroundColor = (on ? Theme.paper : NSColor(white: 0, alpha: 0.45)).cgColor
        badge.borderColor = (on ? Theme.paper : NSColor(white: 1, alpha: 0.85)).cgColor
        badgeCheck.opacity = on ? 1 : 0
        selectButton.symbol = on ? "checkmark.circle.fill" : "circle"
        hoverVeil.opacity = showButtons ? 1 : 0
        crossH.opacity = showButtons ? 1 : 0
        crossV.opacity = showButtons ? 1 : 0
        crossH.setAffineTransform(showButtons ? .identity : CGAffineTransform(scaleX: 0.2, y: 1))
        crossV.setAffineTransform(showButtons ? .identity : CGAffineTransform(scaleX: 1, y: 0.2))
        for b in buttons {
            b.layer.opacity = showButtons ? 1 : 0
            b.layer.setAffineTransform(showButtons ? .identity : CGAffineTransform(scaleX: 0.6, y: 0.6))
        }
        metaShade.opacity = 0
        kindBadge.opacity = showButtons ? 0 : 1
        // A folder is open: everything outside it steps back.
        content.opacity = dimmed && !hovered ? 0.3 : 1
        CATransaction.commit()
        if !showButtons { hotButton = nil }
    }

    private func refreshHot() {
        for (i, b) in buttons.enumerated() {
            b.inverted = i == hotButton
            CATransaction.begin()
            CATransaction.setAnimationDuration(0.16)
            b.layer.setAffineTransform(i == hotButton ? CGAffineTransform(scaleX: 1.14, y: 1.14) : .identity)
            CATransaction.commit()
        }
    }

    private var copied = false
    private var copiedToken = 0

    /// Overlay copy animation: veil fades in, disc springs up, the check
    /// draws, then it all melts away.
    func showCopied(label: String = "Copied") {
        copied = true
        copiedToken += 1
        let token = copiedToken
        copiedLabel.string = label
        refresh()
        CATransaction.begin()
        CATransaction.setAnimationDuration(0.16)
        copiedOverlay.opacity = 1
        CATransaction.commit()

        let spring = CASpringAnimation(keyPath: "transform.scale")
        spring.fromValue = 0.3
        spring.toValue = 1
        spring.damping = 11
        spring.stiffness = 300
        spring.mass = 0.7
        spring.duration = spring.settlingDuration
        copiedDisc.add(spring, forKey: "pop")

        copiedCheck.strokeEnd = 1
        let draw = CABasicAnimation(keyPath: "strokeEnd")
        draw.fromValue = 0
        draw.toValue = 1
        draw.beginTime = CACurrentMediaTime() + 0.08
        draw.duration = 0.26
        draw.fillMode = .backwards
        draw.timingFunction = Theme.ease
        copiedCheck.add(draw, forKey: "draw")

        let rise = CABasicAnimation(keyPath: "opacity")
        rise.fromValue = 0
        rise.toValue = 1
        rise.beginTime = CACurrentMediaTime() + 0.12
        rise.duration = 0.2
        rise.fillMode = .backwards
        copiedLabel.add(rise, forKey: "rise")

        let flash = CABasicAnimation(keyPath: "borderWidth")
        flash.fromValue = 3
        flash.toValue = ring.borderWidth
        flash.duration = 0.5
        ring.add(flash, forKey: "copied")

        DispatchQueue.main.asyncAfter(deadline: .now() + 1.0) { [weak self] in
            guard let self, self.copiedToken == token else { return }
            CATransaction.begin()
            CATransaction.setAnimationDuration(0.3)
            CATransaction.setAnimationTimingFunction(Theme.ease)
            self.copiedOverlay.opacity = 0
            CATransaction.commit()
            self.copied = false
            self.refresh()
        }
    }

    // MARK: Folder motion

    /// Flies in from `offset` (window points, from the tile's centre to the
    /// folder's), growing from a small card into place.
    func emerge(from offset: CGVector, delay: CFTimeInterval = 0) {
        layer?.zPosition = 20
        let from = CATransform3DConcat(CATransform3DMakeScale(0.28, 0.28, 1),
                                       CATransform3DMakeTranslation(offset.dx, offset.dy, 0))
        let move = CASpringAnimation(keyPath: "transform")
        move.fromValue = from
        move.toValue = CATransform3DIdentity
        move.damping = 17
        move.stiffness = 210
        move.mass = 0.9
        move.duration = move.settlingDuration
        move.beginTime = CACurrentMediaTime() + delay
        move.fillMode = .backwards
        let fade = CABasicAnimation(keyPath: "opacity")
        fade.fromValue = 0
        fade.toValue = 1
        fade.duration = 0.18
        fade.beginTime = move.beginTime
        fade.fillMode = .backwards
        content.add(move, forKey: "emerge")
        content.add(fade, forKey: "emergeFade")
        DispatchQueue.main.asyncAfter(deadline: .now() + delay + move.duration) { [weak self] in
            self?.layer?.zPosition = 0
        }
    }

    /// Flies back into the folder at `offset`, shrinking; calls `done` when tucked away.
    func fold(to offset: CGVector, done: @escaping () -> Void) {
        layer?.zPosition = 20
        let to = CATransform3DConcat(CATransform3DMakeScale(0.28, 0.28, 1),
                                     CATransform3DMakeTranslation(offset.dx, offset.dy, 0))
        CATransaction.begin()
        CATransaction.setCompletionBlock(done)
        let move = CABasicAnimation(keyPath: "transform")
        move.fromValue = CATransform3DIdentity
        move.toValue = to
        move.duration = 0.34
        move.timingFunction = CAMediaTimingFunction(controlPoints: 0.5, 0, 0.2, 1)
        move.fillMode = .forwards
        move.isRemovedOnCompletion = false
        let fade = CABasicAnimation(keyPath: "opacity")
        fade.fromValue = 1
        fade.toValue = 0
        fade.beginTime = CACurrentMediaTime() + 0.2
        fade.duration = 0.14
        fade.fillMode = .both
        fade.isRemovedOnCompletion = false
        content.add(move, forKey: "fold")
        content.add(fade, forKey: "foldFade")
        CATransaction.commit()
    }

    func resetMotion() {
        content.removeAnimation(forKey: "fold")
        content.removeAnimation(forKey: "foldFade")
        content.removeAnimation(forKey: "emerge")
        content.removeAnimation(forKey: "emergeFade")
        layer?.zPosition = 0
    }

    override func updateTrackingAreas() {
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.activeAlways, .mouseEnteredAndExited, .mouseMoved, .inVisibleRect],
                                       owner: self))
    }

    override func mouseEntered(with event: NSEvent) {
        hovered = true
        if !copied { staggerIn() }
    }
    override func mouseExited(with event: NSEvent) { hovered = false }
    override func mouseMoved(with event: NSEvent) {
        hotButton = hovered && !copied ? action(at: convert(event.locationInWindow, from: nil)) : nil
    }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    /// The four actions pop in one after another, clockwise from top-left.
    private func staggerIn() {
        for (i, b) in [selectButton, copyButton, extraButton, editButton].enumerated() {
            let spring = CASpringAnimation(keyPath: "transform.scale")
            spring.fromValue = 0.5
            spring.toValue = 1
            spring.damping = 13
            spring.stiffness = 320
            spring.mass = 0.6
            spring.duration = spring.settlingDuration
            spring.beginTime = CACurrentMediaTime() + Double(i) * 0.025
            spring.fillMode = .backwards
            b.layer.add(spring, forKey: "in")
        }
    }

    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        if hovered && !copied, let i = action(at: p) {
            if let layer = buttons[i].layer as CALayer? { Theme.pop(layer, from: 0.8) }
            switch i {
            case 0: onToggleSelect?()
            case 1: onCopy?()
            case 2: onEdit?()
            default: isSVG ? onCopyCode?() : onCopyText?()
            }
            return
        }
        if badgeRect.insetBy(dx: -4, dy: -4).contains(p) && selected {
            onToggleSelect?()
            return
        }
        super.mouseDown(with: event) // NSCollectionView: select / ⇧ range / ⌘ toggle / drag
    }
}

final class TileItem: NSCollectionViewItem {
    static let identifier = NSUserInterfaceItemIdentifier("TileItem")
    var tile: TileView { view as! TileView }

    override func loadView() { view = TileView(frame: .zero) }

    override var isSelected: Bool {
        didSet { tile.selected = isSelected }
    }

    override var highlightState: NSCollectionViewItem.HighlightState {
        didSet { tile.previewSelected = highlightState == .forSelection }
    }

    override func prepareForReuse() {
        super.prepareForReuse()
        tile.hovered = false
        tile.resetMotion()
    }
}

// MARK: - Folder

/// A Finder-style folder: back panel with a tab, the images inside peeking
/// out like papers, and a front panel with the name. Papers leave one by one
/// as the folder opens and drop back in as it closes.
final class FolderTileView: NSView {
    /// Folder colours: one place to change (e.g. to Finder blue).
    static let backTop = NSColor(white: 0.78, alpha: 1)
    static let backBottom = NSColor(white: 0.66, alpha: 1)
    static let frontTop = NSColor(white: 0.97, alpha: 1)
    static let frontBottom = NSColor(white: 0.84, alpha: 1)

    private let body = CALayer()
    private let back = CAGradientLayer()
    private let backMask = CAShapeLayer()
    private var papers: [CALayer] = []
    private let front = CAGradientLayer()
    private let frontMask = CAShapeLayer()
    private let frontEdge = CAShapeLayer()
    private let nameLayer = CATextLayer()
    private let countLayer = CATextLayer()
    private let ring = CALayer()

    var onToggle: (() -> Void)?
    private(set) var groupID: String?
    private var inside = 0
    private var open = false
    private var thumbs: [String] = []
    var hovered = false { didSet { if hovered != oldValue { refresh() } } }
    var dimmed = false { didSet { if dimmed != oldValue { refresh() } } }
    /// Images are being dragged over: the folder opens its mouth for them.
    var dropTarget = false { didSet { if dropTarget != oldValue { refresh() } } }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.masksToBounds = false
        layer?.addSublayer(body)

        ring.cornerRadius = 10
        ring.cornerCurve = .continuous
        ring.borderWidth = 1
        ring.borderColor = Theme.line.cgColor
        ring.backgroundColor = NSColor(white: 1, alpha: 0.03).cgColor
        body.addSublayer(ring)

        back.colors = [Self.backTop.cgColor, Self.backBottom.cgColor]
        back.mask = backMask
        body.addSublayer(back)

        for i in 0..<3 {
            let paper = CALayer()
            paper.backgroundColor = NSColor.white.cgColor
            paper.cornerRadius = 2.5
            paper.borderWidth = 1.5
            paper.borderColor = NSColor.white.cgColor
            paper.masksToBounds = true
            paper.contentsGravity = .resizeAspectFill
            paper.shadowColor = NSColor.black.cgColor
            paper.shadowOpacity = 0.25
            paper.shadowRadius = 2
            paper.shadowOffset = CGSize(width: 0, height: -1)
            paper.opacity = 0
            paper.zPosition = CGFloat(i)
            papers.append(paper)
            body.addSublayer(paper)
        }

        front.colors = [Self.frontTop.cgColor, Self.frontBottom.cgColor]
        front.mask = frontMask
        front.zPosition = 10
        body.addSublayer(front)
        frontEdge.fillColor = nil
        frontEdge.strokeColor = NSColor(white: 1, alpha: 0.9).cgColor
        frontEdge.lineWidth = 0.8
        frontEdge.zPosition = 11
        body.addSublayer(frontEdge)

        for t in [nameLayer, countLayer] {
            t.alignmentMode = .center
            t.truncationMode = .end
            t.foregroundColor = NSColor(white: 0.12, alpha: 1).cgColor
            t.zPosition = 12
            body.addSublayer(t)
        }
        nameLayer.font = Theme.sans(9.5, .semibold)
        nameLayer.fontSize = 9.5
        countLayer.font = Theme.mono(8.5, .medium)
        countLayer.fontSize = 8.5
        countLayer.foregroundColor = NSColor(white: 0.35, alpha: 1).cgColor
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidChangeBackingProperties() {
        let scale = window?.backingScaleFactor ?? 2
        [nameLayer, countLayer].forEach { $0.contentsScale = scale }
    }

    /// Where papers sit when they're inside (the folder's mouth), in view points.
    var mouth: CGPoint { CGPoint(x: bounds.midX, y: bounds.height * 0.5) }

    override func layout() {
        super.layout()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        body.bounds = bounds
        body.position = CGPoint(x: bounds.midX, y: bounds.midY)
        ring.frame = bounds
        let w = bounds.width, h = bounds.height
        let inset = w * 0.1
        let fw = w - inset * 2
        let bottom = h * 0.12
        let backH = h * 0.58
        let frontH = h * 0.42
        // Back panel with the tab on the left.
        let backRect = CGRect(x: inset, y: bottom, width: fw, height: backH)
        back.frame = bounds
        let tabW = fw * 0.38, tabH = h * 0.07, r: CGFloat = 4
        let bp = CGMutablePath()
        bp.move(to: CGPoint(x: backRect.minX, y: backRect.minY + r))
        bp.addLine(to: CGPoint(x: backRect.minX, y: backRect.maxY + tabH - r))
        bp.addQuadCurve(to: CGPoint(x: backRect.minX + r, y: backRect.maxY + tabH), control: CGPoint(x: backRect.minX, y: backRect.maxY + tabH))
        bp.addLine(to: CGPoint(x: backRect.minX + tabW - r, y: backRect.maxY + tabH))
        bp.addQuadCurve(to: CGPoint(x: backRect.minX + tabW + r, y: backRect.maxY), control: CGPoint(x: backRect.minX + tabW, y: backRect.maxY + tabH))
        bp.addLine(to: CGPoint(x: backRect.maxX - r, y: backRect.maxY))
        bp.addQuadCurve(to: CGPoint(x: backRect.maxX, y: backRect.maxY - r), control: CGPoint(x: backRect.maxX, y: backRect.maxY))
        bp.addLine(to: CGPoint(x: backRect.maxX, y: backRect.minY + r))
        bp.addQuadCurve(to: CGPoint(x: backRect.maxX - r, y: backRect.minY), control: CGPoint(x: backRect.maxX, y: backRect.minY))
        bp.addLine(to: CGPoint(x: backRect.minX + r, y: backRect.minY))
        bp.addQuadCurve(to: CGPoint(x: backRect.minX, y: backRect.minY + r), control: CGPoint(x: backRect.minX, y: backRect.minY))
        backMask.frame = bounds
        backMask.path = bp
        // Front panel.
        let frontRect = CGRect(x: inset - 1, y: bottom, width: fw + 2, height: frontH)
        let flap = front.transform
        front.transform = CATransform3DIdentity
        front.bounds = bounds
        front.anchorPoint = CGPoint(x: 0.5, y: 0.14)
        front.position = CGPoint(x: bounds.midX, y: bounds.height * 0.14)
        front.transform = flap
        let fp = CGPath(roundedRect: frontRect, cornerWidth: 4.5, cornerHeight: 4.5, transform: nil)
        frontMask.frame = bounds
        frontMask.path = fp
        frontEdge.path = CGPath(rect: CGRect(x: frontRect.minX + 3, y: frontRect.maxY - 0.6, width: frontRect.width - 6, height: 0.01), transform: nil)
        // Papers.
        // Papers stand up out of the folder, like Finder's document folder.
        let pw = fw * 0.6, ph = backH * 0.8
        for (i, paper) in papers.enumerated() {
            paper.bounds = CGRect(x: 0, y: 0, width: pw, height: ph)
            let dx = CGFloat(i - 1) * fw * 0.11
            paper.position = CGPoint(x: bounds.midX + dx, y: bottom + frontH * 0.5 + ph / 2 + CGFloat(i) * 1.5)
        }
        nameLayer.frame = CGRect(x: frontRect.minX + 4, y: bottom + frontH * 0.42, width: frontRect.width - 8, height: 12)
        countLayer.frame = CGRect(x: frontRect.minX + 4, y: bottom + frontH * 0.12, width: frontRect.width - 8, height: 11)
        CATransaction.commit()
        updatePapers(animated: false)
    }

    func configure(_ group: VaultGroup, members: [VaultItem], inside: Int) {
        let changedGroup = groupID != group.id
        groupID = group.id
        nameLayer.string = group.name
        toolTip = "\(group.name) · \(members.count) image\(members.count == 1 ? "" : "s") — click to \(inside > 0 ? "open" : "close")"
        let ids = members.prefix(3).map(\.id)
        if ids != thumbs || changedGroup {
            thumbs = ids
            for (i, paper) in papers.enumerated() {
                paper.contents = nil
                guard i < members.count else { continue }
                let id = members[i].id
                Thumbnails.shared.load(members[i], maxPixel: 160) { [weak self, weak paper] image in
                    guard let self, self.thumbs.indices.contains(i), self.thumbs[i] == id else { return }
                    paper?.contents = image
                }
            }
        }
        setInside(inside, total: members.count, animated: !changedGroup)
    }

    /// How many images are still in the folder. Papers lift out (or drop in)
    /// to match, and the front flap opens once it's empty.
    func setInside(_ n: Int, total: Int, animated: Bool) {
        let wasInside = inside
        inside = n
        open = n == 0 && total > 0
        countLayer.string = total == 0 ? "Empty" : (n == total ? "\(total)" : (n == 0 ? "\(total) out" : "\(n) of \(total)"))
        updatePapers(animated: animated, previous: wasInside)
        refresh()
    }

    private func updatePapers(animated: Bool, previous: Int? = nil) {
        let shown = min(3, inside)
        let before = min(3, previous ?? inside)
        let angles: [CGFloat] = [-0.12, 0.03, 0.14]
        CATransaction.begin()
        CATransaction.setDisableActions(!animated)
        CATransaction.setAnimationDuration(0.32)
        CATransaction.setAnimationTimingFunction(Theme.ease)
        for (i, paper) in papers.enumerated() {
            let visible = i < shown
            let lift: CGFloat = (hovered || dropTarget) && visible ? 3 + CGFloat(i) * (dropTarget ? 3 : 1.5) : 0
            paper.opacity = visible ? 1 : 0
            var t = CATransform3DMakeRotation(angles[i], 0, 0, 1)
            t = CATransform3DConcat(t, CATransform3DMakeTranslation(0, visible ? lift : bounds.height * 0.35, 0))
            paper.transform = t
        }
        CATransaction.commit()
        if animated, previous != nil, shown != before {
            Theme.pop(body, from: shown < before ? 0.94 : 1.06)
        }
    }

    private func refresh() {
        CATransaction.begin()
        CATransaction.setAnimationDuration(0.24)
        CATransaction.setAnimationTimingFunction(Theme.ease)
        ring.borderColor = (dropTarget ? Theme.paper : (hovered ? Theme.line2 : (open ? Theme.line2 : Theme.line))).cgColor
        ring.borderWidth = dropTarget ? 2 : 1
        ring.backgroundColor = NSColor(white: 1, alpha: open ? 0.07 : (hovered ? 0.05 : 0.03)).cgColor
        // An open (empty) folder tips its front flap forward.
        var flap = CATransform3DIdentity
        flap.m34 = -1 / 220
        if open || dropTarget { flap = CATransform3DRotate(flap, dropTarget ? 0.38 : 0.5, 1, 0, 0) }
        front.transform = flap
        frontEdge.opacity = open ? 0 : 1
        body.setAffineTransform(dropTarget ? CGAffineTransform(scaleX: 1.08, y: 1.08)
                                : (hovered ? CGAffineTransform(scaleX: 1.03, y: 1.03) : .identity))
        body.opacity = dimmed && !hovered && !dropTarget ? 0.3 : 1
        CATransaction.commit()
        updatePapers(animated: true)
    }

    override func updateTrackingAreas() {
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.activeAlways, .mouseEnteredAndExited, .inVisibleRect], owner: self))
    }
    override func mouseEntered(with event: NSEvent) { hovered = true }
    override func mouseExited(with event: NSEvent) { hovered = false }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override func mouseDown(with event: NSEvent) {
        Theme.pop(body, from: 0.92)
        onToggle?()
    }
}

final class FolderItem: NSCollectionViewItem {
    static let identifier = NSUserInterfaceItemIdentifier("FolderItem")
    var folder: FolderTileView { view as! FolderTileView }
    override func loadView() { view = FolderTileView(frame: .zero) }
    override func prepareForReuse() {
        super.prepareForReuse()
        folder.hovered = false
    }
}
