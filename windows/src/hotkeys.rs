//! Global shortcuts (Carbon HotKey in the Mac app) through RegisterHotKey,
//! plus the "Set Shortcut" capture panel.

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

use crate::app::{self, App};
use crate::gfx::{Align, Surface, Trim};
use crate::island;
use crate::theme::{Color, Font, Rect, Weight, rect};
use crate::ui;

pub const TOGGLE: i32 = 1;
pub const COPY: i32 = 2;
pub const OCR: i32 = 3;
pub const SHOT: i32 = 4;

const MOD_ALT_: u32 = 0x1;
const MOD_CONTROL_: u32 = 0x2;
const MOD_SHIFT_: u32 = 0x4;
const MOD_WIN_: u32 = 0x8;
const MOD_NOREPEAT_: u32 = 0x4000;

#[derive(Default)]
pub struct Hotkeys {
    registered: [bool; 5],
    paused: [bool; 5],
}

pub fn name(id: i32) -> &'static str {
    match id {
        OCR => "OCR",
        SHOT => "Screenshot",
        TOGGLE => "Island",
        _ => "Copy Latest",
    }
}

fn default_code(id: i32) -> u32 {
    match id {
        TOGGLE => b'V' as u32,
        COPY => b'B' as u32,
        OCR => b'T' as u32,
        _ => b'S' as u32,
    }
}

/// (virtual key, MOD_* flags) for a shortcut.
pub fn combo(app: &App, id: i32) -> (u32, u32) {
    let s = &app.settings;
    let (code, mods) = match id {
        OCR => (s.ocr_hot_key_code, s.ocr_hot_key_modifiers),
        SHOT => (s.shot_hot_key_code, s.shot_hot_key_modifiers),
        _ => (0, 0),
    };
    (if code != 0 { code } else { default_code(id) }, if mods != 0 { mods } else { MOD_CONTROL_ | MOD_ALT_ })
}

pub fn mods_label(m: u32) -> String {
    let mut s = String::new();
    if m & MOD_CONTROL_ != 0 {
        s.push_str("Ctrl+");
    }
    if m & MOD_ALT_ != 0 {
        s.push_str("Alt+");
    }
    if m & MOD_SHIFT_ != 0 {
        s.push_str("Shift+");
    }
    if m & MOD_WIN_ != 0 {
        s.push_str("Win+");
    }
    s
}

pub fn key_label(vk: u32) -> String {
    match vk {
        0x41..=0x5A | 0x30..=0x39 => char::from_u32(vk).map(|c| c.to_string()).unwrap_or_default(),
        0x20 => "Space".into(),
        0x09 => "Tab".into(),
        0x0D => "Enter".into(),
        0x1B => "Esc".into(),
        0x08 => "Backspace".into(),
        0x2E => "Delete".into(),
        0x25 => "←".into(),
        0x27 => "→".into(),
        0x26 => "↑".into(),
        0x28 => "↓".into(),
        0x70..=0x7B => format!("F{}", vk - 0x6F),
        _ => unsafe {
            let sc = MapVirtualKeyW(vk, MAPVK_VK_TO_VSC);
            let mut buf = [0u16; 32];
            let n = GetKeyNameTextW(((sc as i32) << 16) as i32, &mut buf);
            if n > 0 { String::from_utf16_lossy(&buf[..n as usize]) } else { format!("Key({vk})") }
        },
    }
}

pub fn label(app: &App, id: i32) -> String {
    let (c, m) = combo(app, id);
    format!("{}{}", mods_label(m), key_label(c))
}

pub fn default_label(id: i32) -> String {
    format!("Ctrl+Alt+{}", key_label(default_code(id)))
}

pub fn registered(app: &App, id: i32) -> bool {
    app.hotkeys.registered[id as usize]
}

fn register(app: &mut App, id: i32) {
    let (c, m) = combo(app, id);
    unsafe {
        let _ = UnregisterHotKey(Some(ui::msg_hwnd()), id);
        let ok = RegisterHotKey(Some(ui::msg_hwnd()), id, HOT_KEY_MODIFIERS(m | MOD_NOREPEAT_), c).is_ok();
        app.hotkeys.registered[id as usize] = ok;
    }
}

fn unregister(app: &mut App, id: i32) {
    unsafe {
        let _ = UnregisterHotKey(Some(ui::msg_hwnd()), id);
    }
    app.hotkeys.registered[id as usize] = false;
}

pub fn register_all(app: &mut App) {
    for id in [TOGGLE, COPY, OCR, SHOT] {
        register(app, id);
    }
}

pub fn unregister_all(app: &mut App) {
    for id in [TOGGLE, COPY, OCR, SHOT] {
        unregister(app, id);
    }
}

pub fn fired(id: i32) {
    match id {
        TOGGLE => {
            app::with(island::toggle);
        }
        COPY => {
            app::with(app::copy_latest);
        }
        OCR => crate::screenocr::toggle_ocr(),
        SHOT => crate::capture::take_screenshot(),
        other => {
            crate::screenocr::esc_hotkey(other);
        }
    }
}

