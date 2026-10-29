//! The image grid inside the expanded island (TrayController + TileView +
//! PreviewView). Eight columns, tiles sized to fit with equal margins;
//! file-manager selection (click, Shift range, Ctrl toggle, lasso, Ctrl+A,
//! arrows); every tile drags out as a real file; Ctrl+C copies real images.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use windows::Win32::Graphics::Direct2D::{ID2D1Bitmap1, ID2D1Geometry};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::core::Interface;

use crate::anim::{Curve, key, key_id, motion};
use crate::app::App;
use crate::gfx::{Align, Canvas, Trim};
use crate::icons::{self, Icon};
use crate::island::{self, Act, Hit, Sort, State};
use crate::theme::{self, Color, Font, Pt, Rect, Weight, pt, rect};
use crate::thumbs::{Kind, Thumbs};
use crate::vault::{self, VaultItem};
use crate::{clipboard, imaging, ui, util};

pub const COLUMNS: usize = 8;
const GAP: f32 = 8.0;
const INSET_TOP: f32 = 4.0;
const INSET_BOTTOM: f32 = 16.0;

#[derive(Debug, Clone)]
struct Down {
    tile: Option<String>,
    p: Pt,
    ctrl: bool,
    shift: bool,
    /// Plain click on an already-selected tile: select only it on mouse-up
    /// unless a drag starts (NSCollectionView behaviour).
    deferred: bool,
    dragged: bool,
}

#[derive(Debug, Clone)]
struct Lasso {
    start: Pt,
    cur: Pt,
    base: HashSet<String>,
    ctrl: bool,
    shift: bool,
}

struct Ghost {
    id: String,
    rect: Rect,
    since: f64,
    bmp: Option<ID2D1Bitmap1>,
}

pub struct Preview {
    pub open: bool,
    items: Vec<String>,
    index: usize,
    thumb: Option<ID2D1Bitmap1>,
    full: Option<ID2D1Bitmap1>,
    token: u64,
    closing: bool,
}

pub struct Tray {
    pub sort: Sort,
    pub query: String,
    pub visible: Vec<String>,
    index_of: HashMap<String, usize>,
    pub selected: HashSet<String>,
    anchor: Option<usize>,
    lead: Option<usize>,
    lasso: Option<Lasso>,
    lasso_set: Option<HashSet<String>>,
    pub dragging_out: bool,
    pub bulk_count: usize,
    pub scroll: f32,
    scroll_target: f32,
    content_h: f32,
    pub view: Rect,
    side: f32,
    margin: f32,
    hover: Option<String>,
    down: Option<Down>,
    copied: HashMap<String, f64>,
    played_fresh: HashSet<String>,
    inserted: HashSet<String>,
    ghosts: Vec<Ghost>,
    last_rects: HashMap<String, Rect>,
    scroller_until: f64,
    scroller_drag: Option<(f32, f32)>,
    pub preview: Preview,
    model_version: u64,
    checker: Option<(u32, windows::Win32::Graphics::Direct2D::ID2D1BitmapBrush)>,
    shown_once: bool,
}

