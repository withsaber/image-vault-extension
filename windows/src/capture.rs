//! Screenshots (Capture.swift): the capture document + annotation renderer,
//! the stack of floating previews (bottom-right), pinned shots, and the
//! actions shared by the preview, editor and pin.

use std::collections::HashMap;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Interface, w};

use crate::anim::{Curve, key_id};
use crate::app::{self, App};
use crate::gfx::{Align, Canvas, Surface, Trim, rf};
use crate::icons::{self, Icon};
use crate::imaging::{self, Raw};
use crate::island;
use crate::monitors::{self, Monitor};
use crate::theme::{Color, Font, Pt, Rect, Weight, pt, rect};
use crate::{clipboard, ingest, ui, util};

// ---------------------------------------------------------------------------
// Annotations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Arrow,
    Line,
    Measure,
    Rectangle,
    FilledRect,
    Oval,
    Pencil,
    Highlighter,
    Blur,
    Spotlight,
    Counter,
    Text,
    Image,
}

/// One mark, in image points (origin top-left).
#[derive(Clone)]
pub struct Annotation {
    pub id: u64,
    pub kind: Kind,
    pub points: Vec<Pt>,
    pub color: Color,
    pub width: f32,
    pub text: String,
    pub font_size: f32,
    pub image: Option<std::rc::Rc<(Raw, f32)>>,
}

