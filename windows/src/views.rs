//! IslandView content: header (tabs, message/count, buttons), search bar,
//! bulk bar, empty tray, drop target, live activity, drag hint and the
//! clipboard grid. Metrics and motion follow Island.swift exactly.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::anim::{Curve, key, key_id, motion};
use crate::app::App;
use crate::gfx::{Align, Canvas, Trim};
use crate::icons::{self, Icon};
use crate::island::{self, Act, Hit, State, Tab};
use crate::theme::{Color, Font, Pt, Rect, Weight, pt, rect};
use crate::{cliphistory, util};

pub const SEARCH_FONT: Font = Font::sans(11.0, Weight::Regular);

thread_local! {
    static SEARCH_FIELD: RefCell<Option<Rect>> = const { RefCell::new(None) };
    static CLIP: RefCell<ClipState> = RefCell::new(ClipState::default());
}

#[derive(Default)]
struct ClipState {
    scroll: f32,
    content_h: f32,
    view_h: f32,
    copied: HashMap<String, f64>,
    drag_from: Option<(String, Pt)>,
    dragged: bool,
}

pub fn search_field_rect(_app: &App) -> Option<Rect> {
    SEARCH_FIELD.with(|s| *s.borrow())
}

fn hit(app: &mut App, r: Rect, id: u64, act: Act, tip: Option<&str>) {
    app.island.hits.push(Hit { rect: r, id, tip: tip.map(str::to_string), act, text: false });
}

fn hovered(app: &App, id: u64) -> bool {
    app.island.hover == Some(id)
}

fn pressed(app: &App, id: u64) -> bool {
    app.island.pressed.as_ref().map(|(p, _)| *p == id).unwrap_or(false)
}

fn iw(cv: &Canvas, icon: Icon, size: f32, w: Weight) -> f32 {
    icons::size_of(&cv.g, icon, size, w).0
}

/// SF Symbol image frames are a little wider than the ink.
fn img_w(cv: &Canvas, icon: Icon, size: f32, w: Weight) -> f32 {
    iw(cv, icon, size, w) + size * 0.12
}

fn lh(cv: &Canvas, f: &Font) -> f32 {
    cv.g.line_height(f)
}

// ---------------------------------------------------------------------------
// Activity / drag hint / drag over
// ---------------------------------------------------------------------------

pub fn activity(cv: &mut Canvas, app: &mut App, lr: Rect) {
    let pad = 12.0;
    let img = rect(lr.x + pad, lr.cy() - 11.0, 22.0, 22.0);
    if app.island.activity.bitmap.is_none() {
        if let Some(raw) = &app.island.activity.image {
            app.island.activity.bitmap = cv.g.bitmap(raw.w, raw.h, &raw.px);
        }
    }
    if let Some(b) = app.island.activity.bitmap.clone() {
        let geom = cv.g.squircle(22.0, 22.0, 6.0);
        cv.save();
        cv.translate(img.x, img.y);
        let gm: windows::Win32::Graphics::Direct2D::ID2D1Geometry = windows::core::Interface::cast(&geom).unwrap();
        cv.push_mask(&gm);
        cv.restore();
        cv.image_fill(&b, img, 1.0);
        cv.pop_layer();
        cv.stroke_round_centered(img, 6.0, 0.5, Color::white(0.25));
    } else {
        icons::draw(cv, Icon::Photo, img.center(), 13.0, Weight::Regular, Color::white(1.0));
        cv.stroke_round_centered(img, 6.0, 0.5, Color::white(0.25));
    }
    let title_f = Font::sans(12.0, Weight::Semibold);
    let detail_f = Font::mono(9.5, Weight::Regular);
    let title = app.island.activity.title.clone();
    let detail = app.island.activity.detail.clone();
    let (tw, th) = cv.measure(&title, &title_f);
    let (dw, dh) = if detail.is_empty() { (0.0, 0.0) } else { cv.measure(&detail, &detail_f) };
    let total_h = th + dh;
    let right = lr.r() - pad;
    let y0 = lr.cy() - total_h / 2.0;
    let check_w = img_w(cv, Icon::CheckmarkCircleFill, 11.0, Weight::Bold);
    let row_w = check_w + 4.0 + tw;
    let max_w = (right - img.r() - 8.0).max(10.0);
    let tx = right - row_w.min(max_w);
    icons::draw(cv, Icon::CheckmarkCircleFill, pt(tx + check_w / 2.0, y0 + th / 2.0), 11.0, Weight::Bold, Color::white(1.0));
    cv.text(&title, &title_f, Color::white(1.0), rect(tx + check_w + 4.0, y0, (right - tx - check_w - 4.0).max(1.0), th), Align::Leading, Trim::Tail);
    if !detail.is_empty() {
        let dx = right - dw.min(max_w);
        cv.text(&detail, &detail_f, Color::white(0.55), rect(dx, y0 + th, right - dx, dh), Align::Trailing, Trim::Tail);
    }
}

pub fn drag_hint(cv: &mut Canvas, app: &mut App, lr: Rect) {
    let band = app.island.band();
    let area = rect(lr.x, lr.y + band, lr.w, lr.h - band);
    let f = Font::sans(12.0, Weight::Semibold);
    let text = "Drop here to save";
    let (tw, th) = cv.measure(text, &f);
    let icw = img_w(cv, Icon::ArrowDownCircleFill, 14.0, Weight::Medium);
    let total = icw + 7.0 + tw;
    let x = area.cx() - total / 2.0;
    icons::draw(cv, Icon::ArrowDownCircleFill, pt(x + icw / 2.0, area.cy()), 14.0, Weight::Medium, Color::white(1.0));
    cv.text(text, &f, Color::white(1.0), rect(x + icw + 7.0, area.cy() - th / 2.0, tw + 2.0, th), Align::Leading, Trim::None);
}

