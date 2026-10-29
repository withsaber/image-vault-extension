//! AppController: owns every component, the global hotkeys, the settings
//! menu, the clipboard / screenshot watchers and the frame clock.

use std::cell::RefCell;
use std::path::PathBuf;

use windows::Win32::Foundation::*;
use windows::Win32::System::Com::IDataObject;
use windows::Win32::System::DataExchange::AddClipboardFormatListener;
use windows::Win32::System::Ole::DROPEFFECT;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::anim::Anims;
use crate::cliphistory::History;
use crate::dragdrop::{Phase, Target};
use crate::imaging::Raw;
use crate::island::{self, Island, State};
use crate::menu::Menu;
use crate::nowplaying::NowPlaying;
use crate::settings::{Position, Settings, autostart};
use crate::thumbs::Thumbs;
use crate::tooltip::Tooltip;
use crate::tray::{self, Tray};
use crate::{capture, clipboard, hotkeys, ingest, monitors, ocr, screenocr, statusitem, ui, util, vault};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct App {
    pub settings: Settings,
    pub model: vault::Model,
    pub island: Island,
    pub tray: Tray,
    pub clips: History,
    pub np: NowPlaying,
    pub anims: Anims,
    pub thumbs: Thumbs,
    pub tip: Tooltip,
    pub ocr: ocr::Indexer,
    pub hotkeys: hotkeys::Hotkeys,
    pub captures: capture::Captures,
    pub picker: Option<screenocr::Picker>,
    pub status: statusitem::StatusItem,
    last_tick: f64,
    clip_timer: Option<ui::TimerId>,
    seen_shots: std::collections::HashSet<PathBuf>,
    last_np_frame: f64,
    last_gc: f64,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Run `f` with the app (None if it's busy — a re-entrant message).
pub fn with<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| {
        let mut b = match a.try_borrow_mut() { Ok(b) => b, Err(_) => { util::trace("with: busy"); return None; } };
        let app = b.as_mut()?;
        Some(f(app))
    })
}

impl App {
    pub fn request_frame(&self) {
        ui::animate(true);
    }

    pub fn on_device_lost(&mut self) {
        crate::gfx::reset();
        crate::icons::clear_cache();
        self.thumbs.clear();
        self.island.dirty = true;
        self.captures.device_lost();
        self.request_frame();
    }
}

// ---------------------------------------------------------------------------
// Startup
// ---------------------------------------------------------------------------

fn build() -> App {
    let settings = Settings::load();
    App {
        model: vault::Model::new(),
        island: Island::create(),
        tray: Tray::new(),
        clips: History::load(),
        np: NowPlaying::new(),
        anims: Anims::new(),
        thumbs: Thumbs::new(),
        tip: Tooltip::new(),
        ocr: ocr::Indexer::default(),
        hotkeys: hotkeys::Hotkeys::default(),
        captures: capture::Captures::default(),
        picker: None,
        status: statusitem::StatusItem::default(),
        settings,
        last_tick: util::mono(),
        clip_timer: None,
        seen_shots: Default::default(),
        last_np_frame: 0.0,
        last_gc: 0.0,
    }
}

/// App without windows on screen, hotkeys or watchers (`--snapshot`).
pub fn init_headless() {
    let app = build();
    APP.with(|a| *a.borrow_mut() = Some(app));
}

pub fn init() {
    remove_chrome_bridge();
    let settings = Settings::load();
    let app = App {
        model: vault::Model::new(),
        island: Island::create(),
        tray: Tray::new(),
        clips: History::load(),
        np: NowPlaying::new(),
        anims: Anims::new(),
        thumbs: Thumbs::new(),
        tip: Tooltip::new(),
        ocr: ocr::Indexer::default(),
        hotkeys: hotkeys::Hotkeys::default(),
        captures: capture::Captures::default(),
        picker: None,
        status: statusitem::StatusItem::default(),
        settings,
        last_tick: util::mono(),
        clip_timer: None,
        seen_shots: Default::default(),
        last_np_frame: 0.0,
        last_gc: 0.0,
    };
    APP.with(|a| *a.borrow_mut() = Some(app));
    with(|a| {
        island::layout(a);
        tray::apply(a);
        hotkeys::register_all(a);
        statusitem::install(a);
        unsafe {
            let _ = AddClipboardFormatListener(ui::msg_hwnd());
        }
        ocr::pump(a);
        a.request_frame();
    });
    start_model_poll();
    island::schedule_trim();
    start_screenshot_watcher();
}