static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub fn new_id() -> u64 {
    NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

fn text_font(size: f32) -> Font {
    Font::sans(size, Weight::Bold)
}

impl Annotation {
    pub fn new(kind: Kind, points: Vec<Pt>, color: Color, width: f32) -> Annotation {
        Annotation { id: new_id(), kind, points, color, width, text: String::new(), font_size: 22.0, image: None }
    }

    pub fn rect(&self) -> Rect {
        let Some(a) = self.points.first() else { return Rect::default() };
        let b = self.points.get(1).copied().unwrap_or(*a);
        Rect::from_points(*a, b)
    }

    pub fn counter_radius(&self) -> f32 {
        (self.width * 3.0).max(12.0)
    }

    pub fn highlighter_width(&self) -> f32 {
        (self.width * 4.0).max(16.0)
    }

    pub fn text_size(&self, g: &crate::gfx::Gfx) -> (f32, f32) {
        let t = if self.text.is_empty() { " " } else { &self.text };
        let (w, h) = g.measure(t, &text_font(self.font_size));
        (w.ceil(), h.ceil())
    }

    /// Area used for hit-testing and the selection outline.
    pub fn bounds(&self, g: &crate::gfx::Gfx) -> Rect {
        match self.kind {
            Kind::Counter => {
                let r = self.counter_radius();
                let c = self.points.first().copied().unwrap_or_default();
                rect(c.x - r, c.y - r, r * 2.0, r * 2.0)
            }
            Kind::Text => {
                let p = self.points.first().copied().unwrap_or_default();
                let (w, h) = self.text_size(g);
                rect(p.x, p.y, w, h)
            }
            Kind::Pencil | Kind::Highlighter => {
                let Some(f) = self.points.first() else { return Rect::default() };
                let mut r = rect(f.x, f.y, 0.0, 0.0);
                for p in &self.points {
                    r = r.union(&rect(p.x, p.y, 0.0, 0.0));
                }
                let pad = if self.kind == Kind::Highlighter { self.highlighter_width() } else { self.width } / 2.0;
                r.inset(-pad, -pad)
            }
            _ => self.rect(),
        }
    }

    pub fn hit(&self, g: &crate::gfx::Gfx, p: Pt, tol: f32) -> bool {
        match self.kind {
            Kind::Arrow | Kind::Line | Kind::Measure => {
                self.points.len() > 1 && dist_seg(p, self.points[0], self.points[1]) <= self.width / 2.0 + tol
            }
            Kind::Pencil | Kind::Highlighter => {
                let w = if self.kind == Kind::Highlighter { self.highlighter_width() } else { self.width } / 2.0 + tol;
                for i in 1..self.points.len() {
                    if dist_seg(p, self.points[i - 1], self.points[i]) <= w {
                        return true;
                    }
                }
                self.points.len() == 1 && p.dist(self.points[0]) <= w
            }
            _ => self.bounds(g).inset(-tol, -tol).contains(p),
        }
    }

    pub fn moved(&mut self, dx: f32, dy: f32) {
        for p in self.points.iter_mut() {
            p.x += dx;
            p.y += dy;
        }
    }
}

pub fn dist_seg(p: Pt, a: Pt, b: Pt) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    if len2 <= 0.0 {
        return p.dist(a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0);
    p.dist(pt(a.x + t * dx, a.y + t * dy))
}

// ---------------------------------------------------------------------------
// Document
// ---------------------------------------------------------------------------

pub struct Doc {
    pub id: u64,
    pub base: Raw,
    pub scale: f32,
    pub created: String,
    pub annotations: Vec<Annotation>,
    pub version: u64,
    base_bmp: Option<(u32, ID2D1Bitmap1)>,
    blur_bmp: Option<(u32, ID2D1Bitmap1)>,
    rendered: Option<(u64, Raw)>,
    images: HashMap<usize, (u32, ID2D1Bitmap1)>,
}

impl Doc {
    pub fn new(base: Raw, scale: f32) -> Doc {
        Doc {
            id: new_id(),
            base,
            scale: scale.max(1.0),
            created: util::stamp(),
            annotations: Vec::new(),
            version: 0,
            base_bmp: None,
            blur_bmp: None,
            rendered: None,
            images: HashMap::new(),
        }
    }

    /// Size in image points.
    pub fn size(&self) -> (f32, f32) {
        (self.base.w as f32 / self.scale, self.base.h as f32 / self.scale)
    }

    pub fn name(&self) -> String {
        format!("Screenshot {}.png", self.created)
    }

    pub fn set_annotations(&mut self, a: Vec<Annotation>) {
        self.annotations = a;
        self.version += 1;
        self.rendered = None;
    }

    fn base_bitmap(&mut self, g: &crate::gfx::Gfx) -> Option<ID2D1Bitmap1> {
        if self.base_bmp.as_ref().map(|(gen_, _)| *gen_ != g.generation).unwrap_or(true) {
            self.base_bmp = g.bitmap(self.base.w, self.base.h, &self.base.px).map(|b| (g.generation, b));
        }
        self.base_bmp.as_ref().map(|(_, b)| b.clone())
    }

    /// Pixellated copy (CIPixellate 10 pt); the Gaussian runs on the GPU.
    fn blur_bitmap(&mut self, g: &crate::gfx::Gfx) -> Option<ID2D1Bitmap1> {
        if self.blur_bmp.as_ref().map(|(gen_, _)| *gen_ != g.generation).unwrap_or(true) {
            let block = (10.0 * self.scale).round().max(2.0) as u32;
            let (w, h) = (self.base.w, self.base.h);
            let mut px = self.base.px.clone();
            let mut by = 0;
            while by < h {
                let mut bx = 0;
                while bx < w {
                    let (cx, cy) = ((bx + block / 2).min(w - 1), (by + block / 2).min(h - 1));
                    let ci = ((cy * w + cx) * 4) as usize;
                    let c = [self.base.px[ci], self.base.px[ci + 1], self.base.px[ci + 2], 255];
                    for y in by..(by + block).min(h) {
                        for x in bx..(bx + block).min(w) {
                            let i = ((y * w + x) * 4) as usize;
                            px[i..i + 4].copy_from_slice(&c);
                        }
                    }
                    bx += block;
                }
                by += block;
            }
            self.blur_bmp = g.bitmap(w, h, &px).map(|b| (g.generation, b));
        }
        self.blur_bmp.as_ref().map(|(_, b)| b.clone())
    }

    /// The capture with every annotation flattened in, at full resolution.
    pub fn rendered(&mut self) -> Raw {
        if let Some((v, r)) = &self.rendered {
            if *v == self.version {
                return r.clone();
            }
        }
        if self.annotations.is_empty() {
            return self.base.clone();
        }
        let items = self.annotations.clone();
        let (w, h) = (self.base.w, self.base.h);
        let scale = self.scale;
        let out = crate::gfx::offscreen(w, h, scale, |cv| draw_doc(cv, self, &items, true)).unwrap_or_else(|| self.base.clone());
        self.rendered = Some((self.version, out.clone()));
        out
    }

    pub fn png(&mut self) -> Option<Vec<u8>> {
        let r = self.rendered();
        imaging::encode_png(&r, Some(96.0 * self.scale as f64))
    }

    /// A real file for drag-out and OCR, named like a screenshot.
    pub fn temp_file(&mut self) -> Option<std::path::PathBuf> {
        let dir = std::env::temp_dir().join("ImageVault-captures").join(crate::vault::random_suffix());
        std::fs::create_dir_all(&dir).ok()?;
        let p = dir.join(self.name());
        std::fs::write(&p, self.png()?).ok()?;
        Some(p)
    }

    /// sRGB colour of the original capture at a point.
    pub fn color_at(&self, p: Pt) -> Option<Color> {
        let x = (p.x * self.scale) as i64;
        let y = (p.y * self.scale) as i64;
        if x < 0 || y < 0 || x >= self.base.w as i64 || y >= self.base.h as i64 {
            return None;
        }
        let i = ((y as u32 * self.base.w + x as u32) * 4) as usize;
        let px = &self.base.px;
        Some(Color::rgba(px[i + 2] as f32 / 255.0, px[i + 1] as f32 / 255.0, px[i] as f32 / 255.0, 1.0))
    }
}

/// Midpoint-quadratic smoothing for freehand strokes.
fn smooth(cv: &Canvas, pts: &[Pt]) -> ID2D1Geometry {
    let mut p = cv.g.path().hollow();
    if let Some(f) = pts.first() {
        p.move_to(*f);
        if pts.len() == 1 {
            p.line_to(*f);
        }
        for i in 1..pts.len() {
            let mid = pt((pts[i - 1].x + pts[i].x) / 2.0, (pts[i - 1].y + pts[i].y) / 2.0);
            p.quad_to(pts[i - 1], mid);
        }
        if pts.len() > 1 {
            p.line_to(pts[pts.len() - 1]);
        }
    }
    p.finish().cast().unwrap()
}

/// Draws the capture and its annotations in image-point space at the origin.
pub fn draw_doc(cv: &mut Canvas, doc: &mut Doc, items: &[Annotation], smooth_base: bool) {
    let (w, h) = doc.size();
    let full = rect(0.0, 0.0, w, h);
    if let Some(b) = doc.base_bitmap(&cv.g) {
        cv.image(&b, full, 1.0, smooth_base);
    }
    // Blur/Erase first, so later marks sit on top of the blurred area.
    let blurs: Vec<Rect> = items.iter().filter(|a| a.kind == Kind::Blur && a.rect().w > 0.0 && a.rect().h > 0.0).map(|a| a.rect()).collect();
    if !blurs.is_empty() {
        if let Some(b) = doc.blur_bitmap(&cv.g) {
            unsafe {
                if let Ok(fx) = cv.dc().CreateEffect(&CLSID_D2D1GaussianBlur) {
                    let img: ID2D1Image = b.cast().unwrap();
                    fx.SetInput(0, &img, true);
                    let sigma = 7.0f32;
                    let _ = fx.SetValue(D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32, D2D1_PROPERTY_TYPE_FLOAT, &sigma.to_le_bytes());
                    let _ = fx.SetValue(
                        D2D1_GAUSSIANBLUR_PROP_BORDER_MODE.0 as u32,
                        D2D1_PROPERTY_TYPE_ENUM,
                        &(D2D1_BORDER_MODE_HARD.0 as u32).to_le_bytes(),
                    );
                    if let Ok(out) = fx.GetOutput() {
                        // The bitmap is in pixels at 96 dpi; scale it to points.
                        for r in blurs {
                            cv.clip(r);
                            cv.save();
                            // Effect output is in device pixels of the target.
                            cv.transform(crate::gfx::scale_about(cv.scale / doc.scale, 0.0, 0.0));
                            cv.dc().DrawImage(&out, None, None, D2D1_INTERPOLATION_MODE_LINEAR, D2D1_COMPOSITE_MODE_SOURCE_OVER);
                            cv.restore();
                            cv.unclip();
                        }
                    }
                }
            }
        }
    }
    // Spotlight: dim everything outside the union of spotlight boxes.
    let spots: Vec<Rect> = items.iter().filter(|a| a.kind == Kind::Spotlight && a.rect().w > 1.0 && a.rect().h > 1.0).map(|a| a.rect()).collect();
    if !spots.is_empty() {
        let mut p = cv.g.path().alternate();
        p.rect(full);
        for s in &spots {
            p.round_rect(*s, 6.0);
        }
        let g: ID2D1Geometry = p.finish().cast().unwrap();
        cv.fill_geom(&g, Color::black(0.55));
    }
    let mut counter = 0;
    for a in items {
        if a.kind == Kind::Counter {
            counter += 1;
        }
        draw_annotation(cv, doc, a, counter);
    }
}

fn shadowed(cv: &mut Canvas, f: impl FnOnce(&mut Canvas)) {
    cv.shadowed(1.5, pt(0.0, 1.0), Color::black(0.35), true, f);
}

fn draw_annotation(cv: &mut Canvas, doc: &mut Doc, a: &Annotation, number: usize) {
    let c = a.color;
    match a.kind {
        Kind::Blur | Kind::Spotlight => {}
        Kind::Arrow => {
            if a.points.len() < 2 {
                return;
            }
            let (p0, p1) = (a.points[0], a.points[1]);
            let len = p0.dist(p1);
            if len <= 1.0 {
                return;
            }
            let ang = (p1.y - p0.y).atan2(p1.x - p0.x);
            let head = (a.width * 3.6).max(12.0).min(len * 0.6);
            let base = pt(p1.x - head * 0.8 * ang.cos(), p1.y - head * 0.8 * ang.sin());
            let wdt = a.width;
            shadowed(cv, |cv| {
                cv.line(p0, base, wdt, c, 1);
                let mut p = cv.g.path();
                p.move_to(p1);
                p.line_to(pt(p1.x - head * (ang - 0.48).cos(), p1.y - head * (ang - 0.48).sin()));
                p.line_to(pt(p1.x - head * (ang + 0.48).cos(), p1.y - head * (ang + 0.48).sin()));
                p.close();
                let g: ID2D1Geometry = p.finish().cast().unwrap();
                cv.fill_geom(&g, c);
                cv.stroke_geom(&g, (wdt * 0.5).max(1.0), c, 1);
            });
        }
        Kind::Line => {
            if a.points.len() < 2 {
                return;
            }
            let (p0, p1, wdt) = (a.points[0], a.points[1], a.width);
            shadowed(cv, |cv| cv.line(p0, p1, wdt, c, 1));
        }
        Kind::Measure => {
            if a.points.len() < 2 {
                return;
            }
            let (p0, p1) = (a.points[0], a.points[1]);
            let ang = (p1.y - p0.y).atan2(p1.x - p0.x);
            let (nx, ny) = (-ang.sin() * 6.0, ang.cos() * 6.0);
            let wdt = (a.width * 0.5).max(1.5);
            shadowed(cv, |cv| {
                cv.line(p0, p1, wdt, c, 1);
                for p in [p0, p1] {
                    cv.line(pt(p.x - nx, p.y - ny), pt(p.x + nx, p.y + ny), wdt, c, 1);
                }
            });
            let label = format!("{} pt", p0.dist(p1).round() as i64);
            let f = Font::mono(11.0, Weight::Semibold);
            let (tw, th) = cv.measure(&label, &f);
            let mid = pt((p0.x + p1.x) / 2.0, (p0.y + p1.y) / 2.0);
            let pill = rect(mid.x - tw / 2.0 - 6.0, mid.y - 6.0 - (th + 4.0), tw + 12.0, th + 4.0);
            cv.fill_rrect(pill, pill.h / 2.0, Color::black(0.78));
            cv.text(&label, &f, Color::white(1.0), rect(pill.x + 6.0, pill.y + 2.0, tw + 1.0, th), Align::Leading, Trim::None);
        }
        Kind::Rectangle => {
            let r = a.rect();
            let rad = 4.0f32.min(r.w / 2.0).min(r.h / 2.0);
            let wdt = a.width;
            shadowed(cv, |cv| cv.stroke_rrect(r, rad, wdt, c));
        }
        Kind::FilledRect => {
            let r = a.rect();
            let rad = 3.0f32.min(r.w / 2.0).min(r.h / 2.0);
            cv.fill_rrect(r, rad, c);
        }
        Kind::Oval => {
            let r = a.rect();
            let wdt = a.width;
            shadowed(cv, |cv| {
                let mut p = cv.g.path();
                p.ellipse(r);
                let g: ID2D1Geometry = p.finish().cast().unwrap();
                cv.stroke_geom(&g, wdt, c, 1);
            });
        }
        Kind::Pencil => {
            let g = smooth(cv, &a.points);
            let wdt = a.width;
            shadowed(cv, |cv| cv.stroke_geom(&g, wdt, c, 1));
        }
        Kind::Highlighter => {
            // Plain translucency: multiply vanishes on dark screenshots.
            let g = smooth(cv, &a.points);
            let style = cv.g.stroke_style(None, 0.0, a.highlighter_width(), 0);
            unsafe {
                let gg = &cv.g;
                cv.dc().DrawGeometry(&g, gg.brush(c.with_alpha(0.4).mul_alpha(cv.alpha)), a.highlighter_width(), &style);
            }
        }
        Kind::Counter => {
            let Some(cc) = a.points.first().copied() else { return };
            let r = a.counter_radius();
            shadowed(cv, |cv| cv.fill_circle(cc, r, c));
            let light = c.brightness() > 0.85 && c.saturation() < 0.2;
            let f = Font::sans(r * 1.05, Weight::Bold);
            let label = format!("{number}");
            let (tw, th) = cv.measure(&label, &f);
            cv.text(&label, &f, if light { Color::black(1.0) } else { Color::white(1.0) }, rect(cc.x - tw / 2.0, cc.y - th / 2.0, tw + 1.0, th), Align::Leading, Trim::None);
        }
        Kind::Text => {
            let Some(p) = a.points.first().copied() else { return };
            if a.text.is_empty() {
                return;
            }
            let f = text_font(a.font_size);
            let t = a.text.clone();
            cv.shadowed(1.5, pt(0.0, 1.0), Color::black(0.45), true, |cv| cv.text_at(&t, &f, c, p));
        }
        Kind::Image => {
            let Some(img) = a.image.clone() else { return };
            let r = a.rect();
            let key = std::rc::Rc::as_ptr(&img) as usize;
            let gen_ = cv.g.generation;
            let bmp = match doc.images.get(&key) {
                Some((g, b)) if *g == gen_ => Some(b.clone()),
                _ => {
                    let b = cv.g.bitmap(img.0.w, img.0.h, &img.0.px);
                    if let Some(b) = &b {
                        doc.images.insert(key, (gen_, b.clone()));
                    }
                    b
                }
            };
            if let Some(b) = bmp {
                shadowed(cv, |cv| cv.image(&b, r, 1.0, true));
            }
            cv.stroke_rect(r, 0.5, Color::white(0.35));
        }
    }
}

// ---------------------------------------------------------------------------
// Windows: previews + pins
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum WinKind {
    Preview,
    Pin,
}

struct Hud {
    rect: Rect,
    icon: Icon,
    tip: &'static str,
    name: &'static str,
}

pub struct Preview {
    pub hwnd: HWND,
    pub doc: u64,
    surface: Option<Surface>,
    scale: f32,
    w: f32,
    h: f32,
    image_h: f32,
    /// Work area of the screen it lives on (physical px).
    work: RECT,
    /// Animated position (DIPs from work-area bottom-right) + opacity.
    x: f32,
    y: f32,
    target_y: f32,
    alpha_target: f32,
    closing: bool,
    hover: Option<&'static str>,
    pressed: Option<&'static str>,
    confirm: HashMap<&'static str, f64>,
    down: Option<Pt>,
    bmp: Option<(u64, u32, ID2D1Bitmap1)>,
    dirty: bool,
    shown: bool,
}

pub struct Pin {
    pub hwnd: HWND,
    pub doc: u64,
    surface: Option<Surface>,
    scale: f32,
    pub opacity: f32,
    hover: bool,
    close_hover: bool,
    bmp: Option<(u64, u32, ID2D1Bitmap1)>,
    fade: bool,
    dirty: bool,
}

#[derive(Default)]
pub struct Captures {
    pub docs: HashMap<u64, Doc>,
    pub previews: Vec<Preview>,
    pub pins: Vec<Pin>,
    pub editors: Vec<crate::editor::Editor>,
}

impl Captures {
    pub fn device_lost(&mut self) {
        for p in self.previews.iter_mut() {
            p.surface = None;
            p.bmp = None;
            p.dirty = true;
        }
        for p in self.pins.iter_mut() {
            p.surface = None;
            p.bmp = None;
            p.dirty = true;
        }
        for e in self.editors.iter_mut() {
            e.device_lost();
        }
        for d in self.docs.values_mut() {
            d.base_bmp = None;
            d.blur_bmp = None;
            d.images.clear();
        }
    }

    /// Something visible changed in a document (editor edits): previews and pins refresh.
    pub fn doc_changed(&mut self, id: u64) {
        for p in self.previews.iter_mut().filter(|p| p.doc == id) {
            p.dirty = true;
        }
        for p in self.pins.iter_mut().filter(|p| p.doc == id) {
            p.dirty = true;
        }
    }

    /// Drop documents nobody shows any more.
    fn gc(&mut self) {
        let used: std::collections::HashSet<u64> =
            self.previews.iter().map(|p| p.doc).chain(self.pins.iter().map(|p| p.doc)).chain(self.editors.iter().map(|e| e.doc)).collect();
        self.docs.retain(|id, _| used.contains(id));
    }
}

const PREVIEW_W: f32 = 220.0;

fn register(name: windows::core::PCWSTR, proc_: WNDPROC) {
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: proc_,
            hInstance: hinst.into(),
            lpszClassName: name,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            style: CS_DBLCLKS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
    }
}

