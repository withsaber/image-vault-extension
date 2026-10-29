//! Native popup menus in dark mode (the Mac app's NSMenu with .darkAqua),
//! built from closures. TrackPopupMenuEx is modal, so menus are always shown
//! from deferred work, and the chosen closure runs after the menu closes.

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCSTR, PCWSTR};

use crate::imaging::Raw;
use crate::util::wide;

pub enum Item {
    Action { title: String, key: String, checked: bool, enabled: bool, icon: Option<Raw>, run: Box<dyn FnOnce()> },
    Disabled(String),
    Sub { title: String, menu: Menu, icon: Option<Raw> },
    Sep,
}

#[derive(Default)]
pub struct Menu {
    pub items: Vec<Item>,
}

impl Menu {
    pub fn new() -> Menu {
        Menu { items: Vec::new() }
    }
    pub fn action(&mut self, title: impl Into<String>, run: impl FnOnce() + 'static) -> &mut Self {
        self.items.push(Item::Action { title: title.into(), key: String::new(), checked: false, enabled: true, icon: None, run: Box::new(run) });
        self
    }
    pub fn action_key(&mut self, title: impl Into<String>, key: &str, run: impl FnOnce() + 'static) -> &mut Self {
        self.items.push(Item::Action { title: title.into(), key: key.into(), checked: false, enabled: true, icon: None, run: Box::new(run) });
        self
    }
    pub fn toggle(&mut self, title: impl Into<String>, on: bool, run: impl FnOnce() + 'static) -> &mut Self {
        self.items.push(Item::Action { title: title.into(), key: String::new(), checked: on, enabled: true, icon: None, run: Box::new(run) });
        self
    }
    pub fn item(&mut self, title: impl Into<String>, key: &str, on: bool, icon: Option<Raw>, run: impl FnOnce() + 'static) -> &mut Self {
        self.items.push(Item::Action { title: title.into(), key: key.into(), checked: on, enabled: true, icon, run: Box::new(run) });
        self
    }
    pub fn disabled(&mut self, title: impl Into<String>) -> &mut Self {
        self.items.push(Item::Disabled(title.into()));
        self
    }
    pub fn sep(&mut self) -> &mut Self {
        self.items.push(Item::Sep);
        self
    }
    pub fn sub(&mut self, title: impl Into<String>, menu: Menu) -> &mut Self {
        self.items.push(Item::Sub { title: title.into(), menu, icon: None });
        self
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    /// Drop a trailing separator.
    pub fn trim(&mut self) {
        while matches!(self.items.last(), Some(Item::Sep)) {
            self.items.pop();
        }
    }
}

/// Opt every menu of this process into the system dark theme (uxtheme's
/// SetPreferredAppMode / FlushMenuThemes, the same switch Explorer uses).
pub fn enable_dark() {
    unsafe {
        let Ok(ux) = LoadLibraryExW(windows::core::w!("uxtheme.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32) else { return };
        type SetMode = unsafe extern "system" fn(i32) -> i32;
        type Flush = unsafe extern "system" fn();
        if let Some(f) = GetProcAddress(ux, PCSTR(135 as *const u8)) {
            let f: SetMode = std::mem::transmute(f);
            f(2); // ForceDark
        }
        if let Some(f) = GetProcAddress(ux, PCSTR(136 as *const u8)) {
            let f: Flush = std::mem::transmute(f);
            f();
        }
    }
}

/// Allow dark mode for a window's non-client parts / menus it owns.
pub fn allow_dark(hwnd: HWND) {
    unsafe {
        let Ok(ux) = LoadLibraryExW(windows::core::w!("uxtheme.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32) else { return };
        type Allow = unsafe extern "system" fn(HWND, i32) -> i32;
        if let Some(f) = GetProcAddress(ux, PCSTR(133 as *const u8)) {
            let f: Allow = std::mem::transmute(f);
            f(hwnd, 1);
        }
        let on: i32 = 1;
        let _ = windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(
            hwnd,
            windows::Win32::Graphics::Dwm::DWMWA_USE_IMMERSIVE_DARK_MODE,
            &on as *const _ as *const _,
            4,
        );
    }
}

/// 32-bit premultiplied DIB section for a menu item image.
fn hbitmap(raw: &Raw) -> Option<HBITMAP> {
    unsafe {
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: raw.w as i32,
                biHeight: -(raw.h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let h = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        if bits.is_null() {
            return None;
        }
        std::ptr::copy_nonoverlapping(raw.px.as_ptr(), bits as *mut u8, raw.px.len());
        Some(h)
    }
}

struct Built {
    runs: Vec<Option<Box<dyn FnOnce()>>>,
    bitmaps: Vec<HBITMAP>,
    menus: Vec<HMENU>,
}

fn build(menu: Menu, b: &mut Built) -> HMENU {
    unsafe {
        let h = CreatePopupMenu().unwrap();
        b.menus.push(h);
        for item in menu.items {
            match item {
                Item::Sep => {
                    let _ = AppendMenuW(h, MF_SEPARATOR, 0, PCWSTR::null());
                }
                Item::Disabled(t) => {
                    let w = wide(&crate::util::menu_escape(&t));
                    let _ = AppendMenuW(h, MF_STRING | MF_GRAYED, 0, PCWSTR(w.as_ptr()));
                }
                Item::Action { title, key, checked, enabled, icon, run } => {
                    let id = b.runs.len() + 1;
                    b.runs.push(Some(run));
                    let text = if key.is_empty() { crate::util::menu_escape(&title) } else { format!("{}\t{}", crate::util::menu_escape(&title), key) };
                    let w = wide(&text);
                    let mut flags = MF_STRING;
                    if checked {
                        flags |= MF_CHECKED;
                    }
                    if !enabled {
                        flags |= MF_GRAYED;
                    }
                    let _ = AppendMenuW(h, flags, id, PCWSTR(w.as_ptr()));
                    if let Some(raw) = icon.as_ref().and_then(hbitmap) {
                        b.bitmaps.push(raw);
                        let info = MENUITEMINFOW {
                            cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                            fMask: MIIM_BITMAP,
                            hbmpItem: raw,
                            ..Default::default()
                        };
                        let _ = SetMenuItemInfoW(h, id as u32, false, &info);
                    }
                }
                Item::Sub { title, menu, icon: _ } => {
                    let sub = build(menu, b);
                    let w = wide(&crate::util::menu_escape(&title));
                    let _ = AppendMenuW(h, MF_POPUP | MF_STRING, sub.0 as usize, PCWSTR(w.as_ptr()));
                }
            }
        }
        h
    }
}

/// Show the menu at a screen point (physical pixels). Blocks until it closes,
/// then runs the chosen item. Returns true if something was chosen.
pub fn popup(menu: Menu, owner: HWND, x: i32, y: i32) -> bool {
    let mut b = Built { runs: Vec::new(), bitmaps: Vec::new(), menus: Vec::new() };
    let h = build(menu, &mut b);
    let chosen = unsafe {
        let _ = SetForegroundWindow(owner);
        let r = TrackPopupMenuEx(h, (TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_LEFTALIGN | TPM_TOPALIGN).0, x, y, owner, None);
        let _ = PostMessageW(Some(owner), WM_NULL, WPARAM(0), LPARAM(0));
        r.0 as usize
    };
    unsafe {
        // Submenus are destroyed with their parent.
        let _ = DestroyMenu(h);
        for bm in &b.bitmaps {
            let _ = DeleteObject((*bm).into());
        }
    }
    if chosen > 0 && chosen <= b.runs.len() {
        if let Some(f) = b.runs[chosen - 1].take() {
            f();
            return true;
        }
    }
    false
}
