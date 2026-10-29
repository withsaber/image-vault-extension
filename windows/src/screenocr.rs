//! Shotr-style region picker shared by "capture text" (OCR → clipboard) and
//! screenshots (→ floating preview) — ScreenOCR.swift. One borderless overlay
//! per display; nothing is drawn until you drag (just the crosshair cursor);
//! while dragging the outside dims lightly and the box shows its size. In
//! screenshot mode a plain click takes the whole display. Esc, right-click or
//! the shortcut again cancels.

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

use crate::app::{self, App};
use crate::gfx::{Align, Surface, Trim};
use crate::imaging::{self, Raw};
use crate::island;
use crate::monitors::{self, Monitor};
use crate::theme::{Color, Font, Pt, Rect, Weight, pt, rect};
use crate::{clipboard, ui, util};

const ESC_HOTKEY: i32 = 10;

pub type ImageHandler = Box<dyn FnOnce(Raw, f32, Monitor)>;

struct Overlay {
    hwnd: HWND,
    surface: Option<Surface>,
    mon: Monitor,
    cursor: HCURSOR,
    dirty: bool,
}

pub struct Picker {
    overlays: Vec<Overlay>,
    on_image: Option<ImageHandler>,
    on_cancel: Option<Box<dyn FnOnce()>>,
    click_takes_display: bool,
    start: Option<(usize, Pt)>,
    current: Pt,
    previous: HWND,
}

impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
            if !self.cursor.0.is_null() {
                let _ = DestroyCursor(self.cursor);
            }
        }
    }
}

pub fn is_active(app: &App) -> bool {
    app.picker.is_some()
}

/// ⌃⌥T: capture text from the screen (or cancel).
pub fn toggle_ocr() {
    let active = app::with(|a| a.picker.is_some()).unwrap_or(false);
    if active {
        cancel();
    } else {
        begin(None, None, false);
    }
}

/// Screenshot mode: the captured pixels go to `on_image` instead of OCR.
pub fn toggle_screenshot(on_image: ImageHandler) {
    let active = app::with(|a| a.picker.is_some()).unwrap_or(false);
    if active {
        cancel();
    } else {
        begin(Some(on_image), None, true);
    }
}

pub fn pick_image(on_image: ImageHandler, on_cancel: Box<dyn FnOnce()>) {
    let active = app::with(|a| a.picker.is_some()).unwrap_or(false);
    if !active {
        begin(Some(on_image), Some(on_cancel), true);
    }
}

/// The crosshair from the design: white plus with a dark outline and a soft
/// translucent dot in the middle. No guide lines across the screen.
fn make_cursor(scale: f32) -> HCURSOR {
    let size = 34.0f32;
    let px = (size * scale).round() as u32;
    let raw = crate::gfx::offscreen(px, px, scale, |cv| {
        let c = pt(size / 2.0, size / 2.0);
        cv.fill_circle(c, 9.0, Color::gray(0.55, 0.45));
        cv.stroke_circle(c, 9.0, 1.2, Color::white(0.55));
        let arm = 15.0;
        for (w, col) in [(4.6f32, Color::black(1.0)), (2.4, Color::white(1.0))] {
            cv.line(pt(c.x - arm, c.y), pt(c.x + arm, c.y), w, col, 0);
            cv.line(pt(c.x, c.y - arm), pt(c.x, c.y + arm), w, col, 0);
        }
    });
    let Some(raw) = raw else { return unsafe { LoadCursorW(None, IDC_CROSS).unwrap_or_default() } };
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
        let Ok(color) = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
            return LoadCursorW(None, IDC_CROSS).unwrap_or_default();
        };
        std::ptr::copy_nonoverlapping(raw.px.as_ptr(), bits as *mut u8, raw.px.len());
        let mask = CreateBitmap(raw.w as i32, raw.h as i32, 1, 1, None);
        let info = ICONINFO { fIcon: FALSE, xHotspot: raw.w / 2, yHotspot: raw.h / 2, hbmMask: mask, hbmColor: color };
        let cur = CreateIconIndirect(&info).map(|h| HCURSOR(h.0)).unwrap_or_else(|_| LoadCursorW(None, IDC_CROSS).unwrap_or_default());
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        cur
    }
}

unsafe extern "system" fn overlay_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        let r = app::with(|a| handle(a, hwnd, msg, w, l));
        ui::drain();
        match r {
            Some(Some(v)) => v,
            _ => DefWindowProcW(hwnd, msg, w, l),
        }
    }
}

fn register_class() {
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(overlay_proc),
            hInstance: hinst.into(),
            lpszClassName: w!("ImageVault.Picker"),
            ..Default::default()
        };
        RegisterClassExW(&wc);
    }
}

