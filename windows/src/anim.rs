//! Animation engine reproducing SwiftUI springs, CASpringAnimation "pop" and
//! CAMediaTimingFunction curves. Springs are integrated in closed form, so the
//! motion is frame-rate independent and identical to the Mac app's physics.

use std::collections::HashMap;

/// A timing curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Curve {
    /// No animation: jump to the target.
    Snap,
    /// Damped spring with explicit physics (mass, stiffness, damping coefficient).
    Spring { mass: f32, stiffness: f32, damping: f32, delay: f32 },
    /// Cubic-bezier timing over a fixed duration.
    Tween { dur: f32, x1: f32, y1: f32, x2: f32, y2: f32, delay: f32 },
}

impl Curve {
    /// SwiftUI `.spring(response:dampingFraction:)`.
    pub const fn spring(response: f32, damping_fraction: f32) -> Curve {
        // stiffness = (2π / response)², damping = 4π ζ / response, mass = 1
        let w = 2.0 * std::f32::consts::PI / response;
        Curve::Spring { mass: 1.0, stiffness: w * w, damping: 4.0 * std::f32::consts::PI * damping_fraction / response, delay: 0.0 }
    }
    /// CASpringAnimation with Theme.pop's parameters.
    pub const POP: Curve = Curve::Spring { mass: 0.8, stiffness: 260.0, damping: 12.0, delay: 0.0 };
    pub const fn ease_out(dur: f32) -> Curve {
        Curve::Tween { dur, x1: 0.0, y1: 0.0, x2: 0.58, y2: 1.0, delay: 0.0 }
    }
    pub const fn ease_in_out(dur: f32) -> Curve {
        Curve::Tween { dur, x1: 0.42, y1: 0.0, x2: 0.58, y2: 1.0, delay: 0.0 }
    }
    pub const fn linear(dur: f32) -> Curve {
        Curve::Tween { dur, x1: 0.0, y1: 0.0, x2: 1.0, y2: 1.0, delay: 0.0 }
    }
    /// Theme.ease — cubic-bezier(0.22, 1, 0.36, 1).
    pub const fn mac(dur: f32) -> Curve {
        Curve::Tween { dur, x1: 0.22, y1: 1.0, x2: 0.36, y2: 1.0, delay: 0.0 }
    }
    pub const fn delay(self, d: f32) -> Curve {
        match self {
            Curve::Spring { mass, stiffness, damping, .. } => Curve::Spring { mass, stiffness, damping, delay: d },
            Curve::Tween { dur, x1, y1, x2, y2, .. } => Curve::Tween { dur, x1, y1, x2, y2, delay: d },
            Curve::Snap => Curve::Snap,
        }
    }
}

/// Island.swift `Motion`.
pub mod motion {
    use super::Curve;
    pub const OPEN: Curve = Curve::spring(0.42, 0.74);
    pub const CLOSE: Curve = Curve::spring(0.36, 0.9);
    pub const CONTENT_IN: Curve = Curve::spring(0.38, 0.86).delay(0.07);
    pub const CONTENT_OUT: Curve = Curve::ease_out(0.12);
    pub const HOVER: Curve = Curve::spring(0.26, 0.62);
}

pub fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t <= 0.0 || t >= 1.0 {
        return t;
    }
    let bx = |u: f32| {
        let m = 1.0 - u;
        3.0 * m * m * u * x1 + 3.0 * m * u * u * x2 + u * u * u
    };
    let dbx = |u: f32| {
        let m = 1.0 - u;
        3.0 * m * m * x1 + 6.0 * m * u * (x2 - x1) + 3.0 * u * u * (1.0 - x2)
    };
    // Newton, then bisection fallback.
    let mut u = t;
    for _ in 0..8 {
        let x = bx(u) - t;
        if x.abs() < 1e-5 {
            break;
        }
        let d = dbx(u);
        if d.abs() < 1e-6 {
            break;
        }
        u = (u - x / d).clamp(0.0, 1.0);
    }
    if (bx(u) - t).abs() > 1e-3 {
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..30 {
            u = (lo + hi) / 2.0;
            if bx(u) < t { lo = u } else { hi = u }
        }
    }
    let m = 1.0 - u;
    3.0 * m * m * u * y1 + 3.0 * m * u * u * y2 + u * u * u
}

