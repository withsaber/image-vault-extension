//! The island window (IslandController + IslandModel + the IslandView shell).
//! The window is sized for the largest state and stays put; the black notch
//! shape animates inside it with SwiftUI's springs. Everything outside the
//! shape is click-through (WS_EX_TRANSPARENT follows the pointer).

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::ID2D1Geometry;
use windows::Win32::System::Com::IDataObject;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::{DROPEFFECT, IDropTarget};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Interface, w};

use crate::anim::{Curve, key, motion};
use crate::app::App;
use crate::dragdrop::{self, Phase, Target};
use crate::gfx::{Canvas, Surface};
use crate::imaging::Raw;
use crate::monitors::{self, Monitor};
use crate::settings::Position;
use crate::textfield::TextField;
use crate::theme::{Color, Pt, Rect, pt, rect};
use crate::ui::{self, TimerId};
use crate::{ingest, util, views};

pub const PAD: f32 = 40.0;
const WM_MOUSELEAVE_: u32 = 0x02A3;
pub const REST_H: f32 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Collapsed,
    Peek,
    Activity,
    DragHint,
    DragOver,
    Expanded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Vault,
    Clipboard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    Newest,
    Oldest,
    Name,
    Size,
}

impl Sort {
    pub fn title(self) -> &'static str {
        match self {
            Sort::Newest => "Newest",
            Sort::Oldest => "Oldest",
            Sort::Name => "Name",
            Sort::Size => "Size",
        }
    }
    pub fn next(self) -> Sort {
        match self {
            Sort::Newest => Sort::Oldest,
            Sort::Oldest => Sort::Name,
            Sort::Name => Sort::Size,
            Sort::Size => Sort::Newest,
        }
    }
    pub fn icon(self) -> crate::icons::Icon {
        use crate::icons::Icon;
        match self {
            Sort::Newest => Icon::ArrowDownToLine,
            Sort::Oldest => Icon::ArrowUpToLine,
            Sort::Name => Icon::TextFormat,
            Sort::Size => Icon::ExternalDrive,
        }
    }
}

/// A clickable region registered while drawing.
#[derive(Debug, Clone)]
pub struct Hit {
    pub rect: Rect,
    pub id: u64,
    pub tip: Option<String>,
    pub act: Act,
    /// IBeam cursor (text fields).
    pub text: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    None,
    TabVault,
    TabClip,
    Search,
    Reveal,
    Menu,
    SelectAll,
    ResetClips,
    SearchClear,
    Sort,
    SearchField,
    Count,
    BulkCopy,
    BulkText,
    BulkReveal,
    BulkDelete,
    BulkClear,
    NpArt,
    NpWave,
    NpPrev,
    NpToggle,
    NpNext,
    ClipCard(String),
    ClipCopy(String),
    ClipExpand(String),
    ClipExpCopy,
    ClipExpClose,
    ClipExpText,
    BulkGroup,
    /// The inline folder-rename field (its text area).
    FolderRename(Rect),
    Grid,
    PreviewPrev,
    PreviewNext,
    PreviewText,
    PreviewCopy,
    PreviewClose,
    PreviewImage,
    PreviewName,
    PreviewMeta,
}

pub struct Activity {
    pub image: Option<Raw>,
    pub bitmap: Option<windows::Win32::Graphics::Direct2D::ID2D1Bitmap1>,
    pub title: String,
    pub detail: String,
}

/// Drag-sensor probe (detects a drag anywhere on screen; see `probe_*`).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Probe {
    Idle,
    Armed { x: i32, y: i32 },
    Active { since: f64 },
    Done,
}

pub struct Island {
    pub hwnd: HWND,
    sensor: HWND,
    surface: Option<Surface>,
    _drop: Option<IDropTarget>,
    _sensor_drop: Option<IDropTarget>,
    pub state: State,
    pub state_curve: Curve,
    pub position: Position,
    pub show_notch: bool,
    notch_curve: Curve,
    pub notch: (f32, f32),
    pub expanded: (f32, f32),
    pub monitor: Option<Monitor>,
    pub win: RECT,
    pub scale: f32,
    transparent: bool,
    pub mouse_down_inside: bool,
    expand_work: Option<TimerId>,
    activity_work: Option<TimerId>,
    message_work: Option<TimerId>,
    expand_grace: f64,
    pub menu_open: bool,
    pub tab: Tab,
    pub tab_changed_at: f64,
    pub message: Option<String>,
    pub message_last: String,
    pub message_inserting: bool,
    pub activity: Activity,
    pub drop_busy: bool,
    drop_ok: bool,
    pub search_open: bool,
    pub search: TextField,
    pub search_focused: bool,
    pub confirm_clear: bool,
    confirm_timer: Option<TimerId>,
    pub hover: Option<u64>,
    pub pressed: Option<(u64, Act)>,
    pub hits: Vec<Hit>,
    pub dirty: bool,
    probe: Probe,
    prev_foreground: HWND,
    pub last_render: f64,
    /// Pointer in window DIPs (None when outside the window).
    pub pointer: Option<Pt>,
    last_click: (f64, Pt, u32),
    blink: Option<TimerId>,
}

unsafe extern "system" fn island_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        if let Some(Some(r)) = crate::app::with(|a| handle(a, hwnd, msg, w, l)) {
            ui::drain();
            return r;
        }
        ui::drain();
        DefWindowProcW(hwnd, msg, w, l)
    }
}

unsafe extern "system" fn sensor_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_MOUSEACTIVATE {
            return LRESULT(MA_NOACTIVATE as isize);
        }
        DefWindowProcW(hwnd, msg, w, l)
    }
}

impl Island {
    pub fn create() -> Island {
        unsafe {
            let hinst = GetModuleHandleW(None).unwrap_or_default();
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(island_proc),
                hInstance: hinst.into(),
                lpszClassName: w!("ImageVault.Island"),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                style: CS_DBLCLKS,
                ..Default::default()
            };
            RegisterClassExW(&wc);
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOREDIRECTIONBITMAP | WS_EX_TRANSPARENT,
                w!("ImageVault.Island"),
                w!("Image Vault"),
                WS_POPUP,
                0,
                0,
                100,
                100,
                None,
                None,
                Some(hinst.into()),
                None,
            )
            .expect("island window");
            let _ = windows::Win32::UI::WindowsAndMessaging::SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
            crate::menu::allow_dark(hwnd);