/// Borderless popup with the system shadow and rounded corners.
pub fn shadow_window(class: windows::core::PCWSTR, ex: WINDOW_EX_STYLE, r: RECT, corner: DWM_WINDOW_CORNER_PREFERENCE) -> HWND {
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let hwnd = CreateWindowExW(
            ex | WS_EX_NOREDIRECTIONBITMAP,
            class,
            w!("Image Vault"),
            WS_POPUP,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            None,
            None,
            Some(hinst.into()),
            None,
        )
        .unwrap_or_default();
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as *const _, 4);
        let policy = DWMNCRP_ENABLED;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_NCRENDERING_POLICY, &policy as *const _ as *const _, 4);
        let m = windows::Win32::UI::Controls::MARGINS { cxLeftWidth: 0, cxRightWidth: 0, cyTopHeight: 1, cyBottomHeight: 0 };
        let _ = DwmExtendFrameIntoClientArea(hwnd, &m);
        let none = DWMWA_COLOR_NONE;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &none as *const _ as *const _, 4);
        crate::menu::allow_dark(hwnd);
        hwnd
    }
}

unsafe extern "system" fn win_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCALCSIZE && w.0 != 0 {
            return LRESULT(0);
        }
        let r = app::with(|a| handle(a, hwnd, msg, w, l));
        ui::drain();
        match r {
            Some(Some(v)) => v,
            _ => DefWindowProcW(hwnd, msg, w, l),
        }
    }
}