/// Seconds since a repeating animation started (state entered).
fn since(app: &mut App, k: u64, active: bool) -> f32 {
    let now = util::mono() as f32;
    if !active {
        app.anims.snap(k, -1.0);
        return 0.0;
    }
    let a = app.anims.anim(k, -1.0);
    if a.v < 0.0 {
        a.snap(now);
    }
    now - app.anims.get(k).unwrap_or(now)
}

fn ease_in_out(t: f32) -> f32 {
    crate::anim::cubic_bezier(0.42, 0.0, 0.58, 1.0, t)
}

pub fn drag_over(cv: &mut Canvas, app: &mut App, lr: Rect) {
    let band = app.island.band();
    let r = rect(lr.x + 22.0, lr.y + band + 6.0, lr.w - 44.0, lr.h - band - 6.0 - 20.0);
    let active = app.island.state == State::DragOver;
    let busy = app.island.drop_busy;
    let t = since(app, key("dropt.since"), active);
    // breathe: easeInOut 0.9 s, autoreversing.
    let cycle = (t / 0.9) % 2.0;
    let b = if active { ease_in_out(if cycle < 1.0 { cycle } else { 2.0 - cycle }) } else { 0.0 };
    let phase = if active { -13.0 * ((t / 0.9) % 1.0) } else { 0.0 };
    let s = 0.985 + 0.015 * b;
    cv.save();
    cv.scale_about(s, r.cx(), r.cy());
    cv.fill_round(r, 18.0, Color::white(0.05 + 0.05 * b));
    if busy {
        cv.stroke_round(r, 18.0, 1.5, Color::white(0.85));
    } else {
        cv.stroke_round_dashed(r, 18.0, 1.5, (7.0, 6.0), phase, Color::white(0.85));
    }
    let title_f = Font::sans(12.0, Weight::Bold).kerning(2.6);
    let sub_f = Font::mono(11.0, Weight::Regular);
    let title = if busy { "SAVING" } else { "DROP TO VAULT" };
    let sub = if busy { "Storing a local copy…" } else { "Release to save" };
    let icon = if busy { Icon::ArrowTriangle2Circlepath } else { Icon::TrayAndArrowDownFill };
    let ih = icons::size_of(&cv.g, icon, 30.0, Weight::Light).1;
    let th = lh(cv, &title_f);
    let sh = lh(cv, &sub_f);
    let total = ih + 10.0 + th + 10.0 + sh;
    let y0 = r.cy() - total / 2.0;
    let offset = -2.0 + 5.0 * b;
    let rot_t = since(app, key("dropt.spin"), busy);
    cv.save();
    if busy {
        let deg = 360.0 * ((rot_t / 0.9) % 1.0);
        cv.transform(crate::gfx::rotate_about(deg, r.cx(), y0 + ih / 2.0 + offset));
    }
    icons::draw(cv, icon, pt(r.cx(), y0 + ih / 2.0 + offset), 30.0, Weight::Light, Color::white(1.0));
    cv.restore();
    cv.text(title, &title_f, Color::white(1.0), rect(r.x, y0 + ih + 10.0, r.w, th), Align::Center, Trim::None);
    cv.text(sub, &sub_f, Color::white(0.55), rect(r.x, y0 + ih + 20.0 + th, r.w, sh), Align::Center, Trim::None);
    cv.restore();
}

// ---------------------------------------------------------------------------
// Expanded tray
// ---------------------------------------------------------------------------

pub fn expanded(cv: &mut Canvas, app: &mut App, lr: Rect, hits_on: bool) {
    let header_h = app.island.band() + 8.0;
    let hits_before = app.island.hits.len();

    // Search bar presence (Motion.hover on searchOpen, move-from-top + opacity).
    let search_target = app.island.search_open || !app.island.search.text.is_empty();
    let ps = app.anims.drive(key("search.p"), if search_target { 1.0 } else { 0.0 }, motion::HOVER);
    let search_h = 31.0 * ps;

    // Now Playing presence (Motion.hover on track != nil, move-from-bottom).
    let np_target = app.np.track.is_some() && app.settings.music_controls;
    let pn = app.anims.drive(key("np.p"), if np_target { 1.0 } else { 0.0 }, motion::HOVER);
    let np_h = 44.0 * pn;

    let z = rect(lr.x, lr.y + header_h + search_h, lr.w, (lr.h - header_h - search_h - np_h).max(0.0));

    // ZStack(alignment: .bottom)
    zstack(cv, app, z);

    if ps > 0.002 {
        let y = lr.y + header_h - 31.0 * (1.0 - ps);
        cv.save();
        cv.opacity(ps.clamp(0.0, 1.0));
        search_bar(cv, app, rect(lr.x, y, lr.w, 31.0));
        cv.restore();
    } else {
        SEARCH_FIELD.with(|s| *s.borrow_mut() = None);
    }

    header(cv, app, rect(lr.x, lr.y, lr.w, header_h));

    if pn > 0.002 {
        let y = lr.b() - 44.0 + 44.0 * (1.0 - pn);
        cv.save();
        cv.opacity(pn.clamp(0.0, 1.0));
        crate::nowplaying::draw_bar(cv, app, rect(lr.x + 16.0, y, lr.w - 32.0, 34.0));
        cv.restore();
    }

    if !hits_on {
        app.island.hits.truncate(hits_before);
    }
}