/// VaultModel polls vault.json's modification date once a second.
fn start_model_poll() {
    ui::after(1.0, || {
        with(|a| {
            reload_model_if_changed(a);
        });
        start_model_poll();
    });
}

pub fn reload_model_if_changed(app: &mut App) {
    if app.model.changed_on_disk() {
        app.model.reload(&[]);
        tray::model_changed(app);
        ocr::pump(app);
        island::invalidate(app);
    }
}

/// Earlier builds may have registered a browser native-messaging host.
fn remove_chrome_bridge() {
    use windows::Win32::System::Registry::*;
    for base in [
        "Software\\Google\\Chrome\\NativeMessagingHosts\\com.imagevault.bridge",
        "Software\\Chromium\\NativeMessagingHosts\\com.imagevault.bridge",
        "Software\\Microsoft\\Edge\\NativeMessagingHosts\\com.imagevault.bridge",
        "Software\\BraveSoftware\\Brave-Browser\\NativeMessagingHosts\\com.imagevault.bridge",
        "Software\\Vivaldi\\NativeMessagingHosts\\com.imagevault.bridge",
    ] {
        let w = util::wide(base);
        unsafe {
            let _ = RegDeleteTreeW(HKEY_CURRENT_USER, windows::core::PCWSTR(w.as_ptr()));
        }
    }
}

// ---------------------------------------------------------------------------
// Dispatcher hooks
// ---------------------------------------------------------------------------

pub fn tick() {
    with(|a| frame(a));
}

fn frame(app: &mut App) {
    let now = util::mono();
    let dt = ((now - app.last_tick) as f32).clamp(0.0, 0.05);
    app.last_tick = now;
    app.anims.now = now;
    let moving = app.anims.tick(dt);
    let continuous = island::animating(app);
    let np_frame = crate::nowplaying::wants_frames(app) && now - app.last_np_frame >= 1.0 / 30.0;
    if np_frame {
        app.last_np_frame = now;
    }
    if app.island.dirty || moving || continuous || np_frame {
        app.island.dirty = false;
        island::render(app);
    }
    capture::frame(app, moving);
    screenocr::frame(app);
    if now - app.last_gc > 2.0 {
        app.last_gc = now;
        app.anims.gc(6.0);
    }
    let keep = app.anims.any_active()
        || island::animating(app)
        || crate::nowplaying::wants_frames(app)
        || app.island.dirty
        || capture::animating(app);
    ui::animate(keep);
}

pub fn mouse(evs: Vec<ui::MouseEv>) {
    with(|a| {
        for ev in evs {
            island::mouse_event(a, ev);
            screenocr::global_mouse(a, ev);
        }
    });
}

