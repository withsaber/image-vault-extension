//! Small helpers: wide strings, time, logging, shell.

use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use windows::Win32::Foundation::*;
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PCWSTR;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn wide_path(p: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

pub fn from_wide(w: &[u16]) -> String {
    let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

pub fn now_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0)
}

pub fn now_secs() -> f64 {
    now_ms() / 1000.0
}

/// Monotonic seconds (QueryPerformanceCounter).
pub fn mono() -> f64 {
    use windows::Win32::System::Performance::*;
    unsafe {
        let mut f = 0i64;
        let mut c = 0i64;
        let _ = QueryPerformanceFrequency(&mut f);
        let _ = QueryPerformanceCounter(&mut c);
        c as f64 / f.max(1) as f64
    }
}

/// `yyyy-MM-dd 'at' HH.mm.ss` in local time.
pub fn stamp() -> String {
    unsafe {
        let t = GetLocalTime();
        format!("{:04}-{:02}-{:02} at {:02}.{:02}.{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
    }
}

/// DateFormatter.localizedString(dateStyle: .medium, timeStyle: .short),
/// using the user's locale through GetDateFormatEx / GetTimeFormatEx.
pub fn medium_date(ms: f64) -> String {
    use windows::Win32::Globalization::*;
    use windows::Win32::System::Time::*;
    unsafe {
        let ft_ticks = ((ms / 1000.0) as i64 + 11644473600) * 10_000_000;
        let ft = FILETIME { dwLowDateTime: ft_ticks as u32, dwHighDateTime: (ft_ticks >> 32) as u32 };
        let mut utc = SYSTEMTIME::default();
        if FileTimeToSystemTime(&ft, &mut utc).is_err() {
            return String::new();
        }
        let mut local = SYSTEMTIME::default();
        if SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_err() {
            local = utc;
        }
        let mut d = [0u16; 128];
        let fmt = wide("MMM d, yyyy");
        let n = GetDateFormatEx(PCWSTR::null(), ENUM_DATE_FORMATS_FLAGS(0), Some(&local), PCWSTR(fmt.as_ptr()), Some(&mut d), PCWSTR::null());
        let mut t = [0u16; 64];
        let m = GetTimeFormatEx(PCWSTR::null(), TIME_NOSECONDS, Some(&local), PCWSTR::null(), Some(&mut t));
        let ds = if n > 0 { from_wide(&d) } else { String::new() };
        let ts = if m > 0 { from_wide(&t) } else { String::new() };
        format!("{ds} at {ts}")
    }
}

pub fn log(msg: &str) {
    let path = crate::vault::root().join("error.log");
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > 512 * 1024 {
            let _ = std::fs::rename(&path, path.with_extension("log.1"));
        }
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "[{}] {}", stamp(), msg);
    }
}

pub fn shell_open(path: &str) {
    unsafe {
        let p = wide(path);
        ShellExecuteW(None, windows::core::w!("open"), PCWSTR(p.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

/// Explorer with these files selected (NSWorkspace.activateFileViewerSelecting).
pub fn reveal(paths: &[std::path::PathBuf]) {
    if paths.is_empty() {
        return;
    }
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
        // Group by folder; SHOpenFolderAndSelectItems selects many in one window.
        let parent = paths[0].parent().map(|p| p.to_path_buf());
        let all_same = paths.iter().all(|p| p.parent().map(|q| q.to_path_buf()) == parent);
        if let (Some(parent), true) = (parent, all_same) {
            let pw = wide_path(&parent);
            let folder = ILCreateFromPathW(PCWSTR(pw.as_ptr()));
            if !folder.is_null() {
                let items: Vec<*const windows::Win32::UI::Shell::Common::ITEMIDLIST> = paths
                    .iter()
                    .map(|p| {
                        let w = wide_path(p);
                        ILCreateFromPathW(PCWSTR(w.as_ptr())) as *const _
                    })
                    .filter(|p: &*const windows::Win32::UI::Shell::Common::ITEMIDLIST| !p.is_null())
                    .collect();
                let _ = SHOpenFolderAndSelectItems(folder, Some(&items), 0);
                for it in items {
                    ILFree(Some(it));
                }
                ILFree(Some(folder));
                return;
            }
        }
        // Different folders: one Explorer window per file's folder.
        for p in paths {
            let args = wide(&format!("/select,\"{}\"", p.display()));
            ShellExecuteW(None, windows::core::w!("open"), windows::core::w!("explorer.exe"), PCWSTR(args.as_ptr()), PCWSTR::null(), SW_SHOWNORMAL);
        }
    }
}

pub fn beep() {
    unsafe {
        let _ = windows::Win32::System::Diagnostics::Debug::MessageBeep(MB_OK);
    }
}

/// Run a closure on a fresh background thread with low priority.
pub fn spawn_low(name: &str, f: impl FnOnce() + Send + 'static) {
    let _ = std::thread::Builder::new().name(name.to_string()).spawn(move || {
        unsafe {
            use windows::Win32::System::Threading::*;
            let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
        }
        f()
    });
}

pub fn spawn(name: &str, f: impl FnOnce() + Send + 'static) {
    let _ = std::thread::Builder::new().name(name.to_string()).spawn(f);
}

pub fn exe_dir() -> std::path::PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())).unwrap_or_default()
}

/// Escape for menu text (& is the mnemonic prefix).
pub fn menu_escape(s: &str) -> String {
    s.replace('&', "&&")
}

pub fn lparam_xy(l: LPARAM) -> (i32, i32) {
    let x = (l.0 & 0xffff) as i16 as i32;
    let y = ((l.0 >> 16) & 0xffff) as i16 as i32;
    (x, y)
}

/// Debug trace (IV_TRACE=1): appends to trace.log in the vault root.
pub fn trace(msg: &str) {
    use std::sync::OnceLock;
    static ON: OnceLock<bool> = OnceLock::new();
    if !*ON.get_or_init(|| std::env::var("IV_TRACE").map(|v| v == "1").unwrap_or(false)) {
        return;
    }
    let path = crate::vault::root().join("trace.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{:.3} {}", mono(), msg);
    }
}
