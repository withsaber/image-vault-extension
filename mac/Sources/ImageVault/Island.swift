import AppKit
import SwiftUI

// MARK: - Model

enum IslandPosition: String, CaseIterable {
    case left, center, right
    var title: String { rawValue.capitalized }
}

enum IslandTab: String {
    case vault, clipboard
}

enum IslandState: Equatable {
    case collapsed   // resting notch
    case peek        // pointer arrived: tiny "breath" before opening
    case activity    // live activity after a save
    case dragHint    // an image is being dragged somewhere on the Mac
    case dragOver    // ...and it's over the island: big drop target
    case expanded    // the tray
}

enum SortOrder: String, CaseIterable {
    case newest, oldest, name, size

    var title: String {
        switch self {
        case .newest: return "Newest"
        case .oldest: return "Oldest"
        case .name: return "Name"
        case .size: return "Size"
        }
    }

    var symbol: String {
        switch self {
        case .newest: return "arrow.down.to.line"
        case .oldest: return "arrow.up.to.line"
        case .name: return "textformat"
        case .size: return "externaldrive"
        }
    }

    var next: SortOrder {
        let all = SortOrder.allCases
        guard let index = all.firstIndex(of: self) else { return .newest }
        return all[(index + 1) % all.count]
    }
}

@MainActor
final class IslandModel: ObservableObject {
    @Published var state: IslandState = .collapsed
    @Published var position: IslandPosition = .center
    @Published var notch = CGSize(width: 190, height: 32)
    @Published var hardwareNotch = false
    @Published var expandedSize = CGSize(width: 640, height: 300)
    @Published var count = 0
    @Published var visibleCount = 0
    @Published var selection = 0
    @Published var query = ""
    @Published var sort: SortOrder = .newest
    @Published var searchRequested = 0
    @Published var message: String?
    @Published var activityImage: NSImage?
    @Published var activityTitle = ""
    @Published var activityDetail = ""
    @Published var dropBusy = false
    /// When false the resting notch is invisible; hovering its spot at the
    /// top of the screen still opens the tray.
    @Published var showNotch = true
    @Published var tab: IslandTab = .vault

    /// Height of the resting notch.
    static let restHeight: CGFloat = 10

    var isOpen: Bool { state == .expanded || state == .dragOver }

    /// Body size (without the flared "ears" at the top).
    func size(for state: IslandState) -> CGSize {
        switch state {
        case .collapsed: return CGSize(width: notch.width, height: Self.restHeight)
        case .peek: return CGSize(width: notch.width + 18, height: Self.restHeight + 5)
        case .activity: return CGSize(width: notch.width + 176, height: notch.height + 2)
        case .dragHint: return CGSize(width: max(notch.width + 170, 340), height: notch.height + 46)
        case .dragOver, .expanded: return expandedSize
        }
    }

    func radii(for state: IslandState) -> (top: CGFloat, bottom: CGFloat) {
        switch state {
        case .collapsed, .peek, .activity: return (6, min(14, size(for: state).height * 0.45))
        case .dragHint: return (10, 20)
        case .dragOver, .expanded: return (16, 30)
        }
    }

    /// Full outline size including the ears.
    func outline(for state: IslandState) -> CGSize {
        let s = size(for: state)
        return CGSize(width: s.width + radii(for: state).top * 2, height: s.height)
    }

    var alignment: Alignment {
        switch position {
        case .left: return .topLeading
        case .center: return .top
        case .right: return .topTrailing
        }
    }

    var band: CGFloat { max(notch.height, 30) }
}

// MARK: - Motion

enum Motion {
    /// Opening: quick, a touch of overshoot (the Dynamic Island "pop").
    static let open = Animation.spring(response: 0.42, dampingFraction: 0.74, blendDuration: 0.1)
    /// Closing: tighter, no bounce, so it tucks back into the notch.
    static let close = Animation.spring(response: 0.36, dampingFraction: 0.9, blendDuration: 0.1)
    /// Content fades after the shell has started moving.
    static let contentIn = Animation.spring(response: 0.38, dampingFraction: 0.86).delay(0.07)
    static let contentOut = Animation.easeOut(duration: 0.12)
    static let hover = Animation.spring(response: 0.26, dampingFraction: 0.62)
    /// Clipboard card growing over the grid and back.
    static let expand = Animation.spring(response: 0.44, dampingFraction: 0.84)
}

/// Content reveal: fade, unblur and settle from 94% scale, anchored at the top.
struct Reveal: ViewModifier {
    let visible: Bool
    func body(content: Content) -> some View {
        content
            .opacity(visible ? 1 : 0)
            .blur(radius: visible ? 0 : 7)
            .scaleEffect(visible ? 1 : 0.94, anchor: .top)
            .animation(visible ? Motion.contentIn : Motion.contentOut, value: visible)
    }
}

// MARK: - Shape

/// Notch outline: flared top corners that melt into the screen edge, round bottom.
struct NotchShape: Shape {
    var top: CGFloat
    var bottom: CGFloat

    var animatableData: AnimatablePair<CGFloat, CGFloat> {
        get { AnimatablePair(top, bottom) }
        set { top = newValue.first; bottom = newValue.second }
    }

    func path(in rect: CGRect) -> Path {
        let t = min(top, rect.width / 4)
        let b = max(0, min(bottom, rect.height / 2, (rect.width - 2 * t) / 2))
        var p = Path()
        p.move(to: CGPoint(x: rect.minX, y: rect.minY))
        p.addQuadCurve(to: CGPoint(x: rect.minX + t, y: rect.minY + t), control: CGPoint(x: rect.minX + t, y: rect.minY))
        p.addLine(to: CGPoint(x: rect.minX + t, y: rect.maxY - b))
        p.addQuadCurve(to: CGPoint(x: rect.minX + t + b, y: rect.maxY), control: CGPoint(x: rect.minX + t, y: rect.maxY))
        p.addLine(to: CGPoint(x: rect.maxX - t - b, y: rect.maxY))
        p.addQuadCurve(to: CGPoint(x: rect.maxX - t, y: rect.maxY - b), control: CGPoint(x: rect.maxX - t, y: rect.maxY))
        p.addLine(to: CGPoint(x: rect.maxX - t, y: rect.minY + t))
        p.addQuadCurve(to: CGPoint(x: rect.maxX, y: rect.minY), control: CGPoint(x: rect.maxX - t, y: rect.minY))
        p.closeSubpath()
        return p
    }
}

// MARK: - Views

struct IslandActions {
    var copy: () -> Void = {}
    var copyText: () -> Void = {}
    var group: () -> Void = {}
    var reveal: () -> Void = {}
    var delete: () -> Void = {}
    var clear: () -> Void = {}
    var selectAll: () -> Void = {}
    var menu: () -> Void = {}
    var flash: (String) -> Void = { _ in }
}

struct TrayRepresentable: NSViewControllerRepresentable {
    let controller: TrayController
    func makeNSViewController(context: Context) -> TrayController { controller }
    func updateNSViewController(_ vc: TrayController, context: Context) {}
}

struct IslandView: View {
    @ObservedObject var model: IslandModel
    let tray: TrayController
    let actions: IslandActions
    @ObservedObject var nowPlaying: NowPlaying
    @ObservedObject var clips = ClipboardHistory.shared
    @State private var confirmClear = false
    @State private var searchOpen = false
    @FocusState private var searchFocused: Bool
    @Namespace private var tabSpace