fn begin(on_image: Option<ImageHandler>, on_cancel: Option<Box<dyn FnOnce()>>, click_takes_display: bool) {
    register_class();
    let previous = unsafe { GetForegroundWindow() };
    let mons = monitors::all();
    let mut overlays = Vec::new();
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        for m in mons {
            let r = m.rect;
            let Ok(hwnd) = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOREDIRECTIONBITMAP,
                w!("ImageVault.Picker"),
                w!("Image Vault Capture"),
                WS_POPUP,
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                None,
                None,
                Some(hinst.into()),
                None,
            ) else {
                continue;
            };
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
            let cursor = make_cursor(m.scale);
            overlays.push(Overlay { hwnd, surface: None, mon: m, cursor, dirty: true });
        }
    }
    let first = overlays.first().map(|o| o.hwnd);
    app::with(|a| {
        a.picker = Some(Picker { overlays, on_image, on_cancel, click_takes_display, start: None, current: Pt::default(), previous });
        a.tip.dismiss();
        // Clear surfaces before showing so nothing flashes.
        frame(a);
    });
    unsafe {
        // Esc cancels even if another window keeps the keyboard.
        let _ = RegisterHotKey(Some(ui::msg_hwnd()), ESC_HOTKEY, HOT_KEY_MODIFIERS(0), VK_ESCAPE.0 as u32);
        let hwnds: Vec<HWND> = app::with(|a| a.picker.as_ref().map(|p| p.overlays.iter().map(|o| o.hwnd).collect()).unwrap_or_default()).unwrap_or_default();
        for h in &hwnds {
            let _ = ShowWindow(*h, SW_SHOWNOACTIVATE);
        }
        // Make the overlay under the pointer key so it gets keys.
        let (cx, cy) = monitors::cursor();
        let target = hwnds
            .iter()
            .copied()
            .find(|h| {
                let mut r = RECT::default();
                let _ = GetWindowRect(*h, &mut r);
                cx >= r.left && cx < r.right && cy >= r.top && cy < r.bottom
            })
            .or(first);
        if let Some(t) = target {
            let fg = GetForegroundWindow();
            let fg_thread = GetWindowThreadProcessId(fg, None);
            let me = windows::Win32::System::Threading::GetCurrentThreadId();
            if fg_thread != 0 && fg_thread != me {
                let _ = windows::Win32::System::Threading::AttachThreadInput(me, fg_thread, true);
                let _ = SetForegroundWindow(t);
                let _ = windows::Win32::System::Threading::AttachThreadInput(me, fg_thread, false);
            } else {
                let _ = SetForegroundWindow(t);
            }
            let _ = SetFocus(Some(t));
            let cur = app::with(|a| a.picker.as_ref().and_then(|p| p.overlays.iter().find(|o| o.hwnd == t).map(|o| o.cursor)));
            if let Some(Some(c)) = cur {
                SetCursor(Some(c));
            }
        }
    }
}

fn tear_down(app: &mut App) -> Option<Picker> {
    unsafe {
        let _ = UnregisterHotKey(Some(ui::msg_hwnd()), ESC_HOTKEY);
        let _ = ReleaseCapture();
    }
    app.picker.take()
}

pub fn cancel() {
    let picker = app::with(tear_down).flatten();
    let Some(mut p) = picker else { return };
    let previous = p.previous;
    let cancelled = p.on_cancel.take();
    drop(p);
    match cancelled {
        Some(f) => f(),
        None => unsafe {
            if !previous.0.is_null() {
                let _ = SetForegroundWindow(previous);
            }
        },
    }
}

/// Hotkey message for the temporary Esc registration.
pub fn esc_hotkey(id: i32) -> bool {
    if id == ESC_HOTKEY {
        cancel();
        return true;
    }
    false
}

fn local_rect(p: &Picker) -> Option<(usize, Rect)> {
    let (i, s) = p.start?;
    Some((i, Rect::from_points(s, p.current)))
}

