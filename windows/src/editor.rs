//! Shotr-style screenshot editor (Editor.swift): toolbar in the title bar
//! (Copy, Save, Pin | grip | tools | colour & width | Add Capture | Blur | ···),
//! the colour under the pointer + image size on the right, a zoomable,
//! scrollable canvas that opens at 100%, undo/redo, inline text.

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::ID2D1Geometry;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Interface, w};

use crate::app::{self, App};
use crate::capture::{self, Annotation, Kind};
use crate::gfx::{Align, Canvas, Surface, Trim};
use crate::icons::{self, Icon};
use crate::monitors;
use crate::textfield::TextField;
use crate::theme::{Color, Font, Pt, Rect, Weight, pt, rect};
use crate::{clipboard, ui, util};

const BAR_H: f32 = 40.0;
const MIN_W: f32 = 820.0;
const PAD: f32 = 16.0;
const CAPTION_W: f32 = 46.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Select,
    Arrow,
    Text,
    Measure,
    Rectangle,
    FilledRect,
    Pencil,
    Blur,
    Highlighter,
    Spotlight,
    Counter,
    Oval,
    Line,
}

impl Tool {
    pub const ALL: [Tool; 13] = [
        Tool::Select,
        Tool::Arrow,
        Tool::Text,
        Tool::Measure,
        Tool::Rectangle,
        Tool::FilledRect,
        Tool::Pencil,
        Tool::Blur,
        Tool::Highlighter,
        Tool::Spotlight,
        Tool::Counter,
        Tool::Oval,
        Tool::Line,
    ];
    pub const BAR: [Tool; 7] = [Tool::Select, Tool::Arrow, Tool::Text, Tool::Measure, Tool::Rectangle, Tool::FilledRect, Tool::Pencil];
    pub const MENU: [Tool; 6] = [Tool::Blur, Tool::Highlighter, Tool::Spotlight, Tool::Counter, Tool::Oval, Tool::Line];

    pub fn title(self) -> &'static str {
        match self {
            Tool::Select => "Select & Move",
            Tool::Arrow => "Arrow",
            Tool::Text => "Text",
            Tool::Measure => "Measure",
            Tool::Rectangle => "Rectangle",
            Tool::FilledRect => "Filled Rectangle",
            Tool::Pencil => "Pen",
            Tool::Blur => "Blur/Erase",
            Tool::Highlighter => "Highlighter",
            Tool::Spotlight => "Spotlight",
            Tool::Counter => "Counter",
            Tool::Oval => "Oval",
            Tool::Line => "Line",
        }
    }
    pub fn key(self) -> char {
        match self {
            Tool::Select => 'V',
            Tool::Arrow => 'A',
            Tool::Text => 'T',
            Tool::Measure => 'M',
            Tool::Rectangle => 'R',
            Tool::FilledRect => 'F',
            Tool::Pencil => 'P',
            Tool::Blur => 'B',
            Tool::Highlighter => 'H',
            Tool::Spotlight => 'S',
            Tool::Counter => 'C',
            Tool::Oval => 'O',
            Tool::Line => 'L',
        }
    }
    pub fn icon(self) -> Icon {
        match self {
            Tool::Select => Icon::CursorArrow,
            Tool::Arrow => Icon::ArrowUpRight,
            Tool::Text => Icon::TextFormat,
            Tool::Measure => Icon::Ruler,
            Tool::Rectangle => Icon::Rectangle,
            Tool::FilledRect => Icon::RectangleFill,
            Tool::Pencil => Icon::Pencil,
            Tool::Blur => Icon::Drop,
            Tool::Highlighter => Icon::Highlighter,
            Tool::Spotlight => Icon::RectangleDashed,
            Tool::Counter => Icon::OneCircle,
            Tool::Oval => Icon::Circle,
            Tool::Line => Icon::LineDiagonal,
        }
    }
    pub fn kind(self) -> Option<Kind> {
        match self {
            Tool::Select | Tool::Text => None,
            Tool::Arrow => Some(Kind::Arrow),
            Tool::Measure => Some(Kind::Measure),
            Tool::Rectangle => Some(Kind::Rectangle),
            Tool::FilledRect => Some(Kind::FilledRect),
            Tool::Pencil => Some(Kind::Pencil),
            Tool::Blur => Some(Kind::Blur),
            Tool::Highlighter => Some(Kind::Highlighter),
            Tool::Spotlight => Some(Kind::Spotlight),
            Tool::Counter => Some(Kind::Counter),
            Tool::Oval => Some(Kind::Oval),
            Tool::Line => Some(Kind::Line),
        }
    }
}

pub const COLORS: [(&str, Color); 8] = [
    ("Red", Color::rgba(1.0, 0.23, 0.19, 1.0)),
    ("Orange", Color::rgba(1.0, 0.58, 0.0, 1.0)),
    ("Yellow", Color::rgba(1.0, 0.8, 0.0, 1.0)),
    ("Green", Color::rgba(0.2, 0.78, 0.35, 1.0)),
    ("Blue", Color::rgba(0.0, 0.48, 1.0, 1.0)),
    ("Purple", Color::rgba(0.69, 0.32, 0.87, 1.0)),
    ("White", Color::rgba(1.0, 1.0, 1.0, 1.0)),
    ("Black", Color::rgba(0.0, 0.0, 0.0, 1.0)),
];

#[derive(Debug, Clone, Copy, PartialEq)]
enum Cmd {
    Copy,
    Save,
    SaveAs,
    Pin,
    CopyText,
    AddCapture,
    Paste,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Zoom {
    Actual,
    Fit,
    In,
    Out,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Btn {
    Cmd(u8),
    Grip,
    Tool(Tool),
    Style,
    Add,
    More,
    Size,
    Min,
    Max,
    Close,
}

struct TextEdit {
    ann: Annotation,
    is_new: bool,
    field: TextField,
}

pub struct Editor {
    pub hwnd: HWND,
    pub doc: u64,
    surface: Option<Surface>,
    scale: f32,
    tool: Tool,
    color: Color,
    stroke: f32,
    annotations: Vec<Annotation>,
    undo: Vec<Vec<Annotation>>,
    redo: Vec<Vec<Annotation>>,
    draft: Option<Annotation>,
    selected: Option<u64>,
    drag_start: Option<Pt>,
    drag_moved: bool,
    hover_color: Option<Color>,
    flash: Option<(String, f64)>,
    text: Option<TextEdit>,
    fixed_zoom: Option<f32>,
    zoom: f32,
    scroll: Pt,
    hover: Option<Btn>,
    pressed: Option<Btn>,
    pointer: Option<Pt>,
    panning: Option<(Pt, Pt)>,
    scroller_until: f64,
    pub dirty: bool,
    hidden: bool,
}

impl Editor {
    pub fn device_lost(&mut self) {
        self.surface = None;
        self.dirty = true;
    }
}

fn layout_bar(e: &Editor, w: f32) -> Vec<(Btn, Rect)> {
    let mut v = Vec::new();
    let mut x = 12.0;
    let y = (BAR_H - 28.0) / 2.0;
    let mut place = |b: Btn, wd: f32, x: &mut f32| {
        v.push((b, rect(*x, y, wd, 28.0)));
        *x += wd + 2.0;
    };
    place(Btn::Cmd(0), 30.0, &mut x);
    place(Btn::Cmd(1), 30.0, &mut x);
    place(Btn::Cmd(2), 30.0, &mut x);
    x += 11.0;
    place(Btn::Grip, 34.0, &mut x);
    x += 11.0;
    for t in Tool::BAR {
        place(Btn::Tool(t), 30.0, &mut x);
    }
    place(Btn::Style, 30.0, &mut x);
    place(Btn::Add, 30.0, &mut x);
    place(Btn::Tool(Tool::Blur), 30.0, &mut x);
    place(Btn::More, 30.0, &mut x);
    let _ = e;
    v.push((Btn::Close, rect(w - CAPTION_W, 0.0, CAPTION_W, BAR_H)));
    v.push((Btn::Max, rect(w - CAPTION_W * 2.0, 0.0, CAPTION_W, BAR_H)));
    v.push((Btn::Min, rect(w - CAPTION_W * 3.0, 0.0, CAPTION_W, BAR_H)));
    v
}

fn separators() -> [f32; 2] {
    // After the three commands, and after the grip.
    let a = 12.0 + 3.0 * 32.0 + 4.0;
    let b = a + 11.0 - 4.0 + 36.0 + 4.0;
    [a, b]
}

fn client_size(e: &Editor) -> (f32, f32) {
    let mut r = RECT::default();
    unsafe {
        let _ = GetClientRect(e.hwnd, &mut r);
    }
    ((r.right - r.left) as f32 / e.scale, (r.bottom - r.top) as f32 / e.scale)
}

fn tip_for(b: Btn) -> String {
    match b {
        Btn::Cmd(0) => "Copy  Ctrl+C".into(),
        Btn::Cmd(1) => "Save to Vault  Ctrl+S".into(),
        Btn::Cmd(_) => "Pin to Screen".into(),
        Btn::Grip => "Drag into any app".into(),
        Btn::Tool(t) => format!("{}  {}", t.title(), t.key()),
        Btn::Style => "Colour & thickness".into(),
        Btn::Add => "Add Capture  Ctrl+A".into(),
        Btn::More => "More tools".into(),
        Btn::Size => "Click: toggle 100% / fit  ·  Ctrl+0 actual size, Ctrl+9 fit, Ctrl++ Ctrl+− zoom".into(),
        Btn::Min => "Minimize".into(),
        Btn::Max => "Maximize".into(),
        Btn::Close => "Close".into(),
    }
}

// ---------------------------------------------------------------------------
// Open / close
// ---------------------------------------------------------------------------

unsafe extern "system" fn editor_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCALCSIZE && w.0 != 0 {
            // Keep the native side/bottom resize borders; drop the caption.
            let params = &mut *(l.0 as *mut NCCALCSIZE_PARAMS);
            let top = params.rgrc[0].top;
            let r = DefWindowProcW(hwnd, msg, w, l);
            params.rgrc[0].top = top;
            if IsZoomed(hwnd).as_bool() {
                let dpi = windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd);
                let f = windows::Win32::UI::HiDpi::GetSystemMetricsForDpi(SM_CYFRAME, dpi)
                    + windows::Win32::UI::HiDpi::GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
                params.rgrc[0].top += f;
            }
            return r;
        }
        let r = app::with(|a| handle(a, hwnd, msg, w, l));
        ui::drain();
        match r {
            Some(Some(v)) => v,
            _ => DefWindowProcW(hwnd, msg, w, l),
        }
    }
}