pub fn reset(app: &mut App, id: i32) {
    match id {
        OCR => {
            app.settings.ocr_hot_key_code = 0;
            app.settings.ocr_hot_key_modifiers = 0;
        }
        SHOT => {
            app.settings.shot_hot_key_code = 0;
            app.settings.shot_hot_key_modifiers = 0;
        }
        _ => {}
    }
    app.settings.save();
    register(app, id);
    island::flash(app, &format!("{} shortcut reset to {}", name(id), default_label(id)));
}

// ---------------------------------------------------------------------------
// Capture panel (HotkeyCaptureView inside an alert)
// ---------------------------------------------------------------------------

struct Panel {
    hwnd: HWND,
    surface: Option<Surface>,
    scale: f32,
    label: String,
    captured: Option<(u32, u32)>,
    done: bool,
    cancel_hover: bool,
    id: i32,
}

thread_local! {
    static PANEL: std::cell::RefCell<Option<Panel>> = const { std::cell::RefCell::new(None) };
}

const W: f32 = 340.0;
const H: f32 = 176.0;

fn cur_mods() -> u32 {
    unsafe {
        let mut m = 0;
        if GetKeyState(VK_CONTROL.0 as i32) < 0 {
            m |= MOD_CONTROL_;
        }
        if GetKeyState(VK_MENU.0 as i32) < 0 {
            m |= MOD_ALT_;
        }
        if GetKeyState(VK_SHIFT.0 as i32) < 0 {
            m |= MOD_SHIFT_;
        }
        if GetKeyState(VK_LWIN.0 as i32) < 0 || GetKeyState(VK_RWIN.0 as i32) < 0 {
            m |= MOD_WIN_;
        }
        m
    }
}

fn cancel_rect() -> Rect {
    rect(W - 20.0 - 84.0, H - 20.0 - 28.0, 84.0, 28.0)
}

fn paint() {
    PANEL.with(|p| {
        let mut b = p.borrow_mut();
        let Some(panel) = b.as_mut() else { return };
        if panel.surface.is_none() {
            let (w, h) = ((W * panel.scale) as u32, (H * panel.scale) as u32);
            panel.surface = Surface::new(panel.hwnd, w, h, panel.scale).ok();
        }
        let title = format!("Set {} Shortcut", name(panel.id));
        let info = format!("Press the key combination to use (e.g. {}). Escape to cancel.", default_label(panel.id));
        let label = panel.label.clone();
        let hover = panel.cancel_hover;
        if let Some(s) = &mut panel.surface {
            s.draw(|cv| {
                let r = rect(0.0, 0.0, W, H);
                cv.fill_rect(r, Color::gray(0.13, 1.0));
                cv.text(&title, &Font::sans(13.0, Weight::Semibold), Color::white(0.95), rect(20.0, 18.0, W - 40.0, 18.0), Align::Leading, Trim::Tail);
                cv.text_wrapped(&info, &Font::sans(11.0, Weight::Regular), Color::white(0.6), rect(20.0, 42.0, W - 40.0, 30.0), 2, Align::Leading);
                let cap = rect((W - 260.0) / 2.0, 76.0, 260.0, 44.0);
                cv.fill_round(cap, 8.0, Color::white(0.08));
                cv.stroke_round(cap, 8.0, 1.0, Color::white(0.12));
                cv.text(&label, &Font::sans(13.0, Weight::Medium), Color::white(0.92), cap, Align::Center, Trim::Tail);
                let c = cancel_rect();
                cv.fill_round(c, 6.0, Color::white(if hover { 0.16 } else { 0.10 }));
                cv.text("Cancel", &Font::sans(12.0, Weight::Regular), Color::white(0.9), c, Align::Center, Trim::None);
            });
        }
    });
}