/// Closed-form damped oscillator: advances displacement `d` (from target) and
/// velocity `v` by `dt` seconds.
fn spring_step(d: f64, v: f64, mass: f64, k: f64, c: f64, dt: f64) -> (f64, f64) {
    let w0 = (k / mass).sqrt();
    let z = c / (2.0 * (k * mass).sqrt());
    if z < 1.0 - 1e-6 {
        let wd = w0 * (1.0 - z * z).sqrt();
        let a = (v + z * w0 * d) / wd;
        let e = (-z * w0 * dt).exp();
        let (s, co) = (wd * dt).sin_cos();
        let x = e * (d * co + a * s);
        let vel = e * (-z * w0 * (d * co + a * s) + (-d * wd * s + a * wd * co));
        (x, vel)
    } else if z > 1.0 + 1e-6 {
        let s = (z * z - 1.0).sqrt();
        let r1 = -w0 * (z - s);
        let r2 = -w0 * (z + s);
        let c2 = (v - r1 * d) / (r2 - r1);
        let c1 = d - c2;
        let e1 = (r1 * dt).exp();
        let e2 = (r2 * dt).exp();
        (c1 * e1 + c2 * e2, c1 * r1 * e1 + c2 * r2 * e2)
    } else {
        // Critical: x(t) = (d + (v + w0 d) t) e^{-w0 t}
        let a = v + w0 * d;
        let e = (-w0 * dt).exp();
        ((d + a * dt) * e, (a - w0 * (d + a * dt)) * e)
    }
}

/// One animated scalar.
#[derive(Debug, Clone, Copy)]
pub struct Anim {
    pub v: f32,
    pub target: f32,
    vel: f32,
    from: f32,
    t: f32,
    delay: f32,
    curve: Curve,
    active: bool,
}

impl Anim {
    pub fn new(v: f32) -> Anim {
        Anim { v, target: v, vel: 0.0, from: v, t: 0.0, delay: 0.0, curve: Curve::Snap, active: false }
    }

    /// Animate toward `target`. Springs keep their velocity on retarget (like
    /// SwiftUI); tweens restart from the current value.
    pub fn set(&mut self, target: f32, curve: Curve) {
        if (target - self.target).abs() < 1e-6 && (self.active || (self.v - target).abs() < 1e-6) {
            return;
        }
        self.target = target;
        self.curve = curve;
        match curve {
            Curve::Snap => {
                self.v = target;
                self.vel = 0.0;
                self.active = false;
            }
            Curve::Spring { delay, .. } => {
                self.delay = delay;
                self.active = true;
            }
            Curve::Tween { delay, .. } => {
                self.from = self.v;
                self.t = 0.0;
                self.delay = delay;
                self.active = true;
            }
        }
    }

    /// Jump to `from` and animate to `to` (CASpringAnimation fromValue/toValue).
    pub fn play(&mut self, from: f32, to: f32, curve: Curve) {
        self.v = from;
        self.vel = 0.0;
        self.target = f32::NAN;
        self.set(to, curve);
    }