    var body: some View {
        let r = model.radii(for: model.state)
        let outline = model.outline(for: model.state)
        ZStack(alignment: .top) {
            NotchShape(top: r.top, bottom: r.bottom)
                .fill(Color.black)
                .shadow(color: .black.opacity(model.isOpen ? 0.55 : 0), radius: 24, x: 0, y: 12)
                .opacity(model.state == .collapsed && !model.showNotch ? 0 : 1)
                .animation(Motion.contentOut, value: model.showNotch)

            ZStack(alignment: .top) {
                activity
                dragHint
                expanded
                dragOver
            }
            .frame(width: outline.width, height: outline.height, alignment: .top)
            .clipShape(NotchShape(top: r.top, bottom: r.bottom))
        }
        .frame(width: outline.width, height: outline.height)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: model.alignment)
        .padding(.horizontal, IslandController.pad)
        .preferredColorScheme(.dark)
        .onChange(of: model.query) { _ in
            DispatchQueue.main.async { tray.setFilter(query: model.query, sort: model.sort) }
        }
        .onChange(of: model.sort) { _ in
            DispatchQueue.main.async { tray.setFilter(query: model.query, sort: model.sort) }
        }
        .onChange(of: model.searchRequested) { _ in
            searchOpen = true
            DispatchQueue.main.async { searchFocused = true }
        }
    }

    // A layer that stays laid out at its *final* size and is revealed by the
    // growing shell, which is what makes the island feel like one object.
    private func layer<Content: View>(_ visible: Bool, size: CGSize, @ViewBuilder _ content: () -> Content) -> some View {
        content()
            .frame(width: size.width, height: size.height, alignment: .top)
            .modifier(Reveal(visible: visible))
            .allowsHitTesting(visible)
    }

    private var activity: some View {
        layer(model.state == .activity, size: model.size(for: .activity)) {
            HStack(spacing: 0) {
                Group {
                    if let image = model.activityImage {
                        Image(nsImage: image).resizable().aspectRatio(contentMode: .fill)
                    } else {
                        Image(systemName: "photo").foregroundStyle(.white)
                    }
                }
                .frame(width: 22, height: 22)
                .clipShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous).stroke(.white.opacity(0.25), lineWidth: 0.5))
                Spacer(minLength: model.hardwareNotch ? model.notch.width : 8)
                VStack(alignment: .trailing, spacing: 0) {
                    HStack(spacing: 4) {
                        Image(systemName: "checkmark.circle.fill").font(.system(size: 11, weight: .bold))
                        Text(model.activityTitle).font(.system(size: 12, weight: .semibold))
                    }
                    if !model.activityDetail.isEmpty {
                        Text(model.activityDetail).font(.system(size: 9.5, design: .monospaced)).foregroundStyle(.white.opacity(0.55))
                    }
                }
                .foregroundStyle(.white)
            }
            .padding(.horizontal, 12)
            .frame(maxHeight: .infinity)
        }
    }

    private var dragHint: some View {
        layer(model.state == .dragHint, size: model.size(for: .dragHint)) {
            VStack(spacing: 0) {
                Color.clear.frame(height: model.band)
                HStack(spacing: 7) {
                    Image(systemName: "arrow.down.circle.fill")
                        .font(.system(size: 14, weight: .medium))
                    Text("Drop here to save").font(.system(size: 12, weight: .semibold))
                }
                .foregroundStyle(.white)
                .frame(maxHeight: .infinity)
            }
        }
    }

    private var dragOver: some View {
        layer(model.state == .dragOver, size: model.size(for: .dragOver)) {
            VStack(spacing: 0) {
                Color.clear.frame(height: model.band)
                DropTarget(busy: model.dropBusy, active: model.state == .dragOver)
                    .padding(.horizontal, 22)
                    .padding(.bottom, 20)
                    .padding(.top, 6)
            }
        }
    }

    /// The tray hosts an AppKit grid: scaling or blurring it makes AppKit snap
    /// every tile to whole pixels each frame (the jitter on open). So only the
    /// SwiftUI parts get the scale + blur reveal; the grid just fades.
    private var expanded: some View {
        let visible = model.state == .expanded
        let size = model.size(for: .expanded)
        return VStack(spacing: 0) {
                header
                    .modifier(Reveal(visible: visible))
                if searchOpen || !model.query.isEmpty {
                    searchBar
                        .transition(.move(edge: .top).combined(with: .opacity))
                }
                ZStack(alignment: .bottom) {
                    TrayRepresentable(controller: tray)
                        .opacity(model.tab == .vault ? 1 : 0)
                        .allowsHitTesting(model.tab == .vault)
                    if model.tab == .clipboard {
                        ClipboardGrid(history: clips, query: model.query) { text in actions.flash(text) }
                            .transition(.opacity)
                    }
                    if model.tab == .vault, model.count == 0 { EmptyTray(active: model.state == .expanded) }
                    if model.tab == .vault, model.count > 0, model.visibleCount == 0, !model.query.isEmpty {
                        VStack(spacing: 7) {
                            Image(systemName: "magnifyingglass").font(.system(size: 18, weight: .light))
                            Text("No matches").font(.system(size: 11.5, weight: .medium))
                        }
                        .foregroundStyle(.white.opacity(0.45))
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                    }
                    if model.tab == .vault, model.selection >= 2 {
                        BulkBar(count: model.selection, actions: actions)
                            .padding(.bottom, 12)
                            .transition(.move(edge: .bottom).combined(with: .opacity))
                    }
                }
                .animation(Motion.open, value: model.selection >= 2)
                .animation(Motion.contentOut, value: model.tab)
                if nowPlaying.track != nil {
                    NowPlayingBar(service: nowPlaying)
                        .padding(.horizontal, 16)
                        .padding(.bottom, 10)
                        .modifier(Reveal(visible: visible))
                        .transition(.move(edge: .bottom).combined(with: .opacity))
                }
            }
            .animation(Motion.hover, value: nowPlaying.track != nil)
            .animation(Motion.hover, value: searchOpen)
            .frame(width: size.width, height: size.height, alignment: .top)
            .opacity(visible ? 1 : 0)
            .animation(visible ? Motion.contentIn : Motion.contentOut, value: visible)
            .allowsHitTesting(visible)
    }

    private var searchBar: some View {
        HStack(spacing: 7) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 10, weight: .medium))
                .foregroundStyle(.white.opacity(0.45))
            TextField(model.tab == .vault ? "Search name, site, source or text in images" : "Search copied items",
                      text: $model.query)
                .textFieldStyle(.plain)
                .font(.system(size: 11))
                .foregroundStyle(.white)
                .focused($searchFocused)
                .onSubmit {
                    searchFocused = false
                    tray.focus()
                }
                .onExitCommand {
                    if !model.query.isEmpty {
                        model.query = ""
                    } else {
                        closeSearch()
                    }
                }
            if !model.query.isEmpty {
                Button {
                    model.query = ""
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 10))
                        .foregroundStyle(.white.opacity(0.4))
                }
                .buttonStyle(.plain)
                .help("Clear  Esc")
                if model.tab == .vault {
                    Text("\(model.visibleCount) of \(model.count)")
                        .font(.system(size: 9, design: .monospaced))
                        .foregroundStyle(.white.opacity(0.38))
                        .fixedSize()
                }
            }
            Spacer(minLength: 4)
            if model.tab == .vault {
            Button {
                model.sort = model.sort.next
            } label: {
                HStack(spacing: 3) {
                    Image(systemName: model.sort.symbol).font(.system(size: 9, weight: .semibold))
                    Text(model.sort.title).font(.system(size: 9.5, weight: .medium))
                }
                .foregroundStyle(.white.opacity(0.62))
                .padding(.horizontal, 7)
                .frame(height: 20)
                .background(Capsule().fill(.white.opacity(0.07)))
            }
            .buttonStyle(.plain)
            .help("Sort: \(model.sort.title) — click to change")
            }
        }
        .padding(.horizontal, 9)
        .frame(height: 26)
        .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(.white.opacity(0.06)))
        .padding(.horizontal, 22)
        .padding(.bottom, 5)
    }

    private var countLabel: String {
        if model.tab == .clipboard {
            let shown = clips.items.filter { clips.matches($0, query: model.query) }.count
            return model.query.isEmpty ? "\(clips.items.count)" : "\(shown)/\(clips.items.count)"
        }
        return model.query.isEmpty ? "\(model.count)" : "\(model.visibleCount)/\(model.count)"
    }

    private func select(_ tab: IslandTab) {
        guard model.tab != tab else { return }
        confirmClear = false
        if tab == .clipboard { tray.clearSelection() }
        withAnimation(Motion.hover) { model.tab = tab }
        if tab == .vault { DispatchQueue.main.async { tray.focus() } }
    }

    /// Two clicks: the first arms it (icon turns into a check), the second clears.
    private func resetClipboard() {
        if confirmClear {
            clips.clear()
            confirmClear = false
            actions.flash("Clipboard history cleared")
        } else {
            guard !clips.items.isEmpty else { actions.flash("Clipboard history is empty"); return }
            confirmClear = true
            actions.flash("Click again to clear")
            DispatchQueue.main.asyncAfter(deadline: .now() + 3) { confirmClear = false }
        }
    }

    private func closeSearch() {
        model.query = ""
        searchFocused = false
        searchOpen = false
        tray.focus()
    }

    private var vaultSize: String {
        let bytes = VaultModel.shared.items.reduce(Int64(0)) { $0 + Int64($1.bytes ?? 0) }
        return ByteCountFormatter.string(fromByteCount: bytes, countStyle: .file)
    }

    private var header: some View {
        HStack(spacing: 8) {
            HStack(spacing: 8) {
                HStack(spacing: 2) {
                    IslandTabButton(symbol: "photo.on.rectangle.angled", help: "Vault — saved images",
                                    selected: model.tab == .vault, namespace: tabSpace) { select(.vault) }
                    IslandTabButton(symbol: "list.clipboard", help: "Clipboard — everything you copied",
                                    selected: model.tab == .clipboard, namespace: tabSpace) { select(.clipboard) }
                }
                .padding(2)
                .background(Capsule().fill(.white.opacity(0.07)))
                ZStack(alignment: .leading) {
                    if let message = model.message {
                        Text(message)
                            .font(.system(size: 10.5, weight: .medium))
                            .foregroundStyle(.white.opacity(0.85))
                            .lineLimit(1)
                            .truncationMode(.tail)
                            .transition(.asymmetric(insertion: .move(edge: .bottom).combined(with: .opacity),
                                                    removal: .opacity))
                    } else {
                        Text(countLabel)
                            .font(.system(size: 10, design: .monospaced))
                            .foregroundStyle(.white.opacity(0.5))
                            .padding(.horizontal, 6).padding(.vertical, 1)
                            .overlay(RoundedRectangle(cornerRadius: 3).stroke(.white.opacity(0.15)))
                            .help(model.tab == .vault ? "\(model.count) images · \(vaultSize)" : "\(clips.items.count) copied items")
                            .transition(.opacity)
                    }
                }
                .frame(maxWidth: 190, alignment: .leading)
                .animation(Motion.hover, value: model.message)
            }
            .foregroundStyle(.white)
            Spacer(minLength: model.hardwareNotch && model.position == .center ? model.notch.width + 16 : 12)
            HStack(spacing: 2) {
                IslandButton(symbol: "magnifyingglass", help: searchOpen ? "Close search  Esc" : "Search  /") {
                    if searchOpen {
                        closeSearch()
                    } else {
                        searchOpen = true
                        DispatchQueue.main.async { searchFocused = true }
                    }
                }
                if model.tab == .vault {
                    IslandButton(symbol: "folder", help: "Show in Finder", action: actions.reveal)
                    IslandButton(symbol: "gearshape", help: "Settings", action: actions.menu)
                    IslandButton(symbol: nil, help: "Select all  ⌘A", action: actions.selectAll) { color in
                        SelectAllGlyph(filled: model.selection > 0
                                       && model.selection == (model.query.isEmpty ? model.count : model.visibleCount),
                                       color: color)
                    }
                } else {
                    IslandButton(symbol: "gearshape", help: "Settings", action: actions.menu)
                    IslandButton(symbol: confirmClear ? "checkmark.circle" : "arrow.counterclockwise",
                                 help: confirmClear ? "Click again to clear the clipboard history" : "Reset clipboard history") {
                        resetClipboard()
                    }
                }
            }
        }
        .padding(.horizontal, 22)
        .frame(height: model.band + 8)
    }
}