pub fn open(app: &mut App, doc: u64) {
    if let Some(e) = app.captures.editors.iter().find(|e| e.doc == doc) {
        let h = e.hwnd;
        ui::defer(move || unsafe {
            if IsIconic(h).as_bool() {
                let _ = ShowWindow(h, SW_RESTORE);
            }
            let _ = SetForegroundWindow(h);
        });
        return;
    }
    let Some(d) = app.captures.docs.get(&doc) else { return };
    let (cx, cy) = monitors::cursor();
    let Some(mon) = monitors::at_point(cx, cy) else { return };
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(editor_proc),
            hInstance: hinst.into(),
            lpszClassName: w!("ImageVault.Editor"),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: LoadIconW(Some(hinst.into()), windows::core::PCWSTR(1 as *const u16)).unwrap_or_default(),
            style: CS_DBLCLKS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
    }
    let scale = mon.scale;
    let v = mon.work;
    let (vw, vh) = ((v.right - v.left) as f32 / scale, (v.bottom - v.top) as f32 / scale);
    let (dw, dh) = d.size();
    // Open at 100%: as big as the image needs, up to the screen.
    let w = vw.min(MIN_W.max(dw + PAD * 2.0));
    let h = vh.min((300.0f32).max(dh + PAD * 2.0 + BAR_H));
    let (pw, ph) = ((w * scale) as i32, (h * scale) as i32);
    let x = (v.left + v.right) / 2 - pw / 2;
    let y = (v.top + v.bottom) / 2 - ph / 2;
    let hwnd = unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_APPWINDOW,
            w!("ImageVault.Editor"),
            w!("Edit Screenshot"),
            WS_OVERLAPPEDWINDOW,
            x,
            y,
            pw,
            ph,
            None,
            None,
            Some(hinst.into()),
            None,
        )
        .unwrap_or_default()
    };
    unsafe {
        let on: i32 = 1;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &on as *const _ as *const _, 4);
        let round = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &round as *const _ as *const _, 4);
        let _ = SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER);
    }
    crate::menu::allow_dark(hwnd);
    app.captures.editors.push(Editor {
        hwnd,
        doc,
        surface: None,
        scale,
        tool: Tool::Arrow,
        color: COLORS[0].1,
        stroke: 4.0,
        annotations: d.annotations.clone(),
        undo: Vec::new(),
        redo: Vec::new(),
        draft: None,
        selected: None,
        drag_start: None,
        drag_moved: false,
        hover_color: None,
        flash: None,
        text: None,
        fixed_zoom: Some(1.0),
        zoom: 1.0,
        scroll: Pt::default(),
        hover: None,
        pressed: None,
        pointer: None,
        panning: None,
        scroller_until: 0.0,
        dirty: true,
        hidden: false,
    });
    let i = app.captures.editors.len() - 1;
    relayout(app, i);
    render(app, i);
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
}

