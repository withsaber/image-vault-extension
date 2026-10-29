//! Single-line text editing for our Direct2D UI (search field, editor text):
//! caret, selection, word moves, clipboard, undo, mouse selection and IME
//! window positioning. Rendering uses DirectWrite hit-testing.

use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;

use crate::gfx::{Align, Canvas, Trim};
use crate::theme::{Color, Font, Rect};

#[derive(Default, Clone)]
pub struct TextField {
    pub text: String,
    /// Caret and anchor, as char indices.
    pub caret: usize,
    pub anchor: usize,
    pub scroll: f32,
    undo: Vec<(String, usize)>,
    pub blink_epoch: f64,
    dragging: bool,
}

pub enum KeyResult {
    /// Handled; text may have changed.
    Handled { changed: bool },
    /// Enter / Escape etc. — the owner decides.
    Pass,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl TextField {
    pub fn new() -> TextField {
        TextField::default()
    }

    fn chars(&self) -> Vec<char> {
        self.text.chars().collect()
    }

    pub fn len(&self) -> usize {
        self.text.chars().count()
    }

    pub fn set(&mut self, s: &str) {
        self.text = s.to_string();
        self.caret = self.len();
        self.anchor = self.caret;
    }

    pub fn clear(&mut self) {
        if !self.text.is_empty() {
            self.push_undo();
        }
        self.text.clear();
        self.caret = 0;
        self.anchor = 0;
        self.scroll = 0.0;
    }

    fn sel(&self) -> (usize, usize) {
        (self.caret.min(self.anchor), self.caret.max(self.anchor))
    }

    pub fn has_selection(&self) -> bool {
        self.caret != self.anchor
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.len();
    }

    fn push_undo(&mut self) {
        self.undo.push((self.text.clone(), self.caret));
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
    }

    fn replace_sel(&mut self, s: &str) {
        self.push_undo();
        let c = self.chars();
        let (a, b) = self.sel();
        let mut out: String = c[..a].iter().collect();
        let ins: String = s.chars().filter(|ch| *ch != '\n' && *ch != '\r').collect();
        out.push_str(&ins);
        out.extend(c[b..].iter());
        self.text = out;
        self.caret = a + ins.chars().count();
        self.anchor = self.caret;
    }

    pub fn insert(&mut self, s: &str) {
        self.replace_sel(s);
        self.touch();
    }

    fn touch(&mut self) {
        self.blink_epoch = crate::util::mono();
    }

    fn word_left(&self, from: usize) -> usize {
        let c = self.chars();
        let mut i = from;
        while i > 0 && !is_word(c[i - 1]) {
            i -= 1;
        }
        while i > 0 && is_word(c[i - 1]) {
            i -= 1;
        }
        i
    }

    fn word_right(&self, from: usize) -> usize {
        let c = self.chars();
        let mut i = from;
        while i < c.len() && !is_word(c[i]) {
            i += 1;
        }
        while i < c.len() && is_word(c[i]) {
            i += 1;
        }
        i
    }

    /// WM_KEYDOWN. Clipboard and undo shortcuts included.
    pub fn key(&mut self, vk: u16) -> KeyResult {
        let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
        let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
        let len = self.len();
        let mut changed = false;
        match VIRTUAL_KEY(vk) {
            VK_LEFT => {
                if self.has_selection() && !shift {
                    self.caret = self.sel().0;
                } else {
                    self.caret = if ctrl { self.word_left(self.caret) } else { self.caret.saturating_sub(1) };
                }
                if !shift {
                    self.anchor = self.caret;
                }
            }
            VK_RIGHT => {
                if self.has_selection() && !shift {
                    self.caret = self.sel().1;
                } else {
                    self.caret = if ctrl { self.word_right(self.caret) } else { (self.caret + 1).min(len) };
                }
                if !shift {
                    self.anchor = self.caret;
                }
            }
            VK_HOME | VK_UP => {
                self.caret = 0;
                if !shift {
                    self.anchor = 0;
                }
            }
            VK_END | VK_DOWN => {
                self.caret = len;
                if !shift {
                    self.anchor = len;
                }
            }
            VK_BACK => {
                if !self.has_selection() {
                    if self.caret == 0 {
                        return KeyResult::Handled { changed: false };
                    }
                    self.anchor = if ctrl { self.word_left(self.caret) } else { self.caret - 1 };
                }
                self.replace_sel("");
                changed = true;
            }
            VK_DELETE => {
                if !self.has_selection() {
                    if self.caret >= len {
                        return KeyResult::Handled { changed: false };
                    }
                    self.anchor = if ctrl { self.word_right(self.caret) } else { self.caret + 1 };
                }
                self.replace_sel("");
                changed = true;
            }
            _ if ctrl => match vk as u8 {
                b'A' => self.select_all(),
                b'C' => {
                    if self.has_selection() {
                        crate::clipboard::copy_text(&self.selected());
                    }
                }
                b'X' => {
                    if self.has_selection() {
                        crate::clipboard::copy_text(&self.selected());
                        self.replace_sel("");
                        changed = true;
                    }
                }
                b'V' => {
                    if let Some(s) = crate::clipboard::read().and_then(|s| s.text) {
                        self.replace_sel(&s);
                        changed = true;
                    }
                }
                b'Z' => {
                    if let Some((t, c)) = self.undo.pop() {
                        self.text = t;
                        self.caret = c.min(self.len());
                        self.anchor = self.caret;
                        changed = true;
                    }
                }
                _ => return KeyResult::Pass,
            },
            _ => return KeyResult::Pass,
        }
        self.touch();
        KeyResult::Handled { changed }
    }

    /// WM_CHAR. Returns true when the text changed.
    pub fn char(&mut self, c: char) -> bool {
        if c.is_control() {
            return false;
        }
        self.replace_sel(&c.to_string());
        self.touch();
        true
    }

    pub fn selected(&self) -> String {
        let (a, b) = self.sel();
        self.chars()[a..b].iter().collect()
    }

    fn index_at(&self, cv_g: &crate::gfx::Gfx, font: &Font, x: f32) -> usize {
        let l = cv_g.layout(&self.text, font, 100000.0, 100000.0, false, Align::Leading, Trim::None);
        let mut trailing = windows::core::BOOL(0);
        let mut inside = windows::core::BOOL(0);
        let mut m = DWRITE_HIT_TEST_METRICS::default();
        unsafe {
            let _ = l.HitTestPoint(x, 1.0, &mut trailing, &mut inside, &mut m);
        }
        let utf16 = m.textPosition as usize + if trailing.as_bool() { m.length as usize } else { 0 };
        // UTF-16 offset → char index.
        let mut n = 0usize;
        let mut ci = 0usize;
        for ch in self.text.chars() {
            if n >= utf16 {
                break;
            }
            n += ch.len_utf16();
            ci += 1;
        }
        ci
    }

    fn x_of(&self, g: &crate::gfx::Gfx, font: &Font, idx: usize) -> f32 {
        let prefix: String = self.text.chars().take(idx).collect();
        g.measure(&prefix, font).0
    }

    pub fn mouse_down(&mut self, g: &crate::gfx::Gfx, font: &Font, field: Rect, x: f32, shift: bool, clicks: u32) {
        let i = self.index_at(g, font, x - field.x + self.scroll);
        if clicks >= 3 {
            self.select_all();
        } else if clicks == 2 {
            let c = self.chars();
            let mut a = i.min(c.len());
            let mut b = a;
            while a > 0 && is_word(c[a - 1]) {
                a -= 1;
            }
            while b < c.len() && is_word(c[b]) {
                b += 1;
            }
            self.anchor = a;
            self.caret = b;
        } else {
            self.caret = i;
            if !shift {
                self.anchor = i;
            }
            self.dragging = true;
        }
        self.touch();
    }

    pub fn mouse_drag(&mut self, g: &crate::gfx::Gfx, font: &Font, field: Rect, x: f32) {
        if self.dragging {
            self.caret = self.index_at(g, font, x - field.x + self.scroll);
            self.touch();
        }
    }

    pub fn mouse_up(&mut self) {
        self.dragging = false;
    }

    /// Caret x in window space (for the IME window).
    pub fn caret_x(&self, g: &crate::gfx::Gfx, font: &Font, field: Rect) -> f32 {
        field.x + self.x_of(g, font, self.caret) - self.scroll
    }

    /// Draw text, selection, caret (when focused) or the placeholder.
    pub fn draw(&mut self, cv: &mut Canvas, font: &Font, color: Color, field: Rect, focused: bool, placeholder: &str, placeholder_color: Color) {
        let g = cv.g.clone();
        let caret_x = self.x_of(&g, font, self.caret);
        // Keep the caret visible.
        if caret_x - self.scroll > field.w - 2.0 {
            self.scroll = caret_x - field.w + 2.0;
        }
        if caret_x - self.scroll < 0.0 {
            self.scroll = caret_x.max(0.0);
        }
        let total = g.measure(&self.text, font).0;
        if total - self.scroll < field.w - 2.0 {
            self.scroll = (total - field.w + 2.0).max(0.0);
        }
        cv.clip(field.inset(-1.0, -2.0));
        if self.text.is_empty() {
            if !placeholder.is_empty() {
                cv.text(placeholder, font, placeholder_color, field, Align::Leading, Trim::Tail);
            }
        } else {
            if focused && self.has_selection() {
                let (a, b) = self.sel();
                let x0 = self.x_of(&g, font, a) - self.scroll;
                let x1 = self.x_of(&g, font, b) - self.scroll;
                let lh = g.line_height(font);
                let y = field.cy() - lh / 2.0;
                cv.fill_rect(Rect { x: field.x + x0, y, w: x1 - x0, h: lh }, Color::rgba(0.25, 0.45, 0.85, 0.55));
            }
            let r = Rect { x: field.x - self.scroll, y: field.y, w: total + 4.0, h: field.h };
            cv.text(&self.text, font, color, r, Align::Leading, Trim::None);
        }
        if focused {
            let t = crate::util::mono() - self.blink_epoch;
            let on = t < 0.6 || ((t - 0.6) % 1.0) < 0.5;
            if on {
                let lh = g.line_height(font);
                let x = (field.x + caret_x - self.scroll).round() + 0.5;
                cv.fill_rect(Rect { x: x - 0.5, y: field.cy() - lh / 2.0 + 1.0, w: 1.5, h: lh - 2.0 }, Color::white(0.95));
            }
        }
        cv.unclip();
    }
}