fn zstack(cv: &mut Canvas, app: &mut App, z: Rect) {
    let tab = app.island.tab;
    let vault_op = app.anims.drive(key("tab.vault"), if tab == Tab::Vault { 1.0 } else { 0.0 }, motion::CONTENT_OUT);
    let clip_op = app.anims.drive(key("tab.clip"), if tab == Tab::Clipboard { 1.0 } else { 0.0 }, motion::CONTENT_OUT);
    let recent_tab = util::mono() - app.island.tab_changed_at < 0.2;
    let instant = if recent_tab { motion::CONTENT_OUT } else { Curve::Snap };

    if vault_op > 0.002 {
        cv.save();
        cv.opacity(vault_op.clamp(0.0, 1.0));
        crate::tray::render(cv, app, z, tab == Tab::Vault);
        cv.restore();
    }
    if clip_op > 0.002 {
        cv.save();
        cv.opacity(clip_op.clamp(0.0, 1.0));
        clip_grid(cv, app, z, tab == Tab::Clipboard);
        cv.restore();
    }
    let count = app.model.items.len();
    let empty = app.anims.drive(key("empty.p"), if tab == Tab::Vault && count == 0 { 1.0 } else { 0.0 }, instant);
    if empty > 0.002 {
        cv.save();
        cv.opacity(empty.clamp(0.0, 1.0));
        empty_tray(cv, app, z);
        cv.restore();
    }
    let no_match = tab == Tab::Vault && count > 0 && app.tray.visible.is_empty() && !app.island.search.text.is_empty();
    let nm = app.anims.drive(key("nomatch.p"), if no_match { 1.0 } else { 0.0 }, instant);
    if nm > 0.002 {
        cv.save();
        cv.opacity(nm.clamp(0.0, 1.0));
        no_matches(cv, z);
        cv.restore();
    }
    let sel = app.tray.selected.len();
    let bulk_curve = if recent_tab { motion::CONTENT_OUT } else { motion::OPEN };
    let pb = app.anims.drive(key("bulk.p"), if tab == Tab::Vault && sel >= 2 { 1.0 } else { 0.0 }, bulk_curve);
    if sel >= 2 {
        app.tray.bulk_count = sel;
    }
    if pb > 0.002 {
        let n = app.tray.bulk_count;
        bulk_bar(cv, app, z, n, pb);
    }
}

fn no_matches(cv: &mut Canvas, z: Rect) {
    let f = Font::sans(11.5, Weight::Medium);
    let ih = icons::size_of(&cv.g, Icon::MagnifyingGlass, 18.0, Weight::Light).1;
    let th = lh(cv, &f);
    let total = ih + 7.0 + th;
    let y0 = z.cy() - total / 2.0;
    icons::draw(cv, Icon::MagnifyingGlass, pt(z.cx(), y0 + ih / 2.0), 18.0, Weight::Light, Color::white(0.45));
    cv.text("No matches", &f, Color::white(0.45), rect(z.x, y0 + ih + 7.0, z.w, th), Align::Center, Trim::None);
}

fn empty_tray(cv: &mut Canvas, app: &mut App, z: Rect) {
    let r = rect(z.x + 16.0, z.y + 4.0, z.w - 32.0, z.h - 20.0);
    let active = app.island.state == State::Expanded;
    let t = since(app, key("empty.since"), active);
    let phase = if active { -12.0 * (t % 1.0) } else { 0.0 };
    cv.stroke_round_dashed(r, 16.0, 1.2, (6.0, 6.0), phase, Color::white(0.35));
    let title_f = Font::sans(10.5, Weight::Bold).kerning(2.2);
    let sub_f = Font::sans(11.0, Weight::Regular);
    let ih = icons::size_of(&cv.g, Icon::TrayAndArrowDownFill, 22.0, Weight::Light).1;
    let th = lh(cv, &title_f);
    let sh = lh(cv, &sub_f);
    let total = ih + 8.0 + th + 8.0 + sh;
    let y0 = r.cy() - total / 2.0;
    icons::draw(cv, Icon::TrayAndArrowDownFill, pt(r.cx(), y0 + ih / 2.0), 22.0, Weight::Light, Color::white(1.0));
    cv.text("DROP IMAGES HERE", &title_f, Color::white(1.0), rect(r.x, y0 + ih + 8.0, r.w, th), Align::Center, Trim::None);
    cv.text("Copied images & screenshots land here too", &sub_f, Color::white(0.45), rect(r.x, y0 + ih + 16.0 + th, r.w, sh), Align::Center, Trim::None);
}

// -- header -------------------------------------------------------------------

