//! Design tokens and geometry — values from mac/Sources/ImageVault/Theme.swift.

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Color {
        Color { r, g, b, a }
    }
    /// NSColor(white:alpha:)
    pub const fn gray(w: f32, a: f32) -> Color {
        Color { r: w, g: w, b: w, a }
    }
    pub const fn white(a: f32) -> Color {
        Color::gray(1.0, a)
    }
    pub const fn black(a: f32) -> Color {
        Color::gray(0.0, a)
    }
    pub const CLEAR: Color = Color::gray(0.0, 0.0);
    pub fn with_alpha(self, a: f32) -> Color {
        Color { a, ..self }
    }
    pub fn mul_alpha(self, k: f32) -> Color {
        Color { a: self.a * k, ..self }
    }
    pub fn lerp(self, o: Color, t: f32) -> Color {
        Color {
            r: self.r + (o.r - self.r) * t,
            g: self.g + (o.g - self.g) * t,
            b: self.b + (o.b - self.b) * t,
            a: self.a + (o.a - self.a) * t,
        }
    }
    pub fn hex(&self) -> String {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        format!("#{:02X}{:02X}{:02X}", c(self.r), c(self.g), c(self.b))
    }
    /// HSB brightness / saturation (NSColor.brightnessComponent etc.).
    pub fn brightness(&self) -> f32 {
        self.r.max(self.g).max(self.b)
    }
    pub fn saturation(&self) -> f32 {
        let max = self.r.max(self.g).max(self.b);
        let min = self.r.min(self.g).min(self.b);
        if max <= 0.0 { 0.0 } else { (max - min) / max }
    }
}

pub const INK: Color = Color::gray(0.0, 1.0);
pub const PAPER: Color = Color::gray(1.0, 1.0);
pub const SOFT: Color = Color::gray(0.898, 1.0);
pub const MUTED: Color = Color::gray(0.55, 1.0);
pub const DIM: Color = Color::gray(0.29, 1.0);
pub const CARD: Color = Color::gray(0.047, 1.0);
pub const LINE: Color = Color::gray(1.0, 0.12);
pub const LINE2: Color = Color::gray(1.0, 0.26);

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

pub const fn pt(x: f32, y: f32) -> Pt {
    Pt { x, y }
}

impl Pt {
    pub fn dist(&self, o: Pt) -> f32 {
        ((self.x - o.x).powi(2) + (self.y - o.y).powi(2)).sqrt()
    }
    pub fn add(&self, dx: f32, dy: f32) -> Pt {
        Pt { x: self.x + dx, y: self.y + dy }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

pub const fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect { x, y, w, h }
}

impl Rect {
    pub fn from_ltrb(l: f32, t: f32, r: f32, b: f32) -> Rect {
        Rect { x: l, y: t, w: r - l, h: b - t }
    }
    pub fn from_points(a: Pt, b: Pt) -> Rect {
        Rect::from_ltrb(a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y))
    }
    pub fn r(&self) -> f32 {
        self.x + self.w
    }
    pub fn b(&self) -> f32 {
        self.y + self.h
    }
    pub fn cx(&self) -> f32 {
        self.x + self.w / 2.0
    }
    pub fn cy(&self) -> f32 {
        self.y + self.h / 2.0
    }
    pub fn center(&self) -> Pt {
        pt(self.cx(), self.cy())
    }
    pub fn contains(&self, p: Pt) -> bool {
        p.x >= self.x && p.y >= self.y && p.x < self.r() && p.y < self.b()
    }
    /// NSRect.insetBy(dx:dy:) — negative values grow the rect.
    pub fn inset(&self, dx: f32, dy: f32) -> Rect {
        Rect { x: self.x + dx, y: self.y + dy, w: self.w - 2.0 * dx, h: self.h - 2.0 * dy }
    }
    pub fn offset(&self, dx: f32, dy: f32) -> Rect {
        Rect { x: self.x + dx, y: self.y + dy, ..*self }
    }
    pub fn intersects(&self, o: &Rect) -> bool {
        self.x < o.r() && o.x < self.r() && self.y < o.b() && o.y < self.b()
    }
    pub fn intersection(&self, o: &Rect) -> Rect {
        let l = self.x.max(o.x);
        let t = self.y.max(o.y);
        let r = self.r().min(o.r());
        let b = self.b().min(o.b());
        if r <= l || b <= t { Rect::default() } else { Rect::from_ltrb(l, t, r, b) }
    }
    pub fn union(&self, o: &Rect) -> Rect {
        Rect::from_ltrb(self.x.min(o.x), self.y.min(o.y), self.r().max(o.r()), self.b().max(o.b()))
    }
    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }
    /// Rect of `size` centred in self.
    pub fn centered(&self, w: f32, h: f32) -> Rect {
        Rect { x: self.cx() - w / 2.0, y: self.cy() - h / 2.0, w, h }
    }
    pub fn round(&self) -> Rect {
        Rect::from_ltrb(self.x.round(), self.y.round(), self.r().round(), self.b().round())
    }
    /// Aspect-fit a `iw`×`ih` image inside self.
    pub fn fit(&self, iw: f32, ih: f32) -> Rect {
        if iw <= 0.0 || ih <= 0.0 {
            return *self;
        }
        let k = (self.w / iw).min(self.h / ih);
        self.centered(iw * k, ih * k)
    }
    /// Aspect-fill (cover).
    pub fn fill(&self, iw: f32, ih: f32) -> Rect {
        if iw <= 0.0 || ih <= 0.0 {
            return *self;
        }
        let k = (self.w / iw).max(self.h / ih);
        self.centered(iw * k, ih * k)
    }
}

/// Text weight (NSFont.Weight / SwiftUI Font.Weight).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Weight {
    UltraLight,
    Light,
    Regular,
    Medium,
    Semibold,
    Bold,
    Heavy,
}

impl Weight {
    pub fn dwrite(self) -> i32 {
        match self {
            Weight::UltraLight => 200,
            Weight::Light => 300,
            Weight::Regular => 400,
            Weight::Medium => 500,
            Weight::Semibold => 600,
            Weight::Bold => 700,
            Weight::Heavy => 800,
        }
    }
}

/// A text style: size, weight, monospaced design, letter spacing (kerning).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Font {
    pub size: f32,
    pub weight: Weight,
    pub mono: bool,
    pub kern: f32,
}

impl Font {
    pub const fn sans(size: f32, weight: Weight) -> Font {
        Font { size, weight, mono: false, kern: 0.0 }
    }
    pub const fn mono(size: f32, weight: Weight) -> Font {
        Font { size, weight, mono: true, kern: 0.0 }
    }
    pub const fn kerning(self, k: f32) -> Font {
        Font { kern: k, ..self }
    }
}

/// Byte count like ByteCountFormatter(.file): decimal units, "Zero KB".
pub fn byte_count(bytes: i64) -> String {
    let b = bytes.max(0) as f64;
    if b == 0.0 {
        return "Zero KB".into();
    }
    if b < 1000.0 {
        return format!("{} bytes", b as i64);
    }
    let units = ["KB", "MB", "GB", "TB"];
    let mut v = b / 1000.0;
    let mut i = 0;
    while v >= 1000.0 && i < units.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 {
        format!("{} KB", v.round() as i64)
    } else {
        let s = format!("{:.1}", v);
        let s = s.trim_end_matches(".0").to_string();
        format!("{s} {}", units[i])
    }
}
