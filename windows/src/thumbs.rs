//! Thumbnail cache (Thumbnails.swift): memory first, then small files in
//! `thumbs\`, then the original. Decoding runs on a small LIFO worker pool
//! (most recent request first, so scrolling stays snappy); the UI thread
//! uploads results as GPU bitmaps and keeps them under a byte budget.
//!
//! Grid tiles are ~96 DIP, so 256 px covers them at 200% scaling; the disk
//! copies (`thumbs\<id>@<px>-<hash8>.jpg|png`) make the tray open instantly
//! after a relaunch instead of decoding every original.

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

/// Grid thumbnails: longest side in pixels.
pub const GRID_PX: u32 = 256;
/// Memory budget for decoded thumbnails (bytes), and a cap on their count.
const BUDGET: usize = 96 * 1024 * 1024;
const MAX_COUNT: usize = 1000;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    /// Aspect-fill tiles: shorter side ≈ px.
    Tile,
    /// Longest side ≤ px.
    Fit,
}

struct Entry {
    bmp: ID2D1Bitmap1,
    bytes: usize,
    used: u64,
    generation_: u32,
}

struct Gif {
    frames: Vec<ID2D1Bitmap1>,
    /// Frame start times (seconds into the loop) and the loop length.
    starts: Vec<f32>,
    total: f32,
    used: u64,
    generation_: u32,
}

/// Decoded GIF frames, ready to upload on the UI thread.
pub type GifFrames = Option<(Vec<Raw>, Vec<f32>)>;

pub struct Thumbs {
    map: HashMap<String, Entry>,
    gifs: HashMap<String, Gif>,
    /// Keys that turned out to be stills (or failed): use the plain thumbnail.
    gif_still: HashSet<String>,
    gif_pending: HashSet<String>,
    pending: HashSet<String>,
    failed: HashSet<String>,
    tick: u64,
    /// Bytes held by `map`.
    total: usize,
}

impl Thumbs {
    pub fn new() -> Thumbs {
        Thumbs {
            map: HashMap::new(),
            gifs: HashMap::new(),
            gif_still: HashSet::new(),
            gif_pending: HashSet::new(),
            pending: HashSet::new(),
            failed: HashSet::new(),
            tick: 0,
            total: 0,
        }
    }

    /// The content hash in the key means an edited image never shows a stale thumbnail.
    pub fn key(id: &str, px: u32, kind: Kind, hash: Option<&str>) -> String {
        let h: String = hash.unwrap_or("x").chars().take(8).collect();
        format!("{id}@{px}{}-{h}", if kind == Kind::Tile { "t" } else { "f" })
    }

    /// Cached bitmap, or None while it loads (`thumb_ready` fires on the UI thread).
    pub fn get(&mut self, id: &str, path: PathBuf, hash: Option<&str>, px: u32, kind: Kind) -> Option<ID2D1Bitmap1> {
        let key = Self::key(id, px, kind, hash);
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
        let stem = crate::vault::thumb_stem(id, px, hash);
        pool(move || {
            imaging::com_init();
            let raw = cached(&stem).or_else(|| {
                let raw = match kind {
                    Kind::Tile => imaging::thumbnail(&path, px),
                    Kind::Fit => imaging::thumbnail(&path, px),
                };
                if let Some(r) = &raw {
                    write_cached(r, &stem);
                }
                raw
            });
            crate::ui::post(move || crate::app::thumb_ready(key, raw));
        });
        None
    }

    /// Same as `get` but never schedules work.
    pub fn peek(&mut self, id: &str, hash: Option<&str>, px: u32, kind: Kind) -> Option<ID2D1Bitmap1> {
        let key = Self::key(id, px, kind, hash);
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
        let bytes = (raw.w * raw.h * 4) as usize;
        self.total += bytes;
        if let Some(old) = self.map.insert(key, Entry { bmp, bytes, used: self.tick, generation_: g.generation }) {
            self.total -= old.bytes;
        }
        // Over budget: drop the least recently used until it fits.
        if self.total > BUDGET || self.map.len() > MAX_COUNT {
            let mut v: Vec<(u64, String)> = self.map.iter().map(|(k, e)| (e.used, k.clone())).collect();
            v.sort_unstable();
            for (_, k) in v {
                if self.total <= BUDGET * 9 / 10 && self.map.len() <= MAX_COUNT {
                    break;
                }
                if let Some(e) = self.map.remove(&k) {
                    self.total -= e.bytes;
                }
            }
        }
    }

