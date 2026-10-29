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
    /// Hover-grid button under the pointer (0 select, 1 copy, 2 edit, 3 text/code).
    hot: Option<usize>,
    down: Option<Down>,
    copied: HashMap<String, f64>,
    copied_label: HashMap<String, &'static str>,
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
    /// Soonest GIF frame change among the tiles drawn this frame, and whether
    /// a wake-up for it is already scheduled.
    gif_next: Option<f32>,
    gif_timer: bool,
    // -- folders --
    /// How many of a folder's images are out of it (shown after it).
    out_count: HashMap<String, usize>,
    /// Folders opening or closing right now (clicks wait until done).
    animating: HashSet<String>,
    /// Images flying out of / back into their folder: start time.
    emerge: HashMap<String, f64>,
    folding: HashMap<String, f64>,
    /// Papers each folder showed last frame (pops when it changes).
    folder_shown: HashMap<String, usize>,
    /// Grid ids being dragged, the tile / folder under an in-grid drag, and
    /// whether the last drag ended in the grid (no "Dropped" message then).
    dragged: Vec<String>,
    pub drop_target: Option<String>,
    dropped_inside: bool,
    /// Inline folder rename: group id and its field.
    pub renaming: Option<(String, crate::textfield::TextField)>,
    /// The store's matches for the current query: (query, generation, OCR
    /// generation, ids). Searching runs in SQLite; the model has no OCR text.
    matches: Option<(String, u64, u64, HashSet<String>)>,
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
            hot: None,
            down: None,
            copied: HashMap::new(),
            copied_label: HashMap::new(),
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
            gif_next: None,
            gif_timer: false,
            out_count: HashMap::new(),
            animating: HashSet::new(),
            emerge: HashMap::new(),
            folding: HashMap::new(),
            folder_shown: HashMap::new(),
            dragged: Vec::new(),
            drop_target: None,
            dropped_inside: false,
            renaming: None,
            matches: None,
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

/// Grid ids for folders ("folder:<group id>"); everything else is an image id.
pub const FOLDER: &str = "folder:";

pub fn folder_gid(id: &str) -> Option<&str> {
    id.strip_prefix(FOLDER)
}

fn searching(app: &App) -> bool {
    !app.tray.query.trim().is_empty()
}

/// A folder's images, in folder order.
pub fn members(app: &App, gid: &str) -> Vec<VaultItem> {
    let Some(g) = app.model.group(gid) else { return Vec::new() };
    g.item_ids.iter().filter_map(|id| app.model.get(id)).cloned().collect()
}

