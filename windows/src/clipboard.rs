//! System clipboard: writing vault images (Clipboard.copy), text, files, and
//! reading what other apps copied (ClipboardWatcher / ClipboardHistory).

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Ole::{CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT};
use windows::Win32::UI::Shell::{DROPFILES, DragQueryFileW, HDROP};
use windows::core::PCWSTR;

use crate::imaging::{self, Raw};
use crate::util::wide;
use crate::vault::{self, VaultItem};

/// Our own writes carry this format so the watcher ignores them.
pub const MARKER: &str = "ImageVault.Internal";

pub fn format_id(name: &str) -> u32 {
    let w = wide(name);
    unsafe { RegisterClipboardFormatW(PCWSTR(w.as_ptr())) }
}

fn open() -> bool {
    for _ in 0..8 {
        unsafe {
            if OpenClipboard(Some(crate::ui::msg_hwnd())).is_ok() {
                return true;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    false
}

fn close() {
    unsafe {
        let _ = CloseClipboard();
    }
}

fn hglobal(bytes: &[u8]) -> Option<HGLOBAL> {
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)).ok()?;
        let p = GlobalLock(h);
        if p.is_null() {
            let _ = GlobalFree(Some(h));
            return None;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p as *mut u8, bytes.len());
        let _ = GlobalUnlock(h);
        Some(h)
    }
}

fn put(fmt: u32, bytes: &[u8]) {
    if let Some(h) = hglobal(bytes) {
        unsafe {
            if SetClipboardData(fmt, Some(HANDLE(h.0))).is_err() {
                let _ = GlobalFree(Some(h));
            }
        }
    }
}

fn utf16z(s: &str) -> Vec<u8> {
    let w = wide(s);
    w.iter().flat_map(|c| c.to_le_bytes()).collect()
}

fn dropfiles(paths: &[PathBuf]) -> Vec<u8> {
    let mut out = Vec::new();
    let header = std::mem::size_of::<DROPFILES>() as u32;
    out.extend_from_slice(&header.to_le_bytes()); // pFiles
    out.extend_from_slice(&0i32.to_le_bytes()); // pt.x
    out.extend_from_slice(&0i32.to_le_bytes()); // pt.y
    out.extend_from_slice(&0i32.to_le_bytes()); // fNC
    out.extend_from_slice(&1i32.to_le_bytes()); // fWide
    while out.len() < header as usize {
        out.push(0);
    }
    for p in paths {
        for c in crate::util::wide_path(p) {
            out.extend_from_slice(&c.to_le_bytes());
        }
    }
    out.extend_from_slice(&[0, 0]);
    out
}

/// Plain text (Copy Text, Copy Path). Like the Mac app, no marker — so it
/// also shows up in the clipboard history.
pub fn copy_text(text: &str) {
    if !open() {
        return;
    }
    unsafe {
        let _ = EmptyClipboard();
    }
    put(CF_UNICODETEXT.0 as u32, &utf16z(text));
    close();
}

/// Text written back from the history (marked as ours).
pub fn copy_text_marked(text: &str) {
    if !open() {
        return;
    }
    unsafe {
        let _ = EmptyClipboard();
    }
    put(CF_UNICODETEXT.0 as u32, &utf16z(text));
    put(format_id(MARKER), &[1]);
    close();
}

/// Real files (Explorer, Slack, WhatsApp, Outlook paste them all).
pub fn copy_files(paths: &[PathBuf]) {
    if paths.is_empty() || !open() {
        return;
    }
    unsafe {
        let _ = EmptyClipboard();
    }
    put(CF_HDROP.0 as u32, &dropfiles(paths));
    put(format_id("Preferred DropEffect"), &1u32.to_le_bytes());
    put(format_id(MARKER), &[1]);
    close();
}

