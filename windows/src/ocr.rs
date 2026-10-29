//! Text recognition (OCR.swift): Windows.Media.Ocr for the built-in
//! languages with the Mac's layout pass (reading order, tabs for column gaps,
//! blank lines for paragraph gaps, 2× retry for tiny text), plus Tesseract
//! for scripts Windows OCR can't read (Bangla, Hindi, Tamil…). Also the
//! background indexer that makes text inside images searchable.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;

use crate::imaging::{self, Raw};
use crate::util;

pub const VERSION: i64 = 2;

pub struct Lang {
    pub code: &'static str,
    pub name: &'static str,
}

pub const LANGUAGES: [Lang; 12] = [
    Lang { code: "ben", name: "Bangla" },
    Lang { code: "hin", name: "Hindi" },
    Lang { code: "urd", name: "Urdu" },
    Lang { code: "tam", name: "Tamil" },
    Lang { code: "tel", name: "Telugu" },
    Lang { code: "mar", name: "Marathi" },
    Lang { code: "nep", name: "Nepali" },
    Lang { code: "guj", name: "Gujarati" },
    Lang { code: "pan", name: "Punjabi" },
    Lang { code: "mal", name: "Malayalam" },
    Lang { code: "kan", name: "Kannada" },
    Lang { code: "sin", name: "Sinhala" },
];

thread_local! {
    static ENGINE: RefCell<Option<Option<OcrEngine>>> = const { RefCell::new(None) };
}

fn engine() -> Option<OcrEngine> {
    imaging::com_init();
    ENGINE.with(|e| {
        if e.borrow().is_none() {
            let eng = OcrEngine::TryCreateFromUserProfileLanguages().ok().or_else(|| {
                let l = Language::CreateLanguage(&windows::core::HSTRING::from("en-US")).ok()?;
                OcrEngine::TryCreateFromLanguage(&l).ok()
            });
            *e.borrow_mut() = Some(eng);
        }
        e.borrow().clone().flatten()
    })
}

/// Display names of the languages Windows OCR has installed.
pub fn builtin_languages() -> Vec<String> {
    imaging::com_init();
    OcrEngine::AvailableRecognizerLanguages()
        .ok()
        .map(|v| v.into_iter().filter_map(|l| l.DisplayName().ok().map(|h| h.to_string())).collect())
        .unwrap_or_default()
}

struct Part {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    text: String,
}

struct Row {
    y0: f32,
    y1: f32,
    x0: f32,
    x1: f32,
    parts: Vec<Part>,
}

fn software_bitmap(raw: &Raw) -> Option<SoftwareBitmap> {
    let w = DataWriter::new().ok()?;
    w.WriteBytes(&raw.px).ok()?;
    let buf = w.DetachBuffer().ok()?;
    SoftwareBitmap::CreateCopyWithAlphaFromBuffer(&buf, BitmapPixelFormat::Bgra8, raw.w as i32, raw.h as i32, BitmapAlphaMode::Premultiplied).ok()
}

/// One OCR pass → rows of normalised boxes.
fn run(raw: &Raw) -> Option<String> {
    let eng = engine()?;
    let max = OcrEngine::MaxImageDimension().unwrap_or(10000) as u32;
    let raw = if raw.w > max || raw.h > max { imaging::scale_raw(raw, max) } else { raw.clone() };
    let sb = software_bitmap(&raw)?;
    let result = eng.RecognizeAsync(&sb).ok()?.join().ok()?;
    let lines = result.Lines().ok()?;
    let (iw, ih) = (raw.w as f32, raw.h as f32);
    let mut parts: Vec<Part> = Vec::new();
    for line in lines {
        let words = line.Words().ok()?;
        let mut ws: Vec<(f32, f32, f32, f32, String)> = Vec::new();
        for w in words {
            let r = w.BoundingRect().ok()?;
            let t = w.Text().ok()?.to_string();
            ws.push((r.X / iw, r.Y / ih, (r.X + r.Width) / iw, (r.Y + r.Height) / ih, t));
        }
        if ws.is_empty() {
            continue;
        }
        let full = line.Text().ok()?.to_string();
        // Split a line at wide gaps so columns become separate observations.
        let h = ws.iter().map(|w| w.3 - w.1).fold(0.0f32, f32::max);
        let threshold = (0.012f32).max(h * 0.7);
        let mut groups: Vec<Vec<usize>> = vec![vec![0]];
        for i in 1..ws.len() {
            if ws[i].0 - ws[i - 1].2 > threshold {
                groups.push(vec![i]);
            } else {
                groups.last_mut().unwrap().push(i);
            }
        }
        if groups.len() == 1 {
            let x0 = ws.iter().map(|w| w.0).fold(f32::MAX, f32::min);
            let y0 = ws.iter().map(|w| w.1).fold(f32::MAX, f32::min);
            let x1 = ws.iter().map(|w| w.2).fold(f32::MIN, f32::max);
            let y1 = ws.iter().map(|w| w.3).fold(f32::MIN, f32::max);
            parts.push(Part { x0, y0, x1, y1, text: full });
        } else {
            for g in groups {
                let x0 = g.iter().map(|&i| ws[i].0).fold(f32::MAX, f32::min);
                let y0 = g.iter().map(|&i| ws[i].1).fold(f32::MAX, f32::min);
                let x1 = g.iter().map(|&i| ws[i].2).fold(f32::MIN, f32::max);
                let y1 = g.iter().map(|&i| ws[i].3).fold(f32::MIN, f32::max);
                let text = g.iter().map(|&i| ws[i].4.clone()).collect::<Vec<_>>().join(" ");
                parts.push(Part { x0, y0, x1, y1, text });
            }
        }
    }
    layout(parts)
}