fn find(app: &App, hwnd: HWND) -> Option<(WinKind, usize)> {
    if let Some(i) = app.captures.previews.iter().position(|p| p.hwnd == hwnd) {
        return Some((WinKind::Preview, i));
    }
    app.captures.pins.iter().position(|p| p.hwnd == hwnd).map(|i| (WinKind::Pin, i))
}

// -- center ---------------------------------------------------------------------

/// ⌃⌥S: pick a region (click = whole display) → floating preview.
pub fn take_screenshot() {
    crate::screenocr::toggle_screenshot(Box::new(|img, scale, mon| {
        app::with(|a| present(a, Doc::new(img, scale), &mon));
    }));
}

pub fn present(app: &mut App, doc: Doc, mon: &Monitor) {
    register(w!("ImageVault.Capture"), Some(win_proc));
    let scale = mon.scale;
    let (dw, dh) = doc.size();
    let image_w = PREVIEW_W - 16.0;
    let image_h = (image_w * dh / dw.max(1.0)).max(60.0).min(150.0);
    let h = 26.0 + image_h + 8.0 + 34.0;
    let id = doc.id;
    app.captures.docs.insert(id, doc);
    let work = mon.work;
    let px = |v: f32| (v * scale).round() as i32;
    let r = RECT { left: work.right - px(PREVIEW_W + 16.0), top: work.bottom - px(h + 16.0), right: work.right - px(16.0), bottom: work.bottom - px(16.0) };
    let hwnd = shadow_window(w!("ImageVault.Capture"), WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, r, DWMWCP_ROUND);
    app.captures.previews.push(Preview {
        hwnd,
        doc: id,
        surface: None,
        scale,
        w: PREVIEW_W,
        h,
        image_h,
        work,
        x: 24.0,
        y: 0.0,
        target_y: 0.0,
        alpha_target: 1.0,
        closing: false,
        hover: None,
        pressed: None,
        confirm: HashMap::new(),
        down: None,
        bmp: None,
        dirty: true,
        shown: false,
    });
    relayout(app, false);
    // Appear: slide in from the right while fading in.
    let k = key_id("cap.x", &id.to_string());
    app.anims.play(k, 24.0, 0.0, Curve::mac(0.28));
    app.anims.play(key_id("cap.a", &id.to_string()), 0.0, 1.0, Curve::mac(0.28));
    app.request_frame();
}

/// Newest at the bottom-right corner, older ones stacked above it.
fn relayout(app: &mut App, animated: bool) {
    let mut offsets: HashMap<(i32, i32), f32> = HashMap::new();
    for p in app.captures.previews.iter_mut().rev() {
        if p.closing {
            continue;
        }
        let k = (p.work.right, p.work.bottom);
        let y = *offsets.get(&k).unwrap_or(&0.0);
        p.target_y = y;
        if !animated {
            p.y = y;
        }
        offsets.insert(k, y + p.h + 10.0);
    }
    for p in app.captures.previews.iter() {
        let k = key_id("cap.y", &p.doc.to_string());
        if animated {
            app.anims.drive(k, p.target_y, Curve::mac(0.25));
        } else {
            app.anims.snap(k, p.target_y);
        }
    }
    app.request_frame();
}