fn close(app: &mut App, i: usize) {
    commit_text(app, i);
    if i < app.captures.editors.len() {
        let e = app.captures.editors.remove(i);
        unsafe {
            let _ = DestroyWindow(e.hwnd);
        }
        app.tip.dismiss();
    }
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

fn doc_size(app: &App, e: &Editor) -> (f32, f32) {
    app.captures.docs.get(&e.doc).map(|d| d.size()).unwrap_or((1.0, 1.0))
}

fn view_rect(e: &Editor) -> Rect {
    let (w, h) = client_size(e);
    rect(0.0, BAR_H, w, (h - BAR_H).max(1.0))
}

fn content_size(app: &App, e: &Editor) -> (f32, f32) {
    let v = view_rect(e);
    let (dw, dh) = doc_size(app, e);
    (v.w.max((dw * e.zoom + PAD * 2.0).ceil()), v.h.max((dh * e.zoom + PAD * 2.0).ceil()))
}

/// The image at the current zoom, centred in the content, aligned to pixels.
fn image_rect(app: &App, e: &Editor) -> Rect {
    let v = view_rect(e);
    let (cw, ch) = content_size(app, e);
    let (dw, dh) = doc_size(app, e);
    let (w, h) = (dw * e.zoom, dh * e.zoom);
    let s = e.scale;
    let x = ((v.x + cw / 2.0 - w / 2.0 - e.scroll.x) * s).round() / s;
    let y = ((v.y + ch / 2.0 - h / 2.0 - e.scroll.y) * s).round() / s;
    rect(x, y, (w * s).round() / s, (h * s).round() / s)
}

fn clamp_scroll(app: &App, e: &mut Editor) {
    let v = view_rect(e);
    let (cw, ch) = content_size(app, e);
    e.scroll.x = e.scroll.x.clamp(0.0, (cw - v.w).max(0.0));
    e.scroll.y = e.scroll.y.clamp(0.0, (ch - v.h).max(0.0));
}

fn relayout(app: &mut App, i: usize) {
    let (dw, dh) = doc_size(app, &app.captures.editors[i]);
    let e = &mut app.captures.editors[i];
    let v = view_rect(e);
    let fit = (0.05f32).max((1.0f32).min((v.w - PAD * 2.0) / dw).min((v.h - PAD * 2.0) / dh));
    let new_zoom = e.fixed_zoom.unwrap_or(fit);
    if (new_zoom - e.zoom).abs() > 0.0001 {
        // Keep the view centred on the same image point.
        let ir = image_rect_of(app, i);
        let e = &app.captures.editors[i];
        let anchor = pt((v.cx() - ir.x) / e.zoom.max(0.001), (v.cy() - ir.y) / e.zoom.max(0.001));
        let e = &mut app.captures.editors[i];
        e.zoom = new_zoom;
        e.scroll = Pt::default();
        let ir = image_rect_of(app, i);
        let e = &mut app.captures.editors[i];
        e.scroll = pt(ir.x + anchor.x * e.zoom - v.cx(), ir.y + anchor.y * e.zoom - v.cy());
    }
    let mut e = std::mem::replace(&mut app.captures.editors[i], placeholder());
    clamp_scroll(app, &mut e);
    app.captures.editors[i] = e;
    app.captures.editors[i].dirty = true;
    app.request_frame();
}

fn image_rect_of(app: &App, i: usize) -> Rect {
    image_rect(app, &app.captures.editors[i])
}

fn placeholder() -> Editor {
    Editor {
        hwnd: HWND::default(),
        doc: 0,
        surface: None,
        scale: 1.0,
        tool: Tool::Arrow,
        color: COLORS[0].1,
        stroke: 4.0,
        annotations: Vec::new(),
        undo: Vec::new(),
        redo: Vec::new(),
        draft: None,
        selected: None,
        drag_start: None,
        drag_moved: false,
        hover_color: None,
        flash: None,
        text: None,
        fixed_zoom: None,
        zoom: 1.0,
        scroll: Pt::default(),
        hover: None,
        pressed: None,
        pointer: None,
        panning: None,
        scroller_until: 0.0,
        dirty: false,
        hidden: false,
    }
}

const STEPS: [f32; 13] = [0.1, 0.25, 0.33, 0.5, 0.67, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0];

fn zoom(app: &mut App, i: usize, z: Zoom) {
    commit_text(app, i);
    let e = &mut app.captures.editors[i];
    let cur = e.zoom;
    e.fixed_zoom = match z {
        Zoom::Actual => Some(1.0),
        Zoom::Fit => None,
        Zoom::In => Some(STEPS.iter().copied().find(|s| *s > cur + 0.001).unwrap_or(8.0)),
        Zoom::Out => Some(STEPS.iter().rev().copied().find(|s| *s < cur - 0.001).unwrap_or(0.1)),
    };
    relayout(app, i);
}

fn to_image(app: &App, e: &Editor, p: Pt) -> Pt {
    let r = image_rect(app, e);
    pt((p.x - r.x) / e.zoom, (p.y - r.y) / e.zoom)
}

fn to_view(app: &App, e: &Editor, p: Pt) -> Pt {
    let r = image_rect(app, e);
    pt(r.x + p.x * e.zoom, r.y + p.y * e.zoom)
}

fn clamp_img(app: &App, e: &Editor, p: Pt) -> Pt {
    let (w, h) = doc_size(app, e);
    pt(p.x.clamp(0.0, w), p.y.clamp(0.0, h))
}

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

fn push_undo(e: &mut Editor) {
    e.undo.push(e.annotations.clone());
    if e.undo.len() > 200 {
        e.undo.remove(0);
    }
    e.redo.clear();
}

fn save_doc(app: &mut App, i: usize) {
    let e = &mut app.captures.editors[i];
    e.dirty = true;
    let (id, anns) = (e.doc, e.annotations.clone());
    if let Some(d) = app.captures.docs.get_mut(&id) {
        d.set_annotations(anns);
    }
    app.captures.doc_changed(id);
    app.request_frame();
}

fn commit(app: &mut App, i: usize, a: Annotation) {
    let e = &mut app.captures.editors[i];
    push_undo(e);
    e.annotations.push(a);
    save_doc(app, i);
}

fn undo(app: &mut App, i: usize) {
    commit_text(app, i);
    let e = &mut app.captures.editors[i];
    let Some(prev) = e.undo.pop() else {
        util::beep();
        return;
    };
    e.redo.push(std::mem::replace(&mut e.annotations, prev));
    e.selected = None;
    save_doc(app, i);
}

fn redo(app: &mut App, i: usize) {
    let e = &mut app.captures.editors[i];
    let Some(next) = e.redo.pop() else {
        util::beep();
        return;
    };
    e.undo.push(std::mem::replace(&mut e.annotations, next));
    e.selected = None;
    save_doc(app, i);
}

fn delete_selection(app: &mut App, i: usize) {
    let e = &mut app.captures.editors[i];
    let Some(id) = e.selected else { return };
    push_undo(e);
    e.annotations.retain(|a| a.id != id);
    e.selected = None;
    save_doc(app, i);
}

fn reorder(app: &mut App, i: usize, front: bool) {
    let e = &mut app.captures.editors[i];
    let Some(id) = e.selected else { return };
    let Some(k) = e.annotations.iter().position(|a| a.id == id) else { return };
    push_undo(e);
    let a = e.annotations.remove(k);
    if front {
        e.annotations.push(a);
    } else {
        e.annotations.insert(0, a);
    }
    save_doc(app, i);
}

pub fn font_size_for(width: f32) -> f32 {
    if width <= 2.0 {
        16.0
    } else if width <= 4.0 {
        22.0
    } else {
        32.0
    }
}

fn set_color(app: &mut App, i: usize, c: Color) {
    let e = &mut app.captures.editors[i];
    e.color = c;
    if let Some(id) = e.selected {
        if let Some(k) = e.annotations.iter().position(|a| a.id == id) {
            if !matches!(e.annotations[k].kind, Kind::Blur | Kind::Spotlight | Kind::Image) {
                push_undo(e);
                e.annotations[k].color = c;
                save_doc(app, i);
            }
        }
    }
    let e = &mut app.captures.editors[i];
    if let Some(t) = &mut e.text {
        t.ann.color = c;
    }
    e.dirty = true;
    app.request_frame();
}

fn set_width(app: &mut App, i: usize, wd: f32) {
    let e = &mut app.captures.editors[i];
    e.stroke = wd;
    if let Some(id) = e.selected {
        if let Some(k) = e.annotations.iter().position(|a| a.id == id) {
            push_undo(e);
            e.annotations[k].width = wd;
            if e.annotations[k].kind == Kind::Text {
                e.annotations[k].font_size = font_size_for(wd);
            }
            save_doc(app, i);
        }
    }
    app.captures.editors[i].dirty = true;
    app.request_frame();
}

fn set_tool(app: &mut App, i: usize, t: Tool) {
    let e = &app.captures.editors[i];
    if e.tool != t {
        commit_text(app, i);
        let e = &mut app.captures.editors[i];
        if t != Tool::Select {
            e.selected = None;
        }
        e.tool = t;
    }
    app.captures.editors[i].dirty = true;
    app.request_frame();
}

/// Add Capture / Paste Image: a movable picture, centred, scaled to fit.
fn insert(app: &mut App, i: usize, raw: crate::imaging::Raw, size: (f32, f32)) {
    let (sw, sh) = doc_size(app, &app.captures.editors[i]);
    let k = (1.0f32).min(sw * 0.8 / size.0.max(1.0)).min(sh * 0.8 / size.1.max(1.0));
    let (w, h) = (size.0 * k, size.1 * k);
    let o = pt((sw - w) / 2.0, (sh - h) / 2.0);
    let mut a = Annotation::new(Kind::Image, vec![o, pt(o.x + w, o.y + h)], Color::white(1.0), 0.0);
    let scale = raw.w as f32 / size.0.max(1.0);
    a.image = Some(std::rc::Rc::new((raw, scale)));
    let id = a.id;
    commit(app, i, a);
    set_tool(app, i, Tool::Select);
    app.captures.editors[i].selected = Some(id);
}

fn paste_image(app: &mut App, i: usize) {
    match clipboard::read_image() {
        Some(raw) => {
            let s = app.captures.docs.get(&app.captures.editors[i].doc).map(|d| d.scale).unwrap_or(1.0);
            let size = (raw.w as f32 / s, raw.h as f32 / s);
            insert(app, i, raw, size);
        }
        None => {
            flash(app, i, "No image on the clipboard");
            util::beep();
        }
    }
}

fn flash(app: &mut App, i: usize, text: &str) {
    let e = &mut app.captures.editors[i];
    e.flash = Some((text.to_string(), util::mono() + 1.4));
    e.dirty = true;
    let hwnd = e.hwnd;
    ui::after(1.45, move || {
        app::with(|a| {
            if let Some(e) = a.captures.editors.iter_mut().find(|e| e.hwnd == hwnd) {
                e.dirty = true;
            }
            a.request_frame();
        });
    });
    app.request_frame();
}

fn run(app: &mut App, i: usize, c: Cmd) {
    commit_text(app, i);
    let (id, hwnd) = (app.captures.editors[i].doc, app.captures.editors[i].hwnd);
    match c {
        Cmd::Copy => {
            capture::copy(app, id);
            flash(app, i, "Copied");
        }
        Cmd::Save => {
            capture::save_to_vault(app, id, None);
            flash(app, i, "Saved to Vault");
        }
        Cmd::SaveAs => capture::save_as(app, id, hwnd),
        Cmd::CopyText => capture::copy_text(app, id),
        Cmd::Pin => {
            let mut r = RECT::default();
            unsafe {
                let _ = GetWindowRect(hwnd, &mut r);
            }
            capture::pin(app, id, Some(r));
            if let Some(j) = app.captures.editors.iter().position(|e| e.hwnd == hwnd) {
                close(app, j);
            }
        }
        Cmd::AddCapture => {
            app.captures.editors[i].hidden = true;
            unsafe {
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            let present = move || unsafe {
                let _ = ShowWindow(hwnd, SW_SHOW);
                let _ = SetForegroundWindow(hwnd);
                app::with(|a| {
                    if let Some(e) = a.captures.editors.iter_mut().find(|e| e.hwnd == hwnd) {
                        e.hidden = false;
                        e.dirty = true;
                    }
                    a.request_frame();
                });
            };
            ui::defer(move || {
                crate::screenocr::pick_image(
                    Box::new(move |img, scale, _mon| {
                        let size = (img.w as f32 / scale, img.h as f32 / scale);
                        app::with(|a| {
                            if let Some(j) = a.captures.editors.iter().position(|e| e.hwnd == hwnd) {
                                insert(a, j, img, size);
                            }
                        });
                        present();
                    }),
                    Box::new(present),
                );
            });
        }
        Cmd::Paste => paste_image(app, i),
        Cmd::Close => close(app, i),
    }
}

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

fn begin_text(app: &mut App, i: usize, a: Annotation, is_new: bool) {
    commit_text(app, i);
    let e = &mut app.captures.editors[i];
    let mut field = TextField::new();
    field.set(&a.text);
    field.blink_epoch = util::mono();
    e.selected = None;
    e.text = Some(TextEdit { ann: a, is_new, field });
    e.dirty = true;
    blink(app, e_hwnd(app, i));
    app.request_frame();
}

fn e_hwnd(app: &App, i: usize) -> HWND {
    app.captures.editors[i].hwnd
}

fn blink(_app: &mut App, hwnd: HWND) {
    ui::after(0.5, move || {
        app::with(|a| {
            if let Some(e) = a.captures.editors.iter_mut().find(|e| e.hwnd == hwnd) {
                if e.text.is_some() {
                    e.dirty = true;
                    a.request_frame();
                    blink(a, hwnd);
                }
            }
        });
    });
}

pub fn commit_text(app: &mut App, i: usize) {
    let Some(e) = app.captures.editors.get_mut(i) else { return };
    let Some(t) = e.text.take() else { return };
    let text = t.field.text.trim().to_string();
    let mut a = t.ann;
    a.text = text.clone();
    if t.is_new {
        if !text.is_empty() {
            commit(app, i, a);
        }
    } else if let Some(k) = e.annotations.iter().position(|x| x.id == a.id) {
        push_undo(e);
        if text.is_empty() {
            e.annotations.remove(k);
        } else {
            e.annotations[k] = a;
        }
        save_doc(app, i);
    }
    if let Some(e) = app.captures.editors.get_mut(i) {
        e.dirty = true;
    }
    app.request_frame();
}

fn text_field_rect(app: &App, e: &Editor, t: &TextEdit) -> Rect {
    let o = to_view(app, e, t.ann.points[0]);
    let fs = t.ann.font_size * e.zoom;
    let h = (fs * 1.3).ceil();
    let tw = crate::gfx::gfx().measure(if t.field.text.is_empty() { " " } else { &t.field.text }, &Font::sans(fs, Weight::Bold)).0;
    rect(o.x - 2.0, o.y, (160.0f32).max(tw + 40.0), h)
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

pub fn frame(app: &mut App) {
    for i in 0..app.captures.editors.len() {
        if app.captures.editors[i].dirty && !app.captures.editors[i].hidden {
            app.captures.editors[i].dirty = false;
            render(app, i);
        }
    }
}

pub fn animating(app: &App) -> bool {
    app.captures.editors.iter().any(|e| e.dirty)
}

fn render(app: &mut App, i: usize) {
    let (cw, ch) = client_size(&app.captures.editors[i]);
    let scale = app.captures.editors[i].scale;
    let (pw, ph) = ((cw * scale).round().max(1.0) as u32, (ch * scale).round().max(1.0) as u32);
    {
        let e = &mut app.captures.editors[i];
        match &mut e.surface {
            Some(s) if s.generation == crate::gfx::generation() => {
                let _ = s.resize(pw, ph, scale);
            }
            _ => e.surface = Surface::new(e.hwnd, pw, ph, scale).ok(),
        }
    }
    let Some(mut surface) = app.captures.editors[i].surface.take() else { return };
    // Pre-compute the document size before borrowing it inside the draw.
    let ok = surface.draw(|cv| draw(cv, app, i, cw, ch));
    app.captures.editors[i].surface = Some(surface);
    if !ok {
        app.captures.editors[i].surface = None;
        app.captures.editors[i].dirty = true;
    }
}

fn draw(cv: &mut Canvas, app: &mut App, i: usize, w: f32, h: f32) {
    let bg = Color::gray(0.07, 1.0);
    cv.fill_rect(rect(0.0, 0.0, w, h), bg);
    let ir = image_rect(app, &app.captures.editors[i]);
    let v = view_rect(&app.captures.editors[i]);
    // Canvas.
    cv.clip(v);
    let ir2 = ir;
    cv.shadowed(9.0, pt(0.0, 4.0), Color::black(0.6), true, |cv| cv.fill_rect(ir2, Color::black(1.0)));
    {
        let e = &app.captures.editors[i];
        let editing = e.text.as_ref().filter(|t| !t.is_new).map(|t| t.ann.id);
        let mut items: Vec<Annotation> = e.annotations.iter().filter(|a| Some(a.id) != editing).cloned().collect();
        if let Some(d) = &e.draft {
            items.push(d.clone());
        }
        let zoom = e.zoom;
        let doc_id = e.doc;
        if let Some(doc) = app.captures.docs.get_mut(&doc_id) {
            cv.clip(ir);
            cv.save();
            cv.translate(ir.x, ir.y);
            cv.transform(crate::gfx::scale_about(zoom, 0.0, 0.0));
            // At 100% the capture lands pixel-for-pixel; never smooth it.
            capture::draw_doc(cv, doc, &items, (zoom - 1.0).abs() > 0.001);
            cv.restore();
            cv.unclip();
        }
    }
    // Selection outline.
    let e = &app.captures.editors[i];
    if let Some(id) = e.selected {
        if let Some(a) = e.annotations.iter().find(|a| a.id == id) {
            let b = a.bounds(&cv.g);
            let tl = to_view(app, e, pt(b.x, b.y));
            let br = to_view(app, e, pt(b.r(), b.b()));
            let r = Rect::from_points(tl, br).inset(-4.0, -4.0);
            let mut p = cv.g.path();
            p.round_rect(r, 3.0);
            let g: ID2D1Geometry = p.finish().cast().unwrap();
            cv.stroke_geom(&g, 1.0, Color::black(0.6), 0);
            cv.stroke_geom_dashed(&g, 1.0, (4.0, 3.0), 0.0, Color::white(1.0));
        }
    }
    // Inline text field.
    let fr = app.captures.editors[i].text.as_ref().map(|t| text_field_rect(app, &app.captures.editors[i], t));
    let zoom_ = app.captures.editors[i].zoom;
    if let (Some(fr), Some(t)) = (fr, app.captures.editors[i].text.as_mut()) {
        let f = Font::sans(t.ann.font_size * zoom_, Weight::Bold);
        let color = t.ann.color;
        let mut field = std::mem::take(&mut t.field);
        field.draw(cv, &f, color, fr, true, "Type…", Color::white(0.35));
        if let Some(t) = app.captures.editors[i].text.as_mut() {
            t.field = field;
        }
    }
    // Overlay scrollers.
    let e = &app.captures.editors[i];
    let (cw, ch) = content_size(app, e);
    let show = util::mono() < e.scroller_until;
    if show {
        if ch > v.h + 0.5 {
            let kh = (v.h * v.h / ch).max(24.0);
            let ky = v.y + (v.h - kh) * (e.scroll.y / (ch - v.h));
            cv.fill_capsule(rect(v.r() - 8.0, ky + 2.0, 5.0, kh - 4.0), Color::white(0.45));
        }
        if cw > v.w + 0.5 {
            let kw = (v.w * v.w / cw).max(24.0);
            let kx = v.x + (v.w - kw) * (e.scroll.x / (cw - v.w));
            cv.fill_capsule(rect(kx + 2.0, v.b() - 8.0, kw - 4.0, 5.0), Color::white(0.45));
        }
    }
    cv.unclip();
    toolbar(cv, app, i, w);
}

fn toolbar(cv: &mut Canvas, app: &mut App, i: usize, w: f32) {
    let (dw, dh) = doc_size(app, &app.captures.editors[i]);
    let e = &app.captures.editors[i];
    cv.fill_rect(rect(0.0, 0.0, w, BAR_H), Color::gray(0.12, 1.0));
    cv.fill_rect(rect(0.0, BAR_H - 1.0, w, 1.0), Color::white(0.08));
    for x in separators() {
        cv.fill_rect(rect(x, 11.0, 1.0, BAR_H - 22.0), Color::white(0.14));
    }
    let in_menu = Tool::MENU.contains(&e.tool) && e.tool != Tool::Blur;
    for (b, r) in layout_bar(e, w) {
        let hovered = e.hover == Some(b);
        let pressed = e.pressed == Some(b);
        match b {
            Btn::Cmd(k) => {
                let icon = [Icon::DocOnDoc, Icon::SquareAndArrowDown, Icon::Pin][k as usize];
                capture::draw_hud(cv, r, icon, 13.0, hovered, false, pressed, false);
            }
            Btn::Grip => {
                for row in 0..2 {
                    for k in 0..4 {
                        cv.fill_circle(pt(r.cx() - 9.0 + k as f32 * 6.0, r.cy() - 2.0 + row as f32 * 4.0), 1.0, Color::white(0.45));
                    }
                }
            }
            Btn::Tool(t) => capture::draw_hud(cv, r, t.icon(), 13.0, hovered, e.tool == t, pressed, false),
            Btn::Style => {
                if hovered {
                    cv.fill_rrect(r, 6.0, Color::white(0.1));
                }
                let d = 8.0 + e.stroke * 1.2;
                cv.fill_circle(r.center(), d / 2.0, e.color);
                cv.stroke_circle(r.center(), d / 2.0 + 2.5, 1.0, Color::white(0.45));
            }
            Btn::Add => capture::draw_hud(cv, r, Icon::PlusViewfinder, 13.0, hovered, false, pressed, false),
            Btn::More => {
                let icon = if in_menu { e.tool.icon() } else { Icon::Ellipsis };
                capture::draw_hud(cv, r, icon, 13.0, hovered, in_menu, pressed, false);
            }
            Btn::Min | Btn::Max | Btn::Close => {
                let close = b == Btn::Close;
                if hovered {
                    cv.fill_rect(r, if close { Color::rgba(0.77, 0.17, 0.11, 1.0) } else { Color::white(0.08) });
                }
                let c = r.center();
                let col = Color::white(0.9);
                match b {
                    Btn::Min => cv.line(pt(c.x - 5.0, c.y), pt(c.x + 5.0, c.y), 1.0, col, 0),
                    Btn::Max => {
                        let zoomed = unsafe { IsZoomed(e.hwnd).as_bool() };
                        if zoomed {
                            cv.stroke_rrect(rect(c.x - 5.0, c.y - 3.0, 8.0, 8.0), 1.0, 1.0, col);
                            cv.line(pt(c.x - 3.0, c.y - 5.0), pt(c.x + 5.0, c.y - 5.0), 1.0, col, 0);
                            cv.line(pt(c.x + 5.0, c.y - 5.0), pt(c.x + 5.0, c.y + 3.0), 1.0, col, 0);
                        } else {
                            cv.stroke_rrect(rect(c.x - 5.0, c.y - 5.0, 10.0, 10.0), 1.0, 1.0, col);
                        }
                    }
                    _ => {
                        cv.line(pt(c.x - 5.0, c.y - 5.0), pt(c.x + 5.0, c.y + 5.0), 1.0, col, 0);
                        cv.line(pt(c.x + 5.0, c.y - 5.0), pt(c.x - 5.0, c.y + 5.0), 1.0, col, 0);
                    }
                }
            }
            Btn::Size => {}
        }
    }
    // Right info: swatch + hex / hint | divider | size / zoom.
    let info_r = w - CAPTION_W * 3.0 - 14.0;
    let lf = Font::mono(11.0, Weight::Semibold);
    let hf = Font::sans(9.5, Weight::Regular);
    let size_label = format!("{}\u{d7}{}pt", dw.round() as i64, dh.round() as i64);
    let size_hint = format!("Image size · {}%", (e.zoom * 100.0).round() as i64);
    let sw = cv.measure(&size_label, &lf).0.max(cv.measure(&size_hint, &hf).0);
    let lh = cv.g.line_height(&lf);
    let hh = cv.g.line_height(&hf);
    let y0 = BAR_H / 2.0 - (lh + hh) / 2.0;
    let sx = info_r - sw;
    cv.text(&size_label, &lf, Color::white(1.0), rect(sx, y0, sw + 2.0, lh), Align::Leading, Trim::None);
    cv.text(&size_hint, &hf, crate::theme::MUTED, rect(sx, y0 + lh, sw + 2.0, hh), Align::Leading, Trim::None);
    let div_x = sx - 10.0 - 1.0;
    cv.fill_rect(rect(div_x, BAR_H / 2.0 - 11.0, 1.0, 22.0), Color::white(0.14));
    let now = util::mono();
    let (hex, hint, swatch) = match &e.flash {
        Some((t, until)) if now < *until => (t.clone(), " ".to_string(), Color::gray(0.2, 1.0)),
        _ => match e.hover_color {
            Some(c) => (c.hex(), "Tab to copy".to_string(), c),
            None => ("—".to_string(), "Hover the image".to_string(), Color::gray(0.2, 1.0)),
        },
    };
    let hw = cv.measure(&hex, &lf).0.max(66.0).max(cv.measure(&hint, &hf).0);
    let hx = div_x - 10.0 - hw;
    cv.text(&hex, &lf, Color::white(1.0), rect(hx, y0, hw + 2.0, lh), Align::Leading, Trim::None);
    cv.text(&hint, &hf, crate::theme::MUTED, rect(hx, y0 + lh, hw + 2.0, hh), Align::Leading, Trim::None);
    let sr = rect(hx - 10.0 - 16.0, BAR_H / 2.0 - 8.0, 16.0, 16.0);
    cv.fill_circle(sr.center(), 8.0, swatch);
    cv.stroke_circle(sr.center(), 7.5, 1.0, Color::white(0.25));
}

fn size_rect(app: &App, e: &Editor) -> Rect {
    let (w, _) = client_size(e);
    let (dw, dh) = doc_size(app, e);
    let g = crate::gfx::gfx();
    let lf = Font::mono(11.0, Weight::Semibold);
    let hf = Font::sans(9.5, Weight::Regular);
    let sw = g.measure(&format!("{}\u{d7}{}pt", dw.round() as i64, dh.round() as i64), &lf).0.max(g.measure(&format!("Image size · {}%", (e.zoom * 100.0).round() as i64), &hf).0);
    let info_r = w - CAPTION_W * 3.0 - 14.0;
    rect(info_r - sw, 4.0, sw, BAR_H - 8.0)
}

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

fn btn_at(app: &App, e: &Editor, p: Pt) -> Option<Btn> {
    if p.y > BAR_H {
        return None;
    }
    let (w, _) = client_size(e);
    if let Some((b, _)) = layout_bar(e, w).into_iter().find(|(_, r)| r.contains(p)) {
        return Some(b);
    }
    if size_rect(app, e).contains(p) {
        return Some(Btn::Size);
    }
    None
}

fn index(app: &App, hwnd: HWND) -> Option<usize> {
    app.captures.editors.iter().position(|e| e.hwnd == hwnd)
}

fn constrained(a: Pt, b: Pt, kind: Kind) -> Pt {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    match kind {
        Kind::Arrow | Kind::Line | Kind::Measure => {
            let q = std::f32::consts::FRAC_PI_4;
            let ang = (dy.atan2(dx) / q).round() * q;
            let len = (dx * dx + dy * dy).sqrt();
            pt(a.x + len * ang.cos(), a.y + len * ang.sin())
        }
        _ => {
            let side = dx.abs().max(dy.abs());
            pt(a.x + if dx < 0.0 { -side } else { side }, a.y + if dy < 0.0 { -side } else { side })
        }
    }
}

fn mods() -> (bool, bool, bool) {
    unsafe { (GetKeyState(VK_CONTROL.0 as i32) < 0, GetKeyState(VK_SHIFT.0 as i32) < 0, GetKeyState(VK_MENU.0 as i32) < 0) }
}

fn handle(app: &mut App, hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> Option<LRESULT> {
    let i = index(app, hwnd)?;
    let scale = app.captures.editors[i].scale;
    let (x, y) = util::lparam_xy(l);
    let local = pt(x as f32 / scale, y as f32 / scale);
    match msg {
        WM_NCHITTEST => {
            let r = unsafe { DefWindowProcW(hwnd, msg, w, l) };
            if r.0 != HTCLIENT as isize {
                return Some(r);
            }
            let mut p = POINT { x, y };
            unsafe {
                let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut p);
            }
            let lp = pt(p.x as f32 / scale, p.y as f32 / scale);
            let zoomed = unsafe { IsZoomed(hwnd).as_bool() };
            if lp.y < 5.0 && !zoomed {
                return Some(LRESULT(HTTOP as isize));
            }
            let e = &app.captures.editors[i];
            match btn_at(app, e, lp) {
                Some(Btn::Min) => Some(LRESULT(HTMINBUTTON as isize)),
                Some(Btn::Max) => Some(LRESULT(HTMAXBUTTON as isize)),
                Some(Btn::Close) => Some(LRESULT(HTCLOSE as isize)),
                Some(_) => Some(LRESULT(HTCLIENT as isize)),
                None if lp.y <= BAR_H => Some(LRESULT(HTCAPTION as isize)),
                None => Some(LRESULT(HTCLIENT as isize)),
            }
        }
        WM_NCMOUSEMOVE => {
            let hit = w.0 as u32;
            let b = match hit {
                HTMINBUTTON => Some(Btn::Min),
                HTMAXBUTTON => Some(Btn::Max),
                HTCLOSE => Some(Btn::Close),
                _ => None,
            };
            let e = &mut app.captures.editors[i];
            if e.hover != b {
                e.hover = b;
                e.dirty = true;
                app.request_frame();
            }
            unsafe {
                let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE | TME_NONCLIENT, hwndTrack: hwnd, dwHoverTime: 0 };
                let _ = TrackMouseEvent(&mut tme);
            }
            app.tip.hover(b.map(tip_for));
            if b.is_some() { Some(LRESULT(0)) } else { None }
        }
        WM_NCLBUTTONDOWN => match w.0 as u32 {
            HTMINBUTTON | HTMAXBUTTON | HTCLOSE => {
                let b = match w.0 as u32 {
                    HTMINBUTTON => Btn::Min,
                    HTMAXBUTTON => Btn::Max,
                    _ => Btn::Close,
                };
                app.captures.editors[i].pressed = Some(b);
                Some(LRESULT(0))
            }
            _ => {
                commit_text(app, i);
                None
            }
        },
        WM_NCLBUTTONUP => {
            let pressed = app.captures.editors[i].pressed.take();
            let hit = w.0 as u32;
            match (pressed, hit) {
                (Some(Btn::Min), HTMINBUTTON) => unsafe {
                    let _ = ShowWindow(hwnd, SW_MINIMIZE);
                },
                (Some(Btn::Max), HTMAXBUTTON) => unsafe {
                    let _ = ShowWindow(hwnd, if IsZoomed(hwnd).as_bool() { SW_RESTORE } else { SW_MAXIMIZE });
                },
                (Some(Btn::Close), HTCLOSE) => {
                    ui::defer(move || {
                        app::with(|a| {
                            if let Some(j) = index(a, hwnd) {
                                close(a, j);
                            }
                        });
                    });
                }
                _ => return None,
            }
            Some(LRESULT(0))
        }
        0x02A2 | 0x02A3 => {
            // (NC)MOUSELEAVE
            let e = &mut app.captures.editors[i];
            e.hover = None;
            if msg == 0x02A3 {
                e.pointer = None;
                e.hover_color = None;
            }
            e.dirty = true;
            app.tip.hover(None);
            app.request_frame();
            Some(LRESULT(0))
        }
        WM_MOUSEMOVE => {
            mouse_move(app, i, local);
            Some(LRESULT(0))
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            unsafe {
                SetCapture(hwnd);
            }
            app.tip.dismiss();
            mouse_down(app, i, local, msg == WM_LBUTTONDBLCLK);
            Some(LRESULT(0))
        }
        WM_LBUTTONUP => {
            unsafe {
                let _ = ReleaseCapture();
            }
            mouse_up(app, i, local);
            Some(LRESULT(0))
        }
        WM_MBUTTONDOWN => {
            unsafe {
                SetCapture(hwnd);
            }
            let s = app.captures.editors[i].scroll;
            app.captures.editors[i].panning = Some((local, s));
            Some(LRESULT(0))
        }
        WM_MBUTTONUP => {
            unsafe {
                let _ = ReleaseCapture();
            }
            app.captures.editors[i].panning = None;
            Some(LRESULT(0))
        }
        WM_RBUTTONUP => {
            right_click(app, i, local);
            Some(LRESULT(0))
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let delta = ((w.0 >> 16) & 0xffff) as i16 as f32;
            let (ctrl, shift, _) = mods();
            if ctrl && msg == WM_MOUSEWHEEL {
                // Pinch / Ctrl+wheel zoom (magnify).
                let e = &mut app.captures.editors[i];
                e.fixed_zoom = Some((e.zoom * (1.0 + delta / 120.0 * 0.1)).clamp(0.1, 8.0));
                relayout(app, i);
            } else {
                let e = &mut app.captures.editors[i];
                let step = delta / 120.0 * 60.0;
                if msg == WM_MOUSEHWHEEL || shift {
                    e.scroll.x += if msg == WM_MOUSEHWHEEL { step } else { -step };
                } else {
                    e.scroll.y -= step;
                }
                e.scroller_until = util::mono() + 0.9;
                let mut ed = std::mem::replace(&mut app.captures.editors[i], placeholder());
                clamp_scroll(app, &mut ed);
                ed.dirty = true;
                app.captures.editors[i] = ed;
                ui::after(1.0, move || {
                    app::with(|a| {
                        if let Some(j) = index(a, hwnd) {
                            a.captures.editors[j].dirty = true;
                        }
                        a.request_frame();
                    });
                });
                app.request_frame();
            }
            Some(LRESULT(0))
        }
        WM_SETCURSOR => {
            if (l.0 & 0xffff) as u32 != HTCLIENT {
                return None;
            }
            let e = &app.captures.editors[i];
            let cursor = match e.pointer {
                Some(p) if image_rect(app, e).contains(p) && p.y > BAR_H => match e.tool {
                    Tool::Select => IDC_ARROW,
                    Tool::Text => IDC_IBEAM,
                    _ => IDC_CROSS,
                },
                _ => IDC_ARROW,
            };
            unsafe {
                SetCursor(LoadCursorW(None, cursor).ok());
            }
            Some(LRESULT(1))
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            if key(app, i, w.0 as u16) { Some(LRESULT(0)) } else { None }
        }
        WM_CHAR => {
            if let Some(t) = app.captures.editors[i].text.as_mut() {
                if let Some(c) = char::from_u32(w.0 as u32) {
                    t.field.char(c);
                    app.captures.editors[i].dirty = true;
                    app.request_frame();
                }
                return Some(LRESULT(0));
            }
            Some(LRESULT(0))
        }
        WM_SIZE => {
            if app.captures.editors[i].surface.is_some() {
                relayout(app, i);
                render(app, i);
            }
            None
        }
        WM_GETMINMAXINFO => {
            let mm = unsafe { &mut *(l.0 as *mut MINMAXINFO) };
            mm.ptMinTrackSize = POINT { x: (MIN_W * scale) as i32, y: (260.0 * scale) as i32 };
            Some(LRESULT(0))
        }
        WM_DPICHANGED => {
            app.captures.editors[i].scale = (w.0 & 0xffff) as f32 / 96.0;
            let r = unsafe { &*(l.0 as *const RECT) };
            unsafe {
                let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            }
            relayout(app, i);
            Some(LRESULT(0))
        }
        WM_ACTIVATE => {
            app.captures.editors[i].dirty = true;
            app.request_frame();
            None
        }
        WM_CLOSE => {
            ui::defer(move || {
                app::with(|a| {
                    if let Some(j) = index(a, hwnd) {
                        close(a, j);
                    }
                });
            });
            Some(LRESULT(0))
        }
        _ => None,
    }
}

fn mouse_move(app: &mut App, i: usize, p: Pt) {
    let hwnd = app.captures.editors[i].hwnd;
    unsafe {
        let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
        let _ = TrackMouseEvent(&mut tme);
    }
    app.captures.editors[i].pointer = Some(p);
    let b = btn_at(app, &app.captures.editors[i], p);
    if app.captures.editors[i].hover != b {
        app.captures.editors[i].hover = b;
        app.captures.editors[i].dirty = true;
        app.request_frame();
    }
    app.tip.hover(b.map(tip_for));
    // Middle-button pan.
    if let Some((start, s0)) = app.captures.editors[i].panning {
        let e = &mut app.captures.editors[i];
        e.scroll = pt(s0.x - (p.x - start.x), s0.y - (p.y - start.y));
        let mut ed = std::mem::replace(&mut app.captures.editors[i], placeholder());
        clamp_scroll(app, &mut ed);
        ed.dirty = true;
        app.captures.editors[i] = ed;
        app.request_frame();
        return;
    }
    // Colour under the pointer.
    let e = &app.captures.editors[i];
    let ir = image_rect(app, e);
    let c = if ir.contains(p) && p.y > BAR_H {
        let ip = to_image(app, e, p);
        app.captures.docs.get(&e.doc).and_then(|d| d.color_at(ip))
    } else {
        None
    };
    if c != app.captures.editors[i].hover_color {
        app.captures.editors[i].hover_color = c;
        app.captures.editors[i].dirty = true;
        app.request_frame();
    }
    let down = unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) } < 0;
    if !down {
        return;
    }
    // Text selection drag.
    if let Some(t) = app.captures.editors[i].text.as_ref() {
        let fr = text_field_rect(app, &app.captures.editors[i], t);
        let zoom = app.captures.editors[i].zoom;
        if let Some(t) = app.captures.editors[i].text.as_mut() {
            let f = Font::sans(t.ann.font_size * zoom, Weight::Bold);
            t.field.mouse_drag(&crate::gfx::gfx(), &f, fr, p.x);
        }
        app.captures.editors[i].dirty = true;
        app.request_frame();
        return;
    }
    let e = &app.captures.editors[i];
    let ip = clamp_img(app, e, to_image(app, e, p));
    let zoom = e.zoom;
    let e = &mut app.captures.editors[i];
    if e.tool == Tool::Select {
        if let (Some(start), Some(id)) = (e.drag_start, e.selected) {
            if let Some(k) = e.annotations.iter().position(|a| a.id == id) {
                if !e.drag_moved {
                    push_undo(e);
                    e.drag_moved = true;
                }
                e.annotations[k].moved(ip.x - start.x, ip.y - start.y);
                e.drag_start = Some(ip);
                e.dirty = true;
                app.request_frame();
            }
        }
        return;
    }
    let (_, shift, _) = mods();
    if let Some(d) = &mut e.draft {
        match d.kind {
            Kind::Pencil | Kind::Highlighter => {
                if let Some(last) = d.points.last() {
                    if last.dist(ip) * zoom >= 1.5 {
                        d.points.push(ip);
                    }
                }
            }
            _ => {
                d.points[1] = if shift { constrained(d.points[0], ip, d.kind) } else { ip };
            }
        }
        e.dirty = true;
        app.request_frame();
    }
}

