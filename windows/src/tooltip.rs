//! Help tags (SwiftUI `.help`, NSView.toolTip): a small dark tag that
//! appears after the pointer rests on an element, like macOS tooltips.

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

use crate::gfx::{Align, Surface, Trim};
use crate::theme::{Color, Font, Rect, Weight};
use crate::ui;

pub struct Tooltip {
    hwnd: HWND,
    surface: Option<Surface>,
    /// Text under the pointer right now.
    want: Option<String>,
    shown: Option<String>,
    timer: Option<ui::TimerId>,
    hide_timer: Option<ui::TimerId>,
    /// After a tag was visible, neighbours show without the delay.
    warm_until: f64,
}

const FONT: Font = Font::sans(11.0, Weight::Regular);
const MAX_W: f32 = 300.0;

unsafe extern "system" fn proc_(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        if m == WM_NCHITTEST {
            return LRESULT(HTTRANSPARENT as isize);
        }
        if m == WM_MOUSEACTIVATE {
            return LRESULT(MA_NOACTIVATE as isize);
        }
        DefWindowProcW(h, m, w, l)
    }
}

impl Tooltip {
    pub fn new() -> Tooltip {
        unsafe {
            let hinst = GetModuleHandleW(None).unwrap_or_default();
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(proc_),
                hInstance: hinst.into(),
                lpszClassName: w!("ImageVault.Tip"),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                ..Default::default()
            };
            RegisterClassExW(&wc);
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOREDIRECTIONBITMAP,
                w!("ImageVault.Tip"),
                w!(""),
                WS_POPUP,
                0,
                0,
                10,
                10,
                None,
                None,
                Some(hinst.into()),
                None,
            )
            .unwrap_or_default();
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
            Tooltip { hwnd, surface: None, want: None, shown: None, timer: None, hide_timer: None, warm_until: 0.0 }
        }
    }

    /// Report the help text under the pointer (None when there is none).
    pub fn hover(&mut self, text: Option<String>) {
        if text == self.want {
            return;
        }
        self.want = text.clone();
        if let Some(t) = self.timer.take() {
            ui::cancel(t);
        }
        match text {
            None => {
                if self.shown.is_some() {
                    self.warm_until = crate::util::mono() + 0.6;
                }
                self.hide();
            }
            Some(_) => {
                let warm = self.shown.is_some() || crate::util::mono() < self.warm_until;
                self.hide();
                let delay = if warm { 0.05 } else { 1.0 };
                self.timer = Some(ui::after(delay, || {
                    crate::app::with(|a| a.tip.show_now());
                }));
            }
        }
    }

    /// Hide immediately (click, scroll, window change).
    pub fn dismiss(&mut self) {
        self.want = None;
        if let Some(t) = self.timer.take() {
            ui::cancel(t);
        }
        self.hide();
    }

    fn hide(&mut self) {
        if self.shown.take().is_some() {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }
        if let Some(t) = self.hide_timer.take() {
            ui::cancel(t);
        }
    }

    pub fn show_now(&mut self) {
        self.timer = None;
        let Some(text) = self.want.clone() else { return };
        unsafe {
            let mut p = POINT::default();
            let _ = GetCursorPos(&mut p);
            let mon = MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST);
            let mut dpi_x = 96u32;
            let mut dpi_y = 96u32;
            let _ = windows::Win32::UI::HiDpi::GetDpiForMonitor(mon, windows::Win32::UI::HiDpi::MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
            let scale = dpi_x as f32 / 96.0;
            let g = crate::gfx::gfx();
            let lh = g.line_height(&FONT);
            // Measure (wrap long text).
            let l = g.layout(&text, &FONT, MAX_W, 100000.0, true, Align::Leading, Trim::None);
            let mut m = windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_METRICS::default();
            let _ = l.GetMetrics(&mut m);
            let tw = m.widthIncludingTrailingWhitespace.min(MAX_W).ceil();
            let th = m.height.max(lh).ceil();
            let (pad_x, pad_y) = (7.0, 3.5);
            let w = tw + pad_x * 2.0;
            let h = th + pad_y * 2.0;
            let wpx = (w * scale).ceil() as i32;
            let hpx = (h * scale).ceil() as i32;
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(mon, &mut mi);
            let work = mi.rcWork;
            let mut x = p.x - (6.0 * scale) as i32;
            let mut y = p.y + (22.0 * scale) as i32;
            if x + wpx > work.right {
                x = work.right - wpx;
            }
            if x < work.left {
                x = work.left;
            }
            if y + hpx > work.bottom {
                y = p.y - hpx - (8.0 * scale) as i32;
            }
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), x, y, wpx, hpx, SWP_NOACTIVATE);
            match &mut self.surface {
                Some(s) => {
                    let _ = s.resize(wpx as u32, hpx as u32, scale);
                }
                None => self.surface = Surface::new(self.hwnd, wpx as u32, hpx as u32, scale).ok(),
            }
            let lay = l.clone();
            if let Some(s) = &mut self.surface {
                let ok = s.draw(|cv| {
                    let r = Rect { x: 0.0, y: 0.0, w, h };
                    cv.fill_round(r, 5.0, Color::rgba(0.17, 0.17, 0.18, 0.98));
                    cv.stroke_round(r, 5.0, 1.0, Color::white(0.14));
                    unsafe {
                        cv.dc().DrawTextLayout(
                            windows_numerics::Vector2 { X: pad_x, Y: pad_y },
                            &lay,
                            cv.g.brush(Color::white(0.92)),
                            windows::Win32::Graphics::Direct2D::D2D1_DRAW_TEXT_OPTIONS_NONE,
                        );
                    }
                });
                if !ok {
                    self.surface = None;
                }
            }
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            self.shown = Some(text);
            // macOS hides help tags after ~10 s.
            self.hide_timer = Some(ui::after(10.0, || {
                crate::app::with(|a| a.tip.hide());
            }));
        }
    }
}