fn handle(app: &mut App, hwnd: HWND, msg: u32, _w: WPARAM, l: LPARAM) -> Option<LRESULT> {
    let picker = app.picker.as_mut()?;
    let idx = picker.overlays.iter().position(|o| o.hwnd == hwnd)?;
    let scale = picker.overlays[idx].mon.scale;
    let (x, y) = util::lparam_xy(l);
    let p = pt(x as f32 / scale, y as f32 / scale);
    match msg {
        WM_SETCURSOR => {
            unsafe {
                SetCursor(Some(picker.overlays[idx].cursor));
            }
            Some(LRESULT(1))
        }
        WM_MOUSEACTIVATE => Some(LRESULT(MA_ACTIVATE as isize)),
        WM_LBUTTONDOWN => {
            unsafe {
                SetCapture(hwnd);
                let _ = SetForegroundWindow(hwnd);
            }
            picker.start = Some((idx, p));
            picker.current = p;
            picker.overlays[idx].dirty = true;
            ui::animate(true);
            Some(LRESULT(0))
        }
        WM_MOUSEMOVE => {
            if let Some((si, _)) = picker.start {
                if si == idx {
                    let (w, h) = {
                        let r = picker.overlays[idx].mon.rect;
                        ((r.right - r.left) as f32 / scale, (r.bottom - r.top) as f32 / scale)
                    };
                    picker.current = pt(p.x.clamp(0.0, w), p.y.clamp(0.0, h));
                    picker.overlays[idx].dirty = true;
                    ui::animate(true);
                }
            }
            unsafe {
                SetCursor(Some(picker.overlays[idx].cursor));
            }
            Some(LRESULT(0))
        }
        WM_LBUTTONUP => {
            unsafe {
                let _ = ReleaseCapture();
            }
            let sel = local_rect(picker);
            picker.start = None;
            picker.overlays[idx].dirty = true;
            let mon = picker.overlays[idx].mon.clone();
            let takes = picker.click_takes_display;
            match sel {
                Some((si, r)) if si == idx && r.w >= 4.0 && r.h >= 4.0 => {
                    let px = RECT {
                        left: mon.rect.left + (r.x * scale).round() as i32,
                        top: mon.rect.top + (r.y * scale).round() as i32,
                        right: mon.rect.left + (r.r() * scale).round() as i32,
                        bottom: mon.rect.top + (r.b() * scale).round() as i32,
                    };
                    ui::defer(move || finish(px, mon));
                }
                _ => {
                    // A plain click keeps OCR mode open; in screenshot mode it takes the display.
                    if takes {
                        let full = mon.rect;
                        ui::defer(move || finish(full, mon));
                    } else {
                        ui::animate(true);
                    }
                }
            }
            Some(LRESULT(0))
        }
        WM_RBUTTONDOWN => {
            ui::defer(cancel);
            Some(LRESULT(0))
        }
        WM_KEYDOWN => {
            if _w.0 as u16 == VK_ESCAPE.0 {
                ui::defer(cancel);
                return Some(LRESULT(0));
            }
            None
        }
        WM_CLOSE => {
            ui::defer(cancel);
            Some(LRESULT(0))
        }
        _ => None,
    }
}

/// Overlay painting (only while dragging; otherwise fully transparent).
pub fn frame(app: &mut App) {
    let Some(picker) = app.picker.as_mut() else { return };
    let sel = local_rect(picker);
    for (i, o) in picker.overlays.iter_mut().enumerate() {
        if !o.dirty {
            continue;
        }
        o.dirty = false;
        let r = o.mon.rect;
        let (pw, ph) = ((r.right - r.left) as u32, (r.bottom - r.top) as u32);
        if o.surface.as_ref().map(|s| s.generation != crate::gfx::generation()).unwrap_or(true) {
            o.surface = Surface::new(o.hwnd, pw, ph, o.mon.scale).ok();
        }
        let scale = o.mon.scale;
        let selection = sel.filter(|(si, _)| *si == i).map(|(_, r)| r);
        if let Some(s) = &mut o.surface {
            s.draw(|cv| {
                let Some(rect_) = selection else { return };
                if rect_.w <= 0.0 && rect_.h <= 0.0 {
                    return;
                }
                let (w, h) = (pw as f32 / scale, ph as f32 / scale);
                // Light dim outside the box.
                let mut p = cv.g.path().alternate();
                p.rect(rect(0.0, 0.0, w, h));
                p.rect(rect_);
                let g: windows::Win32::Graphics::Direct2D::ID2D1Geometry = windows::core::Interface::cast(&p.finish()).unwrap();
                cv.fill_geom(&g, Color::black(0.22));
                // White hairline over a soft dark outline.
                cv.stroke_rect(rect_.inset(-1.0, -1.0).inset(0.5, 0.5), 1.0, Color::black(0.35));
                cv.stroke_rect(rect_.inset(0.5, 0.5), 1.0, Color::white(1.0));
                // Size label under the box (or inside if there's no room).
                let label = format!("{} \u{d7} {}", rect_.w as i32, rect_.h as i32);
                let f = Font::mono(11.0, Weight::Medium);
                let (tw, th) = cv.measure(&label, &f);
                let mut pill = rect(rect_.r() - tw - 12.0, rect_.b() + 12.0 - 6.0, tw + 12.0, th + 6.0);
                pill.y = rect_.b() + 6.0;
                if pill.b() > h - 4.0 {
                    pill.y = rect_.b() - 6.0 - pill.h;
                }
                if pill.x < 4.0 {
                    pill.x = rect_.x;
                }
                cv.fill_rrect(pill, pill.h / 2.0, Color::black(0.75));
                cv.text(&label, &f, Color::white(1.0), rect(pill.x + 6.0, pill.y + 3.0, tw + 1.0, th), Align::Leading, Trim::None);
            });
        }
    }
}

