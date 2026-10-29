//! On-disk vault — port of VaultStore.swift + VaultModel.swift.
//!   %LOCALAPPDATA%\ImageVault\vault.sqlite      (index, WAL mode)
//!   %LOCALAPPDATA%\ImageVault\images\<name>     (one folder, unique names)
//!   %LOCALAPPDATA%\ImageVault\thumbs\           (grid thumbnail cache)
//! Same schema as the Mac app: `items`, `groups` + `group_items`, `meta`, and
//! `items_fts` (FTS5 trigram over names, sites and OCR text, kept in sync by
//! triggers). Builds before 2026-10-30 kept the index in vault.json, rewritten
//! whole on every change; it's imported once and kept as vault.json.pre-sqlite.
//! Older builds kept `images\<id>\<name>`; `migrate_to_flat_layout` moves those
//! and `file_path` still finds any it couldn't move. Every write is one small
//! transaction, so saves, deletes and OCR results cost the same at 10 images
//! or 10,000. No Win32 here (it's tested on any OS); the app learns about
//! changes through `set_change_hook`.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::SystemTime;

use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::imaging;
use crate::util;


#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultItem {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(default)]
    pub timestamp: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ocr_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ocr_version: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ocr_languages: Option<String>,
}

impl VaultItem {
    pub fn dims(&self) -> Option<String> {
        match (self.width, self.height) {
            (Some(w), Some(h)) => Some(format!("{w}\u{d7}{h}")),
            _ => None,
        }
    }
}

/// A folder: membership only (files stay in `images`). An image belongs to
/// at most one group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VaultGroup {
    pub id: String,
    pub name: String,
    #[serde(rename = "itemIDs", default)]
    pub item_ids: Vec<String>,
    #[serde(default)]
    pub created: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VaultIndex {
    #[serde(default)]
    pub items: Vec<VaultItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub groups: Option<Vec<VaultGroup>>,
}

#[derive(Debug, Clone, Default)]
pub struct Meta {
    pub name: Option<String>,
    pub url: Option<String>,
    pub page_title: Option<String>,
    pub page_url: Option<String>,
    pub domain: Option<String>,
    pub source: String,
}

#[derive(Debug, Clone)]
pub enum AddResult {
    Added(VaultItem),
    Duplicate(VaultItem),
}

static ROOT: OnceLock<PathBuf> = OnceLock::new();

/// Vault root (IMAGEVAULT_ROOT overrides, like the Mac app).
pub fn root() -> PathBuf {
    ROOT.get_or_init(|| {
        let r = match std::env::var("IMAGEVAULT_ROOT") {
            Ok(v) if !v.is_empty() => PathBuf::from(v),
            _ => std::env::var("LOCALAPPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from(std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".into())).join("AppData\\Local"))
                .join("ImageVault"),
        };
        let _ = fs::create_dir_all(r.join("images"));
        let _ = fs::create_dir_all(r.join("thumbs"));
        r
    })
    .clone()
}

pub fn images_dir() -> PathBuf {
    root().join("images")
}

pub fn thumbs_dir() -> PathBuf {
    root().join("thumbs")
}

fn db_path() -> PathBuf {
    root().join("vault.sqlite")
}

fn legacy_index_path() -> PathBuf {
    root().join("vault.json")
}

/// images\<name>, or the old images\<id>\<name> if the file wasn't moved.
pub fn file_path(item: &VaultItem) -> PathBuf {
    let flat = images_dir().join(&item.name);
    let legacy = legacy_path(item);
    if !flat.exists() && legacy.exists() { legacy } else { flat }
}

fn legacy_path(item: &VaultItem) -> PathBuf {
    images_dir().join(&item.id).join(&item.name)
}

// ---------------------------------------------------------------------------
// SQLite
// ---------------------------------------------------------------------------

/// One connection; every statement runs under this lock (any thread).
static DB: OnceLock<Mutex<Option<Connection>>> = OnceLock::new();
static HAS_FTS: AtomicBool = AtomicBool::new(false);
/// Bumped on every change to items or groups; the UI model compares it to
/// skip reloads it has already done.
static GENERATION: AtomicU64 = AtomicU64::new(1);
/// Bumped when OCR text changes (the grid itself doesn't).
static OCR_GENERATION: AtomicU64 = AtomicU64::new(1);
static HOOK: OnceLock<fn(bool)> = OnceLock::new();

/// `hook(ocr_only)` runs after writes, from whatever thread wrote; the app
/// posts it to the UI thread (at most once per burst, see `changed`).
pub fn set_change_hook(hook: fn(bool)) {
    let _ = HOOK.set(hook);
}

pub fn generation() -> u64 {
    GENERATION.load(Ordering::SeqCst)
}

pub fn ocr_generation() -> u64 {
    OCR_GENERATION.load(Ordering::SeqCst)
}