/// Vault / Clipboard switch in the header: icons only, with a white disc
/// that slides between them.
struct IslandTabButton: View {
    let symbol: String
    let help: String
    let selected: Bool
    let namespace: Namespace.ID
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Image(systemName: symbol)
            .font(.system(size: 11, weight: .semibold))
            .foregroundStyle(selected ? Color.black : .white.opacity(hovering ? 0.95 : 0.55))
            .frame(width: 30, height: 22)
            .background {
                if selected {
                    Capsule().fill(Color.white).matchedGeometryEffect(id: "tab", in: namespace)
                } else {
                    Capsule().fill(.white.opacity(hovering ? 0.1 : 0))
                }
            }
            .scaleEffect(hovering && !selected ? 1.06 : 1)
            .contentShape(Capsule())
            .onHover { hovering = $0 }
            .onTapGesture(perform: action)
            .animation(Motion.hover, value: hovering)
            .help(help)
    }
}

/// Clipboard history as a 3-column grid. Click a card to copy it again;
/// the expand button grows a card over the grid to read all of it.
struct ClipboardGrid: View {
    @ObservedObject var history: ClipboardHistory
    let query: String
    let flash: (String) -> Void
    @Namespace private var space
    @State private var expanded: String?
    /// Snapshot aid: open this clip expanded.
    static var snapshotExpand: String?

    private let columns = Array(repeating: GridItem(.flexible(), spacing: 8), count: 3)

    var body: some View {
        let items = history.items.filter { history.matches($0, query: query) }
        if history.items.isEmpty {
            VStack(spacing: 8) {
                Image(systemName: "doc.on.clipboard").font(.system(size: 22, weight: .light))
                Text("NOTHING COPIED YET").font(.system(size: 10.5, weight: .bold)).kerning(2.2)
                Text(history.enabled ? "Text and files you copy show up here · images go to the vault"
                                     : "Clipboard History is off in Settings")
                    .font(.system(size: 11)).foregroundStyle(.white.opacity(0.45))
            }
            .foregroundStyle(.white)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if items.isEmpty {
            VStack(spacing: 7) {
                Image(systemName: "magnifyingglass").font(.system(size: 18, weight: .light))
                Text("No matches").font(.system(size: 11.5, weight: .medium))
            }
            .foregroundStyle(.white.opacity(0.45))
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            ZStack(alignment: .top) {
                ScrollView(.vertical, showsIndicators: false) {
                    LazyVGrid(columns: columns, spacing: 8) {
                        ForEach(items) { item in
                            if expanded == item.id {
                                Color.clear.frame(height: ClipCard.height)
                            } else {
                                ClipCard(item: item, history: history, flash: flash) {
                                    withAnimation(Motion.expand) { expanded = item.id }
                                }
                                .matchedGeometryEffect(id: item.id, in: space)
                                .zIndex(expanded == nil ? 0 : 1)
                            }
                        }
                    }
                    .padding(.horizontal, 16)
                    .padding(.top, 4)
                    .padding(.bottom, 16)
                }
                .opacity(expanded == nil ? 1 : 0)
                .allowsHitTesting(expanded == nil)

                if let id = expanded, let item = history.items.first(where: { $0.id == id }) {
                    ExpandedClip(item: item, history: history) {
                        withAnimation(Motion.expand) { expanded = nil }
                    }
                    .matchedGeometryEffect(id: id, in: space)
                    .padding(.horizontal, 16)
                    .padding(.top, 4)
                    .padding(.bottom, 12)
                    .zIndex(5)
                }
            }
            .onChange(of: query) { _ in expanded = nil }
            .onAppear { if let id = Self.snapshotExpand { expanded = id } }
        }
    }
}

/// Rendered previews of copied SVG code, cached per clip.
enum SVGPreview {
    private static let cache = NSCache<NSString, NSImage>()

    static func image(for item: ClipItem) -> NSImage? {
        guard item.kind == .text, let text = item.text, isSVG(item) else { return nil }
        if let hit = cache.object(forKey: item.id as NSString) { return hit }
        guard let image = NSImage(data: Data(text.utf8)), image.size.width > 0 else { return nil }
        cache.setObject(image, forKey: item.id as NSString)
        return image
    }

    private static var known: [String: Bool] = [:]
    static func isSVG(_ item: ClipItem) -> Bool {
        if let hit = known[item.id] { return hit }
        let result = item.kind == .text && Imaging.isSVG(text: item.text ?? "")
        known[item.id] = result
        return result
    }

    /// A temporary .svg file so the card drags into Finder, Figma, browsers.
    static func file(for item: ClipItem) -> URL? {
        guard let text = item.text else { return nil }
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("ImageVault-svg", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let url = dir.appendingPathComponent("Copied SVG \(item.id.suffix(6)).svg")
        return (try? Data(text.utf8).write(to: url)) != nil ? url : nil
    }
}

/// Checkerboard behind transparent previews.
struct Checkerboard: View {
    var body: some View {
        Canvas { ctx, size in
            let s: CGFloat = 6
            for y in stride(from: 0, to: size.height, by: s) {
                for x in stride(from: 0, to: size.width, by: s) where (Int(x / s) + Int(y / s)) % 2 == 0 {
                    ctx.fill(Path(CGRect(x: x, y: y, width: s, height: s)), with: .color(.white.opacity(0.06)))
                }
            }
        }
    }
}

/// The copy confirmation that sits over a card: veil, a disc that springs
/// in and a check that draws itself.
struct CopiedOverlay: View {
    var label = "Copied"
    @State private var shown = false