/// Rebuild rows (top→bottom, left→right), tabs for wide column gaps and
/// blank lines for paragraph-sized vertical gaps.
fn layout(mut parts: Vec<Part>) -> Option<String> {
    parts.retain(|p| !p.text.trim().is_empty());
    parts.sort_by(|a, b| a.y0.partial_cmp(&b.y0).unwrap_or(std::cmp::Ordering::Equal));
    let mut rows: Vec<Row> = Vec::new();
    for p in parts {
        let mut matched = None;
        for (i, r) in rows.iter().enumerate().rev() {
            let overlap = r.y1.min(p.y1) - r.y0.max(p.y0);
            if overlap > (r.y1 - r.y0).min(p.y1 - p.y0) * 0.45 {
                matched = Some(i);
                break;
            }
        }
        match matched {
            Some(i) => {
                let r = &mut rows[i];
                r.y0 = r.y0.min(p.y0);
                r.y1 = r.y1.max(p.y1);
                r.x0 = r.x0.min(p.x0);
                r.x1 = r.x1.max(p.x1);
                r.parts.push(p);
            }
            None => rows.push(Row { y0: p.y0, y1: p.y1, x0: p.x0, x1: p.x1, parts: vec![p] }),
        }
    }
    if rows.is_empty() {
        return None;
    }
    rows.sort_by(|a, b| a.y0.partial_cmp(&b.y0).unwrap_or(std::cmp::Ordering::Equal));
    let mut lines: Vec<String> = Vec::new();
    let mut prev: Option<(f32, f32)> = None;
    for row in rows.iter_mut() {
        let rh = row.y1 - row.y0;
        if let Some((py0, py1)) = prev {
            let gap = row.y0 - py1;
            let line_h = rh.max(py1 - py0);
            if gap > (0.02f32).max(line_h * 1.15) {
                lines.push(String::new());
            }
        }
        row.parts.sort_by(|a, b| a.x0.partial_cmp(&b.x0).unwrap_or(std::cmp::Ordering::Equal));
        let mut line = String::new();
        let mut last_x1: Option<f32> = None;
        for p in &row.parts {
            if let Some(t) = last_x1 {
                let gap = p.x0 - t;
                let threshold = (0.012f32).max(rh * 0.7);
                line.push(if gap > threshold { '\t' } else { ' ' });
            }
            line.push_str(&p.text);
            last_x1 = Some(p.x1);
        }
        let cleaned = line.trim_matches(' ').to_string();
        if !cleaned.is_empty() {
            lines.push(cleaned);
        }
        prev = Some((row.y0, row.y1));
    }
    let mut compact: Vec<String> = Vec::new();
    for l in lines {
        if l.is_empty() && compact.last().map(|x: &String| x.is_empty()).unwrap_or(false) {
            continue;
        }
        compact.push(l);
    }
    while compact.first().map(|l| l.is_empty()).unwrap_or(false) {
        compact.remove(0);
    }
    while compact.last().map(|l| l.is_empty()).unwrap_or(false) {
        compact.pop();
    }
    let text = compact.join("\n");
    if text.is_empty() { None } else { Some(text) }
}

