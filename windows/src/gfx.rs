//! Rendering: one shared D3D11 / Direct2D / DirectComposition / DirectWrite
//! stack on the UI thread. Every window owns a premultiplied-alpha composition
//! swap chain; drawing happens in DIPs (points) through `Canvas`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::mem::ManuallyDrop;
use std::rc::Rc;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::core::{Interface, PCWSTR, Result};
use windows_numerics::{Matrix3x2, Vector2};

use crate::theme::{Color, Font, Pt, Rect};
use crate::util::wide;

pub fn col(c: Color) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: c.r, g: c.g, b: c.b, a: c.a }
}
pub fn rf(r: Rect) -> D2D_RECT_F {
    D2D_RECT_F { left: r.x, top: r.y, right: r.x + r.w, bottom: r.y + r.h }
}
pub fn p2(p: Pt) -> Vector2 {
    Vector2 { X: p.x, Y: p.y }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct FmtKey {
    mono: bool,
    weight: i32,
    size: i32,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct LayoutKey {
    text: String,
    fmt: FmtKey,
    kern: i32,
    max_w: i32,
    max_h: i32,
    wrap: bool,
    align: u8,
    trim: u8,
}

/// How text is laid out inside its rect.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Align {
    Leading,
    Center,
    Trailing,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Trim {
    /// Clip, no ellipsis.
    None,
    /// "…" at the end (truncationMode .tail).
    Tail,
    /// "…" in the middle (lineBreakMode .byTruncatingMiddle).
    Middle,
}

pub struct Gfx {
    pub d3d: ID3D11Device,
    pub dxgi: IDXGIDevice,
    pub factory: IDXGIFactory2,
    pub d2f: ID2D1Factory1,
    pub d2d: ID2D1Device,
    pub dc: ID2D1DeviceContext,
    pub dcomp: IDCompositionDevice,
    pub dw: IDWriteFactory,
    pub sans: Vec<u16>,
    pub mono: Vec<u16>,
    pub generation: u32,
    brush: ID2D1SolidColorBrush,
    formats: RefCell<HashMap<FmtKey, IDWriteTextFormat>>,
    layouts: RefCell<HashMap<LayoutKey, (IDWriteTextLayout, u64)>>,
    layout_tick: std::cell::Cell<u64>,
    squircles: RefCell<HashMap<(i32, i32, i32), ID2D1PathGeometry>>,
    strokes: RefCell<HashMap<(i32, i32, i32, u8), ID2D1StrokeStyle>>,
    gradients: RefCell<HashMap<Vec<u32>, ID2D1GradientStopCollection>>,
    ellipsis: RefCell<HashMap<FmtKey, IDWriteInlineObject>>,
}

thread_local! {
    static GFX: RefCell<Option<Rc<Gfx>>> = const { RefCell::new(None) };
    static GENERATION: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// The UI thread's graphics stack (created on first use, re-created after a
/// device loss via `reset`).
pub fn gfx() -> Rc<Gfx> {
    GFX.with(|g| {
        if let Some(x) = g.borrow().as_ref() {
            return x.clone();
        }
        let gen_ = GENERATION.with(|c| {
            c.set(c.get() + 1);
            c.get()
        });
        let created = Rc::new(Gfx::create(gen_).expect("graphics device"));
        *g.borrow_mut() = Some(created.clone());
        created
    })
}

/// Drop the device (after DXGI_ERROR_DEVICE_REMOVED); windows recreate their
/// surfaces on the next frame.
pub fn reset() {
    GFX.with(|g| *g.borrow_mut() = None);
}

pub fn generation() -> u32 {
    GENERATION.with(|c| c.get())
}

fn family_exists(dw: &IDWriteFactory, name: &str) -> bool {
    unsafe {
        let mut coll: Option<IDWriteFontCollection> = None;
        if dw.GetSystemFontCollection(&mut coll, false).is_err() {
            return false;
        }
        let Some(coll) = coll else { return false };
        let mut index = 0u32;
        let mut exists = windows::core::BOOL(0);
        let w = wide(name);
        coll.FindFamilyName(PCWSTR(w.as_ptr()), &mut index, &mut exists).is_ok() && exists.as_bool()
    }
}

impl Gfx {
    fn create(generation: u32) -> Result<Gfx> {
        unsafe {
            let mut device: Option<ID3D11Device> = None;
            let flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT;
            let hw = D3D11CreateDevice(
                None::<&IDXGIAdapter>,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                flags,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            );
            if hw.is_err() || device.is_none() {
                D3D11CreateDevice(
                    None::<&IDXGIAdapter>,
                    D3D_DRIVER_TYPE_WARP,
                    HMODULE::default(),
                    flags,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    None,
                )?;
            }
            let d3d = device.unwrap();
            let dxgi: IDXGIDevice = d3d.cast()?;
            let factory: IDXGIFactory2 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;
            let d2f: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let d2d = d2f.CreateDevice(&dxgi)?;
            let dc = d2d.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            dc.SetUnitMode(D2D1_UNIT_MODE_DIPS);
            let dcomp: IDCompositionDevice = DCompositionCreateDevice(&dxgi)?;
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let brush = dc.CreateSolidColorBrush(&col(Color::white(1.0)), None)?;
            let sans = if family_exists(&dw, "Segoe UI Variable Text") { "Segoe UI Variable Text" } else { "Segoe UI" };
            let mono = if family_exists(&dw, "Cascadia Mono") { "Cascadia Mono" } else { "Consolas" };
            Ok(Gfx {
                d3d,
                dxgi,
                factory,
                d2f,
                d2d,
                dc,
                dcomp,
                dw,
                sans: wide(sans),
                mono: wide(mono),
                generation,
                brush,
                formats: RefCell::new(HashMap::new()),
                layouts: RefCell::new(HashMap::new()),
                layout_tick: std::cell::Cell::new(0),
                squircles: RefCell::new(HashMap::new()),
                strokes: RefCell::new(HashMap::new()),
                gradients: RefCell::new(HashMap::new()),
                ellipsis: RefCell::new(HashMap::new()),
            })
        }
    }

    pub fn brush(&self, c: Color) -> &ID2D1SolidColorBrush {
        unsafe { self.brush.SetColor(&col(c)) };
        &self.brush
    }

    fn fmt_key(font: &Font) -> FmtKey {
        FmtKey { mono: font.mono, weight: font.weight.dwrite(), size: (font.size * 100.0).round() as i32 }
    }

    pub fn format(&self, font: &Font) -> IDWriteTextFormat {
        let k = Self::fmt_key(font);
        if let Some(f) = self.formats.borrow().get(&k) {
            return f.clone();
        }
        let fam = if font.mono { &self.mono } else { &self.sans };
        let f = unsafe {
            self.dw
                .CreateTextFormat(
                    PCWSTR(fam.as_ptr()),
                    None,
                    DWRITE_FONT_WEIGHT(font.weight.dwrite()),
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    font.size,
                    windows::core::w!("en-us"),
                )
                .expect("text format")
        };
        unsafe {
            let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
        }
        self.formats.borrow_mut().insert(k, f.clone());
        f
    }

    fn ellipsis_for(&self, font: &Font, fmt: &IDWriteTextFormat) -> Option<IDWriteInlineObject> {
        let k = Self::fmt_key(font);
        if let Some(e) = self.ellipsis.borrow().get(&k) {
            return Some(e.clone());
        }
        let e = unsafe { self.dw.CreateEllipsisTrimmingSign(fmt).ok()? };
        self.ellipsis.borrow_mut().insert(k, e.clone());
        Some(e)
    }

    /// Cached text layout.
    pub fn layout(&self, text: &str, font: &Font, max_w: f32, max_h: f32, wrap: bool, align: Align, trim: Trim) -> IDWriteTextLayout {
        let key = LayoutKey {
            text: text.to_string(),
            fmt: Self::fmt_key(font),
            kern: (font.kern * 100.0).round() as i32,
            max_w: (max_w.min(100000.0) * 4.0).round() as i32,
            max_h: (max_h.min(100000.0) * 4.0).round() as i32,
            wrap,
            align: align as u8,
            trim: trim as u8,
        };
        let tick = self.layout_tick.get() + 1;
        self.layout_tick.set(tick);
        if let Some((l, t)) = self.layouts.borrow_mut().get_mut(&key) {
            *t = tick;
            return l.clone();
        }
        let fmt = self.format(font);
        let w: Vec<u16> = text.encode_utf16().collect();
        let layout = unsafe {
            self.dw
                .CreateTextLayout(&w, &fmt, max_w.max(0.0).min(100000.0), max_h.max(0.0).min(100000.0))
                .expect("text layout")
        };
        unsafe {
            let _ = layout.SetWordWrapping(if wrap { DWRITE_WORD_WRAPPING_WRAP } else { DWRITE_WORD_WRAPPING_NO_WRAP });
            let _ = layout.SetTextAlignment(match align {
                Align::Leading => DWRITE_TEXT_ALIGNMENT_LEADING,
                Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                Align::Trailing => DWRITE_TEXT_ALIGNMENT_TRAILING,
            });
            if trim == Trim::Tail {
                let sign = self.ellipsis_for(font, &fmt);
                let t = DWRITE_TRIMMING {
                    granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                    delimiter: 0,
                    delimiterCount: 0,
                };
                let _ = layout.SetTrimming(&t, sign.as_ref());
            }
            if font.kern != 0.0 && !w.is_empty() {
                if let Ok(l1) = layout.cast::<IDWriteTextLayout1>() {
                    let _ = l1.SetCharacterSpacing(0.0, font.kern, 0.0, DWRITE_TEXT_RANGE { startPosition: 0, length: w.len() as u32 });
                }
            }
        }
        let mut map = self.layouts.borrow_mut();
        if map.len() > 900 {
            // Drop the least recently used half.
            let mut ticks: Vec<u64> = map.values().map(|(_, t)| *t).collect();
            ticks.sort_unstable();
            let cut = ticks[ticks.len() / 2];
            map.retain(|_, (_, t)| *t > cut);
        }
        map.insert(key, (layout.clone(), tick));
        layout
    }

    /// Width / height of a single line of text (no wrapping).
    pub fn measure(&self, text: &str, font: &Font) -> (f32, f32) {
        let l = self.layout(text, font, 100000.0, 100000.0, false, Align::Leading, Trim::None);
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe {
            let _ = l.GetMetrics(&mut m);
        }
        (m.widthIncludingTrailingWhitespace, m.height)
    }

    pub fn line_height(&self, font: &Font) -> f32 {
        self.measure("Ag", font).1
    }

    pub fn squircle(&self, w: f32, h: f32, r: f32) -> ID2D1PathGeometry {
        let k = ((w * 4.0).round() as i32, (h * 4.0).round() as i32, (r * 4.0).round() as i32);
        if let Some(g) = self.squircles.borrow().get(&k) {
            return g.clone();
        }
        let g = squircle_geometry(&self.d2f, Rect { x: 0.0, y: 0.0, w, h }, r);
        let mut map = self.squircles.borrow_mut();
        if map.len() > 400 {
            map.clear();
        }
        map.insert(k, g.clone());
        g
    }

    /// Stroke style: dashes (in multiples of the line width are NOT used —
    /// we pass absolute DIP lengths), offset, cap (0 flat, 1 round, 2 square).
    pub fn stroke_style(&self, dash: Option<(f32, f32)>, offset: f32, width: f32, cap: u8) -> ID2D1StrokeStyle {
        let k = match dash {
            Some((a, b)) => ((a * 100.0) as i32, (b * 100.0) as i32, ((offset * 100.0) as i32) * 1000 + (width * 100.0) as i32, cap),
            None => (0, 0, 0, cap),
        };
        if let Some(s) = self.strokes.borrow().get(&k) {
            return s.clone();
        }
        let capv = match cap {
            1 => D2D1_CAP_STYLE_ROUND,
            2 => D2D1_CAP_STYLE_SQUARE,
            _ => D2D1_CAP_STYLE_FLAT,
        };
        let props = D2D1_STROKE_STYLE_PROPERTIES1 {
            startCap: capv,
            endCap: capv,
            dashCap: capv,
            lineJoin: if cap == 1 { D2D1_LINE_JOIN_ROUND } else { D2D1_LINE_JOIN_MITER },
            miterLimit: 10.0,
            dashStyle: if dash.is_some() { D2D1_DASH_STYLE_CUSTOM } else { D2D1_DASH_STYLE_SOLID },
            dashOffset: 0.0,
            transformType: D2D1_STROKE_TRANSFORM_TYPE_NORMAL,
        };
        let s = unsafe {
            match dash {
                Some((a, b)) => {
                    // D2D dash lengths are in units of stroke width.
                    let wv = width.max(0.01);
                    let mut p = props;
                    p.dashOffset = offset / wv;
                    let arr = [a / wv, b / wv];
                    self.d2f.CreateStrokeStyle(&p, Some(&arr))
                }
                None => self.d2f.CreateStrokeStyle(&props, None),
            }
        }
        .expect("stroke style");
        let s: ID2D1StrokeStyle = s.cast().unwrap();
        let mut map = self.strokes.borrow_mut();
        if map.len() > 300 {
            map.clear();
        }
        map.insert(k, s.clone());
        s
    }

    pub fn gradient(&self, stops: &[(f32, Color)]) -> Option<ID2D1GradientStopCollection> {
        let key: Vec<u32> = stops
            .iter()
            .flat_map(|(p, c)| [(p * 1000.0) as u32, (c.r * 255.0) as u32, (c.g * 255.0) as u32, (c.b * 255.0) as u32, (c.a * 1000.0) as u32])
            .collect();
        if let Some(g) = self.gradients.borrow().get(&key) {
            return Some(g.clone());
        }
        let s: Vec<D2D1_GRADIENT_STOP> = stops.iter().map(|(p, c)| D2D1_GRADIENT_STOP { position: *p, color: col(*c) }).collect();
        let rt: &ID2D1RenderTarget = &self.dc;
        let g = unsafe { rt.CreateGradientStopCollection(&s, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP).ok()? };
        let mut map = self.gradients.borrow_mut();
        if map.len() > 64 {
            map.clear();
        }
        map.insert(key, g.clone());
        Some(g)
    }

    pub fn path(&self) -> PathB {
        PathB::new(&self.d2f)
    }

    /// Tile checkerboard: 14 pt cell, two 7 pt squares of white 0.07 on #0C0C0C.
    pub fn checker(&self) -> Option<ID2D1BitmapBrush> {
        let n = 14usize;
        let mut px = vec![0u8; n * n * 4];
        for y in 0..n {
            for x in 0..n {
                let alt = (x < 7) == (y < 7);
                let v = if alt { (0.07f32 * 255.0) as u8 } else { (0.047f32 * 255.0) as u8 };
                let i = (y * n + x) * 4;
                px[i] = v;
                px[i + 1] = v;
                px[i + 2] = v;
                px[i + 3] = 255;
            }
        }
        let bmp = self.bitmap(n as u32, n as u32, &px)?;
        unsafe {
            let props = D2D1_BITMAP_BRUSH_PROPERTIES1 {
                extendModeX: D2D1_EXTEND_MODE_WRAP,
                extendModeY: D2D1_EXTEND_MODE_WRAP,
                interpolationMode: D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
            };
            let b = self.dc.CreateBitmapBrush(&bmp, Some(&props), None).ok()?;
            Some(b.cast().ok()?)
        }
    }

    /// Upload premultiplied BGRA pixels as a bitmap.
    pub fn bitmap(&self, w: u32, h: u32, bgra: &[u8]) -> Option<ID2D1Bitmap1> {
        if w == 0 || h == 0 || bgra.len() < (w * h * 4) as usize {
            return None;
        }
        let props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
            colorContext: ManuallyDrop::new(None),
        };
        unsafe {
            self.dc
                .CreateBitmap(D2D_SIZE_U { width: w, height: h }, Some(bgra.as_ptr() as *const _), w * 4, &props)
                .ok()
        }
    }
}

/// iOS-style continuous ("squircle") rounded rect, the RoundedRectangle
/// `.continuous` style used throughout the Mac UI. Falls back to circular
/// corners when the radius is too large for full smoothing (capsules).
pub fn squircle_geometry(f: &ID2D1Factory1, r: Rect, radius: f32) -> ID2D1PathGeometry {
    let mut p = PathB::new(f);
    let min = r.w.min(r.h);
    let radius = radius.max(0.0);
    if radius <= 0.01 {
        p.rect(r);
        return p.finish();
    }
    if radius * 1.528_665 > min / 2.0 {
        let rr = radius.min(min / 2.0);
        p.round_rect(r, rr);
        return p.finish();
    }
    const SEQ: [(f32, f32); 13] = [
        (1.528_665, 0.0),
        (1.088_493, 0.0),
        (0.868_407, 0.0),
        (0.669_934, 0.065_496),
        (0.631_494, 0.074_911),
        (0.372_824, 0.169_059),
        (0.169_060, 0.372_824),
        (0.074_912, 0.631_494),
        (0.0, 0.868_407),
        (0.0, 1.088_493),
        (0.0, 1.528_665),
        (0.0, 0.0),
        (0.0, 0.0),
    ];
    // Corners in clockwise order: (corner point, incoming dir, outgoing dir).
    let corners = [
        ((r.r(), r.y), (1.0, 0.0), (0.0, 1.0)),
        ((r.r(), r.b()), (0.0, 1.0), (-1.0, 0.0)),
        ((r.x, r.b()), (-1.0, 0.0), (0.0, -1.0)),
        ((r.x, r.y), (0.0, -1.0), (1.0, 0.0)),
    ];
    let map = |c: ((f32, f32), (f32, f32), (f32, f32)), a: f32, b: f32| -> Pt {
        let ((cx, cy), (ix, iy), (ox, oy)) = c;
        Pt { x: cx - ix * a * radius + ox * b * radius, y: cy - iy * a * radius + oy * b * radius }
    };
    let start = map(corners[0], SEQ[0].0, SEQ[0].1);
    p.move_to(start);
    for (i, c) in corners.iter().enumerate() {
        let c = *c;
        let s0 = map(c, SEQ[0].0, SEQ[0].1);
        if i > 0 {
            p.line_to(s0);
        }
        p.cubic_to(map(c, SEQ[1].0, SEQ[1].1), map(c, SEQ[2].0, SEQ[2].1), map(c, SEQ[3].0, SEQ[3].1));
        p.line_to(map(c, SEQ[4].0, SEQ[4].1));
        p.cubic_to(map(c, SEQ[5].0, SEQ[5].1), map(c, SEQ[6].0, SEQ[6].1), map(c, SEQ[7].0, SEQ[7].1));
        p.cubic_to(map(c, SEQ[8].0, SEQ[8].1), map(c, SEQ[9].0, SEQ[9].1), map(c, SEQ[10].0, SEQ[10].1));
    }
    p.close();
    p.finish()
}

/// Path builder over ID2D1GeometrySink.
pub struct PathB {
    geom: ID2D1PathGeometry,
    sink: ID2D1GeometrySink,
    open: bool,
    filled: bool,
    pub cur: Pt,
}

impl PathB {
    pub fn new(f: &ID2D1Factory1) -> PathB {
        unsafe {
            let geom: ID2D1PathGeometry = f.CreatePathGeometry().expect("path").into();
            let sink = geom.Open().expect("sink");
            PathB { geom, sink, open: false, filled: true, cur: Pt::default() }
        }
    }
    pub fn alternate(self) -> Self {
        unsafe { self.sink.SetFillMode(D2D1_FILL_MODE_ALTERNATE) };
        self
    }
    pub fn winding(self) -> Self {
        unsafe { self.sink.SetFillMode(D2D1_FILL_MODE_WINDING) };
        self
    }
    /// Figures begin hollow (for open stroked polylines).
    pub fn hollow(mut self) -> Self {
        self.filled = false;
        self
    }
    fn end_open(&mut self) {
        if self.open {
            unsafe { self.sink.EndFigure(D2D1_FIGURE_END_OPEN) };
            self.open = false;
        }
    }
    pub fn move_to(&mut self, p: Pt) {
        self.end_open();
        unsafe {
            self.sink.BeginFigure(p2(p), if self.filled { D2D1_FIGURE_BEGIN_FILLED } else { D2D1_FIGURE_BEGIN_HOLLOW });
        }
        self.open = true;
        self.cur = p;
    }
    pub fn line_to(&mut self, p: Pt) {
        unsafe { self.sink.AddLine(p2(p)) };
        self.cur = p;
    }
    pub fn quad_to(&mut self, c: Pt, p: Pt) {
        unsafe { self.sink.AddQuadraticBezier(&D2D1_QUADRATIC_BEZIER_SEGMENT { point1: p2(c), point2: p2(p) }) };
        self.cur = p;
    }
    pub fn cubic_to(&mut self, c1: Pt, c2: Pt, p: Pt) {
        unsafe { self.sink.AddBezier(&D2D1_BEZIER_SEGMENT { point1: p2(c1), point2: p2(c2), point3: p2(p) }) };
        self.cur = p;
    }
    /// Arc to `p` with radius, sweep clockwise (in y-down space) when `cw`.
    pub fn arc_to(&mut self, p: Pt, rx: f32, ry: f32, cw: bool, large: bool) {
        unsafe {
            self.sink.AddArc(&D2D1_ARC_SEGMENT {
                point: p2(p),
                size: D2D_SIZE_F { width: rx, height: ry },
                rotationAngle: 0.0,
                sweepDirection: if cw { D2D1_SWEEP_DIRECTION_CLOCKWISE } else { D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE },
                arcSize: if large { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
            })
        };
        self.cur = p;
    }
    pub fn close(&mut self) {
        if self.open {
            unsafe { self.sink.EndFigure(D2D1_FIGURE_END_CLOSED) };
            self.open = false;
        }
    }
    pub fn rect(&mut self, r: Rect) {
        self.move_to(Pt { x: r.x, y: r.y });
        self.line_to(Pt { x: r.r(), y: r.y });
        self.line_to(Pt { x: r.r(), y: r.b() });
        self.line_to(Pt { x: r.x, y: r.b() });
        self.close();
    }
    /// Circular-corner rounded rect.
    pub fn round_rect(&mut self, r: Rect, rad: f32) {
        let rad = rad.min(r.w / 2.0).min(r.h / 2.0).max(0.0);
        if rad <= 0.01 {
            self.rect(r);
            return;
        }
        self.move_to(Pt { x: r.x + rad, y: r.y });
        self.line_to(Pt { x: r.r() - rad, y: r.y });
        self.arc_to(Pt { x: r.r(), y: r.y + rad }, rad, rad, true, false);
        self.line_to(Pt { x: r.r(), y: r.b() - rad });
        self.arc_to(Pt { x: r.r() - rad, y: r.b() }, rad, rad, true, false);
        self.line_to(Pt { x: r.x + rad, y: r.b() });
        self.arc_to(Pt { x: r.x, y: r.b() - rad }, rad, rad, true, false);
        self.line_to(Pt { x: r.x, y: r.y + rad });
        self.arc_to(Pt { x: r.x + rad, y: r.y }, rad, rad, true, false);
        self.close();
    }
    pub fn circle(&mut self, c: Pt, r: f32) {
        self.move_to(Pt { x: c.x + r, y: c.y });
        self.arc_to(Pt { x: c.x - r, y: c.y }, r, r, true, false);
        self.arc_to(Pt { x: c.x + r, y: c.y }, r, r, true, false);
        self.close();
    }
    pub fn ellipse(&mut self, r: Rect) {
        let (rx, ry) = (r.w / 2.0, r.h / 2.0);
        self.move_to(Pt { x: r.r(), y: r.cy() });
        self.arc_to(Pt { x: r.x, y: r.cy() }, rx, ry, true, false);
        self.arc_to(Pt { x: r.r(), y: r.cy() }, rx, ry, true, false);
        self.close();
    }
    pub fn finish(mut self) -> ID2D1PathGeometry {
        self.end_open();
        unsafe {
            let _ = self.sink.Close();
        }
        self.geom
    }
}

// ---------------------------------------------------------------------------
// Surface (one per window)
// ---------------------------------------------------------------------------

pub struct Surface {
    pub hwnd: HWND,
    swapchain: IDXGISwapChain1,
    target: Option<ID2D1Bitmap1>,
    _comp: IDCompositionTarget,
    _visual: IDCompositionVisual,
    pub px_w: u32,
    pub px_h: u32,
    pub scale: f32,
    pub generation: u32,
}

impl Surface {
    pub fn new(hwnd: HWND, px_w: u32, px_h: u32, scale: f32) -> Result<Surface> {
        let g = gfx();
        let px_w = px_w.max(1);
        let px_h = px_h.max(1);
        unsafe {
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: px_w,
                Height: px_h,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                ..Default::default()
            };
            let swapchain = g.factory.CreateSwapChainForComposition(&g.d3d, &desc, None)?;
            let comp = g.dcomp.CreateTargetForHwnd(hwnd, true)?;
            let visual = g.dcomp.CreateVisual()?;
            visual.SetContent(&swapchain)?;
            comp.SetRoot(&visual)?;
            let mut s = Surface {
                hwnd,
                swapchain,
                target: None,
                _comp: comp,
                _visual: visual,
                px_w,
                px_h,
                scale,
                generation: g.generation,
            };
            s.make_target()?;
            Ok(s)
        }
    }

    fn make_target(&mut self) -> Result<()> {
        let g = gfx();
        unsafe {
            let surface: IDXGISurface = self.swapchain.GetBuffer(0)?;
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0 * self.scale,
                dpiY: 96.0 * self.scale,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                colorContext: ManuallyDrop::new(None),
            };
            self.target = Some(g.dc.CreateBitmapFromDxgiSurface(&surface, Some(&props))?);
        }
        Ok(())
    }

    pub fn resize(&mut self, px_w: u32, px_h: u32, scale: f32) -> Result<()> {
        let px_w = px_w.max(1);
        let px_h = px_h.max(1);
        if px_w == self.px_w && px_h == self.px_h && (scale - self.scale).abs() < 1e-4 {
            return Ok(());
        }
        self.target = None;
        unsafe {
            let g = gfx();
            g.dc.SetTarget(None);
            self.swapchain.ResizeBuffers(0, px_w, px_h, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0))?;
        }
        self.px_w = px_w;
        self.px_h = px_h;
        self.scale = scale;
        self.make_target()
    }

    /// Size in DIPs.
    pub fn dip_size(&self) -> (f32, f32) {
        (self.px_w as f32 / self.scale, self.px_h as f32 / self.scale)
    }

    /// Draw a frame. Returns false when the device was lost (caller should
    /// recreate the surface).
    pub fn draw(&mut self, f: impl FnOnce(&mut Canvas)) -> bool {
        let g = gfx();
        if g.generation != self.generation {
            return false;
        }
        let Some(target) = self.target.clone() else { return false };
        unsafe {
            g.dc.SetTarget(&target);
            g.dc.SetDpi(96.0 * self.scale, 96.0 * self.scale);
            g.dc.BeginDraw();
            g.dc.SetTransform(&Matrix3x2::identity());
            g.dc.Clear(Some(&col(Color::CLEAR)));
        }
        let mut c = Canvas::new(g.clone(), self.scale);
        f(&mut c);
        drop(c);
        let ok = unsafe {
            let r = g.dc.EndDraw(None, None);
            g.dc.SetTarget(None);
            r.is_ok()
        };
        if !ok {
            reset();
            return false;
        }
        let hr = unsafe { self.swapchain.Present(0, DXGI_PRESENT(0)) };
        if hr.is_err() {
            reset();
            return false;
        }
        unsafe {
            let _ = g.dcomp.Commit();
        }
        true
    }
}