fn dismiss_preview(app: &mut App, i: usize) {
    let Some(p) = app.captures.previews.get_mut(i) else { return };
    if p.closing {
        return;
    }
    p.closing = true;
    let id = p.doc;
    app.anims.drive(key_id("cap.a", &id.to_string()), 0.0, Curve::linear(0.18));
    ui::after(0.2, move || {
        app::with(|a| {
            if let Some(j) = a.captures.previews.iter().position(|p| p.doc == id && p.closing) {
                let p = a.captures.previews.remove(j);
                unsafe {
                    let _ = DestroyWindow(p.hwnd);
                }
                relayout(a, true);
                a.captures.gc();
            }
        });
    });
}

// -- actions ----------------------------------------------------------------------

/// Copy also keeps the shot in the vault (copied images always live there).
pub fn copy(app: &mut App, id: u64) {
    let Some(doc) = app.captures.docs.get_mut(&id) else { return };
    let raw = doc.rendered();
    clipboard::copy_raw(&raw, 96.0 * doc.scale as f64);
    save_to_vault(app, id, Some("Copied"));
}

pub fn save_to_vault(app: &mut App, id: u64, title: Option<&'static str>) {
    let Some(doc) = app.captures.docs.get_mut(&id) else { return };
    let Some(png) = doc.png() else { return };
    let name = doc.name();
    ingest::save_async(vec![ingest::Candidate { data: png, name: Some(name), url: None }], "screenshot", move |s| {
        app::with(|a| {
            app::finish_ingest(a, &s);
            let saved = if s.added.is_empty() { if s.is_empty() { "Save failed" } else { "Already saved" } } else { "Saved" };
            let detail = match title {
                None => format!("{} in vault", a.model.items.len()),
                Some(_) => if s.is_empty() { "Couldn't save to vault".into() } else { "Saved to vault".into() },
            };
            island::show_activity(a, s.first.clone(), title.unwrap_or(saved), &detail);
        });
    });
}

pub fn save_as(app: &mut App, id: u64, owner: HWND) {
    let Some(doc) = app.captures.docs.get_mut(&id) else { return };
    let name = doc.name();
    let Some(png) = doc.png() else { return };
    ui::defer(move || {
        if let Some(path) = save_dialog(owner, &name) {
            if std::fs::write(&path, png).is_err() {
                util::beep();
            }
        }
    });
}

fn save_dialog(owner: HWND, name: &str) -> Option<std::path::PathBuf> {
    use windows::Win32::System::Com::*;
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::*;
    unsafe {
        let d: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let spec = [COMDLG_FILTERSPEC { pszName: w!("PNG image"), pszSpec: w!("*.png") }];
        let _ = d.SetFileTypes(&spec);
        let _ = d.SetDefaultExtension(w!("png"));
        let n = util::wide(name);
        let _ = d.SetFileName(windows::core::PCWSTR(n.as_ptr()));
        d.Show(Some(owner)).ok()?;
        let item = d.GetResult().ok()?;
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok()?;
        CoTaskMemFree(Some(p.0 as *const _));
        Some(std::path::PathBuf::from(s))
    }
}