    var body: some View {
        ZStack {
            Color.black.opacity(0.62)
            VStack(spacing: 5) {
                ZStack {
                    Circle().fill(.white)
                    CheckShape()
                        .trim(from: 0, to: shown ? 1 : 0)
                        .stroke(.black, style: StrokeStyle(lineWidth: 2, lineCap: .round, lineJoin: .round))
                        .frame(width: 10, height: 8)
                        .animation(.easeOut(duration: 0.26).delay(0.08), value: shown)
                }
                .frame(width: 24, height: 24)
                .scaleEffect(shown ? 1 : 0.3)
                .animation(.spring(response: 0.32, dampingFraction: 0.58), value: shown)
                Text(label)
                    .font(.system(size: 9.5, weight: .semibold))
                    .foregroundStyle(.white)
                    .opacity(shown ? 1 : 0)
                    .offset(y: shown ? 0 : 4)
                    .animation(Motion.contentIn, value: shown)
            }
        }
        .onAppear { shown = true }
    }
}

struct ClipCard: View {
    static let height: CGFloat = 70
    let item: ClipItem
    @ObservedObject var history: ClipboardHistory
    let flash: (String) -> Void
    var expand: () -> Void = {}
    @State private var hovering = false
    @State private var copied = false

    private var svg: NSImage? { SVGPreview.image(for: item) }

    var body: some View {
        ZStack(alignment: .bottomLeading) {
            content
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            footer
        }
        .frame(minHeight: Self.height)
        .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(.white.opacity(hovering ? 0.10 : 0.055)))
        .overlay {
            if copied { CopiedOverlay().transition(.opacity) }
        }
        .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous)
            .strokeBorder(.white.opacity(copied ? 0.9 : (hovering ? 0.28 : 0.1)), lineWidth: copied ? 1.2 : 0.5))
        .overlay(alignment: .topTrailing) {
            if hovering && !copied {
                HStack(spacing: 4) {
                    if item.kind == .text {
                        CardButton(symbol: "arrow.up.left.and.arrow.down.right", help: "Expand", action: expand)
                    }
                    CardButton(symbol: "doc.on.doc", help: svg != nil ? "Copy SVG code" : "Copy", action: copy)
                }
                .padding(5)
                .transition(.scale(scale: 0.6, anchor: .topTrailing).combined(with: .opacity))
            }
        }
        .scaleEffect(hovering && !copied ? 1.02 : 1)
        .animation(Motion.hover, value: hovering)
        .animation(Motion.hover, value: copied)
        .contentShape(RoundedRectangle(cornerRadius: 10))
        .onHover { hovering = $0 }
        .onTapGesture { copy() }
        .onDrag { provider }
        .contextMenu {
            Button(svg != nil ? "Copy SVG Code" : "Copy") { copy() }
            if item.kind == .text { Button("Expand") { expand() } }
            if svg != nil {
                Button("Save SVG to Vault") { saveSVG() }
            }
            Divider()
            Button("Delete") { withAnimation(Motion.hover) { history.delete(item) } }
        }
        .help(item.kind == .text ? (svg != nil ? "SVG code · click to copy" : String((item.text ?? "").prefix(400))) : item.preview)
    }

    @ViewBuilder private var content: some View {
        switch item.kind {
        case .text:
            if let svg {
                ZStack {
                    Checkerboard()
                    Image(nsImage: svg).resizable().aspectRatio(contentMode: .fit)
                        .padding(.horizontal, 10).padding(.top, 8).padding(.bottom, 20)
                }
            } else {
                Text(item.preview.trimmingCharacters(in: .whitespacesAndNewlines))
                    .font(.system(size: 10.5))
                    .foregroundStyle(.white.opacity(0.88))
                    .lineLimit(3)
                    .multilineTextAlignment(.leading)
                    .padding(.horizontal, 9)
                    .padding(.top, 8)
            }
        case .image:
            if let image = history.thumbnail(item) {
                Image(nsImage: image).resizable().aspectRatio(contentMode: .fill)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .clipped()
            }
        case .file:
            HStack(alignment: .top, spacing: 6) {
                Image(systemName: "doc").font(.system(size: 11)).foregroundStyle(.white.opacity(0.6))
                Text(item.preview).font(.system(size: 10.5, weight: .medium)).foregroundStyle(.white.opacity(0.88)).lineLimit(3)
            }
            .padding(.horizontal, 9)
            .padding(.top, 8)
        }
    }

    private var footer: some View {
        HStack(spacing: 4) {
            if svg != nil {
                Text("SVG").font(.system(size: 7, weight: .heavy)).foregroundStyle(.black)
                    .padding(.horizontal, 3).padding(.vertical, 1)
                    .background(RoundedRectangle(cornerRadius: 2).fill(.white.opacity(0.9)))
            } else {
                Image(systemName: item.kind == .text ? "text.alignleft" : (item.kind == .image ? "photo" : "doc"))
                    .font(.system(size: 7.5, weight: .semibold))
            }
            Text(item.app ?? (item.kind == .image ? item.preview : "")).lineLimit(1)
            Spacer(minLength: 4)
            Text(Self.age(item.date)).font(.system(size: 8.5, design: .monospaced))
        }
        .font(.system(size: 8.5, weight: .medium))
        .foregroundStyle(.white.opacity(0.5))
        .padding(.horizontal, 9)
        .padding(.vertical, 5)
        .background(item.kind == .image || svg != nil
                    ? AnyView(LinearGradient(colors: [.black.opacity(0.85), .clear], startPoint: .bottom, endPoint: .top))
                    : AnyView(Color.clear))
    }

    private var provider: NSItemProvider {
        switch item.kind {
        case .text:
            if svg != nil, let url = SVGPreview.file(for: item), let p = NSItemProvider(contentsOf: url) {
                p.registerObject((item.text ?? "") as NSString, visibility: .all)
                return p
            }
            return NSItemProvider(object: (item.text ?? "") as NSString)
        case .file:
            let first = (item.text ?? "").split(separator: "\n").first.map(String.init) ?? ""
            return NSItemProvider(contentsOf: URL(fileURLWithPath: first)) ?? NSItemProvider()
        case .image:
            return history.thumbnail(item).map { NSItemProvider(object: $0) } ?? NSItemProvider()
        }
    }

    private func copy() {
        history.copy(item)
        copied = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.1) { copied = false }
    }

    private func saveSVG() {
        guard let text = item.text else { return }
        Task { @MainActor in
            let summary = await Ingest.saveAsync([Ingest.Candidate(data: Data(text.utf8), name: "Copied SVG", url: nil)],
                                                 source: "clipboard")
            flash(summary.added.isEmpty ? (summary.isEmpty ? "Couldn't save the SVG" : "Already in the vault") : "SVG saved to vault")
        }
    }

    static func age(_ date: Date) -> String {
        let s = Int(-date.timeIntervalSinceNow)
        if s < 60 { return "now" }
        if s < 3600 { return "\(s / 60)m" }
        if s < 86_400 { return "\(s / 3600)h" }
        return "\(s / 86_400)d"
    }
}

/// Small round button on a card (expand, copy, close).
struct CardButton: View {
    let symbol: String
    let help: String
    let action: () -> Void
    var size: CGFloat = 18
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.system(size: size * 0.45, weight: .bold))
                .foregroundStyle(hovering ? Color.black : .white)
                .frame(width: size, height: size)
                .background(Circle().fill(hovering ? Color.white : .black.opacity(0.7)))
                .overlay(Circle().strokeBorder(.white.opacity(0.25), lineWidth: 0.5))
        }
        .buttonStyle(PressScale())
        .onHover { hovering = $0 }
        .animation(Motion.hover, value: hovering)
        .help(help)
    }
}

