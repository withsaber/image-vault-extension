//! Turns whatever arrives (drop, clipboard) into saved images — Ingest.swift.
//! Order: image files → virtual files (Chrome, Edge, Outlook) → raw image
//! data → web URLs.

use std::path::PathBuf;

use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Ole::*;
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

use crate::clipboard::format_id;
use crate::imaging::{self, Raw};
use crate::vault::{self, AddResult, Meta, VaultItem};

#[derive(Debug, Clone)]
pub struct Candidate {
    pub data: Vec<u8>,
    pub name: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Default)]
pub struct Summary {
    pub added: Vec<VaultItem>,
    pub duplicates: Vec<VaultItem>,
    pub failed: usize,
    /// 88 px activity thumbnail.
    pub first: Option<Raw>,
}

impl Summary {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.duplicates.is_empty()
    }
}

/// Everything a drop can give us, captured while the drop is alive.
#[derive(Debug, Default)]
pub struct Payload {
    pub files: Vec<PathBuf>,
    pub virtual_files: Vec<Candidate>,
    pub data: Vec<Candidate>,
    pub urls: Vec<String>,
}

fn fmt(cf: u32, tymed: TYMED, lindex: i32) -> FORMATETC {
    FORMATETC { cfFormat: cf as u16, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex, tymed: tymed.0 as u32 }
}

fn has(d: &IDataObject, cf: u32) -> bool {
    unsafe { d.QueryGetData(&fmt(cf, TYMED_HGLOBAL, -1)) == S_OK }
}

fn medium_bytes(m: &STGMEDIUM) -> Option<Vec<u8>> {
    unsafe {
        if m.tymed == TYMED_HGLOBAL.0 as u32 {
            let h = m.u.hGlobal;
            let size = GlobalSize(h);
            let p = GlobalLock(h);
            if p.is_null() {
                return None;
            }
            let v = std::slice::from_raw_parts(p as *const u8, size).to_vec();
            let _ = GlobalUnlock(h);
            return Some(v);
        }
        if m.tymed == TYMED_ISTREAM.0 as u32 {
            let s: Option<&IStream> = m.u.pstm.as_ref();
            let s = s?;
            let mut out = Vec::new();
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let mut read = 0u32;
                let hr = s.Read(buf.as_mut_ptr() as *mut _, buf.len() as u32, Some(&mut read));
                if hr.is_err() || read == 0 {
                    break;
                }
                out.extend_from_slice(&buf[..read as usize]);
                if out.len() > 512 * 1024 * 1024 {
                    break;
                }
            }
            return Some(out);
        }
        None
    }
}

fn get(d: &IDataObject, cf: u32, lindex: i32, tymed: TYMED) -> Option<Vec<u8>> {
    unsafe {
        let f = fmt(cf, tymed, lindex);
        let mut m = d.GetData(&f).ok()?;
        let out = medium_bytes(&m);
        ReleaseStgMedium(&mut m);
        out
    }
}

pub fn get_bytes(d: &IDataObject, name: &str) -> Option<Vec<u8>> {
    get(d, format_id(name), -1, TYMED_HGLOBAL)
}

fn utf16(b: &[u8]) -> String {
    let w: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    crate::util::from_wide(&w)
}

pub fn drop_files(d: &IDataObject) -> Vec<PathBuf> {
    let mut out = Vec::new();
    unsafe {
        let f = fmt(CF_HDROP.0 as u32, TYMED_HGLOBAL, -1);
        if let Ok(mut m) = d.GetData(&f) {
            let drop = HDROP(m.u.hGlobal.0);
            let n = DragQueryFileW(drop, u32::MAX, None);
            for i in 0..n {
                let len = DragQueryFileW(drop, i, None) as usize;
                let mut buf = vec![0u16; len + 1];
                DragQueryFileW(drop, i, Some(&mut buf));
                out.push(PathBuf::from(crate::util::from_wide(&buf)));
            }
            ReleaseStgMedium(&mut m);
        }
    }
    out
}