fn tab_button(cv: &mut Canvas, app: &mut App, x: f32, cy: f32, icon: Icon, title: &str, selected: bool, act: Act, tip: &str) -> f32 {
    let f = Font::sans(9.5, Weight::Bold).kerning(1.6);
    let (tw, th) = cv.measure(title, &f);
    let icw = img_w(cv, icon, 10.0, Weight::Semibold);
    let w = 9.0 + icw + 5.0 + tw + 9.0;
    let r = rect(x, cy - 11.0, w, 22.0);
    let id = key_id("tabbtn", title);
    hit(app, r, id, act, Some(tip));
    let hov = app.anims.drive(key_id("tabbtn.h", title), if hovered(app, id) { 1.0 } else { 0.0 }, motion::HOVER);
    let sel = app.anims.drive(key_id("tabbtn.s", title), if selected { 1.0 } else { 0.0 }, motion::HOVER);
    let bg_a = (0.08 * hov) * (1.0 - sel) + sel;
    cv.fill_capsule(r, Color::white(bg_a.clamp(0.0, 1.0)));
    let fg_unsel = Color::white(0.55 + 0.35 * hov);
    let fg = fg_unsel.lerp(Color::black(1.0), sel.clamp(0.0, 1.0));
    icons::draw(cv, icon, pt(x + 9.0 + icw / 2.0, cy), 10.0, Weight::Semibold, fg);
    cv.text(title, &f, fg, rect(x + 9.0 + icw + 5.0, cy - th / 2.0, tw + 1.0, th), Align::Leading, Trim::None);
    w
}

fn island_button(cv: &mut Canvas, app: &mut App, r: Rect, icon: Option<Icon>, name: &str, act: Act, tip: &str, glyph_filled: Option<bool>) {
    let id = key_id("ibtn", name);
    hit(app, r, id, act, Some(tip));
    let hov = app.anims.drive(key_id("ibtn.h", name), if hovered(app, id) { 1.0 } else { 0.0 }, motion::HOVER);
    let target = if pressed(app, id) { 0.86 } else if hovered(app, id) { 1.06 } else { 1.0 };
    let s = app.anims.drive(key_id("ibtn.s", name), target, motion::HOVER);
    let color = Color::white(0.6 + 0.4 * hov.clamp(0.0, 1.0));
    cv.save();
    cv.scale_about(s, r.cx(), r.cy());
    cv.fill_circle(r.center(), 13.0, Color::white(0.12 * hov.clamp(0.0, 1.0)));
    match icon {
        Some(i) => icons::draw(cv, i, r.center(), 12.5, Weight::Medium, color),
        None => {
            let filled = glyph_filled.unwrap_or(false);
            let fv = app.anims.drive(key("selectall.fill"), if filled { 1.0 } else { 0.0 }, motion::HOVER);
            for row in 0..2 {
                for col in 0..2 {
                    let sq = rect(r.cx() - 6.25 + col as f32 * 7.5, r.cy() - 6.25 + row as f32 * 7.5, 5.0, 5.0);
                    cv.fill_round(sq, 1.5, color.mul_alpha(fv.clamp(0.0, 1.0)));
                    cv.stroke_round_centered(sq, 1.5, 1.2, color);
                }
            }
        }
    }
    cv.restore();
}

fn count_label(app: &App) -> String {
    let q = &app.island.search.text;
    if app.island.tab == Tab::Clipboard {
        let total = app.clips.items.len();
        if q.is_empty() {
            return format!("{total}");
        }
        return format!("{}/{total}", app.clips.filtered(q).len());
    }
    let n = app.model.items.len();
    if q.is_empty() { format!("{n}") } else { format!("{}/{n}", app.tray.visible.len()) }
}