/// A clipboard card grown over the grid: the whole text (selectable), or an
/// SVG's preview beside its code. Close shrinks it back into its slot.
struct ExpandedClip: View {
    let item: ClipItem
    @ObservedObject var history: ClipboardHistory
    let close: () -> Void
    @State private var copied = false
    @State private var shown = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 6) {
                Image(systemName: SVGPreview.isSVG(item) ? "chevron.left.forwardslash.chevron.right" : "text.alignleft")
                    .font(.system(size: 9, weight: .semibold))
                Text(item.app ?? (SVGPreview.isSVG(item) ? "SVG" : "Text")).lineLimit(1)
                Text("· \(ClipCard.age(item.date)) · \((item.text ?? "").count) characters")
                    .font(.system(size: 9, design: .monospaced))
                    .foregroundStyle(.white.opacity(0.4))
                Spacer(minLength: 6)
                CardButton(symbol: "doc.on.doc", help: "Copy", action: copy, size: 22)
                CardButton(symbol: "arrow.down.right.and.arrow.up.left", help: "Close  Esc", action: close, size: 22)
                    .keyboardShortcut(.cancelAction)
            }
            .font(.system(size: 10, weight: .medium))
            .foregroundStyle(.white.opacity(0.7))
            .padding(.horizontal, 12)
            .padding(.vertical, 8)

            Group {
                if let svg = SVGPreview.image(for: item) {
                    HStack(spacing: 8) {
                        ZStack {
                            Checkerboard()
                            Image(nsImage: svg).resizable().aspectRatio(contentMode: .fit).padding(12)
                        }
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        .background(RoundedRectangle(cornerRadius: 8).fill(.white.opacity(0.04)))
                        .clipShape(RoundedRectangle(cornerRadius: 8))
                        codeView.frame(maxWidth: .infinity)
                    }
                } else {
                    codeView
                }
            }
            .padding(.horizontal, 12)
            .padding(.bottom, 12)
            .opacity(shown ? 1 : 0)
            .animation(Motion.contentIn, value: shown)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(RoundedRectangle(cornerRadius: 14, style: .continuous).fill(Color(white: 0.09)))
        .overlay { if copied { CopiedOverlay().transition(.opacity) } }
        .clipShape(RoundedRectangle(cornerRadius: 14, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(.white.opacity(0.2), lineWidth: 0.5))
        .shadow(color: .black.opacity(0.6), radius: 18, y: 8)
        .animation(Motion.hover, value: copied)
        .onAppear { shown = true }
    }

    private var codeView: some View {
        ScrollView(.vertical, showsIndicators: true) {
            Text(item.text ?? "")
                .font(SVGPreview.isSVG(item) ? .system(size: 10, design: .monospaced) : .system(size: 11.5))
                .foregroundStyle(.white.opacity(0.9))
                .textSelection(.enabled)
                .lineSpacing(2)
                .frame(maxWidth: .infinity, alignment: .topLeading)
        }
    }

    private func copy() {
        history.copy(item)
        copied = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.1) { copied = false }
    }
}

struct IslandButton<Glyph: View>: View {
    let symbol: String?
    let help: String
    let action: () -> Void
    let glyph: (Color) -> Glyph
    @State private var hovering = false
    @State private var pressed = false

    init(symbol: String?, help: String, action: @escaping () -> Void, @ViewBuilder glyph: @escaping (Color) -> Glyph) {
        self.symbol = symbol
        self.help = help
        self.action = action
        self.glyph = glyph
    }

    var body: some View {
        let color: Color = hovering ? .white : .white.opacity(0.6)
        Group {
            if let symbol {
                Image(systemName: symbol).font(.system(size: 12.5, weight: .medium)).foregroundStyle(color)
            } else {
                glyph(color)
            }
        }
        .frame(width: 28, height: 26)
        .background(Circle().fill(.white.opacity(hovering ? 0.12 : 0)).frame(width: 26, height: 26))
        .scaleEffect(pressed ? 0.86 : (hovering ? 1.06 : 1))
        .animation(Motion.hover, value: hovering)
        .animation(Motion.hover, value: pressed)
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .gesture(DragGesture(minimumDistance: 0)
            .onChanged { _ in pressed = true }
            .onEnded { _ in pressed = false; action() })
        .help(help)
    }
}

extension IslandButton where Glyph == EmptyView {
    init(symbol: String, help: String, action: @escaping () -> Void) {
        self.init(symbol: symbol, help: help, action: action) { _ in EmptyView() }
    }
}

/// Select-all (Material "select_all" style): a dashed frame around a check.
/// When everything is selected the frame turns solid and fills, and the
/// check flips to black.
struct SelectAllGlyph: View {
    let filled: Bool
    let color: Color
    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 3.5, style: .continuous)
                .stroke(color, style: StrokeStyle(lineWidth: 1.3, lineCap: .round, dash: filled ? [] : [2.4, 2.1]))
            RoundedRectangle(cornerRadius: 3.5, style: .continuous)
                .fill(color)
                .scaleEffect(filled ? 1 : 0.2)
                .opacity(filled ? 1 : 0)
            CheckShape()
                .trim(from: 0, to: filled ? 1 : 0.999)
                .stroke(filled ? Color.black : color, style: StrokeStyle(lineWidth: 1.5, lineCap: .round, lineJoin: .round))
                .frame(width: 7, height: 5.5)
        }
        .frame(width: 13, height: 13)
        .animation(Motion.hover, value: filled)
    }
}

struct CheckShape: Shape {
    func path(in r: CGRect) -> Path {
        var p = Path()
        p.move(to: CGPoint(x: r.minX, y: r.midY + r.height * 0.05))
        p.addLine(to: CGPoint(x: r.minX + r.width * 0.36, y: r.maxY))
        p.addLine(to: CGPoint(x: r.maxX, y: r.minY))
        return p
    }
}

struct BulkBar: View {
    let count: Int
    let actions: IslandActions

    var body: some View {
        HStack(spacing: 4) {
            Text("\(count) selected").font(.system(size: 12, weight: .medium)).padding(.horizontal, 10)
            BarPill(title: "Copy", symbol: "doc.on.doc", action: actions.copy)
            BarPill(title: "Group", symbol: "folder.badge.plus", action: actions.group)
            BarPill(title: "Reveal", symbol: "folder", action: actions.reveal)
            BarPill(title: "Delete", symbol: "trash", action: actions.delete)
            BarPill(title: nil, symbol: "xmark", action: actions.clear)
        }
        .foregroundStyle(.black)
        .padding(4)
        .background(Capsule().fill(.white))
        .shadow(color: .black.opacity(0.6), radius: 16, y: 8)
    }
}

struct BarPill: View {
    let title: String?
    let symbol: String
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 5) {
                Image(systemName: symbol).font(.system(size: 10.5, weight: .semibold))
                if let title { Text(title).font(.system(size: 12, weight: .semibold)) }
            }
            .padding(.horizontal, title == nil ? 8 : 11)
            .frame(height: 26)
            .foregroundStyle(hovering ? .white : .black)
            .background(Capsule().fill(hovering ? .black : .clear))
            .contentShape(Capsule())
        }
        .buttonStyle(PressScale())
        .onHover { hovering = $0 }
        .animation(Motion.hover, value: hovering)
    }
}

struct PressScale: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed ? 0.9 : 1)
            .animation(Motion.hover, value: configuration.isPressed)
    }
}

/// Monochrome now-playing strip at the bottom of the open tray. Works with
/// whatever macOS reports as Now Playing: Music, Spotify, browsers, VLC...
struct NowPlayingBar: View {
    @ObservedObject var service: NowPlaying

