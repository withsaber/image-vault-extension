import Foundation
import CryptoKit

/// One saved image. Field names match the Chrome extension's items so the two
/// vaults can exchange metadata as-is (timestamps are epoch milliseconds).
struct VaultItem: Codable, Hashable, Identifiable {
    var id: String
    var name: String
    var mime: String?
    var bytes: Int?
    var width: Int?
    var height: Int?
    var hash: String?
    var url: String?
    var pageTitle: String?
    var pageUrl: String?
    var domain: String?
    var timestamp: Double
    var source: String?
    /// OCR-recognized text from the image (populated automatically in the background).
    var ocrText: String?
    /// Which OCR pipeline / language set produced ocrText (nil = not read yet).
    var ocrVersion: Int?
    var ocrLanguages: String?
}

/// A user-made folder of vault images ("projects"). Membership only: the
/// files themselves stay in the one images folder.
struct VaultGroup: Codable, Hashable, Identifiable {
    var id: String
    var name: String
    var itemIDs: [String]
    var created: Double
}

struct VaultIndex: Codable {
    var items: [VaultItem] = []
    var tombstones: [String] = []
    /// Optional so older vault.json files (and other readers) stay compatible.
    var groups: [VaultGroup]?
}

/// On-disk vault shared by the app and the Chrome bridge process:
///   ~/Library/Application Support/ImageVault/vault.json
///   ~/Library/Application Support/ImageVault/images/<name>   (one folder, unique names)
/// Older builds kept one sub-folder per image (images/<id>/<name>);
/// `migrateToFlatLayout` moves those into the shared folder.
/// Both processes mutate the index, so every read-modify-write holds a flock.
final class VaultStore {
    static let shared = VaultStore()

    let root: URL
    let indexURL: URL
    let imagesURL: URL
    private let lockURL: URL

    init() {
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        // IMAGEVAULT_ROOT points the app at a throwaway vault (used by --snapshot).
        if let override = ProcessInfo.processInfo.environment["IMAGEVAULT_ROOT"] {
            root = URL(fileURLWithPath: override, isDirectory: true)
        } else {
            root = support.appendingPathComponent("ImageVault", isDirectory: true)
        }
        indexURL = root.appendingPathComponent("vault.json")
        imagesURL = root.appendingPathComponent("images", isDirectory: true)
        lockURL = root.appendingPathComponent(".lock")
        try? FileManager.default.createDirectory(at: imagesURL, withIntermediateDirectories: true)
    }

    // MARK: Index

    private func withLock<T>(_ body: () throws -> T) rethrows -> T {
        let fd = open(lockURL.path, O_CREAT | O_RDWR, 0o644)
        if fd >= 0 { flock(fd, LOCK_EX) }
        defer { if fd >= 0 { flock(fd, LOCK_UN); close(fd) } }
        return try body()
    }

    func load() -> VaultIndex {
        guard let data = try? Data(contentsOf: indexURL) else { return VaultIndex() }
        guard var index = try? JSONDecoder().decode(VaultIndex.self, from: data) else {
            // Keep a copy instead of silently replacing the user's index with
            // an empty one on the next write.
            backupCorruptIndex()
            return VaultIndex()
        }
        // A duplicate id would crash `Dictionary(uniqueKeysWithValues:)` in the
        // model, so drop repeats here (newest-first order keeps the best one).
        var seen = Set<String>()
        index.items = index.items.filter { seen.insert($0.id).inserted }
        return index
    }

    private var corruptBackedUp = false
    private let backupLock = NSLock()

    private func backupCorruptIndex() {
        backupLock.lock()
        defer { backupLock.unlock() }
        guard !corruptBackedUp else { return }
        corruptBackedUp = true
        let stamp = Int(Date().timeIntervalSince1970)
        let backup = root.appendingPathComponent("vault.corrupt-\(stamp).json")
        try? FileManager.default.copyItem(at: indexURL, to: backup)
    }

    @discardableResult
    func mutate<T>(_ body: (inout VaultIndex) throws -> T) rethrows -> T {
        try withLock {
            var index = load()
            let result = try body(&index)
            index.items.sort { $0.timestamp > $1.timestamp }
            if let data = try? JSONEncoder().encode(index) { try? data.write(to: indexURL, options: .atomic) }
            return result
        }
    }

    var indexModified: Date? {
        (try? FileManager.default.attributesOfItem(atPath: indexURL.path)[.modificationDate]) as? Date
    }

    func fileURL(for item: VaultItem) -> URL {
        let flat = imagesURL.appendingPathComponent(item.name)
        let legacy = legacyURL(for: item)
        if !FileManager.default.fileExists(atPath: flat.path), FileManager.default.fileExists(atPath: legacy.path) { return legacy }
        return flat
    }

    private func legacyURL(for item: VaultItem) -> URL {
        imagesURL.appendingPathComponent(item.id, isDirectory: true).appendingPathComponent(item.name)
    }

