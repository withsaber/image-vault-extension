import AppKit
import QuartzCore

/// Borderless icon button with a hover outline and a press snap.
final class IconButton: NSButton {
    var inverted = false { didSet { refresh() } }
    private var hovered = false { didSet { refresh() } }
    private let symbolName: String

    init(_ symbol: String, tip: String, action: @escaping () -> Void) {
        symbolName = symbol
        handler = action
        super.init(frame: NSRect(x: 0, y: 0, width: 30, height: 30))
        isBordered = false
        title = ""
        toolTip = tip
        wantsLayer = true
        layer?.cornerRadius = 3
        layer?.borderWidth = 1
        target = self
        self.action = #selector(fire)
        translatesAutoresizingMaskIntoConstraints = false
        widthAnchor.constraint(equalToConstant: 30).isActive = true
        heightAnchor.constraint(equalToConstant: 30).isActive = true
        refresh()
    }

    required init?(coder: NSCoder) { fatalError() }

    private let handler: () -> Void
    @objc private func fire() {
        if let layer { Theme.pop(layer, from: 0.88) }
        handler()
    }

    private func refresh() {
        let fg: NSColor = inverted ? .black : (hovered ? .white : Theme.muted)
        image = NSImage.symbol(symbolName, size: 13, weight: .medium, color: fg)
        layer?.backgroundColor = (inverted ? Theme.paper : .clear).cgColor
        layer?.borderColor = (inverted ? Theme.paper : (hovered ? Theme.line2 : .clear)).cgColor
    }

    override func updateTrackingAreas() {
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: .zero, options: [.activeAlways, .mouseEnteredAndExited, .inVisibleRect], owner: self))
    }
    override func mouseEntered(with event: NSEvent) { hovered = true }
    override func mouseExited(with event: NSEvent) { hovered = false }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
}

/// Collection view that adds the keyboard shortcuts of a file manager.
final class VaultCollectionView: NSCollectionView {
    var onKey: ((NSEvent) -> Bool)?
    var menuProvider: ((IndexPath) -> NSMenu?)?
    var isFolder: ((IndexPath) -> Bool)?
    override func keyDown(with event: NSEvent) {
        if onKey?(event) == true { return }
        super.keyDown(with: event)
    }
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        // Key equivalents go to every view in the window; only act when the
        // grid itself has focus, so ⌘A / ⌘C still work in the search field.
        guard window?.firstResponder === self else { return super.performKeyEquivalent(with: event) }
        if onKey?(event) == true { return true }
        return super.performKeyEquivalent(with: event)
    }
    override var acceptsFirstResponder: Bool { true }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func menu(for event: NSEvent) -> NSMenu? {
        let point = convert(event.locationInWindow, from: nil)
        guard let path = indexPathForItem(at: point) else { return nil }
        if isFolder?(path) != true, !selectionIndexPaths.contains(path) {
            selectionIndexPaths = [path]
        }
        return menuProvider?(path)
    }
}

/// Full-panel preview (Space / Return), arrows to step through, ⌘C to copy.
final class PreviewView: NSView {
    private let imageView = FitImageView()
    private let nameLabel = Theme.label("", font: Theme.sans(12, .semibold), color: .white)
    private let metaLabel = Theme.label("", font: Theme.mono(10), color: Theme.muted)
    private let counter = Theme.label("", font: Theme.mono(10, .medium), color: Theme.muted)
    private var prevButton: IconButton!
    private var nextButton: IconButton!
    private var items: [VaultItem] = []
    private var index = 0
    private var loadToken = 0
    private var fullLoaded = false
    var onClose: (() -> Void)?
    var onCopy: ((VaultItem) -> Void)?
    var onText: ((VaultItem) -> Void)?