/// FILEGROUPDESCRIPTORW names.
fn descriptor_names(d: &IDataObject) -> Vec<String> {
    let Some(b) = get_bytes(d, "FileGroupDescriptorW") else { return Vec::new() };
    if b.len() < 4 {
        return Vec::new();
    }
    let count = u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize;
    let mut names = Vec::new();
    for i in 0..count.min(256) {
        let off = 4 + i * 592 + 72;
        if off + 520 > b.len() {
            break;
        }
        names.push(utf16(&b[off..off + 520]));
    }
    names
}

fn text_url(d: &IDataObject) -> Option<String> {
    for name in ["UniformResourceLocatorW"] {
        if let Some(b) = get_bytes(d, name) {
            let s = utf16(&b).trim().to_string();
            if s.starts_with("http") {
                return Some(s);
            }
        }
    }
    if let Some(b) = get(d, CF_UNICODETEXT.0 as u32, -1, TYMED_HGLOBAL) {
        let s = utf16(&b).trim().to_string();
        if (s.starts_with("http://") || s.starts_with("https://")) && !s.contains(char::is_whitespace) {
            return Some(s);
        }
    }
    None
}

fn html_img(d: &IDataObject) -> Option<String> {
    let b = get_bytes(d, "HTML Format")?;
    let html = String::from_utf8_lossy(&b).to_string();
    let (_, img) = crate::clipboard::parse_html(&html);
    img.filter(|u| u.starts_with("http"))
}

fn image_data(d: &IDataObject) -> Option<Vec<u8>> {
    if let Some(b) = get_bytes(d, "image/svg+xml") {
        let b: Vec<u8> = b.into_iter().take_while(|c| *c != 0).collect();
        if imaging::is_svg(&b) {
            return Some(b);
        }
    }
    for name in ["GIF", "image/gif", "PNG", "image/png", "JFIF", "image/jpeg", "image/webp"] {
        if let Some(b) = get_bytes(d, name) {
            if !b.is_empty() {
                return Some(b);
            }
        }
    }
    for cf in [CF_DIBV5.0 as u32, CF_DIB.0 as u32] {
        if let Some(dib) = get(d, cf, -1, TYMED_HGLOBAL) {
            if let Some(raw) = imaging::from_dib(&dib) {
                return imaging::encode_png(&raw, None);
            }
        }
    }
    None
}

/// Quick check while a drag hovers (Ingest.looksLikeImage).
pub fn looks_like_image(d: &IDataObject) -> bool {
    if crate::dragdrop::is_ours(d) {
        return false;
    }
    if has(d, CF_HDROP.0 as u32) {
        return drop_files(d).iter().any(|p| imaging::is_image_path(p));
    }
    if has(d, format_id("FileGroupDescriptorW")) {
        let names = descriptor_names(d);
        if names.iter().any(|n| imaging::is_image_path(std::path::Path::new(n)) || !n.contains('.')) {
            return true;
        }
    }
    for name in ["PNG", "image/png", "JFIF", "image/jpeg", "image/gif", "GIF", "image/webp", "image/svg+xml"] {
        if has(d, format_id(name)) {
            return true;
        }
    }
    if has(d, CF_DIB.0 as u32) || has(d, CF_DIBV5.0 as u32) || has(d, CF_BITMAP.0 as u32) {
        return true;
    }
    if html_img(d).is_some() {
        return true;
    }
    text_url(d).is_some()
}