pub fn message(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> Option<LRESULT> {
    match msg {
        WM_HOTKEY => {
            let id = w.0 as i32;
            ui::defer(move || hotkeys::fired(id));
            Some(LRESULT(0))
        }
        WM_CLIPBOARDUPDATE => {
            with(|a| {
                if let Some(t) = a.clip_timer.take() {
                    ui::cancel(t);
                }
                // Let the writer finish, then read.
                a.clip_timer = Some(ui::after(0.12, || {
                    ui::defer(clipboard_changed);
                }));
            });
            Some(LRESULT(0))
        }
        ui::WM_APP_TRAY => {
            let event = (l.0 & 0xffff) as u32;
            ui::defer(move || statusitem::event(event));
            Some(LRESULT(0))
        }
        ui::WM_APP_QUIT => {
            ui::defer(quit);
            Some(LRESULT(0))
        }
        ui::WM_APP_SHOW => {
            with(|a| {
                island::reassert(a);
                island::expand(a, false);
            });
            Some(LRESULT(0))
        }
        WM_DISPLAYCHANGE | WM_SETTINGCHANGE | WM_DPICHANGED => {
            ui::after(0.2, || {
                with(island::reassert);
            });
            None
        }
        WM_POWERBROADCAST => {
            ui::after(1.0, || {
                with(island::reassert);
            });
            None
        }
        _ => {
            if msg == statusitem::taskbar_created() {
                with(statusitem::install);
                return Some(LRESULT(0));
            }
            let _ = hwnd;
            None
        }
    }
}

pub fn on_drop(target: Target, phase: Phase, data: Option<&IDataObject>, p: POINT) -> DROPEFFECT {
    let r = with(|a| match target {
        Target::Island | Target::Sensor => island::drop_event(a, target, phase, data, p),
        Target::Editor(_) => crate::dragdrop::effect_none(),
    });
    ui::drain();
    r.unwrap_or(crate::dragdrop::effect_none())
}

pub fn thumb_ready(key: String, raw: Option<Raw>) {
    // The app is only busy during a re-entrant message; try again next loop.
    let busy = APP.with(|a| a.try_borrow_mut().is_err());
    if busy {
        ui::post(move || thumb_ready(key, raw));
        return;
    }
    with(|a| {
        a.thumbs.insert(key, raw);
        island::invalidate(a);
    });
}

// ---------------------------------------------------------------------------
// Vault actions
// ---------------------------------------------------------------------------

pub fn finish_ingest(app: &mut App, s: &ingest::Summary) {
    let ids: Vec<String> = s.added.iter().map(|i| i.id.clone()).collect();
    app.model.reload(&ids);
    tray::model_changed(app);
    ocr::pump(app);
    island::invalidate(app);
}

/// "Saved · N in vault" live activity after an automatic save.
pub fn announce(app: &mut App, s: &ingest::Summary) {
    let detail = format!("{} in vault", app.model.items.len());
    island::show_activity(app, s.first.clone(), "Saved", &detail);
}

pub fn delete_items(app: &mut App, ids: &[String]) {
    if let Some((token, removed)) = app.model.delete(ids) {
        tray::model_changed(app);
        ui::after(60.0, move || {
            with(|a| {
                a.model.expire(token, &removed);
                for id in &removed {
                    if a.model.get(id).is_none() {
                        a.thumbs.forget(id);
                    }
                }
            });
        });
    }
}

pub fn undo_delete(app: &mut App) -> usize {
    let n = app.model.undo_delete();
    tray::model_changed(app);
    n
}

pub fn copy_latest(app: &mut App) {
    let Some(item) = app.model.items.first().cloned() else {
        island::flash(app, "Vault is empty");
        return;
    };
    copy_item(app, item);
}

pub fn copy_item(app: &mut App, item: vault::VaultItem) {
    util::spawn("copy", move || {
        crate::imaging::com_init();
        clipboard::copy_items(&[item]);
    });
    island::flash(app, "Copied — Ctrl+V anywhere");
}

// ---------------------------------------------------------------------------
// Watchers
// ---------------------------------------------------------------------------

/// ClipboardWatcher.poll: history gets text and files (never ours, never
/// secrets); the vault gets images when "Save Copied Images" is on.
fn clipboard_changed() {
    let Some(snap) = clipboard::read() else { return };
    if snap.ours {
        return;
    }
    let app_name = if snap.secret { None } else { clipboard::front_app_name() };
    with(|a| {
        a.clip_timer = None;
        if !snap.secret && a.settings.clipboard_history {
            a.clips.record(&snap, app_name);
            island::invalidate(a);
        }
        if !a.settings.save_clipboard || snap.secret || !snap.files.is_empty() {
            return;
        }
        // Text that also carries a picture (Office, spreadsheets) counts as text.
        if snap.text.as_deref().map(|t| !t.trim().is_empty()).unwrap_or(false) {
            return;
        }
        let Some(img) = snap.image else { return };
        let c = ingest::Candidate { data: img, name: None, url: snap.url.clone() };
        ingest::save_async(vec![c], "clipboard", |s| {
            with(|a| {
                finish_ingest(a, &s);
                if !s.added.is_empty() {
                    announce(a, &s);
                }
            });
        });
    });
}

/// Imports new screenshots saved to the Screenshots folder (Win+PrtScn,
/// Snipping Tool auto-save) — ScreenshotWatcher.
fn start_screenshot_watcher() {
    use windows::Win32::Storage::FileSystem::*;
    use windows::Win32::UI::Shell::*;
    let folder = unsafe {
        SHGetKnownFolderPath(&FOLDERID_Screenshots, KNOWN_FOLDER_FLAG(0), None)
            .ok()
            .map(|p| {
                let s = p.to_string().unwrap_or_default();
                windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
                PathBuf::from(s)
            })
    }
    .or_else(|| std::env::var("USERPROFILE").ok().map(|h| PathBuf::from(h).join("Pictures").join("Screenshots")));
    let Some(folder) = folder else { return };
    let _ = std::fs::create_dir_all(&folder);
    util::spawn_low("screenshot-watch", move || unsafe {
        let w = util::wide_path(&folder);
        let Ok(h) = CreateFileW(
            windows::core::PCWSTR(w.as_ptr()),
            FILE_LIST_DIRECTORY.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        ) else {
            return;
        };
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let mut got = 0u32;
            let ok = ReadDirectoryChangesW(h, buf.as_mut_ptr() as *mut _, buf.len() as u32, false, FILE_NOTIFY_CHANGE_FILE_NAME, Some(&mut got), None, None);
            if ok.is_err() {
                break;
            }
            let mut off = 0usize;
            while got > 0 && off < got as usize {
                let info = &*(buf.as_ptr().add(off) as *const FILE_NOTIFY_INFORMATION);
                let len = info.FileNameLength as usize / 2;
                let name = String::from_utf16_lossy(std::slice::from_raw_parts(info.FileName.as_ptr(), len));
                if info.Action == FILE_ACTION_ADDED || info.Action == FILE_ACTION_RENAMED_NEW_NAME {
                    let path = folder.join(&name);
                    if crate::imaging::is_image_path(&path) {
                        ui::post(move || screenshot_added(path));
                    }
                }
                if info.NextEntryOffset == 0 {
                    break;
                }
                off += info.NextEntryOffset as usize;
            }
        }
    });
}

