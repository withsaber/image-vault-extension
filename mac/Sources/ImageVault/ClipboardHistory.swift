import AppKit
import UniformTypeIdentifiers

/// One thing that was copied anywhere on the Mac.
struct ClipItem: Codable, Identifiable, Hashable {
    enum Kind: String, Codable { case text, image, file }
    var id: String
    var kind: Kind
    var text: String?          // text, or the file paths (one per line)
    var imageFile: String?     // thumbnail PNG name inside clipboard/
    var width: Int?
    var height: Int?
    var hash: String
    var date: Date
    var app: String?

    var preview: String {
        switch kind {
        case .text: return text ?? ""
        case .file: return (text ?? "").split(separator: "\n").map { URL(fileURLWithPath: String($0)).lastPathComponent }
            .joined(separator: "\n")
        case .image: return width.flatMap { w in height.map { "\(w)×\($0)" } } ?? "Image"
        }
    }
}

/// Clipboard history shown in the island's Clipboard tab: copied text and
/// files. Copied images are not kept here; they go to the vault instead.
/// Kept in `clipboard.json` next to the vault.
@MainActor
final class ClipboardHistory: ObservableObject {
    static let shared = ClipboardHistory()
    static let limit = 300

    @Published private(set) var items: [ClipItem] = []

    private let indexURL: URL
    private let dataURL: URL

    var enabled: Bool {
        get { UserDefaults.standard.object(forKey: "clipboardHistory") as? Bool ?? true }
        set { UserDefaults.standard.set(newValue, forKey: "clipboardHistory"); objectWillChange.send() }
    }

    private init() {
        let root = VaultStore.shared.root
        indexURL = root.appendingPathComponent("clipboard.json")
        dataURL = root.appendingPathComponent("clipboard", isDirectory: true)
        try? FileManager.default.createDirectory(at: dataURL, withIntermediateDirectories: true)
        if let data = try? Data(contentsOf: indexURL) {
            let decoder = JSONDecoder()
            decoder.dateDecodingStrategy = .millisecondsSince1970
            items = (try? decoder.decode([ClipItem].self, from: data)) ?? []
        }
        // Earlier builds kept copied images here too; those belong in the vault.
        if items.contains(where: { $0.kind == .image }) {
            items.removeAll { $0.kind == .image }
            try? FileManager.default.removeItem(at: dataURL)
            try? FileManager.default.createDirectory(at: dataURL, withIntermediateDirectories: true)
            save()
        }
    }

    // MARK: Recording