    pub fn snap(&mut self, v: f32) {
        self.v = v;
        self.target = v;
        self.vel = 0.0;
        self.active = false;
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Advance; returns true while moving.
    pub fn tick(&mut self, dt: f32) -> bool {
        if !self.active {
            return false;
        }
        let mut dt = dt;
        if self.delay > 0.0 {
            if dt <= self.delay {
                self.delay -= dt;
                return true;
            }
            dt -= self.delay;
            self.delay = 0.0;
            if let Curve::Tween { .. } = self.curve {
                self.from = self.v;
            }
        }
        match self.curve {
            Curve::Snap => {
                self.v = self.target;
                self.active = false;
            }
            Curve::Spring { mass, stiffness, damping, .. } => {
                let (d, v) = spring_step(
                    (self.v - self.target) as f64,
                    self.vel as f64,
                    mass as f64,
                    stiffness as f64,
                    damping as f64,
                    dt as f64,
                );
                self.v = self.target + d as f32;
                self.vel = v as f32;
                let scale = 1.0f32.max((self.target).abs() * 0.01).min(10.0);
                if d.abs() < 0.0006 * scale as f64 && v.abs() < 0.01 * scale as f64 {
                    self.v = self.target;
                    self.vel = 0.0;
                    self.active = false;
                }
            }
            Curve::Tween { dur, x1, y1, x2, y2, .. } => {
                self.t += dt;
                if self.t >= dur || dur <= 0.0 {
                    self.v = self.target;
                    self.active = false;
                } else {
                    let e = cubic_bezier(x1, y1, x2, y2, self.t / dur);
                    self.v = self.from + (self.target - self.from) * e;
                }
            }
        }
        self.active
    }
}

/// FNV-1a key helper.
pub const fn key(s: &str) -> u64 {
    let b = s.as_bytes();
    let mut h: u64 = 0xcbf29ce484222325;
    let mut i = 0;
    while i < b.len() {
        h ^= b[i] as u64;
        h = h.wrapping_mul(0x100000001b3);
        i += 1;
    }
    h
}

pub fn key_id(s: &str, id: &str) -> u64 {
    let mut h = key(s);
    for byte in id.as_bytes() {
        h ^= *byte as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

pub fn key_n(s: &str, n: u64) -> u64 {
    key(s) ^ n.wrapping_mul(0x9E3779B97F4A7C15)
}

struct Entry {
    a: Anim,
    touched: f64,
}

/// Declarative animation store: view code states the value it wants each
/// frame; changes animate with the given curve (SwiftUI `.animation(_:value:)`).
pub struct Anims {
    map: HashMap<u64, Entry>,
    pub now: f64,
}

impl Default for Anims {
    fn default() -> Self {
        Self::new()
    }
}

impl Anims {
    pub fn new() -> Anims {
        Anims { map: HashMap::new(), now: 0.0 }
    }

    /// Current value for `k`, animating to `target` with `curve` when it changes.
    /// A new entry starts at `target` (no animation).
    pub fn drive(&mut self, k: u64, target: f32, curve: Curve) -> f32 {
        let now = self.now;
        let e = self.map.entry(k).or_insert_with(|| Entry { a: Anim::new(target), touched: now });
        e.touched = now;
        e.a.set(target, curve);
        e.a.v
    }

    /// Like `drive`, but a new entry starts at `initial` and animates in.
    pub fn drive_from(&mut self, k: u64, initial: f32, target: f32, curve: Curve) -> f32 {
        let now = self.now;
        let e = self.map.entry(k).or_insert_with(|| Entry { a: Anim::new(initial), touched: now });
        e.touched = now;
        e.a.set(target, curve);
        e.a.v
    }

    pub fn get(&self, k: u64) -> Option<f32> {
        self.map.get(&k).map(|e| e.a.v)
    }

    pub fn anim(&mut self, k: u64, initial: f32) -> &mut Anim {
        let now = self.now;
        let e = self.map.entry(k).or_insert_with(|| Entry { a: Anim::new(initial), touched: now });
        e.touched = now;
        &mut e.a
    }

    pub fn play(&mut self, k: u64, from: f32, to: f32, curve: Curve) {
        self.anim(k, to).play(from, to, curve);
    }

    pub fn snap(&mut self, k: u64, v: f32) {
        self.anim(k, v).snap(v);
    }

    pub fn remove(&mut self, k: u64) {
        self.map.remove(&k);
    }

    /// Advance everything; true while any value moves.
    pub fn tick(&mut self, dt: f32) -> bool {
        let mut any = false;
        for e in self.map.values_mut() {
            if e.a.tick(dt) {
                any = true;
            }
        }
        any
    }

    pub fn any_active(&self) -> bool {
        self.map.values().any(|e| e.a.active)
    }

    /// Drop settled values nobody asked for in `age` seconds.
    pub fn gc(&mut self, age: f64) {
        let now = self.now;
        self.map.retain(|_, e| e.a.active || now - e.touched < age);
    }
}