fn changed() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    if let Some(h) = HOOK.get() {
        h(false);
    }
}

fn ocr_changed() {
    OCR_GENERATION.fetch_add(1, Ordering::SeqCst);
    if let Some(h) = HOOK.get() {
        h(true);
    }
}

fn db() -> MutexGuard<'static, Option<Connection>> {
    let m = DB.get_or_init(|| {
        let conn = open().map_err(|e| util::log(&format!("vault: can't open {}: {e}", db_path().display()))).ok();
        Mutex::new(conn)
    });
    let mut guard = m.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = guard.as_mut() {
        if !IMPORTED.swap(true, Ordering::SeqCst) {
            import_legacy_index(c);
        }
    }
    guard
}

static IMPORTED: AtomicBool = AtomicBool::new(false);

fn open() -> rusqlite::Result<Connection> {
    let _ = root();
    let c = Connection::open(db_path())?;
    c.busy_timeout(std::time::Duration::from_secs(3))?;
    let _: String = c.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    c.execute_batch(
        "PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         CREATE TABLE IF NOT EXISTS items (
             rid INTEGER PRIMARY KEY,
             id TEXT NOT NULL UNIQUE,
             name TEXT NOT NULL,
             mime TEXT, bytes INTEGER, width INTEGER, height INTEGER,
             hash TEXT, url TEXT, pageTitle TEXT, pageUrl TEXT, domain TEXT,
             timestamp REAL NOT NULL,
             source TEXT,
             ocrText TEXT, ocrVersion INTEGER, ocrLanguages TEXT
         );
         CREATE INDEX IF NOT EXISTS items_timestamp ON items(timestamp DESC);
         CREATE INDEX IF NOT EXISTS items_hash ON items(hash);
         CREATE INDEX IF NOT EXISTS items_url ON items(url);
         CREATE INDEX IF NOT EXISTS items_name ON items(name COLLATE NOCASE);
         CREATE TABLE IF NOT EXISTS groups (
             id TEXT PRIMARY KEY, name TEXT NOT NULL, created REAL NOT NULL
         );
         -- An image belongs to at most one folder. No foreign key to items: a
         -- deleted image keeps its membership through the undo window.
         CREATE TABLE IF NOT EXISTS group_items (
             item_id TEXT PRIMARY KEY,
             group_id TEXT NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
             position INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT);",
    )?;
    // Substring search over names, sites and OCR text. Trigram matches
    // "siam" inside "Siamese" case-insensitively, in any script.
    let fts = c
        .execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS items_fts USING fts5(
                 name, domain, source, pageTitle, url, pageUrl, ocrText,
                 content='items', content_rowid='rid', tokenize='trigram'
             );
             CREATE TRIGGER IF NOT EXISTS items_ai AFTER INSERT ON items BEGIN
                 INSERT INTO items_fts(rowid, name, domain, source, pageTitle, url, pageUrl, ocrText)
                 VALUES (new.rid, new.name, new.domain, new.source, new.pageTitle, new.url, new.pageUrl, new.ocrText);
             END;
             CREATE TRIGGER IF NOT EXISTS items_ad AFTER DELETE ON items BEGIN
                 INSERT INTO items_fts(items_fts, rowid, name, domain, source, pageTitle, url, pageUrl, ocrText)
                 VALUES ('delete', old.rid, old.name, old.domain, old.source, old.pageTitle, old.url, old.pageUrl, old.ocrText);
             END;
             CREATE TRIGGER IF NOT EXISTS items_au AFTER UPDATE ON items BEGIN
                 INSERT INTO items_fts(items_fts, rowid, name, domain, source, pageTitle, url, pageUrl, ocrText)
                 VALUES ('delete', old.rid, old.name, old.domain, old.source, old.pageTitle, old.url, old.pageUrl, old.ocrText);
                 INSERT INTO items_fts(rowid, name, domain, source, pageTitle, url, pageUrl, ocrText)
                 VALUES (new.rid, new.name, new.domain, new.source, new.pageTitle, new.url, new.pageUrl, new.ocrText);
             END;",
        )
        .map_err(|e| util::log(&format!("vault: no full-text search: {e}")))
        .is_ok();
    HAS_FTS.store(fts, Ordering::SeqCst);
    Ok(c)
}

/// Runs `f` on the open database (None when it couldn't be opened).
fn with_db<R>(f: impl FnOnce(&mut Connection) -> rusqlite::Result<R>) -> Option<R> {
    let mut g = db();
    let c = g.as_mut()?;
    match f(c) {
        Ok(r) => Some(r),
        Err(e) => {
            util::log(&format!("vault SQL error: {e}"));
            None
        }
    }
}

/// Everything in `body` commits together (or not at all on error).
fn transaction<R>(body: impl FnOnce(&rusqlite::Transaction) -> rusqlite::Result<R>) -> Option<R> {
    with_db(|c| {
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let r = body(&tx)?;
        tx.commit()?;
        Ok(r)
    })
}

