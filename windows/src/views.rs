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
    svg_known: HashMap<String, bool>,
    svg_bmp: HashMap<String, (u32, windows::Win32::Graphics::Direct2D::ID2D1Bitmap1)>,
    svg_pending: std::collections::HashSet<String>,
    expanded: Option<Expanded>,
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

/// IslandTabButton: an icon-only 30×22 capsule; the selected one sits on the
/// white disc that slides between the tabs (drawn by `header`).
fn tab_button(cv: &mut Canvas, app: &mut App, r: Rect, icon: Icon, name: &str, selected: bool, act: Act, tip: &str) {
    let id = key_id("tabbtn", name);
    hit(app, r, id, act, Some(tip));
    let hov_b = hovered(app, id) && !selected;
    let hov = app.anims.drive(key_id("tabbtn.h", name), if hov_b { 1.0 } else { 0.0 }, motion::HOVER);
    let sel = app.anims.drive(key_id("tabbtn.s", name), if selected { 1.0 } else { 0.0 }, motion::HOVER);
    let s = 1.0 + 0.06 * hov;
    cv.save();
    cv.scale_about(s, r.cx(), r.cy());
    cv.fill_capsule(r, Color::white(0.1 * hov.clamp(0.0, 1.0)));
    let fg_unsel = Color::white(0.55 + 0.4 * hov.clamp(0.0, 1.0));
    let fg = fg_unsel.lerp(Color::black(1.0), sel.clamp(0.0, 1.0));
    icons::draw(cv, icon, r.center(), 11.0, Weight::Semibold, fg);
    cv.restore();
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
            // SelectAllGlyph: dashed rounded square → filled square with a black check.
            let filled = glyph_filled.unwrap_or(false);
            let fv = app.anims.drive(key("selectall.fill"), if filled { 1.0 } else { 0.0 }, motion::HOVER).clamp(0.0, 1.0);
            let sq = r.center();
            let box_r = rect(sq.x - 6.5, sq.y - 6.5, 13.0, 13.0);
            if fv < 0.5 {
                cv.stroke_round_dashed(box_r.inset(-0.65, -0.65), 3.5, 1.3, (2.4, 2.1), 0.0, color);
            } else {
                cv.stroke_round_centered(box_r, 3.5, 1.3, color);
            }
            if fv > 0.002 {
                let k = 0.2 + 0.8 * fv;
                cv.fill_round(rect(sq.x - 6.5 * k, sq.y - 6.5 * k, 13.0 * k, 13.0 * k), 3.5 * k, color.mul_alpha(fv));
            }
            icons::check(cv, rect(sq.x - 3.5, sq.y - 2.75, 7.0, 5.5), 1.0, 1.5, color.lerp(Color::black(1.0), fv));
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
    if q.is_empty() { format!("{n}") } else { format!("{}/{n}", crate::tray::image_ids(app).len()) }
}