fn header(cv: &mut Canvas, app: &mut App, hr: Rect) {
    let cy = hr.cy();
    let x0 = hr.x + 22.0;
    // Tabs capsule.
    let f = Font::sans(9.5, Weight::Bold).kerning(1.6);
    let w1 = 9.0 + img_w(cv, Icon::PhotoStack, 10.0, Weight::Semibold) + 5.0 + cv.measure("VAULT", &f).0 + 9.0;
    let w2 = 9.0 + img_w(cv, Icon::DocOnClipboard, 10.0, Weight::Semibold) + 5.0 + cv.measure("CLIPBOARD", &f).0 + 9.0;
    let cap = rect(x0, cy - 13.0, 2.0 + w1 + 2.0 + w2 + 2.0, 26.0);
    cv.fill_capsule(cap, Color::white(0.07));
    let tab = app.island.tab;
    tab_button(cv, app, x0 + 2.0, cy, Icon::PhotoStack, "VAULT", tab == Tab::Vault, Act::TabVault, "Saved images");
    tab_button(cv, app, x0 + 2.0 + w1 + 2.0, cy, Icon::DocOnClipboard, "CLIPBOARD", tab == Tab::Clipboard, Act::TabClip, "Everything you copied");

    // Right buttons.
    let n = if tab == Tab::Vault { 4 } else { 3 };
    let total = n as f32 * 28.0 + (n - 1) as f32 * 2.0;
    let mut bx = hr.r() - 22.0 - total;
    let search_tip = if app.island.search_open { "Close search  Esc" } else { "Search  /" };
    island_button(cv, app, rect(bx, cy - 13.0, 28.0, 26.0), Some(Icon::MagnifyingGlass), "search", Act::Search, search_tip, None);
    bx += 30.0;
    if tab == Tab::Vault {
        island_button(cv, app, rect(bx, cy - 13.0, 28.0, 26.0), Some(Icon::Folder), "folder", Act::Reveal, "Show in Explorer", None);
        bx += 30.0;
        island_button(cv, app, rect(bx, cy - 13.0, 28.0, 26.0), Some(Icon::GearShape), "gear", Act::Menu, "Settings", None);
        bx += 30.0;
        let sel = app.tray.selected.len();
        let total_items = if app.island.search.text.is_empty() { app.model.items.len() } else { app.tray.visible.len() };
        let filled = sel > 0 && sel == total_items;
        island_button(cv, app, rect(bx, cy - 13.0, 28.0, 26.0), None, "selectall", Act::SelectAll, "Select all  Ctrl+A", Some(filled));
    } else {
        island_button(cv, app, rect(bx, cy - 13.0, 28.0, 26.0), Some(Icon::GearShape), "gear", Act::Menu, "Settings", None);
        bx += 30.0;
        let confirm = app.island.confirm_clear;
        let (icon, tip) = if confirm {
            (Icon::CheckmarkCircle, "Click again to clear the clipboard history")
        } else {
            (Icon::ArrowCounterclockwise, "Reset clipboard history")
        };
        island_button(cv, app, rect(bx, cy - 13.0, 28.0, 26.0), Some(icon), "reset", Act::ResetClips, tip, None);
    }

    // Message / count (max 190 wide).
    let mx = cap.r() + 8.0;
    let max_w = 190.0f32.min((hr.r() - 22.0 - total - 12.0 - mx).max(0.0));
    let msg_p = app.anims.drive(key("msg.p"), if app.island.message.is_some() { 1.0 } else { 0.0 }, motion::HOVER);
    let count_op = 1.0 - msg_p.clamp(0.0, 1.0);
    if count_op > 0.002 {
        let cf = Font::mono(10.0, Weight::Regular);
        let label = count_label(app);
        let (tw, th) = cv.measure(&label, &cf);
        let r = rect(mx, cy - (th + 2.0) / 2.0, (tw + 12.0).min(max_w), th + 2.0);
        cv.save();
        cv.opacity(count_op);
        cv.stroke_round_centered(r, 3.0, 1.0, Color::white(0.15));
        cv.text(&label, &cf, Color::white(0.5), rect(r.x + 6.0, r.y + 1.0, r.w - 12.0, th), Align::Leading, Trim::Tail);
        cv.restore();
        let bytes = app.model.total_bytes();
        let tip = if app.island.tab == Tab::Vault {
            format!("{} images · {}", app.model.items.len(), crate::theme::byte_count(bytes))
        } else {
            format!("{} copied items", app.clips.items.len())
        };
        if count_op > 0.5 {
            app.island.hits.push(Hit { rect: r, id: key("count"), tip: Some(tip), act: Act::Count, text: false });
        }
    }
    if msg_p > 0.002 {
        let mf = Font::sans(10.5, Weight::Medium);
        let text = app.island.message.clone().unwrap_or_else(|| app.island.message_last.clone());
        let th = lh(cv, &mf);
        let offset = if app.island.message_inserting && app.island.message.is_some() { (1.0 - msg_p) * th } else { 0.0 };
        cv.save();
        cv.opacity(msg_p.clamp(0.0, 1.0));
        cv.text(&text, &mf, Color::white(0.85), rect(mx, cy - th / 2.0 + offset, max_w, th), Align::Leading, Trim::Tail);
        cv.restore();
    }
}

// -- search bar ----------------------------------------------------------------

fn search_bar(cv: &mut Canvas, app: &mut App, r: Rect) {
    let bar = rect(r.x + 22.0, r.y, r.w - 44.0, 26.0);
    cv.fill_round(bar, 8.0, Color::white(0.06));
    let cy = bar.cy();
    let mut x = bar.x + 9.0;
    let icw = img_w(cv, Icon::MagnifyingGlass, 10.0, Weight::Medium);
    icons::draw(cv, Icon::MagnifyingGlass, pt(x + icw / 2.0, cy), 10.0, Weight::Medium, Color::white(0.45));
    x += icw + 7.0;
    let vault = app.island.tab == Tab::Vault;
    let has_query = !app.island.search.text.is_empty();

    // Right-side items, laid out from the trailing edge.
    let mut right = bar.r() - 9.0;
    if vault {
        let sort = app.tray.sort;
        let pf = Font::sans(9.5, Weight::Medium);
        let (tw, th) = cv.measure(sort.title(), &pf);
        let sw = img_w(cv, sort.icon(), 9.0, Weight::Semibold);
        let pw = 7.0 + sw + 3.0 + tw + 7.0;
        let pr = rect(right - pw, cy - 10.0, pw, 20.0);
        let tip = format!("Sort: {} — click to change", sort.title());
        hit(app, pr, key("sortpill"), Act::Sort, Some(&tip));
        cv.fill_capsule(pr, Color::white(0.07));
        icons::draw(cv, sort.icon(), pt(pr.x + 7.0 + sw / 2.0, cy), 9.0, Weight::Semibold, Color::white(0.62));
        cv.text(sort.title(), &pf, Color::white(0.62), rect(pr.x + 7.0 + sw + 3.0, cy - th / 2.0, tw + 1.0, th), Align::Leading, Trim::None);
        right = pr.x - 4.0 - 7.0;
    }
    if has_query {
        if vault {
            let cf = Font::mono(9.0, Weight::Regular);
            let label = format!("{} of {}", app.tray.visible.len(), app.model.items.len());
            let (tw, th) = cv.measure(&label, &cf);
            cv.text(&label, &cf, Color::white(0.38), rect(right - tw, cy - th / 2.0, tw + 1.0, th), Align::Leading, Trim::None);
            right -= tw + 7.0;
        }
        let xw = img_w(cv, Icon::XMarkCircleFill, 10.0, Weight::Regular);
        let xr = rect(right - xw - 2.0, cy - 8.0, xw + 4.0, 16.0);
        hit(app, xr, key("searchclear"), Act::SearchClear, Some("Clear  Esc"));
        icons::draw(cv, Icon::XMarkCircleFill, xr.center(), 10.0, Weight::Regular, Color::white(0.4));
        right = xr.x - 7.0 + 2.0;
    }
    let field = rect(x, bar.y, (right - x).max(10.0), bar.h);
    SEARCH_FIELD.with(|s| *s.borrow_mut() = Some(field));
    app.island.hits.push(Hit { rect: field, id: key("searchfield"), tip: None, act: Act::SearchField, text: true });
    let placeholder = if vault { "Search name, site, source or text in images" } else { "Search copied items" };
    let focused = app.island.search_focused && app.island.state == State::Expanded;
    let mut tf = std::mem::take(&mut app.island.search);
    tf.draw(cv, &SEARCH_FONT, Color::white(1.0), field, focused, placeholder, Color::white(0.25));
    app.island.search = tf;
}