fn mouse_down(app: &mut App, i: usize, p: Pt, double: bool) {
    let e = &app.captures.editors[i];
    if let Some(b) = btn_at(app, e, p) {
        let e = &mut app.captures.editors[i];
        e.pressed = Some(b);
        e.dirty = true;
        app.request_frame();
        if b == Btn::Grip {
            commit_text(app, i);
            let id = app.captures.editors[i].doc;
            if let Some(doc) = app.captures.docs.get_mut(&id) {
                if let Some(path) = doc.temp_file() {
                    let img = crate::imaging::scale_raw(&doc.rendered(), 160);
                    ui::defer(move || {
                        if let Some(obj) = crate::dragdrop::files_object(&[path]) {
                            crate::dragdrop::set_drag_image(&obj, &img, POINT { x: img.w as i32 / 2, y: img.h as i32 / 2 });
                            crate::dragdrop::run(&obj);
                        }
                    });
                }
            }
            app.captures.editors[i].pressed = None;
        }
        return;
    }
    if p.y <= BAR_H {
        return;
    }
    // Clicking outside the text field ends editing.
    if let Some(t) = app.captures.editors[i].text.as_ref() {
        let fr = text_field_rect(app, &app.captures.editors[i], t);
        if fr.contains(p) {
            let zoom = app.captures.editors[i].zoom;
            let (_, shift, _) = mods();
            if let Some(t) = app.captures.editors[i].text.as_mut() {
                let f = Font::sans(t.ann.font_size * zoom, Weight::Bold);
                t.field.mouse_down(&crate::gfx::gfx(), &f, fr, p.x, shift, if double { 2 } else { 1 });
            }
            app.captures.editors[i].dirty = true;
            app.request_frame();
            return;
        }
        commit_text(app, i);
        if app.captures.editors[i].tool == Tool::Text {
            return;
        }
    }
    let e = &app.captures.editors[i];
    let ip = to_image(app, e, p);
    let tol = 5.0 / e.zoom;
    let g = crate::gfx::gfx();
    match e.tool {
        Tool::Select => {
            let hit = e.annotations.iter().rev().find(|a| a.hit(&g, ip, tol)).cloned();
            let e = &mut app.captures.editors[i];
            e.selected = hit.as_ref().map(|a| a.id);
            e.dirty = true;
            if let Some(h) = &hit {
                if double && h.kind == Kind::Text {
                    begin_text(app, i, h.clone(), false);
                    return;
                }
            }
            let e = &mut app.captures.editors[i];
            e.drag_start = hit.map(|_| ip);
            e.drag_moved = false;
            app.request_frame();
        }
        Tool::Text => {
            let hit = e.annotations.iter().rev().find(|a| a.kind == Kind::Text && a.hit(&g, ip, tol)).cloned();
            match hit {
                Some(h) => begin_text(app, i, h, false),
                None => {
                    let start = clamp_img(app, e, ip);
                    let mut a = Annotation::new(Kind::Text, vec![start], e.color, e.stroke);
                    a.font_size = font_size_for(e.stroke);
                    begin_text(app, i, a, true);
                }
            }
        }
        t => {
            let Some(kind) = t.kind() else { return };
            let start = clamp_img(app, e, ip);
            let (color, width) = (e.color, e.stroke);
            if kind == Kind::Counter {
                commit(app, i, Annotation::new(Kind::Counter, vec![start], color, width));
                return;
            }
            let c = if matches!(kind, Kind::Blur | Kind::Spotlight) { Color::CLEAR } else { color };
            let mut d = Annotation::new(kind, vec![start, start], c, width);
            if matches!(kind, Kind::Pencil | Kind::Highlighter) {
                d.points = vec![start];
            }
            let e = &mut app.captures.editors[i];
            e.draft = Some(d);
            e.dirty = true;
            app.request_frame();
        }
    }
}

