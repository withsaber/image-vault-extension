//! Now Playing (NowPlaying.swift) through Windows' Global System Media
//! Transport Controls: title, artist, artwork, live position, seek. Polling
//! only runs while the tray is open, so the app is idle at rest. The bar and
//! waveform scrubber match the Mac UI.

use std::sync::mpsc::{Receiver, Sender, channel};

use windows::Media::Control::*;
use windows::Storage::Streams::DataReader;
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap1;
use windows::core::HSTRING;

use crate::anim::{key, motion};
use crate::app::App;
use crate::gfx::{Align, Canvas, Trim};
use crate::icons::{self, Icon};
use crate::imaging::{self, Raw};
use crate::island::{self, Act, Hit};
use crate::theme::{Color, Font, Pt, Rect, Weight, pt, rect};
use crate::{ui, util};

#[derive(Debug, Clone)]
pub struct Track {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub player: String,
    pub playing: bool,
    /// Position at `anchor` (monotonic seconds).
    pub elapsed: f64,
    pub duration: f64,
    pub anchor: f64,
    pub rate: f64,
}

impl Track {
    pub fn id(&self) -> String {
        format!("{}|{}|{}|{:?}", self.title, self.artist, self.player, self.duration)
    }
    pub fn elapsed_at(&self, now: f64) -> f64 {
        if !self.playing {
            return self.elapsed.min(self.duration.max(self.elapsed));
        }
        let v = self.elapsed + (now - self.anchor).max(0.0) * self.rate.max(0.0);
        if self.duration > 0.0 { v.min(self.duration) } else { v }
    }
    pub fn progress_at(&self, now: f64) -> f64 {
        if self.duration <= 0.0 {
            return 0.0;
        }
        (self.elapsed_at(now) / self.duration).clamp(0.0, 1.0)
    }
    pub fn subtitle(&self) -> String {
        if self.artist.is_empty() { self.album.clone() } else { self.artist.clone() }
    }
    fn same_data(&self, o: &Track) -> bool {
        self.title == o.title
            && self.artist == o.artist
            && self.album == o.album
            && self.player == o.player
            && self.playing == o.playing
            && (self.duration - o.duration).abs() < 0.01
            && (self.elapsed_at(util::mono()) - o.elapsed_at(util::mono())).abs() < 0.75
    }
}

enum Cmd {
    Read,
    Toggle,
    Next,
    Previous,
    Seek(f64),
}

struct Info {
    track: Track,
    art_key: String,
    art: Option<Vec<u8>>,
}

pub struct NowPlaying {
    pub track: Option<Track>,
    /// Last track shown, kept while the bar animates out.
    pub last: Option<Track>,
    pub artwork: Option<Raw>,
    art_bmp: Option<(u32, ID2D1Bitmap1)>,
    art_key: String,
    active: bool,
    timer: Option<ui::TimerId>,
    reading: bool,
    seek_hold_until: f64,
    pub drag: Option<f64>,
    tx: Option<Sender<Cmd>>,
    failures: u32,
    icons: std::collections::HashMap<String, Option<Raw>>,
    names: std::collections::HashMap<String, String>,
}

impl NowPlaying {
    pub fn new() -> NowPlaying {
        NowPlaying {
            track: None,
            last: None,
            artwork: None,
            art_bmp: None,
            art_key: String::new(),
            active: false,
            timer: None,
            reading: false,
            seek_hold_until: 0.0,
            drag: None,
            tx: None,
            failures: 0,
            icons: Default::default(),
            names: Default::default(),
        }
    }

    fn worker(&mut self) -> Option<&Sender<Cmd>> {
        if self.tx.is_none() {
            let (tx, rx) = channel::<Cmd>();
            util::spawn("nowplaying", move || run_worker(rx));
            self.tx = Some(tx);
        }
        self.tx.as_ref()
    }
}

fn ticks_to_secs(t: i64) -> f64 {
    t as f64 / 10_000_000.0
}

fn now_filetime() -> i64 {
    // 100 ns since 1601.
    ((util::now_ms() / 1000.0 + 11644473600.0) * 10_000_000.0) as i64
}

fn read_art(props: &GlobalSystemMediaTransportControlsSessionMediaProperties) -> Option<Vec<u8>> {
    let r = props.Thumbnail().ok()?;
    let stream = r.OpenReadAsync().ok()?.join().ok()?;
    let size = stream.Size().ok()? as u32;
    if size == 0 || size > 32 * 1024 * 1024 {
        return None;
    }
    let input = stream.GetInputStreamAt(0).ok()?;
    let reader = DataReader::CreateDataReader(&input).ok()?;
    reader.LoadAsync(size).ok()?.join().ok()?;
    let mut buf = vec![0u8; size as usize];
    reader.ReadBytes(&mut buf).ok()?;
    Some(buf)
}