fn score(t: &Option<String>) -> f32 {
    // Windows OCR reports no confidence; Vision's term is constant here.
    t.as_ref().map(|s| s.chars().count() as f32 + 40.0).unwrap_or(0.0)
}

fn load(path: &Path) -> Option<Raw> {
    if imaging::is_svg_path(path) {
        let data = std::fs::read(path).ok()?;
        // Vision renders SVG at 2× (min 512 pt) — same here.
        return imaging::svg_raster(&data, 1024, 1024);
    }
    imaging::decode_path(path)
}

/// Built-in engine with layout and the tiny-text retry.
pub fn recognize_builtin(path: &Path) -> Option<String> {
    let src = load(path)?;
    let first = run(&src);
    let first_len = first.as_ref().map(|s| s.chars().count()).unwrap_or(0);
    let small = src.w.max(src.h) <= 1800;
    if first_len < 6 && small && src.w * 2 <= 3600 && src.h * 2 <= 3600 {
        if let Some(up) = upscale(&src) {
            let second = run(&up);
            if score(&second) > score(&first) + 2.0 {
                return second;
            }
        }
    }
    first
}

fn upscale(raw: &Raw) -> Option<Raw> {
    let f = imaging::wic()?;
    use windows::Win32::Graphics::Imaging::*;
    unsafe {
        let bmp = f.CreateBitmapFromMemory(raw.w, raw.h, &GUID_WICPixelFormat32bppPBGRA, raw.w * 4, &raw.px).ok()?;
        let sc = f.CreateBitmapScaler().ok()?;
        sc.Initialize(&bmp, raw.w * 2, raw.h * 2, WICBitmapInterpolationModeHighQualityCubic).ok()?;
        let (w, h) = (raw.w * 2, raw.h * 2);
        let mut px = vec![0u8; (w * h * 4) as usize];
        sc.CopyPixels(std::ptr::null(), w * 4, &mut px).ok()?;
        Some(Raw { w, h, px })
    }
}

/// Whichever engine actually read the non-Latin script wins.
pub fn recognize(path: &Path, langs: &[String]) -> Option<String> {
    let builtin = recognize_builtin(path);
    match tesseract(path, langs) {
        Some(extra) if has_extra_script(&extra, langs) => Some(extra),
        _ => builtin,
    }
}

/// Text for the search index: both engines, so either one can match.
pub fn index_text(path: &Path, langs: &[String]) -> String {
    let builtin = recognize_builtin(path).unwrap_or_default();
    let extra = tesseract(path, langs).unwrap_or_default();
    if extra.is_empty() || !has_extra_script(&extra, langs) {
        return builtin;
    }
    if builtin.is_empty() {
        return extra;
    }
    format!("{builtin}\n{extra}")
}

/// True when the text has a meaningful share of a script the built-in engine
/// can't read (Bengali … Sinhala, or Urdu's Arabic letters when enabled).
pub fn has_extra_script(text: &str, langs: &[String]) -> bool {
    let urdu = langs.iter().any(|l| l == "urd");
    let mut extra = 0usize;
    let mut letters = 0usize;
    for c in text.chars() {
        if !c.is_alphabetic() {
            continue;
        }
        letters += 1;
        let v = c as u32;
        if (0x0900..=0x0DFF).contains(&v) || (urdu && (0x0600..=0x06FF).contains(&v)) {
            extra += 1;
        }
    }
    extra >= 3 && extra as f64 >= letters as f64 * 0.12
}

// ---------------------------------------------------------------------------
// Tesseract
// ---------------------------------------------------------------------------

pub struct Tess {
    pub exe: PathBuf,
    pub tessdata: PathBuf,
}

fn tessdata_for(dir: &Path) -> Option<PathBuf> {
    let d = dir.join("tessdata");
    if d.join("eng.traineddata").exists() || d.join("ben.traineddata").exists() { Some(d) } else { None }
}

/// Bundled `tesseract\` next to the exe, or a system install.
pub fn tess() -> Option<Tess> {
    let local = util::exe_dir().join("tesseract");
    let mut candidates = vec![local.clone()];
    if let Ok(pf) = std::env::var("ProgramFiles") {
        candidates.push(PathBuf::from(pf).join("Tesseract-OCR"));
    }
    if let Ok(la) = std::env::var("LOCALAPPDATA") {
        candidates.push(PathBuf::from(la).join("Programs").join("Tesseract-OCR"));
    }
    for dir in candidates {
        let exe = dir.join("tesseract.exe");
        if exe.exists() {
            let td = tessdata_for(&local).or_else(|| tessdata_for(&dir))?;
            return Some(Tess { exe, tessdata: td });
        }
    }
    None
}