fn filtered_sorted(app: &App, mut items: Vec<&VaultItem>) -> Vec<String> {
    if let Some((_, _, _, ids)) = app.tray.matches.as_ref().filter(|_| searching(app)) {
        items.retain(|i| ids.contains(&i.id));
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

/// Grid order: folders first (newest first), each followed by the images that
/// are out of it, then everything not in a folder. Searching shows a flat list.
pub fn visible_items(app: &App) -> Vec<String> {
    if searching(app) {
        return filtered_sorted(app, app.model.items.iter().collect());
    }
    let mut ids = Vec::new();
    let mut groups: Vec<&vault::VaultGroup> = app.model.groups.iter().collect();
    groups.sort_by(|a, b| b.created.partial_cmp(&a.created).unwrap_or(std::cmp::Ordering::Equal));
    for g in groups {
        ids.push(format!("{FOLDER}{}", g.id));
        let out = app.tray.out_count.get(&g.id).copied().unwrap_or(0);
        ids.extend(members(app, &g.id).into_iter().take(out).map(|i| i.id));
    }
    ids.extend(filtered_sorted(app, app.model.items.iter().filter(|i| !app.model.group_of.contains_key(&i.id)).collect()));
    ids
}

/// The images in the grid (no folders), in grid order.
pub fn image_ids(app: &App) -> Vec<String> {
    app.tray.visible.iter().filter(|id| folder_gid(id).is_none()).cloned().collect()
}

/// applySnapshot: refresh the visible list, keep the selection, animate.
/// Name, site, source and OCR text, matched in SQLite (full-text index);
/// cached per query and store state.
fn refresh_matches(app: &mut App) {
    let q = app.tray.query.trim().to_string();
    if q.is_empty() {
        app.tray.matches = None;
        return;
    }
    let (g, og) = (vault::generation(), vault::ocr_generation());
    if app.tray.matches.as_ref().map(|(mq, mg, mo, _)| *mq == q && *mg == g && *mo == og).unwrap_or(false) {
        return;
    }
    let ids = vault::search(&q).unwrap_or_default();
    app.tray.matches = Some((q, g, og, ids));
}

/// OCR text changed: only an active search can show something different.
pub fn ocr_changed(app: &mut App) {
    if searching(app) {
        apply(app);
    }
}

pub fn apply(app: &mut App) {
    refresh_matches(app);
    // Folders whose images were moved or deleted: keep the counts honest.
    let counts: Vec<(String, usize)> = app.tray.out_count.iter().map(|(g, n)| (g.clone(), *n)).collect();
    for (gid, n) in counts {
        if app.model.group(&gid).is_none() {
            app.tray.out_count.remove(&gid);
        } else {
            let total = members(app, &gid).len();
            app.tray.out_count.insert(gid, n.min(total));
        }
    }
    let next = visible_items(app);
    let old: HashSet<String> = app.tray.visible.iter().cloned().collect();
    let new: HashSet<String> = next.iter().cloned().collect();
    let animate = app.tray.shown_once && app.island.state == State::Expanded;
    if animate {
        let now = util::mono();
        for id in app.tray.visible.iter() {
            if !new.contains(id) && !app.tray.folding.contains_key(id) {
                if let Some(r) = app.tray.last_rects.get(id).copied() {
                    let hash = app.model.get(id).and_then(|i| i.hash.clone());
                    let bmp = app.thumbs.peek(id, hash.as_deref(), crate::thumbs::GRID_PX, Kind::Tile);
                    app.tray.ghosts.push(Ghost { id: id.clone(), rect: r, since: now, bmp });
                }
            }
        }
        app.tray.inserted = next.iter().filter(|id| !old.contains(*id) && !app.tray.emerge.contains_key(*id)).cloned().collect();
    } else {
        app.tray.inserted.clear();
    }
    app.tray.visible = next;
    app.tray.index_of = app.tray.visible.iter().enumerate().map(|(i, id)| (id.clone(), i)).collect();
    let vis = &app.tray.index_of;
    app.tray.selected.retain(|id| vis.contains_key(id) && folder_gid(id).is_none());
    if let Some(a) = app.tray.anchor {
        if a >= app.tray.visible.len() {
            app.tray.anchor = None;
        }
    }
    if app.tray.preview.open {
        app.tray.preview.items.retain(|id| app.model.get(id).is_some());
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
    let images = image_ids(app);
    if !images.is_empty() && app.tray.selected.len() == images.len() {
        app.tray.selected.clear();
    } else {
        app.tray.selected = images.into_iter().collect();
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

/// Copy without the header message (the tile's overlay says it).
fn copy_quiet(app: &mut App, items: Vec<VaultItem>) {
    let _ = app;
    util::spawn("copy", move || {
        imaging::com_init();
        clipboard::copy_items(&items);
    });
}

pub fn copy_selection(app: &mut App) {
    let items = selected_items(app);
    for i in &items {
        show_copied(app, &i.id);
    }
    copy(app, items);
}

fn show_copied(app: &mut App, id: &str) {
    show_copied_label(app, id, "Copied");
}

fn show_copied_label(app: &mut App, id: &str, label: &'static str) {
    app.tray.copied_label.insert(id.to_string(), label);
    app.tray.copied.insert(id.to_string(), util::mono() + 1.4);
    pop(app, "tile.copy.pop", id, 0.7);
    app.anims.play(key_id("tile.ringflash", id), 3.0, 0.0, Curve::mac(0.5));
    crate::views::start_copied(app, key_id("tile.copiedov", id));
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

fn thumb_px(_app: &App) -> u32 {
    crate::thumbs::GRID_PX
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
    app.tray.gif_next = None;
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
        if let Some(gid) = folder_gid(&id) {
            let gid = gid.to_string();
            folder_tile(cv, app, &gid, vr, px, active);
        } else {
            tile(cv, app, &id, vr, on, px, active);
        }
    }
    if !app.tray.emerge.is_empty() || !app.tray.folding.is_empty() {
        let now = util::mono();
        app.tray.emerge.retain(|_, t| now - *t < 1.2);
        app.request_frame();
    }
    // Wake for the next GIF frame (one timer at a time; none when no GIF shows).
    if let (Some(next), false) = (app.tray.gif_next, app.tray.gif_timer) {
        app.tray.gif_timer = true;
        ui::after(next as f64, || {
            crate::app::with(|a| {
                a.tray.gif_timer = false;
                island::invalidate(a);
            });
        });
    }
    // Prefetch the next row.
    for i in end..(end + COLUMNS).min(app.tray.visible.len()) {
        let id = app.tray.visible[i].clone();
        if let Some(item) = app.model.get(&id) {
            let path = vault::file_path(item);
            let hash = item.hash.clone();
            let _ = app.thumbs.get(&id, path, hash.as_deref(), px, Kind::Tile);
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

pub fn is_svg(item: &VaultItem) -> bool {
    item.mime.as_deref() == Some("image/svg+xml") || item.name.to_lowercase().ends_with(".svg")
}

/// An SVG image's source as plain text, unmarked so it shows up (with a
/// preview) in the Clipboard tab and isn't saved to the vault again.
pub fn copy_svg_code(app: &mut App, item: &VaultItem, on_tile: bool) {
    match std::fs::read(vault::file_path(item)).ok().and_then(|d| String::from_utf8(d).ok()) {
        Some(code) => {
            clipboard::copy_text(&code);
            if on_tile {
                show_copied_label(app, &item.id, "Code copied");
            }
            island::flash(app, "SVG code copied");
        }
        None => island::flash(app, "Couldn't read the SVG"),
    }
}

pub fn is_gif(item: &VaultItem) -> bool {
    item.mime.as_deref() == Some("image/gif") || item.name.to_lowercase().ends_with(".gif")
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
    // The 2×2 hover grid shows while hovered (not during the copy overlay);
    // the corner check marks a selection while the grid is away.
    let show_buttons = hovered && !copied && app.tray.drop_target.is_none();
    let badge_v = app.anims.drive(key_id("tile.badge", id), if on && !show_buttons { 1.0 } else { 0.0 }, ease);
    let grid_v = app.anims.drive(key_id("tile.grid", id), if show_buttons { 1.0 } else { 0.0 }, ease);
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
    // A folder is open: everything outside it steps back. Another tile being
    // dragged onto this one (drop = new folder): it shrinks with a white ring.
    let dim = app.anims.drive(key_id("tile.dim", id), if is_dimmed(app, id) && !hovered { 0.3 } else { 1.0 }, ease);
    let drop = app.tray.drop_target.as_deref() == Some(id);
    let drop_s = app.anims.drive(key_id("tile.drop", id), if drop { 0.9 } else { 1.0 }, ease);
    let drop_v = app.anims.drive(key_id("tile.dropv", id), if drop { 1.0 } else { 0.0 }, ease).clamp(0.0, 1.0);
    let (motion_k, motion_op, offset) = folder_motion(app, id, r);

    let path = vault::file_path(&item);
    // Live GIFs: the current frame, falling back to the still while decoding.
    let gif = if is_gif(&item) && active { app.thumbs.gif_frame(id, path.clone(), 240, now) } else { None };
    let bmp = match gif {
        Some((frame, next)) => {
            app.tray.gif_next = Some(app.tray.gif_next.map_or(next, |n| n.min(next)));
            Some(frame)
        }
        None => app.thumbs.get(id, path, item.hash.as_deref(), px, Kind::Tile),
    };
    let img_op = app.anims.drive(key_id("tile.img", id), if bmp.is_some() { 1.0 } else { 0.0 }, Curve::mac(0.28));

    cv.save();
    cv.opacity((fade * dim * motion_op).clamp(0.0, 1.0));
    // Flying out of / into a folder: from its mouth, small, into place.
    if motion_k < 0.999 || motion_k > 1.001 {
        cv.translate(offset.x * (1.0 - motion_k), offset.y * (1.0 - motion_k));
        cv.scale_about(0.28 + 0.72 * motion_k, r.cx(), r.cy());
    }
    // Card (scales on hover and pops on entrance).
    cv.save();
    let cs = (1.0 + 0.03 * hov * (1.0 - drop_v)) * card_pop * drop_s;
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
        if is_svg(&item) {
            // SVGs are logos and icons: show them whole, not cropped.
            cv.image_fit(b, r, img_op.clamp(0.0, 1.0), true);
        } else {
            cv.image_fill(b, r, img_op.clamp(0.0, 1.0));
        }
        cv.restore();
    }
    // Kind badge (bottom-left): SVG / GIF, hidden while the hover grid shows.
    let kind = if is_svg(&item) { Some("SVG") } else if is_gif(&item) { Some("GIF") } else { None };
    if let Some(k) = kind {
        let kb = rect(r.x + 6.0, r.b() - 6.0 - 11.0, 22.0, 11.0);
        cv.save();
        cv.opacity((1.0 - hov).clamp(0.0, 1.0));
        cv.fill_rrect(kb, 3.0, Color::white(0.9));
        cv.text(k, &Font::sans(7.5, Weight::Heavy), Color::black(1.0), kb, Align::Center, Trim::None);
        cv.restore();
    }
    // Hover veil and the cross dividing the tile into four actions.
    if grid_v > 0.002 {
        let g = grid_v.clamp(0.0, 1.0);
        cv.fill_rect(r, Color::black(0.58 * g));
        let inset = 9.0;
        let hw = (r.w - inset * 2.0) * (0.2 + 0.8 * g);
        let vh = (r.h - inset * 2.0) * (0.2 + 0.8 * g);
        cv.fill_rect(rect(r.cx() - hw / 2.0, r.cy() - 0.25, hw, 0.5), Color::white(0.22 * g));
        cv.fill_rect(rect(r.cx() - 0.25, r.cy() - vh / 2.0, 0.5, vh), Color::white(0.22 * g));
    }
    if let Some(t) = crate::views::copied_t(app, key_id("tile.copiedov", id)) {
        let label = app.tray.copied_label.get(id).copied().unwrap_or("Copied");
        crate::views::copied_overlay(cv, r, t, if r.h >= 64.0 { label } else { "" });
    }
    cv.pop_layer();
    // Ring.
    let ring_c = theme::LINE.lerp(theme::LINE2, hov.clamp(0.0, 1.0)).lerp(theme::PAPER, onv.clamp(0.0, 1.0).max(drop_v));
    let ring_w = (1.0 + onv.clamp(0.0, 1.0).max(drop_v)) + ring_flash.max(0.0);
    cv.stroke_round(r, 10.0, ring_w.min(r.w / 2.0), ring_c);
    cv.restore();

    // Corner check (top-left) — not scaled with the card.
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
    // The four hover actions: select · copy / edit · text (or SVG code).
    if grid_v > 0.002 {
        let svg = is_svg(&item);
        let icons4 = [
            if on { Icon::CheckmarkCircleFill } else { Icon::Circle },
            Icon::DocOnDoc,
            Icon::Pencil,
            if svg { Icon::Code } else { Icon::TextAlignLeft },
        ];
        let hot = if show_buttons { app.tray.hot } else { None };
        for (i, icon) in icons4.iter().enumerate() {
            let br = button_rect(r, i);
            let is_hot = hot == Some(i);
            let hv = app.anims.drive(key_id(&format!("tile.hot{i}"), id), if is_hot { 1.0 } else { 0.0 }, Curve::mac(0.16)).clamp(0.0, 1.0);
            let pop_in = app.anims.get(key_id(&format!("tile.bin{i}"), id)).unwrap_or(1.0);
            let press = app.anims.get(key_id(&format!("tile.bpop{i}"), id)).unwrap_or(1.0);
            let sc = (0.6 + 0.4 * grid_v) * pop_in * press * (1.0 + 0.14 * hv);
            cv.save();
            cv.scale_about(sc, br.cx(), br.cy());
            cv.opacity(grid_v.clamp(0.0, 1.0));
            cv.fill_circle(br.center(), 10.5, Color::black(0.72).lerp(theme::PAPER, hv));
            cv.stroke_circle(br.center(), 10.0, 1.0, Color::white(0.18 * (1.0 - hv)));
            let w = if is_hot { Weight::Bold } else { Weight::Medium };
            icons::draw(cv, *icon, br.center(), 21.0 * 0.43, w, Color::white(1.0).lerp(Color::black(1.0), hv));
            cv.restore();
        }
    }
    cv.restore();
}

/// Centre of each quadrant: 0 top-left, 1 top-right, 2 bottom-left, 3 bottom-right.
fn button_rect(r: Rect, i: usize) -> Rect {
    let side = 21.0;
    let x = if i % 2 == 0 { r.x + r.w * 0.25 } else { r.x + r.w * 0.75 };
    let y = if i < 2 { r.y + r.h * 0.25 } else { r.y + r.h * 0.75 };
    rect(x - side / 2.0, y - side / 2.0, side, side)
}

/// Which hover action the point is over (the button plus a small margin).
fn action_at(r: Rect, p: Pt) -> Option<usize> {
    (0..4).find(|i| button_rect(r, *i).inset(-5.0, -5.0).contains(p))
}

/// The four actions pop in one after another, clockwise from top-left.
fn stagger_in(app: &mut App, id: &str) {
    for (n, i) in [0usize, 1, 3, 2].iter().enumerate() {
        let curve = Curve::Spring { mass: 0.6, stiffness: 320.0, damping: 13.0, delay: n as f32 * 0.025 };
        app.anims.play(key_id(&format!("tile.bin{i}"), id), 0.5, 1.0, curve);
    }
    app.request_frame();
}

/// Tooltip for the tile under the pointer: name · domain · source.
pub fn tip_at(app: &App, p: Pt) -> Option<String> {
    if app.island.tab != crate::island::Tab::Vault || app.tray.preview.open || app.island.state != State::Expanded {
        return None;
    }
    let (_, id) = tile_at(app, p)?;
    if let Some(gid) = folder_gid(&id) {
        let g = app.model.group(gid)?;
        let n = members(app, gid).len();
        let inside = n - app.tray.out_count.get(gid).copied().unwrap_or(0).min(n);
        let verb = if inside > 0 { "open" } else { "close" };
        return Some(format!("{} · {n} image{} — click to {verb}", g.name, if n == 1 { "" } else { "s" }));
    }
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
        app.tray.hover = t.clone();
        if let Some(id) = t.as_ref().filter(|id| folder_gid(id).is_none()) {
            if !app.tray.copied.get(id).map(|until| util::mono() < *until).unwrap_or(false) {
                stagger_in(app, id);
            }
        }
        island::invalidate(app);
    }
    let hot = match (&t, p) {
        (Some(id), Some(p)) if app.tray.lasso.is_none() && folder_gid(id).is_none() => {
            app.tray.last_rects.get(id).and_then(|r| action_at(to_view(app, *r), p))
        }
        _ => None,
    };
    if hot != app.tray.hot {
        app.tray.hot = hot;
        island::invalidate(app);
    }
}

/// What a press on a tile hits: a hover action (`Some(0..4)`), the corner
/// check of a selected tile (`Some(4)`), or the tile itself (`None`).
fn tile_zone(app: &App, id: &str, p: Pt) -> Option<usize> {
    let r = app.tray.last_rects.get(id).map(|r| to_view(app, *r))?;
    let hovered = app.tray.hover.as_deref() == Some(id);
    let copied = app.tray.copied.get(id).map(|t| util::mono() < *t).unwrap_or(false);
    if hovered && !copied {
        if let Some(i) = action_at(r, p) {
            return Some(i);
        }
    }
    let badge = rect(r.x + 7.0, r.y + 7.0, 16.0, 16.0);
    if app.tray.selected.contains(id) && badge.inset(-4.0, -4.0).contains(p) {
        return Some(4);
    }
    None
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
    if app.tray.renaming.is_some() {
        commit_rename(app);
    }
    if let Some((_, id)) = tile_at(app, p) {
        if let Some(gid) = folder_gid(&id) {
            let gid = gid.to_string();
            app.anims.play(key_id("fold.click", &gid), 0.92, 1.0, Curve::POP);
            toggle_folder(app, gid);
            app.tray.down = None;
            return;
        }
    }
    match tile_at(app, p) {
        Some((i, id)) => {
            if clicks >= 2 && !ctrl && !shift {
                let list = image_ids(app);
                show_preview(app, &id, list);
                app.tray.down = None;
                return;
            }
            if let Some(zone) = tile_zone(app, &id, p) {
                if zone < 4 {
                    app.anims.play(key_id(&format!("tile.bpop{zone}"), &id), 0.8, 1.0, Curve::POP);
                }
                let Some(item) = app.model.get(&id).cloned() else { return };
                match zone {
                    1 => {
                        show_copied(app, &id);
                        copy_quiet(app, vec![item]);
                    }
                    2 => crate::capture::edit_vault_item(app, &item),
                    3 if is_svg(&item) => copy_svg_code(app, &item, true),
                    3 => copy_text(app, vec![item]),
                    _ => toggle_one(app, &id),
                }
                return;
            }
            let mut deferred = false;
            if ctrl {
                toggle_one(app, &id);
            } else if shift {
                let a = app.tray.anchor.unwrap_or(i);
                let (lo, hi) = (a.min(i), a.max(i));
                app.tray.selected = app.tray.visible[lo..=hi].iter().filter(|id| folder_gid(id).is_none()).cloned().collect();
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
        if folder_gid(id).is_none() && slot(app, i).intersects(&r) {
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
    if let Some(gid) = folder_gid(&id) {
        folder_menu(app, gid.to_string(), app.island.screen(p));
        return true;
    }
    if !app.tray.selected.contains(&id) {
        select_only(app, &id);
    }
    let selected = selected_items(app);
    let item = app.model.get(&id).cloned();
    let targets = if selected.is_empty() { item.clone().into_iter().collect::<Vec<_>>() } else { selected };
    let sp = app.island.screen(p);
    let info_id = id.clone();
    let groups: Vec<(String, String, bool)> = app
        .model
        .groups
        .iter()
        .map(|g| (g.id.clone(), g.name.clone(), targets.iter().all(|t| app.model.group_of.get(&t.id) == Some(&g.id))))
        .collect();
    let any_grouped = targets.iter().any(|t| app.model.group_of.contains_key(&t.id));
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
        if let (1, Some(it)) = (targets.len(), item.clone().filter(is_svg)) {
            m.action("Copy SVG Code", move || {
                crate::app::with(|a| copy_svg_code(a, &it, false));
            });
        }
        if let Some(it) = item.clone() {
            m.action("Edit Image", move || {
                crate::app::with(|a| crate::capture::edit_vault_item(a, &it));
            });
        }
        m.sep();
        let mut folders = crate::menu::Menu::new();
        let t = targets.clone();
        folders.action("New Folder", move || {
            crate::app::with(|a| {
                a.tray.selected = t.iter().map(|i| i.id.clone()).collect();
                group_selection(a);
            });
        });
        if !groups.is_empty() {
            folders.sep();
        }
        for (gid, name, all_in) in groups.clone() {
            let t = targets.clone();
            folders.toggle(name, all_in, move || {
                crate::app::with(|a| move_to_group(a, &gid, &t));
            });
        }
        m.sub("Move to Folder", folders);
        if any_grouped {
            let t = targets.clone();
            m.action("Take Out of Folder", move || {
                crate::app::with(|a| remove_from_group(a, &t));
            });
        }
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
                let list = image_ids(a);
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
    app.tray.dragged = items.iter().map(|i| i.id.clone()).collect();
    app.tray.dropped_inside = false;
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
            a.tray.dragged.clear();
            a.tray.drop_target = None;
            a.island.mouse_down_inside = false;
            island::pointer_moved(a);
            if effect != crate::dragdrop::effect_none() && !std::mem::take(&mut a.tray.dropped_inside) {
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
                let list = image_ids(app);
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
    // Folders aren't selectable: keep going the same way to the next image.
    let mut next = next;
    let step: isize = match v {
        VK_LEFT => -1,
        VK_RIGHT => 1,
        VK_UP => -(COLUMNS as isize),
        _ => COLUMNS as isize,
    };
    while folder_gid(&app.tray.visible[next]).is_some() {
        let k = next as isize + step;
        if k < 0 || k >= n as isize {
            break;
        }
        next = k as usize;
    }
    if folder_gid(&app.tray.visible[next]).is_some() {
        return;
    }
    let id = app.tray.visible[next].clone();
    if shift {
        let a = app.tray.anchor.unwrap_or(next);
        let (lo, hi) = (a.min(next), a.max(next));
        app.tray.selected = app.tray.visible[lo..=hi].iter().filter(|id| folder_gid(id).is_none()).cloned().collect();
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
// Folders (FolderTileView + TrayController's folder motion)
// ---------------------------------------------------------------------------

/// Folder colours: one place to change (e.g. to Finder blue).
const BACK_TOP: Color = Color::gray(0.78, 1.0);
const BACK_BOTTOM: Color = Color::gray(0.66, 1.0);
const FRONT_TOP: Color = Color::gray(0.97, 1.0);
const FRONT_BOTTOM: Color = Color::gray(0.84, 1.0);

/// While a folder is open, everything that isn't that folder or its images fades back.
fn is_dimmed(app: &App, id: &str) -> bool {
    if searching(app) {
        return false;
    }
    let open: HashSet<&str> = app.tray.out_count.iter().filter(|(_, n)| **n > 0).map(|(g, _)| g.as_str()).collect();
    if open.is_empty() {
        return false;
    }
    match folder_gid(id) {
        Some(gid) => !open.contains(gid),
        None => app.model.group_of.get(id).map(|g| !open.contains(g.as_str())).unwrap_or(true),
    }
}

/// View-space point where a folder's papers sit (its mouth).
fn folder_mouth(app: &App, gid: &str) -> Option<Pt> {
    let fid = format!("{FOLDER}{gid}");
    let r = app.tray.last_rects.get(&fid).copied().or_else(|| app.tray.index_of.get(&fid).map(|i| slot(app, *i)))?;
    let r = to_view(app, r);
    Some(pt(r.cx(), r.y + r.h * 0.5))
}

/// An image flying out of (emerge) or back into (fold) its folder:
/// (progress 0 = at the mouth … 1 = in place, opacity, offset to the mouth).
fn folder_motion(app: &mut App, id: &str, r: Rect) -> (f32, f32, Pt) {
    let now = util::mono();
    let gid = app.model.group_of.get(id).cloned();
    let offset = gid.as_deref().and_then(|g| folder_mouth(app, g)).map(|m| pt(m.x - r.cx(), m.y - r.cy())).unwrap_or(pt(0.0, 0.0));
    if let Some(start) = app.tray.folding.get(id).copied() {
        let t = (now - start) as f32;
        let p = crate::anim::cubic_bezier(0.5, 0.0, 0.2, 1.0, t / 0.34);
        let op = 1.0 - ((t - 0.2) / 0.14).clamp(0.0, 1.0);
        return (1.0 - p, op, offset);
    }
    if let Some(start) = app.tray.emerge.get(id).copied() {
        let k = app.anims.get(key_id("tile.emerge", id)).unwrap_or(1.0);
        let op = (((now - start) / 0.18) as f32).clamp(0.0, 1.0);
        return (k, op, offset);
    }
    (1.0, 1.0, offset)
}

/// Opens a closed folder (images come out one by one and line up after it)
/// or closes an open one (they fly back in one by one).
pub fn toggle_folder(app: &mut App, gid: String) {
    if app.tray.animating.contains(&gid) || searching(app) {
        return;
    }
    let total = members(app, &gid).len();
    if total == 0 {
        island::flash(app, "This folder is empty");
        return;
    }
    let out = app.tray.out_count.get(&gid).copied().unwrap_or(0);
    app.tray.animating.insert(gid.clone());
    if out < total {
        open_step(app, gid, total);
    } else {
        close_step(app, gid, out);
    }
}

fn open_step(app: &mut App, gid: String, total: usize) {
    let out = app.tray.out_count.get(&gid).copied().unwrap_or(0);
    let list = members(app, &gid);
    if out >= total.min(list.len()) {
        app.tray.animating.remove(&gid);
        return;
    }
    app.tray.out_count.insert(gid.clone(), out + 1);
    let id = list[out].id.clone();
    app.tray.emerge.insert(id.clone(), util::mono());
    app.anims.play(key_id("tile.emerge", &id), 0.0, 1.0, Curve::Spring { mass: 0.9, stiffness: 210.0, damping: 17.0, delay: 0.0 });
    apply(app);
    app.request_frame();
    // Long folders speed up so opening never drags on.
    let gap = if total > 12 { 0.035 } else { 0.07 };
    ui::after(gap, move || {
        crate::app::with(|a| open_step(a, gid, total));
    });
}

/// The next image starts flying in once the previous one has left.
fn close_step(app: &mut App, gid: String, expected: usize) {
    if !app.tray.animating.contains(&gid) {
        return;
    }
    if expected == 0 {
        // Wait for the last fold to land before allowing another toggle.
        ui::after(0.36, move || {
            crate::app::with(|a| {
                a.tray.animating.remove(&gid);
            });
        });
        return;
    }
    let list = members(app, &gid);
    let Some(member) = list.get(expected - 1).map(|i| i.id.clone()) else {
        app.tray.out_count.insert(gid.clone(), expected.saturating_sub(1).min(list.len()));
        apply(app);
        close_step(app, gid, expected.saturating_sub(1).min(list.len()));
        return;
    };
    let on_screen = app.tray.last_rects.get(&member).map(|r| to_view(app, *r).intersects(&app.tray.view)).unwrap_or(false);
    let g2 = gid.clone();
    let finish = move |a: &mut App| {
        let cur = a.tray.out_count.get(&g2).copied().unwrap_or(0);
        a.tray.out_count.insert(g2.clone(), cur.min(expected - 1));
        apply(a);
        a.tray.folding.remove(&member);
    };
    if on_screen {
        let id = list[expected - 1].id.clone();
        app.tray.folding.insert(id, util::mono());
        app.request_frame();
        ui::after(0.34, move || {
            crate::app::with(finish);
        });
    } else {
        finish(app);
    }
    let gap = if expected > 12 { 0.035 } else { 0.07 };
    ui::after(gap, move || {
        crate::app::with(|a| close_step(a, gid, expected - 1));
    });
}

fn reload_groups(app: &mut App) {
    app.model.reload(&[]);
    model_changed(app);
    island::invalidate(app);
}

fn next_group_name(app: &App) -> String {
    let mut k = app.model.groups.len() + 1;
    loop {
        let name = format!("Group {k}");
        if !app.model.groups.iter().any(|g| g.name == name) {
            return name;
        }
        k += 1;
    }
}

/// Puts the selected images in a new folder at the front of the grid.
pub fn group_selection(app: &mut App) {
    let items = selected_items(app);
    if items.is_empty() {
        return;
    }
    let name = next_group_name(app);
    let ids: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
    let group = vault::create_group(&name, &ids);
    app.tray.selected.clear();
    app.tray.out_count.insert(group.id, 0);
    reload_groups(app);
    scroll_to(app, 0.0, true);
    island::flash(app, &format!("{} in {name}", items.len()));
}

fn move_to_group(app: &mut App, gid: &str, items: &[VaultItem]) {
    let ids: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
    vault::add_to_group(gid, &ids);
    app.tray.selected.clear();
    reload_groups(app);
    let name = app.model.group(gid).map(|g| g.name.clone()).unwrap_or_else(|| "folder".into());
    island::flash(app, &format!("Moved {} to {name}", items.len()));
}

fn remove_from_group(app: &mut App, items: &[VaultItem]) {
    let ids: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
    vault::remove_from_groups(&ids);
    app.tray.selected.clear();
    reload_groups(app);
    island::flash(app, &if items.len() == 1 { "Taken out of the folder".to_string() } else { format!("{} taken out of the folder", items.len()) });
}

fn ungroup(app: &mut App, gid: &str) {
    app.tray.out_count.remove(gid);
    vault::delete_group(gid);
    reload_groups(app);
    island::flash(app, "Folder removed · images kept");
}

fn folder_menu(app: &mut App, gid: String, at: windows::Win32::Foundation::POINT) {
    let list = members(app, &gid);
    let is_open = app.tray.out_count.get(&gid).copied().unwrap_or(0) > 0;
    ui::defer(move || {
        let mut m = crate::menu::Menu::new();
        let g = gid.clone();
        m.action(if is_open { "Close Folder" } else { "Open Folder" }, move || {
            crate::app::with(|a| toggle_folder(a, g));
        });
        let l = list.clone();
        m.action("Copy All Images", move || {
            crate::app::with(|a| copy(a, l));
        });
        let l = list.clone();
        m.action("Select All Inside", move || {
            crate::app::with(|a| {
                let shown: Vec<String> = l.iter().filter(|i| a.tray.index_of.contains_key(&i.id)).map(|i| i.id.clone()).collect();
                if shown.len() < l.len() {
                    island::flash(a, "Open the folder to select its images");
                }
                a.tray.selected = shown.into_iter().collect();
                island::invalidate(a);
            });
        });
        m.sep();
        let g = gid.clone();
        m.action("Rename…", move || {
            crate::app::with(|a| begin_rename(a, &g));
        });
        let g = gid.clone();
        m.action("Remove Folder (Keep Images)", move || {
            crate::app::with(|a| ungroup(a, &g));
        });
        crate::app::popup_menu(m, Some((at.x, at.y)));
    });
}

// -- rename (inline field over the folder) -----------------------------------------

fn begin_rename(app: &mut App, gid: &str) {
    let Some(g) = app.model.group(gid) else { return };
    let mut f = crate::textfield::TextField::new();
    f.set(&g.name);
    f.select_all();
    f.blink_epoch = util::mono();
    app.tray.renaming = Some((gid.to_string(), f));
    app.island.search_focused = false;
    island::schedule_blink(app);
    island::invalidate(app);
}

pub fn commit_rename(app: &mut App) {
    let Some((gid, f)) = app.tray.renaming.take() else { return };
    let name = f.text.trim().to_string();
    if !name.is_empty() && app.model.group(&gid).map(|g| g.name != name).unwrap_or(false) {
        vault::rename_group(&gid, &name);
        reload_groups(app);
    }
    island::invalidate(app);
}

/// Keys while renaming: Enter keeps, Esc cancels, the rest edits.
pub fn rename_key(app: &mut App, vk: u16) -> bool {
    if app.tray.renaming.is_none() {
        return false;
    }
    match VIRTUAL_KEY(vk) {
        VK_RETURN => commit_rename(app),
        VK_ESCAPE => {
            app.tray.renaming = None;
        }
        _ => {
            if let Some((_, f)) = app.tray.renaming.as_mut() {
                if let crate::textfield::KeyResult::Pass = f.key(vk) {
                    return false;
                }
            }
        }
    }
    island::invalidate(app);
    true
}

pub fn rename_char(app: &mut App, c: char) -> bool {
    let Some((_, f)) = app.tray.renaming.as_mut() else { return false };
    if c != '\r' && c != '\u{1b}' {
        f.char(c);
    }
    island::invalidate(app);
    true
}

pub fn rename_press(app: &mut App, field: Rect, p: Pt, shift: bool, clicks: u32) {
    if let Some((_, f)) = app.tray.renaming.as_mut() {
        let g = crate::gfx::gfx();
        f.mouse_down(&g, &crate::views::SEARCH_FONT, field, p.x, shift, clicks);
    }
    island::invalidate(app);
}

pub fn rename_drag(app: &mut App, field: Rect, p: Pt) {
    if let Some((_, f)) = app.tray.renaming.as_mut() {
        let g = crate::gfx::gfx();
        f.mouse_drag(&g, &crate::views::SEARCH_FONT, field, p.x);
    }
    island::invalidate(app);
}

pub fn rename_release(app: &mut App) {
    if let Some((_, f)) = app.tray.renaming.as_mut() {
        f.mouse_up();
    }
}

// -- drag a tile onto a folder (add) or onto an image (new folder) ---------------

/// An in-grid drag over the island at `p`: lights up the folder or image
/// under it. False when it isn't a place to drop.
pub fn internal_drag_over(app: &mut App, p: Pt) -> bool {
    let ok = (|| {
        if app.island.tab != crate::island::Tab::Vault || app.island.state != State::Expanded || app.tray.preview.open || app.tray.dragged.is_empty() {
            return None;
        }
        let (_, id) = tile_at(app, p)?;
        if let Some(gid) = folder_gid(&id) {
            // Already all in this folder: nothing to do.
            if app.tray.dragged.iter().all(|d| app.model.group_of.get(d).map(String::as_str) == Some(gid)) {
                return None;
            }
        } else if app.tray.dragged.contains(&id) {
            return None;
        }
        Some(id)
    })();
    set_drop_target(app, ok.clone());
    ok.is_some()
}

fn set_drop_target(app: &mut App, id: Option<String>) {
    if id == app.tray.drop_target {
        return;
    }
    if let Some(t) = &id {
        if folder_gid(t).is_none() {
            pop(app, "tile.card.pop", t, 0.94);
        }
    }
    app.tray.drop_target = id;
    island::invalidate(app);
}

pub fn internal_drag_leave(app: &mut App) {
    set_drop_target(app, None);
}

pub fn internal_drop(app: &mut App, p: Pt) -> bool {
    if !internal_drag_over(app, p) {
        return false;
    }
    let Some(target) = app.tray.drop_target.take() else { return false };
    let dragged: Vec<VaultItem> = app.tray.dragged.iter().filter_map(|id| app.model.get(id).cloned()).collect();
    app.tray.dropped_inside = true;
    app.tray.selected.clear();
    if let Some(gid) = folder_gid(&target) {
        let gid = gid.to_string();
        app.anims.play(key_id("fold.click", &gid), 1.12, 1.0, Curve::POP);
        move_to_group(app, &gid, &dragged);
    } else if let Some(gid) = app.model.group_of.get(&target).cloned() {
        // Dropped on an image that's out of an open folder: join that folder.
        move_to_group(app, &gid, &dragged);
    } else {
        let name = next_group_name(app);
        let mut ids = vec![target.clone()];
        ids.extend(dragged.iter().map(|i| i.id.clone()));
        let group = vault::create_group(&name, &ids);
        app.tray.out_count.insert(group.id, 0);
        reload_groups(app);
        scroll_to(app, 0.0, true);
        island::flash(app, &format!("{} in {name}", dragged.len() + 1));
    }
    true
}

// -- drawing ------------------------------------------------------------------------

fn folder_tile(cv: &mut Canvas, app: &mut App, gid: &str, r: Rect, px: u32, active: bool) {
    let Some(group) = app.model.group(gid).cloned() else { return };
    let fid = format!("{FOLDER}{gid}");
    let list = members(app, gid);
    let total = list.len();
    let inside = total - app.tray.out_count.get(gid).copied().unwrap_or(0).min(total);
    let open = inside == 0 && total > 0;
    let hovered = active && app.tray.hover.as_deref() == Some(fid.as_str()) && app.tray.lasso.is_none();
    let drop = app.tray.drop_target.as_deref() == Some(fid.as_str());
    let ease = Curve::mac(0.24);
    let paper_ease = Curve::mac(0.32);
    // Body: scale on hover / drop, pops on clicks and paper changes.
    let shown = inside.min(3);
    let before = app.tray.folder_shown.insert(gid.to_string(), shown);
    if let Some(b) = before {
        if b != shown {
            app.anims.play(key_id("fold.pop", gid), if shown < b { 0.94 } else { 1.06 }, 1.0, Curve::POP);
        }
    }
    let pop_v = app.anims.get(key_id("fold.pop", gid)).unwrap_or(1.0) * app.anims.get(key_id("fold.click", gid)).unwrap_or(1.0);
    let body_s = app.anims.drive(key_id("fold.s", gid), if drop { 1.08 } else if hovered { 1.03 } else { 1.0 }, ease) * pop_v;
    let dim = app.anims.drive(key_id("fold.dim", gid), if is_dimmed(app, &fid) && !hovered && !drop { 0.3 } else { 1.0 }, ease);
    let flap = app.anims.drive(key_id("fold.flap", gid), if drop { 0.38 } else if open { 0.5 } else { 0.0 }, ease);
    let hov_v = app.anims.drive(key_id("fold.h", gid), if hovered { 1.0 } else { 0.0 }, ease).clamp(0.0, 1.0);
    let open_v = app.anims.drive(key_id("fold.o", gid), if open { 1.0 } else { 0.0 }, ease).clamp(0.0, 1.0);
    let drop_v = app.anims.drive(key_id("fold.d", gid), if drop { 1.0 } else { 0.0 }, ease).clamp(0.0, 1.0);

    cv.save();
    cv.opacity(dim.clamp(0.0, 1.0));
    cv.scale_about(body_s, r.cx(), r.cy());
    // Ring.
    let bg = Color::white(0.03 + 0.02 * hov_v + 0.04 * open_v);
    cv.fill_round(r, 10.0, bg);
    let ring_c = theme::LINE.lerp(theme::LINE2, hov_v.max(open_v)).lerp(theme::PAPER, drop_v);
    cv.stroke_round(r, 10.0, 1.0 + drop_v, ring_c);

    let (w, h) = (r.w, r.h);
    let inset = w * 0.1;
    let fw = w - inset * 2.0;
    let bottom = h * 0.12;
    let back_h = h * 0.58;
    let front_h = h * 0.42;
    let (x0, x1) = (r.x + inset, r.x + inset + fw);
    let (y0, y1) = (r.b() - bottom - back_h, r.b() - bottom);
    // Back panel with the tab on the left.
    let (tab_w, tab_h, rad) = (fw * 0.38, h * 0.07, 4.0);
    let mut b = cv.g.path();
    b.move_to(pt(x0, y1 - rad));
    b.line_to(pt(x0, y0 - tab_h + rad));
    b.quad_to(pt(x0, y0 - tab_h), pt(x0 + rad, y0 - tab_h));
    b.line_to(pt(x0 + tab_w - rad, y0 - tab_h));
    b.quad_to(pt(x0 + tab_w, y0 - tab_h), pt(x0 + tab_w + rad, y0));
    b.line_to(pt(x1 - rad, y0));
    b.quad_to(pt(x1, y0), pt(x1, y0 + rad));
    b.line_to(pt(x1, y1 - rad));
    b.quad_to(pt(x1, y1), pt(x1 - rad, y1));
    b.line_to(pt(x0 + rad, y1));
    b.quad_to(pt(x0, y1), pt(x0, y1 - rad));
    b.close();
    let back: ID2D1Geometry = b.finish().cast().unwrap();
    cv.fill_gradient_v(rect(x0, y0 - tab_h, fw, back_h + tab_h), &[(0.0, BACK_TOP), (1.0, BACK_BOTTOM)], Some(&back));

    // Papers stand up out of the folder; they lift out as images leave.
    let (pw, ph) = (fw * 0.6, back_h * 0.8);
    let angles = [-0.12f32, 0.03, 0.14];
    for (i, angle) in angles.iter().enumerate() {
        let visible = i < shown;
        let vis = app.anims.drive(key_id(&format!("fold.pv{i}"), gid), if visible { 1.0 } else { 0.0 }, paper_ease);
        let lift_t = if (hovered || drop) && visible { 3.0 + i as f32 * if drop { 3.0 } else { 1.5 } } else { 0.0 };
        let lift = app.anims.drive(key_id(&format!("fold.pl{i}"), gid), lift_t, paper_ease);
        if vis <= 0.002 {
            continue;
        }
        let cx = r.cx() + (i as f32 - 1.0) * fw * 0.11;
        let cy = r.b() - (bottom + front_h * 0.5 + ph / 2.0 + i as f32 * 1.5) - lift - (1.0 - vis) * h * 0.35;
        let pr = rect(cx - pw / 2.0, cy - ph / 2.0, pw, ph);
        cv.save();
        cv.opacity(vis.clamp(0.0, 1.0));
        cv.transform(crate::gfx::rotate_about(-angle.to_degrees(), cx, cy));
        cv.fill_round(pr.offset(0.0, 1.0), 2.5, Color::black(0.18));
        cv.fill_round(pr, 2.5, Color::white(1.0));
        if let Some(item) = list.get(i) {
            let path = vault::file_path(item);
            if let Some(bmp) = app.thumbs.get(&item.id, path, item.hash.as_deref(), px, Kind::Tile) {
                let inner = pr.inset(1.5, 1.5);
                clip_round(cv, inner, 1.5);
                cv.image_fill(&bmp, inner, 1.0);
                cv.pop_layer();
            }
        }
        cv.restore();
    }

    // Front panel; an open (empty) folder tips its flap forward.
    let f = rect(r.x + inset - 1.0, r.b() - bottom - front_h, fw + 2.0, front_h);
    let (sa, ca) = (flap.sin(), flap.cos());
    let widen = 220.0 / (220.0 - front_h * sa).max(60.0);
    let top_y = f.b() - front_h * ca;
    let half = f.w / 2.0;
    let (tl, tr) = (pt(f.cx() - half * widen, top_y), pt(f.cx() + half * widen, top_y));
    let fr = 4.5f32.min(front_h * ca / 2.0).max(0.5);
    let mut fp = cv.g.path();
    fp.move_to(pt(f.x, f.b() - fr));
    fp.line_to(pt(tl.x, tl.y + fr));
    fp.quad_to(tl, pt(tl.x + fr, tl.y));
    fp.line_to(pt(tr.x - fr, tr.y));
    fp.quad_to(tr, pt(tr.x, tr.y + fr));
    fp.line_to(pt(f.r(), f.b() - fr));
    fp.quad_to(pt(f.r(), f.b()), pt(f.r() - fr, f.b()));
    fp.line_to(pt(f.x + fr, f.b()));
    fp.quad_to(pt(f.x, f.b()), pt(f.x, f.b() - fr));
    fp.close();
    let front: ID2D1Geometry = fp.finish().cast().unwrap();
    cv.fill_gradient_v(rect(tl.x.min(f.x), top_y, (tr.x - tl.x).max(f.w), f.b() - top_y), &[(0.0, FRONT_TOP), (1.0, FRONT_BOTTOM)], Some(&front));
    let edge_a = (1.0 - flap / 0.5).clamp(0.0, 1.0);
    if edge_a > 0.01 {
        cv.line(pt(tl.x + 3.0, top_y + 0.6), pt(tr.x - 3.0, top_y + 0.6), 0.8, Color::white(0.9 * edge_a), 0);
    }
    // Name and count.
    let renaming = app.tray.renaming.as_ref().map(|(g, _)| g == gid).unwrap_or(false);
    if !renaming {
        let nf = Font::sans(9.5, Weight::Semibold);
        cv.text(&group.name, &nf, Color::gray(0.12, 1.0), rect(f.x + 4.0, r.b() - bottom - front_h * 0.42 - 12.0, f.w - 8.0, 12.0), Align::Center, Trim::Tail);
    }
    let count = if total == 0 {
        "Empty".to_string()
    } else if inside == total {
        format!("{total}")
    } else if inside == 0 {
        format!("{total} out")
    } else {
        format!("{inside} of {total}")
    };
    let cf = Font::mono(8.5, Weight::Medium);
    cv.text(&count, &cf, Color::gray(0.35, 1.0), rect(f.x + 4.0, r.b() - bottom - front_h * 0.12 - 11.0, f.w - 8.0, 11.0), Align::Center, Trim::Tail);
    cv.restore();

    // Inline rename field, over the folder's front.
    if renaming {
        let fw2 = (r.w + 40.0).max(120.0);
        let mut field = rect(r.cx() - fw2 / 2.0, r.b() - 26.0, fw2, 22.0);
        field.x = field.x.clamp(app.tray.view.x + 4.0, (app.tray.view.r() - field.w - 4.0).max(app.tray.view.x + 4.0));
        cv.fill_round(field, 6.0, Color::gray(0.12, 1.0));
        cv.stroke_round(field, 6.0, 1.0, Color::white(0.35));
        let inner = field.inset(7.0, 0.0);
        if active {
            app.island.hits.push(Hit { rect: field, id: key("fold.rename"), tip: None, act: Act::FolderRename(inner), text: true });
        }
        if let Some((_, tf)) = app.tray.renaming.as_mut() {
            let mut tf = std::mem::take(tf);
            tf.draw(cv, &crate::views::SEARCH_FONT, Color::white(1.0), inner, true, "Folder name", Color::white(0.3));
            if let Some((_, slot)) = app.tray.renaming.as_mut() {
                *slot = tf;
            }
        }
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
    app.tray.preview.thumb = app.thumbs.peek(&id, item.hash.as_deref(), thumb_px(app), Kind::Tile);
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
