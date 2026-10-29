//! On-disk vault — port of VaultStore.swift + VaultModel.swift.
//!   %LOCALAPPDATA%\ImageVault\vault.json
//!   %LOCALAPPDATA%\ImageVault\images\<id>\<name>
//! The index is byte-compatible with the Mac app (camelCase, nil fields
//! omitted, newest first). Every read-modify-write holds a file lock.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE};
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::IO::OVERLAPPED;
use windows::core::PCWSTR;

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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VaultIndex {
    #[serde(default)]
    pub items: Vec<VaultItem>,
    #[serde(default)]
    pub tombstones: Vec<String>,
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
        r
    })
    .clone()
}

pub fn images_dir() -> PathBuf {
    root().join("images")
}

pub fn index_path() -> PathBuf {
    root().join("vault.json")
}

pub fn file_path(item: &VaultItem) -> PathBuf {
    images_dir().join(&item.id).join(&item.name)
}

static IN_PROCESS: Mutex<()> = Mutex::new(());
static CORRUPT_BACKED_UP: AtomicBool = AtomicBool::new(false);

struct FileLock(HANDLE);

impl FileLock {
    fn acquire() -> Option<FileLock> {
        unsafe {
            let w = util::wide_path(&root().join(".lock"));
            let h = CreateFileW(
                PCWSTR(w.as_ptr()),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_ALWAYS,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
            .ok()?;
            let mut ov = OVERLAPPED::default();
            if LockFileEx(h, LOCKFILE_EXCLUSIVE_LOCK, None, u32::MAX, u32::MAX, &mut ov).is_err() {
                let _ = CloseHandle(h);
                return None;
            }
            Some(FileLock(h))
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        unsafe {
            let mut ov = OVERLAPPED::default();
            let _ = UnlockFileEx(self.0, None, u32::MAX, u32::MAX, &mut ov);
            let _ = CloseHandle(self.0);
        }
    }
}

pub fn load() -> VaultIndex {
    let Ok(data) = fs::read(index_path()) else { return VaultIndex::default() };
    match serde_json::from_slice::<VaultIndex>(&data) {
        Ok(mut index) => {
            let mut seen = HashSet::new();
            index.items.retain(|i| seen.insert(i.id.clone()));
            index
        }
        Err(_) => {
            if !data.is_empty() && !CORRUPT_BACKED_UP.swap(true, Ordering::SeqCst) {
                let stamp = (util::now_ms() / 1000.0) as i64;
                let _ = fs::copy(index_path(), root().join(format!("vault.corrupt-{stamp}.json")));
            }
            VaultIndex::default()
        }
    }
}

/// Read-modify-write under the lock; items re-sorted newest first.
pub fn mutate<T>(body: impl FnOnce(&mut VaultIndex) -> T) -> T {
    let _p = IN_PROCESS.lock().unwrap_or_else(|e| e.into_inner());
    let _l = FileLock::acquire();
    let mut index = load();
    let out = body(&mut index);
    index.items.sort_by(|a, b| b.timestamp.partial_cmp(&a.timestamp).unwrap_or(std::cmp::Ordering::Equal));
    if let Ok(json) = serde_json::to_vec(&index) {
        let tmp = index_path().with_extension("json.tmp");
        let ok = fs::File::create(&tmp).and_then(|mut f| {
            f.write_all(&json)?;
            f.sync_all()
        });
        if ok.is_ok() {
            unsafe {
                let a = util::wide_path(&tmp);
                let b = util::wide_path(&index_path());
                let _ = MoveFileExW(PCWSTR(a.as_ptr()), PCWSTR(b.as_ptr()), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH);
            }
        }
    }
    out
}

pub fn index_modified() -> Option<SystemTime> {
    fs::metadata(index_path()).ok().and_then(|m| m.modified().ok())
}

/// Normalises the image, de-duplicates by content hash or source URL, and
/// stores it as a real file.
pub fn add(raw: &[u8], meta: &Meta) -> Result<AddResult, imaging::Error> {
    let image = imaging::portable(raw)?;
    let hash = hash(&image.data);
    let res = mutate(|index| {
        if let Some(i) = index
            .items
            .iter()
            .position(|it| it.hash.as_deref() == Some(hash.as_str()) || (meta.url.is_some() && it.url == meta.url))
        {
            let mut dup = index.items.remove(i);
            dup.timestamp = util::now_ms();
            index.items.insert(0, dup.clone());
            return Ok(AddResult::Duplicate(dup));
        }
        let url_name = meta.url.as_deref().and_then(url_last_component);
        let item = VaultItem {
            id: format!("win_{}_{}", base36(util::now_ms() as u64), random_suffix()),
            name: file_name(meta.name.as_deref().or(url_name.as_deref()), &image.ext),
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
        let path = file_path(&item);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(imaging::Error::Io)?;
        }
        fs::write(&path, &image.data).map_err(imaging::Error::Io)?;
        index.items.insert(0, item.clone());
        Ok(AddResult::Added(item))
    });
    res
}

/// Removes items and records tombstones. Files stay until `purge_files`.
pub fn delete(ids: &[String]) -> Vec<VaultItem> {
    let set: HashSet<&str> = ids.iter().map(String::as_str).collect();
    mutate(|index| {
        let removed: Vec<VaultItem> = index.items.iter().filter(|i| set.contains(i.id.as_str())).cloned().collect();
        index.items.retain(|i| !set.contains(i.id.as_str()));
        for id in ids {
            if !index.tombstones.contains(id) {
                index.tombstones.push(id.clone());
            }
        }
        removed
    })
}

pub fn restore(items: &[VaultItem]) {
    let ids: HashSet<&str> = items.iter().map(|i| i.id.as_str()).collect();
    mutate(|index| {
        index.tombstones.retain(|t| !ids.contains(t.as_str()));
        for item in items {
            if !index.items.iter().any(|i| i.id == item.id) {
                index.items.push(item.clone());
            }
        }
    });
}

pub fn purge_files(ids: &[String]) {
    let live: HashSet<String> = load().items.into_iter().map(|i| i.id).collect();
    for id in ids {
        if !live.contains(id) && !id.is_empty() && !id.contains(['\\', '/', '.']) {
            let _ = fs::remove_dir_all(images_dir().join(id));
        }
    }
}

/// Updates one item's OCR fields.
pub fn set_ocr(id: &str, text: Option<String>, version: Option<i64>, languages: Option<String>) {
    mutate(|index| {
        if let Some(it) = index.items.iter_mut().find(|i| i.id == id) {
            it.ocr_text = text;
            if version.is_some() {
                it.ocr_version = version;
            }
            if languages.is_some() {
                it.ocr_languages = languages;
            }
        }
    });
}

/// SHA-256, first 16 bytes, lowercase hex.
pub fn hash(data: &[u8]) -> String {
    let out = Sha256::digest(data);
    out[..16].iter().map(|b| format!("{b:02x}")).collect()
}

pub fn random_suffix() -> String {
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut buf = [0u8; 6];
    unsafe {
        let _ = windows::Win32::Security::Cryptography::BCryptGenRandom(
            None,
            &mut buf,
            windows::Win32::Security::Cryptography::BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        );
    }
    buf.iter().map(|b| CHARS[*b as usize % CHARS.len()] as char).collect()
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
    pub items: Vec<VaultItem>,
    pub by_id: HashMap<String, usize>,
    /// Ids added since the last reload — tiles play their entrance.
    pub fresh: HashSet<String>,
    last_modified: Option<SystemTime>,
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
            fresh: HashSet::new(),
            last_modified: None,
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

    /// True when the index changed on disk (caller then reloads).
    pub fn changed_on_disk(&self) -> bool {
        index_modified() != self.last_modified
    }

    pub fn reload(&mut self, new_ids: &[String]) {
        self.last_modified = index_modified();
        let old: HashSet<String> = self.items.iter().map(|i| i.id.clone()).collect();
        let mut seen = HashSet::new();
        let next: Vec<VaultItem> = load().items.into_iter().filter(|i| seen.insert(i.id.clone())).collect();
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
        self.undo.retain(|(t, _)| *t != token);
        let keep: HashSet<&str> = self.undo.iter().flat_map(|(_, b)| b.iter().map(|i| i.id.as_str())).collect();
        let purge: Vec<String> = ids.iter().filter(|id| !keep.contains(id.as_str())).cloned().collect();
        purge_files(&purge);
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