pub fn copy_text(app: &mut App, id: u64) {
    let Some(doc) = app.captures.docs.get_mut(&id) else { return };
    let Some(path) = doc.temp_file() else { return };
    let thumb = imaging::scale_raw(&doc.rendered(), 88);
    island::show_activity(app, Some(thumb.clone()), "Reading text…", "");
    let langs = app.settings.ocr_extra_languages.clone();
    util::spawn("capture-ocr", move || {
        imaging::com_init();
        let text = crate::ocr::recognize(&path, &langs);
        if let Some(dir) = path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
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

pub fn edit(app: &mut App, id: u64) {
    crate::editor::open(app, id);
}

/// Drag the capture out as a real PNG file.
fn drag_out(app: &mut App, id: u64) {
    let Some(doc) = app.captures.docs.get_mut(&id) else { return };
    let Some(path) = doc.temp_file() else { return };
    let r = doc.rendered();
    let (dw, _) = doc.size();
    let wpx = (160.0f32.min(dw) * doc.scale.min(2.0)) as u32;
    let img = imaging::scale_raw(&r, wpx.max(16));
    ui::defer(move || {
        if let Some(obj) = crate::dragdrop::files_object(&[path]) {
            crate::dragdrop::set_drag_image(&obj, &img, POINT { x: img.w as i32 / 2, y: img.h as i32 / 2 });
            crate::dragdrop::run(&obj);
        }
    });
}

// -- pin -----------------------------------------------------------------------------

/// Float a capture above every window, actual size unless it can't fit.
pub fn pin(app: &mut App, id: u64, near: Option<RECT>) {
    register(w!("ImageVault.Pin"), Some(win_proc));
    let Some(doc) = app.captures.docs.get(&id) else { return };
    let (cx, cy) = monitors::cursor();
    let mon = match near {
        Some(r) => monitors::at_point((r.left + r.right) / 2, (r.top + r.bottom) / 2),
        None => monitors::at_point(cx, cy),
    };
    let Some(mon) = mon else { return };
    let v = mon.work;
    let (dw, dh) = doc.size();
    let scale = mon.scale;
    let (vw, vh) = ((v.right - v.left) as f32 / scale, (v.bottom - v.top) as f32 / scale);
    let fit = (1.0f32).min(vw * 0.9 / dw).min(vh * 0.9 / dh);
    let (w, h) = ((dw * fit).round(), (dh * fit).round());
    let (pw, ph) = ((w * scale).round() as i32, (h * scale).round() as i32);
    let (ox, oy) = match near {
        Some(r) => ((r.left + r.right) / 2 - pw / 2, (r.top + r.bottom) / 2 - ph / 2),
        None => ((v.left + v.right) / 2 - pw / 2, (v.top + v.bottom) / 2 - ph / 2),
    };
    let r = RECT { left: ox, top: oy, right: ox + pw, bottom: oy + ph };
    let hwnd = shadow_window(w!("ImageVault.Pin"), WS_EX_TOPMOST | WS_EX_TOOLWINDOW, r, DWMWCP_ROUNDSMALL);
    app.captures.pins.push(Pin { hwnd, doc: id, surface: None, scale, opacity: 1.0, hover: false, close_hover: false, bmp: None, fade: true, dirty: true });
    app.anims.play(key_id("pin.a", &(hwnd.0 as usize).to_string()), 0.0, 1.0, Curve::linear(0.2));
    render_pins(app);
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
    app.request_frame();
}

fn close_pin(app: &mut App, i: usize) {
    if i < app.captures.pins.len() {
        let p = app.captures.pins.remove(i);
        unsafe {
            let _ = DestroyWindow(p.hwnd);
        }
        app.captures.gc();
    }
}

fn resize_pin(app: &mut App, i: usize, factor: f32) {
    let Some(p) = app.captures.pins.get(i) else { return };
    let Some(doc) = app.captures.docs.get(&p.doc) else { return };
    let (dw, dh) = doc.size();
    unsafe {
        let mut r = RECT::default();
        let _ = GetWindowRect(p.hwnd, &mut r);
        let cur_w = (r.right - r.left) as f32 / p.scale;
        let w = (cur_w * factor).max(60.0).min(dw * 3.0);
        let h = w * dh / dw.max(1.0);
        let (pw, ph) = ((w * p.scale).round() as i32, (h * p.scale).round() as i32);
        let (cx, cy) = ((r.left + r.right) / 2, (r.top + r.bottom) / 2);
        let _ = SetWindowPos(p.hwnd, None, cx - pw / 2, cy - ph / 2, pw, ph, SWP_NOZORDER | SWP_NOACTIVATE);
    }
    if let Some(p) = app.captures.pins.get_mut(i) {
        p.dirty = true;
    }
    render_pins(app);
}

fn pin_menu(app: &mut App, i: usize) {
    let Some(p) = app.captures.pins.get(i) else { return };
    let (id, hwnd, op) = (p.doc, p.hwnd, p.opacity);
    let actual = app.captures.docs.get(&id).map(|d| d.size().0).unwrap_or(1.0);
    ui::defer(move || {
        let mut m = crate::menu::Menu::new();
        m.action("Copy", move || {
            app::with(|a| copy(a, id));
        });
        m.action("Save to Vault", move || {
            app::with(|a| save_to_vault(a, id, None));
        });
        m.action("Save As…", move || {
            app::with(|a| save_as(a, id, hwnd));
        });
        m.action("Copy Text", move || {
            app::with(|a| copy_text(a, id));
        });
        m.action("Edit", move || {
            app::with(|a| edit(a, id));
        });
        m.sep();
        let mut o = crate::menu::Menu::new();
        for v in [1.0f32, 0.75, 0.5, 0.25] {
            o.toggle(format!("{}%", (v * 100.0) as i32), (op - v).abs() < 0.01, move || {
                app::with(|a| {
                    if let Some(p) = a.captures.pins.iter_mut().find(|p| p.hwnd == hwnd) {
                        p.opacity = v;
                        p.dirty = true;
                    }
                    a.request_frame();
                });
            });
        }
        m.sub("Opacity", o);
        m.action("Actual Size", move || {
            app::with(|a| {
                if let Some(i) = a.captures.pins.iter().position(|p| p.hwnd == hwnd) {
                    let mut r = RECT::default();
                    unsafe {
                        let _ = GetWindowRect(hwnd, &mut r);
                    }
                    let cur = (r.right - r.left) as f32 / a.captures.pins[i].scale;
                    resize_pin(a, i, actual / cur.max(1.0));
                }
            });
        });
        m.sep();
        m.action("Close", move || {
            app::with(|a| {
                if let Some(i) = a.captures.pins.iter().position(|p| p.hwnd == hwnd) {
                    close_pin(a, i);
                }
            });
        });
        let (x, y) = monitors::cursor();
        crate::menu::popup(m, hwnd, x, y);
    });
}

// -- rendering ---------------------------------------------------------------------

fn hud_buttons(p: &Preview) -> Vec<Hud> {
    let mut v = vec![Hud { rect: rect(6.0, 2.0, 22.0, 22.0), icon: Icon::XMark, tip: "Close", name: "close" }];
    let actions: [(Icon, &str, &str); 5] = [
        (Icon::Pencil, "Edit", "edit"),
        (Icon::DocOnDoc, "Copy  Ctrl+C", "copy"),
        (Icon::SquareAndArrowDown, "Save to Vault", "save"),
        (Icon::TextViewfinder, "Copy Text", "text"),
        (Icon::Pin, "Pin to Screen", "pin"),
    ];
    let slot = (p.w - 16.0) / actions.len() as f32;
    for (i, (icon, tip, name)) in actions.iter().enumerate() {
        v.push(Hud { rect: rect(8.0 + slot * i as f32 + (slot - 28.0) / 2.0, p.h - 5.0 - 26.0, 28.0, 26.0), icon: *icon, tip, name });
    }
    v
}

fn grip_rect(p: &Preview) -> Rect {
    rect(p.w / 2.0 - 20.0, 2.0, 40.0, 22.0)
}

fn image_rect(p: &Preview) -> Rect {
    rect(8.0, 26.0, p.w - 16.0, p.image_h)
}

/// HUDButton look: hover / on background, white icon, press scale, confirm check.
pub fn draw_hud(cv: &mut Canvas, r: Rect, icon: Icon, size: f32, hovered: bool, on: bool, pressed: bool, confirmed: bool) {
    cv.save();
    if pressed {
        cv.scale_about(0.9, r.cx(), r.cy());
    }
    let a = if on { 0.2 } else if hovered { 0.1 } else { 0.0 };
    cv.fill_round(r, 6.0, Color::white(a));
    let tint = if hovered || on || confirmed { Color::white(1.0) } else { Color::white(0.72) };
    icons::draw(cv, if confirmed { Icon::Checkmark } else { icon }, r.center(), size, Weight::Medium, tint);
    cv.restore();
}

fn render_previews(app: &mut App) {
    let now = util::mono();
    for i in 0..app.captures.previews.len() {
        let (id, scale) = (app.captures.previews[i].doc, app.captures.previews[i].scale);
        let ks = id.to_string();
        let x = app.anims.get(key_id("cap.x", &ks)).unwrap_or(0.0);
        let y = app.anims.get(key_id("cap.y", &ks)).unwrap_or(app.captures.previews[i].target_y);
        let alpha = app.anims.get(key_id("cap.a", &ks)).unwrap_or(1.0);
        let p = &mut app.captures.previews[i];
        // Position.
        let px = |v: f32| (v * scale).round() as i32;
        let (left, top) = (p.work.right - px(16.0 + p.w - x), p.work.bottom - px(16.0 + p.h + y));
        unsafe {
            let flags = SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOSIZE;
            let _ = SetWindowPos(p.hwnd, None, left, top, 0, 0, flags);
        }
        if (p.x - x).abs() > 0.01 || (p.y - y).abs() > 0.01 {
            p.x = x;
            p.y = y;
        }
        let pw = px(p.w) as u32;
        let ph = px(p.h) as u32;
        if p.surface.as_ref().map(|s| s.generation != crate::gfx::generation()).unwrap_or(true) {
            p.surface = Surface::new(p.hwnd, pw, ph, scale).ok();
        }
        let doc_id = p.doc;
        let Some(doc) = app.captures.docs.get_mut(&doc_id) else { continue };
        let p = &mut app.captures.previews[i];
        let version = doc.version;
        let gen_ = crate::gfx::generation();
        if p.bmp.as_ref().map(|(v, g, _)| *v != version || *g != gen_).unwrap_or(true) {
            let r = doc.rendered();
            let small = imaging::scale_raw(&r, (300.0 * scale) as u32);
            p.bmp = crate::gfx::gfx().bitmap(small.w, small.h, &small.px).map(|b| (version, gen_, b));
        }
        let huds = hud_buttons(p);
        let grip = grip_rect(p);
        let img_r = image_rect(p);
        let (w, h) = (p.w, p.h);
        let hover = p.hover;
        let pressed = p.pressed;
        let confirm: Vec<&'static str> = p.confirm.iter().filter(|(_, t)| now < **t).map(|(k, _)| *k).collect();
        let bmp = p.bmp.as_ref().map(|(_, _, b)| b.clone());
        if let Some(s) = &mut p.surface {
            s.draw(|cv| {
                cv.opacity(alpha.clamp(0.0, 1.0));
                let bg = rect(0.0, 0.0, w, h);
                cv.fill_round(bg, 12.0, Color::gray(0.11, 0.97));
                cv.stroke_round(bg, 12.0, 0.5, Color::white(0.16));
                // Drag grip "• • • •".
                for row in 0..2 {
                    for k in 0..4 {
                        let cx = grip.cx() - 9.0 + k as f32 * 6.0;
                        let cy = grip.cy() - 2.0 + row as f32 * 4.0;
                        cv.fill_circle(pt(cx, cy), 1.0, Color::white(0.45));
                    }
                }
                cv.fill_round(img_r, 7.0, Color::black(0.5));
                if let Some(b) = &bmp {
                    let geom = cv.g.squircle(img_r.w, img_r.h, 7.0);
                    let gm: ID2D1Geometry = geom.cast().unwrap();
                    cv.save();
                    cv.translate(img_r.x, img_r.y);
                    cv.push_mask(&gm);
                    cv.restore();
                    cv.image_fit(b, img_r, 1.0, true);
                    cv.pop_layer();
                }
                cv.stroke_round(img_r, 7.0, 0.5, Color::white(0.12));
                for hb in &huds {
                    let size = if hb.name == "close" { 9.5 } else { 12.0 };
                    draw_hud(cv, hb.rect, hb.icon, size, hover == Some(hb.name), false, pressed == Some(hb.name), confirm.contains(&hb.name));
                }
            });
        }
        if !p.shown {
            p.shown = true;
            unsafe {
                let _ = ShowWindow(p.hwnd, SW_SHOWNOACTIVATE);
            }
        }
    }
}

fn render_pins(app: &mut App) {
    for i in 0..app.captures.pins.len() {
        let hwnd = app.captures.pins[i].hwnd;
        let fade = app.anims.get(key_id("pin.a", &(hwnd.0 as usize).to_string())).unwrap_or(1.0);
        let p = &mut app.captures.pins[i];
        let mut r = RECT::default();
        unsafe {
            let _ = GetWindowRect(p.hwnd, &mut r);
        }
        let (pw, ph) = ((r.right - r.left) as u32, (r.bottom - r.top) as u32);
        let scale = p.scale;
        match &mut p.surface {
            Some(s) if s.generation == crate::gfx::generation() => {
                let _ = s.resize(pw, ph, scale);
            }
            _ => p.surface = Surface::new(p.hwnd, pw, ph, scale).ok(),
        }
        let doc_id = p.doc;
        let Some(doc) = app.captures.docs.get_mut(&doc_id) else { continue };
        let p = &mut app.captures.pins[i];
        let version = doc.version;
        let gen_ = crate::gfx::generation();
        if p.bmp.as_ref().map(|(v, g, _)| *v != version || *g != gen_).unwrap_or(true) {
            let rr = doc.rendered();
            p.bmp = crate::gfx::gfx().bitmap(rr.w, rr.h, &rr.px).map(|b| (version, gen_, b));
        }
        let (w, h) = (pw as f32 / scale, ph as f32 / scale);
        let bmp = p.bmp.as_ref().map(|(_, _, b)| b.clone());
        let (op, hover, ch) = (p.opacity * fade.clamp(0.0, 1.0), p.hover, p.close_hover);
        if let Some(s) = &mut p.surface {
            s.draw(|cv| {
                cv.opacity(op);
                let all = rect(0.0, 0.0, w, h);
                cv.fill_rect(all, Color::black(1.0));
                if let Some(b) = &bmp {
                    cv.image(b, all, 1.0, true);
                }
                cv.stroke_rrect(all.inset(0.25, 0.25), 6.0, 0.5, Color::white(0.2));
                if hover {
                    let c = rect(6.0, 6.0, 20.0, 20.0);
                    cv.fill_rrect(c, 10.0, Color::black(0.6));
                    draw_hud(cv, c, Icon::XMark, 9.0, ch, false, false, false);
                }
            });
        }
        p.dirty = false;
    }
}

pub fn frame(app: &mut App, moving: bool) {
    if !app.captures.previews.is_empty() && (moving || app.captures.previews.iter().any(|p| p.dirty)) {
        render_previews(app);
        for p in app.captures.previews.iter_mut() {
            p.dirty = false;
        }
    }
    if !app.captures.pins.is_empty() && (moving || app.captures.pins.iter().any(|p| p.dirty)) {
        render_pins(app);
    }
    crate::editor::frame(app);
}

pub fn animating(app: &App) -> bool {
    crate::editor::animating(app)
}

// -- input -------------------------------------------------------------------------

fn handle(app: &mut App, hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> Option<LRESULT> {
    let (kind, i) = find(app, hwnd)?;
    let (x, y) = util::lparam_xy(l);
    match kind {
        WinKind::Preview => preview_msg(app, i, msg, x, y),
        WinKind::Pin => pin_msg(app, i, hwnd, msg, w, l, x, y),
    }
}

fn preview_hit(p: &Preview, pnt: Pt) -> Option<&'static str> {
    hud_buttons(p).into_iter().find(|h| h.rect.contains(pnt)).map(|h| h.name)
}

fn preview_msg(app: &mut App, i: usize, msg: u32, x: i32, y: i32) -> Option<LRESULT> {
    let p = &mut app.captures.previews[i];
    let pnt = pt(x as f32 / p.scale, y as f32 / p.scale);
    match msg {
        WM_MOUSEACTIVATE => Some(LRESULT(MA_NOACTIVATE as isize)),
        WM_MOUSEMOVE => {
            let h = preview_hit(p, pnt);
            if h != p.hover {
                p.hover = h;
                p.dirty = true;
                ui::animate(true);
            }
            unsafe {
                let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: p.hwnd, dwHoverTime: 0 };
                let _ = TrackMouseEvent(&mut tme);
            }
            let p = &app.captures.previews[i];
            let tip = if let Some(name) = h {
                hud_buttons(p).into_iter().find(|b| b.name == name).map(|b| b.tip.to_string())
            } else if image_rect(p).contains(pnt) {
                Some("Drag into any app · double-click to edit".to_string())
            } else if grip_rect(p).contains(pnt) {
                Some("Drag into any app".to_string())
            } else {
                None
            };
            app.tip.hover(tip);
            // Drag out (image after 3 pt, grip immediately).
            let p = &mut app.captures.previews[i];
            if let Some(d) = p.down {
                if pnt.dist(d) > 3.0 && unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) } < 0 {
                    p.down = None;
                    let id = p.doc;
                    app.tip.dismiss();
                    drag_out(app, id);
                }
            }
            Some(LRESULT(0))
        }
        0x02A3 => {
            p.hover = None;
            p.dirty = true;
            app.tip.hover(None);
            ui::animate(true);
            Some(LRESULT(0))
        }
        WM_SETCURSOR => {
            let (cx, cy) = monitors::cursor();
            let mut r = RECT::default();
            unsafe {
                let _ = GetWindowRect(p.hwnd, &mut r);
            }
            let lp = pt((cx - r.left) as f32 / p.scale, (cy - r.top) as f32 / p.scale);
            let c = if grip_rect(p).contains(lp) { IDC_SIZEALL } else { IDC_ARROW };
            unsafe {
                SetCursor(LoadCursorW(None, c).ok());
            }
            Some(LRESULT(1))
        }
        WM_LBUTTONDOWN => {
            app.tip.dismiss();
            if let Some(h) = preview_hit(p, pnt) {
                p.pressed = Some(h);
                p.dirty = true;
            } else if image_rect(p).contains(pnt) || grip_rect(p).contains(pnt) {
                p.down = Some(pnt);
            }
            ui::animate(true);
            Some(LRESULT(0))
        }
        WM_LBUTTONDBLCLK => {
            if image_rect(p).contains(pnt) {
                let id = p.doc;
                p.down = None;
                edit(app, id);
            }
            Some(LRESULT(0))
        }
        WM_LBUTTONUP => {
            p.down = None;
            let pressed = p.pressed.take();
            p.dirty = true;
            let id = p.doc;
            if let Some(name) = pressed {
                if preview_hit(&app.captures.previews[i], pnt) == Some(name) {
                    let confirm_until = util::mono() + 1.2;
                    match name {
                        "close" => dismiss_preview(app, i),
                        "edit" => edit(app, id),
                        "copy" => {
                            copy(app, id);
                            app.captures.previews[i].confirm.insert("copy", confirm_until);
                        }
                        "save" => {
                            save_to_vault(app, id, None);
                            app.captures.previews[i].confirm.insert("save", confirm_until);
                        }
                        "text" => copy_text(app, id),
                        "pin" => {
                            pin(app, id, None);
                            if let Some(j) = app.captures.previews.iter().position(|p| p.doc == id) {
                                dismiss_preview(app, j);
                            }
                        }
                        _ => {}
                    }
                    ui::after(1.25, move || {
                        app::with(|a| {
                            if let Some(p) = a.captures.previews.iter_mut().find(|p| p.doc == id) {
                                p.dirty = true;
                            }
                            a.request_frame();
                        });
                    });
                }
            }
            ui::animate(true);
            Some(LRESULT(0))
        }
        _ => None,
    }
}