    // Never ask the island for more room than it gives us.
    override var intrinsicContentSize: NSSize { NSSize(width: NSView.noIntrinsicMetric, height: NSView.noIntrinsicMetric) }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.backgroundColor = Theme.ink.cgColor
        layer?.masksToBounds = true
        imageView.translatesAutoresizingMaskIntoConstraints = false
        nameLabel.lineBreakMode = .byTruncatingMiddle
        metaLabel.lineBreakMode = .byTruncatingTail
        [nameLabel, metaLabel, counter].forEach {
            $0.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
            $0.setContentHuggingPriority(.defaultLow, for: .horizontal)
        }
        counter.setContentCompressionResistancePriority(.required, for: .horizontal)
        let close = IconButton("xmark", tip: "Close  Esc") { [weak self] in self?.hide() }
        let copyButton = IconButton("doc.on.doc", tip: "Copy  ⌘C") { [weak self] in self?.copyCurrent() }
        let textButton = IconButton("doc.text.magnifyingglass", tip: "Copy Text  ⌘T") { [weak self] in self?.textCurrent() }
        prevButton = IconButton("chevron.left", tip: "Previous  ←") { [weak self] in self?.step(-1) }
        nextButton = IconButton("chevron.right", tip: "Next  →") { [weak self] in self?.step(1) }
        let buttons = [prevButton!, counter, nextButton!, textButton, copyButton, close] as [NSView]
        let bar = NSStackView(views: buttons)
        bar.orientation = .horizontal
        bar.spacing = 2
        bar.setCustomSpacing(10, after: nextButton)
        bar.translatesAutoresizingMaskIntoConstraints = false
        [imageView, nameLabel, metaLabel, bar].forEach(addSubview)
        NSLayoutConstraint.activate([
            nameLabel.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 18),
            nameLabel.topAnchor.constraint(equalTo: topAnchor, constant: 4),
            nameLabel.trailingAnchor.constraint(lessThanOrEqualTo: bar.leadingAnchor, constant: -12),
            metaLabel.leadingAnchor.constraint(equalTo: nameLabel.leadingAnchor),
            metaLabel.topAnchor.constraint(equalTo: nameLabel.bottomAnchor, constant: 1),
            metaLabel.trailingAnchor.constraint(lessThanOrEqualTo: bar.leadingAnchor, constant: -12),
            bar.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -14),
            bar.centerYAnchor.constraint(equalTo: nameLabel.bottomAnchor, constant: 1),
            imageView.topAnchor.constraint(equalTo: metaLabel.bottomAnchor, constant: 8),
            imageView.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 16),
            imageView.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -16),
            imageView.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -14),
        ])
        imageView.onClick = { [weak self] point in
            guard let self else { return }
            // Click the left/right third of the image to step through.
            if point.x < self.imageView.bounds.width / 3 { self.step(-1) }
            else if point.x > self.imageView.bounds.width * 2 / 3 { self.step(1) }
        }
    }

    required init?(coder: NSCoder) { fatalError() }

    func show(_ item: VaultItem, in all: [VaultItem]) {
        items = all
        index = all.firstIndex(of: item) ?? 0
        render()
        if isHidden {
            isHidden = false
            alphaValue = 0
            Theme.animate(0.22) { animator().alphaValue = 1 }
        }
    }

    func hide() {
        guard !isHidden else { return }
        Theme.animate(0.2, { animator().alphaValue = 0 }, completion: { [weak self] in
            self?.isHidden = true
            self?.imageView.image = nil
            self?.onClose?()
        })
    }

    private func step(_ delta: Int) {
        let next = index + delta
        guard items.indices.contains(next) else { NSSound.beep(); return }
        index = next
        render()
    }

    private func render() {
        guard items.indices.contains(index) else { return }
        let item = items[index]
        let url = VaultStore.shared.fileURL(for: item)
        nameLabel.stringValue = item.name
        nameLabel.toolTip = url.path
        let dims = item.width.flatMap { w in item.height.map { "\(w)×\($0)" } }
        let size = item.bytes.map { ByteCountFormatter.string(fromByteCount: Int64($0), countStyle: .file) }
        let date: String? = item.timestamp > 0
            ? DateFormatter.localizedString(from: Date(timeIntervalSince1970: item.timestamp / 1000),
                                            dateStyle: .medium, timeStyle: .short)
            : nil
        metaLabel.stringValue = [dims, size, item.domain ?? item.source, date]
            .compactMap { $0 }.joined(separator: "  ·  ")
        metaLabel.toolTip = [item.pageTitle, item.pageUrl, item.url].compactMap { $0 }.joined(separator: "\n")
        counter.stringValue = "\(index + 1) / \(items.count)"
        prevButton.isEnabled = index > 0
        nextButton.isEnabled = index < items.count - 1
        prevButton.alphaValue = index > 0 ? 1 : 0.3
        nextButton.alphaValue = index < items.count - 1 ? 1 : 0.3

        // Decode a screen-sized copy off the main thread; big originals would
        // otherwise stall the island. Grid thumbnail first so it's never blank.
        loadToken += 1
        let token = loadToken
        fullLoaded = false
        Thumbnails.shared.load(item) { [weak self] thumb in
            guard let self, token == self.loadToken, !self.fullLoaded, let thumb else { return }
            self.imageView.image = thumb
        }
        let maxPixel = Int(max(1200, (window?.backingScaleFactor ?? 2) * 900))
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let image = Imaging.thumbnail(at: url, maxPixel: maxPixel)
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    guard let self, token == self.loadToken, let image else { return }
                    self.fullLoaded = true
                    self.imageView.image = image
                }
            }
        }
    }

    private func copyCurrent() {
        guard items.indices.contains(index) else { return }
        onCopy?(items[index])
    }

    private func textCurrent() {
        guard items.indices.contains(index) else { return }
        onText?(items[index])
    }

    func handleKey(_ event: NSEvent) -> Bool {
        switch event.keyCode {
        case 53, 49: hide()                // Esc, Space
        case 123: step(-1)                 // ←
        case 124: step(1)                  // →
        default:
            if event.modifierFlags.contains(.command), event.charactersIgnoringModifiers == "c" { copyCurrent() }
            else if event.modifierFlags.contains(.command), event.charactersIgnoringModifiers == "t" { textCurrent() }
            else { return false }
        }
        return true
    }
}

/// Shows an image scaled to fit (aspect preserved, centred) and never reports
/// an intrinsic size, so a 4K picture can't stretch the island's layout.
final class FitImageView: NSView {
    private let imageLayer = CALayer()
    var onClick: ((NSPoint) -> Void)?
    var image: CGImage? {
        didSet {
            CATransaction.begin()
            CATransaction.setAnimationDuration(0.15)
            imageLayer.contents = image
            CATransaction.commit()
        }
    }

    override var intrinsicContentSize: NSSize { NSSize(width: NSView.noIntrinsicMetric, height: NSView.noIntrinsicMetric) }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.masksToBounds = true
        imageLayer.contentsGravity = .resizeAspect
        imageLayer.minificationFilter = .trilinear
        imageLayer.magnificationFilter = .linear
        layer?.addSublayer(imageLayer)
        setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        setContentCompressionResistancePriority(.defaultLow, for: .vertical)
        setContentHuggingPriority(.defaultLow, for: .horizontal)
        setContentHuggingPriority(.defaultLow, for: .vertical)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func layout() {
        super.layout()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        imageLayer.frame = bounds
        imageLayer.contentsScale = window?.backingScaleFactor ?? 2
        CATransaction.commit()
    }

    override func mouseUp(with event: NSEvent) {
        onClick?(convert(event.locationInWindow, from: nil))
    }
}