fn screenshot_added(path: PathBuf) {
    with(|a| {
        if !a.settings.save_screenshots || !a.seen_shots.insert(path.clone()) {
            return;
        }
        // Give the writer a moment to finish the file.
        ui::after(0.6, move || {
            util::spawn("screenshot-read", move || {
                let data = (0..5).find_map(|_| {
                    let d = std::fs::read(&path).ok().filter(|d| !d.is_empty());
                    if d.is_none() {
                        std::thread::sleep(std::time::Duration::from_millis(300));
                    }
                    d
                });
                let Some(data) = data else { return };
                let name = path.file_name().map(|n| n.to_string_lossy().to_string());
                crate::imaging::com_init();
                let s = ingest::persist(vec![ingest::Candidate { data, name, url: None }], "screenshot");
                ui::post(move || {
                    with(|a| {
                        finish_ingest(a, &s);
                        if !s.added.is_empty() {
                            announce(a, &s);
                        }
                    });
                });
            });
        });
    });
}

// ---------------------------------------------------------------------------
// Menus
// ---------------------------------------------------------------------------

/// Show a popup at a screen point (or the cursor). Keeps the island open.
pub fn popup_menu(m: Menu, at: Option<(i32, i32)>) {
    let (x, y) = at.unwrap_or_else(monitors::cursor);
    let owner = with(|a| {
        a.island.menu_open = true;
        a.tip.dismiss();
        a.island.hwnd
    });
    let Some(owner) = owner else { return };
    crate::menu::popup(m, owner, x, y);
    with(|a| {
        a.island.menu_open = false;
        island::pointer_moved(a);
    });
}

pub fn stats_title(app: &App) -> String {
    format!(
        "Image Vault {VERSION}  ·  {} images  ·  {}",
        app.model.items.len(),
        crate::theme::byte_count(app.model.total_bytes())
    )
}

/// Settings menu (gear in the island, or right-click it).
pub fn show_settings_menu(at: Option<(i32, i32)>) {
    ui::defer(move || {
        let Some(m) = with(build_settings_menu) else { return };
        popup_menu(m, at);
    });
}