pub fn tesseract_available() -> bool {
    tess().is_some()
}

/// `tesseract <image> stdout -l <langs>+eng --psm 3` (background threads only).
pub fn tesseract(path: &Path, langs: &[String]) -> Option<String> {
    let t = tess()?;
    let mut use_langs: Vec<String> = langs
        .iter()
        .filter(|c| LANGUAGES.iter().any(|l| l.code == c.as_str()))
        .filter(|c| t.tessdata.join(format!("{c}.traineddata")).exists())
        .cloned()
        .collect();
    if use_langs.is_empty() {
        return None;
    }
    if t.tessdata.join("eng.traineddata").exists() {
        use_langs.push("eng".into());
    }
    // Tesseract can't read SVG (or HEIC/AVIF): hand it a PNG.
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mut temp: Option<PathBuf> = None;
    let input = if matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "tif" | "tiff" | "bmp" | "gif" | "webp") {
        path.to_path_buf()
    } else {
        let raw = load(path)?;
        let png = imaging::encode_png(&raw, None)?;
        let p = std::env::temp_dir().join(format!("iv-ocr-{}.png", crate::vault::random_suffix()));
        std::fs::write(&p, png).ok()?;
        temp = Some(p.clone());
        p
    };
    use std::os::windows::process::CommandExt;
    let child = std::process::Command::new(&t.exe)
        .arg(&input)
        .arg("stdout")
        .arg("-l")
        .arg(use_langs.join("+"))
        .arg("--psm")
        .arg("3")
        .env("TESSDATA_PREFIX", &t.tessdata)
        .env("OMP_THREAD_LIMIT", "2")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .spawn();
    let out = match child {
        Ok(mut c) => {
            // 30 s watchdog.
            let start = std::time::Instant::now();
            loop {
                match c.try_wait() {
                    Ok(Some(_)) => break c.wait_with_output().ok(),
                    Ok(None) => {
                        if start.elapsed().as_secs() > 30 {
                            let _ = c.kill();
                            break None;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(30));
                    }
                    Err(_) => break None,
                }
            }
        }
        Err(_) => None,
    };
    if let Some(p) = temp {
        let _ = std::fs::remove_file(p);
    }
    let text = String::from_utf8_lossy(&out?.stdout).replace('\u{0C}', "").trim().to_string();
    if text.is_empty() { None } else { Some(text) }
}

// ---------------------------------------------------------------------------
// Indexer (OCRIndexer)
// ---------------------------------------------------------------------------

pub fn language_key(langs: &[String]) -> String {
    let mut l = langs.to_vec();
    l.sort();
    l.join("+")
}

#[derive(Default)]
pub struct Indexer {
    running: bool,
    /// (id, language key) pairs already attempted this session.
    tried: std::collections::HashSet<String>,
}

/// OCR the next image that hasn't been read with the current pipeline.
pub fn pump(app: &mut crate::app::App) {
    if app.ocr.running {
        return;
    }
    let key = language_key(&app.settings.ocr_extra_languages);
    let next = app
        .model
        .items
        .iter()
        .find(|i| {
            (i.ocr_version.unwrap_or(0) < VERSION || i.ocr_languages.as_deref().unwrap_or("") != key)
                && !app.ocr.tried.contains(&format!("{}|{key}", i.id))
        })
        .cloned();
    let Some(item) = next else { return };
    app.ocr.tried.insert(format!("{}|{key}", item.id));
    app.ocr.running = true;
    let path = crate::vault::file_path(&item);
    let langs = app.settings.ocr_extra_languages.clone();
    util::spawn_low("ocr-index", move || {
        imaging::com_init();
        let text = if path.exists() { index_text(&path, &langs) } else { String::new() };
        crate::vault::set_ocr(&item.id, if text.is_empty() { None } else { Some(text) }, Some(VERSION), Some(key));
        crate::ui::post(|| {
            crate::app::with(|a| {
                a.ocr.running = false;
                crate::app::reload_model_if_changed(a);
                // Give the UI a breath between images.
                crate::ui::after(0.05, || {
                    crate::app::with(pump);
                });
            });
        });
    });
}

/// `--ocr <file>`.
pub fn cli(path: &str, langs: &[String]) -> Option<String> {
    imaging::com_init();
    recognize(Path::new(path), langs)
}