const GRID_COLUMNS: &str = "id, name, mime, bytes, width, height, hash, url, pageTitle, pageUrl, domain, timestamp, source, ocrVersion, ocrLanguages";
const ALL_COLUMNS: &str = "id, name, mime, bytes, width, height, hash, url, pageTitle, pageUrl, domain, timestamp, source, ocrVersion, ocrLanguages, ocrText";

fn row_item(r: &Row) -> rusqlite::Result<VaultItem> {
    Ok(VaultItem {
        id: r.get(0)?,
        name: r.get(1)?,
        mime: r.get(2)?,
        bytes: r.get(3)?,
        width: r.get(4)?,
        height: r.get(5)?,
        hash: r.get(6)?,
        url: r.get(7)?,
        page_title: r.get(8)?,
        page_url: r.get(9)?,
        domain: r.get(10)?,
        timestamp: r.get(11)?,
        source: r.get(12)?,
        ocr_version: r.get(13)?,
        ocr_languages: r.get(14)?,
        ocr_text: if r.as_ref().column_count() > 15 { r.get(15)? } else { None },
    })
}

/// Never `OR REPLACE`: SQLite doesn't fire delete triggers for replaced rows,
/// which would leave stale search entries behind.
fn insert(c: &Connection, i: &VaultItem) -> rusqlite::Result<usize> {
    c.prepare_cached(
        "INSERT OR IGNORE INTO items (id, name, mime, bytes, width, height, hash, url, pageTitle, pageUrl,
             domain, timestamp, source, ocrVersion, ocrLanguages, ocrText)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
    )?
    .execute(params![
        i.id, i.name, i.mime, i.bytes, i.width, i.height, i.hash, i.url, i.page_title, i.page_url, i.domain, i.timestamp, i.source,
        i.ocr_version, i.ocr_languages, i.ocr_text
    ])
}

fn item_by_id(c: &Connection, id: &str) -> rusqlite::Result<Option<VaultItem>> {
    c.prepare_cached(&format!("SELECT {ALL_COLUMNS} FROM items WHERE id = ?1"))?.query_row([id], row_item).optional()
}

fn name_taken(c: &Connection, name: &str, excluding: Option<&str>) -> rusqlite::Result<bool> {
    Ok(c.prepare_cached("SELECT 1 FROM items WHERE name = ?1 COLLATE NOCASE AND id != ?2 LIMIT 1")?
        .query_row(params![name, excluding.unwrap_or("")], |_| Ok(()))
        .optional()?
        .is_some())
}