    var body: some View {
        if let track = service.track {
            // One compact row: artwork, title, progress (display only), transport.
            HStack(spacing: 9) {
                artwork(track)
                    .onTapGesture { PlayerIcon.open(track.player) }
                    .help(track.player.isEmpty
                          ? "Now playing"
                          : "Open \(PlayerIcon.name(for: track.player))")
                VStack(alignment: .leading, spacing: 0) {
                    Text(track.title)
                        .font(.system(size: 10.5, weight: .semibold))
                        .foregroundStyle(.white)
                        .lineLimit(1)
                    Text(track.subtitle.isEmpty
                         ? (PlayerIcon.name(for: track.player).isEmpty ? " " : PlayerIcon.name(for: track.player))
                         : track.subtitle)
                        .font(.system(size: 9))
                        .foregroundStyle(.white.opacity(0.52))
                        .lineLimit(1)
                }
                .frame(width: 150, alignment: .leading)
                if track.duration > 0 {
                    WaveformProgress(track: track)
                } else {
                    Spacer(minLength: 0)
                }
                HStack(spacing: 0) {
                    TransportButton(symbol: "backward.fill", help: "Previous", action: service.previous)
                    TransportButton(symbol: track.isPlaying ? "pause.fill" : "play.fill",
                                    help: track.isPlaying ? "Pause" : "Play", action: service.toggle)
                    TransportButton(symbol: "forward.fill", help: "Next", action: service.next)
                }
            }
            .padding(.leading, 5)
            .padding(.trailing, 4)
            .frame(height: 34)
            .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(.white.opacity(0.055)))
            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous)
                .strokeBorder(.white.opacity(0.10), lineWidth: 0.5))
        }
    }

    private func artwork(_ track: NowPlaying.Track) -> some View {
        Group {
            if let image = track.artwork {
                Image(nsImage: image).resizable().aspectRatio(contentMode: .fill)
            } else if let icon = PlayerIcon.icon(for: track.player) {
                Image(nsImage: icon).resizable().aspectRatio(contentMode: .fill)
            } else {
                Image(systemName: "music.note")
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(.white.opacity(0.7))
            }
        }
        .frame(width: 24, height: 24)
        .background(.white.opacity(0.06))
        .clipShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous).stroke(.white.opacity(0.18), lineWidth: 0.5))
    }
}

/// Compact transport control for the now-playing strip: hover halo, press snap.
struct TransportButton: View {
    let symbol: String
    let help: String
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.system(size: 10, weight: .semibold))
                .foregroundStyle(.white.opacity(hovering ? 1 : 0.72))
                .frame(width: 24, height: 24)
                .background(Circle().fill(.white.opacity(hovering ? 0.12 : 0)))
                .contentShape(Circle())
        }
        .buttonStyle(PressScale())
        .onHover { hovering = $0 }
        .animation(Motion.hover, value: hovering)
        .help(help)
    }
}

/// Waveform progress in the style of the reference: thin rounded bars of
/// varying height, centred on a line. Played part bright, the rest dim; bars
/// breathe gently while playing. Display only: it can't be clicked or dragged.
struct WaveformProgress: View {
    let track: NowPlaying.Track

    var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 30, paused: !track.isPlaying)) { context in
            let elapsed = track.elapsed(at: context.date)
            let progress = track.progress(at: context.date)
            let t = context.date.timeIntervalSinceReferenceDate
            HStack(spacing: 8) {
                Text(Self.time(elapsed))
                    .font(.system(size: 9, weight: .medium, design: .monospaced))
                    .foregroundStyle(.white.opacity(0.5))
                    .frame(width: 30, alignment: .leading)
                Canvas { ctx, size in
                    let bar: CGFloat = 2.4, gap: CGFloat = 2.2
                    let count = max(1, Int((size.width + gap) / (bar + gap)))
                    let span = CGFloat(count) * (bar + gap) - gap
                    let x0 = (size.width - span) / 2
                    for i in 0..<count {
                        let x = x0 + CGFloat(i) * (bar + gap)
                        let base = Self.level(i, seed: track.id)
                        let live = track.isPlaying ? 0.86 + 0.14 * sin(t * 4.2 + Double(i) * 0.83) : 1
                        let h = max(3, size.height * CGFloat(base * live))
                        let rect = CGRect(x: x, y: (size.height - h) / 2, width: bar, height: h)
                        let played = (x + bar / 2 - x0) / max(1, span) <= CGFloat(progress)
                        ctx.fill(Path(roundedRect: rect, cornerRadius: bar / 2),
                                 with: .color(.white.opacity(played ? 0.95 : 0.22)))
                    }
                }
                .frame(height: 16)
                Text("-" + Self.time(max(0, track.duration - elapsed)))
                    .font(.system(size: 9, weight: .medium, design: .monospaced))
                    .foregroundStyle(.white.opacity(0.5))
                    .frame(width: 34, alignment: .trailing)
            }
        }
        .allowsHitTesting(false)
    }

    /// Deterministic, smooth "audio-like" bar heights per track (0.28…1).
    static func level(_ i: Int, seed: String) -> Double {
        var hash: UInt64 = 1469598103934665603
        for byte in seed.utf8 { hash = (hash ^ UInt64(byte)) &* 1099511628211 }
        let s = Double(hash % 1000) / 1000 * 6.28
        let x = Double(i)
        let wave = 0.55 * abs(sin(x * 0.61 + s)) + 0.30 * abs(sin(x * 0.23 + s * 1.7)) + 0.15 * abs(sin(x * 1.37 + s * 0.4))
        return 0.28 + 0.72 * min(1, wave)
    }

    static func time(_ seconds: Double) -> String {
        let s = max(0, Int(seconds.rounded(.down)))
        return s >= 3600 ? String(format: "%d:%02d:%02d", s / 3600, s / 60 % 60, s % 60) : String(format: "%d:%02d", s / 60, s % 60)
    }
}

struct EmptyTray: View {
    let active: Bool
    @State private var phase: CGFloat = 0

    var body: some View {
        RoundedRectangle(cornerRadius: 16, style: .continuous)
            .strokeBorder(style: StrokeStyle(lineWidth: 1.2, dash: [6, 6], dashPhase: phase))
            .foregroundStyle(.white.opacity(0.35))
            .overlay(
                VStack(spacing: 8) {
                    Image(systemName: "tray.and.arrow.down.fill").font(.system(size: 22, weight: .light))
                    Text("DROP IMAGES HERE").font(.system(size: 10.5, weight: .bold)).kerning(2.2)
                    Text("Copied images & screenshots land here too")
                        .font(.system(size: 11)).foregroundStyle(.white.opacity(0.45))
                }
                .foregroundStyle(.white)
            )
            .padding(.horizontal, 16)
            .padding(.bottom, 16)
            .padding(.top, 4)
            .onAppear { if active { start() } }
            .onChange(of: active) { on in
                if on { start() } else { stop() }
            }
            .allowsHitTesting(false)
    }

    private func start() {
        phase = 0
        withAnimation(.linear(duration: 1).repeatForever(autoreverses: false)) { phase = -12 }
    }

    private func stop() {
        withAnimation(.linear(duration: 0.01)) { phase = 0 }
    }
}

struct DropTarget: View {
    let busy: Bool
    let active: Bool
    @State private var phase: CGFloat = 0
    @State private var breathe = false

    var body: some View {
        RoundedRectangle(cornerRadius: 18, style: .continuous)
            .fill(.white.opacity(breathe ? 0.10 : 0.05))
            .overlay(
                RoundedRectangle(cornerRadius: 18, style: .continuous)
                    .strokeBorder(style: StrokeStyle(lineWidth: 1.5, dash: busy ? [] : [7, 6], dashPhase: phase))
                    .foregroundStyle(.white.opacity(0.85))
            )
            .overlay(
                VStack(spacing: 10) {
                    Image(systemName: busy ? "arrow.triangle.2.circlepath" : "tray.and.arrow.down.fill")
                        .font(.system(size: 30, weight: .light))
                        .rotationEffect(.degrees(busy ? 360 : 0))
                        .animation(busy ? .linear(duration: 0.9).repeatForever(autoreverses: false) : .default, value: busy)
                        .offset(y: breathe ? 3 : -2)
                    Text(busy ? "SAVING" : "DROP TO VAULT").font(.system(size: 12, weight: .bold)).kerning(2.6)
                    Text(busy ? "Storing a local copy…" : "Release to save")
                        .font(.system(size: 11, design: .monospaced)).foregroundStyle(.white.opacity(0.55))
                }
                .foregroundStyle(.white)
            )
            .scaleEffect(breathe ? 1.0 : 0.985)
            .onAppear { if active { start() } }
            .onChange(of: active) { on in
                if on { start() } else { stop() }
            }
    }

    private func start() {
        phase = 0
        breathe = false
        withAnimation(.linear(duration: 0.9).repeatForever(autoreverses: false)) { phase = -13 }
        withAnimation(.easeInOut(duration: 0.9).repeatForever(autoreverses: true)) { breathe = true }
    }

    private func stop() {
        // Replace the repeating animations with a short one-shot assignment.
        withAnimation(.linear(duration: 0.01)) { phase = 0 }
        withAnimation(.linear(duration: 0.01)) { breathe = false }
    }
}