fn run_worker(rx: Receiver<Cmd>) {
    imaging::com_init();
    let mut mgr: Option<GlobalSystemMediaTransportControlsSessionManager> = None;
    let mut last_art_key = String::new();
    while let Ok(cmd) = rx.recv() {
        if mgr.is_none() {
            mgr = GlobalSystemMediaTransportControlsSessionManager::RequestAsync().ok().and_then(|op| op.join().ok());
        }
        let session = mgr.as_ref().and_then(|m| m.GetCurrentSession().ok());
        match cmd {
            Cmd::Read => {
                let info = session.as_ref().and_then(|s| {
                    let props = s.TryGetMediaPropertiesAsync().ok()?.join().ok()?;
                    let title = props.Title().ok()?.to_string();
                    if title.is_empty() {
                        return None;
                    }
                    let artist = props.Artist().map(|h| h.to_string()).unwrap_or_default();
                    let album = props.AlbumTitle().map(|h| h.to_string()).unwrap_or_default();
                    let player = s.SourceAppUserModelId().map(|h| h.to_string()).unwrap_or_default();
                    let pb = s.GetPlaybackInfo().ok()?;
                    let playing = pb.PlaybackStatus().ok() == Some(GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing);
                    let rate = pb.PlaybackRate().ok().and_then(|r| r.Value().ok()).unwrap_or(1.0);
                    let tl = s.GetTimelineProperties().ok()?;
                    let start = tl.StartTime().map(|t| ticks_to_secs(t.Duration)).unwrap_or(0.0);
                    let end = tl.EndTime().map(|t| ticks_to_secs(t.Duration)).unwrap_or(0.0);
                    let pos = tl.Position().map(|t| ticks_to_secs(t.Duration)).unwrap_or(0.0);
                    let updated = tl.LastUpdatedTime().map(|d| d.UniversalTime).unwrap_or(0);
                    let age = if updated > 0 && playing { ticks_to_secs(now_filetime() - updated).max(0.0) } else { 0.0 };
                    let art_key = format!("{title}|{artist}|{album}");
                    let art = if art_key != last_art_key { read_art(&props) } else { None };
                    Some(Info {
                        track: Track {
                            title,
                            artist,
                            album,
                            player,
                            playing,
                            elapsed: (pos - start).max(0.0),
                            duration: (end - start).max(0.0),
                            anchor: util::mono() - age.min(36000.0),
                            rate: if playing && rate > 0.01 { rate } else { 1.0 },
                        },
                        art_key,
                        art,
                    })
                });
                if let Some(i) = &info {
                    last_art_key = i.art_key.clone();
                } else {
                    last_art_key.clear();
                }
                ui::post(move || {
                    crate::app::with(|a| consume(a, info));
                });
            }
            Cmd::Toggle => {
                if let Some(s) = &session {
                    let _ = s.TryTogglePlayPauseAsync().map(|op| op.join());
                }
            }
            Cmd::Next => {
                if let Some(s) = &session {
                    let _ = s.TrySkipNextAsync().map(|op| op.join());
                }
            }
            Cmd::Previous => {
                if let Some(s) = &session {
                    let _ = s.TrySkipPreviousAsync().map(|op| op.join());
                }
            }
            Cmd::Seek(secs) => {
                if let Some(s) = &session {
                    let start = s.GetTimelineProperties().ok().and_then(|t| t.StartTime().ok()).map(|t| t.Duration).unwrap_or(0);
                    let ticks = start + (secs * 10_000_000.0) as i64;
                    let _ = s.TryChangePlaybackPositionAsync(ticks).map(|op| op.join());
                }
            }
        }
    }
}