fn mouse_up(app: &mut App, i: usize, p: Pt) {
    if let Some(b) = app.captures.editors[i].pressed.take() {
        app.captures.editors[i].dirty = true;
        app.request_frame();
        if btn_at(app, &app.captures.editors[i], p) != Some(b) {
            return;
        }
        match b {
            Btn::Cmd(0) => run(app, i, Cmd::Copy),
            Btn::Cmd(1) => run(app, i, Cmd::Save),
            Btn::Cmd(_) => run(app, i, Cmd::Pin),
            Btn::Tool(t) => set_tool(app, i, t),
            Btn::Style => style_menu(app, i),
            Btn::Add => run(app, i, Cmd::AddCapture),
            Btn::More => more_menu(app, i),
            Btn::Size => {
                let z = app.captures.editors[i].zoom;
                zoom(app, i, if (z - 1.0).abs() < 0.001 { Zoom::Fit } else { Zoom::Actual });
            }
            _ => {}
        }
        return;
    }
    if let Some(t) = app.captures.editors[i].text.as_mut() {
        t.field.mouse_up();
        return;
    }
    let e = &mut app.captures.editors[i];
    if e.tool == Tool::Select {
        if e.drag_moved {
            save_doc(app, i);
        }
        let e = &mut app.captures.editors[i];
        e.drag_start = None;
        e.drag_moved = false;
        return;
    }
    let Some(d) = e.draft.take() else { return };
    let z = e.zoom;
    let big = match d.kind {
        Kind::Pencil | Kind::Highlighter => d.points.len() > 1,
        Kind::Arrow | Kind::Line | Kind::Measure => d.points[0].dist(d.points[1]) * z > 4.0,
        _ => d.rect().w * z > 3.0 && d.rect().h * z > 3.0,
    };
    if big {
        commit(app, i, d);
    } else {
        e.dirty = true;
        app.request_frame();
    }
}