/// Render off-screen at `scale` and read the pixels back (premultiplied BGRA).
/// Used for exports (screenshots with annotations) and design snapshots.
pub fn offscreen(px_w: u32, px_h: u32, scale: f32, f: impl FnOnce(&mut Canvas)) -> Option<crate::imaging::Raw> {
    let g = gfx();
    unsafe {
        let size = D2D_SIZE_U { width: px_w.max(1), height: px_h.max(1) };
        let props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0 * scale,
            dpiY: 96.0 * scale,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
            colorContext: ManuallyDrop::new(None),
        };
        let target = g.dc.CreateBitmap(size, None, 0, &props).ok()?;
        let read_props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: props.pixelFormat,
            dpiX: props.dpiX,
            dpiY: props.dpiY,
            bitmapOptions: D2D1_BITMAP_OPTIONS_CPU_READ | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
            colorContext: ManuallyDrop::new(None),
        };
        let read = g.dc.CreateBitmap(size, None, 0, &read_props).ok()?;
        g.dc.SetTarget(&target);
        g.dc.SetDpi(96.0 * scale, 96.0 * scale);
        g.dc.BeginDraw();
        g.dc.SetTransform(&Matrix3x2::identity());
        g.dc.Clear(Some(&col(Color::CLEAR)));
        let mut c = Canvas::new(g.clone(), scale);
        f(&mut c);
        drop(c);
        let ok = g.dc.EndDraw(None, None).is_ok();
        g.dc.SetTarget(None);
        if !ok {
            return None;
        }
        read.CopyFromBitmap(None, &target, None).ok()?;
        let map = read.Map(D2D1_MAP_OPTIONS_READ).ok()?;
        let mut px = vec![0u8; (size.width * size.height * 4) as usize];
        for y in 0..size.height as usize {
            let src = std::slice::from_raw_parts(map.bits.add(y * map.pitch as usize), size.width as usize * 4);
            px[y * size.width as usize * 4..(y + 1) * size.width as usize * 4].copy_from_slice(src);
        }
        let _ = read.Unmap();
        Some(crate::imaging::Raw { w: size.width, h: size.height, px })
    }
}