fn build_settings_menu(app: &mut App) -> Menu {
    let mut m = Menu::new();
    m.disabled(stats_title(app));
    m.sep();

    let mut positions = Menu::new();
    for p in Position::ALL {
        positions.toggle(format!("Top {}", p.title()), app.settings.island_position == p, move || {
            with(|a| {
                a.settings.island_position = p;
                a.settings.save();
                island::layout(a);
            });
        });
    }
    m.sub("Island Position", positions);

    let mut displays = Menu::new();
    let mons = monitors::all();
    let current = app.island.monitor.as_ref().map(|m| m.device.clone());
    for mon in mons {
        let dev = mon.device.clone();
        displays.toggle(mon.name.clone(), Some(&mon.device) == current.as_ref(), move || {
            with(|a| {
                a.settings.island_display = Some(dev);
                a.settings.save();
                island::layout(a);
            });
        });
    }
    m.sub("Display", displays);
    m.toggle("Show Notch", app.settings.show_notch, || {
        with(|a| {
            a.settings.show_notch = !a.settings.show_notch;
            a.settings.save();
            let on = a.settings.show_notch;
            island::set_show_notch(a, on);
        });
    });
    m.sep();

    m.toggle("Save Copied Images", app.settings.save_clipboard, || {
        with(|a| {
            a.settings.save_clipboard = !a.settings.save_clipboard;
            a.settings.save();
        });
    });
    m.toggle("Save New Screenshots", app.settings.save_screenshots, || {
        with(|a| {
            a.settings.save_screenshots = !a.settings.save_screenshots;
            a.settings.save();
        });
    });
    m.toggle("Clipboard History", app.settings.clipboard_history, || {
        with(|a| {
            a.settings.clipboard_history = !a.settings.clipboard_history;
            a.settings.save();
            island::invalidate(a);
        });
    });
    m.toggle("Music Controls", app.settings.music_controls, || {
        with(|a| {
            let on = !a.settings.music_controls;
            crate::nowplaying::set_enabled(a, on);
        });
    });
    m.toggle("Open at Login", autostart::enabled(), || {
        let on = !autostart::enabled();
        if !autostart::set(on) {
            with(|a| island::flash(a, "Couldn't change login item"));
        }
    });
    m.sep();

    let shot = hotkeys::label(app, hotkeys::SHOT);
    let ocr_label = hotkeys::label(app, hotkeys::OCR);
    m.action_key("Take Screenshot", &shot, || {
        ui::after(0.15, || ui::defer(capture::take_screenshot));
    });
    m.action_key("Capture Text from Screen", &ocr_label, || {
        ui::after(0.15, || ui::defer(screenocr::toggle_ocr));
    });
    let mut shortcuts = Menu::new();
    for id in [hotkeys::SHOT, hotkeys::OCR] {
        let name = hotkeys::name(id);
        shortcuts.disabled(format!("{name}: {}", hotkeys::label(app, id)));
        shortcuts.action(format!("Change {name} Shortcut…"), move || hotkeys::change(id));
        shortcuts.action(format!("Reset to {}", hotkeys::default_label(id)), move || {
            with(|a| hotkeys::reset(a, id));
        });
        if !hotkeys::registered(app, id) {
            shortcuts.disabled("⚠ Taken by another app — pick a different one");
        }
        shortcuts.sep();
    }
    shortcuts.trim();
    m.sub("Shortcuts", shortcuts);

    let mut langs = Menu::new();
    let builtin = ocr::builtin_languages();
    let builtin_text = if builtin.is_empty() { "English".to_string() } else { builtin.join(", ") };
    langs.disabled(format!("Built in: {builtin_text}"));
    langs.sep();
    for l in ocr::LANGUAGES.iter() {
        let on = app.settings.ocr_extra_languages.iter().any(|c| c == l.code);
        let code = l.code.to_string();
        langs.toggle(l.name, on, move || {
            with(|a| {
                if on {
                    a.settings.ocr_extra_languages.retain(|c| *c != code);
                } else {
                    a.settings.ocr_extra_languages.push(code.clone());
                }
                a.settings.save();
                // The language key changed: everything is read again.
                ocr::pump(a);
            });
        });
    }
    if !ocr::tesseract_available() {
        langs.sep();
        langs.disabled("Extra languages engine missing from this build");
    }
    m.sub("OCR Languages", langs);
    m.sep();
    m.action("Open Vault Folder", || util::shell_open(&vault::images_dir().display().to_string()));
    m.action_key("Copy Latest Image", &hotkeys::label(app, hotkeys::COPY), || {
        with(copy_latest);
    });
    m.disabled(format!("Open / close: hover the island or {}", hotkeys::label(app, hotkeys::TOGGLE)));
    m.sep();
    m.action("Quit Image Vault", quit);
    m
}

pub fn quit() {
    with(|a| {
        statusitem::remove(a);
        hotkeys::unregister_all(a);
    });
    ui::quit();
}

/// Single-instance: a second launch asks the first to open the island.
pub fn signal_running_instance() -> bool {
    unsafe {
        let Ok(h) = FindWindowW(windows::core::w!("ImageVault.Dispatcher"), None) else { return false };
        if h.0.is_null() {
            return false;
        }
        let _ = PostMessageW(Some(h), ui::WM_APP_SHOW, WPARAM(0), LPARAM(0));
        true
    }
}

pub fn shift_down() -> bool {
    unsafe { GetKeyState(VK_SHIFT.0 as i32) < 0 }
}

pub fn state_is(app: &App, s: State) -> bool {
    app.island.state == s
}

/// Release every window surface and the graphics device before the process
/// exits; some GPU drivers otherwise keep a thread alive in the kernel and the
/// process never fully goes away.
pub fn shutdown() {
    let app = APP.with(|a| a.borrow_mut().take());
    drop(app);
    crate::icons::clear_cache();
    crate::gfx::reset();
}

/// `--quit`: ask a running instance to quit (like Quit Image Vault).
pub fn quit_running_instance() -> bool {
    unsafe {
        let Ok(h) = FindWindowW(windows::core::w!("ImageVault.Dispatcher"), None) else { return false };
        PostMessageW(Some(h), ui::WM_APP_QUIT, WPARAM(0), LPARAM(0)).is_ok()
    }
}
