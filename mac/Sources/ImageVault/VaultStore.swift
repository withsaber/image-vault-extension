import Foundation
import CryptoKit
import SQLite3

/// One saved image. Field names match the Chrome extension's items so older
/// vault.json files import as-is (timestamps are epoch milliseconds).
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
    /// Not loaded into the UI model: search runs in SQLite, so it stays nil there.
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

struct VaultIndex {
    var items: [VaultItem] = []
    var groups: [VaultGroup]?
}

extension Notification.Name {
    /// Posted on the main thread after items or groups change in the store.
    static let vaultStoreChanged = Notification.Name("ImageVault.vaultStoreChanged")
    /// Posted on the main thread after OCR text changes (the grid itself is unchanged).
    static let vaultOCRChanged = Notification.Name("ImageVault.vaultOCRChanged")
}

/// On-disk vault:
///   ~/Library/Application Support/ImageVault/vault.sqlite   (index, WAL mode)
///   ~/Library/Application Support/ImageVault/images/<name>  (one folder, unique names)
///   ~/Library/Application Support/ImageVault/thumbs/        (grid thumbnail cache)
/// Builds before 2026-10-30 kept the index in vault.json, rewritten whole on
/// every change; it's imported once and kept as vault.json.pre-sqlite.
/// Older builds also kept one sub-folder per image (images/<id>/<name>);
/// `migrateToFlatLayout` moves those into the shared folder.
/// Every write is one small transaction, so saves, deletes and OCR results
/// cost the same at 10 images or 10,000.
final class VaultStore: @unchecked Sendable {
    static let shared = VaultStore()

    let root: URL
    let imagesURL: URL
    let thumbsURL: URL
    private let dbURL: URL
    private let legacyIndexURL: URL

    private var db: OpaquePointer?
    /// SQLite is built multi-thread (one connection, one thread at a time), so
    /// every statement runs under this lock. Recursive: helpers nest.
    private let lock = NSRecursiveLock()
    private var statements: [String: OpaquePointer] = [:]
    private var hasFTS = false

    /// Bumped on every change to items or groups; the UI model compares it to
    /// skip reloads it has already done.
    private(set) var generation = 0
    private var changePosted = false
    private var ocrChangePosted = false

    init() {
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        // IMAGEVAULT_ROOT points the app at a throwaway vault (used by --snapshot).
        if let override = ProcessInfo.processInfo.environment["IMAGEVAULT_ROOT"] {
            root = URL(fileURLWithPath: override, isDirectory: true)
        } else {
            root = support.appendingPathComponent("ImageVault", isDirectory: true)
        }
        imagesURL = root.appendingPathComponent("images", isDirectory: true)
        thumbsURL = root.appendingPathComponent("thumbs", isDirectory: true)
        dbURL = root.appendingPathComponent("vault.sqlite")
        legacyIndexURL = root.appendingPathComponent("vault.json")
        try? FileManager.default.createDirectory(at: imagesURL, withIntermediateDirectories: true)
        try? FileManager.default.createDirectory(at: thumbsURL, withIntermediateDirectories: true)
        open()
        importLegacyIndex()
    }

    // MARK: SQLite