// -- bulk bar --------------------------------------------------------------------

fn bulk_bar(cv: &mut Canvas, app: &mut App, z: Rect, count: usize, p: f32) {
    let label = format!("{count} selected");
    let lf = Font::sans(12.0, Weight::Medium);
    let pf = Font::sans(12.0, Weight::Semibold);
    let (lw, _) = cv.measure(&label, &lf);
    let pills: [(Option<&str>, Icon, Act, &str); 5] = [
        (Some("Copy"), Icon::DocOnDoc, Act::BulkCopy, "copy"),
        (Some("Text"), Icon::DocTextMagnifyingGlass, Act::BulkText, "text"),
        (Some("Reveal"), Icon::Folder, Act::BulkReveal, "reveal"),
        (Some("Delete"), Icon::Trash, Act::BulkDelete, "delete"),
        (None, Icon::XMark, Act::BulkClear, "clear"),
    ];
    let mut widths = Vec::new();
    for (t, i, _, _) in pills.iter() {
        let icw = img_w(cv, *i, 10.5, Weight::Semibold);
        let w = match t {
            Some(t) => 11.0 + icw + 5.0 + cv.measure(t, &pf).0 + 11.0,
            None => 8.0 + icw + 8.0,
        };
        widths.push(w);
    }
    let inner = (lw + 20.0) + 4.0 + widths.iter().sum::<f32>() + 4.0 * (widths.len() - 1) as f32;
    let w = inner + 8.0;
    let h = 34.0;
    let offset = (1.0 - p) * (h + 12.0);
    let r = rect(z.cx() - w / 2.0, z.b() - 12.0 - h + offset, w, h);
    cv.save();
    cv.opacity(p.clamp(0.0, 1.0));
    let rr = r;
    cv.shadowed(8.0, pt(0.0, 8.0), Color::black(0.6), true, move |cv| cv.fill_capsule(rr, Color::white(1.0)));
    let cy = r.cy();
    cv.text(&label, &lf, Color::black(1.0), rect(r.x + 4.0 + 10.0, cy - 9.0, lw + 2.0, 18.0), Align::Leading, Trim::None);
    let mut x = r.x + 4.0 + lw + 20.0 + 4.0;
    for (i, (t, icon, act, name)) in pills.iter().enumerate() {
        let pw = widths[i];
        let pr = rect(x, cy - 13.0, pw, 26.0);
        let id = key_id("bulk", name);
        if p > 0.9 {
            hit(app, pr, id, act.clone(), None);
        }
        let hov = app.anims.drive(key_id("bulk.h", name), if hovered(app, id) { 1.0 } else { 0.0 }, motion::HOVER);
        let s = app.anims.drive(key_id("bulk.s", name), if pressed(app, id) { 0.9 } else { 1.0 }, motion::HOVER);
        cv.save();
        cv.scale_about(s, pr.cx(), pr.cy());
        cv.fill_capsule(pr, Color::black(hov.clamp(0.0, 1.0)));
        let fg = Color::black(1.0).lerp(Color::white(1.0), hov.clamp(0.0, 1.0));
        let icw = img_w(cv, *icon, 10.5, Weight::Semibold);
        let lx = pr.x + if t.is_some() { 11.0 } else { 8.0 };
        icons::draw(cv, *icon, pt(lx + icw / 2.0, cy), 10.5, Weight::Semibold, fg);
        if let Some(t) = t {
            let (tw, th) = cv.measure(t, &pf);
            cv.text(t, &pf, fg, rect(lx + icw + 5.0, cy - th / 2.0, tw + 1.0, th), Align::Leading, Trim::None);
        }
        cv.restore();
        x += pw + 4.0;
    }
    cv.restore();
}

// ---------------------------------------------------------------------------
// Clipboard grid
// ---------------------------------------------------------------------------

