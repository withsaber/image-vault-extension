//! Vector icons drawn to match the SF Symbols used by the Mac app. Each glyph
//! is built once per (icon, size, weight) as a single filled geometry —
//! strokes are widened with round caps/joins, `.fill` variants knock their
//! inner marks out — and then drawn with one FillGeometry call.

use std::cell::RefCell;
use std::collections::HashMap;

use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::core::{Interface, PCWSTR};
use windows_numerics::Matrix3x2;

use crate::gfx::{Canvas, Gfx, PathB};
use crate::theme::{Color, Pt, Rect, Weight, pt};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    MagnifyingGlass,
    Folder,
    GearShape,
    PhotoStack,
    DocOnClipboard,
    ArrowCounterclockwise,
    Checkmark,
    CheckmarkCircle,
    CheckmarkCircleFill,
    XMark,
    XMarkCircleFill,
    DocOnDoc,
    DocTextMagnifyingGlass,
    TextViewfinder,
    Trash,
    ArrowDownCircleFill,
    TrayAndArrowDownFill,
    ArrowTriangle2Circlepath,
    MusicNote,
    Photo,
    Doc,
    TextAlignLeft,
    ArrowDownToLine,
    ArrowUpToLine,
    TextFormat,
    ExternalDrive,
    ChevronLeft,
    ChevronRight,
    Pencil,
    SquareAndArrowDown,
    SquareAndArrowDownOnSquare,
    Pin,
    CursorArrow,
    ArrowUpRight,
    Ruler,
    Rectangle,
    RectangleFill,
    Drop,
    Highlighter,
    RectangleDashed,
    OneCircle,
    Circle,
    LineDiagonal,
    PlusViewfinder,
    Ellipsis,
    PhotoOnRectangle,
    OneMagnifyingGlass,
    ArrowsOutward,
    PlusMagnifyingGlass,
    MinusMagnifyingGlass,
    PlayFill,
    PauseFill,
    ForwardFill,
    BackwardFill,
    /// arrow.down.right.and.arrow.up.left
    ArrowsInward,
    /// chevron.left.forwardslash.chevron.right
    Code,
    /// folder.badge.plus
    FolderBadgePlus,
}

/// SF Symbols stroke weight as a fraction of the point size.
fn stroke_k(w: Weight) -> f32 {
    match w {
        Weight::UltraLight => 0.045,
        Weight::Light => 0.068,
        Weight::Regular => 0.088,
        Weight::Medium => 0.10,
        Weight::Semibold => 0.115,
        Weight::Bold => 0.13,
        Weight::Heavy => 0.155,
    }
}

thread_local! {
    static CACHE: RefCell<HashMap<(Icon, i32, Weight, u32), Option<ID2D1Geometry>>> = RefCell::new(HashMap::new());
}

const TOL: f32 = 0.02;

struct Glyph<'a> {
    g: &'a Gfx,
    s: f32,
    sw: f32,
    acc: Option<ID2D1Geometry>,
}