/// The image grid inside the expanded island. Native NSCollectionView, so it
/// gets click / ⇧ range / ⌘ toggle / lasso selection and virtualisation for free;
/// every tile drags out as a real file and ⌘C puts real images on the clipboard.
@MainActor
final class TrayController: NSViewController, NSCollectionViewDelegate {
    var onSelection: ((Int) -> Void)?
    var onVisibleCount: ((Int) -> Void)?
    var onMessage: ((String) -> Void)?
    var onDragOut: ((Bool) -> Void)?
    var onEscape: (() -> Void)?
    var onSearch: (() -> Void)?

    private let collection = VaultCollectionView()
    private let scroll = NSScrollView()
    private let flow = NSCollectionViewFlowLayout()
    private var dataSource: NSCollectionViewDiffableDataSource<Int, String>!
    private let preview = PreviewView()
    private(set) var isDraggingOut = false
    private var query = ""
    private var sortOrder: SortOrder = .newest
    /// Folder id → how many of its images are out in the grid right now.
    private var outCount: [String: Int] = [:]
    private var animatingGroups: Set<String> = []
    var onEdit: ((VaultItem) -> Void)?

    static let folderPrefix = "g:"
    /// Marks a drag that started on our own tiles.
    static let tileType = NSPasteboard.PasteboardType("com.imagevault.tile")
    private var draggedIDs: [String] = []
    private var dropTargetID: String?

    static let tileSide: CGFloat = 96
    /// The grid always shows this many columns; tiles size to fit.
    static let columns = 8

    override func loadView() {
        view = NSView(frame: NSRect(x: 0, y: 0, width: 600, height: 230))
        view.wantsLayer = true

        flow.itemSize = NSSize(width: Self.tileSide, height: Self.tileSide)
        flow.minimumInteritemSpacing = 8
        flow.minimumLineSpacing = 8
        flow.sectionInset = NSEdgeInsets(top: 4, left: 16, bottom: 16, right: 16)
        collection.collectionViewLayout = flow
        collection.backgroundColors = [.clear]
        collection.isSelectable = true
        collection.allowsMultipleSelection = true
        collection.allowsEmptySelection = true
        collection.delegate = self
        collection.register(TileItem.self, forItemWithIdentifier: TileItem.identifier)
        collection.register(FolderItem.self, forItemWithIdentifier: FolderItem.identifier)
        // Drops are handled by the island's host view, not the grid.
        // The grid only accepts its own tiles (dropped on a folder or on
        // another tile to group them). Outside drops still go to the island.
        collection.unregisterDraggedTypes()
        collection.registerForDraggedTypes([Self.tileType])
        collection.setDraggingSourceOperationMask(.copy, forLocal: false)
        collection.setDraggingSourceOperationMask(.move, forLocal: true)
        collection.onKey = { [weak self] event in self?.handleKey(event) ?? false }
        collection.menuProvider = { [weak self] path in self?.menu(for: path) }
        collection.isFolder = { [weak self] path in self?.folderID(at: path) != nil }
        let doubleClick = NSClickGestureRecognizer(target: self, action: #selector(doubleClicked(_:)))
        doubleClick.numberOfClicksRequired = 2
        doubleClick.delaysPrimaryMouseButtonEvents = false
        collection.addGestureRecognizer(doubleClick)

        dataSource = NSCollectionViewDiffableDataSource<Int, String>(collectionView: collection) { [weak self] cv, indexPath, id in
            if id.hasPrefix(Self.folderPrefix) {
                let cell = cv.makeItem(withIdentifier: FolderItem.identifier, for: indexPath) as! FolderItem
                let gid = String(id.dropFirst(Self.folderPrefix.count))
                self?.configureFolder(cell.folder, gid)
                cell.folder.dimmed = self?.isDimmed(id) ?? false
                cell.folder.onToggle = { [weak self] in self?.toggleFolder(gid) }
                return cell
            }
            let cell = cv.makeItem(withIdentifier: TileItem.identifier, for: indexPath) as! TileItem
            guard let item = VaultModel.shared.byID[id] else { return cell }
            cell.tile.configure(item, fresh: VaultModel.shared.fresh.contains(id))
            cell.tile.dimmed = self?.isDimmed(id) ?? false
            cell.tile.onCopy = { [weak self, weak cell] in
                self?.copy([item], announce: false)
                cell?.tile.showCopied()
            }
            cell.tile.onCopyText = { [weak self] in
                self?.copyText([item])
            }
            cell.tile.onCopyCode = { [weak self, weak cell] in
                if Clipboard.copySVGCode(item) { cell?.tile.showCopied(label: "Code copied") }
                else { self?.onMessage?("Couldn't read the SVG") }
            }
            cell.tile.onEdit = { [weak self] in self?.onEdit?(item) }
            cell.tile.onToggleSelect = { [weak self] in self?.toggleSelection(id) }
            return cell
        }

        scroll.documentView = collection
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.scrollerStyle = .overlay
        scroll.scrollerKnobStyle = .light
        scroll.automaticallyAdjustsContentInsets = false
        scroll.contentView.drawsBackground = false
        scroll.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(scroll)

        preview.translatesAutoresizingMaskIntoConstraints = false
        preview.isHidden = true
        preview.layer?.backgroundColor = NSColor.black.cgColor
        preview.onClose = { [weak self] in self?.focus() }
        preview.onCopy = { [weak self] item in self?.copy([item]) }
        preview.onText = { [weak self] item in self?.copyText([item]) }
        view.addSubview(preview)

        NSLayoutConstraint.activate([
            scroll.topAnchor.constraint(equalTo: view.topAnchor),
            scroll.bottomAnchor.constraint(equalTo: view.bottomAnchor),
            scroll.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            preview.topAnchor.constraint(equalTo: view.topAnchor),
            preview.bottomAnchor.constraint(equalTo: view.bottomAnchor),
            preview.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            preview.trailingAnchor.constraint(equalTo: view.trailingAnchor),
        ])

        NotificationCenter.default.addObserver(forName: .vaultChanged, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.applySnapshot() }
        }
        applySnapshot()
    }