fn clip_grid(cv: &mut Canvas, app: &mut App, z: Rect, active: bool) {
    let query = app.island.search.text.clone();
    if app.clips.items.is_empty() {
        let title_f = Font::sans(10.5, Weight::Bold).kerning(2.2);
        let sub_f = Font::sans(11.0, Weight::Regular);
        let ih = icons::size_of(&cv.g, Icon::DocOnClipboard, 22.0, Weight::Light).1;
        let th = lh(cv, &title_f);
        let sh = lh(cv, &sub_f);
        let total = ih + 8.0 + th + 8.0 + sh;
        let y0 = z.cy() - total / 2.0;
        icons::draw(cv, Icon::DocOnClipboard, pt(z.cx(), y0 + ih / 2.0), 22.0, Weight::Light, Color::white(1.0));
        cv.text("NOTHING COPIED YET", &title_f, Color::white(1.0), rect(z.x, y0 + ih + 8.0, z.w, th), Align::Center, Trim::None);
        let sub = if app.settings.clipboard_history {
            "Text and files you copy show up here · images go to the vault"
        } else {
            "Clipboard History is off in Settings"
        };
        cv.text(sub, &sub_f, Color::white(0.45), rect(z.x, y0 + ih + 16.0 + th, z.w, sh), Align::Center, Trim::None);
        return;
    }
    let items: Vec<cliphistory::ClipItem> = app.clips.filtered(&query).into_iter().cloned().collect();
    if items.is_empty() {
        no_matches(cv, z);
        return;
    }
    let cols = 3usize;
    let gap = 8.0;
    let cw = ((z.w - 32.0 - gap * 2.0) / 3.0).max(10.0);
    let ch = 70.0;
    let rows = items.len().div_ceil(cols);
    let content_h = 4.0 + rows as f32 * ch + (rows.saturating_sub(1)) as f32 * gap + 16.0;
    let scroll = CLIP.with(|c| {
        let mut c = c.borrow_mut();
        c.content_h = content_h;
        c.view_h = z.h;
        c.scroll = c.scroll.clamp(0.0, (content_h - z.h).max(0.0));
        c.scroll
    });
    cv.clip(z);
    let now = util::mono();
    for (i, item) in items.iter().enumerate() {
        let (r, c) = (i / cols, i % cols);
        let cell = rect(z.x + 16.0 + c as f32 * (cw + gap), z.y + 4.0 + r as f32 * (ch + gap) - scroll, cw, ch);
        if cell.b() < z.y - 4.0 || cell.y > z.b() + 4.0 {
            continue;
        }
        clip_card(cv, app, item, cell, z, active, now);
    }
    cv.unclip();
}

fn clip_card(cv: &mut Canvas, app: &mut App, item: &cliphistory::ClipItem, r: Rect, z: Rect, active: bool, now: f64) {
    let id = key_id("clip", &item.id);
    let visible_rect = r.intersection(&z);
    if active && !visible_rect.is_empty() {
        let tip = match item.kind {
            cliphistory::Kind::Text => item.text.as_deref().unwrap_or("").chars().take(400).collect::<String>(),
            _ => item.preview(),
        };
        app.island.hits.push(Hit { rect: visible_rect, id, tip: Some(tip), act: Act::ClipCard(item.id.clone()), text: false });
    }
    let hov_b = hovered(app, id);
    let copied = CLIP.with(|c| c.borrow().copied.get(&item.id).map(|t| now < *t).unwrap_or(false));
    let hov = app.anims.drive(key_id("clip.h", &item.id), if hov_b { 1.0 } else { 0.0 }, motion::HOVER);
    let cp = app.anims.drive(key_id("clip.c", &item.id), if copied { 1.0 } else { 0.0 }, motion::HOVER);
    let badge = app.anims.drive(key_id("clip.b", &item.id), if copied || hov_b { 1.0 } else { 0.0 }, motion::HOVER);
    let s = 1.0 + 0.02 * hov;
    cv.save();
    cv.scale_about(s, r.cx(), r.cy());
    cv.fill_round(r, 10.0, Color::white(0.055 + 0.045 * hov.clamp(0.0, 1.0)));
    // Content, clipped to the card.
    let geom = cv.g.squircle(r.w, r.h, 10.0);
    let gm: windows::Win32::Graphics::Direct2D::ID2D1Geometry = windows::core::Interface::cast(&geom).unwrap();
    cv.save();
    cv.translate(r.x, r.y);
    cv.push_mask(&gm);
    cv.restore();
    let body = Font::sans(10.5, Weight::Regular);
    match item.kind {
        cliphistory::Kind::Text => {
            let text = item.preview().trim().to_string();
            cv.text_wrapped(&text, &body, Color::white(0.88), rect(r.x + 9.0, r.y + 8.0, r.w - 18.0, r.h - 24.0), 3, Align::Leading);
        }
        cliphistory::Kind::File => {
            let icw = img_w(cv, Icon::Doc, 11.0, Weight::Regular);
            let lhh = lh(cv, &body);
            icons::draw(cv, Icon::Doc, pt(r.x + 9.0 + icw / 2.0, r.y + 8.0 + lhh / 2.0), 11.0, Weight::Regular, Color::white(0.6));
            let f = Font::sans(10.5, Weight::Medium);
            cv.text_wrapped(&item.preview(), &f, Color::white(0.88), rect(r.x + 9.0 + icw + 6.0, r.y + 8.0, r.w - 24.0 - icw, r.h - 24.0), 3, Align::Leading);
        }
        cliphistory::Kind::Image => {}
    }
    // Footer.
    let ff = Font::sans(8.5, Weight::Medium);
    let af = Font::mono(8.5, Weight::Regular);
    let fh = lh(cv, &ff).max(lh(cv, &af));
    let fy = r.b() - 5.0 - fh;
    let icon = match item.kind {
        cliphistory::Kind::Text => Icon::TextAlignLeft,
        cliphistory::Kind::Image => Icon::Photo,
        cliphistory::Kind::File => Icon::Doc,
    };
    let fc = Color::white(0.5);
    let icw = img_w(cv, icon, 7.5, Weight::Semibold);
    icons::draw(cv, icon, pt(r.x + 9.0 + icw / 2.0, fy + fh / 2.0), 7.5, Weight::Semibold, fc);
    let age = cliphistory::age(item.date);
    let (aw, _) = cv.measure(&age, &af);
    cv.text(&age, &af, fc, rect(r.r() - 9.0 - aw, fy, aw + 1.0, fh), Align::Leading, Trim::None);
    let app_name = item.app.clone().unwrap_or_else(|| if item.kind == cliphistory::Kind::Image { item.preview() } else { String::new() });
    let nx = r.x + 9.0 + icw + 4.0;
    cv.text(&app_name, &ff, fc, rect(nx, fy, (r.r() - 9.0 - aw - 4.0 - nx).max(1.0), fh), Align::Leading, Trim::Tail);
    cv.pop_layer();
    // Border.
    let bw = 0.5 + 0.7 * cp.clamp(0.0, 1.0);
    let ba = (0.1 + 0.18 * hov.clamp(0.0, 1.0)) * (1.0 - cp.clamp(0.0, 1.0)) + 0.9 * cp.clamp(0.0, 1.0);
    cv.stroke_round(r, 10.0, bw, Color::white(ba));
    // Copy badge (top trailing): scale 0.6 + opacity transition.
    if badge > 0.002 {
        let b = rect(r.r() - 5.0 - 18.0, r.y + 5.0, 18.0, 18.0);
        let bs = 0.6 + 0.4 * badge;
        cv.save();
        cv.scale_about(bs, b.cx(), b.cy());
        cv.opacity(badge.clamp(0.0, 1.0));
        let bg = Color::black(0.7).lerp(Color::white(1.0), cp.clamp(0.0, 1.0));
        cv.fill_circle(b.center(), 9.0, bg);
        let (icon, color) = if copied { (Icon::Checkmark, Color::black(1.0)) } else { (Icon::DocOnDoc, Color::white(1.0)) };
        icons::draw(cv, icon, b.center(), 8.5, Weight::Bold, color);
        cv.restore();
    }
    cv.restore();
}