fn consume(app: &mut App, info: Option<Info>) {
    app.np.reading = false;
    let Some(info) = info else {
        if app.np.track.is_some() {
            app.np.last = app.np.track.take();
        }
        app.np.art_key.clear();
        app.np.artwork = None;
        app.np.art_bmp = None;
        island::invalidate(app);
        return;
    };
    app.np.failures = 0;
    if info.art_key != app.np.art_key {
        app.np.art_key = info.art_key.clone();
        app.np.artwork = None;
        app.np.art_bmp = None;
    }
    if let Some(bytes) = info.art {
        app.np.artwork = imaging::decode_bytes(&bytes, Some(96));
        app.np.art_bmp = None;
    }
    let mut next = info.track;
    // Right after a seek the system can still report the old position; keep
    // the requested one for a moment instead of snapping back.
    if util::mono() < app.np.seek_hold_until {
        if let Some(cur) = &app.np.track {
            if cur.id() == next.id() {
                next.elapsed = cur.elapsed_at(util::mono());
                next.anchor = util::mono();
            }
        }
    }
    let changed = app.np.track.as_ref().map(|t| !t.same_data(&next)).unwrap_or(true);
    if changed {
        app.np.track = Some(next.clone());
        app.np.last = Some(next);
        island::invalidate(app);
    }
}

/// Polling only runs while the tray is open (or opening).
pub fn set_active(app: &mut App, on: bool) {
    app.np.active = on;
    if on && app.settings.music_controls {
        refresh(app);
        if app.np.timer.is_none() {
            schedule(app);
        }
    } else if let Some(t) = app.np.timer.take() {
        ui::cancel(t);
    }
}

fn schedule(app: &mut App) {
    app.np.timer = Some(ui::after(1.0, || {
        crate::app::with(|a| {
            a.np.timer = None;
            if a.np.active && a.settings.music_controls {
                refresh(a);
                schedule(a);
            }
        });
    }));
}

pub fn refresh(app: &mut App) {
    if !app.settings.music_controls || app.np.reading {
        return;
    }
    app.np.reading = true;
    if let Some(tx) = app.np.worker() {
        if tx.send(Cmd::Read).is_err() {
            app.np.tx = None;
            app.np.reading = false;
        }
    }
}

pub fn set_enabled(app: &mut App, on: bool) {
    app.settings.music_controls = on;
    app.settings.save();
    if on {
        if app.np.active {
            set_active(app, true);
        }
    } else {
        if let Some(t) = app.np.timer.take() {
            ui::cancel(t);
        }
        app.np.track = None;
        app.np.art_key.clear();
        app.np.artwork = None;
        app.np.art_bmp = None;
    }
    island::invalidate(app);
}

fn send(app: &mut App, c: Cmd, refresh_after: f64) {
    if !app.settings.music_controls {
        return;
    }
    if let Some(tx) = app.np.worker() {
        let _ = tx.send(c);
    }
    ui::after(refresh_after, || {
        crate::app::with(refresh);
    });
}

pub fn play_pause(app: &mut App) {
    if let Some(t) = &mut app.np.track {
        t.elapsed = t.elapsed_at(util::mono());
        t.anchor = util::mono();
        t.playing = !t.playing;
    }
    send(app, Cmd::Toggle, 0.2);
    island::invalidate(app);
}

pub fn next(app: &mut App) {
    send(app, Cmd::Next, 0.2);
}

pub fn previous(app: &mut App) {
    send(app, Cmd::Previous, 0.2);
}

pub fn seek(app: &mut App, progress: f64) {
    let Some(t) = &mut app.np.track else { return };
    let target = t.duration * progress;
    t.elapsed = target;
    t.anchor = util::mono();
    app.np.seek_hold_until = util::mono() + 1.5;
    send(app, Cmd::Seek(target), 0.4);
    island::invalidate(app);
}

/// Waveform click / drag.
pub fn scrub(app: &mut App, wave: Rect, p: Pt, end: bool) {
    let x = ((p.x - wave.x) / wave.w.max(1.0)).clamp(0.0, 1.0) as f64;
    if end {
        app.np.drag = None;
        seek(app, x);
    } else {
        app.np.drag = Some(x);
    }
    island::invalidate(app);
}

// ---------------------------------------------------------------------------
// Player name / icon / activation (PlayerIcon)
// ---------------------------------------------------------------------------

fn shell_item(aumid: &str) -> Option<windows::Win32::UI::Shell::IShellItem> {
    use windows::Win32::UI::Shell::*;
    unsafe {
        let h = HSTRING::from(aumid);
        SHCreateItemInKnownFolder(&FOLDERID_AppsFolder, KNOWN_FOLDER_FLAG(0), &h).ok()
    }
}