/// One image as real image data (PNG + bitmap), several as files.
pub fn copy_items(items: &[VaultItem]) {
    if items.is_empty() {
        return;
    }
    if items.len() > 1 {
        let paths: Vec<PathBuf> = items.iter().map(vault::file_path).collect();
        copy_files(&paths);
        return;
    }
    let item = &items[0];
    let path = vault::file_path(item);
    let png = imaging::png_bytes(&path);
    let raw = imaging::decode_path(&path);
    let jpeg = if item.mime.as_deref() == Some("image/jpeg") { std::fs::read(&path).ok() } else { None };
    if !open() {
        return;
    }
    unsafe {
        let _ = EmptyClipboard();
    }
    if let Some(png) = png {
        put(format_id("PNG"), &png);
    }
    if let Some(raw) = raw {
        put(CF_DIBV5.0 as u32, &imaging::to_dibv5(&raw));
    }
    if let Some(j) = jpeg {
        put(format_id("JFIF"), &j);
    }
    put(format_id(MARKER), &[1]);
    close();
}

/// A rendered capture: PNG + bitmap, marked as ours.
pub fn copy_raw(raw: &Raw, dpi: f64) {
    let png = imaging::encode_png(raw, Some(dpi));
    if !open() {
        return;
    }
    unsafe {
        let _ = EmptyClipboard();
    }
    if let Some(png) = png {
        put(format_id("PNG"), &png);
    }
    put(CF_DIBV5.0 as u32, &imaging::to_dibv5(raw));
    put(format_id(MARKER), &[1]);
    close();
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// Snapshot of what's on the clipboard right now.
#[derive(Default, Debug)]
pub struct Snapshot {
    pub ours: bool,
    pub secret: bool,
    pub files: Vec<PathBuf>,
    pub text: Option<String>,
    pub image: Option<Vec<u8>>,
    pub url: Option<String>,
}

fn get_bytes(fmt: u32) -> Option<Vec<u8>> {
    unsafe {
        if IsClipboardFormatAvailable(fmt).is_err() {
            return None;
        }
        let h = GetClipboardData(fmt).ok()?;
        let g = HGLOBAL(h.0);
        let size = GlobalSize(g);
        let p = GlobalLock(g);
        if p.is_null() {
            return None;
        }
        let v = std::slice::from_raw_parts(p as *const u8, size).to_vec();
        let _ = GlobalUnlock(g);
        Some(v)
    }
}

fn utf16_bytes(b: &[u8]) -> String {
    let w: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    crate::util::from_wide(&w)
}

pub fn has(fmt: u32) -> bool {
    unsafe { IsClipboardFormatAvailable(fmt).is_ok() }
}

/// HTML Format → (SourceURL, first <img src>).
pub fn parse_html(html: &str) -> (Option<String>, Option<String>) {
    let source = html.lines().find_map(|l| l.strip_prefix("SourceURL:").map(|s| s.trim().to_string()));
    let lower = html.to_lowercase();
    let mut img = None;
    if let Some(i) = lower.find("<img") {
        let tag = &html[i..];
        let tl = &lower[i..];
        if let Some(j) = tl.find("src=") {
            let rest = &tag[j + 4..];
            let quote = rest.chars().next().unwrap_or('"');
            if quote == '"' || quote == '\'' {
                if let Some(end) = rest[1..].find(quote) {
                    let s = rest[1..1 + end].replace("&amp;", "&");
                    img = Some(s);
                }
            }
        }
    }
    (source, img)
}

pub fn read() -> Option<Snapshot> {
    if !open() {
        return None;
    }
    let mut s = Snapshot::default();
    s.ours = has(format_id(MARKER));
    // Password managers and history-excluded content.
    s.secret = has(format_id("ExcludeClipboardContentFromMonitorProcessing"))
        || has(format_id("Clipboard Viewer Ignore"))
        || get_bytes(format_id("CanIncludeInClipboardHistory")).map(|b| b.len() >= 4 && b[..4] == [0, 0, 0, 0]).unwrap_or(false);
    if !s.ours && !s.secret {
        if has(CF_HDROP.0 as u32) {
            unsafe {
                if let Ok(h) = GetClipboardData(CF_HDROP.0 as u32) {
                    let drop = HDROP(h.0);
                    let n = DragQueryFileW(drop, u32::MAX, None);
                    for i in 0..n {
                        let len = DragQueryFileW(drop, i, None) as usize;
                        let mut buf = vec![0u16; len + 1];
                        DragQueryFileW(drop, i, Some(&mut buf));
                        s.files.push(PathBuf::from(crate::util::from_wide(&buf)));
                    }
                }
            }
        }
        if let Some(b) = get_bytes(CF_UNICODETEXT.0 as u32) {
            s.text = Some(utf16_bytes(&b));
        }
        for name in ["PNG", "image/png"] {
            if s.image.is_none() {
                s.image = get_bytes(format_id(name));
            }
        }
        for name in ["JFIF", "image/jpeg", "image/gif", "image/webp"] {
            if s.image.is_none() {
                s.image = get_bytes(format_id(name));
            }
        }
        if s.image.is_none() {
            let dib = get_bytes(CF_DIBV5.0 as u32).or_else(|| get_bytes(CF_DIB.0 as u32));
            if let Some(dib) = dib {
                if let Some(raw) = imaging::from_dib(&dib) {
                    s.image = imaging::encode_png(&raw, None);
                }
            }
        }
        if let Some(h) = get_bytes(format_id("HTML Format")) {
            let html = String::from_utf8_lossy(&h).to_string();
            let (_src, img) = parse_html(&html);
            s.url = img.filter(|u| u.starts_with("http"));
        }
    }
    close();
    Some(s)
}

/// An image on the clipboard (editor "Paste Image").
pub fn read_image() -> Option<Raw> {
    if !open() {
        return None;
    }
    let mut out = None;
    for name in ["PNG", "image/png", "JFIF"] {
        if out.is_none() {
            out = get_bytes(format_id(name)).and_then(|b| imaging::decode_bytes(&b, None));
        }
    }
    if out.is_none() {
        out = get_bytes(CF_DIBV5.0 as u32).or_else(|| get_bytes(CF_DIB.0 as u32)).and_then(|d| imaging::from_dib(&d));
    }
    close();
    out
}

/// Display name of the app in front (NSWorkspace.frontmostApplication.localizedName).
pub fn front_app_name() -> Option<String> {
    use windows::Win32::UI::WindowsAndMessaging::*;
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == std::process::id() {
            return Some("Image Vault".into());
        }
        process_name(pid)
    }
}