impl<'a> Glyph<'a> {
    fn p(&self, x: f32, y: f32) -> Pt {
        pt(x * self.s, y * self.s)
    }
    fn path(&self) -> PathB {
        self.g.path()
    }
    fn union(&mut self, geom: ID2D1Geometry) {
        self.acc = Some(match self.acc.take() {
            None => geom,
            Some(a) => combine(self.g, &a, &geom, D2D1_COMBINE_MODE_UNION),
        });
    }
    fn exclude(&mut self, geom: ID2D1Geometry) {
        if let Some(a) = self.acc.take() {
            self.acc = Some(combine(self.g, &a, &geom, D2D1_COMBINE_MODE_EXCLUDE));
        }
    }
    fn widen(&self, geom: &ID2D1Geometry, w: f32, dash: Option<(f32, f32)>) -> ID2D1Geometry {
        unsafe {
            let props = D2D1_STROKE_STYLE_PROPERTIES1 {
                transformType: D2D1_STROKE_TRANSFORM_TYPE_NORMAL,
                startCap: D2D1_CAP_STYLE_ROUND,
                endCap: D2D1_CAP_STYLE_ROUND,
                dashCap: D2D1_CAP_STYLE_ROUND,
                lineJoin: D2D1_LINE_JOIN_ROUND,
                miterLimit: 10.0,
                dashStyle: if dash.is_some() { D2D1_DASH_STYLE_CUSTOM } else { D2D1_DASH_STYLE_SOLID },
                dashOffset: 0.0,
            };
            let style = match dash {
                Some((a, b)) => self.g.d2f.CreateStrokeStyle(&props, Some(&[a / w, b / w])),
                None => self.g.d2f.CreateStrokeStyle(&props, None),
            }
            .unwrap();
            let out = self.g.d2f.CreatePathGeometry().unwrap();
            let sink = out.Open().unwrap();
            let _ = geom.Widen(w, &style, None, TOL, &sink);
            let _ = sink.Close();
            // Widen output can overlap itself; normalise through a union with nothing.
            out.cast().unwrap()
        }
    }
    /// Stroke a path built in unit space with the glyph's weight.
    fn stroke(&mut self, b: PathB) {
        let geom: ID2D1Geometry = b.finish().cast().unwrap();
        let w = self.widen(&geom, self.sw, None);
        let w = simplify(self.g, &w);
        self.union(w);
    }
    fn stroke_w(&mut self, b: PathB, k: f32) {
        let geom: ID2D1Geometry = b.finish().cast().unwrap();
        let w = self.widen(&geom, self.sw * k, None);
        let w = simplify(self.g, &w);
        self.union(w);
    }
    fn stroke_dashed(&mut self, b: PathB, dash: (f32, f32)) {
        let geom: ID2D1Geometry = b.finish().cast().unwrap();
        let w = self.widen(&geom, self.sw, Some((dash.0 * self.s, dash.1 * self.s)));
        let w = simplify(self.g, &w);
        self.union(w);
    }
    fn fill(&mut self, b: PathB) {
        let geom: ID2D1Geometry = b.finish().cast().unwrap();
        self.union(simplify(self.g, &geom));
    }
    fn knock_stroke(&mut self, b: PathB, k: f32) {
        let geom: ID2D1Geometry = b.finish().cast().unwrap();
        let w = self.widen(&geom, self.sw * k, None);
        let w = simplify(self.g, &w);
        self.exclude(w);
    }
    fn knock_fill(&mut self, b: PathB) {
        let geom: ID2D1Geometry = b.finish().cast().unwrap();
        self.exclude(geom);
    }
    // -- shape helpers in unit space -----------------------------------------
    fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) {
        let mut b = self.path().hollow();
        b.move_to(self.p(x0, y0));
        b.line_to(self.p(x1, y1));
        self.stroke(b);
    }
    fn poly(&mut self, pts: &[(f32, f32)], closed: bool) {
        let mut b = if closed { self.path() } else { self.path().hollow() };
        b.move_to(self.p(pts[0].0, pts[0].1));
        for &(x, y) in &pts[1..] {
            b.line_to(self.p(x, y));
        }
        if closed {
            b.close();
        }
        self.stroke(b);
    }
    fn rrect_path(&self, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> PathB {
        let mut b = self.path();
        b.round_rect(crate::theme::Rect::from_ltrb(x0 * self.s, y0 * self.s, x1 * self.s, y1 * self.s), r * self.s);
        b
    }
    fn rrect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) {
        let b = self.rrect_path(x0, y0, x1, y1, r);
        self.stroke(b);
    }
    fn circle_path(&self, cx: f32, cy: f32, r: f32) -> PathB {
        let mut b = self.path();
        b.circle(self.p(cx, cy), r * self.s);
        b
    }
    fn circle(&mut self, cx: f32, cy: f32, r: f32) {
        let b = self.circle_path(cx, cy, r);
        self.stroke(b);
    }
    fn dot(&mut self, cx: f32, cy: f32, r: f32) {
        let b = self.circle_path(cx, cy, r);
        self.fill(b);
    }
    fn arc(&mut self, cx: f32, cy: f32, r: f32, a0: f32, a1: f32) -> (Pt, Pt) {
        // Clockwise (y-down) from a0 to a1 in degrees.
        let mut b = self.path().hollow();
        let pa = |a: f32| pt(cx + r * a.to_radians().cos(), cy + r * a.to_radians().sin());
        let start = pa(a0);
        let end = pa(a1);
        b.move_to(self.p(start.x, start.y));
        let sweep = (a1 - a0).rem_euclid(360.0);
        if sweep > 180.0 {
            let mid = pa(a0 + sweep / 2.0);
            b.arc_to(self.p(mid.x, mid.y), r * self.s, r * self.s, true, false);
        }
        b.arc_to(self.p(end.x, end.y), r * self.s, r * self.s, true, false);
        self.stroke(b);
        (start, end)
    }
    /// Arrow head: two strokes meeting at `tip`, pointing along angle `deg`.
    fn head(&mut self, tip: Pt, deg: f32, len: f32, spread: f32) {
        let a = deg.to_radians();
        let l = (a + std::f32::consts::PI - spread.to_radians()).sin_cos();
        let r = (a + std::f32::consts::PI + spread.to_radians()).sin_cos();
        self.poly(&[(tip.x + len * l.1, tip.y + len * l.0), (tip.x, tip.y), (tip.x + len * r.1, tip.y + len * r.0)], false);
    }
    fn magnifier(&mut self, cx: f32, cy: f32, r: f32) {
        self.circle(cx, cy, r);
        let d = r * 0.7071 + 0.02;
        self.line(cx + d, cy + d, cx + r * 1.62, cy + r * 1.62);
    }
    fn mountains(&mut self, x0: f32, y0: f32, x1: f32, y1: f32) {
        // Photo glyph contents: two peaks and a sun.
        let w = x1 - x0;
        let h = y1 - y0;
        self.poly(
            &[
                (x0 + w * 0.06, y1 - h * 0.1),
                (x0 + w * 0.36, y0 + h * 0.48),
                (x0 + w * 0.58, y0 + h * 0.72),
                (x0 + w * 0.70, y0 + h * 0.60),
                (x0 + w * 0.94, y1 - h * 0.1),
            ],
            false,
        );
        self.dot(x0 + w * 0.70, y0 + h * 0.28, w.min(h) * 0.1);
    }
    fn viewfinder(&mut self, e: f32, arm: f32) {
        for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let mut b = self.path().hollow();
            b.move_to(self.p(sx * e, sy * (e - arm)));
            b.line_to(self.p(sx * e, sy * (e - 0.08)));
            b.quad_to(self.p(sx * e, sy * e), self.p(sx * (e - 0.08), sy * e));
            b.line_to(self.p(sx * (e - arm), sy * e));
            self.stroke(b);
        }
    }
    fn text_glyph(&mut self, text: &str, size: f32, cx: f32, cy: f32, weight: i32) {
        if let Some(gm) = text_outline(self.g, text, size * self.s, weight, self.p(cx, cy)) {
            self.union(gm);
        }
    }
}