/// Must run synchronously from IDropTarget::Drop.
pub fn read_drop(d: &IDataObject) -> Payload {
    let mut p = Payload::default();
    // 1. Real image files.
    let files: Vec<PathBuf> = drop_files(d).into_iter().filter(|f| imaging::is_image_path(f)).collect();
    if !files.is_empty() {
        p.files = files;
        return p;
    }
    // 2. Virtual files (images dragged out of browsers / mail).
    let names = descriptor_names(d);
    if !names.is_empty() {
        let cf = format_id("FileContents");
        for (i, n) in names.iter().enumerate() {
            let path = std::path::Path::new(n);
            if path.extension().is_some() && !imaging::is_image_path(path) {
                continue;
            }
            let bytes = get(d, cf, i as i32, TYMED(TYMED_ISTREAM.0 | TYMED_HGLOBAL.0));
            if let Some(b) = bytes.filter(|b| !b.is_empty()) {
                p.virtual_files.push(Candidate { data: b, name: Some(n.clone()), url: None });
            }
        }
    }
    // 3. Raw image bytes.
    if let Some(b) = image_data(d) {
        p.data.push(Candidate { data: b, name: None, url: text_url(d) });
    }
    // 4. A web address pointing at an image.
    if let Some(u) = html_img(d) {
        p.urls.push(u);
    }
    if let Some(u) = text_url(d) {
        if !p.urls.contains(&u) {
            p.urls.push(u);
        }
    }
    p
}

pub fn download(url: &str) -> Option<Vec<u8>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(20)))
        .build()
        .into();
    let resp = agent
        .get(url)
        .header("Accept", "image/png,image/jpeg,image/gif;q=0.9,image/*;q=0.5,*/*;q=0.3")
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) ImageVault/2.2")
        .call()
        .ok()?;
    if !(200..300).contains(&resp.status().as_u16()) {
        return None;
    }
    resp.into_body().with_config().limit(256 * 1024 * 1024).read_to_vec().ok()
}

/// Resolves a payload (file reads, downloads) — worker thread.
pub fn candidates(p: Payload) -> Vec<Candidate> {
    if !p.files.is_empty() {
        let v: Vec<Candidate> = p
            .files
            .iter()
            .filter_map(|f| {
                std::fs::read(f).ok().map(|d| Candidate { data: d, name: f.file_name().map(|n| n.to_string_lossy().to_string()), url: None })
            })
            .collect();
        if !v.is_empty() {
            return v;
        }
    }
    if !p.virtual_files.is_empty() {
        return p.virtual_files;
    }
    if !p.data.is_empty() {
        return p.data;
    }
    let mut out = Vec::new();
    for u in &p.urls {
        if let Some(d) = download(u) {
            out.push(Candidate { data: d, name: vault::url_last_component(u), url: Some(u.clone()) });
        }
    }
    out
}

/// Store work only (any thread).
pub fn persist(cands: Vec<Candidate>, source: &str) -> Summary {
    let mut s = Summary::default();
    for c in cands {
        let meta = Meta {
            name: c.name.clone(),
            url: c.url.clone(),
            page_title: None,
            page_url: None,
            domain: c.url.as_deref().and_then(vault::url_host),
            source: source.to_string(),
        };
        match vault::add(&c.data, &meta) {
            Ok(AddResult::Added(i)) => s.added.push(i),
            Ok(AddResult::Duplicate(i)) => s.duplicates.push(i),
            Err(_) => s.failed += 1,
        }
    }
    if let Some(item) = s.added.first().or(s.duplicates.first()) {
        s.first = imaging::thumbnail(&vault::file_path(item), 88);
    }
    s
}

/// Persist on a worker, then call `done` on the UI thread.
pub fn save_async(cands: Vec<Candidate>, source: &'static str, done: impl FnOnce(Summary) + Send + 'static) {
    crate::util::spawn("ingest", move || {
        imaging::com_init();
        let s = persist(cands, source);
        crate::ui::post(move || done(s));
    });
}

/// Resolve a drop payload and save it.
pub fn import_drop(p: Payload, done: impl FnOnce(Summary) + Send + 'static) {
    crate::util::spawn("ingest-drop", move || {
        imaging::com_init();
        let c = candidates(p);
        let s = persist(c, "drop");
        crate::ui::post(move || done(s));
    });
}