// MARK: - Host view (drop target)

final class IslandHostView: NSHostingView<IslandView> {
    var onDragEntered: ((NSDraggingInfo) -> Bool)?
    var onDragExited: (() -> Void)?
    var onDrop: ((NSPasteboard) -> Void)?
    var onRightClick: ((NSEvent) -> Void)?

    required init(rootView: IslandView) {
        super.init(rootView: rootView)
        registerForDraggedTypes(Ingest.acceptedTypes)
    }

    @MainActor required dynamic init?(coder: NSCoder) { fatalError() }

    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation {
        onDragEntered?(sender) == true ? .copy : []
    }
    override func draggingUpdated(_ sender: NSDraggingInfo) -> NSDragOperation {
        sender.draggingSource == nil ? .copy : []
    }
    override func draggingExited(_ sender: NSDraggingInfo?) { onDragExited?() }
    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        guard sender.draggingSource == nil else { return false }
        onDrop?(sender.draggingPasteboard)
        return true
    }
    override func rightMouseDown(with event: NSEvent) { onRightClick?(event) }
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
}

// MARK: - Controller

/// Owns the island window. The window is sized for the largest state and
/// stays put; the black shape animates inside it. Everything outside the
/// shape is click-through (ignoresMouseEvents follows the pointer).
@MainActor
final class IslandController {
    static let pad: CGFloat = 40
    static var shared: IslandController?

    let model = IslandModel()
    let tray = TrayController()
    let window: FloatingPanel
    private var host: IslandHostView!
    private var monitors: [Any] = []
    private let dragPasteboard = NSPasteboard(name: .drag)
    private var dragBaseline = 0
    private var mouseDownInside = false
    private var expandWork: DispatchWorkItem?
    private var activityWork: DispatchWorkItem?
    private var messageWork: DispatchWorkItem?
    private var expandGrace: Date = .distantPast
    var menuOpen = false
    var onMenu: ((NSEvent?) -> Void)?

    // MARK: Settings

    var position: IslandPosition {
        get { IslandPosition(rawValue: UserDefaults.standard.string(forKey: "islandPosition") ?? "") ?? .center }
        set { UserDefaults.standard.set(newValue.rawValue, forKey: "islandPosition"); layout() }
    }

    var displayID: CGDirectDisplayID? {
        get { (UserDefaults.standard.object(forKey: "islandDisplay") as? NSNumber)?.uint32Value }
        set {
            if let newValue { UserDefaults.standard.set(NSNumber(value: newValue), forKey: "islandDisplay") }
            else { UserDefaults.standard.removeObject(forKey: "islandDisplay") }
            layout()
        }
    }

    var showNotch: Bool {
        get { UserDefaults.standard.object(forKey: "showNotch") as? Bool ?? true }
        set {
            UserDefaults.standard.set(newValue, forKey: "showNotch")
            withAnimation(Motion.close) { model.showNotch = newValue }
        }
    }