fn combine(g: &Gfx, a: &ID2D1Geometry, b: &ID2D1Geometry, mode: D2D1_COMBINE_MODE) -> ID2D1Geometry {
    unsafe {
        let out = g.d2f.CreatePathGeometry().unwrap();
        let sink = out.Open().unwrap();
        let _ = a.CombineWithGeometry(b, mode, None, TOL, &sink);
        let _ = sink.Close();
        out.cast().unwrap()
    }
}

/// Resolve self-overlaps into a clean winding outline.
fn simplify(g: &Gfx, a: &ID2D1Geometry) -> ID2D1Geometry {
    unsafe {
        let out = g.d2f.CreatePathGeometry().unwrap();
        let sink = out.Open().unwrap();
        sink.SetFillMode(D2D1_FILL_MODE_WINDING);
        let _ = a.Outline(None, TOL, &sink);
        let _ = sink.Close();
        out.cast().unwrap()
    }
}

/// Glyph outline of `text` (system UI font) centred on `center`.
fn text_outline(g: &Gfx, text: &str, em: f32, weight: i32, center: Pt) -> Option<ID2D1Geometry> {
    unsafe {
        let mut coll: Option<IDWriteFontCollection> = None;
        g.dw.GetSystemFontCollection(&mut coll, false).ok()?;
        let coll = coll?;
        let mut idx = 0u32;
        let mut exists = windows::core::BOOL(0);
        let name = crate::util::wide("Segoe UI");
        coll.FindFamilyName(PCWSTR(name.as_ptr()), &mut idx, &mut exists).ok()?;
        if !exists.as_bool() {
            return None;
        }
        let fam = coll.GetFontFamily(idx).ok()?;
        let font = fam
            .GetFirstMatchingFont(DWRITE_FONT_WEIGHT(weight), DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL)
            .ok()?;
        let face = font.CreateFontFace().ok()?;
        let cps: Vec<u32> = text.chars().map(|c| c as u32).collect();
        let mut glyphs = vec![0u16; cps.len()];
        face.GetGlyphIndices(cps.as_ptr(), cps.len() as u32, glyphs.as_mut_ptr()).ok()?;
        let mut metrics = vec![DWRITE_GLYPH_METRICS::default(); glyphs.len()];
        face.GetDesignGlyphMetrics(glyphs.as_ptr(), glyphs.len() as u32, metrics.as_mut_ptr(), false).ok()?;
        let mut fm = DWRITE_FONT_METRICS::default();
        face.GetMetrics(&mut fm);
        let k = em / fm.designUnitsPerEm as f32;
        let advances: Vec<f32> = metrics.iter().map(|m| m.advanceWidth as f32 * k).collect();
        let total: f32 = advances.iter().sum();
        let path = g.d2f.CreatePathGeometry().ok()?;
        let sink = path.Open().ok()?;
        face.GetGlyphRunOutline(em, glyphs.as_ptr(), Some(advances.as_ptr()), None, glyphs.len() as u32, false, false, &sink)
            .ok()?;
        sink.Close().ok()?;
        // Baseline origin → centre the cap height on `center`.
        let cap = fm.capHeight as f32 * k;
        let m = Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: center.x - total / 2.0, M32: center.y + cap / 2.0 };
        let t = g.d2f.CreateTransformedGeometry(&path, &m).ok()?;
        Some(simplify(g, &t.cast().ok()?))
    }
}