    /// Fixed column count: tiles size to the width, with equal side margins
    /// so they never hug the left edge.
    override func viewDidLayout() {
        super.viewDidLayout()
        let width = scroll.contentSize.width
        guard width > 0 else { return }
        let cols = CGFloat(Self.columns), gap = flow.minimumInteritemSpacing
        let side = max(2, floor((width - 32 - (cols - 1) * gap) / cols))
        let used = cols * side + (cols - 1) * gap
        let margin = max(12, floor((width - used) / 2))
        if abs(flow.itemSize.width - side) > 0.5 || abs(flow.sectionInset.left - margin) > 0.5 {
            flow.itemSize = NSSize(width: side, height: side)
            flow.sectionInset = NSEdgeInsets(top: 4, left: margin, bottom: 16, right: margin)
            flow.invalidateLayout()
        }
        if abs(collection.frame.width - width) > 0.5 {
            collection.frame.size.width = width
        }
    }

    func focus() { view.window?.makeFirstResponder(collection) }
    func closePreview() { preview.hide() }

    /// Snapshot aid: show the hover grid on the nth tile.
    func snapshotHover(_ n: Int) {
        let paths = collection.indexPathsForVisibleItems().sorted()
        let tiles = paths.compactMap { collection.item(at: $0) as? TileItem }
        if tiles.indices.contains(n) { tiles[n].tile.hovered = true }
    }

    /// Snapshot aid: open the preview on the largest visible image.
    func previewLargest() {
        let items = visibleItems()
        if let big = items.max(by: { ($0.width ?? 0) * ($0.height ?? 0) < ($1.width ?? 0) * ($1.height ?? 0) }) {
            preview.show(big, in: items)
        }
    }

    // MARK: Data & selection

    func setFilter(query: String, sort: SortOrder) {
        guard query != self.query || sort != sortOrder else { return }
        self.query = query
        sortOrder = sort
        applySnapshot()
    }

    /// Images in the order the grid shows them (folders' open images included).
    /// All selection actions and previews operate on this list.
    func visibleItems() -> [VaultItem] {
        displayIDs().compactMap { VaultModel.shared.byID[$0] }
    }

    private var searching: Bool { !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }

    /// Folders first (newest first), each followed by the images that are out
    /// of it, then everything that isn't in a folder. Searching shows a flat list.
    private func displayIDs() -> [String] {
        let model = VaultModel.shared
        if searching { return filteredSorted(model.items).map(\.id) }
        var ids: [String] = []
        for group in model.groups {
            ids.append(Self.folderPrefix + group.id)
            let members = self.members(of: group.id)
            ids.append(contentsOf: members.prefix(outCount[group.id] ?? 0).map(\.id))
        }
        ids.append(contentsOf: filteredSorted(model.items.filter { model.groupOf[$0.id] == nil }).map(\.id))
        return ids
    }

    func members(of groupID: String) -> [VaultItem] {
        let model = VaultModel.shared
        guard let group = model.groups.first(where: { $0.id == groupID }) else { return [] }
        var seen = Set<String>()
        return group.itemIDs.filter { model.groupOf[$0] == groupID && seen.insert($0).inserted }.compactMap { model.byID[$0] }
    }

    private func filteredSorted(_ source: [VaultItem]) -> [VaultItem] {
        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        var items = source
        if !needle.isEmpty {
            items = items.filter { item in
                let fields = [item.name, item.domain, item.source, item.pageTitle, item.url, item.pageUrl, item.ocrText]
                return fields.contains { ($0 ?? "").lowercased().contains(needle) }
            }
        }
        switch sortOrder {
        case .newest:
            items.sort { $0.timestamp > $1.timestamp }
        case .oldest:
            items.sort { $0.timestamp < $1.timestamp }
        case .name:
            items.sort {
                let result = $0.name.localizedCaseInsensitiveCompare($1.name)
                return result == .orderedSame ? $0.timestamp > $1.timestamp : result == .orderedAscending
            }
        case .size:
            items.sort { ($0.bytes ?? 0) > ($1.bytes ?? 0) }
        }
        return items
    }

    private func applySnapshot() {
        // Folders whose images were moved or deleted: keep the counts honest.
        for (gid, n) in outCount { outCount[gid] = min(n, members(of: gid).count) }
        let ids = displayIDs()
        let selectedIDs = selectedItems().map(\.id)
        var snapshot = NSDiffableDataSourceSnapshot<Int, String>()
        snapshot.appendSections([0])
        snapshot.appendItems(ids)
        dataSource.apply(snapshot, animatingDifferences: view.window != nil)
        collection.selectionIndexPaths = Set(selectedIDs.compactMap { dataSource.indexPath(for: $0) })
        refreshFolders()
        onSelection?(collection.selectionIndexPaths.count)
        onVisibleCount?(ids.filter { !$0.hasPrefix(Self.folderPrefix) }.count)
    }

    // MARK: Folders

    private func folderID(at path: IndexPath) -> String? {
        guard let id = dataSource.itemIdentifier(for: path), id.hasPrefix(Self.folderPrefix) else { return nil }
        return String(id.dropFirst(Self.folderPrefix.count))
    }

    private func configureFolder(_ view: FolderTileView, _ gid: String) {
        guard let group = VaultModel.shared.groups.first(where: { $0.id == gid }) else { return }
        let members = members(of: gid)
        view.configure(group, members: members, inside: members.count - min(outCount[gid] ?? 0, members.count))
    }