    private func open() {
        guard sqlite3_open_v2(dbURL.path, &db, SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE, nil) == SQLITE_OK else {
            NSLog("ImageVault: can't open \(dbURL.path): \(errorMessage)")
            return
        }
        sqlite3_busy_timeout(db, 3000)
        exec("PRAGMA journal_mode = WAL")
        exec("PRAGMA synchronous = NORMAL")
        exec("PRAGMA foreign_keys = ON")
        exec("""
            CREATE TABLE IF NOT EXISTS items (
                rid INTEGER PRIMARY KEY,
                id TEXT NOT NULL UNIQUE,
                name TEXT NOT NULL,
                mime TEXT, bytes INTEGER, width INTEGER, height INTEGER,
                hash TEXT, url TEXT, pageTitle TEXT, pageUrl TEXT, domain TEXT,
                timestamp REAL NOT NULL,
                source TEXT,
                ocrText TEXT, ocrVersion INTEGER, ocrLanguages TEXT
            )
            """)
        exec("CREATE INDEX IF NOT EXISTS items_timestamp ON items(timestamp DESC)")
        exec("CREATE INDEX IF NOT EXISTS items_hash ON items(hash)")
        exec("CREATE INDEX IF NOT EXISTS items_url ON items(url)")
        exec("CREATE INDEX IF NOT EXISTS items_name ON items(name COLLATE NOCASE)")
        exec("""
            CREATE TABLE IF NOT EXISTS groups (
                id TEXT PRIMARY KEY, name TEXT NOT NULL, created REAL NOT NULL
            )
            """)
        // An image belongs to at most one folder. No foreign key to items: a
        // deleted image keeps its membership through the undo window.
        exec("""
            CREATE TABLE IF NOT EXISTS group_items (
                item_id TEXT PRIMARY KEY,
                group_id TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
                position INTEGER NOT NULL
            )
            """)
        exec("CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT)")

        // Substring search over names, sites and OCR text. Trigram matches
        // "siam" inside "Siamese" case-insensitively, in any script.
        hasFTS = exec("""
            CREATE VIRTUAL TABLE IF NOT EXISTS items_fts USING fts5(
                name, domain, source, pageTitle, url, pageUrl, ocrText,
                content='items', content_rowid='rid', tokenize='trigram'
            )
            """)
        if hasFTS {
            let cols = "name, domain, source, pageTitle, url, pageUrl, ocrText"
            let new = "new.name, new.domain, new.source, new.pageTitle, new.url, new.pageUrl, new.ocrText"
            let old = "old.name, old.domain, old.source, old.pageTitle, old.url, old.pageUrl, old.ocrText"
            exec("""
                CREATE TRIGGER IF NOT EXISTS items_ai AFTER INSERT ON items BEGIN
                    INSERT INTO items_fts(rowid, \(cols)) VALUES (new.rid, \(new));
                END
                """)
            exec("""
                CREATE TRIGGER IF NOT EXISTS items_ad AFTER DELETE ON items BEGIN
                    INSERT INTO items_fts(items_fts, rowid, \(cols)) VALUES ('delete', old.rid, \(old));
                END
                """)
            exec("""
                CREATE TRIGGER IF NOT EXISTS items_au AFTER UPDATE ON items BEGIN
                    INSERT INTO items_fts(items_fts, rowid, \(cols)) VALUES ('delete', old.rid, \(old));
                    INSERT INTO items_fts(rowid, \(cols)) VALUES (new.rid, \(new));
                END
                """)
        }
    }

    private var errorMessage: String { db.map { String(cString: sqlite3_errmsg($0)) } ?? "no database" }

    @discardableResult
    private func exec(_ sql: String) -> Bool {
        lock.lock(); defer { lock.unlock() }
        guard sqlite3_exec(db, sql, nil, nil, nil) == SQLITE_OK else {
            NSLog("ImageVault SQL error: \(errorMessage) in \(sql.prefix(80))")
            return false
        }
        return true
    }

    /// A cached prepared statement, reset and bound to `args`. Use under `lock`.
    private func prepare(_ sql: String, _ args: [Any?]) -> OpaquePointer? {
        var stmt = statements[sql]
        if stmt == nil {
            guard sqlite3_prepare_v3(db, sql, -1, UInt32(SQLITE_PREPARE_PERSISTENT), &stmt, nil) == SQLITE_OK else {
                NSLog("ImageVault SQL error: \(errorMessage) in \(sql.prefix(80))")
                return nil
            }
            statements[sql] = stmt
        }
        sqlite3_reset(stmt)
        sqlite3_clear_bindings(stmt)
        let transient = unsafeBitCast(-1, to: sqlite3_destructor_type.self)
        for (i, arg) in args.enumerated() {
            let n = Int32(i + 1)
            switch arg {
            case nil: sqlite3_bind_null(stmt, n)
            case let v as String: sqlite3_bind_text(stmt, n, v, -1, transient)
            case let v as Int: sqlite3_bind_int64(stmt, n, Int64(v))
            case let v as Double: sqlite3_bind_double(stmt, n, v)
            default: sqlite3_bind_null(stmt, n)
            }
        }
        return stmt
    }

    /// Runs a write statement.
    @discardableResult
    private func run(_ sql: String, _ args: Any?...) -> Bool {
        lock.lock(); defer { lock.unlock() }
        guard let stmt = prepare(sql, args) else { return false }
        defer { sqlite3_reset(stmt) }
        let rc = sqlite3_step(stmt)
        if rc != SQLITE_DONE && rc != SQLITE_ROW {
            NSLog("ImageVault SQL error: \(errorMessage) in \(sql.prefix(80))")
            return false
        }
        return true
    }