pub fn player_name(app: &mut App, aumid: &str) -> String {
    if aumid.is_empty() {
        return String::new();
    }
    if let Some(n) = app.np.names.get(aumid) {
        return n.clone();
    }
    use windows::Win32::UI::Shell::*;
    let name = shell_item(aumid)
        .and_then(|it| unsafe {
            let p = it.GetDisplayName(SIGDN_NORMALDISPLAY).ok()?;
            let s = p.to_string().ok();
            windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
            s
        })
        .unwrap_or_else(|| {
            let base = aumid.rsplit('!').next().unwrap_or(aumid);
            let base = base.trim_end_matches(".exe").trim_end_matches(".EXE");
            let mut c = base.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        });
    app.np.names.insert(aumid.to_string(), name.clone());
    name
}

fn player_icon(app: &mut App, aumid: &str) -> Option<Raw> {
    if aumid.is_empty() {
        return None;
    }
    if let Some(i) = app.np.icons.get(aumid) {
        return i.clone();
    }
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::UI::Shell::*;
    let icon = shell_item(aumid).and_then(|it| unsafe {
        let f: IShellItemImageFactory = windows::core::Interface::cast(&it).ok()?;
        let hbm = f.GetImage(windows::Win32::Foundation::SIZE { cx: 64, cy: 64 }, SIIGBF_ICONONLY).ok()?;
        let mut bmp = BITMAP::default();
        GetObjectW(hbm.into(), std::mem::size_of::<BITMAP>() as i32, Some(&mut bmp as *mut _ as *mut _));
        let (w, h) = (bmp.bmWidth.max(1) as u32, bmp.bmHeight.unsigned_abs().max(1));
        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut px = vec![0u8; (w * h * 4) as usize];
        let dc = GetDC(None);
        GetDIBits(dc, hbm, 0, h, Some(px.as_mut_ptr() as *mut _), &mut bi, DIB_RGB_COLORS);
        ReleaseDC(None, dc);
        let _ = DeleteObject(hbm.into());
        let mut raw = Raw { w, h, px };
        imaging::grayscale(&mut raw);
        Some(raw)
    });
    app.np.icons.insert(aumid.to_string(), icon.clone());
    icon
}

/// Bring the player forward (click on the artwork).
pub fn open_player(app: &mut App) {
    let Some(t) = &app.np.track else { return };
    let aumid = t.player.clone();
    if aumid.is_empty() {
        return;
    }
    ui::defer(move || activate_player(&aumid));
}

fn activate_player(aumid: &str) {
    use windows::Win32::Foundation::*;
    use windows::Win32::UI::WindowsAndMessaging::*;
    let stem = aumid.rsplit(['!', '\\']).next().unwrap_or(aumid).trim_end_matches(".exe").to_lowercase();
    struct Find {
        stem: String,
        aumid: String,
        found: HWND,
    }
    unsafe extern "system" fn cb(h: HWND, l: LPARAM) -> windows::core::BOOL {
        unsafe {
            let f = &mut *(l.0 as *mut Find);
            if !IsWindowVisible(h).as_bool() || GetWindow(h, GW_OWNER).map(|o| !o.0.is_null()).unwrap_or(false) {
                return windows::core::BOOL(1);
            }
            let mut len = [0u16; 2];
            if GetWindowTextW(h, &mut len) == 0 {
                return windows::core::BOOL(1);
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(h, Some(&mut pid));
            if let Some(path) = crate::clipboard::process_path(pid) {
                let exe = path.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
                let a = f.aumid.to_lowercase();
                if !exe.is_empty() && (exe == f.stem || a.contains(&exe) || (exe == "msedge" && a.contains("edge"))) {
                    f.found = h;
                    return windows::core::BOOL(0);
                }
            }
            windows::core::BOOL(1)
        }
    }
    let mut f = Find { stem, aumid: aumid.to_string(), found: HWND::default() };
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut f as *mut _ as isize));
        if !f.found.0.is_null() {
            if IsIconic(f.found).as_bool() {
                let _ = ShowWindow(f.found, SW_RESTORE);
            }
            let _ = SetForegroundWindow(f.found);
            return;
        }
    }
    util::shell_open(&format!("shell:AppsFolder\\{aumid}"));
}

// ---------------------------------------------------------------------------
// Bar + waveform
// ---------------------------------------------------------------------------

/// Deterministic, smooth "audio-like" bar heights per track (0.28…1).
pub fn level(i: usize, seed: &str) -> f64 {
    let mut hash: u64 = 1469598103934665603;
    for b in seed.as_bytes() {
        hash = (hash ^ *b as u64).wrapping_mul(1099511628211);
    }
    let s = (hash % 1000) as f64 / 1000.0 * 6.28;
    let x = i as f64;
    let wave = 0.55 * (x * 0.61 + s).sin().abs() + 0.30 * (x * 0.23 + s * 1.7).sin().abs() + 0.15 * (x * 1.37 + s * 0.4).sin().abs();
    0.28 + 0.72 * wave.min(1.0)
}