/// A name no other item (or leftover file) uses: "Shot.png", "Shot 2.png"…
fn unique_name(c: &Connection, name: &str, excluding: Option<&str>) -> rusqlite::Result<String> {
    let (base, ext) = match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], Some(&name[dot + 1..])),
        _ => (name, None),
    };
    let mut candidate = name.to_string();
    let mut n = 2;
    while name_taken(c, &candidate, excluding)? || images_dir().join(&candidate).exists() {
        candidate = match ext {
            Some(e) => format!("{base} {n}.{e}"),
            None => format!("{base} {n}"),
        };
        n += 1;
    }
    Ok(candidate)
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// Every item (newest first, without OCR text) and every folder, read
/// together with the generation they belong to.
pub fn snapshot() -> (VaultIndex, u64) {
    let mut g = db();
    let generation = generation();
    let Some(c) = g.as_mut() else { return (VaultIndex::default(), generation) };
    let read = || -> rusqlite::Result<VaultIndex> {
        let items = c
            .prepare_cached(&format!("SELECT {GRID_COLUMNS} FROM items ORDER BY timestamp DESC"))?
            .query_map([], row_item)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut members: HashMap<String, Vec<String>> = HashMap::new();
        let mut st = c.prepare_cached("SELECT group_id, item_id FROM group_items ORDER BY position")?;
        for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (gid, iid) = row?;
            members.entry(gid).or_default().push(iid);
        }
        let groups = c
            .prepare_cached("SELECT id, name, created FROM groups ORDER BY created DESC")?
            .query_map([], |r| {
                let id: String = r.get(0)?;
                Ok(VaultGroup { item_ids: members.get(&id).cloned().unwrap_or_default(), id, name: r.get(1)?, created: r.get(2)? })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(VaultIndex { items, groups: Some(groups) })
    };
    match read() {
        Ok(index) => (index, generation),
        Err(e) => {
            util::log(&format!("vault SQL error: {e}"));
            (VaultIndex::default(), generation)
        }
    }
}

pub fn load() -> VaultIndex {
    snapshot().0
}

pub fn item(id: &str) -> Option<VaultItem> {
    with_db(|c| item_by_id(c, id)).flatten()
}

/// Ids of items whose name, site, source or OCR text contains `needle`
/// (case-insensitive). None means "no filter".
pub fn search(needle: &str) -> Option<HashSet<String>> {
    let needle = needle.trim();
    if needle.is_empty() {
        return None;
    }
    let ids = with_db(|c| {
        // Trigram FTS needs 3+ characters; shorter needles scan with LIKE.
        if HAS_FTS.load(Ordering::SeqCst) && needle.chars().count() >= 3 {
            let phrase = format!("\"{}\"", needle.replace('"', "\"\""));
            let mut st = c.prepare_cached("SELECT items.id FROM items_fts JOIN items ON items.rid = items_fts.rowid WHERE items_fts MATCH ?1")?;
            return st.query_map([phrase], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<HashSet<_>>>();
        }
        let escaped = needle.to_lowercase().replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
        let like = format!("%{escaped}%");
        let clause = ["name", "domain", "source", "pageTitle", "url", "pageUrl", "ocrText"]
            .iter()
            .map(|f| format!("lower({f}) LIKE ?1 ESCAPE '\\'"))
            .collect::<Vec<_>>()
            .join(" OR ");
        let mut st = c.prepare_cached(&format!("SELECT id FROM items WHERE {clause}"))?;
        st.query_map([like], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<HashSet<_>>>()
    });
    Some(ids.unwrap_or_default())
}

pub fn ocr_text(id: &str) -> Option<String> {
    with_db(|c| c.prepare_cached("SELECT ocrText FROM items WHERE id = ?1")?.query_row([id], |r| r.get::<_, Option<String>>(0)).optional())
        .flatten()
        .flatten()
}

/// The newest image whose OCR is missing or out of date.
pub fn next_pending_ocr(version: i64, languages: &str) -> Option<VaultItem> {
    with_db(|c| {
        c.prepare_cached(&format!(
            "SELECT {GRID_COLUMNS} FROM items WHERE IFNULL(ocrVersion, 0) < ?1 OR IFNULL(ocrLanguages, '') != ?2
             ORDER BY timestamp DESC LIMIT 1"
        ))?
        .query_row(params![version, languages], row_item)
        .optional()
    })
    .flatten()
}

// ---------------------------------------------------------------------------
// Migrations
// ---------------------------------------------------------------------------

/// The index format before 2026-10-30 (vault.json, rewritten whole on every change).
#[derive(Deserialize)]
struct LegacyIndex {
    #[serde(default)]
    items: Vec<VaultItem>,
    #[serde(default)]
    groups: Option<Vec<VaultGroup>>,
}

/// One-time import of vault.json. The JSON file is kept, renamed, as a backup.
fn import_legacy_index(c: &mut Connection) {
    let json = legacy_index_path();
    if !json.exists() {
        return;
    }
    let done: Option<String> = c.query_row("SELECT value FROM meta WHERE key = 'importedJSON'", [], |r| r.get(0)).optional().ok().flatten();
    if done.is_some() {
        return;
    }
    let stamp = (util::now_ms() / 1000.0) as i64;
    let data = fs::read(&json).unwrap_or_default();
    let Ok(legacy) = serde_json::from_slice::<LegacyIndex>(&data) else {
        // Keep a copy rather than lose it; the vault starts empty, as it did
        // before with a corrupt index.
        let _ = fs::copy(&json, root().join(format!("vault.corrupt-{stamp}.json")));
        let _ = c.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('importedJSON', 'corrupt')", []);
        return;
    };
    let result = (|| -> rusqlite::Result<()> {
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut seen = HashSet::new();
        for item in legacy.items.iter().filter(|i| seen.insert(i.id.clone())) {
            insert(&tx, item)?;
        }
        for g in legacy.groups.iter().flatten() {
            tx.execute("INSERT OR REPLACE INTO groups (id, name, created) VALUES (?1, ?2, ?3)", params![g.id, g.name, g.created])?;
            for (i, iid) in g.item_ids.iter().enumerate() {
                tx.execute("INSERT OR IGNORE INTO group_items (item_id, group_id, position) VALUES (?1, ?2, ?3)", params![iid, g.id, i as i64])?;
            }
        }
        tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('importedJSON', ?1)", [stamp.to_string()])?;
        tx.commit()
    })();
    if let Err(e) = result {
        util::log(&format!("vault: import of vault.json failed: {e}"));
        return;
    }
    let backup = root().join("vault.json.pre-sqlite");
    let _ = fs::remove_file(&backup);
    let _ = fs::rename(&json, &backup);
    let _ = fs::remove_file(root().join(".lock"));
    util::log(&format!("vault: imported {} items from vault.json into vault.sqlite", legacy.items.len()));
}

/// Moves images\<id>\<name> files into images\ (renaming on clashes) and
/// removes the emptied per-image folders. Runs at launch; cheap when done.
pub fn migrate_to_flat_layout() {
    let Ok(entries) = fs::read_dir(images_dir()) else { return };
    let dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    if dirs.is_empty() {
        return;
    }
    let items = load().items;
    transaction(|tx| {
        for item in &items {
            let legacy = legacy_path(item);
            if !legacy.exists() {
                continue;
            }
            // Items still in their old folder don't hold a name yet (so the
            // first of two "Shot.png" keeps it): only files already moved count.
            let (base, ext) = match item.name.rfind('.') {
                Some(dot) if dot > 0 => (&item.name[..dot], Some(&item.name[dot + 1..])),
                _ => (item.name.as_str(), None),
            };
            let mut name = item.name.clone();
            let mut n = 2;
            while images_dir().join(&name).exists() {
                name = match ext {
                    Some(e) => format!("{base} {n}.{e}"),
                    None => format!("{base} {n}"),
                };
                n += 1;
            }
            if fs::rename(&legacy, images_dir().join(&name)).is_ok() {
                tx.execute("UPDATE items SET name = ?1 WHERE id = ?2", params![name, item.id])?;
                let _ = fs::remove_dir(images_dir().join(&item.id));
            }
        }
        Ok(())
    });
    // Empty per-image folders left by deleted items.
    for dir in dirs {
        if dir.exists() && fs::read_dir(&dir).map(|mut d| d.next().is_none()).unwrap_or(false) {
            let _ = fs::remove_dir(&dir);
        }
    }
    changed();
}

// ---------------------------------------------------------------------------
// Adding
// ---------------------------------------------------------------------------

/// Normalises the image, de-duplicates by content hash or source URL, and
/// stores it as a real file.
pub fn add(raw: &[u8], meta: &Meta) -> Result<AddResult, imaging::Error> {
    let image = imaging::portable(raw)?;
    let hash = hash(&image.data);
    let mut io_err = None;
    let res = transaction(|tx| {
        let dup = tx
            .prepare_cached(&format!("SELECT {GRID_COLUMNS} FROM items WHERE hash = ?1 OR (?2 IS NOT NULL AND url = ?2) LIMIT 1"))?
            .query_row(params![hash, meta.url], row_item)
            .optional()?;
        if let Some(mut dup) = dup {
            dup.timestamp = util::now_ms();
            tx.execute("UPDATE items SET timestamp = ?1 WHERE id = ?2", params![dup.timestamp, dup.id])?;
            return Ok(Some(AddResult::Duplicate(dup)));
        }
        let url_name = meta.url.as_deref().and_then(url_last_component);
        let name = unique_name(tx, &file_name(meta.name.as_deref().or(url_name.as_deref()), &image.ext), None)?;
        let item = VaultItem {
            id: format!("win_{}_{}", base36(util::now_ms() as u64), random_suffix()),
            name,
            mime: Some(image.mime.clone()),
            bytes: Some(image.data.len() as i64),
            width: image.width.map(|v| v as i64),
            height: image.height.map(|v| v as i64),
            hash: Some(hash.clone()),
            url: meta.url.as_deref().filter(|u| u.starts_with("http")).map(str::to_owned),
            page_title: meta.page_title.clone(),
            page_url: meta.page_url.clone(),
            domain: meta.domain.clone(),
            timestamp: util::now_ms(),
            source: Some(meta.source.clone()),
            ocr_text: None,
            ocr_version: None,
            ocr_languages: None,
        };
        if let Err(e) = write_file(&item.name, &image.data) {
            io_err = Some(e);
            return Ok(None);
        }
        insert(tx, &item)?;
        Ok(Some(AddResult::Added(item)))
    });
    if let Some(e) = io_err {
        return Err(imaging::Error::Io(e));
    }
    let res = res.flatten().ok_or_else(|| imaging::Error::Io(std::io::Error::other("vault database unavailable")))?;
    changed();
    Ok(res)
}

fn write_file(name: &str, data: &[u8]) -> std::io::Result<()> {
    fs::create_dir_all(images_dir())?;
    let path = images_dir().join(name);
    let tmp = images_dir().join(format!(".{name}.tmp"));
    fs::write(&tmp, data)?;
    let _ = fs::remove_file(&path);
    fs::rename(&tmp, &path)
}

// ---------------------------------------------------------------------------
// Updating
// ---------------------------------------------------------------------------

/// Saves OCR text for one image: one row update, safe off the UI thread. It
/// doesn't touch the grid, so it signals an OCR change, not a reload. None
/// for version/languages keeps the stored ones (Copy Text stores text
/// without marking it indexed).
pub fn set_ocr(id: &str, text: Option<String>, version: Option<i64>, languages: Option<String>) {
    with_db(|c| {
        c.prepare_cached(
            "UPDATE items SET ocrText = ?1, ocrVersion = COALESCE(?2, ocrVersion), ocrLanguages = COALESCE(?3, ocrLanguages) WHERE id = ?4",
        )?
        .execute(params![text, version, languages, id])
    });
    ocr_changed();
}

/// Writes new pixels over an item (editing a vault image). Same content:
/// nothing is written. A new extension renames the file (kept unique).
pub fn replace_data(id: &str, raw: &[u8]) -> Result<Option<VaultItem>, imaging::Error> {
    let image = imaging::portable(raw)?;
    let new_hash = hash(&image.data);
    let mut io_err = None;
    let mut wrote = false;
    let res = transaction(|tx| {
        let Some(mut item) = item_by_id(tx, id)? else { return Ok(None) };
        // Same pixels (e.g. Copy on an unedited screenshot): nothing to write.
        if item.hash.as_deref() == Some(new_hash.as_str()) {
            return Ok(Some(item));
        }
        let old = file_path(&item);
        let base = match item.name.rfind('.') {
            Some(dot) if dot > 0 => item.name[..dot].to_string(),
            _ => item.name.clone(),
        };
        let wanted = format!("{base}.{}", image.ext);
        let same_name = wanted.to_lowercase() == item.name.to_lowercase() && old.parent() == Some(images_dir().as_path());
        if !same_name {
            let _ = fs::remove_file(&old);
            item.name = unique_name(tx, &wanted, Some(id))?;
        }
        item.mime = Some(image.mime.clone());
        item.bytes = Some(image.data.len() as i64);
        item.width = image.width.map(|v| v as i64);
        item.height = image.height.map(|v| v as i64);
        item.hash = Some(new_hash.clone());
        item.ocr_text = None;
        item.ocr_version = None;
        item.timestamp = util::now_ms();
        if let Err(e) = write_file(&item.name, &image.data) {
            io_err = Some(e);
            return Ok(None);
        }
        tx.execute(
            "UPDATE items SET name = ?1, mime = ?2, bytes = ?3, width = ?4, height = ?5, hash = ?6, timestamp = ?7,
                 ocrText = NULL, ocrVersion = NULL WHERE id = ?8",
            params![item.name, item.mime, item.bytes, item.width, item.height, item.hash, item.timestamp, id],
        )?;
        wrote = true;
        Ok(Some(item))
    });
    if let Some(e) = io_err {
        return Err(imaging::Error::Io(e));
    }
    if wrote {
        remove_thumbnails(id);
        changed();
    }
    Ok(res.flatten())
}

// ---------------------------------------------------------------------------
// Removing
// ---------------------------------------------------------------------------

/// Removes items from the index. Files (and folder membership) stay until
/// `purge_files`, so undo can put them back.
pub fn delete(ids: &[String]) -> Vec<VaultItem> {
    let removed = transaction(|tx| {
        let mut out = Vec::new();
        for id in ids {
            if let Some(item) = item_by_id(tx, id)? {
                tx.execute("DELETE FROM items WHERE id = ?1", [id])?;
                out.push(item);
            }
        }
        Ok(out)
    })
    .unwrap_or_default();
    if !removed.is_empty() {
        changed();
    }
    removed
}

pub fn restore(items: &[VaultItem]) {
    transaction(|tx| {
        for item in items {
            if item_by_id(tx, &item.id)?.is_none() {
                insert(tx, item)?;
            }
        }
        Ok(())
    });
    changed();
}

/// Deletes the files of items that are still gone once the undo window
/// closes, their thumbnails, and their folder membership.
pub fn purge_files(items: &[VaultItem]) {
    let mut purged = Vec::new();
    transaction(|tx| {
        for item in items {
            if item_by_id(tx, &item.id)?.is_some() {
                continue;
            }
            if !item.name.is_empty() && !item.name.contains(['\\', '/']) && !name_taken(tx, &item.name, None)? {
                let _ = fs::remove_file(images_dir().join(&item.name));
            }
            if !item.id.is_empty() && !item.id.contains(['\\', '/', '.']) {
                let _ = fs::remove_dir_all(images_dir().join(&item.id));
            }
            tx.execute("DELETE FROM group_items WHERE item_id = ?1", [&item.id])?;
            purged.push(item.id.clone());
        }
        Ok(())
    });
    for id in &purged {
        remove_thumbnails(id);
    }
    changed();
}

// ---------------------------------------------------------------------------
// Thumbnail files (thumbs\<id>@<px>-<hash8>.jpg|png)
// ---------------------------------------------------------------------------

/// Cache file stem for an item's thumbnail at `px`; the content hash in the
/// name means an edited image never shows a stale thumbnail.
pub fn thumb_stem(id: &str, px: u32, hash: Option<&str>) -> PathBuf {
    let h: String = hash.unwrap_or("x").chars().take(8).collect();
    thumbs_dir().join(format!("{id}@{px}-{h}"))
}

/// Deletes every cached thumbnail file of an image (edited or gone).
pub fn remove_thumbnails(id: &str) {
    let prefix = format!("{id}@");
    if let Ok(entries) = fs::read_dir(thumbs_dir()) {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().starts_with(&prefix) {
                let _ = fs::remove_file(e.path());
            }
        }
    }
}

// -- groups (folders) ----------------------------------------------------------

fn next_position(tx: &Connection, gid: &str) -> rusqlite::Result<i64> {
    tx.query_row("SELECT IFNULL(MAX(position), -1) + 1 FROM group_items WHERE group_id = ?1", [gid], |r| r.get(0))
}

/// Puts `ids` (in order) into the folder, taking them out of any other one.
/// Membership is the row's primary key, so it's delete-then-insert, never
/// `OR REPLACE`.
fn put_in_group(tx: &Connection, gid: &str, ids: &[String]) -> rusqlite::Result<()> {
    let mut next = next_position(tx, gid)?;
    for iid in ids {
        let already: Option<String> =
            tx.query_row("SELECT group_id FROM group_items WHERE item_id = ?1", [iid], |r| r.get(0)).optional()?;
        if already.as_deref() == Some(gid) {
            continue;
        }
        tx.execute("DELETE FROM group_items WHERE item_id = ?1", [iid])?;
        tx.execute("INSERT INTO group_items (item_id, group_id, position) VALUES (?1, ?2, ?3)", params![iid, gid, next])?;
        next += 1;
    }
    Ok(())
}

pub fn create_group(name: &str, ids: &[String]) -> VaultGroup {
    let group = VaultGroup {
        id: format!("grp_{}_{}", base36(util::now_ms() as u64), random_suffix()),
        name: name.to_string(),
        item_ids: ids.to_vec(),
        created: util::now_ms(),
    };
    transaction(|tx| {
        tx.execute("INSERT INTO groups (id, name, created) VALUES (?1, ?2, ?3)", params![group.id, group.name, group.created])?;
        put_in_group(tx, &group.id, ids)
    });
    changed();
    group
}

pub fn add_to_group(group_id: &str, ids: &[String]) {
    transaction(|tx| put_in_group(tx, group_id, ids));
    changed();
}

pub fn remove_from_groups(ids: &[String]) {
    transaction(|tx| {
        for iid in ids {
            tx.execute("DELETE FROM group_items WHERE item_id = ?1", [iid])?;
        }
        Ok(())
    });
    changed();
}

pub fn rename_group(group_id: &str, name: &str) {
    with_db(|c| c.execute("UPDATE groups SET name = ?1 WHERE id = ?2", params![name, group_id]));
    changed();
}

/// Removes the folder; its images go back to the main grid.
pub fn delete_group(group_id: &str) {
    with_db(|c| c.execute("DELETE FROM groups WHERE id = ?1", [group_id]));
    changed();
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------


/// SHA-256, first 16 bytes, lowercase hex.
pub fn hash(data: &[u8]) -> String {
    let out = Sha256::digest(data);
    out[..16].iter().map(|b| format!("{b:02x}")).collect()
}

pub fn random_suffix() -> String {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::AtomicU64;
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    h.write_u128(SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    let mut n = h.finish();
    (0..6)
        .map(|_| {
            let c = CHARS[(n % 36) as usize] as char;
            n /= 36;
            c
        })
        .collect()
}

pub fn file_name(given: Option<&str>, ext: &str) -> String {
    let given = given.unwrap_or("");
    let mut base = percent_decode(given).unwrap_or_else(|| given.to_string());
    if let Some(dot) = base.rfind('.') {
        if base[dot..].chars().count() <= 6 {
            base.truncate(dot);
        }
    }
    let mut out = String::with_capacity(base.len());
    let mut run = false;
    for ch in base.chars() {
        if ch.is_ascii_alphanumeric() || ch == ' ' || ch == '_' || ch == '-' {
            out.push(ch);
            run = false;
        } else if !run {
            out.push('_');
            run = true;
        }
    }
    let mut base: String = out.trim_matches(|c| c == ' ' || c == '_').to_string();
    if base.chars().count() > 60 {
        base = base.chars().take(60).collect();
    }
    if base.is_empty() {
        base = format!("Image {}", util::stamp());
    }
    format!("{base}.{ext}")
}

fn base36(mut n: u64) -> String {
    const C: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".into();
    }
    let mut s = Vec::new();
    while n > 0 {
        s.push(C[(n % 36) as usize]);
        n /= 36;
    }
    s.reverse();
    String::from_utf8(s).unwrap()
}

pub fn percent_decode(s: &str) -> Option<String> {
    if !s.contains('%') {
        return Some(s.to_string());
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// URL(string:)?.lastPathComponent
pub fn url_last_component(url: &str) -> Option<String> {
    let no_q = url.split(['?', '#']).next().unwrap_or(url);
    let after_scheme = no_q.split_once("://").map(|(_, r)| r).unwrap_or(no_q);
    let path = after_scheme.split_once('/').map(|(_, p)| p).unwrap_or("");
    let last = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    if last.is_empty() { None } else { Some(last.to_string()) }
}

/// URL(string:)?.host
pub fn url_host(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit('@').next()?;
    let host = host.split(':').next()?;
    if host.is_empty() { None } else { Some(host.to_lowercase()) }
}


// ---------------------------------------------------------------------------
// UI-side model (VaultModel.swift)
// ---------------------------------------------------------------------------

pub struct Model {
    /// Newest first, without OCR text (search runs in SQLite).
    pub items: Vec<VaultItem>,
    pub by_id: HashMap<String, usize>,
    /// Folders, newest first, holding only ids that are in the vault.
    pub groups: Vec<VaultGroup>,
    /// item id → group id.
    pub group_of: HashMap<String, String>,
    /// Ids added since the last reload — tiles play their entrance.
    pub fresh: HashSet<String>,
    /// The store generation this model reflects.
    generation: u64,
    undo: Vec<(u64, Vec<VaultItem>)>,
    next_token: u64,
    /// Bumped on every reload so views can refresh.
    pub version: u64,
}

impl Model {
    pub fn new() -> Model {
        let mut m = Model {
            items: Vec::new(),
            by_id: HashMap::new(),
            groups: Vec::new(),
            group_of: HashMap::new(),
            fresh: HashSet::new(),
            generation: 0,
            undo: Vec::new(),
            next_token: 1,
            version: 0,
        };
        m.reload(&[]);
        m
    }

    pub fn get(&self, id: &str) -> Option<&VaultItem> {
        self.by_id.get(id).map(|&i| &self.items[i])
    }

    /// True when the store changed since the last reload.
    pub fn changed(&self) -> bool {
        generation() != self.generation
    }

    pub fn reload(&mut self, new_ids: &[String]) {
        let (index, generation) = snapshot();
        self.generation = generation;
        let old: HashSet<String> = self.items.iter().map(|i| i.id.clone()).collect();
        let mut seen = HashSet::new();
        let next: Vec<VaultItem> = index.items.into_iter().filter(|i| seen.insert(i.id.clone())).collect();
        // Membership of deleted (undoable) images stays in the store but not here.
        let mut claimed = HashSet::new();
        self.groups = index
            .groups
            .unwrap_or_default()
            .into_iter()
            .map(|mut g| {
                g.item_ids.retain(|id| seen.contains(id) && claimed.insert(id.clone()));
                g
            })
            .collect();
        self.group_of = self.groups.iter().flat_map(|g| g.item_ids.iter().map(move |id| (id.clone(), g.id.clone()))).collect();
        let mut fresh: HashSet<String> = new_ids.iter().cloned().collect();
        if !old.is_empty() {
            for i in &next {
                if !old.contains(&i.id) {
                    fresh.insert(i.id.clone());
                }
            }
        }
        self.by_id = next.iter().enumerate().map(|(i, it)| (it.id.clone(), i)).collect();
        self.items = next;
        self.fresh = fresh;
        self.version += 1;
    }

    /// Delete with undo; returns the undo token (files purge after 60 s).
    pub fn delete(&mut self, ids: &[String]) -> Option<(u64, Vec<String>)> {
        let removed = delete(ids);
        if removed.is_empty() {
            return None;
        }
        let token = self.next_token;
        self.next_token += 1;
        let ids: Vec<String> = removed.iter().map(|i| i.id.clone()).collect();
        self.undo.push((token, removed));
        if self.undo.len() > 20 {
            self.undo.remove(0);
        }
        self.reload(&[]);
        Some((token, ids))
    }

    /// The 60-second undo window for `token` closed.
    pub fn expire(&mut self, token: u64, ids: &[String]) {
        let batch: Vec<VaultItem> = self.undo.iter().filter(|(t, _)| *t == token).flat_map(|(_, b)| b.iter().cloned()).collect();
        self.undo.retain(|(t, _)| *t != token);
        let keep: HashSet<&str> = self.undo.iter().flat_map(|(_, b)| b.iter().map(|i| i.id.as_str())).collect();
        let purge: Vec<VaultItem> = batch.into_iter().filter(|i| ids.contains(&i.id) && !keep.contains(i.id.as_str())).collect();
        purge_files(&purge);
    }

    pub fn group(&self, id: &str) -> Option<&VaultGroup> {
        self.groups.iter().find(|g| g.id == id)
    }

    pub fn undo_delete(&mut self) -> usize {
        let Some((_, items)) = self.undo.pop() else { return 0 };
        restore(&items);
        self.reload(&[]);
        items.len()
    }

    pub fn total_bytes(&self) -> i64 {
        self.items.iter().map(|i| i.bytes.unwrap_or(0)).sum()
    }
}

pub fn exists(item: &VaultItem) -> bool {
    Path::new(&file_path(item)).exists()
}