    /// Diffable snapshots don't re-run the cell provider for unchanged ids.
    private func refreshFolders() {
        for path in collection.indexPathsForVisibleItems() {
            guard let id = dataSource.itemIdentifier(for: path) else { continue }
            if let cell = collection.item(at: path) as? FolderItem, let gid = folderID(at: path) {
                configureFolder(cell.folder, gid)
                cell.folder.dimmed = isDimmed(id)
            } else if let cell = collection.item(at: path) as? TileItem {
                cell.tile.dimmed = isDimmed(id)
            }
        }
    }

    /// While a folder is open, everything that isn't that folder or its
    /// images fades back.
    private func isDimmed(_ id: String) -> Bool {
        let open = Set(outCount.filter { $0.value > 0 }.map(\.key))
        guard !open.isEmpty, !searching else { return false }
        if id.hasPrefix(Self.folderPrefix) { return !open.contains(String(id.dropFirst(Self.folderPrefix.count))) }
        guard let gid = VaultModel.shared.groupOf[id] else { return true }
        return !open.contains(gid)
    }

    private func folderCell(_ gid: String) -> FolderItem? {
        dataSource.indexPath(for: Self.folderPrefix + gid).flatMap { collection.item(at: $0) as? FolderItem }
    }

    /// Window-space vector from a tile's centre to its folder's mouth.
    private func offset(from tile: NSView, to folder: FolderTileView) -> CGVector {
        let a = tile.convert(NSPoint(x: tile.bounds.midX, y: tile.bounds.midY), to: nil)
        let b = folder.convert(folder.mouth, to: nil)
        return CGVector(dx: b.x - a.x, dy: b.y - a.y)
    }

    /// Opens a closed folder (images come out one by one and line up after
    /// it) or closes an open one (they fly back in one by one).
    func toggleFolder(_ gid: String) {
        guard !animatingGroups.contains(gid), !searching else { return }
        let total = members(of: gid).count
        guard total > 0 else { onMessage?("This folder is empty"); return }
        let out = outCount[gid] ?? 0
        animatingGroups.insert(gid)
        if out < total { openStep(gid, total: total) } else { closeStep(gid) }
    }

    private func openStep(_ gid: String, total: Int) {
        let out = outCount[gid] ?? 0
        guard out < total else { animatingGroups.remove(gid); return }
        outCount[gid] = out + 1
        applySnapshot()
        collection.layoutSubtreeIfNeeded()
        let member = members(of: gid)[out]
        if let path = dataSource.indexPath(for: member.id), let tile = (collection.item(at: path) as? TileItem)?.tile,
           let folder = folderCell(gid)?.folder {
            tile.emerge(from: offset(from: tile, to: folder))
        }
        // Long folders speed up so opening never drags on.
        let gap = total > 12 ? 0.035 : 0.07
        DispatchQueue.main.asyncAfter(deadline: .now() + gap) { [weak self] in self?.openStep(gid, total: total) }
    }

    private func closeStep(_ gid: String) {
        closeStepIfStill(gid, expected: outCount[gid] ?? 0)
    }