            // The drag sensor: a tiny near-invisible window that briefly sits
            // under the pointer when a drag starts elsewhere, so OLE tells us
            // what is being dragged (macOS reads the drag pasteboard instead).
            let wc2 = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(sensor_proc),
                hInstance: hinst.into(),
                lpszClassName: w!("ImageVault.Sensor"),
                ..Default::default()
            };
            RegisterClassExW(&wc2);
            let sensor = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_NOACTIVATE,
                w!("ImageVault.Sensor"),
                w!(""),
                WS_POPUP,
                0,
                0,
                64,
                64,
                None,
                None,
                Some(hinst.into()),
                None,
            )
            .unwrap_or_default();
            let _ = windows::Win32::UI::WindowsAndMessaging::SetLayeredWindowAttributes(sensor, COLORREF(0), 1, LWA_ALPHA);

            let drop = dragdrop::DropTarget::register(hwnd, Target::Island, crate::app::on_drop);
            let sensor_drop = dragdrop::DropTarget::register(sensor, Target::Sensor, crate::app::on_drop);

            Island {
                hwnd,
                sensor,
                surface: None,
                _drop: drop,
                _sensor_drop: sensor_drop,
                state: State::Collapsed,
                state_curve: motion::CLOSE,
                position: Position::Center,
                show_notch: true,
                notch_curve: motion::CLOSE,
                notch: (190.0, 32.0),
                expanded: (640.0, 276.0),
                monitor: None,
                win: RECT::default(),
                scale: 1.0,
                transparent: true,
                mouse_down_inside: false,
                expand_work: None,
                activity_work: None,
                message_work: None,
                expand_grace: 0.0,
                menu_open: false,
                tab: Tab::Vault,
                tab_changed_at: -10.0,
                message: None,
                message_last: String::new(),
                message_inserting: false,
                activity: Activity { image: None, bitmap: None, title: String::new(), detail: String::new() },
                drop_busy: false,
                drop_ok: false,
                search_open: false,
                search: TextField::new(),
                search_focused: false,
                confirm_clear: false,
                confirm_timer: None,
                hover: None,
                pressed: None,
                hits: Vec::new(),
                dirty: true,
                probe: Probe::Idle,
                prev_foreground: HWND::default(),
                last_render: 0.0,
                pointer: None,
                last_click: (0.0, Pt::default(), 0),
                blink: None,
            }
        }
    }

    pub fn is_open(&self) -> bool {
        matches!(self.state, State::Expanded | State::DragOver)
    }

    pub fn band(&self) -> f32 {
        self.notch.1.max(30.0)
    }

    /// Body size (without the flared ears).
    pub fn size(&self, s: State) -> (f32, f32) {
        let (nw, nh) = self.notch;
        match s {
            State::Collapsed => (nw, REST_H),
            State::Peek => (nw + 18.0, REST_H + 5.0),
            State::Activity => (nw + 176.0, nh + 2.0),
            State::DragHint => ((nw + 170.0).max(340.0), nh + 46.0),
            State::DragOver | State::Expanded => self.expanded,
        }
    }

    pub fn radii(&self, s: State) -> (f32, f32) {
        match s {
            State::Collapsed | State::Peek | State::Activity => (6.0, 14.0f32.min(self.size(s).1 * 0.45)),
            State::DragHint => (10.0, 20.0),
            State::DragOver | State::Expanded => (16.0, 30.0),
        }
    }

    pub fn outline(&self, s: State) -> (f32, f32) {
        let (w, h) = self.size(s);
        (w + self.radii(s).0 * 2.0, h)
    }

    /// Window size in DIPs.
    pub fn win_size(&self) -> (f32, f32) {
        let o = self.outline(State::Expanded);
        (o.0 + PAD * 2.0, o.1 + PAD)
    }

    /// Shape rect (target, not mid-animation) in window DIPs.
    pub fn rect_for(&self, s: State) -> Rect {
        let (ow, oh) = self.outline(s);
        self.rect_with(ow, oh)
    }

    fn rect_with(&self, ow: f32, oh: f32) -> Rect {
        let (ww, _) = self.win_size();
        let x = match self.position {
            Position::Center => ww / 2.0 - ow / 2.0,
            Position::Left => PAD,
            Position::Right => ww - PAD - ow,
        };
        rect(x, 0.0, ow, oh)
    }

    fn hover_zone(&self) -> Rect {
        let r = self.rect_for(State::Peek);
        let h = r.h.max(self.band());
        rect(r.x - 6.0, -2.0, r.w + 12.0, h + 2.0)
    }

    /// Screen pixels → window DIPs.
    pub fn local(&self, x: i32, y: i32) -> Pt {
        pt((x - self.win.left) as f32 / self.scale, (y - self.win.top) as f32 / self.scale)
    }

    /// Window DIPs → screen pixels.
    pub fn screen(&self, p: Pt) -> POINT {
        POINT { x: self.win.left + (p.x * self.scale).round() as i32, y: self.win.top + (p.y * self.scale).round() as i32 }
    }

    pub fn set_transparent(&mut self, on: bool) {
        if on == self.transparent {
            return;
        }
        self.transparent = on;
        unsafe {
            let style = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            let t = WS_EX_TRANSPARENT.0 as isize;
            let next = if on { style | t } else { style & !t };
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, next);
        }
    }
}