fn build(g: &Gfx, icon: Icon, size: f32, weight: Weight) -> Option<ID2D1Geometry> {
    let mut gl = Glyph { g, s: size, sw: size * stroke_k(weight), acc: None };
    use Icon::*;
    match icon {
        MagnifyingGlass => gl.magnifier(-0.09, -0.09, 0.34),
        Folder => {
            let mut b = gl.path();
            b.move_to(gl.p(-0.56, 0.32));
            b.line_to(gl.p(-0.56, -0.33));
            b.quad_to(gl.p(-0.56, -0.42), gl.p(-0.47, -0.42));
            b.line_to(gl.p(-0.2, -0.42));
            b.quad_to(gl.p(-0.14, -0.42), gl.p(-0.1, -0.37));
            b.line_to(gl.p(-0.04, -0.3));
            b.line_to(gl.p(0.47, -0.3));
            b.quad_to(gl.p(0.56, -0.3), gl.p(0.56, -0.21));
            b.line_to(gl.p(0.56, 0.32));
            b.quad_to(gl.p(0.56, 0.42), gl.p(0.46, 0.42));
            b.line_to(gl.p(-0.46, 0.42));
            b.quad_to(gl.p(-0.56, 0.42), gl.p(-0.56, 0.32));
            b.close();
            gl.stroke(b);
            gl.line(-0.56, -0.16, 0.56, -0.16);
        }
        GearShape => {
            let teeth = 8;
            let (ro, ri) = (0.5, 0.39);
            let mut b = gl.path();
            for i in 0..teeth {
                let base = i as f32 * 360.0 / teeth as f32 - 90.0;
                let half_out = 11.0;
                let half_in = 17.0;
                let pts = [(base - half_in, ri), (base - half_out, ro), (base + half_out, ro), (base + half_in, ri)];
                for (j, (a, r)) in pts.iter().enumerate() {
                    let p = gl.p(r * a.to_radians().cos(), r * a.to_radians().sin());
                    if i == 0 && j == 0 {
                        b.move_to(p);
                    } else {
                        b.line_to(p);
                    }
                }
            }
            b.close();
            gl.stroke_w(b, 0.95);
            gl.circle(0.0, 0.0, 0.16);
        }
        PhotoStack => {
            // Front photo + two stacked edges behind it.
            gl.rrect(-0.52, -0.2, 0.36, 0.48, 0.1);
            gl.mountains(-0.52, -0.2, 0.36, 0.48);
            let mut b = gl.path().hollow();
            b.move_to(gl.p(-0.4, -0.34));
            b.line_to(gl.p(0.4, -0.34));
            b.quad_to(gl.p(0.5, -0.34), gl.p(0.5, -0.24));
            b.line_to(gl.p(0.5, 0.34));
            gl.stroke(b);
            let mut b = gl.path().hollow();
            b.move_to(gl.p(-0.26, -0.48));
            b.line_to(gl.p(0.54, -0.48));
            b.quad_to(gl.p(0.64, -0.48), gl.p(0.64, -0.38));
            b.line_to(gl.p(0.64, 0.2));
            gl.stroke(b);
        }
        DocOnClipboard => {
            gl.rrect(-0.42, -0.4, 0.42, 0.52, 0.1);
            let clip = gl.rrect_path(-0.2, -0.53, 0.2, -0.33, 0.07);
            let c2: ID2D1Geometry = clip.finish().cast().unwrap();
            let pad = gl.widen(&c2, gl.sw * 2.6, None);
            gl.exclude(pad);
            let clip = gl.rrect_path(-0.2, -0.53, 0.2, -0.33, 0.07);
            gl.stroke(clip);
            gl.line(-0.2, -0.05, 0.2, -0.05);
            gl.line(-0.2, 0.12, 0.2, 0.12);
            gl.line(-0.2, 0.29, 0.06, 0.29);
        }
        ArrowCounterclockwise => {
            let (s, _e) = gl.arc(0.0, 0.04, 0.42, -100.0, 210.0);
            gl.head(s, 180.0 + 10.0, 0.2, 42.0);
        }
        Checkmark => gl.poly(&[(-0.38, 0.02), (-0.12, 0.3), (0.4, -0.36)], false),
        CheckmarkCircle => {
            gl.circle(0.0, 0.0, 0.48);
            gl.poly(&[(-0.2, 0.01), (-0.06, 0.17), (0.22, -0.18)], false);
        }
        CheckmarkCircleFill => {
            let c = gl.circle_path(0.0, 0.0, 0.52);
            gl.fill(c);
            let mut b = gl.path().hollow();
            b.move_to(gl.p(-0.2, 0.01));
            b.line_to(gl.p(-0.06, 0.17));
            b.line_to(gl.p(0.22, -0.18));
            gl.knock_stroke(b, 1.15);
        }
        XMark => {
            gl.line(-0.34, -0.34, 0.34, 0.34);
            gl.line(0.34, -0.34, -0.34, 0.34);
        }
        XMarkCircleFill => {
            let c = gl.circle_path(0.0, 0.0, 0.52);
            gl.fill(c);
            let mut b = gl.path().hollow();
            b.move_to(gl.p(-0.17, -0.17));
            b.line_to(gl.p(0.17, 0.17));
            b.move_to(gl.p(0.17, -0.17));
            b.line_to(gl.p(-0.17, 0.17));
            gl.knock_stroke(b, 1.1);
        }
        DocOnDoc => {
            // Back sheet, hidden behind the front one.
            gl.rrect(-0.24, -0.52, 0.46, 0.26, 0.1);
            let front = gl.rrect_path(-0.46, -0.3, 0.24, 0.5, 0.1);
            let fg: ID2D1Geometry = front.finish().cast().unwrap();
            let pad = gl.widen(&fg, gl.sw * 2.4, None);
            gl.exclude(pad);
            gl.exclude(fg);
            gl.rrect(-0.46, -0.3, 0.24, 0.5, 0.1);
        }
        DocTextMagnifyingGlass => {
            let mut b = gl.path();
            b.move_to(gl.p(-0.4, -0.42));
            b.quad_to(gl.p(-0.4, -0.5), gl.p(-0.32, -0.5));
            b.line_to(gl.p(0.1, -0.5));
            b.line_to(gl.p(0.36, -0.24));
            b.line_to(gl.p(0.36, 0.42));
            b.quad_to(gl.p(0.36, 0.5), gl.p(0.28, 0.5));
            b.line_to(gl.p(-0.32, 0.5));
            b.quad_to(gl.p(-0.4, 0.5), gl.p(-0.4, 0.42));
            b.close();
            gl.stroke(b);
            gl.line(-0.22, -0.2, 0.02, -0.2);
            gl.line(-0.22, -0.04, 0.16, -0.04);
            let lens = gl.circle_path(0.16, 0.24, 0.24);
            let lg: ID2D1Geometry = lens.finish().cast().unwrap();
            let pad = gl.widen(&lg, gl.sw * 2.6, None);
            gl.exclude(pad);
            gl.exclude(lg);
            gl.circle(0.16, 0.24, 0.17);
            gl.line(0.29, 0.37, 0.48, 0.56);
        }
        TextViewfinder => {
            gl.viewfinder(0.5, 0.2);
            gl.line(-0.24, -0.16, 0.24, -0.16);
            gl.line(-0.24, 0.0, 0.24, 0.0);
            gl.line(-0.24, 0.16, 0.08, 0.16);
        }
        Trash => {
            gl.line(-0.48, -0.32, 0.48, -0.32);
            let mut b = gl.path().hollow();
            b.move_to(gl.p(-0.18, -0.32));
            b.line_to(gl.p(-0.18, -0.42));
            b.quad_to(gl.p(-0.18, -0.5), gl.p(-0.1, -0.5));
            b.line_to(gl.p(0.1, -0.5));
            b.quad_to(gl.p(0.18, -0.5), gl.p(0.18, -0.42));
            b.line_to(gl.p(0.18, -0.32));
            gl.stroke(b);
            let mut b = gl.path().hollow();
            b.move_to(gl.p(-0.37, -0.32));
            b.line_to(gl.p(-0.31, 0.4));
            b.quad_to(gl.p(-0.3, 0.5), gl.p(-0.2, 0.5));
            b.line_to(gl.p(0.2, 0.5));
            b.quad_to(gl.p(0.3, 0.5), gl.p(0.31, 0.4));
            b.line_to(gl.p(0.37, -0.32));
            gl.stroke(b);
            gl.line(-0.12, -0.12, -0.1, 0.3);
            gl.line(0.12, -0.12, 0.1, 0.3);
        }
        ArrowDownCircleFill => {
            let c = gl.circle_path(0.0, 0.0, 0.52);
            gl.fill(c);
            let mut b = gl.path().hollow();
            b.move_to(gl.p(0.0, -0.26));
            b.line_to(gl.p(0.0, 0.24));
            b.move_to(gl.p(-0.19, 0.05));
            b.line_to(gl.p(0.0, 0.24));
            b.line_to(gl.p(0.19, 0.05));
            gl.knock_stroke(b, 1.15);
        }
        TrayAndArrowDownFill => {
            let mut b = gl.path();
            b.move_to(gl.p(-0.56, 0.08));
            b.line_to(gl.p(-0.24, 0.08));
            b.quad_to(gl.p(-0.2, 0.08), gl.p(-0.18, 0.12));
            b.quad_to(gl.p(-0.1, 0.26), gl.p(0.0, 0.26));
            b.quad_to(gl.p(0.1, 0.26), gl.p(0.18, 0.12));
            b.quad_to(gl.p(0.2, 0.08), gl.p(0.24, 0.08));
            b.line_to(gl.p(0.56, 0.08));
            b.line_to(gl.p(0.56, 0.38));
            b.quad_to(gl.p(0.56, 0.5), gl.p(0.44, 0.5));
            b.line_to(gl.p(-0.44, 0.5));
            b.quad_to(gl.p(-0.56, 0.5), gl.p(-0.56, 0.38));
            b.close();
            gl.fill(b);
            let mut b = gl.path().hollow();
            b.move_to(gl.p(-0.54, 0.08));
            b.line_to(gl.p(-0.4, -0.3));
            b.move_to(gl.p(0.54, 0.08));
            b.line_to(gl.p(0.4, -0.3));
            gl.stroke(b);
            let mut b = gl.path().hollow();
            b.move_to(gl.p(0.0, -0.56));
            b.line_to(gl.p(0.0, 0.08));
            b.move_to(gl.p(-0.17, -0.1));
            b.line_to(gl.p(0.0, 0.08));
            b.line_to(gl.p(0.17, -0.1));
            gl.stroke_w(b, 1.35);
        }
        ArrowTriangle2Circlepath => {
            let (_s1, e1) = gl.arc(0.0, 0.0, 0.42, 200.0, 335.0);
            gl.head(e1, 335.0 + 90.0, 0.17, 45.0);
            let (_s2, e2) = gl.arc(0.0, 0.0, 0.42, 20.0, 155.0);
            gl.head(e2, 155.0 + 90.0, 0.17, 45.0);
        }
        PlayFill => {
            let mut b = gl.path();
            b.move_to(gl.p(-0.3, -0.42));
            b.line_to(gl.p(0.42, 0.0));
            b.line_to(gl.p(-0.3, 0.42));
            b.close();
            gl.fill(b);
        }
        PauseFill => {
            let a = gl.rrect_path(-0.34, -0.42, -0.1, 0.42, 0.06);
            gl.fill(a);
            let b = gl.rrect_path(0.1, -0.42, 0.34, 0.42, 0.06);
            gl.fill(b);
        }
        ForwardFill | BackwardFill => {
            // forward.fill / backward.fill: two touching triangles.
            let k = if icon == ForwardFill { 1.0 } else { -1.0 };
            for x0 in [-0.54f32, 0.0] {
                let mut b = gl.path();
                b.move_to(gl.p(k * x0, -0.34));
                b.line_to(gl.p(k * (x0 + 0.54), 0.0));
                b.line_to(gl.p(k * x0, 0.34));
                b.close();
                gl.fill(b);
            }
        }
        MusicNote => {
            let mut b = gl.path();
            b.ellipse(crate::theme::Rect::from_ltrb(-0.38 * gl.s, 0.2 * gl.s, -0.02 * gl.s, 0.48 * gl.s));
            gl.fill(b);
            gl.line(-0.04, 0.32, -0.04, -0.46);
            let mut b = gl.path().hollow();
            b.move_to(gl.p(-0.04, -0.46));
            b.cubic_to(gl.p(0.06, -0.3), gl.p(0.34, -0.26), gl.p(0.3, -0.04));
            gl.stroke(b);
        }
        Photo => {
            gl.rrect(-0.54, -0.4, 0.54, 0.4, 0.1);
            gl.mountains(-0.54, -0.4, 0.54, 0.4);
        }
        Doc => {
            let mut b = gl.path();
            b.move_to(gl.p(-0.36, -0.42));
            b.quad_to(gl.p(-0.36, -0.5), gl.p(-0.28, -0.5));
            b.line_to(gl.p(0.1, -0.5));
            b.line_to(gl.p(0.36, -0.24));
            b.line_to(gl.p(0.36, 0.42));
            b.quad_to(gl.p(0.36, 0.5), gl.p(0.28, 0.5));
            b.line_to(gl.p(-0.28, 0.5));
            b.quad_to(gl.p(-0.36, 0.5), gl.p(-0.36, 0.42));
            b.close();
            gl.stroke(b);
            gl.poly(&[(0.08, -0.5), (0.08, -0.24), (0.36, -0.24)], false);
        }
        TextAlignLeft => {
            gl.line(-0.46, -0.32, 0.46, -0.32);
            gl.line(-0.46, -0.11, 0.18, -0.11);
            gl.line(-0.46, 0.1, 0.46, 0.1);
            gl.line(-0.46, 0.31, 0.06, 0.31);
        }
        ArrowDownToLine => {
            gl.line(0.0, -0.48, 0.0, 0.2);
            gl.poly(&[(-0.24, -0.04), (0.0, 0.2), (0.24, -0.04)], false);
            gl.line(-0.4, 0.44, 0.4, 0.44);
        }
        ArrowUpToLine => {
            gl.line(-0.4, -0.44, 0.4, -0.44);
            gl.line(0.0, 0.48, 0.0, -0.2);
            gl.poly(&[(-0.24, 0.04), (0.0, -0.2), (0.24, 0.04)], false);
        }
        TextFormat => {
            let wt = match weight {
                Weight::Bold | Weight::Heavy => 800,
                Weight::Semibold => 700,
                Weight::Medium => 600,
                _ => 500,
            };
            gl.text_glyph("A", 1.12, -0.2, 0.02, wt);
            gl.text_glyph("a", 0.9, 0.32, 0.1, wt);
        }
        ExternalDrive => {
            gl.rrect(-0.54, -0.3, 0.54, 0.3, 0.12);
            gl.line(-0.54, 0.06, 0.54, 0.06);
            gl.dot(0.34, 0.18, 0.05);
        }
        ChevronLeft => gl.poly(&[(0.12, -0.38), (-0.18, 0.0), (0.12, 0.38)], false),
        ChevronRight => gl.poly(&[(-0.12, -0.38), (0.18, 0.0), (-0.12, 0.38)], false),
        Pencil => {
            // Body along the diagonal, tip bottom-left.
            let mut b = gl.path();
            b.move_to(gl.p(0.26, -0.46));
            b.line_to(gl.p(0.46, -0.26));
            b.line_to(gl.p(-0.24, 0.44));
            b.line_to(gl.p(-0.48, 0.48));
            b.line_to(gl.p(-0.44, 0.24));
            b.close();
            gl.stroke(b);
            gl.line(0.12, -0.32, 0.32, -0.12);
        }
        SquareAndArrowDown => {
            gl.poly(&[(-0.16, -0.18), (-0.4, -0.18), (-0.4, 0.5), (0.4, 0.5), (0.4, -0.18), (0.16, -0.18)], false);
            gl.line(0.0, -0.56, 0.0, 0.18);
            gl.poly(&[(-0.17, 0.01), (0.0, 0.18), (0.17, 0.01)], false);
        }
        SquareAndArrowDownOnSquare => {
            gl.poly(&[(-0.24, -0.06), (-0.4, -0.06), (-0.4, 0.42), (0.28, 0.42), (0.28, -0.06), (0.12, -0.06)], false);
            gl.poly(&[(-0.14, 0.56), (0.5, 0.56), (0.5, 0.06)], false);
            gl.line(-0.06, -0.56, -0.06, 0.2);
            gl.poly(&[(-0.2, 0.06), (-0.06, 0.2), (0.08, 0.06)], false);
        }
        Pin => {
            let mut b = gl.path();
            b.move_to(gl.p(-0.16, -0.5));
            b.line_to(gl.p(0.16, -0.5));
            b.line_to(gl.p(0.14, -0.1));
            b.line_to(gl.p(0.32, 0.1));
            b.line_to(gl.p(-0.32, 0.1));
            b.line_to(gl.p(-0.14, -0.1));
            b.close();
            gl.stroke(b);
            gl.line(0.0, 0.1, 0.0, 0.54);
        }
        CursorArrow => {
            gl.poly(
                &[(-0.26, -0.48), (-0.26, 0.3), (-0.06, 0.12), (0.08, 0.44), (0.2, 0.39), (0.06, 0.07), (0.32, 0.07)],
                true,
            );
        }
        ArrowUpRight => {
            gl.line(-0.34, 0.34, 0.34, -0.34);
            gl.poly(&[(-0.1, -0.36), (0.36, -0.36), (0.36, 0.1)], false);
        }
        Ruler => {
            gl.rrect(-0.56, -0.22, 0.56, 0.22, 0.06);
            for (i, x) in [-0.32f32, -0.12, 0.08, 0.28].iter().enumerate() {
                let len = if i % 2 == 0 { 0.2 } else { 0.11 };
                gl.line(*x, -0.22, *x, -0.22 + len);
            }
        }
        Rectangle => gl.rrect(-0.54, -0.38, 0.54, 0.38, 0.1),
        RectangleFill => {
            let b = gl.rrect_path(-0.54, -0.38, 0.54, 0.38, 0.1);
            gl.fill(b);
        }
        Drop => {
            let mut b = gl.path();
            b.move_to(gl.p(0.0, -0.5));
            b.cubic_to(gl.p(-0.1, -0.32), gl.p(-0.36, -0.06), gl.p(-0.36, 0.16));
            b.cubic_to(gl.p(-0.36, 0.36), gl.p(-0.2, 0.5), gl.p(0.0, 0.5));
            b.cubic_to(gl.p(0.2, 0.5), gl.p(0.36, 0.36), gl.p(0.36, 0.16));
            b.cubic_to(gl.p(0.36, -0.06), gl.p(0.1, -0.32), gl.p(0.0, -0.5));
            b.close();
            gl.stroke(b);
        }
        Highlighter => {
            let mut b = gl.path();
            b.move_to(gl.p(0.18, -0.52));
            b.line_to(gl.p(0.46, -0.24));
            b.line_to(gl.p(0.02, 0.2));
            b.line_to(gl.p(-0.26, -0.08));
            b.close();
            gl.stroke(b);
            gl.poly(&[(-0.26, -0.08), (-0.34, 0.16), (-0.18, 0.28), (0.02, 0.2)], false);
            gl.poly(&[(-0.34, 0.16), (-0.48, 0.3), (-0.3, 0.36), (-0.18, 0.28)], false);
            gl.line(-0.5, 0.5, 0.5, 0.5);
        }
        RectangleDashed => {
            let b = gl.rrect_path(-0.54, -0.38, 0.54, 0.38, 0.1);
            gl.stroke_dashed(b, (0.16, 0.14));
        }
        OneCircle => {
            gl.circle(0.0, 0.0, 0.48);
            gl.text_glyph("1", 0.62, 0.0, 0.0, 600);
        }
        Circle => gl.circle(0.0, 0.0, 0.48),
        LineDiagonal => gl.line(-0.42, 0.42, 0.42, -0.42),
        PlusViewfinder => {
            gl.viewfinder(0.5, 0.2);
            gl.line(-0.2, 0.0, 0.2, 0.0);
            gl.line(0.0, -0.2, 0.0, 0.2);
        }
        Ellipsis => {
            gl.dot(-0.36, 0.0, 0.08);
            gl.dot(0.0, 0.0, 0.08);
            gl.dot(0.36, 0.0, 0.08);
        }
        PhotoOnRectangle => {
            gl.rrect(-0.56, -0.24, 0.32, 0.46, 0.1);
            gl.mountains(-0.56, -0.24, 0.32, 0.46);
            let mut b = gl.path().hollow();
            b.move_to(gl.p(-0.4, -0.38));
            b.line_to(gl.p(0.4, -0.38));
            b.quad_to(gl.p(0.5, -0.38), gl.p(0.5, -0.28));
            b.line_to(gl.p(0.5, 0.3));
            gl.stroke(b);
        }
        OneMagnifyingGlass => {
            gl.magnifier(-0.09, -0.09, 0.34);
            gl.text_glyph("1", 0.4, -0.09, -0.09, 600);
        }
        ArrowsOutward => {
            gl.line(-0.42, -0.42, -0.08, -0.08);
            gl.poly(&[(-0.42, -0.14), (-0.42, -0.42), (-0.14, -0.42)], false);
            gl.line(0.08, 0.08, 0.42, 0.42);
            gl.poly(&[(0.42, 0.14), (0.42, 0.42), (0.14, 0.42)], false);
        }
        ArrowsInward => {
            gl.line(-0.42, -0.42, -0.1, -0.1);
            gl.poly(&[(-0.1, -0.36), (-0.1, -0.1), (-0.36, -0.1)], false);
            gl.line(0.1, 0.1, 0.42, 0.42);
            gl.poly(&[(0.1, 0.36), (0.1, 0.1), (0.36, 0.1)], false);
        }
        Code => {
            gl.poly(&[(-0.24, -0.26), (-0.5, 0.0), (-0.24, 0.26)], false);
            gl.poly(&[(0.24, -0.26), (0.5, 0.0), (0.24, 0.26)], false);
            gl.line(0.1, -0.4, -0.1, 0.4);
        }
        FolderBadgePlus => {
            let mut b = gl.path();
            b.move_to(gl.p(0.0, 0.32));
            b.line_to(gl.p(-0.56, 0.32));
            b.line_to(gl.p(-0.56, -0.33));
            b.quad_to(gl.p(-0.56, -0.42), gl.p(-0.47, -0.42));
            b.line_to(gl.p(-0.2, -0.42));
            b.quad_to(gl.p(-0.14, -0.42), gl.p(-0.1, -0.37));
            b.line_to(gl.p(-0.04, -0.3));
            b.line_to(gl.p(0.47, -0.3));
            b.quad_to(gl.p(0.56, -0.3), gl.p(0.56, -0.21));
            b.line_to(gl.p(0.56, -0.04));
            gl.stroke(b);
            gl.line(0.32, 0.06, 0.32, 0.5);
            gl.line(0.1, 0.28, 0.54, 0.28);
        }
        PlusMagnifyingGlass => {
            gl.magnifier(-0.09, -0.09, 0.34);
            gl.line(-0.24, -0.09, 0.06, -0.09);
            gl.line(-0.09, -0.24, -0.09, 0.06);
        }
        MinusMagnifyingGlass => {
            gl.magnifier(-0.09, -0.09, 0.34);
            gl.line(-0.24, -0.09, 0.06, -0.09);
        }
    }
    gl.acc
}