// ---------------------------------------------------------------------------
// Canvas
// ---------------------------------------------------------------------------

pub struct Canvas {
    pub g: Rc<Gfx>,
    pub scale: f32,
    stack: Vec<Matrix3x2>,
    pub xf: Matrix3x2,
    /// Multiplied into every colour (group opacity for simple content).
    pub alpha: f32,
    alpha_stack: Vec<f32>,
    layers: u32,
}

fn mul(a: &Matrix3x2, b: &Matrix3x2) -> Matrix3x2 {
    // a then b
    Matrix3x2 {
        M11: a.M11 * b.M11 + a.M12 * b.M21,
        M12: a.M11 * b.M12 + a.M12 * b.M22,
        M21: a.M21 * b.M11 + a.M22 * b.M21,
        M22: a.M21 * b.M12 + a.M22 * b.M22,
        M31: a.M31 * b.M11 + a.M32 * b.M21 + b.M31,
        M32: a.M31 * b.M12 + a.M32 * b.M22 + b.M32,
    }
}

pub fn translate(x: f32, y: f32) -> Matrix3x2 {
    Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: x, M32: y }
}

/// Scale about an anchor point.
pub fn scale_about(s: f32, ax: f32, ay: f32) -> Matrix3x2 {
    Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: ax - s * ax, M32: ay - s * ay }
}