fn right_click(app: &mut App, i: usize, p: Pt) {
    if p.y <= BAR_H {
        return;
    }
    let e = &app.captures.editors[i];
    let ip = to_image(app, e, p);
    let g = crate::gfx::gfx();
    let Some(hit) = e.annotations.iter().rev().find(|a| a.hit(&g, ip, 5.0 / e.zoom)).map(|a| a.id) else { return };
    let hwnd = e.hwnd;
    app.captures.editors[i].selected = Some(hit);
    app.captures.editors[i].dirty = true;
    app.request_frame();
    ui::defer(move || {
        let mut m = crate::menu::Menu::new();
        let with_ed = move |f: fn(&mut App, usize)| {
            move || {
                app::with(|a| {
                    if let Some(j) = index(a, hwnd) {
                        f(a, j);
                    }
                });
            }
        };
        m.action("Delete", with_ed(delete_selection));
        m.action("Bring to Front", with_ed(|a, j| reorder(a, j, true)));
        m.action("Send to Back", with_ed(|a, j| reorder(a, j, false)));
        let (x, y) = monitors::cursor();
        crate::menu::popup(m, hwnd, x, y);
    });
}

fn menu_icon(icon: Icon, scale: f32) -> Option<crate::imaging::Raw> {
    let px = (16.0 * scale).round() as u32;
    icons::rasterize(&crate::gfx::gfx(), icon, px, 13.0 * scale, Weight::Regular, Color::white(0.92))
}