pub fn time(seconds: f64) -> String {
    let s = seconds.max(0.0).floor() as i64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

pub fn draw_bar(cv: &mut Canvas, app: &mut App, r: Rect) {
    let Some(track) = app.np.track.clone().or(app.np.last.clone()) else { return };
    let live = app.np.track.is_some();
    cv.fill_round(r, 10.0, Color::white(0.055));
    cv.stroke_round(r, 10.0, 0.5, Color::white(0.10));
    let cy = r.cy();
    // Artwork 24×24.
    let art = rect(r.x + 5.0, cy - 12.0, 24.0, 24.0);
    let name = player_name(app, &track.player);
    if live {
        let tip = if track.player.is_empty() { "Now playing".to_string() } else { format!("Open {name}") };
        app.island.hits.push(Hit { rect: art, id: key("np.art"), tip: Some(tip), act: Act::NpArt, text: false });
    }
    cv.fill_round(art, 6.0, Color::white(0.06));
    let gen_ = cv.g.generation;
    if app.np.art_bmp.as_ref().map(|(g, _)| *g != gen_).unwrap_or(true) {
        let raw = app.np.artwork.clone().or_else(|| player_icon(app, &track.player));
        app.np.art_bmp = raw.and_then(|r| cv.g.bitmap(r.w, r.h, &r.px)).map(|b| (gen_, b));
    }
    if let Some((_, b)) = app.np.art_bmp.clone() {
        let geom = cv.g.squircle(24.0, 24.0, 6.0);
        let gm: windows::Win32::Graphics::Direct2D::ID2D1Geometry = windows::core::Interface::cast(&geom).unwrap();
        cv.save();
        cv.translate(art.x, art.y);
        cv.push_mask(&gm);
        cv.restore();
        cv.image_fill(&b, art, 1.0);
        cv.pop_layer();
    } else {
        icons::draw(cv, Icon::MusicNote, art.center(), 12.0, Weight::Medium, Color::white(0.7));
    }
    cv.stroke_round_centered(art, 6.0, 0.5, Color::white(0.18));
    // Title / subtitle (150 wide).
    let tf = Font::sans(10.5, Weight::Semibold);
    let sf = Font::sans(9.0, Weight::Regular);
    let th = cv.g.line_height(&tf);
    let sh = cv.g.line_height(&sf);
    let tx = art.r() + 9.0;
    let ty = cy - (th + sh) / 2.0;
    cv.text(&track.title, &tf, Color::white(1.0), rect(tx, ty, 150.0, th), Align::Leading, Trim::Tail);
    let sub = if track.subtitle().is_empty() { if name.is_empty() { " ".to_string() } else { name.clone() } } else { track.subtitle() };
    cv.text(&sub, &sf, Color::white(0.52), rect(tx, ty + th, 150.0, sh), Align::Leading, Trim::Tail);
    // Transport: previous, play/pause, next (24 pt each, 4 pt from the edge).
    let bx = r.r() - 4.0 - 72.0;
    if live {
        transport_button(cv, app, rect(bx, cy - 12.0, 24.0, 24.0), Icon::BackwardFill, "prev", Act::NpPrev, "Previous");
        let (icon, tip) = if track.playing { (Icon::PauseFill, "Pause") } else { (Icon::PlayFill, "Play") };
        transport_button(cv, app, rect(bx + 24.0, cy - 12.0, 24.0, 24.0), icon, "toggle", Act::NpToggle, tip);
        transport_button(cv, app, rect(bx + 48.0, cy - 12.0, 24.0, 24.0), Icon::ForwardFill, "next", Act::NpNext, "Next");
    }
    if track.duration <= 0.0 {
        return;
    }
    // Waveform progress: display only (no seeking).
    let wx = tx + 150.0 + 9.0;
    let wr = rect(wx, r.y, (bx - 9.0 - wx).max(10.0), r.h);
    let now = util::mono();
    let drag: Option<f64> = None;
    let elapsed = drag.map(|d| d * track.duration).unwrap_or_else(|| track.elapsed_at(now));
    let tfont = Font::mono(9.0, Weight::Medium);
    let tl = cv.g.line_height(&tfont);
    cv.text(&time(elapsed), &tfont, Color::white(0.5), rect(wr.x, cy - tl / 2.0, 30.0, tl), Align::Leading, Trim::None);
    let remain = format!("-{}", time((track.duration - drag.map(|d| d * track.duration).unwrap_or(track.elapsed)).max(0.0)));
    cv.text(&remain, &tfont, Color::white(0.5), rect(wr.r() - 34.0, cy - tl / 2.0, 34.0, tl), Align::Trailing, Trim::None);
    let wave = rect(wr.x + 30.0 + 8.0, cy - 8.0, (wr.w - 30.0 - 34.0 - 16.0).max(4.0), 16.0);
    let progress = drag.unwrap_or_else(|| track.progress_at(now));
    let (bar, gap) = (2.4f32, 2.2f32);
    let count = (((wave.w + gap) / (bar + gap)) as usize).max(1);
    let span = count as f32 * (bar + gap) - gap;
    let x0 = wave.x + (wave.w - span) / 2.0;
    let t = util::now_secs() - 978_307_200.0; // seconds since 2001 (timeIntervalSinceReferenceDate)
    let seed = track.id();
    for i in 0..count {
        let x = x0 + i as f32 * (bar + gap);
        let base = level(i, &seed);
        let lv = if track.playing { 0.86 + 0.14 * (t * 4.2 + i as f64 * 0.83).sin() } else { 1.0 };
        let h = (wave.h * (base * lv) as f32).max(3.0);
        let br = rect(x, wave.y + (wave.h - h) / 2.0, bar, h);
        let played = (x + bar / 2.0 - x0) / span.max(1.0) <= progress as f32;
        let a = if played { 0.95 } else { 0.22 };
        cv.fill_rrect(br, bar / 2.0, Color::white(a));
    }
}

/// Compact transport control: hover halo, white icon, press snap (Mac TransportButton).
fn transport_button(cv: &mut Canvas, app: &mut App, r: Rect, icon: Icon, name: &str, act: Act, tip: &str) {
    let id = key(&format!("np.btn.{name}"));
    app.island.hits.push(Hit { rect: r, id, tip: Some(tip.to_string()), act, text: false });
    let hover = app.island.hover == Some(id);
    let down = app.island.pressed.as_ref().map(|(p, _)| *p == id).unwrap_or(false);
    let hv = app.anims.drive(key(&format!("np.btn.h.{name}")), if hover { 1.0 } else { 0.0 }, motion::HOVER).clamp(0.0, 1.0);
    let s = app.anims.drive(key(&format!("np.btn.s.{name}")), if down { 0.9 } else { 1.0 }, motion::HOVER);
    cv.save();
    cv.scale_about(s, r.cx(), r.cy());
    cv.fill_circle(r.center(), 12.0, Color::white(0.12 * hv));
    icons::draw(cv, icon, r.center(), 10.0, Weight::Semibold, Color::white(0.72 + 0.28 * hv));
    cv.restore();
}

/// The waveform breathes at 30 fps while playing and visible.
pub fn wants_frames(app: &App) -> bool {
    app.island.state == island::State::Expanded
        && app.settings.music_controls
        && app.np.track.as_ref().map(|t| t.playing && t.duration > 0.0).unwrap_or(false)
}

/// Dev aid (`--nowplaying`): print what the system reports.
pub fn print_now_playing() {
    imaging::com_init();
    let mgr = GlobalSystemMediaTransportControlsSessionManager::RequestAsync().ok().and_then(|op| op.join().ok());
    let s = mgr.as_ref().and_then(|m| m.GetCurrentSession().ok());
    match s {
        Some(s) => {
            let props = s.TryGetMediaPropertiesAsync().ok().and_then(|o| o.join().ok());
            let title = props.as_ref().and_then(|p| p.Title().ok()).map(|h| h.to_string()).unwrap_or_default();
            let artist = props.as_ref().and_then(|p| p.Artist().ok()).map(|h| h.to_string()).unwrap_or_default();
            let playing = s.GetPlaybackInfo().ok().and_then(|p| p.PlaybackStatus().ok()) == Some(GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing);
            let player = s.SourceAppUserModelId().map(|h| h.to_string()).unwrap_or_default();
            let art = props.as_ref().map(|p| p.Thumbnail().is_ok()).unwrap_or(false);
            println!("now playing: {title} — {artist} · {} · player={player} · art={art}", if playing { "playing" } else { "paused" });
        }
        None => println!("now playing: nothing (available={})", mgr.is_some()),
    }
}