pub fn rotate_about(deg: f32, ax: f32, ay: f32) -> Matrix3x2 {
    let (s, c) = deg.to_radians().sin_cos();
    Matrix3x2 { M11: c, M12: s, M21: -s, M22: c, M31: ax - c * ax + s * ay, M32: ay - s * ax - c * ay }
}

impl Canvas {
    pub fn new(g: Rc<Gfx>, scale: f32) -> Canvas {
        Canvas { g, scale, stack: Vec::new(), xf: Matrix3x2::identity(), alpha: 1.0, alpha_stack: Vec::new(), layers: 0 }
    }

    pub fn dc(&self) -> &ID2D1DeviceContext {
        &self.g.dc
    }

    fn c(&self, c: Color) -> Color {
        c.mul_alpha(self.alpha)
    }

    pub fn save(&mut self) {
        self.stack.push(self.xf);
        self.alpha_stack.push(self.alpha);
    }
    pub fn restore(&mut self) {
        if let Some(m) = self.stack.pop() {
            self.xf = m;
            unsafe { self.g.dc.SetTransform(&self.xf) };
        }
        if let Some(a) = self.alpha_stack.pop() {
            self.alpha = a;
        }
    }
    /// Pre-multiply a transform (applied in the current local space).
    pub fn transform(&mut self, m: Matrix3x2) {
        self.xf = mul(&m, &self.xf);
        unsafe { self.g.dc.SetTransform(&self.xf) };
    }
    pub fn translate(&mut self, x: f32, y: f32) {
        self.transform(translate(x, y));
    }
    pub fn scale_about(&mut self, s: f32, ax: f32, ay: f32) {
        if (s - 1.0).abs() > 1e-5 {
            self.transform(scale_about(s, ax, ay));
        }
    }
    pub fn opacity(&mut self, a: f32) {
        self.alpha *= a.clamp(0.0, 1.0);
    }