/// Draw `icon` centred on `c` at SF point size `size`.
pub fn draw(cv: &mut Canvas, icon: Icon, c: Pt, size: f32, weight: Weight, color: Color) {
    if color.a * cv.alpha <= 0.0 {
        return;
    }
    let key = (icon, (size * 100.0).round() as i32, weight, cv.g.generation);
    let geom = CACHE.with(|m| {
        if let Some(g) = m.borrow().get(&key) {
            return g.clone();
        }
        let g = build(&cv.g, icon, size, weight);
        let mut map = m.borrow_mut();
        if map.len() > 600 {
            map.clear();
        }
        map.insert(key, g.clone());
        g
    });
    if let Some(geom) = geom {
        cv.save();
        cv.translate(c.x, c.y);
        cv.fill_geom(&geom, color);
        cv.restore();
    }
}

/// Ink bounds (width, height) of an icon — used to lay out HStacks.
pub fn size_of(g: &Gfx, icon: Icon, size: f32, weight: Weight) -> (f32, f32) {
    let key = (icon, (size * 100.0).round() as i32, weight, g.generation);
    let geom = CACHE.with(|m| {
        if let Some(gm) = m.borrow().get(&key) {
            return gm.clone();
        }
        let gm = build(g, icon, size, weight);
        m.borrow_mut().insert(key, gm.clone());
        gm
    });
    match geom {
        Some(gm) => unsafe {
            match gm.GetBounds(None) {
                Ok(b) => (b.right - b.left, b.bottom - b.top),
                Err(_) => (size, size),
            }
        },
        None => (size, size),
    }
}