impl Tray {
    pub fn new() -> Tray {
        Tray {
            sort: Sort::Newest,
            query: String::new(),
            visible: Vec::new(),
            index_of: HashMap::new(),
            selected: HashSet::new(),
            anchor: None,
            lead: None,
            lasso: None,
            lasso_set: None,
            dragging_out: false,
            bulk_count: 0,
            scroll: 0.0,
            scroll_target: 0.0,
            content_h: 0.0,
            view: Rect::default(),
            side: 64.0,
            margin: 16.0,
            hover: None,
            down: None,
            copied: HashMap::new(),
            played_fresh: HashSet::new(),
            inserted: HashSet::new(),
            ghosts: Vec::new(),
            last_rects: HashMap::new(),
            scroller_until: 0.0,
            scroller_drag: None,
            preview: Preview { open: false, items: Vec::new(), index: 0, thumb: None, full: None, token: 0, closing: false },
            model_version: u64::MAX,
            checker: None,
            shown_once: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Data & selection
// ---------------------------------------------------------------------------

fn compare_names(a: &str, b: &str) -> std::cmp::Ordering {
    use windows::Win32::Globalization::*;
    let wa: Vec<u16> = a.encode_utf16().collect();
    let wb: Vec<u16> = b.encode_utf16().collect();
    let r = unsafe { CompareStringEx(windows::core::PCWSTR::null(), LINGUISTIC_IGNORECASE | NORM_IGNORECASE, &wa, &wb, None, None, None) };
    match r {
        r if r.0 == 1 => std::cmp::Ordering::Less,
        r if r.0 == 3 => std::cmp::Ordering::Greater,
        r if r.0 == 2 => std::cmp::Ordering::Equal,
        _ => a.to_lowercase().cmp(&b.to_lowercase()),
    }
}

/// Filtered + sorted view of the vault (all selection actions use this).
pub fn visible_items(app: &App) -> Vec<String> {
    let needle = app.tray.query.trim().to_lowercase();
    let mut items: Vec<&VaultItem> = app.model.items.iter().collect();
    if !needle.is_empty() {
        items.retain(|i| {
            let fields = [
                Some(i.name.as_str()),
                i.domain.as_deref(),
                i.source.as_deref(),
                i.page_title.as_deref(),
                i.url.as_deref(),
                i.page_url.as_deref(),
                i.ocr_text.as_deref(),
            ];
            fields.iter().any(|f| f.unwrap_or("").to_lowercase().contains(&needle))
        });
    }
    match app.tray.sort {
        Sort::Newest => items.sort_by(|a, b| b.timestamp.partial_cmp(&a.timestamp).unwrap_or(std::cmp::Ordering::Equal)),
        Sort::Oldest => items.sort_by(|a, b| a.timestamp.partial_cmp(&b.timestamp).unwrap_or(std::cmp::Ordering::Equal)),
        Sort::Name => items.sort_by(|a, b| {
            let r = compare_names(&a.name, &b.name);
            if r == std::cmp::Ordering::Equal {
                b.timestamp.partial_cmp(&a.timestamp).unwrap_or(std::cmp::Ordering::Equal)
            } else {
                r
            }
        }),
        Sort::Size => items.sort_by(|a, b| b.bytes.unwrap_or(0).cmp(&a.bytes.unwrap_or(0))),
    }
    items.into_iter().map(|i| i.id.clone()).collect()
}

/// applySnapshot: refresh the visible list, keep the selection, animate.
pub fn apply(app: &mut App) {
    let next = visible_items(app);
    let old: HashSet<String> = app.tray.visible.iter().cloned().collect();
    let new: HashSet<String> = next.iter().cloned().collect();
    let animate = app.tray.shown_once && app.island.state == State::Expanded;
    if animate {
        let now = util::mono();
        for id in app.tray.visible.iter() {
            if !new.contains(id) {
                if let Some(r) = app.tray.last_rects.get(id).copied() {
                    let bmp = app.thumbs.peek(id, thumb_px(app), Kind::Tile);
                    app.tray.ghosts.push(Ghost { id: id.clone(), rect: r, since: now, bmp });
                }
            }
        }
        app.tray.inserted = next.iter().filter(|id| !old.contains(*id)).cloned().collect();
    } else {
        app.tray.inserted.clear();
    }
    app.tray.visible = next;
    app.tray.index_of = app.tray.visible.iter().enumerate().map(|(i, id)| (id.clone(), i)).collect();
    let vis = &app.tray.index_of;
    app.tray.selected.retain(|id| vis.contains_key(id));
    if let Some(a) = app.tray.anchor {
        if a >= app.tray.visible.len() {
            app.tray.anchor = None;
        }
    }
    if app.tray.preview.open {
        app.tray.preview.items.retain(|id| vis.contains_key(id));
        if app.tray.preview.items.is_empty() {
            close_preview(app);
        } else if app.tray.preview.index >= app.tray.preview.items.len() {
            app.tray.preview.index = app.tray.preview.items.len() - 1;
            load_preview(app);
        }
    }
    island::invalidate(app);
}

pub fn set_filter(app: &mut App, query: &str, sort: Sort) {
    if query == app.tray.query && sort == app.tray.sort {
        return;
    }
    app.tray.query = query.to_string();
    app.tray.sort = sort;
    apply(app);
}

pub fn model_changed(app: &mut App) {
    if app.model.version != app.tray.model_version {
        app.tray.model_version = app.model.version;
        apply(app);
    }
}

pub fn selected_items(app: &App) -> Vec<VaultItem> {
    // Visible order.
    app.tray.visible.iter().filter(|id| app.tray.selected.contains(*id)).filter_map(|id| app.model.get(id).cloned()).collect()
}

fn items_for(app: &App, ids: &[String]) -> Vec<VaultItem> {
    ids.iter().filter_map(|id| app.model.get(id).cloned()).collect()
}

pub fn toggle_select_all(app: &mut App) {
    let total = app.tray.visible.len();
    if total > 0 && app.tray.selected.len() == total {
        app.tray.selected.clear();
    } else {
        app.tray.selected = app.tray.visible.iter().cloned().collect();
    }
    island::invalidate(app);
}

pub fn clear_selection(app: &mut App) {
    app.tray.selected.clear();
    island::invalidate(app);
}

fn toggle_one(app: &mut App, id: &str) {
    if !app.tray.selected.remove(id) {
        app.tray.selected.insert(id.to_string());
        pop(app, "tile.badge.pop", id, 0.6);
    }
    app.tray.anchor = app.tray.index_of.get(id).copied();
    island::invalidate(app);
}

fn select_only(app: &mut App, id: &str) {
    let was = app.tray.selected.contains(id);
    app.tray.selected.clear();
    app.tray.selected.insert(id.to_string());
    if !was {
        pop(app, "tile.badge.pop", id, 0.6);
    }
    app.tray.anchor = app.tray.index_of.get(id).copied();
    island::invalidate(app);
}

fn pop(app: &mut App, what: &str, id: &str, from: f32) {
    app.anims.play(key_id(what, id), from, 1.0, Curve::POP);
    app.request_frame();
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

pub fn copy(app: &mut App, items: Vec<VaultItem>) {
    if items.is_empty() {
        return;
    }
    let n = items.len();
    util::spawn("copy", move || {
        imaging::com_init();
        clipboard::copy_items(&items);
    });
    island::flash(app, &if n == 1 { "Copied — Ctrl+V anywhere".to_string() } else { format!("{n} copied — Ctrl+V anywhere") });
}

pub fn copy_selection(app: &mut App) {
    let items = selected_items(app);
    for i in &items {
        show_copied(app, &i.id);
    }
    copy(app, items);
}

fn show_copied(app: &mut App, id: &str) {
    app.tray.copied.insert(id.to_string(), util::mono() + 1.4);
    pop(app, "tile.copy.pop", id, 0.7);
    app.anims.play(key_id("tile.ringflash", id), 3.0, 0.0, Curve::mac(0.5));
    ui::after(1.45, || {
        crate::app::with(island::invalidate);
    });
}

pub fn reveal_selection(app: &mut App) {
    let paths: Vec<PathBuf> = selected_items(app).iter().map(vault::file_path).collect();
    ui::defer(move || util::reveal(&paths));
}

pub fn delete_selection(app: &mut App) {
    let items = selected_items(app);
    if items.is_empty() {
        return;
    }
    let ids: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
    crate::app::delete_items(app, &ids);
    island::flash(app, &format!("{} deleted — Ctrl+Z to undo", items.len()));
}

pub fn copy_text_selection(app: &mut App) {
    let items = selected_items(app);
    copy_text(app, items);
}

/// OCR in the background; copies one combined block (first 10 images).
pub fn copy_text(app: &mut App, items: Vec<VaultItem>) {
    if items.is_empty() {
        return;
    }
    let total = items.len();
    let targets: Vec<VaultItem> = items.into_iter().take(10).collect();
    let n = targets.len();
    island::flash(app, &if n == 1 { "Reading text…".to_string() } else { format!("Reading {n} images…") });
    let langs = app.settings.ocr_extra_languages.clone();
    util::spawn("copy-text", move || {
        imaging::com_init();
        let mut chunks = Vec::new();
        for item in &targets {
            if let Some(text) = crate::ocr::recognize(&vault::file_path(item), &langs) {
                vault::set_ocr(&item.id, Some(text.clone()), None, None);
                chunks.push(text);
            }
        }
        let text = chunks.join("\n\n");
        ui::post(move || {
            crate::app::with(|a| {
                if text.is_empty() {
                    island::flash(a, "No text found");
                } else {
                    clipboard::copy_text(&text);
                    let capped = if total > n { format!(" (first {n})") } else { String::new() };
                    island::flash(a, &format!("Text copied · {} characters{capped}", text.chars().count()));
                }
                crate::app::reload_model_if_changed(a);
            });
        });
    });
}

fn copy_paths(app: &mut App, items: &[VaultItem]) {
    let paths: Vec<String> = items.iter().map(|i| vault::file_path(i).display().to_string()).collect();
    if paths.is_empty() {
        return;
    }
    clipboard::copy_text(&paths.join("\r\n"));
    island::flash(app, &if paths.len() == 1 { "Path copied".to_string() } else { format!("{} paths copied", paths.len()) });
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

fn thumb_px(app: &App) -> u32 {
    let px = (app.tray.side * app.island.scale).ceil() as u32;
    px.div_ceil(32) * 32
}

fn layout(app: &mut App, w: f32) {
    let cols = COLUMNS as f32;
    let side = (2.0f32).max(((w - 32.0 - (cols - 1.0) * GAP) / cols).floor());
    let used = cols * side + (cols - 1.0) * GAP;
    let margin = (12.0f32).max(((w - used) / 2.0).floor());
    app.tray.side = side;
    app.tray.margin = margin;
    let rows = app.tray.visible.len().div_ceil(COLUMNS);
    app.tray.content_h = if rows == 0 { INSET_TOP + INSET_BOTTOM } else { INSET_TOP + rows as f32 * side + (rows - 1) as f32 * GAP + INSET_BOTTOM };
}

/// Tile rect in content space.
fn slot(app: &App, i: usize) -> Rect {
    let (r, c) = (i / COLUMNS, i % COLUMNS);
    let s = app.tray.side;
    rect(app.tray.margin + c as f32 * (s + GAP), INSET_TOP + r as f32 * (s + GAP), s, s)
}

fn max_scroll(app: &App) -> f32 {
    (app.tray.content_h - app.tray.view.h).max(0.0)
}

fn to_view(app: &App, r: Rect) -> Rect {
    r.offset(app.tray.view.x, app.tray.view.y - app.tray.scroll)
}

fn to_content(app: &App, p: Pt) -> Pt {
    pt(p.x - app.tray.view.x, p.y - app.tray.view.y + app.tray.scroll)
}

fn tile_at(app: &App, p: Pt) -> Option<(usize, String)> {
    if !app.tray.view.contains(p) {
        return None;
    }
    let c = to_content(app, p);
    let s = app.tray.side;
    let col = ((c.x - app.tray.margin) / (s + GAP)).floor();
    let row = ((c.y - INSET_TOP) / (s + GAP)).floor();
    if col < 0.0 || row < 0.0 || col >= COLUMNS as f32 {
        return None;
    }
    let i = row as usize * COLUMNS + col as usize;
    if i >= app.tray.visible.len() {
        return None;
    }
    if slot(app, i).contains(c) { Some((i, app.tray.visible[i].clone())) } else { None }
}

fn ensure_visible(app: &mut App, i: usize) {
    let r = slot(app, i);
    let top = app.tray.scroll;
    let bottom = top + app.tray.view.h;
    let target = if r.y - INSET_TOP < top {
        r.y - INSET_TOP
    } else if r.b() + INSET_BOTTOM > bottom {
        r.b() + INSET_BOTTOM - app.tray.view.h
    } else {
        return;
    };
    scroll_to(app, target.clamp(0.0, max_scroll(app)), true);
}

fn scroll_to(app: &mut App, y: f32, animated: bool) {
    let y = y.clamp(0.0, max_scroll(app));
    app.tray.scroll_target = y;
    if animated {
        app.anims.drive(key("tray.scroll"), y, Curve::mac(0.25));
    } else {
        app.anims.snap(key("tray.scroll"), y);
        app.tray.scroll = y;
    }
    app.tray.scroller_until = util::mono() + 0.9;
    ui::after(1.25, || {
        crate::app::with(island::invalidate);
    });
    island::invalidate(app);
}

pub fn wheel(app: &mut App, delta: f32) {
    if app.island.tab == crate::island::Tab::Clipboard {
        crate::views::clip_wheel(app, delta);
        return;
    }
    if app.tray.preview.open {
        return;
    }
    // Notched wheels glide; touchpads (small deltas) track the finger.
    let step = delta / 120.0 * 72.0;
    let base = if delta.abs() >= 120.0 { app.tray.scroll_target } else { app.tray.scroll };
    let target = (base - step).clamp(0.0, max_scroll(app));
    scroll_to(app, target, delta.abs() >= 120.0);
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

pub fn render(cv: &mut Canvas, app: &mut App, z: Rect, active: bool) {
    model_changed(app);
    app.tray.view = z;
    app.tray.shown_once = true;
    layout(app, z.w);
    // Scrolling animation.
    let target = app.tray.scroll_target.clamp(0.0, max_scroll(app));
    app.tray.scroll_target = target;
    let s = app.anims.drive(key("tray.scroll"), target, Curve::mac(0.25));
    app.tray.scroll = s.clamp(0.0, max_scroll(app).max(0.0));
    if active {
        app.island.hits.push(Hit { rect: z, id: key("grid"), tip: None, act: Act::Grid, text: false });
    }
    if app.tray.checker.as_ref().map(|(g, _)| *g != cv.g.generation).unwrap_or(true) {
        app.tray.checker = cv.g.checker().map(|b| (cv.g.generation, b));
    }

    cv.clip(z);
    let px = thumb_px(app);
    let now = util::mono();
    // Ghosts of removed tiles fade out.
    app.tray.ghosts.retain(|g| now - g.since < 0.3);
    let ghosts: Vec<(Rect, Option<ID2D1Bitmap1>, f32)> = app
        .tray
        .ghosts
        .iter()
        .map(|g| (g.rect, g.bmp.clone(), 1.0 - crate::anim::cubic_bezier(0.22, 1.0, 0.36, 1.0, ((now - g.since) / 0.25) as f32)))
        .collect();
    for (r, bmp, a) in ghosts {
        let vr = to_view(app, r);
        cv.save();
        cv.opacity(a.clamp(0.0, 1.0));
        cv.fill_round(vr, 10.0, theme::CARD);
        if let Some(b) = bmp {
            clip_round(cv, vr, 10.0);
            cv.image_fill(&b, vr, 1.0);
            cv.pop_layer();
        }
        cv.restore();
    }
    if !app.tray.ghosts.is_empty() {
        app.request_frame();
    }

    let first_row = ((app.tray.scroll - INSET_TOP) / (app.tray.side + GAP)).floor().max(0.0) as usize;
    let rows_visible = (z.h / (app.tray.side + GAP)).ceil() as usize + 2;
    let start = first_row * COLUMNS;
    let end = ((first_row + rows_visible) * COLUMNS).min(app.tray.visible.len());
    app.tray.last_rects.clear();
    let lasso_set = app.tray.lasso_set.clone();
    for i in start..end {
        let id = app.tray.visible[i].clone();
        let r = slot(app, i);
        // Animated position (moves when the order changes).
        let x = app.anims.drive(key_id("tile.x", &id), r.x, Curve::mac(0.25));
        let y = app.anims.drive(key_id("tile.y", &id), r.y, Curve::mac(0.25));
        let cr = rect(x, y, r.w, r.h);
        app.tray.last_rects.insert(id.clone(), cr);
        let vr = to_view(app, cr);
        if vr.b() < z.y || vr.y > z.b() {
            continue;
        }
        let on = match &lasso_set {
            Some(set) => set.contains(&id),
            None => app.tray.selected.contains(&id),
        };
        tile(cv, app, &id, vr, on, px, active);
    }
    // Prefetch the next row.
    for i in end..(end + COLUMNS).min(app.tray.visible.len()) {
        let id = app.tray.visible[i].clone();
        if let Some(item) = app.model.get(&id) {
            let path = vault::file_path(item);
            let _ = app.thumbs.get(&id, path, px, Kind::Tile);
        }
    }

    // Lasso.
    if let Some(l) = &app.tray.lasso {
        let a = pt(l.start.x + z.x, l.start.y + z.y - app.tray.scroll);
        let b = pt(l.cur.x + z.x, l.cur.y + z.y - app.tray.scroll);
        let r = Rect::from_points(a, b);
        cv.fill_rect(r, Color::white(0.08));
        cv.stroke_rect(r.inset(0.5, 0.5), 1.0, Color::white(0.35));
    }

    // Overlay scroller.
    let max = max_scroll(app);
    let show = app.anims.drive(key("tray.scroller"), if now < app.tray.scroller_until || app.tray.scroller_drag.is_some() { 1.0 } else { 0.0 }, Curve::ease_out(0.3));
    if max > 0.5 && show > 0.01 {
        let track = rect(z.r() - 9.0, z.y + 2.0, 7.0, z.h - 4.0);
        let kh = (track.h * (z.h / app.tray.content_h)).max(24.0).min(track.h);
        let ky = track.y + (track.h - kh) * (app.tray.scroll / max);
        cv.fill_capsule(rect(track.r() - 6.0, ky, 5.0, kh), Color::white(0.45 * show));
    }
    cv.unclip();

    // Preview overlays the grid.
    preview_render(cv, app, z, active);
}

fn clip_round(cv: &mut Canvas, r: Rect, rad: f32) {
    let geom = cv.g.squircle(r.w, r.h, rad);
    let gm: ID2D1Geometry = geom.cast().unwrap();
    cv.save();
    cv.translate(r.x, r.y);
    cv.push_mask(&gm);
    cv.restore();
}

fn tile(cv: &mut Canvas, app: &mut App, id: &str, r: Rect, on: bool, px: u32, active: bool) {
    let Some(item) = app.model.get(id).cloned() else { return };
    let hovered = active && app.tray.hover.as_deref() == Some(id) && app.tray.lasso.is_none();
    let now = util::mono();
    let copied = app.tray.copied.get(id).map(|t| now < *t).unwrap_or(false);
    let ease = Curve::mac(0.24);
    let hov = app.anims.drive(key_id("tile.hov", id), if hovered { 1.0 } else { 0.0 }, ease);
    let onv = app.anims.drive(key_id("tile.on", id), if on { 1.0 } else { 0.0 }, ease);
    let badge_v = app.anims.drive(key_id("tile.badge", id), if hovered || on { 1.0 } else { 0.0 }, ease);
    let copy_v = app.anims.drive(key_id("tile.copybtn", id), if hovered || copied { 1.0 } else { 0.0 }, ease);
    let copied_v = app.anims.drive(key_id("tile.copied", id), if copied { 1.0 } else { 0.0 }, Curve::mac(0.18));
    let text_v = app.anims.drive(key_id("tile.textbtn", id), if hovered { 1.0 } else { 0.0 }, ease);
    let meta_v = app.anims.drive(key_id("tile.meta", id), if hovered { 1.0 } else { 0.0 }, ease);
    // Entrance (fresh items) and insertion fade.
    if app.model.fresh.contains(id) && !app.tray.played_fresh.contains(id) {
        app.tray.played_fresh.insert(id.to_string());
        pop(app, "tile.card.pop", id, 0.6);
        app.anims.play(key_id("tile.ringflash", id), 2.0, 0.0, Curve::mac(0.9));
    }
    let fade = if app.tray.inserted.contains(id) {
        app.anims.drive_from(key_id("tile.fade", id), 0.0, 1.0, Curve::mac(0.25))
    } else {
        app.anims.drive(key_id("tile.fade", id), 1.0, Curve::Snap)
    };
    let card_pop = app.anims.get(key_id("tile.card.pop", id)).unwrap_or(1.0);
    let ring_flash = app.anims.get(key_id("tile.ringflash", id)).unwrap_or(0.0);

    let path = vault::file_path(&item);
    let bmp = app.thumbs.get(id, path, px, Kind::Tile);
    let img_op = app.anims.drive(key_id("tile.img", id), if bmp.is_some() { 1.0 } else { 0.0 }, Curve::mac(0.28));

    cv.save();
    cv.opacity(fade.clamp(0.0, 1.0));
    // Card (scales on hover and pops on entrance).
    cv.save();
    let cs = (1.0 + 0.03 * hov) * card_pop;
    cv.scale_about(cs, r.cx(), r.cy());
    let card_bg = theme::CARD.lerp(Color::gray(0.1, 1.0), onv.clamp(0.0, 1.0));
    cv.fill_round(r, 10.0, card_bg);
    clip_round(cv, r, 10.0);
    if let Some((_, brush)) = &app.tray.checker {
        let a = cv.alpha * (1.0 - onv.clamp(0.0, 1.0) * 0.0);
        unsafe {
            use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
            let _ = D2D1_COLOR_F::default();
            brush.SetOpacity(a);
            let m = windows_numerics::Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: r.x, M32: r.y };
            brush.SetTransform(&m);
            cv.dc().FillRectangle(&crate::gfx::rf(r), brush);
        }
    }
    if let Some(b) = &bmp {
        let is = 1.0 - 0.12 * onv;
        cv.save();
        cv.scale_about(is, r.cx(), r.cy());
        cv.image_fill(b, r, img_op.clamp(0.0, 1.0));
        cv.restore();
    }
    if meta_v > 0.002 {
        let shade = rect(r.x, r.b() - 38.0, r.w, 38.0);
        cv.save();
        cv.opacity(meta_v.clamp(0.0, 1.0));
        cv.fill_gradient_v(shade, &[(0.0, Color::CLEAR), (1.0, Color::black(0.88))], None);
        let roomy = r.w >= 120.0;
        let dims = item.dims().unwrap_or_default();
        let df = Font::mono(9.5, Weight::Regular);
        if roomy {
            let nf = Font::sans(11.0, Weight::Regular);
            cv.text(&item.name, &nf, theme::PAPER, rect(r.x + 8.0, r.b() - 20.0, (r.w - 90.0).max(0.0), 14.0), Align::Leading, Trim::Tail);
            cv.text(&dims, &df, theme::MUTED, rect(r.r() - 78.0, r.b() - 19.0, 70.0, 13.0), Align::Trailing, Trim::Tail);
        } else {
            cv.text(&dims, &df, theme::MUTED, rect(r.x + 6.0, r.b() - 18.0, r.w - 12.0, 13.0), Align::Trailing, Trim::Tail);
        }
        cv.restore();
    }
    cv.pop_layer();
    // Ring.
    let ring_c = theme::LINE.lerp(theme::LINE2, hov.clamp(0.0, 1.0)).lerp(theme::PAPER, onv.clamp(0.0, 1.0));
    let ring_w = (1.0 + onv.clamp(0.0, 1.0)) + ring_flash.max(0.0);
    cv.stroke_round(r, 10.0, ring_w.min(r.w / 2.0), ring_c);
    cv.restore();

    // Badge (top-left), copy and Copy Text buttons (top-right) — not scaled with the card.
    let badge = rect(r.x + 7.0, r.y + 7.0, 16.0, 16.0);
    if badge_v > 0.002 {
        let bp = app.anims.get(key_id("tile.badge.pop", id)).unwrap_or(1.0);
        cv.save();
        cv.scale_about(bp, badge.cx(), badge.cy());
        cv.opacity(badge_v.clamp(0.0, 1.0));
        let bg = Color::black(0.45).lerp(theme::PAPER, onv.clamp(0.0, 1.0));
        let border = Color::white(0.85).lerp(theme::PAPER, onv.clamp(0.0, 1.0));
        cv.fill_circle(badge.center(), 8.0, bg);
        cv.stroke_circle(badge.center(), 7.25, 1.5, border);
        if onv > 0.01 {
            icons::draw(cv, Icon::Checkmark, badge.center(), 8.0, Weight::Heavy, Color::black(onv.clamp(0.0, 1.0)));
        }
        cv.restore();
    }
    let copy_r = rect(r.r() - 28.0, r.y + 6.0, 22.0, 22.0);
    if copy_v > 0.002 {
        let cp = app.anims.get(key_id("tile.copy.pop", id)).unwrap_or(1.0);
        cv.save();
        cv.scale_about(cp, copy_r.cx(), copy_r.cy());
        cv.opacity(copy_v.clamp(0.0, 1.0));
        let bg = Color::black(0.72).lerp(theme::PAPER, copied_v.clamp(0.0, 1.0));
        cv.fill_circle(copy_r.center(), 11.0, bg);
        cv.stroke_circle(copy_r.center(), 10.5, 1.0, theme::LINE2);
        if copied {
            icons::draw(cv, Icon::Checkmark, copy_r.center(), 9.5, Weight::Bold, Color::black(1.0));
        } else {
            icons::draw(cv, Icon::DocOnDoc, copy_r.center(), 9.5, Weight::Medium, Color::white(1.0));
        }
        cv.restore();
    }
    let side = 22.0 * 0.8;
    let text_r = rect(copy_r.x - 4.0 - side, copy_r.cy() - side / 2.0, side, side);
    if text_v > 0.002 {
        cv.save();
        cv.opacity(text_v.clamp(0.0, 1.0));
        cv.fill_circle(text_r.center(), side / 2.0, Color::black(0.72));
        cv.stroke_circle(text_r.center(), side / 2.0 - 0.5, 1.0, theme::LINE2);
        icons::draw(cv, Icon::TextViewfinder, text_r.center(), 9.5 * 0.8, Weight::Medium, Color::white(1.0));
        cv.restore();
    }
    cv.restore();
}

/// Tooltip for the tile under the pointer: name · domain · source.
pub fn tip_at(app: &App, p: Pt) -> Option<String> {
    if app.island.tab != crate::island::Tab::Vault || app.tray.preview.open || app.island.state != State::Expanded {
        return None;
    }
    let (_, id) = tile_at(app, p)?;
    let item = app.model.get(&id)?;
    let parts: Vec<&str> = [Some(item.name.as_str()), item.domain.as_deref(), item.source.as_deref()].into_iter().flatten().collect();
    Some(parts.join("  ·  "))
}

// ---------------------------------------------------------------------------
// Pointer
// ---------------------------------------------------------------------------

pub fn pointer(app: &mut App, p: Option<Pt>) {
    if app.island.tab != crate::island::Tab::Vault || app.island.state != State::Expanded {
        if app.tray.hover.take().is_some() {
            island::invalidate(app);
        }
        return;
    }
    let over_grid = p.map(|p| app.island.hits.iter().rev().find(|h| h.rect.contains(p)).map(|h| h.act == Act::Grid).unwrap_or(false)).unwrap_or(false);
    let t = if over_grid && !app.tray.preview.open { p.and_then(|p| tile_at(app, p)).map(|(_, id)| id) } else { None };
    if t != app.tray.hover {
        app.tray.hover = t;
        island::invalidate(app);
    }
}

fn tile_zone(app: &App, id: &str, p: Pt) -> u8 {
    // 1 copy, 2 text, 3 badge, 0 body
    let Some(r) = app.tray.last_rects.get(id).map(|r| to_view(app, *r)) else { return 0 };
    let copy_r = rect(r.r() - 28.0, r.y + 6.0, 22.0, 22.0);
    let side = 22.0 * 0.8;
    let text_r = rect(copy_r.x - 4.0 - side, copy_r.cy() - side / 2.0, side, side);
    let badge = rect(r.x + 7.0, r.y + 7.0, 16.0, 16.0);
    let hovered = app.tray.hover.as_deref() == Some(id);
    if hovered && copy_r.contains(p) {
        return 1;
    }
    if hovered && text_r.contains(p) {
        return 2;
    }
    if (hovered || app.tray.selected.contains(id)) && badge.inset(-4.0, -4.0).contains(p) {
        return 3;
    }
    0
}

pub fn mouse_down(app: &mut App, p: Pt, clicks: u32, ctrl: bool, shift: bool) {
    if app.island.tab != crate::island::Tab::Vault || app.island.state != State::Expanded {
        return;
    }
    if app.tray.preview.open {
        return;
    }
    if !app.tray.view.contains(p) {
        return;
    }
    // Scroller knob.
    let max = max_scroll(app);
    if max > 0.5 && p.x > app.tray.view.r() - 11.0 {
        app.tray.scroller_drag = Some((p.y, app.tray.scroll));
        return;
    }
    match tile_at(app, p) {
        Some((i, id)) => {
            if clicks >= 2 && !ctrl && !shift {
                let list = app.tray.visible.clone();
                show_preview(app, &id, list);
                app.tray.down = None;
                return;
            }
            match tile_zone(app, &id, p) {
                1 => {
                    if let Some(item) = app.model.get(&id).cloned() {
                        show_copied(app, &id);
                        copy(app, vec![item]);
                    }
                    return;
                }
                2 => {
                    if let Some(item) = app.model.get(&id).cloned() {
                        copy_text(app, vec![item]);
                    }
                    return;
                }
                3 => {
                    toggle_one(app, &id);
                    return;
                }
                _ => {}
            }
            let mut deferred = false;
            if ctrl {
                toggle_one(app, &id);
            } else if shift {
                let a = app.tray.anchor.unwrap_or(i);
                let (lo, hi) = (a.min(i), a.max(i));
                app.tray.selected = app.tray.visible[lo..=hi].iter().cloned().collect();
                island::invalidate(app);
            } else if app.tray.selected.contains(&id) {
                deferred = true;
            } else {
                select_only(app, &id);
            }
            app.tray.down = Some(Down { tile: Some(id), p, ctrl, shift, deferred, dragged: false });
        }
        None => {
            let base = if ctrl || shift { app.tray.selected.clone() } else { HashSet::new() };
            if !ctrl && !shift {
                app.tray.selected.clear();
            }
            let c = to_content(app, p);
            app.tray.lasso = Some(Lasso { start: c, cur: c, base, ctrl, shift });
            app.tray.lasso_set = Some(app.tray.selected.clone());
            app.tray.down = Some(Down { tile: None, p, ctrl, shift, deferred: false, dragged: false });
            island::invalidate(app);
        }
    }
}

pub fn mouse_drag(app: &mut App, p: Pt) {
    if let Some((y0, s0)) = app.tray.scroller_drag {
        let max = max_scroll(app);
        let track_h = app.tray.view.h - 4.0;
        let kh = (track_h * (app.tray.view.h / app.tray.content_h)).max(24.0).min(track_h);
        let k = if track_h - kh > 1.0 { max / (track_h - kh) } else { 0.0 };
        scroll_to(app, s0 + (p.y - y0) * k, false);
        return;
    }
    let Some(down) = app.tray.down.clone() else { return };
    if let Some(id) = &down.tile {
        if !down.dragged && p.dist(down.p) > 4.0 {
            if let Some(d) = &mut app.tray.down {
                d.dragged = true;
            }
            begin_drag_out(app, id.clone(), down.p);
        }
        return;
    }
    // Lasso with edge auto-scroll.
    let c = to_content(app, p);
    if let Some(l) = &mut app.tray.lasso {
        l.cur = c;
    }
    update_lasso(app);
    let v = app.tray.view;
    let dy = if p.y < v.y + 8.0 {
        -(v.y + 8.0 - p.y).min(40.0) * 0.6
    } else if p.y > v.b() - 8.0 {
        (p.y - (v.b() - 8.0)).min(40.0) * 0.6
    } else {
        0.0
    };
    if dy != 0.0 {
        let target = (app.tray.scroll + dy).clamp(0.0, max_scroll(app));
        scroll_to(app, target, false);
        // Keep scrolling while the pointer stays at the edge.
        ui::after(0.03, move || {
            crate::app::with(|a| {
                if a.tray.lasso.is_some() && unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) } < 0 {
                    if let Some(lp) = a.island.pointer {
                        mouse_drag(a, lp);
                    }
                }
            });
        });
    }
    island::invalidate(app);
}

fn update_lasso(app: &mut App) {
    let Some(l) = app.tray.lasso.clone() else { return };
    let r = Rect::from_points(l.start, l.cur);
    let mut hit: HashSet<String> = HashSet::new();
    for (i, id) in app.tray.visible.iter().enumerate() {
        if slot(app, i).intersects(&r) {
            hit.insert(id.clone());
        }
    }
    let next: HashSet<String> = if l.ctrl {
        l.base.symmetric_difference(&hit).cloned().collect()
    } else if l.shift {
        l.base.union(&hit).cloned().collect()
    } else {
        hit
    };
    app.tray.lasso_set = Some(next);
}

pub fn mouse_up(app: &mut App, _p: Pt) {
    app.tray.scroller_drag = None;
    let Some(down) = app.tray.down.take() else { return };
    if down.tile.is_none() {
        if let Some(set) = app.tray.lasso_set.take() {
            for id in set.iter() {
                if !app.tray.selected.contains(id) {
                    pop(app, "tile.badge.pop", id, 0.6);
                }
            }
            app.tray.selected = set;
        }
        app.tray.lasso = None;
        island::invalidate(app);
        return;
    }
    if down.deferred && !down.dragged {
        if let Some(id) = down.tile {
            select_only(app, &id);
        }
    }
}

pub fn right_click(app: &mut App, p: Pt) -> bool {
    if app.island.tab != crate::island::Tab::Vault || app.tray.preview.open {
        return false;
    }
    let Some((_, id)) = tile_at(app, p) else { return false };
    if !app.tray.selected.contains(&id) {
        select_only(app, &id);
    }
    let selected = selected_items(app);
    let item = app.model.get(&id).cloned();
    let targets = if selected.is_empty() { item.clone().into_iter().collect::<Vec<_>>() } else { selected };
    let sp = app.island.screen(p);
    let info_id = id.clone();
    ui::defer(move || {
        let mut m = crate::menu::Menu::new();
        let t = targets.clone();
        m.action_key("Copy Image", "Ctrl+C", move || {
            crate::app::with(|a| {
                for i in &t {
                    show_copied(a, &i.id);
                }
                copy(a, t);
            });
        });
        let t = targets.clone();
        m.action_key("Copy Text", "Ctrl+T", move || {
            crate::app::with(|a| copy_text(a, t));
        });
        let t = targets.clone();
        m.action("Copy Path", move || {
            crate::app::with(|a| copy_paths(a, &t));
        });
        m.sep();
        let t = targets.clone();
        m.action("Open", move || {
            if let Some(f) = t.first() {
                util::shell_open(&vault::file_path(f).display().to_string());
            }
        });
        let t = targets.clone();
        m.action("Show in Explorer", move || {
            let paths: Vec<PathBuf> = t.iter().map(vault::file_path).collect();
            util::reveal(&paths);
        });
        m.sep();
        m.action("Get Info", move || {
            crate::app::with(|a| {
                let list = a.tray.visible.clone();
                show_preview(a, &info_id, list);
            });
        });
        m.sep();
        m.action_key("Delete", "Del", move || {
            crate::app::with(delete_selection);
        });
        crate::app::popup_menu(m, Some((sp.x, sp.y)));
    });
    true
}

// ---------------------------------------------------------------------------
// Drag out
// ---------------------------------------------------------------------------

fn begin_drag_out(app: &mut App, id: String, _from: Pt) {
    let items = if app.tray.selected.contains(&id) {
        selected_items(app)
    } else {
        select_only(app, &id);
        app.model.get(&id).cloned().into_iter().collect()
    };
    if items.is_empty() {
        return;
    }
    let paths: Vec<PathBuf> = items.iter().map(vault::file_path).filter(|p| p.exists()).collect();
    if paths.is_empty() {
        return;
    }
    let n = paths.len();
    app.tray.dragging_out = true;
    app.tray.down = None;
    let scale = app.island.scale;
    let thumb_paths: Vec<PathBuf> = paths.iter().take(3).cloned().collect();
    ui::defer(move || {
        let Some(obj) = crate::dragdrop::files_object(&paths) else {
            crate::app::with(|a| a.tray.dragging_out = false);
            return;
        };
        if let Some((img, off)) = drag_image(&thumb_paths, n, scale) {
            crate::dragdrop::set_drag_image(&obj, &img, off);
        }
        let effect = crate::dragdrop::run(&obj);
        crate::app::with(|a| {
            a.tray.dragging_out = false;
            a.island.mouse_down_inside = false;
            island::pointer_moved(a);
            if effect != crate::dragdrop::effect_none() {
                island::flash(a, &if n > 1 { format!("Dropped {n} images") } else { "Dropped".to_string() });
            }
        });
    });
}

/// Stacked tile images for the drag (NSDraggingFormation.stack) + count badge.
fn drag_image(paths: &[PathBuf], count: usize, scale: f32) -> Option<(crate::imaging::Raw, windows::Win32::Foundation::POINT)> {
    let side = (72.0 * scale).round() as u32;
    let step = (6.0 * scale).round() as u32;
    let n = paths.len().min(3) as u32;
    let size = side + step * n.saturating_sub(1) + (14.0 * scale) as u32;
    let mut canvas = vec![0u8; (size * size * 4) as usize];
    let blend = |dst: &mut [u8], src: &[u8]| {
        let a = src[3] as u32;
        for k in 0..4 {
            dst[k] = (src[k] as u32 + dst[k] as u32 * (255 - a) / 255).min(255) as u8;
        }
    };
    let radius = 10.0 * scale;
    for (layer, p) in paths.iter().take(3).enumerate().rev() {
        let raw = crate::imaging::tile_thumbnail(p, side)?;
        let off = step * layer as u32;
        // Aspect-fill crop into side × side with rounded corners.
        let k = (side as f32 / raw.w as f32).max(side as f32 / raw.h as f32);
        let ox = (raw.w as f32 * k - side as f32) / 2.0;
        let oy = (raw.h as f32 * k - side as f32) / 2.0;
        for y in 0..side {
            for x in 0..side {
                // Rounded-corner mask.
                let fx = x as f32 + 0.5;
                let fy = y as f32 + 0.5;
                let cx = fx.clamp(radius, side as f32 - radius);
                let cy = fy.clamp(radius, side as f32 - radius);
                let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
                let m = (radius - d + 0.5).clamp(0.0, 1.0);
                if m <= 0.0 {
                    continue;
                }
                let sx = (((x as f32 + ox) / k) as u32).min(raw.w - 1);
                let sy = (((y as f32 + oy) / k) as u32).min(raw.h - 1);
                let si = ((sy * raw.w + sx) * 4) as usize;
                let mut px = [raw.px[si], raw.px[si + 1], raw.px[si + 2], raw.px[si + 3]];
                // Opaque card under transparent images.
                let a = px[3] as u32;
                let card = 12u32;
                for c in 0..3 {
                    px[c] = (px[c] as u32 + card * (255 - a) / 255) as u8;
                }
                px[3] = 255;
                let alpha = 0.92 * m;
                let src = [(px[0] as f32 * alpha) as u8, (px[1] as f32 * alpha) as u8, (px[2] as f32 * alpha) as u8, (255.0 * alpha) as u8];
                let dx = x + off;
                let dy = y + off;
                let di = ((dy * size + dx) * 4) as usize;
                blend(&mut canvas[di..di + 4], &src);
            }
        }
    }
    if count > 1 {
        // Count badge, like the macOS drag badge.
        let r = 10.0 * scale;
        let cx = size as f32 - r - 1.0;
        let cy = r + 1.0;
        for y in 0..size {
            for x in 0..size {
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                let m = (r - d + 0.5).clamp(0.0, 1.0);
                if m > 0.0 {
                    let i = ((y * size + x) * 4) as usize;
                    let src = [(48.0 * m) as u8, (59.0 * m) as u8, (255.0 * m) as u8, (255.0 * m) as u8];
                    blend(&mut canvas[i..i + 4], &src);
                }
            }
        }
        let _ = count;
    }
    Some((crate::imaging::Raw { w: size, h: size, px: canvas }, windows::Win32::Foundation::POINT { x: (side / 2) as i32, y: (side / 2) as i32 }))
}

// ---------------------------------------------------------------------------
// Keyboard
// ---------------------------------------------------------------------------

pub fn key_down(app: &mut App, vk: u16) -> bool {
    let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
    let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
    if app.tray.preview.open {
        return preview_key(app, vk, ctrl);
    }
    let v = VIRTUAL_KEY(vk);
    if ctrl {
        match vk as u8 {
            b'C' => {
                copy_selection(app);
                return true;
            }
            b'T' => {
                copy_text_selection(app);
                return true;
            }
            b'A' => {
                toggle_select_all(app);
                return true;
            }
            b'Z' => {
                let n = crate::app::undo_delete(app);
                if n > 0 {
                    island::flash(app, &format!("Restored {n}"));
                }
                return true;
            }
            _ => {}
        }
    }
    match v {
        VK_DELETE | VK_BACK => {
            delete_selection(app);
            true
        }
        VK_SPACE | VK_RETURN => {
            if let Some(first) = selected_items(app).first() {
                let id = first.id.clone();
                let list = app.tray.visible.clone();
                show_preview(app, &id, list);
            }
            true
        }
        VK_ESCAPE => {
            if app.tray.selected.is_empty() {
                island::collapse(app);
            } else {
                clear_selection(app);
            }
            true
        }
        VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN => {
            arrow(app, v, shift);
            true
        }
        _ => false,
    }
}

fn arrow(app: &mut App, v: VIRTUAL_KEY, shift: bool) {
    let n = app.tray.visible.len();
    if n == 0 {
        return;
    }
    let lead = app.tray.anchor_lead().unwrap_or(usize::MAX);
    let next = if lead == usize::MAX {
        0
    } else {
        match v {
            VK_LEFT => lead.saturating_sub(1),
            VK_RIGHT => (lead + 1).min(n - 1),
            VK_UP => {
                if lead >= COLUMNS { lead - COLUMNS } else { lead }
            }
            _ => {
                if lead + COLUMNS < n { lead + COLUMNS } else { lead }
            }
        }
    };
    let id = app.tray.visible[next].clone();
    if shift {
        let a = app.tray.anchor.unwrap_or(next);
        let (lo, hi) = (a.min(next), a.max(next));
        app.tray.selected = app.tray.visible[lo..=hi].iter().cloned().collect();
        app.tray.lead = Some(next);
        if app.tray.anchor.is_none() {
            app.tray.anchor = Some(next);
        }
    } else {
        select_only(app, &id);
        app.tray.lead = Some(next);
    }
    ensure_visible(app, next);
    island::invalidate(app);
}

impl Tray {
    fn anchor_lead(&self) -> Option<usize> {
        if self.selected.is_empty() {
            return None;
        }
        if let Some(l) = self.lead {
            if l < self.visible.len() && self.selected.contains(&self.visible[l]) {
                return Some(l);
            }
        }
        if let Some(a) = self.anchor {
            if a < self.visible.len() {
                return Some(a);
            }
        }
        self.visible.iter().position(|id| self.selected.contains(id))
    }
}

// ---------------------------------------------------------------------------
// Preview
// ---------------------------------------------------------------------------

pub fn show_preview(app: &mut App, id: &str, list: Vec<String>) {
    let index = list.iter().position(|x| x == id).unwrap_or(0);
    app.tray.preview.items = list;
    app.tray.preview.index = index;
    let was_open = app.tray.preview.open && !app.tray.preview.closing;
    app.tray.preview.open = true;
    app.tray.preview.closing = false;
    if !was_open {
        app.anims.snap(key("preview.alpha"), 0.0);
    }
    load_preview(app);
    island::invalidate(app);
}

pub fn close_preview(app: &mut App) {
    if !app.tray.preview.open || app.tray.preview.closing {
        return;
    }
    app.tray.preview.closing = true;
    island::invalidate(app);
}

fn load_preview(app: &mut App) {
    let p = &mut app.tray.preview;
    p.token += 1;
    p.thumb = None;
    p.full = None;
    let Some(id) = p.items.get(p.index).cloned() else { return };
    let Some(item) = app.model.get(&id).cloned() else { return };
    let token = p.token;
    let path = vault::file_path(&item);
    // Grid thumbnail first so it's never blank, then a screen-sized decode.
    app.tray.preview.thumb = app.thumbs.peek(&id, thumb_px(app), Kind::Tile);
    let max_px = (1200.0f32).max(app.island.scale * 900.0) as u32;
    crate::thumbs::pool(move || {
        let raw = imaging::thumbnail(&path, max_px);
        ui::post(move || {
            crate::app::with(|a| {
                if a.tray.preview.token != token {
                    return;
                }
                if let Some(raw) = raw {
                    a.tray.preview.full = crate::gfx::gfx().bitmap(raw.w, raw.h, &raw.px);
                    island::invalidate(a);
                }
            });
        });
    });
}

pub fn preview_step(app: &mut App, delta: i32) {
    let p = &app.tray.preview;
    let next = p.index as i32 + delta;
    if next < 0 || next >= p.items.len() as i32 {
        util::beep();
        return;
    }
    app.tray.preview.index = next as usize;
    load_preview(app);
    island::invalidate(app);
}

fn current_item(app: &App) -> Option<VaultItem> {
    let p = &app.tray.preview;
    p.items.get(p.index).and_then(|id| app.model.get(id)).cloned()
}

pub fn preview_copy(app: &mut App) {
    if let Some(i) = current_item(app) {
        copy(app, vec![i]);
    }
}

pub fn preview_text(app: &mut App) {
    if let Some(i) = current_item(app) {
        copy_text(app, vec![i]);
    }
}

pub fn preview_click(app: &mut App, p: Pt) {
    if let Some(r) = app.island.hits.iter().find(|h| h.act == Act::PreviewImage).map(|h| h.rect) {
        if p.x < r.x + r.w / 3.0 {
            preview_step(app, -1);
        } else if p.x > r.x + r.w * 2.0 / 3.0 {
            preview_step(app, 1);
        }
    }
}

fn preview_key(app: &mut App, vk: u16, ctrl: bool) -> bool {
    match VIRTUAL_KEY(vk) {
        VK_ESCAPE | VK_SPACE => close_preview(app),
        VK_LEFT => preview_step(app, -1),
        VK_RIGHT => preview_step(app, 1),
        _ if ctrl && vk == b'C' as u16 => preview_copy(app),
        _ if ctrl && vk == b'T' as u16 => preview_text(app),
        _ => return false,
    }
    true
}

fn icon_button(cv: &mut Canvas, app: &mut App, r: Rect, icon: Icon, name: &str, act: Act, tip: &str, enabled: bool, active: bool) {
    let id = key_id("pvbtn", name);
    if active && enabled {
        app.island.hits.push(Hit { rect: r, id, tip: Some(tip.to_string()), act, text: false });
    }
    let hovered = app.island.hover == Some(id);
    let pressed = app.island.pressed.as_ref().map(|(p, _)| *p == id).unwrap_or(false);
    if pressed {
        app.anims.play(key_id("pvbtn.pop", name), 0.88, 1.0, Curve::POP);
    }
    let s = app.anims.get(key_id("pvbtn.pop", name)).unwrap_or(1.0);
    cv.save();
    cv.scale_about(s, r.cx(), r.cy());
    cv.opacity(if enabled { 1.0 } else { 0.3 });
    if hovered && enabled {
        cv.stroke_rrect(r.inset(0.5, 0.5), 3.0, 1.0, theme::LINE2);
    }
    let fg = if hovered && enabled { Color::white(1.0) } else { theme::MUTED };
    icons::draw(cv, icon, r.center(), 13.0, Weight::Medium, fg);
    cv.restore();
}

fn preview_render(cv: &mut Canvas, app: &mut App, z: Rect, active: bool) {
    if !app.tray.preview.open {
        return;
    }
    let target = if app.tray.preview.closing { 0.0 } else { 1.0 };
    let dur = if app.tray.preview.closing { 0.2 } else { 0.22 };
    let a = app.anims.drive(key("preview.alpha"), target, Curve::mac(dur));
    if app.tray.preview.closing && a <= 0.001 {
        app.tray.preview.open = false;
        app.tray.preview.closing = false;
        app.tray.preview.thumb = None;
        app.tray.preview.full = None;
        return;
    }
    let Some(item) = current_item(app) else { return };
    let hits = active && !app.tray.preview.closing;
    cv.save();
    cv.opacity(a.clamp(0.0, 1.0));
    cv.fill_rect(z, theme::INK);
    if hits {
        // Swallow grid clicks under the preview.
        app.island.hits.push(Hit { rect: z, id: key("preview.bg"), tip: None, act: Act::None, text: false });
    }
    let name_f = Font::sans(12.0, Weight::Semibold);
    let meta_f = Font::mono(10.0, Weight::Regular);
    let counter_f = Font::mono(10.0, Weight::Medium);
    let name_h = cv.g.line_height(&name_f);
    let meta_h = cv.g.line_height(&meta_f);
    let name_y = z.y + 4.0;
    let meta_y = name_y + name_h + 1.0;
    let bar_cy = name_y + name_h + 1.0;
    // Button bar (trailing −14).
    let p = &app.tray.preview;
    let (index, count) = (p.index, p.items.len());
    let counter = format!("{} / {}", index + 1, count);
    let (cw, ch) = cv.measure(&counter, &counter_f);
    let mut x = z.r() - 14.0;
    let bt = |x: f32| rect(x - 30.0, bar_cy - 15.0, 30.0, 30.0);
    let close_r = bt(x);
    x -= 32.0;
    let copy_r = bt(x);
    x -= 32.0;
    let text_r = bt(x);
    x -= 30.0 + 10.0;
    let next_r = bt(x);
    x -= 32.0;
    let counter_r = rect(x - cw, bar_cy - ch / 2.0, cw, ch);
    x -= cw + 2.0;
    let prev_r = bt(x);
    icon_button(cv, app, prev_r, Icon::ChevronLeft, "prev", Act::PreviewPrev, "Previous  ←", index > 0, hits);
    cv.text(&counter, &counter_f, theme::MUTED, counter_r, Align::Leading, Trim::None);
    icon_button(cv, app, next_r, Icon::ChevronRight, "next", Act::PreviewNext, "Next  →", index + 1 < count, hits);
    icon_button(cv, app, text_r, Icon::DocTextMagnifyingGlass, "text", Act::PreviewText, "Copy Text  Ctrl+T", true, hits);
    icon_button(cv, app, copy_r, Icon::DocOnDoc, "copy", Act::PreviewCopy, "Copy  Ctrl+C", true, hits);
    icon_button(cv, app, close_r, Icon::XMark, "close", Act::PreviewClose, "Close  Esc", true, hits);
    // Labels.
    let label_w = (prev_r.x - 12.0 - (z.x + 18.0)).max(10.0);
    let path = vault::file_path(&item);
    let name_r = rect(z.x + 18.0, name_y, label_w, name_h);
    cv.text(&item.name, &name_f, Color::white(1.0), name_r, Align::Leading, Trim::Middle);
    let dims = item.dims();
    let size = item.bytes.map(theme::byte_count);
    let date = if item.timestamp > 0.0 { Some(util::medium_date(item.timestamp)) } else { None };
    let src = item.domain.clone().or(item.source.clone());
    let meta: Vec<String> = [dims, size, src, date].into_iter().flatten().collect();
    let meta_r = rect(z.x + 18.0, meta_y, label_w, meta_h);
    cv.text(&meta.join("  ·  "), &meta_f, theme::MUTED, meta_r, Align::Leading, Trim::Tail);
    if hits {
        let nw = cv.measure(&item.name, &name_f).0.min(label_w);
        app.island.hits.push(Hit { rect: rect(name_r.x, name_r.y, nw, name_r.h), id: key("pv.name"), tip: Some(path.display().to_string()), act: Act::PreviewName, text: false });
        let mt: Vec<String> = [item.page_title.clone(), item.page_url.clone(), item.url.clone()].into_iter().flatten().collect();
        if !mt.is_empty() {
            app.island.hits.push(Hit { rect: meta_r, id: key("pv.meta"), tip: Some(mt.join("\n")), act: Act::PreviewMeta, text: false });
        }
    }
    // Image (aspect fit, 16 / 16 / 14 insets).
    let img_r = rect(z.x + 16.0, meta_y + meta_h + 8.0, z.w - 32.0, (z.b() - 14.0 - (meta_y + meta_h + 8.0)).max(1.0));
    if hits {
        app.island.hits.push(Hit { rect: img_r, id: key("pv.image"), tip: None, act: Act::PreviewImage, text: false });
    }
    let bmp = app.tray.preview.full.clone().or(app.tray.preview.thumb.clone());
    if let Some(b) = bmp {
        cv.clip(img_r);
        cv.image_fit(&b, img_r, 1.0, true);
        cv.unclip();
    }
    cv.restore();
}
