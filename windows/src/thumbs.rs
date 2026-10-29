//! Thumbnail cache (Thumbnails.swift): decoding runs on a small LIFO worker
//! pool, the UI thread uploads results as GPU bitmaps and keeps an LRU of
//! them. Most recently requested work runs first, so scrolling stays snappy.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Condvar, Mutex};

use windows::Win32::Graphics::Direct2D::ID2D1Bitmap1;

use crate::imaging::{self, Raw};

type Job = Box<dyn FnOnce() + Send>;

static JOBS: Mutex<Vec<Job>> = Mutex::new(Vec::new());
static READY: Condvar = Condvar::new();
static STARTED: std::sync::Once = std::sync::Once::new();

/// Run `f` on the decode pool (LIFO).
pub fn pool(f: impl FnOnce() + Send + 'static) {
    STARTED.call_once(|| {
        for i in 0..2 {
            crate::util::spawn(&format!("decode-{i}"), || loop {
                let job = {
                    let mut q = JOBS.lock().unwrap_or_else(|e| e.into_inner());
                    while q.is_empty() {
                        q = READY.wait(q).unwrap_or_else(|e| e.into_inner());
                    }
                    q.pop()
                };
                if let Some(j) = job {
                    j();
                }
            });
        }
    });
    let mut q = JOBS.lock().unwrap_or_else(|e| e.into_inner());
    q.push(Box::new(f));
    drop(q);
    READY.notify_one();
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    /// Aspect-fill tiles: shorter side ≈ px.
    Tile,
    /// Longest side ≤ px.
    Fit,
}

struct Entry {
    bmp: ID2D1Bitmap1,
    used: u64,
    generation_: u32,
}

pub struct Thumbs {
    map: HashMap<String, Entry>,
    pending: HashSet<String>,
    failed: HashSet<String>,
    tick: u64,
    limit: usize,
}

impl Thumbs {
    pub fn new() -> Thumbs {
        Thumbs { map: HashMap::new(), pending: HashSet::new(), failed: HashSet::new(), tick: 0, limit: 260 }
    }

    pub fn key(id: &str, px: u32, kind: Kind) -> String {
        format!("{id}@{px}{}", if kind == Kind::Tile { "t" } else { "f" })
    }

    /// Cached bitmap, or None while it loads (`on_ready` fires on the UI thread).
    pub fn get(&mut self, id: &str, path: PathBuf, px: u32, kind: Kind) -> Option<ID2D1Bitmap1> {
        let key = Self::key(id, px, kind);
        self.tick += 1;
        let generation_ = crate::gfx::generation();
        if let Some(e) = self.map.get_mut(&key) {
            if e.generation_ == generation_ {
                e.used = self.tick;
                return Some(e.bmp.clone());
            }
        }
        if self.pending.contains(&key) || self.failed.contains(&key) {
            return None;
        }
        self.pending.insert(key.clone());
        pool(move || {
            let raw = match kind {
                Kind::Tile => imaging::tile_thumbnail(&path, px),
                Kind::Fit => imaging::thumbnail(&path, px),
            };
            crate::ui::post(move || crate::app::thumb_ready(key, raw));
        });
        None
    }

    /// Same as `get` but never schedules work.
    pub fn peek(&mut self, id: &str, px: u32, kind: Kind) -> Option<ID2D1Bitmap1> {
        let key = Self::key(id, px, kind);
        let generation_ = crate::gfx::generation();
        self.tick += 1;
        let e = self.map.get_mut(&key)?;
        if e.generation_ != generation_ {
            return None;
        }
        e.used = self.tick;
        Some(e.bmp.clone())
    }

    pub fn insert(&mut self, key: String, raw: Option<Raw>) {
        self.pending.remove(&key);
        let Some(raw) = raw else {
            self.failed.insert(key);
            return;
        };
        let g = crate::gfx::gfx();
        let Some(bmp) = g.bitmap(raw.w, raw.h, &raw.px) else {
            self.failed.insert(key);
            return;
        };
        self.tick += 1;
        self.map.insert(key, Entry { bmp, used: self.tick, generation_: g.generation });
        if self.map.len() > self.limit {
            let mut v: Vec<(u64, String)> = self.map.iter().map(|(k, e)| (e.used, k.clone())).collect();
            v.sort_unstable();
            for (_, k) in v.into_iter().take(self.map.len() - self.limit) {
                self.map.remove(&k);
            }
        }
    }

    /// Forget an item's thumbnails (deleted or replaced).
    pub fn forget(&mut self, id: &str) {
        let prefix = format!("{id}@");
        self.map.retain(|k, _| !k.starts_with(&prefix));
        self.failed.retain(|k| !k.starts_with(&prefix));
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.pending.clear();
        self.failed.clear();
    }
}