/// Rasterise an icon (for menu items and the tray icon): premultiplied BGRA.
pub fn rasterize(g: &Gfx, icon: Icon, px: u32, size_pt: f32, weight: Weight, color: Color) -> Option<crate::imaging::Raw> {
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
    use windows::Win32::Graphics::Imaging::*;
    let wic = crate::imaging::wic()?;
    unsafe {
        let bmp = wic.CreateBitmap(px, px, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnLoad).ok()?;
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let rt = g.d2f.CreateWicBitmapRenderTarget(&bmp, &props).ok()?;
        let geom = build(g, icon, size_pt, weight)?;
        let brush = rt.CreateSolidColorBrush(&crate::gfx::col(color), None).ok()?;
        rt.BeginDraw();
        rt.Clear(Some(&crate::gfx::col(Color::CLEAR)));
        let c = px as f32 / 2.0;
        rt.SetTransform(&Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: c, M32: c });
        rt.FillGeometry(&geom, &brush, None);
        rt.EndDraw(None, None).ok()?;
        drop(rt);
        let mut out = vec![0u8; (px * px * 4) as usize];
        bmp.CopyPixels(std::ptr::null(), px * 4, &mut out).ok()?;
        Some(crate::imaging::Raw { w: px, h: px, px: out })
    }
}

pub fn clear_cache() {
    CACHE.with(|m| m.borrow_mut().clear());
}

/// CheckShape (Island.swift) stroked inside `r`, drawn up to `t` (0…1) so it
/// can "draw itself" like SwiftUI's `.trim(from: 0, to: t)`.
pub fn check(cv: &mut Canvas, r: Rect, t: f32, width: f32, color: Color) {
    let t = t.clamp(0.0, 1.0);
    if t <= 0.0 {
        return;
    }
    let a = pt(r.x, r.cy() + r.h * 0.05);
    let b = pt(r.x + r.w * 0.36, r.b());
    let c = pt(r.r(), r.y);
    let l1 = a.dist(b);
    let l2 = b.dist(c);
    let len = (l1 + l2) * t;
    let lerp = |p: Pt, q: Pt, k: f32| pt(p.x + (q.x - p.x) * k, p.y + (q.y - p.y) * k);
    if len <= l1 {
        cv.line(a, lerp(a, b, len / l1.max(0.001)), width, color, 1);
    } else {
        cv.line(a, b, width, color, 1);
        cv.line(b, lerp(b, c, (len - l1) / l2.max(0.001)), width, color, 1);
    }
}