fn pin_msg(app: &mut App, i: usize, hwnd: HWND, msg: u32, w: WPARAM, _l: LPARAM, x: i32, y: i32) -> Option<LRESULT> {
    let scale = app.captures.pins[i].scale;
    match msg {
        WM_NCHITTEST => {
            // Drag anywhere to move, except the close button while hovering.
            let mut p = POINT { x, y };
            unsafe {
                let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut p);
            }
            let lp = pt(p.x as f32 / scale, p.y as f32 / scale);
            let pin = &mut app.captures.pins[i];
            let over_close = rect(6.0, 6.0, 20.0, 20.0).contains(lp);
            if !pin.hover {
                pin.hover = true;
                pin.dirty = true;
                ui::animate(true);
                unsafe {
                    let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE | TME_NONCLIENT, hwndTrack: hwnd, dwHoverTime: 0 };
                    let _ = TrackMouseEvent(&mut tme);
                }
            }
            if pin.close_hover != over_close {
                pin.close_hover = over_close;
                pin.dirty = true;
                ui::animate(true);
            }
            Some(LRESULT(if over_close { HTCLIENT as isize } else { HTCAPTION as isize }))
        }
        WM_MOUSEMOVE => {
            let pin = &mut app.captures.pins[i];
            if !pin.hover {
                pin.hover = true;
                pin.dirty = true;
                app.request_frame();
            }
            unsafe {
                let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                let _ = TrackMouseEvent(&mut tme);
            }
            app.tip.hover(Some("Close  Esc".into()));
            Some(LRESULT(0))
        }
        0x02A3 | 0x02A2 => {
            // WM_MOUSELEAVE / WM_NCMOUSELEAVE: hide the close button unless still inside.
            let (cx, cy) = monitors::cursor();
            let mut r = RECT::default();
            unsafe {
                let _ = GetWindowRect(hwnd, &mut r);
            }
            let inside = cx >= r.left && cx < r.right && cy >= r.top && cy < r.bottom;
            let pin = &mut app.captures.pins[i];
            if !inside {
                pin.hover = false;
                pin.close_hover = false;
                pin.dirty = true;
                app.request_frame();
            }
            app.tip.hover(None);
            Some(LRESULT(0))
        }
        WM_LBUTTONUP => {
            if rect(6.0, 6.0, 20.0, 20.0).contains(pt(x as f32 / scale, y as f32 / scale)) {
                close_pin(app, i);
            }
            Some(LRESULT(0))
        }
        WM_NCLBUTTONDBLCLK => {
            let id = app.captures.pins[i].doc;
            edit(app, id);
            Some(LRESULT(0))
        }
        WM_NCRBUTTONUP | WM_RBUTTONUP => {
            pin_menu(app, i);
            Some(LRESULT(0))
        }
        WM_NCRBUTTONDOWN => Some(LRESULT(0)),
        WM_MOUSEWHEEL => {
            let delta = ((w.0 >> 16) & 0xffff) as i16 as f32;
            resize_pin(app, i, 1.0 + (delta / 120.0) / 20.0);
            Some(LRESULT(0))
        }
        WM_KEYDOWN => {
            let vk = w.0 as u16;
            let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
            if vk == VK_ESCAPE.0 {
                close_pin(app, i);
            } else if ctrl && vk == b'C' as u16 {
                let id = app.captures.pins[i].doc;
                copy(app, id);
            } else {
                return None;
            }
            Some(LRESULT(0))
        }
        WM_SIZE | WM_DPICHANGED => {
            if msg == WM_DPICHANGED {
                app.captures.pins[i].scale = (w.0 & 0xffff) as f32 / 96.0;
            }
            app.captures.pins[i].dirty = true;
            app.request_frame();
            None
        }
        _ => None,
    }
}

pub fn _rf(r: Rect) -> D2D_RECT_F {
    rf(r)
}