pub fn process_path(pid: u32) -> Option<PathBuf> {
    use windows::Win32::System::Threading::*;
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return None;
        }
        Some(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
    }
}

pub fn process_name(pid: u32) -> Option<String> {
    let path = process_path(pid)?;
    Some(file_description(&path).unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()))
}

/// FileDescription from the version resource ("Google Chrome").
pub fn file_description(path: &Path) -> Option<String> {
    use windows::Win32::Storage::FileSystem::*;
    unsafe {
        let w = crate::util::wide_path(path);
        let size = GetFileVersionInfoSizeW(PCWSTR(w.as_ptr()), None);
        if size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        GetFileVersionInfoW(PCWSTR(w.as_ptr()), None, size, buf.as_mut_ptr() as *mut _).ok()?;
        let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let q = wide("\\VarFileInfo\\Translation");
        if !VerQueryValueW(buf.as_ptr() as *const _, PCWSTR(q.as_ptr()), &mut ptr, &mut len).as_bool() || len < 4 {
            return None;
        }
        let lang = *(ptr as *const u16);
        let cp = *((ptr as *const u16).add(1));
        let q = wide(&format!("\\StringFileInfo\\{lang:04x}{cp:04x}\\FileDescription"));
        if !VerQueryValueW(buf.as_ptr() as *const _, PCWSTR(q.as_ptr()), &mut ptr, &mut len).as_bool() || len == 0 {
            return None;
        }
        let s = std::slice::from_raw_parts(ptr as *const u16, len as usize);
        let s = crate::util::from_wide(s).trim().to_string();
        if s.is_empty() { None } else { Some(s) }
    }
}