    /// A name no other item (or leftover file) uses: "Shot.png", "Shot 2.png"...
    private func uniqueName(_ name: String, in index: VaultIndex, excluding id: String? = nil) -> String {
        let taken = Set(index.items.filter { $0.id != id }.map { $0.name.lowercased() })
        let ext = (name as NSString).pathExtension
        let base = (name as NSString).deletingPathExtension
        var candidate = name
        var n = 2
        while taken.contains(candidate.lowercased())
                || FileManager.default.fileExists(atPath: imagesURL.appendingPathComponent(candidate).path) {
            candidate = ext.isEmpty ? "\(base) \(n)" : "\(base) \(n).\(ext)"
            n += 1
        }
        return candidate
    }

    /// One-time move from images/<id>/<name> to images/<name>.
    func migrateToFlatLayout() {
        let fm = FileManager.default
        guard let entries = try? fm.contentsOfDirectory(at: imagesURL, includingPropertiesForKeys: [.isDirectoryKey]),
              entries.contains(where: { (try? $0.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true }) else { return }
        mutate { index in
            for i in index.items.indices {
                let item = index.items[i]
                let legacy = legacyURL(for: item)
                guard fm.fileExists(atPath: legacy.path) else { continue }
                let name = uniqueName(item.name, in: index, excluding: item.id)
                do {
                    try fm.moveItem(at: legacy, to: imagesURL.appendingPathComponent(name))
                    index.items[i].name = name
                    try? fm.removeItem(at: legacy.deletingLastPathComponent())
                } catch { continue }
            }
        }
        // Empty per-image folders left by deleted items.
        for dir in (try? fm.contentsOfDirectory(at: imagesURL, includingPropertiesForKeys: nil)) ?? []
        where (try? dir.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true {
            if (try? fm.contentsOfDirectory(atPath: dir.path))?.filter({ $0 != ".DS_Store" }).isEmpty == true {
                try? fm.removeItem(at: dir)
            }
        }
    }

    // MARK: Adding

    struct Meta {
        var name: String?
        var url: String?
        var pageTitle: String?
        var pageUrl: String?
        var domain: String?
        var source: String
    }

    enum AddResult {
        case added(VaultItem)
        case duplicate(VaultItem)
    }

    /// Normalises the image (AVIF/WebP/HEIC/TIFF → PNG or JPEG), de-duplicates by
    /// content hash or source URL, and stores it as a real file.
    func add(data raw: Data, meta: Meta) throws -> AddResult {
        let image = try Imaging.portable(raw)
        let hash = Self.hash(image.data)
        return try mutate { index -> AddResult in
            if let i = index.items.firstIndex(where: { $0.hash == hash || (meta.url != nil && $0.url == meta.url) }) {
                var dup = index.items.remove(at: i)
                dup.timestamp = Self.now
                index.items.insert(dup, at: 0)
                return .duplicate(dup)
            }
            let id = "mac_" + String(Int(Self.now), radix: 36) + "_" + Self.randomSuffix()
            let item = VaultItem(
                id: id,
                name: uniqueName(Self.fileName(meta.name ?? meta.url.flatMap { URL(string: $0)?.lastPathComponent }, ext: image.ext),
                                 in: index),
                mime: image.mime, bytes: image.data.count, width: image.width, height: image.height, hash: hash,
                url: meta.url.flatMap { $0.hasPrefix("http") ? $0 : nil },
                pageTitle: meta.pageTitle, pageUrl: meta.pageUrl, domain: meta.domain,
                timestamp: Self.now, source: meta.source
            )
            try write(image.data, for: item)
            index.items.insert(item, at: 0)
            return .added(item)
        }
    }

    /// Stores an item that came from the Chrome extension, keeping its id.
    func importExternal(_ incoming: VaultItem, data: Data) throws {
        try mutate { index in
            if index.tombstones.contains(incoming.id) || index.items.contains(where: { $0.id == incoming.id }) { return }
            if let hash = incoming.hash, index.items.contains(where: { $0.hash == hash }) { return }
            var item = incoming
            let image = (try? Imaging.portable(data)) ?? Imaging.Portable(data: data, mime: incoming.mime ?? "image/png",
                                                                          ext: "png", width: nil, height: nil)
            item.name = uniqueName(Self.fileName(incoming.name, ext: image.ext), in: index)
            item.mime = image.mime
            item.bytes = image.data.count
            item.width = item.width ?? image.width
            item.height = item.height ?? image.height
            item.source = item.source ?? "chrome"
            try write(image.data, for: item)
            index.items.append(item)
        }
    }

    private func write(_ data: Data, for item: VaultItem) throws {
        let url = fileURL(for: item)
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try data.write(to: url, options: .atomic)
    }

    // MARK: Removing

    /// Removes items from the index and records tombstones so the deletion
    /// syncs to Chrome. Files are kept until `purgeFiles` (undo window).
    @discardableResult
    func delete(ids: [String]) -> [VaultItem] {
        let set = Set(ids)
        return mutate { index in
            let removed = index.items.filter { set.contains($0.id) }
            index.items.removeAll { set.contains($0.id) }
            index.tombstones.append(contentsOf: ids.filter { !index.tombstones.contains($0) })
            return removed
        }
    }

    func restore(_ items: [VaultItem]) {
        let ids = Set(items.map(\.id))
        mutate { index in
            index.tombstones.removeAll { ids.contains($0) }
            for item in items where !index.items.contains(where: { $0.id == item.id }) { index.items.append(item) }
        }
    }

    func purgeFiles(_ items: [VaultItem]) {
        let index = load()
        let live = Set(index.items.map(\.id))
        let liveNames = Set(index.items.map { $0.name.lowercased() })
        for item in items where !live.contains(item.id) {
            if !liveNames.contains(item.name.lowercased()) {
                try? FileManager.default.removeItem(at: imagesURL.appendingPathComponent(item.name))
            }
            try? FileManager.default.removeItem(at: imagesURL.appendingPathComponent(item.id, isDirectory: true))
        }
        // Forget group members that are gone for good.
        mutate { index in
            let ids = Set(index.items.map(\.id))
            for g in (index.groups ?? []).indices { index.groups![g].itemIDs.removeAll { !ids.contains($0) } }
        }
    }

    /// Replaces an item's pixels in place (used by Edit → Save to Vault).
    func replaceData(of id: String, with raw: Data) throws -> VaultItem? {
        let image = try Imaging.portable(raw)
        return try mutate { index -> VaultItem? in
            guard let i = index.items.firstIndex(where: { $0.id == id }) else { return nil }
            var item = index.items[i]
            // Same pixels (e.g. Copy on an unedited screenshot): nothing to write.
            if item.hash == Self.hash(image.data) { return item }
            let old = fileURL(for: item)
            let base = (item.name as NSString).deletingPathExtension
            // Same extension: overwrite the file in place; otherwise swap it.
            let sameName = "\(base).\(image.ext)".lowercased() == item.name.lowercased()
                && old.deletingLastPathComponent() == imagesURL
            if !sameName {
                try? FileManager.default.removeItem(at: old)
                item.name = uniqueName("\(base).\(image.ext)", in: index, excluding: id)
            }
            item.mime = image.mime
            item.bytes = image.data.count
            item.width = image.width
            item.height = image.height
            item.hash = Self.hash(image.data)
            item.ocrText = nil
            item.ocrVersion = nil
            item.timestamp = Self.now
            try write(image.data, for: item)
            index.items[i] = item
            return item
        }
    }

    // MARK: Groups

    @discardableResult
    func createGroup(name: String, ids: [String]) -> VaultGroup {
        mutate { index in
            var groups = index.groups ?? []
            let set = Set(ids)
            for g in groups.indices { groups[g].itemIDs.removeAll { set.contains($0) } }
            let group = VaultGroup(id: "grp_" + String(Int(Self.now), radix: 36) + "_" + Self.randomSuffix(),
                                   name: name, itemIDs: ids, created: Self.now)
            groups.insert(group, at: 0)
            index.groups = groups
            return group
        }
    }

    func addToGroup(_ groupID: String, ids: [String]) {
        mutate { index in
            var groups = index.groups ?? []
            let set = Set(ids)
            for g in groups.indices { groups[g].itemIDs.removeAll { set.contains($0) } }
            if let g = groups.firstIndex(where: { $0.id == groupID }) { groups[g].itemIDs.append(contentsOf: ids) }
            index.groups = groups
        }
    }

    func removeFromGroups(ids: [String]) {
        let set = Set(ids)
        mutate { index in
            for g in (index.groups ?? []).indices { index.groups![g].itemIDs.removeAll { set.contains($0) } }
        }
    }

    func renameGroup(_ groupID: String, to name: String) {
        mutate { index in
            if let g = index.groups?.firstIndex(where: { $0.id == groupID }) { index.groups![g].name = name }
        }
    }

    /// Removes the folder; its images go back to the main grid.
    func deleteGroup(_ groupID: String) {
        mutate { index in index.groups?.removeAll { $0.id == groupID } }
    }

    // MARK: Helpers

    static var now: Double { Date().timeIntervalSince1970 * 1000 }

    static func hash(_ data: Data) -> String {
        SHA256.hash(data: data).prefix(16).map { String(format: "%02x", $0) }.joined()
    }

    static func randomSuffix() -> String {
        String((0..<6).map { _ in "abcdefghijklmnopqrstuvwxyz0123456789".randomElement()! })
    }

    static func fileName(_ given: String?, ext: String) -> String {
        var base = (given ?? "").removingPercentEncoding ?? (given ?? "")
        if let dot = base.lastIndex(of: "."), base.distance(from: dot, to: base.endIndex) <= 6 { base = String(base[..<dot]) }
        base = base.replacingOccurrences(of: "[^A-Za-z0-9 _\\-]+", with: "_", options: .regularExpression)
            .trimmingCharacters(in: CharacterSet(charactersIn: " _"))
        if base.count > 60 { base = String(base.prefix(60)) }
        if base.isEmpty {
            let f = DateFormatter()
            f.dateFormat = "yyyy-MM-dd 'at' HH.mm.ss"
            base = "Image \(f.string(from: Date()))"
        }
        return "\(base).\(ext)"
    }
}