    static func id(of screen: NSScreen) -> CGDirectDisplayID {
        (screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value ?? 0
    }

    var screen: NSScreen? {
        if let id = displayID, let match = NSScreen.screens.first(where: { Self.id(of: $0) == id }) { return match }
        // Default: the built-in display (it has the notch), else the main one.
        return NSScreen.screens.first(where: { $0.safeAreaInsets.top > 0 })
            ?? NSScreen.main
            ?? NSScreen.screens.first
    }

    // MARK: Setup

    init() {
        window = FloatingPanel(frame: NSRect(x: 0, y: 0, width: 800, height: 400))
        IslandController.shared = self
        window.level = NSWindow.Level(rawValue: NSWindow.Level.mainMenu.rawValue + 3)
        window.acceptsMouseMovedEvents = true
        window.ignoresMouseEvents = true

        let actions = IslandActions(
            copy: { [weak self] in self?.tray.copySelection() },
            copyText: { [weak self] in self?.tray.copyTextSelection() },
            group: { [weak self] in self?.tray.groupSelection() },
            reveal: { [weak self] in self?.revealAction() },
            delete: { [weak self] in self?.tray.deleteSelection() },
            clear: { [weak self] in self?.tray.clearSelection() },
            selectAll: { [weak self] in self?.tray.toggleSelectAll() },
            menu: { [weak self] in self?.onMenu?(nil) },
            flash: { [weak self] text in self?.flash(text) }
        )
        host = IslandHostView(rootView: IslandView(model: model, tray: tray, actions: actions, nowPlaying: .shared))
        host.onDragEntered = { [weak self] info in self?.dragEntered(info) ?? false }
        host.onDragExited = { [weak self] in self?.dragExited() }
        host.onDrop = { [weak self] pb in self?.drop(pb) }
        host.onRightClick = { [weak self] event in self?.onMenu?(event) }
        window.contentView = host

        tray.onSelection = { [weak self] n in self?.model.selection = n }
        tray.onVisibleCount = { [weak self] n in self?.model.visibleCount = n }
        tray.onSearch = { [weak self] in self?.model.searchRequested += 1 }
        tray.onMessage = { [weak self] text in self?.flash(text) }
        tray.onEscape = { [weak self] in self?.collapse() }
        tray.onEdit = { [weak self] item in
            self?.collapse()
            ImageEditing.open(item)
        }
        tray.onDragOut = { [weak self] active in if !active { self?.pointerMoved() } }

        NotificationCenter.default.addObserver(forName: .vaultChanged, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.model.count = VaultModel.shared.items.count }
        }
        NotificationCenter.default.addObserver(forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.layout() }
        }
        // If the system put the panel away (wake from sleep, display change,
        // another space, an app hiding accessory windows) bring it back.
        for name in [NSApplication.didBecomeActiveNotification, NSApplication.didUnhideNotification] {
            NotificationCenter.default.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.reassert() }
            }
        }
        NSWorkspace.shared.notificationCenter.addObserver(forName: NSWorkspace.didWakeNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.reassert() }
        }

        model.count = VaultModel.shared.items.count
        layout()
        window.orderFrontRegardless()
        installMonitors()
    }

    /// Re-place and re-front the panel without disturbing an open tray.
    func reassert() {
        layout()
        window.orderFrontRegardless()
    }

    private func revealAction() {
        if model.selection > 0 { tray.revealSelection() } else { NSWorkspace.shared.open(VaultStore.shared.imagesURL) }
    }

    // MARK: Geometry

    func layout() {
        guard let s = screen else { return }
        let f = s.frame
        var notchWidth: CGFloat = 0
        var notchCenter = f.midX
        if s.safeAreaInsets.top > 0, let l = s.auxiliaryTopLeftArea, let r = s.auxiliaryTopRightArea {
            notchWidth = f.width - l.width - r.width
            notchCenter = f.minX + l.width + notchWidth / 2
        }
        let menuBar = f.maxY - s.visibleFrame.maxY
        let bandHeight = s.safeAreaInsets.top > 0 ? s.safeAreaInsets.top : (menuBar > 10 ? menuBar : 32)

        model.position = position
        model.showNotch = showNotch
        model.hardwareNotch = notchWidth > 0 && position == .center
        model.notch = model.hardwareNotch ? CGSize(width: notchWidth, height: bandHeight)
                                          : CGSize(width: 190, height: max(26, bandHeight))
        model.expandedSize = CGSize(width: min(660, f.width - 60), height: model.band + 8 + 236)

        let outline = model.outline(for: .expanded)
        let w = outline.width + Self.pad * 2
        let h = outline.height + Self.pad
        let x: CGFloat
        switch position {
        case .center: x = (model.hardwareNotch ? notchCenter : f.midX) - w / 2
        case .left: x = f.minX + 14 - Self.pad
        case .right: x = f.maxX - 14 - w + Self.pad
        }
        window.setFrame(NSRect(x: x, y: f.maxY - h, width: w, height: h), display: true)
    }

    /// The shape's rectangle on screen for a state (target, not mid-animation).
    func rect(for state: IslandState) -> NSRect {
        let o = model.outline(for: state)
        let win = window.frame
        let x: CGFloat
        switch model.position {
        case .center: x = win.midX - o.width / 2
        case .left: x = win.minX + Self.pad
        case .right: x = win.maxX - Self.pad - o.width
        }
        return NSRect(x: x, y: win.maxY - o.height, width: o.width, height: o.height)
    }

    // MARK: Pointer

    private func installMonitors() {
        let events: NSEvent.EventTypeMask = [.mouseMoved, .leftMouseDragged, .leftMouseDown, .leftMouseUp]
        if let g = NSEvent.addGlobalMonitorForEvents(matching: events, handler: { [weak self] event in
            MainActor.assumeIsolated { self?.handle(event, local: false) }
        }) { monitors.append(g) }
        if let l = NSEvent.addLocalMonitorForEvents(matching: events, handler: { [weak self] event in
            MainActor.assumeIsolated { self?.handle(event, local: true) }
            return event
        }) { monitors.append(l) }
    }

    private func handle(_ event: NSEvent, local: Bool) {
        switch event.type {
        case .leftMouseDown:
            if local { mouseDownInside = true } else { dragBaseline = dragPasteboard.changeCount }
        case .leftMouseUp:
            mouseDownInside = false
            if !local, model.state == .dragHint || model.state == .dragOver, !model.dropBusy {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { [weak self] in
                    guard let self, !self.model.dropBusy, self.model.state == .dragHint || self.model.state == .dragOver else { return }
                    self.collapse()
                }
            }
        case .leftMouseDragged where !local:
            detectDrag()
        default:
            break
        }
        pointerMoved()
    }

    /// Click-through outside the shape + hover expand / collapse-on-leave.
    func pointerMoved() {
        // A drag-out session can swallow the mouse-up, which used to leave the
        // whole (mostly transparent) window capturing clicks forever.
        if NSEvent.pressedMouseButtons == 0 { mouseDownInside = false }
        let p = NSEvent.mouseLocation
        let current = rect(for: model.state)
        window.ignoresMouseEvents = !(current.insetBy(dx: -2, dy: -2).contains(p) || mouseDownInside || tray.isDraggingOut)

        switch model.state {
        case .collapsed:
            if triggerZone(width: hoverZone).contains(p) { beginHover() }
        case .activity:
            // The Live Activity is wider than the resting notch; touching the
            // top edge anywhere above it opens the tray.
            if triggerZone(width: current.insetBy(dx: -6, dy: 0)).contains(p) { beginHover() }
        case .peek:
            if !hoverZone.insetBy(dx: -4, dy: -4).contains(p) { collapse() }
        case .expanded:
            // Close the moment the pointer leaves; no delay.
            let inside = rect(for: .expanded).insetBy(dx: -2, dy: -2).contains(p)
            let grace = Date() < expandGrace
            if !(inside || grace || mouseDownInside || tray.isDraggingOut || menuOpen) { collapse() }
        case .dragHint, .dragOver:
            break
        }
    }

    /// Opening needs the pointer pushed against the very top edge of the
    /// screen (the top pixel row), within the notch's width. Passing near the
    /// notch on the way to a browser tab bar never opens it.
    private func triggerZone(width zone: NSRect) -> NSRect {
        let top = window.frame.maxY
        return NSRect(x: zone.minX, y: top - 2, width: zone.width, height: 3)
    }

    /// Where the open-in-progress island stays open: the notch's spot along
    /// the top edge, at least as tall as the menu bar, whether or not it's drawn.
    private var hoverZone: NSRect {
        let r = rect(for: .peek)
        let h = max(r.height, model.band)
        return NSRect(x: r.minX - 6, y: window.frame.maxY - h, width: r.width + 12, height: h + 2)
    }

    private func beginHover() {
        guard expandWork == nil else { return }
        activityWork?.cancel()
        withAnimation(Motion.hover) { model.state = .peek }
        // Warm the now-playing read during the 90ms breath so the bar is
        // usually ready by the time the tray opens.
        NowPlaying.shared.setActive(true)
        let work = DispatchWorkItem { [weak self] in
            guard let self else { return }
            self.expandWork = nil
            guard self.model.state == .peek else { return }
            self.expand(fromHover: true)
        }
        expandWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.09, execute: work)
    }

    // MARK: Transitions

    func expand(fromHover: Bool = false) {
        guard model.state != .expanded else { return }
        withAnimation(Motion.open) { model.state = .expanded }
        // Menubar/hotkey opening: give the pointer a moment to reach the island.
        // Hover opening needs none, so leaving closes it straight away.
        expandGrace = fromHover ? .distantPast : Date().addingTimeInterval(1.6)
        NowPlaying.shared.setActive(true)
        haptic()
        window.acceptsKey = true
        window.makeKey()
        DispatchQueue.main.async { [weak self] in self?.tray.focus() }
        pointerMoved()
    }

    func collapse() {
        expandWork?.cancel(); expandWork = nil
        NowPlaying.shared.setActive(false)
        guard model.state != .collapsed else { return }
        tray.closePreview()
        withAnimation(Motion.close) { model.state = .collapsed }
        if window.isKeyWindow { window.resignKey() }
        window.acceptsKey = false
        pointerMoved()
    }

    func toggle() { model.state == .expanded ? collapse() : expand() }

    /// Live Activity: the island widens with a thumbnail + status, then tucks back.
    func showActivity(image: NSImage?, title: String, detail: String) {
        guard model.state == .collapsed || model.state == .activity else { return }
        model.activityImage = image
        model.activityTitle = title
        model.activityDetail = detail
        withAnimation(Motion.open) { model.state = .activity }
        haptic()
        activityWork?.cancel()
        let work = DispatchWorkItem { [weak self] in
            guard let self, self.model.state == .activity else { return }
            withAnimation(Motion.close) { self.model.state = .collapsed }
        }
        activityWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 2.4, execute: work)
    }

    func flash(_ text: String) {
        withAnimation(Motion.hover) { model.message = text }
        messageWork?.cancel()
        let work = DispatchWorkItem { [weak self] in withAnimation(Motion.hover) { self?.model.message = nil } }
        messageWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 2.2, execute: work)
    }

    private func haptic() {
        NSHapticFeedbackManager.defaultPerformer.perform(.levelChange, performanceTime: .now)
    }

    // MARK: Drag & drop

    private func detectDrag() {
        guard dragPasteboard.changeCount != dragBaseline else { return }
        dragBaseline = dragPasteboard.changeCount
        guard Ingest.looksLikeImage(dragPasteboard) else { return }
        guard [.collapsed, .peek, .activity].contains(model.state) else { return }
        activityWork?.cancel()
        withAnimation(Motion.open) { model.state = .dragHint }
    }

    private func dragEntered(_ info: NSDraggingInfo) -> Bool {
        guard info.draggingSource == nil else { return false } // our own tiles
        guard Ingest.looksLikeImage(info.draggingPasteboard) else { return false } // text, links to pages...
        if model.state != .dragOver {
            withAnimation(Motion.open) { model.state = .dragOver }
            NowPlaying.shared.setActive(false)
            haptic()
        }
        return true
    }

    private func dragExited() {
        guard !model.dropBusy, model.state == .dragOver else { return }
        withAnimation(Motion.close) { model.state = .dragHint }
    }

    private func drop(_ pb: NSPasteboard) {
        // Read the payload and start any promised-file copies *now*, before
        // performDragOperation returns. AppKit raises an ObjC exception if
        // promises are requested after the drag has ended.
        let payload = Ingest.readDrop(pb)
        withAnimation(Motion.hover) { model.dropBusy = true }

        // Never leave the island stuck on "Saving" if a promise source or a
        // download never answers.
        DispatchQueue.main.asyncAfter(deadline: .now() + 45) { [weak self] in
            guard let self, self.model.dropBusy else { return }
            self.model.dropBusy = false
            if self.model.state == .dragOver { self.collapse() }
        }

        Task { @MainActor in
            let summary = await Ingest.importDrop(payload, source: "drop")
            model.dropBusy = false
            if summary.isEmpty {
                withAnimation(Motion.close) { model.state = .collapsed }
                NowPlaying.shared.setActive(false)
                showActivity(image: nil, title: "No image found", detail: "")
                model.activityTitle = "No image found"
                return
            }
            // The big drop target shrinks straight into a live activity.
            withAnimation(Motion.close) { model.state = .collapsed }
            NowPlaying.shared.setActive(false)
            let n = summary.added.count
            showActivity(image: summary.firstImage,
                         title: n == 0 ? "Already saved" : (n > 1 ? "\(n) saved" : "Saved"),
                         detail: "\(VaultModel.shared.items.count) in vault")
        }
    }
}