    /// Runs a query and maps each row.
    private func query<T>(_ sql: String, _ args: [Any?] = [], _ row: (OpaquePointer) -> T) -> [T] {
        lock.lock(); defer { lock.unlock() }
        guard let stmt = prepare(sql, args) else { return [] }
        defer { sqlite3_reset(stmt) }
        var out: [T] = []
        while sqlite3_step(stmt) == SQLITE_ROW { out.append(row(stmt)) }
        return out
    }

    /// Everything in `body` commits together (or not at all if it throws).
    private func transaction<T>(_ body: () throws -> T) rethrows -> T {
        lock.lock(); defer { lock.unlock() }
        exec("BEGIN IMMEDIATE")
        do {
            let result = try body()
            exec("COMMIT")
            return result
        } catch {
            exec("ROLLBACK")
            throw error
        }
    }

    private static func text(_ s: OpaquePointer, _ i: Int32) -> String? {
        sqlite3_column_type(s, i) == SQLITE_NULL ? nil : String(cString: sqlite3_column_text(s, i))
    }

    private static func int(_ s: OpaquePointer, _ i: Int32) -> Int? {
        sqlite3_column_type(s, i) == SQLITE_NULL ? nil : Int(sqlite3_column_int64(s, i))
    }

    // MARK: Rows

    private static let gridColumns = "id, name, mime, bytes, width, height, hash, url, pageTitle, pageUrl, domain, timestamp, source, ocrVersion, ocrLanguages"
    private static let allColumns = gridColumns + ", ocrText"

    private static func item(_ s: OpaquePointer) -> VaultItem {
        VaultItem(id: text(s, 0) ?? "", name: text(s, 1) ?? "", mime: text(s, 2), bytes: int(s, 3),
                  width: int(s, 4), height: int(s, 5), hash: text(s, 6), url: text(s, 7), pageTitle: text(s, 8),
                  pageUrl: text(s, 9), domain: text(s, 10), timestamp: sqlite3_column_double(s, 11),
                  source: text(s, 12), ocrText: sqlite3_column_count(s) > 15 ? text(s, 15) : nil,
                  ocrVersion: int(s, 13), ocrLanguages: text(s, 14))
    }

    /// Never `OR REPLACE`: SQLite doesn't fire delete triggers for replaced
    /// rows, which would leave stale search entries behind.
    private func insert(_ item: VaultItem) {
        run("""
            INSERT OR IGNORE INTO items (id, name, mime, bytes, width, height, hash, url, pageTitle, pageUrl,
                domain, timestamp, source, ocrVersion, ocrLanguages, ocrText)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """, item.id, item.name, item.mime, item.bytes, item.width, item.height, item.hash, item.url,
            item.pageTitle, item.pageUrl, item.domain, item.timestamp, item.source, item.ocrVersion,
            item.ocrLanguages, item.ocrText)
    }

    private func item(id: String) -> VaultItem? {
        query("SELECT \(Self.allColumns) FROM items WHERE id = ?", [id], Self.item).first
    }

    // MARK: Change notifications

    /// Items or groups changed: bump the generation and tell the UI once per run loop.
    private func changed() {
        lock.lock()
        generation += 1
        let post = !changePosted
        changePosted = true
        lock.unlock()
        guard post else { return }
        DispatchQueue.main.async { [self] in
            lock.lock(); changePosted = false; lock.unlock()
            NotificationCenter.default.post(name: .vaultStoreChanged, object: nil)
        }
    }

    private func ocrChanged() {
        lock.lock()
        let post = !ocrChangePosted
        ocrChangePosted = true
        lock.unlock()
        guard post else { return }
        DispatchQueue.main.async { [self] in
            lock.lock(); ocrChangePosted = false; lock.unlock()
            NotificationCenter.default.post(name: .vaultOCRChanged, object: nil)
        }
    }

    // MARK: Reading