    /// Called by the clipboard watcher on every change (ours excluded).
    func record(_ pb: NSPasteboard) {
        guard enabled else { return }
        let types = pb.types ?? []
        let app = NSWorkspace.shared.frontmostApplication?.localizedName

        if types.contains(.fileURL),
           let urls = pb.readObjects(forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true]) as? [URL], !urls.isEmpty {
            let paths = urls.map(\.path).joined(separator: "\n")
            add(ClipItem(id: Self.newID(), kind: .file, text: paths, hash: VaultStore.hash(Data(paths.utf8)), date: Date(), app: app))
            return
        }
        // Images go to the vault (ClipboardWatcher), never into the history.
        // Rich text that also carries a picture still counts as text.
        if let text = pb.string(forType: .string), !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            let clipped = text.count > 20_000 ? String(text.prefix(20_000)) : text
            add(ClipItem(id: Self.newID(), kind: .text, text: clipped, hash: VaultStore.hash(Data(clipped.utf8)), date: Date(), app: app))
        }
    }

    private func add(_ item: ClipItem) {
        if bump(hash: item.hash) { return }
        items.insert(item, at: 0)
        if items.count > Self.limit {
            for old in items[Self.limit...] { removeFiles(old) }
            items.removeLast(items.count - Self.limit)
        }
        save()
    }

    /// Copying something already in the history moves it to the top.
    private func bump(hash: String) -> Bool {
        guard let i = items.firstIndex(where: { $0.hash == hash }) else { return false }
        var item = items.remove(at: i)
        item.date = Date()
        items.insert(item, at: 0)
        save()
        return true
    }

    // MARK: Actions

    func copy(_ item: ClipItem) {
        let pb = NSPasteboard.general
        pb.clearContents()
        switch item.kind {
        case .text:
            pb.setString(item.text ?? "", forType: .string)
            pb.setData(Data(), forType: Clipboard.marker)
        case .file:
            let urls = (item.text ?? "").split(separator: "\n").map { URL(fileURLWithPath: String($0)) as NSURL }
            pb.writeObjects(urls)
            pb.setData(Data(), forType: Clipboard.marker)
        case .image:
            let entry = NSPasteboardItem()
            if let data = try? Data(contentsOf: dataURL.appendingPathComponent("\(item.id).orig")),
               let image = NSImage(data: data) {
                if let png = NSBitmapImageRep(data: image.tiffRepresentation ?? Data())?.representation(using: .png, properties: [:]) {
                    entry.setData(png, forType: .png)
                }
                if let tiff = image.tiffRepresentation { entry.setData(tiff, forType: .tiff) }
            }
            entry.setData(Data(), forType: Clipboard.marker)
            pb.writeObjects([entry])
        }
        // Copying from the history leaves the card where it is.
    }

    func delete(_ item: ClipItem) {
        items.removeAll { $0.id == item.id }
        removeFiles(item)
        save()
    }

    func clear() {
        items.removeAll()
        try? FileManager.default.removeItem(at: dataURL)
        try? FileManager.default.createDirectory(at: dataURL, withIntermediateDirectories: true)
        save()
    }

    func thumbnail(_ item: ClipItem) -> NSImage? {
        guard let name = item.imageFile else { return nil }
        return NSImage(contentsOf: dataURL.appendingPathComponent(name))
    }

    func matches(_ item: ClipItem, query: String) -> Bool {
        let q = query.trimmingCharacters(in: .whitespaces)
        guard !q.isEmpty else { return true }
        return item.preview.localizedCaseInsensitiveContains(q) || (item.app?.localizedCaseInsensitiveContains(q) ?? false)
    }

    // MARK: Storage

    private func removeFiles(_ item: ClipItem) {
        guard item.kind == .image else { return }
        try? FileManager.default.removeItem(at: dataURL.appendingPathComponent("\(item.id).png"))
        try? FileManager.default.removeItem(at: dataURL.appendingPathComponent("\(item.id).orig"))
    }

    private func save() {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .millisecondsSince1970
        if let data = try? encoder.encode(items) { try? data.write(to: indexURL, options: .atomic) }
    }

    private static func newID() -> String { "clip_" + UUID().uuidString.prefix(12).lowercased() }

    /// Developer aid for snapshots: fill the history with sample clips.
    func loadSamples() {
        items = [
            "Meeting moved to Thursday 3pm — same room, bring the Q4 numbers.",
            "https://www.figma.com/file/AbC123/Image-Vault-Island",
            "brew install tesseract dylibbundler",
            "আমার সোনার বাংলা, আমি তোমায় ভালোবাসি",
            "shadat@example.com",
            "let side = max(2, floor((width - 32 - (cols - 1) * gap) / cols))",
            "#15191D",
            #"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M12 2l3 7h7l-5.5 4.5 2 7.5-6.5-4.5-6.5 4.5 2-7.5L2 9h7z" fill="white"/></svg>"#,
        ].enumerated().map { i, text in
            ClipItem(id: "sample\(i)", kind: .text, text: text, hash: "s\(i)", date: Date().addingTimeInterval(Double(-i * 340)),
                     app: ["Slack", "Comet", "Terminal", "Notes", "Mail", "Xcode", "Image Vault", "Figma"][i])
        }
    }
}