fn more_menu(app: &mut App, i: usize) {
    let e = &app.captures.editors[i];
    let (hwnd, scale, cur) = (e.hwnd, e.scale, e.tool);
    let (w, _) = client_size(e);
    let anchor = layout_bar(e, w).into_iter().find(|(b, _)| *b == Btn::More).map(|(_, r)| r).unwrap_or_default();
    ui::defer(move || {
        let mut m = crate::menu::Menu::new();
        let ed = move |f: Box<dyn FnOnce(&mut App, usize)>| {
            move || {
                app::with(|a| {
                    if let Some(j) = index(a, hwnd) {
                        f(a, j);
                    }
                });
            }
        };
        m.item("Add Capture", "Ctrl+A", false, menu_icon(Icon::PlusViewfinder, scale), ed(Box::new(|a, j| run(a, j, Cmd::AddCapture))));
        for t in Tool::MENU {
            m.item(t.title(), &t.key().to_string(), cur == t, menu_icon(t.icon(), scale), ed(Box::new(move |a, j| set_tool(a, j, t))));
        }
        m.item("Paste Image", "Ctrl+V", false, menu_icon(Icon::PhotoOnRectangle, scale), ed(Box::new(|a, j| run(a, j, Cmd::Paste))));
        m.sep();
        m.item("Actual Size (100%)", "Ctrl+0", false, menu_icon(Icon::OneMagnifyingGlass, scale), ed(Box::new(|a, j| zoom(a, j, Zoom::Actual))));
        m.item("Zoom to Fit", "Ctrl+9", false, menu_icon(Icon::ArrowsOutward, scale), ed(Box::new(|a, j| zoom(a, j, Zoom::Fit))));
        m.item("Zoom In", "Ctrl+=", false, menu_icon(Icon::PlusMagnifyingGlass, scale), ed(Box::new(|a, j| zoom(a, j, Zoom::In))));
        m.item("Zoom Out", "Ctrl+-", false, menu_icon(Icon::MinusMagnifyingGlass, scale), ed(Box::new(|a, j| zoom(a, j, Zoom::Out))));
        m.sep();
        m.item("Copy Text", "", false, menu_icon(Icon::TextViewfinder, scale), ed(Box::new(|a, j| run(a, j, Cmd::CopyText))));
        m.item("Save As…", "Ctrl+Shift+S", false, menu_icon(Icon::SquareAndArrowDownOnSquare, scale), ed(Box::new(|a, j| run(a, j, Cmd::SaveAs))));
        let mut p = POINT { x: (anchor.x * scale) as i32, y: (anchor.b() * scale + 4.0) as i32 };
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut p);
        }
        crate::menu::popup(m, hwnd, p.x, p.y);
    });
}

