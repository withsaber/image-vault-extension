//! Clipboard history (ClipboardHistory.swift): copied text and files, newest
//! first, capped at 300, stored in clipboard.json next to the vault. Copied
//! images go to the vault instead.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{clipboard, util, vault};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Text,
    Image,
    File,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipItem {
    pub id: String,
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<i64>,
    pub hash: String,
    /// Milliseconds since 1970.
    pub date: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

impl ClipItem {
    pub fn preview(&self) -> String {
        match self.kind {
            Kind::Text => self.text.clone().unwrap_or_default(),
            Kind::File => self
                .text
                .as_deref()
                .unwrap_or("")
                .split('\n')
                .filter(|s| !s.is_empty())
                .map(|p| PathBuf::from(p).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.to_string()))
                .collect::<Vec<_>>()
                .join("\n"),
            Kind::Image => match (self.width, self.height) {
                (Some(w), Some(h)) => format!("{w}\u{d7}{h}"),
                _ => "Image".into(),
            },
        }
    }
}

pub const LIMIT: usize = 300;

pub struct History {
    pub items: Vec<ClipItem>,
    /// Bumped on every change (views re-layout).
    pub version: u64,
}

fn index_path() -> PathBuf {
    vault::root().join("clipboard.json")
}

fn data_dir() -> PathBuf {
    vault::root().join("clipboard")
}

fn new_id() -> String {
    let mut b = [0u8; 8];
    unsafe {
        let _ = windows::Win32::Security::Cryptography::BCryptGenRandom(
            None,
            &mut b,
            windows::Win32::Security::Cryptography::BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        );
    }
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("clip_{}-{}", &hex[..8], &hex[8..11])
}

impl History {
    pub fn load() -> History {
        let _ = std::fs::create_dir_all(data_dir());
        let mut items: Vec<ClipItem> = std::fs::read(index_path()).ok().and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default();
        let mut h = History { items: Vec::new(), version: 0 };
        // Earlier builds kept copied images here too; those belong in the vault.
        if items.iter().any(|i| i.kind == Kind::Image) {
            items.retain(|i| i.kind != Kind::Image);
            let _ = std::fs::remove_dir_all(data_dir());
            let _ = std::fs::create_dir_all(data_dir());
            h.items = items;
            h.save();
        } else {
            h.items = items;
        }
        h
    }

    fn save(&mut self) {
        self.version += 1;
        if let Ok(json) = serde_json::to_vec(&self.items) {
            let tmp = index_path().with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, index_path());
            }
        }
    }

    /// Called by the clipboard watcher on every change (ours excluded).
    pub fn record(&mut self, snap: &clipboard::Snapshot, app: Option<String>) {
        if !snap.files.is_empty() {
            let paths = snap.files.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n");
            let hash = vault::hash(paths.as_bytes());
            self.add(ClipItem {
                id: new_id(),
                kind: Kind::File,
                text: Some(paths),
                image_file: None,
                width: None,
                height: None,
                hash,
                date: util::now_ms(),
                app,
            });
            return;
        }
        if let Some(text) = &snap.text {
            if !text.trim().is_empty() {
                let clipped: String = if text.chars().count() > 20_000 { text.chars().take(20_000).collect() } else { text.clone() };
                let hash = vault::hash(clipped.as_bytes());
                self.add(ClipItem {
                    id: new_id(),
                    kind: Kind::Text,
                    text: Some(clipped),
                    image_file: None,
                    width: None,
                    height: None,
                    hash,
                    date: util::now_ms(),
                    app,
                });
            }
        }
    }

    fn add(&mut self, item: ClipItem) {
        if self.bump(&item.hash) {
            return;
        }
        self.items.insert(0, item);
        if self.items.len() > LIMIT {
            self.items.truncate(LIMIT);
        }
        self.save();
    }

    /// Copying something already in the history moves it to the top.
    fn bump(&mut self, hash: &str) -> bool {
        let Some(i) = self.items.iter().position(|x| x.hash == hash) else { return false };
        let mut item = self.items.remove(i);
        item.date = util::now_ms();
        self.items.insert(0, item);
        self.save();
        true
    }

    pub fn copy(&mut self, id: &str) {
        let Some(item) = self.items.iter().find(|i| i.id == id).cloned() else { return };
        match item.kind {
            Kind::Text => clipboard::copy_text_marked(item.text.as_deref().unwrap_or("")),
            Kind::File => {
                let paths: Vec<PathBuf> = item.text.as_deref().unwrap_or("").split('\n').filter(|s| !s.is_empty()).map(PathBuf::from).collect();
                clipboard::copy_files(&paths);
            }
            Kind::Image => {}
        }
        // Copying a card leaves it where it is (copying the same thing again
        // from another app still moves it to the top, via `add`).
    }

    pub fn delete(&mut self, id: &str) {
        self.items.retain(|i| i.id != id);
        self.save();
    }

    pub fn clear(&mut self) {
        self.items.clear();
        let _ = std::fs::remove_dir_all(data_dir());
        let _ = std::fs::create_dir_all(data_dir());
        self.save();
    }

    pub fn matches(item: &ClipItem, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return true;
        }
        item.preview().to_lowercase().contains(&q) || item.app.as_deref().map(|a| a.to_lowercase().contains(&q)).unwrap_or(false)
    }

    pub fn filtered(&self, query: &str) -> Vec<&ClipItem> {
        self.items.iter().filter(|i| Self::matches(i, query)).collect()
    }
}

/// "now", "5m", "3h", "2d".
pub fn age(date_ms: f64) -> String {
    let s = ((util::now_ms() - date_ms) / 1000.0).max(0.0) as i64;
    if s < 60 {
        "now".into()
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86_400 {
        format!("{}h", s / 3600)
    } else {
        format!("{}d", s / 86_400)
    }
}