unsafe extern "system" fn panel_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                let vk = w.0 as u32;
                let modifier_only = matches!(
                    VIRTUAL_KEY(vk as u16),
                    VK_SHIFT | VK_CONTROL | VK_MENU | VK_LSHIFT | VK_RSHIFT | VK_LCONTROL | VK_RCONTROL | VK_LMENU | VK_RMENU | VK_LWIN | VK_RWIN
                );
                PANEL.with(|p| {
                    let mut b = p.borrow_mut();
                    let Some(panel) = b.as_mut() else { return };
                    if panel.captured.is_some() {
                        return;
                    }
                    if vk == VK_ESCAPE.0 as u32 {
                        panel.done = true;
                        return;
                    }
                    let m = cur_mods();
                    if modifier_only {
                        if m != 0 {
                            panel.label = format!("{} key…", mods_label(m));
                        }
                        return;
                    }
                    if m == 0 {
                        panel.label = "Please include a modifier (Ctrl Alt Shift Win)".into();
                        return;
                    }
                    panel.captured = Some((vk, m));
                    panel.label = format!("{}{}", mods_label(m), key_label(vk));
                    ui::after(0.5, || {
                        PANEL.with(|p| {
                            if let Some(panel) = p.borrow_mut().as_mut() {
                                panel.done = true;
                            }
                        });
                    });
                });
                paint();
                LRESULT(0)
            }
            WM_KEYUP | WM_SYSKEYUP => {
                PANEL.with(|p| {
                    if let Some(panel) = p.borrow_mut().as_mut() {
                        if panel.captured.is_none() {
                            let m = cur_mods();
                            panel.label = if m != 0 { format!("{} key…", mods_label(m)) } else { "Press key combination…".into() };
                        }
                    }
                });
                paint();
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                let (x, y) = crate::util::lparam_xy(l);
                PANEL.with(|p| {
                    if let Some(panel) = p.borrow_mut().as_mut() {
                        let pt = crate::theme::pt(x as f32 / panel.scale, y as f32 / panel.scale);
                        panel.cancel_hover = cancel_rect().contains(pt);
                    }
                });
                paint();
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let (x, y) = crate::util::lparam_xy(l);
                PANEL.with(|p| {
                    if let Some(panel) = p.borrow_mut().as_mut() {
                        let pt = crate::theme::pt(x as f32 / panel.scale, y as f32 / panel.scale);
                        if cancel_rect().contains(pt) {
                            panel.done = true;
                        }
                    }
                });
                LRESULT(0)
            }
            WM_NCHITTEST => {
                let r = DefWindowProcW(hwnd, msg, w, l);
                if r.0 == HTCLIENT as isize {
                    // Drag the panel by its background.
                    let mut p = POINT { x: (l.0 & 0xffff) as i16 as i32, y: ((l.0 >> 16) & 0xffff) as i16 as i32 };
                    let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut p);
                    let scale = PANEL.with(|q| q.borrow().as_ref().map(|x| x.scale).unwrap_or(1.0));
                    let pt = crate::theme::pt(p.x as f32 / scale, p.y as f32 / scale);
                    if !cancel_rect().contains(pt) {
                        return LRESULT(HTCAPTION as isize);
                    }
                }
                r
            }
            WM_CLOSE => {
                PANEL.with(|p| {
                    if let Some(panel) = p.borrow_mut().as_mut() {
                        panel.done = true;
                    }
                });
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, w, l),
        }
    }
}

/// "Change … Shortcut…": a modal panel that records the next combo.
pub fn change(id: i32) {
    // The current shortcut would otherwise fire instead of being recorded.
    let mon = app::with(|a| {
        unregister(a, id);
        a.island.monitor.clone()
    })
    .flatten()
    .or_else(|| crate::monitors::all().into_iter().next());
    let Some(mon) = mon else { return };
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(panel_proc),
            hInstance: hinst.into(),
            lpszClassName: w!("ImageVault.Shortcut"),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let scale = mon.scale;
        let (pw, ph) = ((W * scale) as i32, (H * scale) as i32);
        let x = (mon.work.left + mon.work.right) / 2 - pw / 2;
        let y = (mon.work.top + mon.work.bottom) / 2 - ph / 2 - (60.0 * scale) as i32;
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW,
            w!("ImageVault.Shortcut"),
            w!("Set Shortcut"),
            WS_POPUP,
            x,
            y,
            pw,
            ph,
            None,
            None,
            Some(hinst.into()),
            None,
        )
        .unwrap_or_default();
        let round = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &round as *const _ as *const _, 4);
        let m = windows::Win32::UI::Controls::MARGINS { cxLeftWidth: 1, cxRightWidth: 1, cyTopHeight: 1, cyBottomHeight: 1 };
        let _ = DwmExtendFrameIntoClientArea(hwnd, &m);
        PANEL.with(|p| {
            *p.borrow_mut() =
                Some(Panel { hwnd, surface: None, scale, label: "Press key combination…".into(), captured: None, done: false, cancel_hover: false, id })
        });
        paint();
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
        // Modal loop (NSAlert.runModal).
        let mut msg = MSG::default();
        loop {
            let done = PANEL.with(|p| p.borrow().as_ref().map(|x| x.done).unwrap_or(true));
            if done {
                break;
            }
            if !GetMessageW(&mut msg, None, 0, 0).as_bool() {
                PostQuitMessage(0);
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
            ui::drain();
        }
        let captured = PANEL.with(|p| p.borrow_mut().take().and_then(|x| x.captured));
        let _ = DestroyWindow(hwnd);
        app::with(|a| {
            if let Some((vk, m)) = captured {
                match id {
                    OCR => {
                        a.settings.ocr_hot_key_code = vk;
                        a.settings.ocr_hot_key_modifiers = m;
                    }
                    SHOT => {
                        a.settings.shot_hot_key_code = vk;
                        a.settings.shot_hot_key_modifiers = m;
                    }
                    _ => {}
                }
                a.settings.save();
                register(a, id);
                let label = label(a, id);
                if !registered(a, id) {
                    island::show_activity(a, None, "Shortcut taken", &format!("{label} is used by another app"));
                } else {
                    island::show_activity(a, None, &format!("{} shortcut set", name(id)), &label);
                }
            } else if !registered(a, id) {
                register(a, id);
            }
        });
    }
    let _ = &app::with(|a| a.hotkeys.paused[id as usize]);
}