    /// Every item (newest first, without OCR text) and every folder, read
    /// together with the generation they belong to.
    func snapshot() -> (index: VaultIndex, generation: Int) {
        lock.lock(); defer { lock.unlock() }
        let items = query("SELECT \(Self.gridColumns) FROM items ORDER BY timestamp DESC", [], Self.item)
        var members: [String: [String]] = [:]
        for (gid, iid) in query("SELECT group_id, item_id FROM group_items ORDER BY position", [], {
            (Self.text($0, 0) ?? "", Self.text($0, 1) ?? "")
        }) { members[gid, default: []].append(iid) }
        let groups = query("SELECT id, name, created FROM groups ORDER BY created DESC", []) { s -> VaultGroup in
            let id = Self.text(s, 0) ?? ""
            return VaultGroup(id: id, name: Self.text(s, 1) ?? "", itemIDs: members[id] ?? [],
                              created: sqlite3_column_double(s, 2))
        }
        return (VaultIndex(items: items, groups: groups), generation)
    }

    func load() -> VaultIndex { snapshot().index }

    /// Ids of items whose name, site, source or OCR text contains `needle`
    /// (case-insensitive). Nil means "no filter".
    func search(_ needle: String) -> Set<String>? {
        let needle = needle.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !needle.isEmpty else { return nil }
        // Trigram FTS needs 3+ characters; shorter needles scan with LIKE.
        if hasFTS && needle.count >= 3 {
            let phrase = "\"" + needle.replacingOccurrences(of: "\"", with: "\"\"") + "\""
            return Set(query("SELECT items.id FROM items_fts JOIN items ON items.rid = items_fts.rowid WHERE items_fts MATCH ?",
                             [phrase]) { Self.text($0, 0) ?? "" })
        }
        let escaped = needle.lowercased()
            .replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "%", with: "\\%")
            .replacingOccurrences(of: "_", with: "\\_")
        let like = "%\(escaped)%"
        let fields = ["name", "domain", "source", "pageTitle", "url", "pageUrl", "ocrText"]
        let clause = fields.map { "lower(\($0)) LIKE ?1 ESCAPE '\\'" }.joined(separator: " OR ")
        return Set(query("SELECT id FROM items WHERE \(clause)", [like]) { Self.text($0, 0) ?? "" })
    }

    func ocrText(for id: String) -> String? {
        query("SELECT ocrText FROM items WHERE id = ?", [id]) { Self.text($0, 0) }.first ?? nil
    }

    /// The newest image whose OCR is missing or out of date.
    func nextPendingOCR(version: Int, languages: String) -> VaultItem? {
        query("""
            SELECT \(Self.gridColumns) FROM items
            WHERE IFNULL(ocrVersion, 0) < ? OR IFNULL(ocrLanguages, '') != ?
            ORDER BY timestamp DESC LIMIT 1
            """, [version, languages], Self.item).first
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
    private func uniqueName(_ name: String, excluding id: String? = nil) -> String {
        let ext = (name as NSString).pathExtension
        let base = (name as NSString).deletingPathExtension
        var candidate = name
        var n = 2
        while !query("SELECT 1 FROM items WHERE name = ? COLLATE NOCASE AND id != ? LIMIT 1",
                     [candidate, id ?? ""], { _ in true }).isEmpty
                || FileManager.default.fileExists(atPath: imagesURL.appendingPathComponent(candidate).path) {
            candidate = ext.isEmpty ? "\(base) \(n)" : "\(base) \(n).\(ext)"
            n += 1
        }
        return candidate
    }

    // MARK: Migrations

    /// The index format before 2026-10-30 (vault.json, rewritten whole on every change).
    private struct LegacyIndex: Decodable {
        var items: [VaultItem] = []
        var groups: [VaultGroup]?
    }

    /// One-time import of vault.json into SQLite. The JSON file is kept,
    /// renamed, as a backup.
    private func importLegacyIndex() {
        let fm = FileManager.default
        guard db != nil, fm.fileExists(atPath: legacyIndexURL.path),
              query("SELECT value FROM meta WHERE key = 'importedJSON'", [], { _ in true }).isEmpty else { return }
        let stamp = Int(Date().timeIntervalSince1970)
        guard let data = try? Data(contentsOf: legacyIndexURL),
              let legacy = try? JSONDecoder().decode(LegacyIndex.self, from: data) else {
            // Keep a copy rather than lose it; the vault starts empty, as it
            // did before with a corrupt index.
            try? fm.copyItem(at: legacyIndexURL, to: root.appendingPathComponent("vault.corrupt-\(stamp).json"))
            run("INSERT OR REPLACE INTO meta (key, value) VALUES ('importedJSON', ?)", "corrupt")
            return
        }
        transaction {
            var seen = Set<String>()
            for item in legacy.items where seen.insert(item.id).inserted { insert(item) }
            for group in legacy.groups ?? [] {
                run("INSERT OR REPLACE INTO groups (id, name, created) VALUES (?, ?, ?)", group.id, group.name, group.created)
                for (i, iid) in group.itemIDs.enumerated() {
                    run("INSERT OR IGNORE INTO group_items (item_id, group_id, position) VALUES (?, ?, ?)", iid, group.id, i)
                }
            }
            run("INSERT OR REPLACE INTO meta (key, value) VALUES ('importedJSON', ?)", String(stamp))
        }
        let backup = root.appendingPathComponent("vault.json.pre-sqlite")
        if fm.fileExists(atPath: backup.path) { try? fm.removeItem(at: backup) }
        try? fm.moveItem(at: legacyIndexURL, to: backup)
        try? fm.removeItem(at: root.appendingPathComponent(".lock"))
        NSLog("ImageVault: imported \(legacy.items.count) items from vault.json into vault.sqlite")
    }

    /// One-time move from images/<id>/<name> to images/<name>.
    func migrateToFlatLayout() {
        let fm = FileManager.default
        guard let entries = try? fm.contentsOfDirectory(at: imagesURL, includingPropertiesForKeys: [.isDirectoryKey]),
              entries.contains(where: { (try? $0.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true }) else { return }
        transaction {
            for item in load().items {
                let legacy = legacyURL(for: item)
                guard fm.fileExists(atPath: legacy.path) else { continue }
                let name = uniqueName(item.name, excluding: item.id)
                do {
                    try fm.moveItem(at: legacy, to: imagesURL.appendingPathComponent(name))
                    run("UPDATE items SET name = ? WHERE id = ?", name, item.id)
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
        changed()
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
        let url = meta.url.flatMap { $0.hasPrefix("http") ? $0 : nil }
        let result = try transaction { () throws -> AddResult in
            if var dup = query("SELECT \(Self.gridColumns) FROM items WHERE hash = ?1 OR (?2 IS NOT NULL AND url = ?2) LIMIT 1",
                               [hash, meta.url], Self.item).first {
                dup.timestamp = Self.now
                run("UPDATE items SET timestamp = ? WHERE id = ?", dup.timestamp, dup.id)
                return .duplicate(dup)
            }
            let id = "mac_" + String(Int(Self.now), radix: 36) + "_" + Self.randomSuffix()
            let item = VaultItem(
                id: id,
                name: uniqueName(Self.fileName(meta.name ?? meta.url.flatMap { URL(string: $0)?.lastPathComponent }, ext: image.ext)),
                mime: image.mime, bytes: image.data.count, width: image.width, height: image.height, hash: hash,
                url: url, pageTitle: meta.pageTitle, pageUrl: meta.pageUrl, domain: meta.domain,
                timestamp: Self.now, source: meta.source
            )
            try write(image.data, for: item)
            insert(item)
            return .added(item)
        }
        changed()
        return result
    }

    private func write(_ data: Data, for item: VaultItem) throws {
        let url = fileURL(for: item)
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try data.write(to: url, options: .atomic)
    }

    // MARK: Updating

    /// Saves OCR text for one image. Doesn't touch the grid, so it posts
    /// `vaultOCRChanged` instead of a full reload. Nil version/languages keep
    /// the stored ones (Copy Text stores text without marking it indexed).
    func setOCR(id: String, text: String?, version: Int? = nil, languages: String? = nil) {
        run("""
            UPDATE items SET ocrText = ?, ocrVersion = COALESCE(?, ocrVersion), ocrLanguages = COALESCE(?, ocrLanguages)
            WHERE id = ?
            """, text, version, languages, id)
        ocrChanged()
    }

    /// Replaces an item's pixels in place (used by Edit → Save to Vault).
    func replaceData(of id: String, with raw: Data) throws -> VaultItem? {
        let image = try Imaging.portable(raw)
        let newHash = Self.hash(image.data)
        let result = try transaction { () throws -> VaultItem? in
            guard var item = item(id: id) else { return nil }
            // Same pixels (e.g. Copy on an unedited screenshot): nothing to write.
            if item.hash == newHash { return item }
            let old = fileURL(for: item)
            let base = (item.name as NSString).deletingPathExtension
            // Same extension: overwrite the file in place; otherwise swap it.
            let sameName = "\(base).\(image.ext)".lowercased() == item.name.lowercased()
                && old.deletingLastPathComponent() == imagesURL
            if !sameName {
                try? FileManager.default.removeItem(at: old)
                item.name = uniqueName("\(base).\(image.ext)", excluding: id)
            }
            item.mime = image.mime
            item.bytes = image.data.count
            item.width = image.width
            item.height = image.height
            item.hash = newHash
            item.ocrText = nil
            item.ocrVersion = nil
            item.timestamp = Self.now
            try write(image.data, for: item)
            run("""
                UPDATE items SET name = ?, mime = ?, bytes = ?, width = ?, height = ?, hash = ?, timestamp = ?,
                    ocrText = NULL, ocrVersion = NULL WHERE id = ?
                """, item.name, item.mime, item.bytes, item.width, item.height, item.hash, item.timestamp, id)
            return item
        }
        removeThumbnails(of: id)
        changed()
        return result
    }

    // MARK: Removing

    /// Removes items from the index. Files (and folder membership) are kept
    /// until `purgeFiles`, so undo can put them back.
    @discardableResult
    func delete(ids: [String]) -> [VaultItem] {
        let removed: [VaultItem] = transaction {
            var out: [VaultItem] = []
            for id in ids {
                guard let item = item(id: id) else { continue }
                run("DELETE FROM items WHERE id = ?", id)
                out.append(item)
            }
            return out
        }
        if !removed.isEmpty { changed() }
        return removed
    }

    func restore(_ items: [VaultItem]) {
        transaction {
            for item in items where self.item(id: item.id) == nil { insert(item) }
        }
        changed()
    }

    func purgeFiles(_ items: [VaultItem]) {
        transaction {
            for item in items where self.item(id: item.id) == nil {
                let nameInUse = !query("SELECT 1 FROM items WHERE name = ? COLLATE NOCASE LIMIT 1", [item.name], { _ in true }).isEmpty
                if !nameInUse { try? FileManager.default.removeItem(at: imagesURL.appendingPathComponent(item.name)) }
                try? FileManager.default.removeItem(at: imagesURL.appendingPathComponent(item.id, isDirectory: true))
                run("DELETE FROM group_items WHERE item_id = ?", item.id)
                removeThumbnails(of: item.id)
            }
        }
        changed()
    }

    private func removeThumbnails(of id: String) {
        Thumbnails.shared.forget(id)
    }

    // MARK: Groups

    @discardableResult
    func createGroup(name: String, ids: [String]) -> VaultGroup {
        let group = VaultGroup(id: "grp_" + String(Int(Self.now), radix: 36) + "_" + Self.randomSuffix(),
                               name: name, itemIDs: ids, created: Self.now)
        transaction {
            run("INSERT INTO groups (id, name, created) VALUES (?, ?, ?)", group.id, group.name, group.created)
            for (i, iid) in ids.enumerated() {
                run("INSERT OR REPLACE INTO group_items (item_id, group_id, position) VALUES (?, ?, ?)", iid, group.id, i)
            }
        }
        changed()
        return group
    }

    func addToGroup(_ groupID: String, ids: [String]) {
        transaction {
            var next = query("SELECT IFNULL(MAX(position), -1) + 1 FROM group_items WHERE group_id = ?", [groupID]) {
                Int(sqlite3_column_int64($0, 0))
            }.first ?? 0
            for iid in ids {
                run("INSERT OR REPLACE INTO group_items (item_id, group_id, position) VALUES (?, ?, ?)", iid, groupID, next)
                next += 1
            }
        }
        changed()
    }

    func removeFromGroups(ids: [String]) {
        transaction { for iid in ids { run("DELETE FROM group_items WHERE item_id = ?", iid) } }
        changed()
    }

    func renameGroup(_ groupID: String, to name: String) {
        run("UPDATE groups SET name = ? WHERE id = ?", name, groupID)
        changed()
    }

    /// Removes the folder; its images go back to the main grid.
    func deleteGroup(_ groupID: String) {
        run("DELETE FROM groups WHERE id = ?", groupID)
        changed()
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