    pub fn clip(&mut self, r: Rect) {
        unsafe { self.g.dc.PushAxisAlignedClip(&rf(r), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE) };
    }
    pub fn unclip(&mut self) {
        unsafe { self.g.dc.PopAxisAlignedClip() };
    }

    /// Clip subsequent drawing to a geometry (in local space).
    pub fn push_mask(&mut self, geom: &ID2D1Geometry) {
        let params = D2D1_LAYER_PARAMETERS1 {
            contentBounds: D2D_RECT_F { left: -1e7, top: -1e7, right: 1e7, bottom: 1e7 },
            geometricMask: ManuallyDrop::new(Some(geom.clone())),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: Matrix3x2::identity(),
            opacity: 1.0,
            opacityBrush: ManuallyDrop::new(None),
            layerOptions: D2D1_LAYER_OPTIONS1_NONE,
        };
        unsafe { self.g.dc.PushLayer(&params, None) };
        let _ = ManuallyDrop::into_inner(params.geometricMask);
        self.layers += 1;
    }
    /// A real compositing group (opacity applied to the flattened result).
    pub fn push_group(&mut self, opacity: f32) {
        let params = D2D1_LAYER_PARAMETERS1 {
            contentBounds: D2D_RECT_F { left: -1e7, top: -1e7, right: 1e7, bottom: 1e7 },
            geometricMask: ManuallyDrop::new(None),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: Matrix3x2::identity(),
            opacity: opacity.clamp(0.0, 1.0),
            opacityBrush: ManuallyDrop::new(None),
            layerOptions: D2D1_LAYER_OPTIONS1_NONE,
        };
        unsafe { self.g.dc.PushLayer(&params, None) };
        self.layers += 1;
    }
    pub fn pop_layer(&mut self) {
        if self.layers > 0 {
            unsafe { self.g.dc.PopLayer() };
            self.layers -= 1;
        }
    }