fn swatch(c: Color, scale: f32) -> Option<crate::imaging::Raw> {
    let px = (14.0 * scale).round() as u32;
    crate::gfx::offscreen(px, px, scale, |cv| {
        let ctr = pt(7.0, 7.0);
        cv.fill_circle(ctr, 6.0, c);
        cv.stroke_circle(ctr, 6.0, 1.0, Color::white(0.3));
    })
}

fn style_menu(app: &mut App, i: usize) {
    let e = &app.captures.editors[i];
    let (hwnd, scale, color, width) = (e.hwnd, e.scale, e.color, e.stroke);
    let (w, _) = client_size(e);
    let anchor = layout_bar(e, w).into_iter().find(|(b, _)| *b == Btn::Style).map(|(_, r)| r).unwrap_or_default();
    ui::defer(move || {
        let mut m = crate::menu::Menu::new();
        for (name, c) in COLORS {
            m.item(name, "", color.hex() == c.hex(), swatch(c, scale), move || {
                app::with(|a| {
                    if let Some(j) = index(a, hwnd) {
                        set_color(a, j, c);
                    }
                });
            });
        }
        m.sep();
        for (name, wd) in [("Thin", 2.0f32), ("Medium", 4.0), ("Thick", 7.0)] {
            m.toggle(name, (width - wd).abs() < 0.01, move || {
                app::with(|a| {
                    if let Some(j) = index(a, hwnd) {
                        set_width(a, j, wd);
                    }
                });
            });
        }
        let mut p = POINT { x: (anchor.x * scale) as i32, y: (anchor.b() * scale + 4.0) as i32 };
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut p);
        }
        crate::menu::popup(m, hwnd, p.x, p.y);
    });
}

fn key(app: &mut App, i: usize, vk: u16) -> bool {
    let (ctrl, shift, alt) = mods();
    let v = VIRTUAL_KEY(vk);
    // Text field first.
    if app.captures.editors[i].text.is_some() {
        if v == VK_RETURN || v == VK_ESCAPE {
            commit_text(app, i);
            return true;
        }
        if let Some(t) = app.captures.editors[i].text.as_mut() {
            if let crate::textfield::KeyResult::Handled { .. } = t.field.key(vk) {
                app.captures.editors[i].dirty = true;
                app.request_frame();
                return true;
            }
        }
        return ctrl;
    }
    if ctrl {
        match vk as u8 {
            b'Z' => {
                if shift { redo(app, i) } else { undo(app, i) }
            }
            b'Y' => redo(app, i),
            b'C' => run(app, i, Cmd::Copy),
            b'S' => run(app, i, if shift { Cmd::SaveAs } else { Cmd::Save }),
            b'V' => run(app, i, Cmd::Paste),
            b'A' => run(app, i, Cmd::AddCapture),
            b'W' => run(app, i, Cmd::Close),
            b'0' | 0x60 => zoom(app, i, Zoom::Actual),
            b'9' | 0x69 => zoom(app, i, Zoom::Fit),
            0xBB | 0x6B => zoom(app, i, Zoom::In),
            0xBD | 0x6D => zoom(app, i, Zoom::Out),
            _ => return false,
        }
        return true;
    }
    match v {
        VK_ESCAPE => {
            let e = &mut app.captures.editors[i];
            if e.selected.is_some() {
                e.selected = None;
                e.dirty = true;
                app.request_frame();
            } else {
                run(app, i, Cmd::Close);
            }
            true
        }
        VK_DELETE | VK_BACK => {
            delete_selection(app, i);
            true
        }
        VK_TAB => {
            match app.captures.editors[i].hover_color {
                Some(c) => {
                    clipboard::copy_text(&c.hex());
                    flash(app, i, &format!("Copied {}", c.hex()));
                }
                None => util::beep(),
            }
            true
        }
        VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN => {
            let e = &mut app.captures.editors[i];
            let Some(id) = e.selected else { return false };
            let Some(k) = e.annotations.iter().position(|a| a.id == id) else { return false };
            let step = if shift { 10.0 } else { 1.0 };
            let (dx, dy) = match v {
                VK_LEFT => (-step, 0.0),
                VK_RIGHT => (step, 0.0),
                VK_UP => (0.0, -step),
                _ => (0.0, step),
            };
            push_undo(e);
            e.annotations[k].moved(dx, dy);
            save_doc(app, i);
            true
        }
        _ => {
            if alt {
                return false;
            }
            if let Some(t) = Tool::ALL.iter().find(|t| t.key() as u16 == vk) {
                set_tool(app, i, *t);
                return true;
            }
            false
        }
    }
}