pub fn global_mouse(_app: &mut App, _ev: ui::MouseEv) {}

// ---------------------------------------------------------------------------
// Capture
// ---------------------------------------------------------------------------

/// Our own windows stay out of captures (ScreenCaptureKit excludes the app).
fn own_windows() -> Vec<HWND> {
    let mut out = Vec::new();
    unsafe extern "system" fn cb(h: HWND, l: LPARAM) -> windows::core::BOOL {
        unsafe {
            let v = &mut *(l.0 as *mut Vec<HWND>);
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, Some(&mut pid));
            if pid == std::process::id() && IsWindowVisible(h).as_bool() {
                v.push(h);
            }
            windows::core::BOOL(1)
        }
    }
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut out as *mut _ as isize));
    }
    out
}

/// Grab screen pixels (physical) for `r`.
pub fn capture(r: RECT) -> Option<Raw> {
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    if w <= 0 || h <= 0 {
        return None;
    }
    let own = own_windows();
    unsafe {
        for hw in &own {
            let _ = SetWindowDisplayAffinity(*hw, WDA_EXCLUDEFROMCAPTURE);
        }
        let _ = DwmFlush();
        let _ = DwmFlush();
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bmp.into());
        let ok = BitBlt(mem, 0, 0, w, h, Some(screen), r.left, r.top, SRCCOPY | CAPTUREBLT).is_ok();
        SelectObject(mem, old);
        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut px = vec![0u8; (w * h * 4) as usize];
        GetDIBits(mem, bmp, 0, h as u32, Some(px.as_mut_ptr() as *mut _), &mut bi, DIB_RGB_COLORS);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        for hw in &own {
            let _ = SetWindowDisplayAffinity(*hw, WDA_NONE);
        }
        if !ok {
            return None;
        }
        for p in px.chunks_exact_mut(4) {
            p[3] = 255;
        }
        Some(Raw { w: w as u32, h: h as u32, px })
    }
}

fn finish(r: RECT, mon: Monitor) {
    let picker = app::with(tear_down).flatten();
    let Some(mut p) = picker else { return };
    let previous = p.previous;
    let handler = p.on_image.take();
    let cancel_handler = p.on_cancel.take();
    drop(p);
    // The preview never takes focus, so hand it straight back.
    unsafe {
        if !previous.0.is_null() && cancel_handler.is_none() {
            let _ = SetForegroundWindow(previous);
        }
    }
    if r.right - r.left < 4 || r.bottom - r.top < 4 {
        if let Some(c) = cancel_handler {
            c();
        }
        return;
    }
    // Let the overlays leave the screen before grabbing pixels.
    ui::after(0.06, move || {
        let image = capture(r);
        match (image, handler) {
            (Some(img), Some(h)) => h(img, mon.scale, mon),
            (None, Some(_)) => {
                app::with(|a| island::show_activity(a, None, "Capture failed", ""));
                if let Some(c) = cancel_handler {
                    c();
                }
            }
            (Some(img), None) => ocr_flow(img),
            (None, None) => {
                app::with(|a| island::show_activity(a, None, "Capture failed", ""));
            }
        }
    });
}

/// OCR → clipboard, with a live activity.
fn ocr_flow(img: Raw) {
    let thumb = imaging::scale_raw(&img, 88);
    app::with(|a| island::show_activity(a, Some(thumb.clone()), "Reading text…", ""));
    let langs = app::with(|a| a.settings.ocr_extra_languages.clone()).unwrap_or_default();
    util::spawn("screen-ocr", move || {
        imaging::com_init();
        let path = std::env::temp_dir().join(format!("ImageVault-ocr-{}.png", crate::vault::random_suffix()));
        let text = imaging::encode_png(&img, None)
            .and_then(|png| std::fs::write(&path, png).ok())
            .and_then(|_| crate::ocr::recognize(&path, &langs));
        let _ = std::fs::remove_file(&path);
        ui::post(move || {
            app::with(|a| match text {
                Some(t) if !t.is_empty() => {
                    clipboard::copy_text(&t);
                    island::show_activity(a, Some(thumb), "Text copied", &format!("{} characters", t.chars().count()));
                }
                _ => island::show_activity(a, Some(thumb), "No text found", ""),
            });
        });
    });
}