fn header(cv: &mut Canvas, app: &mut App, hr: Rect) {
    let cy = hr.cy();
    let x0 = hr.x + 22.0;
    // Tabs capsule: two icon-only buttons with a sliding white disc.
    let cap = rect(x0, cy - 13.0, 2.0 + 30.0 + 2.0 + 30.0 + 2.0, 26.0);
    cv.fill_capsule(cap, Color::white(0.07));
    let tab = app.island.tab;
    let slot = |i: f32| rect(x0 + 2.0 + i * 32.0, cy - 11.0, 30.0, 22.0);
    let disc_x = app.anims.drive(key("tab.disc"), if tab == Tab::Vault { 0.0 } else { 1.0 }, motion::HOVER);
    cv.fill_capsule(slot(disc_x), Color::white(1.0));
    tab_button(cv, app, slot(0.0), Icon::PhotoStack, "vault", tab == Tab::Vault, Act::TabVault, "Vault — saved images");
    tab_button(cv, app, slot(1.0), Icon::DocOnClipboard, "clipboard", tab == Tab::Clipboard, Act::TabClip, "Clipboard — everything you copied");

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
        let total_items = crate::tray::image_ids(app).len();
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
            let label = format!("{} of {}", crate::tray::image_ids(app).len(), app.model.items.len());
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
        (Some("Group"), Icon::FolderBadgePlus, Act::BulkGroup, "group"),
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
// Copied overlay (CopiedOverlay / TileView.showCopied)
// ---------------------------------------------------------------------------

/// How long the overlay lives: in 0.16 s, held until 1.0 s, out over 0.3 s.
pub const COPIED_SECS: f32 = 1.3;

/// Start the overlay for `k`; it repaints itself while the clock runs.
pub fn start_copied(app: &mut App, k: u64) {
    app.anims.play(k, 0.0, COPIED_SECS, Curve::linear(COPIED_SECS));
    app.request_frame();
}

/// Seconds since `start_copied(k)`, while the overlay is showing.
pub fn copied_t(app: &App, k: u64) -> Option<f32> {
    app.anims.get(k).filter(|t| *t > 0.0 && *t < COPIED_SECS)
}

/// Dark veil, a white disc that springs in (0.3 → 1), a check that draws
/// itself and the label rising in. Draw it clipped to the card.
pub fn copied_overlay(cv: &mut Canvas, r: Rect, t: f32, label: &str) {
    let ease = |x: f32| crate::anim::cubic_bezier(0.22, 1.0, 0.36, 1.0, x);
    let veil = if t < 1.0 { (t / 0.16).min(1.0) } else { 1.0 - ease((t - 1.0) / 0.3) };
    if veil <= 0.002 {
        return;
    }
    cv.save();
    cv.opacity(veil.clamp(0.0, 1.0));
    cv.fill_rect(r, Color::black(0.62));
    // CASpringAnimation(mass 0.7, stiffness 300, damping 11) from 0.3 to 1.
    let (w0, z) = ((300.0f32 / 0.7).sqrt(), 11.0 / (2.0 * (300.0f32 * 0.7).sqrt()));
    let wd = w0 * (1.0 - z * z).sqrt();
    let s = 1.0 - 0.7 * (-z * w0 * t).exp() * ((wd * t).cos() + (z * w0 / wd) * (wd * t).sin());
    let small = r.h < 52.0 || label.is_empty();
    let lf = Font::sans(9.5, Weight::Semibold);
    let lhh = if small { 0.0 } else { lh(cv, &lf) };
    let total = 24.0 + if small { 0.0 } else { 5.0 + lhh };
    let disc = pt(r.cx(), r.cy() - total / 2.0 + 12.0);
    cv.save();
    cv.scale_about(s, disc.x, disc.y);
    cv.fill_circle(disc, 12.0, Color::white(1.0));
    let draw = ease(((t - 0.08) / 0.26).clamp(0.0, 1.0));
    icons::check(cv, rect(disc.x - 5.0, disc.y - 4.0, 10.0, 8.0), draw, 2.0, Color::black(1.0));
    cv.restore();
    if !small {
        let rise = ((t - 0.12) / 0.2).clamp(0.0, 1.0);
        let y = disc.y + 12.0 + 5.0 + 4.0 * (1.0 - rise);
        cv.save();
        cv.opacity(rise);
        cv.text(label, &lf, Color::white(1.0), rect(r.x, y, r.w, lhh), Align::Center, Trim::None);
        cv.restore();
    }
    cv.restore();
}

// ---------------------------------------------------------------------------
// Clipboard grid
// ---------------------------------------------------------------------------

/// A clipboard card grown over the grid (ExpandedClip).
struct Expanded {
    id: String,
    closing: bool,
    scroll: f32,
    content_h: f32,
    view_h: f32,
    /// Selection in UTF-16 units: (anchor, caret).
    sel: (u32, u32),
    /// Where the text is laid out (view space) and how wide.
    text_origin: Pt,
    text_w: f32,
}

const EXPAND: Curve = Curve::spring(0.44, 0.84);

fn is_svg_clip(c: &mut ClipState, item: &cliphistory::ClipItem) -> bool {
    if item.kind != cliphistory::Kind::Text {
        return false;
    }
    *c.svg_known.entry(item.id.clone()).or_insert_with(|| crate::imaging::is_svg_text(item.text.as_deref().unwrap_or("")))
}

/// Rendered preview of copied SVG code (SVGPreview.image), made off-thread.
fn svg_bitmap(item: &cliphistory::ClipItem) -> Option<windows::Win32::Graphics::Direct2D::ID2D1Bitmap1> {
    let generation = crate::gfx::generation();
    let hit = CLIP.with(|c| {
        let c = c.borrow();
        c.svg_bmp.get(&item.id).filter(|(g, _)| *g == generation).map(|(_, b)| b.clone())
    });
    if hit.is_some() {
        return hit;
    }
    let start = CLIP.with(|c| c.borrow_mut().svg_pending.insert(item.id.clone()));
    if start {
        let id = item.id.clone();
        let data = item.text.clone().unwrap_or_default().into_bytes();
        crate::thumbs::pool(move || {
            crate::imaging::com_init();
            let raw = crate::imaging::svg_raster(&data, 384, 384);
            crate::ui::post(move || {
                let bmp = raw.and_then(|r| crate::gfx::gfx().bitmap(r.w, r.h, &r.px));
                CLIP.with(|c| {
                    let mut c = c.borrow_mut();
                    if let Some(b) = bmp {
                        c.svg_bmp.insert(id.clone(), (crate::gfx::generation(), b));
                        c.svg_pending.remove(&id);
                    }
                    // A failed render stays pending, so it isn't retried every frame.
                });
                crate::app::with(island::invalidate);
            });
        });
    }
    None
}

/// A temporary .svg file so the card drags into Explorer, Figma, browsers.
fn svg_file(item: &cliphistory::ClipItem) -> Option<std::path::PathBuf> {
    let text = item.text.as_deref()?;
    let dir = std::env::temp_dir().join("ImageVault-svg");
    std::fs::create_dir_all(&dir).ok()?;
    let tail: String = item.id.chars().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect();
    let path = dir.join(format!("Copied SVG {tail}.svg"));
    std::fs::write(&path, text).ok()?;
    Some(path)
}

fn checkerboard(cv: &mut Canvas, r: Rect) {
    let s: f32 = 6.0;
    let mut y = r.y;
    let mut row = 0;
    while y < r.b() {
        let mut x = r.x;
        let mut col = 0;
        while x < r.r() {
            if (row + col) % 2 == 0 {
                cv.fill_rect(rect(x, y, s.min(r.r() - x), s.min(r.b() - y)), Color::white(0.06));
            }
            x += s;
            col += 1;
        }
        y += s;
        row += 1;
    }
}

fn round_mask(cv: &mut Canvas, r: Rect, rad: f32) {
    let geom = cv.g.squircle(r.w, r.h, rad);
    let gm: windows::Win32::Graphics::Direct2D::ID2D1Geometry = windows::core::Interface::cast(&geom).unwrap();
    cv.save();
    cv.translate(r.x, r.y);
    cv.push_mask(&gm);
    cv.restore();
}

/// CardButton: a small round button (expand, copy, close) that inverts on hover.
fn card_button(cv: &mut Canvas, app: &mut App, r: Rect, icon: Icon, name: &str, act: Act, tip: &str, hits: bool, alpha: f32) {
    let id = key_id("cardbtn", name);
    if hits {
        app.island.hits.push(Hit { rect: r, id, tip: Some(tip.to_string()), act, text: false });
    }
    let hov = app.anims.drive(key_id("cardbtn.h", name), if hovered(app, id) { 1.0 } else { 0.0 }, motion::HOVER).clamp(0.0, 1.0);
    let s = app.anims.drive(key_id("cardbtn.s", name), if pressed(app, id) { 0.9 } else { 1.0 }, motion::HOVER);
    cv.save();
    cv.scale_about(s, r.cx(), r.cy());
    cv.opacity(alpha.clamp(0.0, 1.0));
    cv.fill_circle(r.center(), r.w / 2.0, Color::black(0.7).lerp(Color::white(1.0), hov));
    cv.stroke_circle(r.center(), r.w / 2.0 - 0.25, 0.5, Color::white(0.25));
    icons::draw(cv, icon, r.center(), r.w * 0.45, Weight::Bold, Color::white(1.0).lerp(Color::black(1.0), hov));
    cv.restore();
}

fn clip_grid(cv: &mut Canvas, app: &mut App, z: Rect, active: bool) {
    let query = app.island.search.text.clone();
    if app.clips.items.is_empty() {
        CLIP.with(|c| c.borrow_mut().expanded = None);
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
        CLIP.with(|c| c.borrow_mut().expanded = None);
        no_matches(cv, z);
        return;
    }
    // Expansion progress (spring), and whether the grid is interactive.
    let exp = CLIP.with(|c| c.borrow().expanded.as_ref().map(|e| (e.id.clone(), e.closing)));
    if let Some((id, _)) = &exp {
        if !items.iter().any(|i| &i.id == id) {
            CLIP.with(|c| c.borrow_mut().expanded = None);
        }
    }
    let exp = CLIP.with(|c| c.borrow().expanded.as_ref().map(|e| (e.id.clone(), e.closing)));
    let p = app.anims.drive(key("clip.exp"), if matches!(exp, Some((_, false))) { 1.0 } else { 0.0 }, EXPAND);
    if let Some((_, true)) = exp {
        if p <= 0.01 {
            CLIP.with(|c| c.borrow_mut().expanded = None);
        }
    }
    let exp_id = CLIP.with(|c| c.borrow().expanded.as_ref().map(|e| e.id.clone()));
    let grid_hits = active && exp_id.is_none();

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
    let grid_op = (1.0 - p).clamp(0.0, 1.0);
    let mut slot_of = None;
    cv.clip(z);
    cv.save();
    cv.opacity(grid_op);
    let now = util::mono();
    for (i, item) in items.iter().enumerate() {
        let (r, c) = (i / cols, i % cols);
        let cell = rect(z.x + 16.0 + c as f32 * (cw + gap), z.y + 4.0 + r as f32 * (ch + gap) - scroll, cw, ch);
        if exp_id.as_deref() == Some(item.id.as_str()) {
            slot_of = Some(cell);
            continue;
        }
        if cell.b() < z.y - 4.0 || cell.y > z.b() + 4.0 || grid_op <= 0.002 {
            continue;
        }
        clip_card(cv, app, item, cell, z, grid_hits, now);
    }
    cv.restore();
    cv.unclip();
    if let Some(id) = exp_id {
        if let Some(item) = items.iter().find(|i| i.id == id) {
            let from = slot_of.unwrap_or(rect(z.cx() - cw / 2.0, z.y + 4.0, cw, ch));
            let to = rect(z.x + 16.0, z.y + 4.0, z.w - 32.0, (z.h - 16.0).max(ch));
            let k = p.max(0.0);
            let lerp = |a: f32, b: f32| a + (b - a) * k;
            let f = rect(lerp(from.x, to.x), lerp(from.y, to.y), lerp(from.w, to.w), lerp(from.h, to.h));
            expanded_clip(cv, app, item, f, active);
        }
    }
}

fn clip_card(cv: &mut Canvas, app: &mut App, item: &cliphistory::ClipItem, r: Rect, z: Rect, active: bool, now: f64) {
    let id = key_id("clip", &item.id);
    let svg = CLIP.with(|c| is_svg_clip(&mut c.borrow_mut(), item));
    let visible_rect = r.intersection(&z);
    if active && !visible_rect.is_empty() {
        let tip = match item.kind {
            cliphistory::Kind::Text if svg => "SVG code · click to copy".to_string(),
            cliphistory::Kind::Text => item.text.as_deref().unwrap_or("").chars().take(400).collect::<String>(),
            _ => item.preview(),
        };
        app.island.hits.push(Hit { rect: visible_rect, id, tip: Some(tip), act: Act::ClipCard(item.id.clone()), text: false });
    }
    let copied = CLIP.with(|c| c.borrow().copied.get(&item.id).map(|t| now < *t).unwrap_or(false));
    // The card counts as hovered while the pointer is on it or on its buttons.
    let on_card = app.island.pointer.map(|p| visible_rect.contains(p)).unwrap_or(false) && active;
    let hov_b = on_card && !copied;
    let hov = app.anims.drive(key_id("clip.h", &item.id), if hov_b { 1.0 } else { 0.0 }, motion::HOVER);
    let cp = app.anims.drive(key_id("clip.c", &item.id), if copied { 1.0 } else { 0.0 }, motion::HOVER);
    let btn = app.anims.drive(key_id("clip.b", &item.id), if hov_b { 1.0 } else { 0.0 }, motion::HOVER);
    let s = 1.0 + 0.02 * hov;
    cv.save();
    cv.scale_about(s, r.cx(), r.cy());
    cv.fill_round(r, 10.0, Color::white(0.055 + 0.045 * hov.clamp(0.0, 1.0)));
    // Content, clipped to the card.
    round_mask(cv, r, 10.0);
    let body = Font::sans(10.5, Weight::Regular);
    match item.kind {
        cliphistory::Kind::Text if svg => {
            checkerboard(cv, r);
            if let Some(b) = svg_bitmap(item) {
                cv.image_fit(&b, rect(r.x + 10.0, r.y + 8.0, r.w - 20.0, r.h - 28.0), 1.0, true);
            }
        }
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
    if svg || item.kind == cliphistory::Kind::Image {
        let shade = rect(r.x, fy - 5.0, r.w, fh + 10.0);
        cv.fill_gradient_v(shade, &[(0.0, Color::CLEAR), (1.0, Color::black(0.85))], None);
    }
    let fc = Color::white(0.5);
    let lead_w = if svg {
        let bf = Font::sans(7.0, Weight::Heavy);
        let (tw, th) = cv.measure("SVG", &bf);
        let br = rect(r.x + 9.0, fy + fh / 2.0 - (th + 2.0) / 2.0, tw + 6.0, th + 2.0);
        cv.fill_rrect(br, 2.0, Color::white(0.9));
        cv.text("SVG", &bf, Color::black(1.0), br, Align::Center, Trim::None);
        br.w
    } else {
        let icon = match item.kind {
            cliphistory::Kind::Text => Icon::TextAlignLeft,
            cliphistory::Kind::Image => Icon::Photo,
            cliphistory::Kind::File => Icon::Doc,
        };
        let icw = img_w(cv, icon, 7.5, Weight::Semibold);
        icons::draw(cv, icon, pt(r.x + 9.0 + icw / 2.0, fy + fh / 2.0), 7.5, Weight::Semibold, fc);
        icw
    };
    let age = cliphistory::age(item.date);
    let (aw, _) = cv.measure(&age, &af);
    cv.text(&age, &af, fc, rect(r.r() - 9.0 - aw, fy, aw + 1.0, fh), Align::Leading, Trim::None);
    let app_name = item.app.clone().unwrap_or_else(|| if item.kind == cliphistory::Kind::Image { item.preview() } else { String::new() });
    let nx = r.x + 9.0 + lead_w + 4.0;
    cv.text(&app_name, &ff, fc, rect(nx, fy, (r.r() - 9.0 - aw - 4.0 - nx).max(1.0), fh), Align::Leading, Trim::Tail);
    if let Some(t) = copied_t(app, key_id("clip.copied", &item.id)) {
        copied_overlay(cv, r, t, "Copied");
    }
    cv.pop_layer();
    // Border.
    let bw = 0.5 + 0.7 * cp.clamp(0.0, 1.0);
    let ba = (0.1 + 0.18 * hov.clamp(0.0, 1.0)) * (1.0 - cp.clamp(0.0, 1.0)) + 0.9 * cp.clamp(0.0, 1.0);
    cv.stroke_round(r, 10.0, bw, Color::white(ba));
    cv.restore();
    // Hover buttons (top trailing): expand (text) and copy; scale 0.6 + fade.
    if btn > 0.002 {
        let bs = 0.6 + 0.4 * btn;
        let anchor = pt(r.r() - 5.0, r.y + 5.0);
        cv.save();
        cv.scale_about(bs, anchor.x, anchor.y);
        let copy_r = rect(r.r() - 5.0 - 18.0, r.y + 5.0, 18.0, 18.0);
        let tip = if svg { "Copy SVG code" } else { "Copy" };
        let hits = active && btn > 0.5;
        card_button(cv, app, copy_r, Icon::DocOnDoc, &format!("copy.{}", item.id), Act::ClipCopy(item.id.clone()), tip, hits, btn);
        if item.kind == cliphistory::Kind::Text {
            let exp_r = rect(copy_r.x - 4.0 - 18.0, copy_r.y, 18.0, 18.0);
            card_button(cv, app, exp_r, Icon::ArrowsOutward, &format!("exp.{}", item.id), Act::ClipExpand(item.id.clone()), "Expand", hits, btn);
        }
        cv.restore();
    }
}

fn expanded_clip(cv: &mut Canvas, app: &mut App, item: &cliphistory::ClipItem, f: Rect, active: bool) {
    let closing = CLIP.with(|c| c.borrow().expanded.as_ref().map(|e| e.closing).unwrap_or(true));
    let shown = app.anims.drive(key("clip.exp.c"), if closing { 0.0 } else { 1.0 }, if closing { motion::CONTENT_OUT } else { motion::CONTENT_IN });
    let hits = active && !closing;
    let svg = CLIP.with(|c| is_svg_clip(&mut c.borrow_mut(), item));
    let ff = f;
    cv.shadowed(18.0, pt(0.0, 8.0), Color::black(0.6), true, move |cv| cv.fill_round(ff, 14.0, Color::gray(0.09, 1.0)));
    if hits {
        // Swallow clicks on the panel's background.
        app.island.hits.push(Hit { rect: f, id: key("clip.exp.bg"), tip: None, act: Act::None, text: false });
    }
    round_mask(cv, f, 14.0);
    // Header row.
    let hy = f.y + 8.0;
    let cy = hy + 11.0;
    let hf = Font::sans(10.0, Weight::Medium);
    let mf = Font::mono(9.0, Weight::Regular);
    let icon = if svg { Icon::Code } else { Icon::TextAlignLeft };
    let icw = img_w(cv, icon, 9.0, Weight::Semibold);
    icons::draw(cv, icon, pt(f.x + 12.0 + icw / 2.0, cy), 9.0, Weight::Semibold, Color::white(0.7));
    let name = item.app.clone().unwrap_or_else(|| if svg { "SVG".into() } else { "Text".into() });
    let text = item.text.clone().unwrap_or_default();
    let meta = format!("· {} · {} characters", cliphistory::age(item.date), text.chars().count());
    let close_r = rect(f.r() - 12.0 - 22.0, cy - 11.0, 22.0, 22.0);
    let copy_r = rect(close_r.x - 6.0 - 22.0, cy - 11.0, 22.0, 22.0);
    let nx = f.x + 12.0 + icw + 6.0;
    let room = (copy_r.x - 6.0 - nx).max(0.0);
    let (nw, nh) = cv.measure(&name, &hf);
    let nw = nw.min(room);
    cv.text(&name, &hf, Color::white(0.7), rect(nx, cy - nh / 2.0, nw + 1.0, nh), Align::Leading, Trim::Tail);
    let mx = nx + nw + 6.0;
    let mh = lh(cv, &mf);
    cv.text(&meta, &mf, Color::white(0.4), rect(mx, cy - mh / 2.0, (copy_r.x - 6.0 - mx).max(0.0), mh), Align::Leading, Trim::Tail);
    card_button(cv, app, copy_r, Icon::DocOnDoc, "exp.copy", Act::ClipExpCopy, "Copy", hits, 1.0);
    card_button(cv, app, close_r, Icon::ArrowsInward, "exp.close", Act::ClipExpClose, "Close  Esc", hits, 1.0);
    // Body: the whole text (selectable, scrolls), or an SVG's preview beside its code.
    let body = rect(f.x + 12.0, hy + 22.0 + 8.0, f.w - 24.0, (f.b() - 12.0 - (hy + 30.0)).max(0.0));
    cv.save();
    cv.opacity(shown.clamp(0.0, 1.0));
    let code_r = if svg {
        let half = ((body.w - 8.0) / 2.0).max(0.0);
        let pr = rect(body.x, body.y, half, body.h);
        cv.fill_round(pr, 8.0, Color::white(0.04));
        round_mask(cv, pr, 8.0);
        checkerboard(cv, pr);
        if let Some(b) = svg_bitmap(item) {
            cv.image_fit(&b, pr.inset(12.0, 12.0), 1.0, true);
        }
        cv.pop_layer();
        rect(pr.r() + 8.0, body.y, half, body.h)
    } else {
        body
    };
    let font = if svg { Font::mono(10.0, Weight::Regular) } else { Font::sans(11.5, Weight::Regular) };
    let layout = cv.g.layout(&text, &font, code_r.w.max(1.0), 100000.0, true, Align::Leading, Trim::None);
    let mut m = windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_METRICS::default();
    unsafe {
        let _ = layout.GetMetrics(&mut m);
    }
    let (scroll, sel) = CLIP.with(|c| {
        let mut c = c.borrow_mut();
        let Some(e) = c.expanded.as_mut() else { return (0.0, (0, 0)) };
        e.content_h = m.height;
        e.view_h = code_r.h;
        e.scroll = e.scroll.clamp(0.0, (m.height - code_r.h).max(0.0));
        e.text_origin = pt(code_r.x, code_r.y - e.scroll);
        e.text_w = code_r.w;
        (e.scroll, e.sel)
    });
    if hits {
        app.island.hits.push(Hit { rect: code_r, id: key("clip.exp.text"), tip: None, act: Act::ClipExpText, text: true });
    }
    cv.clip(code_r);
    let origin = pt(code_r.x, code_r.y - scroll);
    let (lo, hi) = (sel.0.min(sel.1), sel.0.max(sel.1));
    if hi > lo {
        for hr in selection_rects(&layout, lo, hi - lo) {
            cv.fill_rect(hr.offset(origin.x, origin.y), Color::white(0.25));
        }
    }
    cv.text_layout(&layout, origin, Color::white(0.9));
    // Thin scroller while the text overflows.
    if m.height > code_r.h + 0.5 {
        let kh = (code_r.h * code_r.h / m.height).max(18.0);
        let ky = code_r.y + (code_r.h - kh) * (scroll / (m.height - code_r.h));
        cv.fill_capsule(rect(code_r.r() - 4.0, ky, 3.0, kh), Color::white(0.3));
    }
    cv.unclip();
    cv.restore();
    if let Some(t) = copied_t(app, key("clip.exp.copied")) {
        copied_overlay(cv, f, t, "Copied");
    }
    cv.pop_layer();
    cv.stroke_round(f, 14.0, 0.5, Color::white(0.2));
}

fn selection_rects(layout: &windows::Win32::Graphics::DirectWrite::IDWriteTextLayout, start: u32, len: u32) -> Vec<Rect> {
    use windows::Win32::Graphics::DirectWrite::DWRITE_HIT_TEST_METRICS;
    let mut out = Vec::new();
    unsafe {
        let mut n = 0u32;
        let _ = layout.HitTestTextRange(start, len, 0.0, 0.0, None, &mut n);
        if n == 0 {
            return out;
        }
        let mut buf = vec![DWRITE_HIT_TEST_METRICS::default(); n as usize];
        if layout.HitTestTextRange(start, len, 0.0, 0.0, Some(&mut buf), &mut n).is_ok() {
            for h in buf.iter().take(n as usize) {
                out.push(rect(h.left, h.top, h.width.max(2.0), h.height));
            }
        }
    }
    out
}

/// Text position (UTF-16) under a view-space point in the expanded text.
fn text_pos_at(app: &App, item_text: &str, p: Pt) -> Option<u32> {
    let (origin, w, svg) = CLIP.with(|c| {
        let c = c.borrow();
        c.expanded.as_ref().map(|e| (e.text_origin, e.text_w, c.svg_known.get(&e.id).copied().unwrap_or(false)))
    })?;
    let _ = app;
    let font = if svg { Font::mono(10.0, Weight::Regular) } else { Font::sans(11.5, Weight::Regular) };
    let layout = crate::gfx::gfx().layout(item_text, &font, w.max(1.0), 100000.0, true, Align::Leading, Trim::None);
    unsafe {
        let mut trailing = windows::core::BOOL(0);
        let mut inside = windows::core::BOOL(0);
        let mut m = windows::Win32::Graphics::DirectWrite::DWRITE_HIT_TEST_METRICS::default();
        layout.HitTestPoint(p.x - origin.x, p.y - origin.y, &mut trailing, &mut inside, &mut m).ok()?;
        Some(m.textPosition + if trailing.as_bool() { m.length } else { 0 })
    }
}

fn expanded_item(app: &App) -> Option<cliphistory::ClipItem> {
    let id = CLIP.with(|c| c.borrow().expanded.as_ref().map(|e| e.id.clone()))?;
    app.clips.items.iter().find(|i| i.id == id).cloned()
}

pub fn clip_expanded() -> bool {
    CLIP.with(|c| c.borrow().expanded.as_ref().map(|e| !e.closing).unwrap_or(false))
}

pub fn clip_expand(app: &mut App, id: &str) {
    CLIP.with(|c| {
        c.borrow_mut().expanded = Some(Expanded {
            id: id.to_string(),
            closing: false,
            scroll: 0.0,
            content_h: 0.0,
            view_h: 0.0,
            sel: (0, 0),
            text_origin: pt(0.0, 0.0),
            text_w: 1.0,
        })
    });
    app.anims.snap(key("clip.exp"), 0.0);
    app.anims.snap(key("clip.exp.c"), 0.0);
    island::invalidate(app);
}

pub fn clip_collapse(app: &mut App) {
    CLIP.with(|c| {
        if let Some(e) = c.borrow_mut().expanded.as_mut() {
            e.closing = true;
        }
    });
    island::invalidate(app);
}

/// Search changed: the expanded card just goes away.
pub fn clip_collapse_now() {
    CLIP.with(|c| c.borrow_mut().expanded = None);
}

pub fn clip_exp_copy(app: &mut App) {
    let Some(item) = expanded_item(app) else { return };
    app.clips.copy(&item.id);
    island::flash(app, "Copied — Ctrl+V anywhere");
    start_copied(app, key("clip.exp.copied"));
}

/// Mouse down in the expanded text: place the caret / extend / word / all.
pub fn clip_exp_press(app: &mut App, p: Pt, clicks: u32, shift: bool) {
    let Some(item) = expanded_item(app) else { return };
    let text = item.text.unwrap_or_default();
    let Some(pos) = text_pos_at(app, &text, p) else { return };
    let w: Vec<u16> = text.encode_utf16().collect();
    CLIP.with(|c| {
        let mut c = c.borrow_mut();
        let Some(e) = c.expanded.as_mut() else { return };
        if clicks >= 3 {
            e.sel = (0, w.len() as u32);
        } else if clicks == 2 {
            let is_word = |u: u16| char::from_u32(u as u32).map(|ch| ch.is_alphanumeric() || ch == '_').unwrap_or(true);
            let mut a = pos as usize;
            let mut b = pos as usize;
            while a > 0 && is_word(w[a - 1]) {
                a -= 1;
            }
            while b < w.len() && is_word(w[b]) {
                b += 1;
            }
            e.sel = (a as u32, b as u32);
        } else if shift {
            e.sel.1 = pos;
        } else {
            e.sel = (pos, pos);
        }
    });
    island::invalidate(app);
}

pub fn clip_exp_drag(app: &mut App, p: Pt) {
    let Some(item) = expanded_item(app) else { return };
    let text = item.text.unwrap_or_default();
    // Dragging past the top or bottom edge scrolls.
    CLIP.with(|c| {
        let mut c = c.borrow_mut();
        if let Some(e) = c.expanded.as_mut() {
            let top = e.text_origin.y + e.scroll;
            let max = (e.content_h - e.view_h).max(0.0);
            if p.y < top {
                e.scroll = (e.scroll - (top - p.y).min(30.0) * 0.5).clamp(0.0, max);
            } else if p.y > top + e.view_h {
                e.scroll = (e.scroll + (p.y - top - e.view_h).min(30.0) * 0.5).clamp(0.0, max);
            }
        }
    });
    if let Some(pos) = text_pos_at(app, &text, p) {
        CLIP.with(|c| {
            if let Some(e) = c.borrow_mut().expanded.as_mut() {
                e.sel.1 = pos;
            }
        });
    }
    island::invalidate(app);
}

/// Keys while a card is expanded: Esc closes, Ctrl+C copies the selection
/// (or the whole text), Ctrl+A selects everything.
pub fn clip_exp_key(app: &mut App, vk: u16, ctrl: bool) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
    if !clip_expanded() {
        return false;
    }
    if vk == VK_ESCAPE.0 {
        clip_collapse(app);
        return true;
    }
    if ctrl && vk == b'A' as u16 {
        if let Some(item) = expanded_item(app) {
            let n = item.text.unwrap_or_default().encode_utf16().count() as u32;
            CLIP.with(|c| {
                if let Some(e) = c.borrow_mut().expanded.as_mut() {
                    e.sel = (0, n);
                }
            });
            island::invalidate(app);
        }
        return true;
    }
    if ctrl && vk == b'C' as u16 {
        let Some(item) = expanded_item(app) else { return true };
        let (a, b) = CLIP.with(|c| c.borrow().expanded.as_ref().map(|e| (e.sel.0.min(e.sel.1), e.sel.0.max(e.sel.1))).unwrap_or((0, 0)));
        if b > a {
            let w: Vec<u16> = item.text.unwrap_or_default().encode_utf16().collect();
            let part = String::from_utf16_lossy(&w[(a as usize).min(w.len())..(b as usize).min(w.len())]);
            crate::clipboard::copy_text(&part);
            island::flash(app, "Copied selection");
        } else {
            clip_exp_copy(app);
        }
        return true;
    }
    false
}

pub fn clip_wheel(app: &mut App, delta: f32) {
    let handled = CLIP.with(|c| {
        let mut c = c.borrow_mut();
        match c.expanded.as_mut() {
            Some(e) if !e.closing => {
                let max = (e.content_h - e.view_h).max(0.0);
                e.scroll = (e.scroll - delta / 120.0 * 48.0).clamp(0.0, max);
                true
            }
            _ => false,
        }
    });
    if !handled {
        CLIP.with(|c| {
            let mut c = c.borrow_mut();
            let max = (c.content_h - c.view_h).max(0.0);
            c.scroll = (c.scroll - delta / 120.0 * 60.0).clamp(0.0, max);
        });
    }
    island::invalidate(app);
}

pub fn clip_click(app: &mut App, id: &str) {
    app.clips.copy(id);
    island::flash(app, "Copied — Ctrl+V anywhere");
    CLIP.with(|c| {
        c.borrow_mut().copied.insert(id.to_string(), util::mono() + 1.1);
    });
    start_copied(app, key_id("clip.copied", id));
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
    let svg = CLIP.with(|c| is_svg_clip(&mut c.borrow_mut(), &item));
    app.island.pressed = None;
    app.tray.dragging_out = true;
    crate::ui::defer(move || {
        let obj = match item.kind {
            // Copied SVG code drags out as a real .svg file.
            cliphistory::Kind::Text if svg => svg_file(&item).and_then(|f| crate::dragdrop::files_object(&[f])),
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

fn save_svg(app: &mut App, item: &cliphistory::ClipItem) {
    let Some(text) = item.text.clone() else { return };
    let _ = app;
    let cand = crate::ingest::Candidate { data: text.into_bytes(), name: Some("Copied SVG".into()), url: None };
    crate::ingest::save_async(vec![cand], "clipboard", |summary| {
        crate::app::with(|a| {
            let msg = if !summary.added.is_empty() {
                "SVG saved to vault"
            } else if summary.is_empty() {
                "Couldn't save the SVG"
            } else {
                "Already in the vault"
            };
            island::flash(a, msg);
            crate::app::reload_model_if_changed(a);
        });
    });
}

pub fn clip_menu(app: &mut App, id: String) {
    let Some(item) = app.clips.items.iter().find(|i| i.id == id).cloned() else { return };
    let svg = CLIP.with(|c| is_svg_clip(&mut c.borrow_mut(), &item));
    crate::ui::defer(move || {
        let mut m = crate::menu::Menu::new();
        let id2 = id.clone();
        m.action(if svg { "Copy SVG Code" } else { "Copy" }, move || {
            crate::app::with(|a| clip_click(a, &id2));
        });
        if item.kind == cliphistory::Kind::Text {
            let id3 = id.clone();
            m.action("Expand", move || {
                crate::app::with(|a| clip_expand(a, &id3));
            });
        }
        if svg {
            let it = item.clone();
            m.action("Save SVG to Vault", move || {
                crate::app::with(|a| save_svg(a, &it));
            });
        }
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