pub fn clip_wheel(app: &mut App, delta: f32) {
    CLIP.with(|c| {
        let mut c = c.borrow_mut();
        let max = (c.content_h - c.view_h).max(0.0);
        c.scroll = (c.scroll - delta / 120.0 * 60.0).clamp(0.0, max);
    });
    island::invalidate(app);
}

pub fn clip_click(app: &mut App, id: &str) {
    app.clips.copy(id);
    island::flash(app, "Copied — Ctrl+V anywhere");
    CLIP.with(|c| {
        c.borrow_mut().copied.insert(id.to_string(), util::mono() + 1.1);
    });
    crate::ui::after(1.15, || {
        crate::app::with(island::invalidate);
    });
    island::invalidate(app);
}

pub fn clip_press(id: &str, p: Pt) {
    CLIP.with(|c| {
        let mut c = c.borrow_mut();
        c.drag_from = Some((id.to_string(), p));
        c.dragged = false;
    });
}

pub fn clip_dragged() -> bool {
    CLIP.with(|c| c.borrow().dragged)
}

pub fn clip_drag_end() {
    CLIP.with(|c| {
        let mut c = c.borrow_mut();
        c.drag_from = None;
        c.dragged = false;
    });
}

/// Pointer moved with the button down on a card: start a real drag.
pub fn clip_drag(app: &mut App, id: &str, p: Pt) {
    let start = CLIP.with(|c| {
        let mut c = c.borrow_mut();
        match &c.drag_from {
            Some((cid, sp)) if cid == id => {
                if !c.dragged && p.dist(*sp) > 4.0 {
                    c.dragged = true;
                    true
                } else {
                    false
                }
            }
            _ => {
                c.drag_from = Some((id.to_string(), p));
                false
            }
        }
    });
    if !start {
        return;
    }
    let Some(item) = app.clips.items.iter().find(|i| i.id == id).cloned() else { return };
    app.island.pressed = None;
    app.tray.dragging_out = true;
    crate::ui::defer(move || {
        let obj = match item.kind {
            cliphistory::Kind::Text => crate::dragdrop::text_object(item.text.as_deref().unwrap_or("")),
            cliphistory::Kind::File => {
                let paths: Vec<std::path::PathBuf> =
                    item.text.as_deref().unwrap_or("").split('\n').filter(|s| !s.is_empty()).map(std::path::PathBuf::from).collect();
                crate::dragdrop::files_object(&paths)
            }
            cliphistory::Kind::Image => None,
        };
        if let Some(obj) = obj {
            crate::dragdrop::run(&obj);
        }
        crate::app::with(|a| {
            a.tray.dragging_out = false;
            a.island.mouse_down_inside = false;
            clip_drag_end();
            island::pointer_moved(a);
        });
    });
}

pub fn clip_menu(app: &mut App, id: String) {
    let _ = app;
    crate::ui::defer(move || {
        let mut m = crate::menu::Menu::new();
        let id2 = id.clone();
        m.action("Copy", move || {
            crate::app::with(|a| clip_click(a, &id2));
        });
        m.sep();
        m.action("Delete", move || {
            crate::app::with(|a| {
                a.clips.delete(&id);
                island::invalidate(a);
            });
        });
        crate::app::popup_menu(m, None);
    });
}

pub fn clip_tip_scroll_reset() {
    CLIP.with(|c| c.borrow_mut().scroll = 0.0);
}
