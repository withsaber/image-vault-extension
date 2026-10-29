//! Developer aid: `IMAGEVAULT_ROOT=<tmp> ImageVault --snapshot <dir>` renders
//! every island state off-screen to PNGs (Snapshot.swift).

use std::path::PathBuf;

use crate::anim::motion;
use crate::app::{self, App};
use crate::island::{self, State, Tab};
use crate::theme::{Color, Font, Weight, rect};
use crate::{cliphistory, gfx, imaging, nowplaying, tray, ui, util, vault};

fn pump(ms: u64) {
    use windows::Win32::UI::WindowsAndMessaging::*;
    let end = std::time::Instant::now() + std::time::Duration::from_millis(ms);
    unsafe {
        let mut msg = MSG::default();
        while std::time::Instant::now() < end {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
                ui::drain();
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

fn seed() {
    if !vault::load().items.is_empty() {
        return;
    }
    for i in 0..14 {
        let raw = gfx::offscreen(480, 320, 1.0, |cv| {
            let a = (i % 7) as f32 / 8.0 + 0.1;
            let b = 1.0 - (i % 5) as f32 / 6.0;
            cv.fill_gradient_v(rect(0.0, 0.0, 480.0, 320.0), &[(0.0, Color::gray(a, 1.0)), (1.0, Color::gray(b, 1.0))], None);
            cv.text_at(&format!("{}", i + 1), &Font::sans(90.0, Weight::Bold), Color::black(1.0), crate::theme::pt(24.0, 10.0));
        });
        if let Some(raw) = raw {
            if let Some(png) = imaging::encode_png(&raw, None) {
                let meta = vault::Meta { name: Some(format!("sample-{}", i + 1)), source: "snapshot".into(), ..Default::default() };
                let _ = vault::add(&png, &meta);
            }
        }
    }
}

/// Let springs settle (0.9 s at 60 fps) and thumbnails load, then render.
fn shoot(dir: &PathBuf, name: &str) {
    for _ in 0..54 {
        pump(4);
        app::with(|a| app_frame(a, 1.0 / 60.0));
    }
    app::with(|app| {
        let scale = 2.0f32;
        let (w, h) = app.island.win_size();
        let raw = gfx::offscreen((w * scale) as u32, (h * scale) as u32, scale, |cv| {
            cv.fill_rect(rect(0.0, 0.0, w, h), Color::gray(0.32, 1.0));
            app.island.hits.clear();
            island::draw(cv, app);
        });
        if let Some(png) = raw.and_then(|r| imaging::encode_png(&r, None)) {
            let _ = std::fs::write(dir.join(format!("{name}.png")), png);
        }
    });
}

fn app_frame(app: &mut App, dt: f32) {
    app.anims.now += dt as f64;
    app.anims.tick(dt);
    let (w, h) = app.island.win_size();
    let _ = gfx::offscreen((w * 2.0) as u32, (h * 2.0) as u32, 2.0, |cv| {
        app.island.hits.clear();
        island::draw(cv, app);
    });
}

pub fn run(dir: &str) {
    let dir = PathBuf::from(dir);
    let _ = std::fs::create_dir_all(&dir);
    imaging::com_init();
    seed();
    app::init_headless();
    app::with(|a| {
        a.model.reload(&[]);
        a.island.scale = 2.0;
        a.island.position = crate::settings::Position::Center;
        a.island.notch = (190.0, 32.0);
        a.island.expanded = (660.0, a.island.band() + 8.0 + 236.0);
        tray::apply(a);
        let first = a.model.items.first().cloned();
        a.island.activity.image = first.as_ref().and_then(|i| imaging::thumbnail(&vault::file_path(i), 88));
        a.island.activity.title = "Saved".into();
        a.island.activity.detail = format!("{} in vault", a.model.items.len());
        a.settings.music_controls = true;
        a.np.track = Some(nowplaying::Track {
            title: "Midnight City".into(),
            artist: "M83".into(),
            album: "Hurry Up, We're Dreaming".into(),
            player: String::new(),
            playing: true,
            elapsed: 68.0,
            duration: 244.0,
            anchor: util::mono(),
            rate: 1.0,
        });
        a.np.artwork = gfx::offscreen(60, 60, 1.0, |cv| {
            cv.fill_rect(rect(0.0, 0.0, 60.0, 60.0), Color::gray(0.18, 1.0));
            cv.fill_circle(crate::theme::pt(30.0, 30.0), 12.0, Color::white(0.75));
        });
    });
    let states = [
        ("1-collapsed", State::Collapsed),
        ("2-peek", State::Peek),
        ("3-activity", State::Activity),
        ("4-draghint", State::DragHint),
        ("5-dragover", State::DragOver),
        ("6-expanded", State::Expanded),
    ];
    for (name, s) in states {
        app::with(|a| {
            a.island.state = s;
            a.island.state_curve = if s == State::Collapsed { motion::CLOSE } else { motion::OPEN };
        });
        shoot(&dir, name);
    }
    app::with(|a| {
        tray::toggle_select_all(a);
        island::flash(a, "Copied — Ctrl+V anywhere");
    });
    shoot(&dir, "7-expanded-selected");
    app::with(|a| {
        tray::clear_selection(a);
        a.island.message = None;
    });
    shoot(&dir, "8-expanded-cleared");
    app::with(|a| {
        a.island.search_open = true;
        a.island.search.set("sample");
        island::query_changed(a);
    });
    shoot(&dir, "9-expanded-search");
    app::with(|a| {
        a.island.search.set("zzz");
        island::query_changed(a);
    });
    shoot(&dir, "10-expanded-no-matches");
    app::with(|a| {
        a.island.search.set("");
        a.island.search_open = false;
        island::query_changed(a);
        if let Some(id) = a.tray.visible.first().cloned() {
            let list = a.tray.visible.clone();
            tray::show_preview(a, &id, list);
        }
    });
    shoot(&dir, "12-preview");
    app::with(|a| {
        tray::close_preview(a);
        let samples = [
            ("Meeting moved to Thursday 3pm — same room, bring the Q4 numbers.", "Slack"),
            ("https://www.figma.com/file/AbC123/Image-Vault-Island", "Comet"),
            ("brew install tesseract dylibbundler", "Terminal"),
            ("আমার সোনার বাংলা, আমি তোমায় ভালোবাসি", "Notes"),
            ("shadat@example.com", "Mail"),
            ("let side = max(2, floor((width - 32 - (cols - 1) * gap) / cols))", "Xcode"),
            ("#15191D", "Image Vault"),
        ];
        a.clips.items = samples
            .iter()
            .enumerate()
            .map(|(i, (t, app_name))| cliphistory::ClipItem {
                id: format!("sample{i}"),
                kind: cliphistory::Kind::Text,
                text: Some(t.to_string()),
                image_file: None,
                width: None,
                height: None,
                hash: format!("s{i}"),
                date: util::now_ms() - i as f64 * 340_000.0,
                app: Some(app_name.to_string()),
            })
            .collect();
        a.island.tab = Tab::Clipboard;
        a.island.tab_changed_at = util::mono();
    });
    shoot(&dir, "11-clipboard");
    app::with(|a| {
        a.island.tab = Tab::Vault;
        a.np.track = None;
        a.model.items.clear();
        a.model.version += 1;
        tray::model_changed(a);
    });
    shoot(&dir, "13-empty");
    println!("snapshots in {}", dir.display());
    app::shutdown();
}

/// `--capture-demo <image> <dir>`: exercises the screenshot renderer and windows.
pub fn capture_demo(image: &str, out: &str) {
    use crate::capture::{Annotation, Doc, Kind};
    let dir = PathBuf::from(out);
    let _ = std::fs::create_dir_all(&dir);
    imaging::com_init();
    let Some(src) = imaging::decode_path(std::path::Path::new(image)) else {
        println!("can't read {image}");
        return;
    };
    app::init_headless();
    let mut doc = Doc::new(src.clone(), 2.0);
    let (w, h) = doc.size();
    let p = |x: f32, y: f32| crate::theme::pt(w * x, h * y);
    let red = crate::editor::COLORS[0].1;
    let blue = crate::editor::COLORS[4].1;
    let yellow = crate::editor::COLORS[2].1;
    let mut text = Annotation::new(Kind::Text, vec![p(0.06, 0.05)], Color::white(1.0), 4.0);
    text.text = "Text annotation".into();
    let mut pasted = Annotation::new(Kind::Image, vec![p(0.72, 0.75), p(0.95, 0.95)], Color::white(1.0), 0.0);
    pasted.image = Some(std::rc::Rc::new((src.clone(), 2.0)));
    let pencil: Vec<crate::theme::Pt> = (0..30).map(|i| p(0.05 + i as f32 * 0.008, 0.25 + 0.05 * (i as f32 / 3.0).sin())).collect();
    doc.set_annotations(vec![
        Annotation::new(Kind::Blur, vec![p(0.05, 0.75), p(0.3, 0.95)], Color::CLEAR, 4.0),
        Annotation::new(Kind::Spotlight, vec![p(0.35, 0.35), p(0.65, 0.65)], Color::CLEAR, 4.0),
        Annotation::new(Kind::Arrow, vec![p(0.1, 0.5), p(0.33, 0.4)], red, 4.0),
        Annotation::new(Kind::Line, vec![p(0.1, 0.6), p(0.3, 0.6)], blue, 3.0),
        Annotation::new(Kind::Measure, vec![p(0.4, 0.8), p(0.6, 0.8)], yellow, 4.0),
        Annotation::new(Kind::Rectangle, vec![p(0.68, 0.15), p(0.9, 0.4)], red, 4.0),
        Annotation::new(Kind::FilledRect, vec![p(0.7, 0.52), p(0.8, 0.6)], Color::black(1.0), 4.0),
        Annotation::new(Kind::Oval, vec![p(0.4, 0.1), p(0.55, 0.25)], blue, 4.0),
        Annotation::new(Kind::Pencil, pencil, red, 4.0),
        Annotation::new(Kind::Highlighter, vec![p(0.35, 0.7), p(0.6, 0.7)], yellow, 4.0),
        Annotation::new(Kind::Counter, vec![p(0.62, 0.3)], red, 4.0),
        Annotation::new(Kind::Counter, vec![p(0.66, 0.7)], blue, 4.0),
        text,
        pasted,
    ]);
    let r = doc.rendered();
    if let Some(png) = imaging::encode_png(&r, Some(192.0)) {
        let _ = std::fs::write(dir.join("rendered.png"), png);
    }
    println!("export {}×{} vs capture {}×{}", r.w, r.h, src.w, src.h);
    let id = doc.id;
    let mon = crate::monitors::all().into_iter().next();
    app::with(|a| {
        if let Some(m) = &mon {
            crate::capture::present(a, doc, m);
        }
        crate::capture::edit(a, id);
    });
    let t0 = std::time::Instant::now();
    while t0.elapsed().as_millis() < 1500 {
        pump(10);
        app::tick();
    }
    println!("windows shown");
    // Keep them up for a moment so the screen can be captured externally.
    let t0 = std::time::Instant::now();
    while t0.elapsed().as_millis() < 4000 {
        pump(10);
        app::tick();
    }
    app::shutdown();
}

/// `--blur-test <image> <dir>`: blur-only exports at 1× and 2×.
pub fn blur_test(image: &str, out: &str) {
    use crate::capture::{Annotation, Doc, Kind};
    imaging::com_init();
    let Some(src) = imaging::decode_path(std::path::Path::new(image)) else { return };
    for s in [1.0f32, 2.0] {
        let mut doc = Doc::new(src.clone(), s);
        let (w, h) = doc.size();
        doc.set_annotations(vec![Annotation::new(Kind::Blur, vec![crate::theme::pt(0.0, 0.0), crate::theme::pt(w * 0.5, h * 0.5)], Color::CLEAR, 4.0)]);
        let r = doc.rendered();
        if let Some(png) = imaging::encode_png(&r, None) {
            let _ = std::fs::write(PathBuf::from(out).join(format!("blur{s}.png")), png);
        }
    }
    gfx::reset();
}