    /// The next image starts flying in once the previous one has left.
    private func closeStepIfStill(_ gid: String, expected: Int) {
        guard animatingGroups.contains(gid) else { return }
        guard expected > 0 else {
            // Wait for the last fold to land before allowing another toggle.
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.36) { [weak self] in self?.animatingGroups.remove(gid) }
            return
        }
        let member = members(of: gid)[expected - 1]
        let finish = { [weak self] in
            guard let self else { return }
            self.outCount[gid] = min(self.outCount[gid] ?? 0, expected - 1)
            self.applySnapshot()
        }
        if let path = dataSource.indexPath(for: member.id), let tile = (collection.item(at: path) as? TileItem)?.tile,
           let folder = folderCell(gid)?.folder,
           collection.visibleRect.intersects((collection.layoutAttributesForItem(at: path)?.frame ?? .zero)) {
            tile.fold(to: offset(from: tile, to: folder)) { finish() }
        } else {
            finish()
        }
        let gap = expected > 12 ? 0.035 : 0.07
        DispatchQueue.main.asyncAfter(deadline: .now() + gap) { [weak self] in self?.closeStepIfStill(gid, expected: expected - 1) }
    }

    /// Puts the selected images in a new folder at the front of the grid.
    func groupSelection() {
        let items = selectedItems()
        guard !items.isEmpty else { return }
        let n = VaultModel.shared.groups.count + 1
        var name = "Group \(n)"
        var k = n
        while VaultModel.shared.groups.contains(where: { $0.name == name }) { k += 1; name = "Group \(k)" }
        let group = VaultStore.shared.createGroup(name: name, ids: items.map(\.id))
        collection.deselectAll(nil)
        outCount[group.id] = 0
        VaultModel.shared.reload()
        collection.scroll(.zero)
        onMessage?("\(items.count) in \(name)")
    }

    private func moveToGroup(_ gid: String, _ items: [VaultItem]) {
        VaultStore.shared.addToGroup(gid, ids: items.map(\.id))
        collection.deselectAll(nil)
        VaultModel.shared.reload()
        let name = VaultModel.shared.groups.first { $0.id == gid }?.name ?? "folder"
        onMessage?("Moved \(items.count) to \(name)")
    }

    private func removeFromGroup(_ items: [VaultItem]) {
        VaultStore.shared.removeFromGroups(ids: items.map(\.id))
        collection.deselectAll(nil)
        VaultModel.shared.reload()
        onMessage?(items.count == 1 ? "Taken out of the folder" : "\(items.count) taken out of the folder")
    }

    private func renameGroup(_ gid: String) {
        guard let group = VaultModel.shared.groups.first(where: { $0.id == gid }) else { return }
        let alert = NSAlert()
        alert.messageText = "Rename Folder"
        alert.addButton(withTitle: "Rename")
        alert.addButton(withTitle: "Cancel")
        let field = NSTextField(string: group.name)
        field.frame = NSRect(x: 0, y: 0, width: 240, height: 24)
        alert.accessoryView = field
        alert.window.initialFirstResponder = field
        IslandController.shared?.menuOpen = true
        NSApp.activate(ignoringOtherApps: true)
        let response = alert.runModal()
        IslandController.shared?.menuOpen = false
        let name = field.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard response == .alertFirstButtonReturn, !name.isEmpty else { return }
        VaultStore.shared.renameGroup(gid, to: name)
        VaultModel.shared.reload()
    }

    private func ungroup(_ gid: String) {
        outCount[gid] = nil
        VaultStore.shared.deleteGroup(gid)
        VaultModel.shared.reload()
        onMessage?("Folder removed · images kept")
    }

    private func folderMenu(_ gid: String) -> NSMenu {
        let members = members(of: gid)
        let isOpen = (outCount[gid] ?? 0) > 0
        let menu = NSMenu()
        menu.autoenablesItems = false
        menu.appearance = NSAppearance(named: .darkAqua)
        menu.addItem(ClosureMenuItem(title: isOpen ? "Close Folder" : "Open Folder") { [weak self] in self?.toggleFolder(gid) })
        menu.addItem(ClosureMenuItem(title: "Copy All Images") { [weak self] in self?.copy(members) })
        menu.addItem(ClosureMenuItem(title: "Select All Inside") { [weak self] in
            guard let self else { return }
            let paths = members.compactMap { self.dataSource.indexPath(for: $0.id) }
            if paths.count < members.count { self.onMessage?("Open the folder to select its images") }
            self.collection.selectionIndexPaths = Set(paths)
            self.onSelection?(paths.count)
        })
        menu.addItem(.separator())
        menu.addItem(ClosureMenuItem(title: "Rename…") { [weak self] in
            DispatchQueue.main.async { self?.renameGroup(gid) }
        })
        menu.addItem(ClosureMenuItem(title: "Remove Folder (Keep Images)") { [weak self] in self?.ungroup(gid) })
        return menu
    }

    func selectedItems() -> [VaultItem] {
        guard dataSource != nil else { return [] }
        return collection.selectionIndexPaths.sorted()
            .compactMap { dataSource.itemIdentifier(for: $0) }
            .compactMap { VaultModel.shared.byID[$0] }
    }

    func collectionView(_ cv: NSCollectionView, shouldSelectItemsAt indexPaths: Set<IndexPath>) -> Set<IndexPath> {
        indexPaths.filter { folderID(at: $0) == nil }
    }

    func collectionView(_ cv: NSCollectionView, shouldChangeItemsAt indexPaths: Set<IndexPath>,
                        to highlightState: NSCollectionViewItem.HighlightState) -> Set<IndexPath> {
        indexPaths.filter { folderID(at: $0) == nil }
    }

    private func toggleSelection(_ id: String) {
        guard let path = dataSource.indexPath(for: id) else { return }
        if collection.selectionIndexPaths.contains(path) { collection.deselectItems(at: [path]) }
        else { collection.selectItems(at: [path], scrollPosition: []) }
        onSelection?(collection.selectionIndexPaths.count)
    }

    func toggleSelectAll() {
        let images = Set(visibleItems().compactMap { dataSource.indexPath(for: $0.id) })
        if !images.isEmpty, collection.selectionIndexPaths == images {
            collection.deselectAll(nil)
        } else {
            collection.selectionIndexPaths = images
        }
        onSelection?(collection.selectionIndexPaths.count)
    }

    func clearSelection() {
        collection.deselectAll(nil)
        onSelection?(0)
    }

    func collectionView(_ cv: NSCollectionView, didSelectItemsAt indexPaths: Set<IndexPath>) { onSelection?(cv.selectionIndexPaths.count) }
    func collectionView(_ cv: NSCollectionView, didDeselectItemsAt indexPaths: Set<IndexPath>) { onSelection?(cv.selectionIndexPaths.count) }

    // MARK: Actions

    /// `announce` is off when the tile's own copy animation says it.
    func copy(_ items: [VaultItem], announce: Bool = true) {
        guard !items.isEmpty else { return }
        Clipboard.copy(items)
        if announce { onMessage?(items.count == 1 ? "Copied — ⌘V anywhere" : "\(items.count) copied — ⌘V anywhere") }
    }

    func copySelection() {
        let items = selectedItems()
        let paths = collection.selectionIndexPaths
        let onScreen = paths.filter { collection.visibleRect.intersects((collection.layoutAttributesForItem(at: $0)?.frame ?? .zero)) }
        // The overlay on each tile is the confirmation; text only when none are on screen.
        copy(items, announce: onScreen.isEmpty)
        for (i, path) in paths.sorted().enumerated() {
            let tile = (collection.item(at: path) as? TileItem)?.tile
            DispatchQueue.main.asyncAfter(deadline: .now() + min(0.4, Double(i) * 0.025)) { tile?.showCopied() }
        }
    }

    func revealSelection() {
        NSWorkspace.shared.activateFileViewerSelecting(selectedItems().map { VaultStore.shared.fileURL(for: $0) })
    }

    func deleteSelection() {
        let items = selectedItems()
        guard !items.isEmpty else { return }
        VaultModel.shared.delete(ids: items.map(\.id))
        onMessage?("\(items.count) deleted — ⌘Z to undo")
    }

    // MARK: Text, paths, menus

    func copyTextSelection() { copyText(selectedItems()) }

    /// OCR on a background task; copies one combined text block. Capped so a
    /// whole-grid selection can't kick off a five-minute Vision job.
    func copyText(_ items: [VaultItem]) {
        guard !items.isEmpty else { return }
        let targets = Array(items.prefix(10))
        onMessage?(targets.count == 1 ? "Reading text…" : "Reading \(targets.count) images…")
        Task { [weak self] in
            var chunks: [String] = []
            for item in targets {
                if let text = await OCR.text(at: VaultStore.shared.fileURL(for: item)) {
                    chunks.append(text)
                    // Store OCR text in vault for future search
                    await MainActor.run {
                        VaultStore.shared.mutate { index in
                            if let i = index.items.firstIndex(where: { $0.id == item.id }) {
                                index.items[i].ocrText = text
                            }
                        }
                    }
                }
            }
            guard let self else { return }
            let text = chunks.joined(separator: "\n\n")
            if text.isEmpty {
                self.onMessage?("No text found")
            } else {
                Clipboard.copyText(text)
                let capped = items.count > targets.count ? " (first \(targets.count))" : ""
                self.onMessage?("Text copied · \(text.count) characters\(capped)")
            }
        }
    }

    private func copyPaths(_ items: [VaultItem]) {
        let paths = items.map { VaultStore.shared.fileURL(for: $0).path }
        guard !paths.isEmpty else { return }
        Clipboard.copyText(paths.joined(separator: "\n"))
        onMessage?(paths.count == 1 ? "Path copied" : "\(paths.count) paths copied")
    }

    private func open(_ items: [VaultItem]) {
        guard let first = items.first else { return }
        NSWorkspace.shared.open(VaultStore.shared.fileURL(for: first))
    }

    private func reveal(_ items: [VaultItem]) {
        guard !items.isEmpty else { return }
        NSWorkspace.shared.activateFileViewerSelecting(items.map { VaultStore.shared.fileURL(for: $0) })
    }

    private func menu(for path: IndexPath) -> NSMenu? {
        if let gid = folderID(at: path) { return folderMenu(gid) }
        guard let id = dataSource.itemIdentifier(for: path),
              let item = VaultModel.shared.byID[id] else { return nil }
        onSelection?(collection.selectionIndexPaths.count)
        let items = selectedItems()
        let targets = items.isEmpty ? [item] : items
        let menu = NSMenu()
        menu.autoenablesItems = false
        menu.appearance = NSAppearance(named: .darkAqua)
        menu.addItem(ClosureMenuItem(title: "Copy Image", key: "c") { [weak self] in self?.copy(targets) })
        menu.addItem(ClosureMenuItem(title: "Copy Text", key: "t") { [weak self] in self?.copyText(targets) })
        menu.addItem(ClosureMenuItem(title: "Copy Path") { [weak self] in self?.copyPaths(targets) })
        if targets.count == 1, item.mime == "image/svg+xml" {
            menu.addItem(ClosureMenuItem(title: "Copy SVG Code") { [weak self] in
                if !Clipboard.copySVGCode(item) { self?.onMessage?("Couldn't read the SVG") } else { self?.onMessage?("SVG code copied") }
            })
        }
        menu.addItem(ClosureMenuItem(title: "Edit Image") { [weak self] in self?.onEdit?(item) })
        menu.addItem(.separator())
        let groupItem = NSMenuItem(title: "Move to Folder", action: nil, keyEquivalent: "")
        let groups = NSMenu()
        groups.addItem(ClosureMenuItem(title: "New Folder") { [weak self] in
            guard let self else { return }
            self.collection.selectionIndexPaths = Set(targets.compactMap { self.dataSource.indexPath(for: $0.id) })
            self.groupSelection()
        })
        let model = VaultModel.shared
        if !model.groups.isEmpty { groups.addItem(.separator()) }
        for g in model.groups {
            let entry = ClosureMenuItem(title: g.name) { [weak self] in self?.moveToGroup(g.id, targets) }
            entry.state = targets.allSatisfy({ model.groupOf[$0.id] == g.id }) ? .on : .off
            groups.addItem(entry)
        }
        groupItem.submenu = groups
        menu.addItem(groupItem)
        if targets.contains(where: { model.groupOf[$0.id] != nil }) {
            menu.addItem(ClosureMenuItem(title: "Take Out of Folder") { [weak self] in self?.removeFromGroup(targets) })
        }
        menu.addItem(.separator())
        menu.addItem(ClosureMenuItem(title: "Open") { [weak self] in self?.open(targets) })
        menu.addItem(ClosureMenuItem(title: "Reveal in Finder") { [weak self] in self?.reveal(targets) })
        menu.addItem(.separator())
        menu.addItem(ClosureMenuItem(title: "Get Info") { [weak self] in
            self?.preview.show(item, in: self?.visibleItems() ?? [])
        })
        menu.addItem(.separator())
        menu.addItem(ClosureMenuItem(title: "Delete", key: "\u{8}", modifiers: []) { [weak self] in
            self?.deleteSelection()
        })
        return menu
    }

    @objc private func doubleClicked(_ g: NSClickGestureRecognizer) {
        guard let path = collection.indexPathForItem(at: g.location(in: collection)),
              let id = dataSource.itemIdentifier(for: path), let item = VaultModel.shared.byID[id] else { return }
        preview.show(item, in: visibleItems())
    }

    private func handleKey(_ event: NSEvent) -> Bool {
        if !preview.isHidden { return preview.handleKey(event) }
        let cmd = event.modifierFlags.contains(.command)
        let key = event.charactersIgnoringModifiers?.lowercased() ?? ""
        switch (cmd, key, event.keyCode) {
        case (true, "c", _): copySelection(); return true
        case (true, "t", _): copyTextSelection(); return true
        case (true, "a", _): toggleSelectAll(); return true
        case (true, "z", _):
            let n = VaultModel.shared.undoDelete()
            if n > 0 { onMessage?("Restored \(n)") }
            return true
        case (_, _, 51), (_, _, 117): deleteSelection(); return true
        case (false, "/", _): onSearch?(); return true
        case (false, " ", _), (false, "\r", _):
            if let first = selectedItems().first { preview.show(first, in: visibleItems()) }
            return true
        case (_, _, 53):
            if collection.selectionIndexPaths.isEmpty { onEscape?() } else { clearSelection() }
            return true
        default:
            return false
        }
    }

    // MARK: Drag out — real files, so every app receives every selected image

    func collectionView(_ cv: NSCollectionView, canDragItemsAt indexPaths: Set<IndexPath>, with event: NSEvent) -> Bool {
        !indexPaths.contains { folderID(at: $0) != nil }
    }

    func collectionView(_ cv: NSCollectionView, pasteboardWriterForItemAt indexPath: IndexPath) -> NSPasteboardWriting? {
        guard let id = dataSource.itemIdentifier(for: indexPath), let item = VaultModel.shared.byID[id] else { return nil }
        return VaultStore.shared.fileURL(for: item) as NSURL
    }

    func collectionView(_ cv: NSCollectionView, draggingSession session: NSDraggingSession, willBeginAt screenPoint: NSPoint, forItemsAt indexPaths: Set<IndexPath>) {
        session.draggingFormation = .stack
        session.animatesToStartingPositionsOnCancelOrFail = true
        draggedIDs = indexPaths.sorted().compactMap { dataSource.itemIdentifier(for: $0) }
            .filter { !$0.hasPrefix(Self.folderPrefix) }
        // Tag the drag as ours (on top of the real file URLs other apps read).
        session.draggingPasteboard.addTypes([Self.tileType], owner: nil)
        session.draggingPasteboard.setData(Data(), forType: Self.tileType)
        isDraggingOut = true
        onDragOut?(true)
    }

    func collectionView(_ cv: NSCollectionView, draggingSession session: NSDraggingSession, endedAt screenPoint: NSPoint, dragOperation operation: NSDragOperation) {
        isDraggingOut = false
        onDragOut?(false)
        setDropTarget(nil)
        let wasInternal = operation == .move
        draggedIDs = []
        guard !wasInternal else { return }
        let n = session.draggingPasteboard.pasteboardItems?.count ?? 0
        if !operation.isEmpty { onMessage?(n > 1 ? "Dropped \(n) images" : "Dropped") }
    }

    /// Our own tiles over a folder (add to it) or over another image (make a
    /// folder of both). Anything else isn't a drop target inside the grid.
    func collectionView(_ cv: NSCollectionView, validateDrop info: NSDraggingInfo, proposedIndexPath: AutoreleasingUnsafeMutablePointer<NSIndexPath>, dropOperation: UnsafeMutablePointer<NSCollectionView.DropOperation>) -> NSDragOperation {
        guard (info.draggingSource as? NSCollectionView) === collection, !draggedIDs.isEmpty,
              let path = collection.indexPathForItem(at: collection.convert(info.draggingLocation, from: nil)),
              let id = dataSource.itemIdentifier(for: path) else { setDropTarget(nil); return [] }
        if id.hasPrefix(Self.folderPrefix) {
            let gid = String(id.dropFirst(Self.folderPrefix.count))
            // Already all in this folder: nothing to do.
            guard draggedIDs.contains(where: { VaultModel.shared.groupOf[$0] != gid }) else { setDropTarget(nil); return [] }
        } else if draggedIDs.contains(id) {
            setDropTarget(nil)
            return []
        }
        proposedIndexPath.pointee = path as NSIndexPath
        dropOperation.pointee = .on
        setDropTarget(id)
        return .move
    }

    func collectionView(_ cv: NSCollectionView, acceptDrop info: NSDraggingInfo, indexPath: IndexPath, dropOperation: NSCollectionView.DropOperation) -> Bool {
        guard let id = dataSource.itemIdentifier(for: indexPath), !draggedIDs.isEmpty else { return false }
        let dragged = draggedIDs.compactMap { VaultModel.shared.byID[$0] }
        setDropTarget(nil)
        collection.deselectAll(nil)
        if id.hasPrefix(Self.folderPrefix) {
            let gid = String(id.dropFirst(Self.folderPrefix.count))
            folderCell(gid).map { Theme.pop($0.folder.layer!, from: 1.12) }
            moveToGroup(gid, dragged)
        } else if let gid = VaultModel.shared.groupOf[id] {
            // Dropped on an image that's out of an open folder: join that folder.
            moveToGroup(gid, dragged)
        } else {
            let n = VaultModel.shared.groups.count + 1
            var name = "Group \(n)", k = n
            while VaultModel.shared.groups.contains(where: { $0.name == name }) { k += 1; name = "Group \(k)" }
            let group = VaultStore.shared.createGroup(name: name, ids: [id] + dragged.map(\.id))
            outCount[group.id] = 0
            VaultModel.shared.reload()
            collection.scroll(.zero)
            onMessage?("\(dragged.count + 1) in \(name)")
        }
        return true
    }

    /// Lights up the folder or tile under a drag.
    private func setDropTarget(_ id: String?) {
        guard id != dropTargetID else { return }
        for old in [dropTargetID, id].compactMap({ $0 }) {
            guard let path = dataSource.indexPath(for: old) else { continue }
            let on = old == id
            if let f = collection.item(at: path) as? FolderItem { f.folder.dropTarget = on }
            if let t = collection.item(at: path) as? TileItem { t.tile.dropTarget = on }
        }
        dropTargetID = id
    }
}