    pub fn fill_rect(&mut self, r: Rect, c: Color) {
        let c = self.c(c);
        if c.a <= 0.0 {
            return;
        }
        unsafe { self.g.dc.FillRectangle(&rf(r), self.g.brush(c)) };
    }
    pub fn stroke_rect(&mut self, r: Rect, w: f32, c: Color) {
        let c = self.c(c);
        if c.a <= 0.0 {
            return;
        }
        unsafe { self.g.dc.DrawRectangle(&rf(r), self.g.brush(c), w, None) };
    }

    /// Continuous rounded rect.
    pub fn fill_round(&mut self, r: Rect, rad: f32, c: Color) {
        let c = self.c(c);
        if c.a <= 0.0 || r.w <= 0.0 || r.h <= 0.0 {
            return;
        }
        let geom = self.g.squircle(r.w, r.h, rad);
        self.save();
        self.translate(r.x, r.y);
        unsafe { self.g.dc.FillGeometry(&geom, self.g.brush(c), None) };
        self.restore();
    }
    /// strokeBorder: the stroke sits inside the rect.
    pub fn stroke_round(&mut self, r: Rect, rad: f32, w: f32, c: Color) {
        let c = self.c(c);
        if c.a <= 0.0 || r.w <= 0.0 || r.h <= 0.0 {
            return;
        }
        let ir = r.inset(w / 2.0, w / 2.0);
        let geom = self.g.squircle(ir.w, ir.h, (rad - w / 2.0).max(0.0));
        self.save();
        self.translate(ir.x, ir.y);
        unsafe { self.g.dc.DrawGeometry(&geom, self.g.brush(c), w, None) };
        self.restore();
    }
    /// Stroke centred on the edge (SwiftUI `.stroke`).
    pub fn stroke_round_centered(&mut self, r: Rect, rad: f32, w: f32, c: Color) {
        let c = self.c(c);
        if c.a <= 0.0 {
            return;
        }
        let geom = self.g.squircle(r.w, r.h, rad);
        self.save();
        self.translate(r.x, r.y);
        unsafe { self.g.dc.DrawGeometry(&geom, self.g.brush(c), w, None) };
        self.restore();
    }
    pub fn stroke_round_dashed(&mut self, r: Rect, rad: f32, w: f32, dash: (f32, f32), phase: f32, c: Color) {
        let c = self.c(c);
        if c.a <= 0.0 {
            return;
        }
        let ir = r.inset(w / 2.0, w / 2.0);
        let geom = self.g.squircle(ir.w, ir.h, (rad - w / 2.0).max(0.0));
        let style = self.g.stroke_style(Some(dash), phase, w, 0);
        self.save();
        self.translate(ir.x, ir.y);
        unsafe { self.g.dc.DrawGeometry(&geom, self.g.brush(c), w, &style) };
        self.restore();
    }
    pub fn fill_capsule(&mut self, r: Rect, c: Color) {
        let c = self.c(c);
        if c.a <= 0.0 {
            return;
        }
        let rad = r.h.min(r.w) / 2.0;
        unsafe {
            self.g.dc.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rf(r), radiusX: rad, radiusY: rad }, self.g.brush(c));
        }
    }
    pub fn stroke_capsule(&mut self, r: Rect, w: f32, c: Color) {
        let c = self.c(c);
        if c.a <= 0.0 {
            return;
        }
        let ir = r.inset(w / 2.0, w / 2.0);
        let rad = ir.h.min(ir.w) / 2.0;
        unsafe {
            self.g.dc.DrawRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rf(ir), radiusX: rad, radiusY: rad }, self.g.brush(c), w, None);
        }
    }
    /// Plain circular rounded rect (NSBezierPath roundedRect / CALayer cornerRadius).
    pub fn fill_rrect(&mut self, r: Rect, rad: f32, c: Color) {
        let c = self.c(c);
        if c.a <= 0.0 {
            return;
        }
        unsafe {
            self.g.dc.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rf(r), radiusX: rad, radiusY: rad }, self.g.brush(c));
        }
    }
    pub fn stroke_rrect(&mut self, r: Rect, rad: f32, w: f32, c: Color) {
        let c = self.c(c);
        if c.a <= 0.0 {
            return;
        }
        unsafe {
            self.g.dc.DrawRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rf(r), radiusX: rad, radiusY: rad }, self.g.brush(c), w, None);
        }
    }
    pub fn fill_circle(&mut self, c0: Pt, r: f32, c: Color) {
        let c = self.c(c);
        if c.a <= 0.0 {
            return;
        }
        unsafe {
            self.g.dc.FillEllipse(&D2D1_ELLIPSE { point: p2(c0), radiusX: r, radiusY: r }, self.g.brush(c));
        }
    }
    pub fn stroke_circle(&mut self, c0: Pt, r: f32, w: f32, c: Color) {
        let c = self.c(c);
        if c.a <= 0.0 {
            return;
        }
        unsafe {
            self.g.dc.DrawEllipse(&D2D1_ELLIPSE { point: p2(c0), radiusX: r, radiusY: r }, self.g.brush(c), w, None);
        }
    }
    pub fn fill_ellipse(&mut self, r: Rect, c: Color) {
        let c = self.c(c);
        if c.a <= 0.0 {
            return;
        }
        unsafe {
            self.g.dc.FillEllipse(&D2D1_ELLIPSE { point: p2(r.center()), radiusX: r.w / 2.0, radiusY: r.h / 2.0 }, self.g.brush(c));
        }
    }
    pub fn line(&mut self, a: Pt, b: Pt, w: f32, c: Color, cap: u8) {
        let c = self.c(c);
        if c.a <= 0.0 {
            return;
        }
        let style = self.g.stroke_style(None, 0.0, w, cap);
        unsafe { self.g.dc.DrawLine(p2(a), p2(b), self.g.brush(c), w, &style) };
    }
    pub fn fill_geom(&mut self, geom: &ID2D1Geometry, c: Color) {
        let c = self.c(c);
        if c.a <= 0.0 {
            return;
        }
        unsafe { self.g.dc.FillGeometry(geom, self.g.brush(c), None) };
    }
    pub fn stroke_geom(&mut self, geom: &ID2D1Geometry, w: f32, c: Color, cap: u8) {
        let c = self.c(c);
        if c.a <= 0.0 {
            return;
        }
        let style = self.g.stroke_style(None, 0.0, w, cap);
        unsafe { self.g.dc.DrawGeometry(geom, self.g.brush(c), w, &style) };
    }
    pub fn stroke_geom_dashed(&mut self, geom: &ID2D1Geometry, w: f32, dash: (f32, f32), phase: f32, c: Color) {
        let c = self.c(c);
        if c.a <= 0.0 {
            return;
        }
        let style = self.g.stroke_style(Some(dash), phase, w, 0);
        unsafe { self.g.dc.DrawGeometry(geom, self.g.brush(c), w, &style) };
    }

    /// Vertical linear gradient filling `r` (stops top→bottom).
    pub fn fill_gradient_v(&mut self, r: Rect, stops: &[(f32, Color)], clip: Option<&ID2D1Geometry>) {
        let a = self.alpha;
        let stops: Vec<(f32, Color)> = stops.iter().map(|(p, c)| (*p, c.mul_alpha(a))).collect();
        let Some(coll) = self.g.gradient(&stops) else { return };
        unsafe {
            let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                startPoint: Vector2 { X: r.x, Y: r.y },
                endPoint: Vector2 { X: r.x, Y: r.b() },
            };
            if let Ok(brush) = self.g.dc.CreateLinearGradientBrush(&props, None, &coll) {
                match clip {
                    Some(gm) => self.g.dc.FillGeometry(gm, &brush, None),
                    None => self.g.dc.FillRectangle(&rf(r), &brush),
                }
            }
        }
    }

    // -- text ---------------------------------------------------------------

    /// Draws a single line of text inside `r`, vertically centred.
    pub fn text(&mut self, s: &str, font: &Font, c: Color, r: Rect, align: Align, trim: Trim) {
        let c = self.c(c);
        if c.a <= 0.0 || s.is_empty() {
            return;
        }
        let s = if trim == Trim::Middle { self.middle_truncate(s, font, r.w) } else { s.to_string() };
        let l = self.g.layout(&s, font, r.w.max(1.0), 100000.0, false, align, trim);
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe {
            let _ = l.GetMetrics(&mut m);
            let y = r.y + (r.h - m.height) / 2.0;
            self.g.dc.DrawTextLayout(
                Vector2 { X: r.x, Y: y },
                &l,
                self.g.brush(c),
                D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
            );
        }
    }

    /// Text at a top-left point (no box).
    pub fn text_at(&mut self, s: &str, font: &Font, c: Color, p: Pt) {
        let c = self.c(c);
        if c.a <= 0.0 || s.is_empty() {
            return;
        }
        let l = self.g.layout(s, font, 100000.0, 100000.0, false, Align::Leading, Trim::None);
        unsafe {
            self.g.dc.DrawTextLayout(p2(p), &l, self.g.brush(c), D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT);
        }
    }

    /// A prepared layout with its top-left at `p`.
    pub fn text_layout(&mut self, l: &IDWriteTextLayout, p: Pt, c: Color) {
        let c = self.c(c);
        if c.a <= 0.0 {
            return;
        }
        unsafe {
            self.g.dc.DrawTextLayout(p2(p), l, self.g.brush(c), D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT);
        }
    }

    /// Multi-line wrapped text from the top of `r`, at most `lines` lines.
    pub fn text_wrapped(&mut self, s: &str, font: &Font, c: Color, r: Rect, lines: u32, align: Align) {
        let c = self.c(c);
        if c.a <= 0.0 || s.is_empty() {
            return;
        }
        let lh = self.g.line_height(font);
        let max_h = lh * lines as f32 + 0.5;
        let l = self.g.layout(s, font, r.w.max(1.0), max_h, true, align, Trim::Tail);
        unsafe {
            self.clip(Rect { x: r.x - 2.0, y: r.y - 2.0, w: r.w + 4.0, h: max_h.min(r.h) + 2.0 });
            self.g.dc.DrawTextLayout(p2(Pt { x: r.x, y: r.y }), &l, self.g.brush(c), D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT);
            self.unclip();
        }
    }

    pub fn measure(&self, s: &str, font: &Font) -> (f32, f32) {
        self.g.measure(s, font)
    }

    /// NSLineBreakMode.byTruncatingMiddle.
    pub fn middle_truncate(&self, s: &str, font: &Font, max_w: f32) -> String {
        if self.g.measure(s, font).0 <= max_w {
            return s.to_string();
        }
        let chars: Vec<char> = s.chars().collect();
        let (mut lo, mut hi) = (0usize, chars.len());
        let mut best = String::from("…");
        while lo <= hi {
            let keep = (lo + hi) / 2;
            let head = keep.div_ceil(2);
            let tail = keep / 2;
            let cand: String = chars[..head].iter().collect::<String>() + "…" + &chars[chars.len() - tail..].iter().collect::<String>();
            if self.g.measure(&cand, font).0 <= max_w {
                best = cand;
                lo = keep + 1;
            } else {
                if keep == 0 {
                    break;
                }
                hi = keep - 1;
            }
        }
        best
    }

    // -- images -------------------------------------------------------------

    pub fn image(&mut self, bmp: &ID2D1Bitmap1, dest: Rect, opacity: f32, smooth: bool) {
        let a = (opacity * self.alpha).clamp(0.0, 1.0);
        if a <= 0.0 {
            return;
        }
        unsafe {
            self.g.dc.DrawBitmap(
                bmp,
                Some(&rf(dest)),
                a,
                if smooth { D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC } else { D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR },
                None,
                None,
            );
        }
    }

    pub fn image_src(&mut self, bmp: &ID2D1Bitmap1, dest: Rect, src: Rect, opacity: f32, smooth: bool) {
        let a = (opacity * self.alpha).clamp(0.0, 1.0);
        if a <= 0.0 {
            return;
        }
        unsafe {
            self.g.dc.DrawBitmap(
                bmp,
                Some(&rf(dest)),
                a,
                if smooth { D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC } else { D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR },
                Some(&rf(src)),
                None,
            );
        }
    }

    /// Aspect-fill an image into `r` (clipped).
    pub fn image_fill(&mut self, bmp: &ID2D1Bitmap1, r: Rect, opacity: f32) {
        let s = unsafe { bmp.GetPixelSize() };
        let (iw, ih) = (s.width as f32, s.height as f32);
        if iw <= 0.0 || ih <= 0.0 {
            return;
        }
        // Source crop so we never draw outside r (no layer needed).
        let k = (r.w / iw).max(r.h / ih);
        let sw = r.w / k;
        let sh = r.h / k;
        let src = Rect { x: (iw - sw) / 2.0, y: (ih - sh) / 2.0, w: sw, h: sh };
        // DrawBitmap's source rect is in DIPs of the bitmap (96 dpi = px).
        self.image_src(bmp, r, src, opacity, true);
    }

    pub fn image_fit(&mut self, bmp: &ID2D1Bitmap1, r: Rect, opacity: f32, smooth: bool) -> Rect {
        let s = unsafe { bmp.GetPixelSize() };
        let d = r.fit(s.width as f32, s.height as f32);
        self.image(bmp, d, opacity, smooth);
        d
    }

    // -- effects ------------------------------------------------------------

    /// Record drawing into a command list (for blur / shadow).
    pub fn record(&mut self, f: impl FnOnce(&mut Canvas)) -> Option<ID2D1CommandList> {
        unsafe {
            let prev = self.g.dc.GetTarget().ok();
            let cl = self.g.dc.CreateCommandList().ok()?;
            self.g.dc.SetTarget(&cl);
            f(self);
            let _ = cl.Close();
            if let Some(p) = prev {
                self.g.dc.SetTarget(&p);
            }
            Some(cl)
        }
    }

    fn draw_image(&mut self, img: &ID2D1Image, offset: Pt) {
        unsafe {
            self.g.dc.DrawImage(
                img,
                Some(&p2(offset)),
                None,
                D2D1_INTERPOLATION_MODE_LINEAR,
                D2D1_COMPOSITE_MODE_SOURCE_OVER,
            );
        }
    }

    /// Draws `content` blurred by `sigma` DIPs (SwiftUI `.blur(radius:)`).
    /// The command list is recorded with an identity transform, so `content`
    /// draws in the current local space.
    pub fn blurred(&mut self, sigma: f32, opacity: f32, content: impl FnOnce(&mut Canvas)) {
        let xf = self.xf;
        let alpha = self.alpha;
        self.alpha = alpha * opacity;
        if sigma < 0.25 {
            content(self);
            self.alpha = alpha;
            return;
        }
        unsafe { self.g.dc.SetTransform(&Matrix3x2::identity()) };
        let saved = self.xf;
        self.xf = Matrix3x2::identity();
        let cl = self.record(|c| {
            unsafe { c.g.dc.SetTransform(&xf) };
            c.xf = xf;
            content(c);
            c.xf = Matrix3x2::identity();
            unsafe { c.g.dc.SetTransform(&Matrix3x2::identity()) };
        });
        self.xf = saved;
        self.alpha = alpha;
        if let Some(cl) = cl {
            unsafe {
                if let Ok(fx) = self.g.dc.CreateEffect(&CLSID_D2D1GaussianBlur) {
                    let cli: ID2D1Image = cl.cast().unwrap();
                    fx.SetInput(0, &cli, true);
                    let s = sigma.to_le_bytes();
                    let _ = fx.SetValue(D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32, D2D1_PROPERTY_TYPE_FLOAT, &s);
                    let _ = fx.SetValue(
                        D2D1_GAUSSIANBLUR_PROP_BORDER_MODE.0 as u32,
                        D2D1_PROPERTY_TYPE_ENUM,
                        &(D2D1_BORDER_MODE_SOFT.0 as u32).to_le_bytes(),
                    );
                    if let Ok(out) = fx.GetOutput() {
                        self.draw_image(&out, Pt::default());
                    }
                }
            }
        }
        unsafe { self.g.dc.SetTransform(&self.xf) };
    }

    /// Draws a drop shadow of `content` (offset in DIPs, blur sigma, colour),
    /// then the content itself when `with_content`.
    pub fn shadowed(&mut self, sigma: f32, offset: Pt, color: Color, with_content: bool, content: impl FnOnce(&mut Canvas)) {
        let xf = self.xf;
        unsafe { self.g.dc.SetTransform(&Matrix3x2::identity()) };
        let saved = self.xf;
        self.xf = Matrix3x2::identity();
        let cl = self.record(|c| {
            unsafe { c.g.dc.SetTransform(&xf) };
            c.xf = xf;
            content(c);
            c.xf = Matrix3x2::identity();
            unsafe { c.g.dc.SetTransform(&Matrix3x2::identity()) };
        });
        self.xf = saved;
        let Some(cl) = cl else {
            unsafe { self.g.dc.SetTransform(&self.xf) };
            return;
        };
        let color = self.c(color);
        unsafe {
            let cli: ID2D1Image = cl.cast().unwrap();
            if color.a > 0.0 {
                if let Ok(fx) = self.g.dc.CreateEffect(&CLSID_D2D1Shadow) {
                    fx.SetInput(0, &cli, true);
                    let _ = fx.SetValue(D2D1_SHADOW_PROP_BLUR_STANDARD_DEVIATION.0 as u32, D2D1_PROPERTY_TYPE_FLOAT, &sigma.max(0.0).to_le_bytes());
                    let v: [f32; 4] = [color.r, color.g, color.b, color.a];
                    let bytes: Vec<u8> = v.iter().flat_map(|f| f.to_le_bytes()).collect();
                    let _ = fx.SetValue(D2D1_SHADOW_PROP_COLOR.0 as u32, D2D1_PROPERTY_TYPE_VECTOR4, &bytes);
                    if let Ok(out) = fx.GetOutput() {
                        // Offset is in the current local space; convert via the matrix.
                        let ox = offset.x * xf.M11 + offset.y * xf.M21;
                        let oy = offset.x * xf.M12 + offset.y * xf.M22;
                        self.draw_image(&out, Pt { x: ox, y: oy });
                    }
                }
            }
            if with_content {
                self.draw_image(&cli, Pt::default());
            }
            self.g.dc.SetTransform(&self.xf);
        }
    }
}