pub fn set_state(app: &mut App, s: State, curve: Curve) {
    util::trace(&format!("state {:?} -> {:?}", app.island.state, s));
    let isl = &mut app.island;
    if isl.state != s {
        isl.state = s;
        isl.state_curve = curve;
    } else {
        isl.state_curve = curve;
    }
    isl.dirty = true;
    app.request_frame();
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

pub fn layout(app: &mut App) {
    let mons = monitors::all();
    let want = app.settings.island_display.clone();
    let mon = want
        .as_deref()
        .and_then(|d| mons.iter().find(|m| m.device.eq_ignore_ascii_case(d)))
        .or_else(|| mons.iter().find(|m| m.primary))
        .or(mons.first())
        .cloned();
    let Some(mon) = mon else { return };
    let isl = &mut app.island;
    isl.position = app.settings.island_position;
    isl.show_notch = app.settings.show_notch;
    let scale = mon.scale.max(0.5);
    isl.scale = scale;
    let f_w = (mon.rect.right - mon.rect.left) as f32 / scale;
    // Windows has no menu bar or hardware notch: the island draws its own
    // notch at the Mac's fallback band height.
    let band_height = 32.0f32;
    isl.notch = (190.0, band_height.max(26.0));
    isl.expanded = ((660.0f32).min(f_w - 60.0), isl.band() + 8.0 + 236.0);
    let (w, h) = isl.win_size();
    let x = match isl.position {
        Position::Center => f_w / 2.0 - w / 2.0,
        Position::Left => 14.0 - PAD,
        Position::Right => f_w - 14.0 - w + PAD,
    };
    let px = mon.rect.left + (x * scale).round() as i32;
    let py = mon.rect.top;
    let pw = (w * scale).round() as i32;
    let ph = (h * scale).round() as i32;
    isl.win = RECT { left: px, top: py, right: px + pw, bottom: py + ph };
    isl.monitor = Some(mon);
    unsafe {
        let _ = SetWindowPos(isl.hwnd, Some(HWND_TOPMOST), px, py, pw, ph, SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }
    match &mut isl.surface {
        Some(s) => {
            if s.resize(pw as u32, ph as u32, scale).is_err() {
                isl.surface = None;
            }
        }
        None => {}
    }
    isl.dirty = true;
    app.request_frame();
}

/// Re-place and re-front without disturbing an open tray.
pub fn reassert(app: &mut App) {
    layout(app);
    unsafe {
        let _ = SetWindowPos(app.island.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }
}

// ---------------------------------------------------------------------------
// Pointer
// ---------------------------------------------------------------------------

fn left_down() -> bool {
    unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) < 0 }
}

/// Click-through outside the shape + hover expand / collapse-on-leave.
pub fn pointer_moved(app: &mut App) {
    if !left_down() {
        app.island.mouse_down_inside = false;
    }
    let (cx, cy) = monitors::cursor();
    let p = app.island.local(cx, cy);
    let isl = &app.island;
    let current = isl.rect_for(isl.state);
    let interactive = current.inset(-2.0, -2.0).contains(p) || isl.mouse_down_inside || app.tray.dragging_out;
    app.island.set_transparent(!interactive);

    match app.island.state {
        State::Collapsed => {
            if trigger_zone(app.island.hover_zone()).contains(p) {
                begin_hover(app);
            }
        }
        State::Activity => {
            // Touching the top edge anywhere above the Live Activity opens the tray.
            if trigger_zone(current.inset(-6.0, 0.0)).contains(p) {
                begin_hover(app);
            }
        }
        State::Peek => {
            if !app.island.hover_zone().inset(-4.0, -4.0).contains(p) {
                util::trace(&format!("peek-collapse: p={p:?} zone={:?}", app.island.hover_zone()));
                collapse(app);
            }
        }
        State::Expanded => {
            let inside = app.island.rect_for(State::Expanded).inset(-2.0, -2.0).contains(p);
            let grace = util::mono() < app.island.expand_grace;
            if !(inside || grace || app.island.mouse_down_inside || app.tray.dragging_out || app.island.menu_open) {
                util::trace(&format!("collapse: p={p:?} cursor=({cx},{cy}) win={:?} rect={:?}", app.island.win, app.island.rect_for(State::Expanded)));
                collapse(app);
            }
        }
        State::DragHint | State::DragOver => {}
    }
}

/// Opening needs the pointer pushed against the very top edge of the screen
/// (the top pixel row, window-local y = 0), within the notch's width. Passing
/// near the notch on the way to a browser tab bar never opens it.
fn trigger_zone(zone: Rect) -> Rect {
    rect(zone.x, -2.0, zone.w, 2.5)
}

fn begin_hover(app: &mut App) {
    { let (cx, cy) = monitors::cursor(); util::trace(&format!("begin_hover work={} cursor=({cx},{cy}) win={:?} scale={}", app.island.expand_work.is_some(), app.island.win, app.island.scale)); }
    if app.island.expand_work.is_some() {
        return;
    }
    if let Some(t) = app.island.activity_work.take() {
        ui::cancel(t);
    }
    set_state(app, State::Peek, motion::HOVER);
    // Warm the now-playing read during the 90 ms breath.
    crate::nowplaying::set_active(app, true);
    let t = ui::after(0.09, || {
        crate::app::with(|a| {
            a.island.expand_work = None;
            if a.island.state == State::Peek {
                expand(a, true);
            }
        });
    });
    app.island.expand_work = Some(t);
}

/// Global mouse events from the low-level hook.
pub fn mouse_event(app: &mut App, ev: ui::MouseEv) {
    if !matches!(ev, ui::MouseEv::Move(..)) { util::trace(&format!("mouse {ev:?}")); }
    use ui::MouseEv::*;
    match ev {
        LDown(x, y) => {
            let p = app.island.local(x, y);
            let local = !app.island.transparent && app.island.rect_for(app.island.state).inset(-2.0, -2.0).contains(p);
            if !local {
                app.island.probe = Probe::Armed { x, y };
            }
        }
        LUp(..) => {
            probe_end(app);
            let s = app.island.state;
            if matches!(s, State::DragHint | State::DragOver) && !app.island.drop_busy {
                ui::after(0.3, || {
                    crate::app::with(|a| {
                        if !a.island.drop_busy && matches!(a.island.state, State::DragHint | State::DragOver) {
                            collapse(a);
                        }
                    });
                });
            }
        }
        Move(x, y) => {
            if left_down() {
                probe_move(app, x, y);
            }
        }
        _ => {}
    }
    pointer_moved(app);
}

// -- drag sensor ---------------------------------------------------------------

fn fullscreen_app() -> bool {
    use windows::Win32::UI::Shell::*;
    unsafe {
        match SHQueryUserNotificationState() {
            Ok(s) => s == QUNS_RUNNING_D3D_FULL_SCREEN || s == QUNS_PRESENTATION_MODE,
            Err(_) => false,
        }
    }
}

fn probe_move(app: &mut App, x: i32, y: i32) {
    match app.island.probe {
        Probe::Armed { x: sx, y: sy } => {
            let d = (((x - sx) as f32).powi(2) + ((y - sy) as f32).powi(2)).sqrt();
            if d < 6.0 * app.island.scale {
                return;
            }
            if !matches!(app.island.state, State::Collapsed | State::Peek | State::Activity) || app.tray.dragging_out || fullscreen_app() {
                app.island.probe = Probe::Done;
                return;
            }
            app.island.probe = Probe::Active { since: util::mono() };
            place_sensor(app, x, y, true);
        }
        Probe::Active { since } => {
            if util::mono() - since > 0.45 {
                probe_end(app);
                app.island.probe = Probe::Done;
            } else {
                place_sensor(app, x, y, true);
            }
        }
        _ => {}
    }
}

fn place_sensor(app: &mut App, x: i32, y: i32, show: bool) {
    let s = (32.0 * app.island.scale) as i32;
    unsafe {
        let flags = SWP_NOACTIVATE | if show { SWP_SHOWWINDOW } else { SWP_HIDEWINDOW };
        let _ = SetWindowPos(app.island.sensor, Some(HWND_TOPMOST), x - s, y - s, s * 2, s * 2, flags);
    }
}

fn probe_end(app: &mut App) {
    if let Probe::Active { .. } = app.island.probe {
        unsafe {
            let _ = ShowWindow(app.island.sensor, SW_HIDE);
        }
    }
    app.island.probe = Probe::Idle;
}

// ---------------------------------------------------------------------------
// Transitions
// ---------------------------------------------------------------------------

pub fn expand(app: &mut App, from_hover: bool) {
    if app.island.state == State::Expanded {
        return;
    }
    set_state(app, State::Expanded, motion::OPEN);
    // Hotkey / menu opening: give the pointer a moment to reach the island.
    app.island.expand_grace = if from_hover { 0.0 } else { util::mono() + 1.6 };
    crate::nowplaying::set_active(app, true);
    app.island.search_focused = false;
    let hwnd = app.island.hwnd;
    ui::defer(move || activate(hwnd));
    pointer_moved(app);
}

/// Make the island the key window (keyboard for the tray), remembering who
/// had focus so collapsing hands it back.
fn activate(hwnd: HWND) {
    unsafe {
        let fg = GetForegroundWindow();
        if fg != hwnd {
            crate::app::with(|a| a.island.prev_foreground = fg);
        }
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        if !SetForegroundWindow(hwnd).as_bool() {
            // Borrow the foreground thread's input state for a moment.
            let fg_thread = GetWindowThreadProcessId(fg, None);
            let me = windows::Win32::System::Threading::GetCurrentThreadId();
            if fg_thread != 0 && fg_thread != me {
                let _ = windows::Win32::System::Threading::AttachThreadInput(me, fg_thread, true);
                let _ = SetForegroundWindow(hwnd);
                let _ = SetFocus(Some(hwnd));
                let _ = windows::Win32::System::Threading::AttachThreadInput(me, fg_thread, false);
            }
        }
        let _ = SetFocus(Some(hwnd));
    }
}

pub fn collapse(app: &mut App) {
    if let Some(t) = app.island.expand_work.take() {
        ui::cancel(t);
    }
    crate::nowplaying::set_active(app, false);
    if app.island.state == State::Collapsed {
        return;
    }
    schedule_trim();
    crate::tray::close_preview(app);
    app.tip.dismiss();
    set_state(app, State::Collapsed, motion::CLOSE);
    app.island.search_focused = false;
    let hwnd = app.island.hwnd;
    let prev = std::mem::take(&mut app.island.prev_foreground);
    ui::defer(move || unsafe {
        if GetForegroundWindow() == hwnd && !prev.0.is_null() && IsWindowVisible(prev).as_bool() {
            let _ = SetForegroundWindow(prev);
        }
    });
    pointer_moved(app);
}

pub fn toggle(app: &mut App) {
    if app.island.state == State::Expanded {
        collapse(app);
    } else {
        reassert(app);
        expand(app, false);
    }
}

/// Live Activity: the island widens with a thumbnail + status, then tucks back.
pub fn show_activity(app: &mut App, image: Option<Raw>, title: &str, detail: &str) {
    if !matches!(app.island.state, State::Collapsed | State::Activity) {
        return;
    }
    app.island.activity = Activity { image, bitmap: None, title: title.to_string(), detail: detail.to_string() };
    set_state(app, State::Activity, motion::OPEN);
    unsafe {
        let _ = SetWindowPos(app.island.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
    if let Some(t) = app.island.activity_work.take() {
        ui::cancel(t);
    }
    app.island.activity_work = Some(ui::after(2.4, || {
        crate::app::with(|a| {
            a.island.activity_work = None;
            if a.island.state == State::Activity {
                set_state(a, State::Collapsed, motion::CLOSE);
            }
        });
    }));
}

pub fn flash(app: &mut App, text: &str) {
    if app.island.message.is_none() {
        app.island.message_inserting = true;
    }
    app.island.message = Some(text.to_string());
    app.island.message_last = text.to_string();
    app.island.dirty = true;
    if let Some(t) = app.island.message_work.take() {
        ui::cancel(t);
    }
    app.island.message_work = Some(ui::after(2.2, || {
        crate::app::with(|a| {
            a.island.message = None;
            a.island.message_inserting = false;
            a.island.message_work = None;
            a.island.dirty = true;
            a.request_frame();
        });
    }));
    app.request_frame();
}

pub fn select_tab(app: &mut App, tab: Tab) {
    if app.island.tab == tab {
        return;
    }
    app.island.confirm_clear = false;
    if tab == Tab::Clipboard {
        crate::tray::clear_selection(app);
    }
    app.island.tab = tab;
    app.island.tab_changed_at = util::mono();
    app.island.dirty = true;
    if tab == Tab::Vault {
        app.island.search_focused = false;
    }
    app.request_frame();
}

pub fn open_search(app: &mut App) {
    app.island.search_open = true;
    app.island.search_focused = true;
    app.island.search.select_all();
    app.island.search.blink_epoch = util::mono();
    app.island.dirty = true;
    schedule_blink(app);
    app.request_frame();
}

pub fn close_search(app: &mut App) {
    app.island.search.clear();
    app.island.search_focused = false;
    app.island.search_open = false;
    query_changed(app);
    app.island.dirty = true;
    app.request_frame();
}

pub fn query_changed(app: &mut App) {
    views::clip_collapse_now();
    let q = app.island.search.text.clone();
    crate::tray::set_filter(app, &q, app.tray.sort);
    app.island.dirty = true;
}

pub fn schedule_blink(app: &mut App) {
    if app.island.blink.is_some() {
        return;
    }
    app.island.blink = Some(ui::after(0.5, || {
        crate::app::with(|a| {
            a.island.blink = None;
            if (a.island.search_focused || a.tray.renaming.is_some()) && a.island.state == State::Expanded {
                a.island.dirty = true;
                a.request_frame();
                schedule_blink(a);
            }
        });
    }));
}

/// Two clicks: the first arms it (icon turns into a check), the second clears.
pub fn reset_clipboard(app: &mut App) {
    if app.island.confirm_clear {
        app.clips.clear();
        app.island.confirm_clear = false;
        flash(app, "Clipboard history cleared");
    } else {
        if app.clips.items.is_empty() {
            flash(app, "Clipboard history is empty");
            return;
        }
        app.island.confirm_clear = true;
        flash(app, "Click again to clear");
        if let Some(t) = app.island.confirm_timer.take() {
            ui::cancel(t);
        }
        app.island.confirm_timer = Some(ui::after(3.0, || {
            crate::app::with(|a| {
                a.island.confirm_clear = false;
                a.island.confirm_timer = None;
                a.island.dirty = true;
                a.request_frame();
            });
        }));
    }
    app.island.dirty = true;
}

pub fn reveal_action(app: &mut App) {
    if app.tray.selected.is_empty() {
        util::shell_open(&crate::vault::images_dir().display().to_string());
    } else {
        crate::tray::reveal_selection(app);
    }
}

// ---------------------------------------------------------------------------
// Drag & drop (island target + sensor)
// ---------------------------------------------------------------------------

pub fn drop_event(app: &mut App, target: Target, phase: Phase, data: Option<&IDataObject>, _p: POINT) -> DROPEFFECT {
    match target {
        Target::Sensor => {
            if phase == Phase::Enter {
                if let Some(d) = data {
                    let ok = ingest::looks_like_image(d);
                    probe_end(app);
                    app.island.probe = Probe::Done;
                    if ok && matches!(app.island.state, State::Collapsed | State::Peek | State::Activity) && !app.tray.dragging_out {
                        if let Some(t) = app.island.activity_work.take() {
                            ui::cancel(t);
                        }
                        if let Some(t) = app.island.expand_work.take() {
                            ui::cancel(t);
                        }
                        set_state(app, State::DragHint, motion::OPEN);
                    }
                }
            }
            dragdrop::effect_none()
        }
        // Our own tiles dragged within the grid: onto a folder or another image.
        Target::Island if app.tray.dragging_out && data.map(dragdrop::is_ours).unwrap_or(phase != Phase::Enter && phase != Phase::Drop) => {
            let lp = pt((_p.x - app.island.win.left) as f32 / app.island.scale, (_p.y - app.island.win.top) as f32 / app.island.scale);
            match phase {
                Phase::Enter | Phase::Over => {
                    if crate::tray::internal_drag_over(app, lp) { dragdrop::effect_copy() } else { dragdrop::effect_none() }
                }
                Phase::Leave => {
                    crate::tray::internal_drag_leave(app);
                    dragdrop::effect_none()
                }
                Phase::Drop => {
                    if crate::tray::internal_drop(app, lp) { dragdrop::effect_copy() } else { dragdrop::effect_none() }
                }
            }
        }
        Target::Island => match phase {
            Phase::Enter => {
                let ok = data.map(|d| !app.tray.dragging_out && ingest::looks_like_image(d)).unwrap_or(false);
                app.island.drop_ok = ok;
                if !ok {
                    return dragdrop::effect_none();
                }
                if app.island.state != State::DragOver && !app.island.drop_busy {
                    if let Some(t) = app.island.expand_work.take() {
                        ui::cancel(t);
                    }
                    set_state(app, State::DragOver, motion::OPEN);
                    crate::nowplaying::set_active(app, false);
                }
                dragdrop::effect_copy()
            }
            Phase::Over => {
                if app.island.drop_ok { dragdrop::effect_copy() } else { dragdrop::effect_none() }
            }
            Phase::Leave => {
                if !app.island.drop_busy && app.island.state == State::DragOver {
                    set_state(app, State::DragHint, motion::CLOSE);
                }
                app.island.drop_ok = false;
                dragdrop::effect_none()
            }
            Phase::Drop => {
                let Some(d) = data else { return dragdrop::effect_none() };
                if !app.island.drop_ok {
                    return dragdrop::effect_none();
                }
                app.island.drop_ok = false;
                // Read the payload now, while the drop is alive.
                let payload = ingest::read_drop(d);
                app.island.drop_busy = true;
                app.island.dirty = true;
                ui::after(45.0, || {
                    crate::app::with(|a| {
                        if a.island.drop_busy {
                            a.island.drop_busy = false;
                            if a.island.state == State::DragOver {
                                collapse(a);
                            }
                        }
                    });
                });
                ingest::import_drop(payload, |summary| {
                    crate::app::with(|a| {
                        a.island.drop_busy = false;
                        crate::app::finish_ingest(a, &summary);
                        set_state(a, State::Collapsed, motion::CLOSE);
                        crate::nowplaying::set_active(a, false);
                        if summary.is_empty() {
                            show_activity(a, None, "No image found", "");
                            return;
                        }
                        let n = summary.added.len();
                        let title = if n == 0 {
                            "Already saved".to_string()
                        } else if n > 1 {
                            format!("{n} saved")
                        } else {
                            "Saved".to_string()
                        };
                        let detail = format!("{} in vault", a.model.items.len());
                        show_activity(a, summary.first.clone(), &title, &detail);
                    });
                });
                dragdrop::effect_copy()
            }
        },
        Target::Editor(_) => dragdrop::effect_none(),
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// The NotchShape path: flared top corners that melt into the screen edge.
pub fn notch_path(cv: &Canvas, r: Rect, top: f32, bottom: f32) -> ID2D1Geometry {
    let t = top.min(r.w / 4.0).max(0.0);
    let b = bottom.min(r.h / 2.0).min((r.w - 2.0 * t) / 2.0).max(0.0);
    let mut p = cv.g.path();
    p.move_to(pt(r.x, r.y));
    p.quad_to(pt(r.x + t, r.y), pt(r.x + t, r.y + t));
    p.line_to(pt(r.x + t, r.b() - b));
    p.quad_to(pt(r.x + t, r.b()), pt(r.x + t + b, r.b()));
    p.line_to(pt(r.r() - t - b, r.b()));
    p.quad_to(pt(r.r() - t, r.b()), pt(r.r() - t, r.b() - b));
    p.line_to(pt(r.r() - t, r.y + t));
    p.quad_to(pt(r.r() - t, r.y), pt(r.r(), r.y));
    p.close();
    p.finish().cast().unwrap()
}

pub fn render(app: &mut App) {
    let isl = &mut app.island;
    if isl.surface.as_ref().map(|s| s.generation != crate::gfx::generation()).unwrap_or(true) {
        isl.surface = None;
        let pw = (isl.win.right - isl.win.left).max(1) as u32;
        let ph = (isl.win.bottom - isl.win.top).max(1) as u32;
        isl.surface = Surface::new(isl.hwnd, pw, ph, isl.scale).ok();
        isl.activity.bitmap = None;
    }
    let Some(mut surface) = app.island.surface.take() else { return };
    app.island.hits.clear();
    let ok = surface.draw(|cv| draw(cv, app));
    app.island.surface = Some(surface);
    app.island.last_render = util::mono();
    if !ok {
        app.island.surface = None;
        app.on_device_lost();
    }
}

pub fn draw(cv: &mut Canvas, app: &mut App) {
    let curve = app.island.state_curve;
    let state = app.island.state;
    let (ow, oh) = app.island.outline(state);
    let (rt, rb) = app.island.radii(state);
    let a = &mut app.anims;
    let w = a.drive(key("isl.w"), ow, curve);
    let h = a.drive(key("isl.h"), oh, curve);
    let top = a.drive(key("isl.top"), rt, curve);
    let bottom = a.drive(key("isl.bottom"), rb, curve);
    let shadow = a.drive(key("isl.shadow"), if app.island.is_open() { 0.55 } else { 0.0 }, curve);
    let notch_target = if state == State::Collapsed && !app.island.show_notch { 0.0 } else { 1.0 };
    let notch_curve = if app.island.notch_curve == motion::CONTENT_OUT { motion::CONTENT_OUT } else { curve };
    let notch_op = a.drive(key("isl.notch"), notch_target, notch_curve);
    let shape = app.island.rect_with(w.max(0.0), h.max(0.0));
    let path = notch_path(cv, shape, top, bottom);

    // Shell: black notch with the open-state shadow.
    cv.save();
    cv.opacity(notch_op.clamp(0.0, 1.0));
    let p2 = path.clone();
    cv.shadowed(12.0, pt(0.0, 12.0), Color::black(shadow.clamp(0.0, 1.0)), true, move |cv| {
        cv.fill_geom(&p2, Color::black(1.0));
    });
    cv.restore();

    // Content layers, clipped to the shape.
    cv.push_mask(&path);
    let layers = [State::Activity, State::DragHint, State::Expanded, State::DragOver];
    for l in layers {
        let visible = state == l;
        let k = match l {
            State::Activity => key("layer.activity"),
            State::DragHint => key("layer.draghint"),
            State::Expanded => key("layer.expanded"),
            _ => key("layer.dragover"),
        };
        let v = app.anims.drive(k, if visible { 1.0 } else { 0.0 }, if visible { motion::CONTENT_IN } else { motion::CONTENT_OUT });
        if v <= 0.002 {
            continue;
        }
        let (lw, lh) = app.island.size(l);
        let lr = rect(shape.x + (shape.w - lw) / 2.0, shape.y, lw, lh);
        let blur = (7.0 * (1.0 - v)).max(0.0);
        let scale = 0.94 + 0.06 * v;
        let opacity = v.clamp(0.0, 1.0);
        let hits_on = visible;
        cv.save();
        cv.scale_about(scale, lr.cx(), lr.y);
        let sigma = blur * 0.5;
        cv.blurred(sigma, opacity, |cv| match l {
            State::Activity => views::activity(cv, app, lr),
            State::DragHint => views::drag_hint(cv, app, lr),
            State::Expanded => views::expanded(cv, app, lr, hits_on),
            _ => views::drag_over(cv, app, lr),
        });
        cv.restore();
    }
    cv.pop_layer();
}

/// Anything still moving on the island (keeps the frame clock running).
pub fn animating(app: &App) -> bool {
    let isl = &app.island;
    if isl.state == State::Expanded && isl.tab == Tab::Vault && app.model.items.is_empty() {
        return true; // EmptyTray's marching dashes
    }
    if isl.state == State::DragOver {
        return true; // DropTarget breathing / spinning
    }
    false
}

// ---------------------------------------------------------------------------
// Input (window messages)
// ---------------------------------------------------------------------------

fn hit_at(app: &App, p: Pt) -> Option<Hit> {
    app.island.hits.iter().rev().find(|h| h.rect.contains(p)).cloned()
}

fn update_hover(app: &mut App, p: Option<Pt>) {
    app.island.pointer = p;
    let hit = p.and_then(|p| hit_at(app, p));
    let id = hit.as_ref().map(|h| h.id);
    if id != app.island.hover {
        app.island.hover = id;
        app.island.dirty = true;
        app.request_frame();
    }
    let tip = hit.and_then(|h| h.tip).or_else(|| p.and_then(|p| crate::tray::tip_at(app, p)));
    app.tip.hover(tip);
    crate::tray::pointer(app, p);
}

fn mods() -> (bool, bool) {
    unsafe { (GetKeyState(VK_CONTROL.0 as i32) < 0, GetKeyState(VK_SHIFT.0 as i32) < 0) }
}

fn handle(app: &mut App, hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> Option<LRESULT> {
    match msg {
        WM_MOUSEMOVE => {
            let (x, y) = util::lparam_xy(l);
            let p = pt(x as f32 / app.island.scale, y as f32 / app.island.scale);
            if !app.island.hover.is_some() && app.island.pointer.is_none() {
                unsafe {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut tme);
                }
            }
            update_hover(app, Some(p));
            if left_down() {
                drag(app, p);
            }
            Some(LRESULT(0))
        }
        WM_MOUSELEAVE_ => {
            update_hover(app, None);
            Some(LRESULT(0))
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            let (x, y) = util::lparam_xy(l);
            let p = pt(x as f32 / app.island.scale, y as f32 / app.island.scale);
            app.island.mouse_down_inside = true;
            app.tip.dismiss();
            unsafe {
                SetCapture(hwnd);
            }
            let now = util::mono();
            let (t0, p0, n0) = app.island.last_click;
            let dbl = unsafe { GetDoubleClickTime() } as f64 / 1000.0;
            let clicks = if now - t0 < dbl && p.dist(p0) < 5.0 { n0 + 1 } else { 1 };
            app.island.last_click = (now, p, clicks);
            mouse_down(app, p, clicks);
            Some(LRESULT(0))
        }
        WM_LBUTTONUP => {
            let (x, y) = util::lparam_xy(l);
            let p = pt(x as f32 / app.island.scale, y as f32 / app.island.scale);
            unsafe {
                let _ = ReleaseCapture();
            }
            mouse_up(app, p);
            app.island.mouse_down_inside = false;
            pointer_moved(app);
            Some(LRESULT(0))
        }
        WM_RBUTTONUP => {
            let (x, y) = util::lparam_xy(l);
            let p = pt(x as f32 / app.island.scale, y as f32 / app.island.scale);
            app.tip.dismiss();
            right_click(app, p);
            Some(LRESULT(0))
        }
        WM_MOUSEWHEEL => {
            let delta = ((w.0 >> 16) & 0xffff) as i16 as f32;
            crate::tray::wheel(app, delta);
            app.tip.dismiss();
            Some(LRESULT(0))
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let vk = w.0 as u16;
            if key_down(app, vk) { Some(LRESULT(0)) } else { None }
        }
        WM_CHAR => {
            let c = w.0 as u32;
            if app.tray.renaming.is_some() && app.island.state == State::Expanded {
                if let Some(ch) = char::from_u32(c) {
                    crate::tray::rename_char(app, ch);
                }
                return Some(LRESULT(0));
            }
            if app.island.search_focused && app.island.state == State::Expanded {
                if let Some(ch) = char::from_u32(c) {
                    if app.island.search.char(ch) {
                        query_changed(app);
                    }
                    app.island.dirty = true;
                    app.request_frame();
                }
                return Some(LRESULT(0));
            }
            Some(LRESULT(0))
        }
        WM_SETCURSOR => {
            let p = app.island.pointer;
            let text = p.and_then(|p| hit_at(app, p)).map(|h| h.text).unwrap_or(false);
            unsafe {
                let c = LoadCursorW(None, if text { IDC_IBEAM } else { IDC_ARROW }).unwrap_or_default();
                SetCursor(Some(c));
            }
            Some(LRESULT(1))
        }
        WM_MOUSEACTIVATE => Some(LRESULT(MA_ACTIVATE as isize)),
        WM_DPICHANGED | WM_DISPLAYCHANGE => {
            let _ = ui::after(0.05, || {
                crate::app::with(layout);
            });
            Some(LRESULT(0))
        }
        WM_IME_STARTCOMPOSITION => {
            ime_position(app);
            None
        }
        WM_CLOSE => Some(LRESULT(0)),
        _ => None,
    }
}

fn ime_position(app: &mut App) {
    use windows::Win32::UI::Input::Ime::*;
    let Some(field) = views::search_field_rect(app) else { return };
    let g = crate::gfx::gfx();
    let x = app.island.search.caret_x(&g, &views::SEARCH_FONT, field);
    unsafe {
        let himc = ImmGetContext(app.island.hwnd);
        if !himc.is_invalid() {
            let p = app.island.screen(pt(x, field.y));
            let mut cp = POINT { x: p.x - app.island.win.left, y: p.y - app.island.win.top };
            let cf = COMPOSITIONFORM { dwStyle: CFS_POINT, ptCurrentPos: cp, rcArea: RECT::default() };
            let _ = ImmSetCompositionWindow(himc, &cf);
            cp.y += (field.h * app.island.scale) as i32;
            let _ = ImmReleaseContext(app.island.hwnd, himc);
        }
    }
}

fn mouse_down(app: &mut App, p: Pt, clicks: u32) {
    let hit = hit_at(app, p);
    let (ctrl, shift) = mods();
    // Clicking anywhere but the rename field keeps the new name.
    if app.tray.renaming.is_some() && !matches!(hit.as_ref().map(|h| &h.act), Some(Act::FolderRename(_))) {
        crate::tray::commit_rename(app);
    }
    if let Some(h) = &hit {
        match &h.act {
            Act::FolderRename(field) => {
                crate::tray::rename_press(app, *field, p, shift, clicks);
                app.island.pressed = Some((h.id, h.act.clone()));
                return;
            }
            Act::SearchField => {
                app.island.search_focused = true;
                if let Some(field) = views::search_field_rect(app) {
                    let g = crate::gfx::gfx();
                    app.island.search.mouse_down(&g, &views::SEARCH_FONT, field, p.x, shift, clicks);
                }
                schedule_blink(app);
                app.island.pressed = Some((h.id, h.act.clone()));
                app.island.dirty = true;
                app.request_frame();
                return;
            }
            Act::Grid | Act::PreviewImage => {}
            Act::NpWave => {
                app.island.pressed = Some((h.id, h.act.clone()));
                crate::nowplaying::scrub(app, h.rect, p, false);
                return;
            }
            Act::ClipCard(id) => {
                views::clip_press(id, p);
                app.island.pressed = Some((h.id, h.act.clone()));
                app.island.dirty = true;
                return;
            }
            Act::ClipExpText => {
                app.island.search_focused = false;
                views::clip_exp_press(app, p, clicks, shift);
                app.island.pressed = Some((h.id, h.act.clone()));
                return;
            }
            _ => {
                app.island.pressed = Some((h.id, h.act.clone()));
                app.island.dirty = true;
                app.request_frame();
                return;
            }
        }
    }
    // Clicking anything but the field ends text editing.
    if app.island.search_focused {
        app.island.search_focused = false;
        app.island.dirty = true;
    }
    if let Some(h) = &hit {
        if h.act == Act::PreviewImage {
            app.island.pressed = Some((h.id, h.act.clone()));
            return;
        }
        if let Act::ClipCard(_) = h.act {
            return;
        }
    }
    crate::tray::mouse_down(app, p, clicks, ctrl, shift);
}

fn drag(app: &mut App, p: Pt) {
    if let Some((_, act)) = app.island.pressed.clone() {
        match act {
            Act::SearchField => {
                if let Some(field) = views::search_field_rect(app) {
                    let g = crate::gfx::gfx();
                    app.island.search.mouse_drag(&g, &views::SEARCH_FONT, field, p.x);
                    app.island.dirty = true;
                    app.request_frame();
                }
            }
            Act::NpWave => {
                if let Some(h) = app.island.hits.iter().find(|h| h.act == Act::NpWave).cloned() {
                    crate::nowplaying::scrub(app, h.rect, p, false);
                }
            }
            Act::ClipCard(id) => {
                views::clip_drag(app, &id, p);
            }
            Act::ClipExpText => views::clip_exp_drag(app, p),
            Act::FolderRename(field) => crate::tray::rename_drag(app, field, p),
            _ => {}
        }
        return;
    }
    crate::tray::mouse_drag(app, p);
}

fn mouse_up(app: &mut App, p: Pt) {
    if let Some((id, act)) = app.island.pressed.take() {
        app.island.dirty = true;
        app.request_frame();
        let inside = app.island.hits.iter().find(|h| h.id == id).map(|h| h.rect.contains(p)).unwrap_or(false);
        match act {
            Act::SearchField => {
                app.island.search.mouse_up();
            }
            Act::FolderRename(_) => crate::tray::rename_release(app),
            Act::NpWave => {
                if let Some(h) = app.island.hits.iter().find(|h| h.act == Act::NpWave).cloned() {
                    crate::nowplaying::scrub(app, h.rect, p, true);
                }
            }
            // IslandButton uses a zero-distance DragGesture: fires wherever the pointer is released.
            Act::Search => {
                if app.island.search_open {
                    close_search(app);
                } else {
                    open_search(app);
                }
            }
            Act::Reveal => reveal_action(app),
            Act::Menu => crate::app::show_settings_menu(None),
            Act::SelectAll => crate::tray::toggle_select_all(app),
            Act::ResetClips => reset_clipboard(app),
            // Clipboard cards fire as taps (not after a drag).
            Act::ClipCard(cid) => {
                if inside && !views::clip_dragged() {
                    views::clip_click(app, &cid);
                }
                views::clip_drag_end();
            }
            other if inside => click(app, other),
            _ => {}
        }
        return;
    }
    crate::tray::mouse_up(app, p);
}

fn click(app: &mut App, act: Act) {
    match act {
        Act::TabVault => select_tab(app, Tab::Vault),
        Act::TabClip => select_tab(app, Tab::Clipboard),
        Act::SearchClear => {
            app.island.search.clear();
            query_changed(app);
        }
        Act::Sort => {
            let next = app.tray.sort.next();
            let q = app.island.search.text.clone();
            crate::tray::set_filter(app, &q, next);
        }
        Act::BulkCopy => crate::tray::copy_selection(app),
        Act::BulkGroup => crate::tray::group_selection(app),
        Act::BulkText => crate::tray::copy_text_selection(app),
        Act::BulkReveal => reveal_action(app),
        Act::BulkDelete => crate::tray::delete_selection(app),
        Act::BulkClear => crate::tray::clear_selection(app),
        Act::NpArt => crate::nowplaying::open_player(app),
        Act::NpPrev => crate::nowplaying::previous(app),
        Act::NpToggle => crate::nowplaying::play_pause(app),
        Act::NpNext => crate::nowplaying::next(app),
        Act::PreviewPrev => crate::tray::preview_step(app, -1),
        Act::PreviewNext => crate::tray::preview_step(app, 1),
        Act::PreviewText => crate::tray::preview_text(app),
        Act::PreviewCopy => crate::tray::preview_copy(app),
        Act::PreviewClose => crate::tray::close_preview(app),
        Act::PreviewImage => {
            if let Some(p) = app.island.pointer {
                crate::tray::preview_click(app, p);
            }
        }
        Act::ClipCopy(id) => views::clip_click(app, &id),
        Act::ClipExpand(id) => views::clip_expand(app, &id),
        Act::ClipExpCopy => views::clip_exp_copy(app),
        Act::ClipExpClose => views::clip_collapse(app),
        _ => {}
    }
}

fn right_click(app: &mut App, p: Pt) {
    if let Some(h) = hit_at(app, p) {
        if let Act::ClipCard(id) = &h.act {
            views::clip_menu(app, id.clone());
            return;
        }
    }
    if crate::tray::right_click(app, p) {
        return;
    }
    let sp = app.island.screen(p);
    crate::app::show_settings_menu(Some((sp.x, sp.y)));
}

fn key_down(app: &mut App, vk: u16) -> bool {
    if app.island.state != State::Expanded {
        return false;
    }
    app.tip.dismiss();
    if crate::tray::rename_key(app, vk) {
        return true;
    }
    if app.island.search_focused {
        match VIRTUAL_KEY(vk) {
            VK_RETURN => {
                app.island.search_focused = false;
                app.island.dirty = true;
                app.request_frame();
                return true;
            }
            VK_ESCAPE => {
                if !app.island.search.text.is_empty() {
                    app.island.search.clear();
                    query_changed(app);
                } else {
                    close_search(app);
                }
                app.island.dirty = true;
                app.request_frame();
                return true;
            }
            _ => {}
        }
        match app.island.search.key(vk) {
            crate::textfield::KeyResult::Handled { changed } => {
                if changed {
                    query_changed(app);
                }
                app.island.dirty = true;
                app.request_frame();
                return true;
            }
            crate::textfield::KeyResult::Pass => return false,
        }
    }
    if app.island.tab == Tab::Clipboard {
        let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
        if views::clip_exp_key(app, vk, ctrl) {
            return true;
        }
        if VIRTUAL_KEY(vk) == VK_ESCAPE {
            collapse(app);
            return true;
        }
        if vk == VK_OEM_2.0 {
            open_search(app);
            return true;
        }
        return false;
    }
    crate::tray::key_down(app, vk)
}

pub fn invalidate(app: &mut App) {
    app.island.dirty = true;
    app.request_frame();
}

pub fn set_show_notch(app: &mut App, on: bool) {
    app.island.show_notch = on;
    app.island.notch_curve = motion::CONTENT_OUT;
    app.island.dirty = true;
    app.request_frame();
    // The next state change uses the state curve again.
    ui::after(0.3, || {
        crate::app::with(|a| a.island.notch_curve = motion::CLOSE);
    });
}

/// Hand idle memory back to the system once the island has rested a moment.
pub fn schedule_trim() {
    ui::after(4.0, || {
        crate::app::with(|a| {
            if a.island.state == State::Collapsed && a.captures.editors.is_empty() {
                unsafe {
                    let _ = windows::Win32::System::ProcessStatus::EmptyWorkingSet(windows::Win32::System::Threading::GetCurrentProcess());
                }
            }
        });
    });
}