    /// Bytes of decoded thumbnails held right now.
    pub fn bytes(&self) -> usize {
        self.total
    }

    /// The frame of an animated GIF to show at `now` (seconds, any clock) and
    /// how long until the next one. None while decoding or for stills, where
    /// the caller shows the plain thumbnail.
    pub fn gif_frame(&mut self, id: &str, path: PathBuf, side: u32, now: f64) -> Option<(ID2D1Bitmap1, f32)> {
        let key = format!("{id}@{side}g");
        self.tick += 1;
        let generation_ = crate::gfx::generation();
        if let Some(g) = self.gifs.get_mut(&key) {
            if g.generation_ == generation_ {
                g.used = self.tick;
                let t = (now % g.total as f64) as f32;
                let i = g.starts.partition_point(|s| *s <= t).saturating_sub(1);
                let next = g.starts.get(i + 1).copied().unwrap_or(g.total);
                return Some((g.frames[i].clone(), (next - t).max(0.01)));
            }
            self.gifs.remove(&key);
        }
        if self.gif_still.contains(&key) || self.gif_pending.contains(&key) {
            return None;
        }
        self.gif_pending.insert(key.clone());
        pool(move || {
            let frames = imaging::gif_frames(&path, side);
            crate::ui::post(move || crate::app::gif_ready(key, frames));
        });
        None
    }

    pub fn insert_gif(&mut self, key: String, frames: GifFrames) {
        self.gif_pending.remove(&key);
        let Some((raws, delays)) = frames else {
            self.gif_still.insert(key);
            return;
        };
        let g = crate::gfx::gfx();
        let bmps: Vec<ID2D1Bitmap1> = raws.iter().filter_map(|r| g.bitmap(r.w, r.h, &r.px)).collect();
        if bmps.len() != raws.len() || bmps.is_empty() {
            self.gif_still.insert(key);
            return;
        }
        let mut starts = Vec::with_capacity(delays.len());
        let mut t = 0.0f32;
        for d in &delays {
            starts.push(t);
            t += d;
        }
        self.tick += 1;
        self.gifs.insert(key, Gif { frames: bmps, starts, total: t.max(0.02), used: self.tick, generation_: g.generation });
        // Each GIF holds up to 400 frames: keep only the most recent few.
        const GIF_LIMIT: usize = 16;
        if self.gifs.len() > GIF_LIMIT {
            let mut v: Vec<(u64, String)> = self.gifs.iter().map(|(k, e)| (e.used, k.clone())).collect();
            v.sort_unstable();
            for (_, k) in v.into_iter().take(self.gifs.len() - GIF_LIMIT) {
                self.gifs.remove(&k);
            }
        }
    }

    /// Forget an item's thumbnails (deleted or replaced).
    pub fn forget(&mut self, id: &str) {
        let prefix = format!("{id}@");
        let mut freed = 0;
        self.map.retain(|k, e| {
            let keep = !k.starts_with(&prefix);
            if !keep {
                freed += e.bytes;
            }
            keep
        });
        self.total -= freed;
        self.failed.retain(|k| !k.starts_with(&prefix));
        self.gifs.retain(|k, _| !k.starts_with(&prefix));
        self.gif_still.retain(|k| !k.starts_with(&prefix));
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.total = 0;
        self.pending.clear();
        self.failed.clear();
        self.gifs.clear();
        self.gif_pending.clear();
        self.gif_still.clear();
    }
}

/// A thumbnail saved earlier (`<stem>.jpg` or `.png`).
fn cached(stem: &std::path::Path) -> Option<Raw> {
    ["jpg", "png"].iter().find_map(|ext| {
        let p = stem.with_extension(ext);
        if p.exists() { imaging::decode_path(&p) } else { None }
    })
}

/// JPEG when the thumbnail has no transparency (small), PNG otherwise.
fn write_cached(raw: &Raw, stem: &std::path::Path) {
    let opaque = raw.px.chunks_exact(4).all(|p| p[3] == 255);
    let (ext, data) = if opaque { ("jpg", imaging::encode_jpeg(raw, 0.85)) } else { ("png", imaging::encode_png(raw, None)) };
    if let Some(d) = data {
        let path = stem.with_extension(ext);
        let tmp = stem.with_extension(format!("{ext}.tmp"));
        if std::fs::write(&tmp, d).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}
