//! Imaging over WIC — port of Imaging.swift: format normalisation,
//! thumbnails (EXIF orientation respected), PNG/JPEG encoding, DIBs for the
//! clipboard and SVG rasterisation through Direct2D.

use std::cell::RefCell;
use std::path::Path;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::StructuredStorage::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::Shell::SHCreateMemStream;
use windows::Win32::System::Variant::VARIANT;
use windows::core::{GUID, Interface, PCWSTR, w};

#[derive(Debug)]
pub enum Error {
    NotAnImage,
    Io(std::io::Error),
}

impl From<windows::core::Error> for Error {
    fn from(_: windows::core::Error) -> Self {
        Error::NotAnImage
    }
}

#[derive(Debug, Clone)]
pub struct Portable {
    pub data: Vec<u8>,
    pub mime: String,
    pub ext: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// Premultiplied BGRA8, tightly packed.
#[derive(Debug, Clone)]
pub struct Raw {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

pub fn com_init() {
    thread_local!(static INIT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
    INIT.with(|i| {
        if !i.get() {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            i.set(true);
        }
    });
}

thread_local! {
    static WIC: RefCell<Option<IWICImagingFactory>> = const { RefCell::new(None) };
}

pub fn wic() -> Option<IWICImagingFactory> {
    com_init();
    WIC.with(|w| {
        if w.borrow().is_none() {
            let f: Option<IWICImagingFactory> =
                unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok() };
            *w.borrow_mut() = f;
        }
        w.borrow().clone()
    })
}

/// SVG markup, also with a BOM, XML prolog, DOCTYPE or leading comments.
pub fn is_svg(data: &[u8]) -> bool {
    let head = &data[..data.len().min(4096)];
    let Ok(s) = std::str::from_utf8(head).or_else(|e| std::str::from_utf8(&head[..e.valid_up_to()])) else { return false };
    let s = s.trim_start_matches('\u{feff}').trim().to_lowercase();
    s.starts_with("<svg") || ((s.starts_with("<?xml") || s.starts_with("<!")) && s.contains("<svg"))
}

/// Copied text that is a whole SVG document (Imaging.isSVG(text:)).
pub fn is_svg_text(text: &str) -> bool {
    let t = text.trim();
    if t.len() >= 2_000_000 || !(t.to_lowercase().contains("</svg>") || t.ends_with("/>")) {
        return false;
    }
    is_svg(t.as_bytes())
}

fn stream_from(data: &[u8]) -> Option<IStream> {
    unsafe { SHCreateMemStream(Some(data)) }
}

fn decoder_bytes(f: &IWICImagingFactory, data: &[u8]) -> Option<IWICBitmapDecoder> {
    let s = stream_from(data)?;
    unsafe { f.CreateDecoderFromStream(&s, std::ptr::null(), WICDecodeMetadataCacheOnDemand).ok() }
}

fn decoder_path(f: &IWICImagingFactory, path: &Path) -> Option<IWICBitmapDecoder> {
    let w = crate::util::wide_path(path);
    unsafe {
        f.CreateDecoderFromFilename(PCWSTR(w.as_ptr()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand).ok()
    }
}

fn orientation(frame: &IWICBitmapFrameDecode) -> u32 {
    unsafe {
        let Ok(reader) = frame.GetMetadataQueryReader() else { return 1 };
        for q in [w!("/app1/ifd/{ushort=274}"), w!("/ifd/{ushort=274}"), w!("System.Photo.Orientation")] {
            let mut v = PROPVARIANT::default();
            if reader.GetMetadataByName(q, &mut v).is_ok() {
                let vt = v.Anonymous.Anonymous.vt;
                let val = match vt.0 {
                    18 => v.Anonymous.Anonymous.Anonymous.uiVal as u32,
                    19 => v.Anonymous.Anonymous.Anonymous.ulVal,
                    2 => v.Anonymous.Anonymous.Anonymous.iVal as u32,
                    3 => v.Anonymous.Anonymous.Anonymous.lVal as u32,
                    _ => 1,
                };
                let _ = PropVariantClear(&mut v);
                if (1..=8).contains(&val) {
                    return val;
                }
            }
        }
        1
    }
}

fn transform_for(o: u32) -> WICBitmapTransformOptions {
    match o {
        2 => WICBitmapTransformFlipHorizontal,
        3 => WICBitmapTransformRotate180,
        4 => WICBitmapTransformFlipVertical,
        5 => WICBitmapTransformOptions(WICBitmapTransformRotate90.0 | WICBitmapTransformFlipHorizontal.0),
        6 => WICBitmapTransformRotate90,
        7 => WICBitmapTransformOptions(WICBitmapTransformRotate270.0 | WICBitmapTransformFlipHorizontal.0),
        8 => WICBitmapTransformRotate270,
        _ => WICBitmapTransformRotate0,
    }
}

/// Frame → premultiplied BGRA, scaled so the longest side ≤ `max_px` (or so the
/// shortest side ≥ `min_side` when given), EXIF orientation applied.
fn frame_raw(f: &IWICImagingFactory, frame: &IWICBitmapFrameDecode, max_px: Option<u32>, min_side: Option<u32>) -> Option<Raw> {
    unsafe {
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h).ok()?;
        if w == 0 || h == 0 {
            return None;
        }
        let o = orientation(frame);
        let (ow, oh) = if o >= 5 { (h, w) } else { (w, h) };
        let mut k = 1.0f64;
        if let Some(m) = max_px {
            k = k.min(m as f64 / ow.max(oh) as f64);
        }
        if let Some(ms) = min_side {
            k = k.min(ms as f64 / ow.min(oh) as f64);
        }
        let mut src: IWICBitmapSource = frame.cast().ok()?;
        if k < 0.999 {
            let sw = ((w as f64 * k).round() as u32).max(1);
            let sh = ((h as f64 * k).round() as u32).max(1);
            let sc = f.CreateBitmapScaler().ok()?;
            sc.Initialize(&src, sw, sh, WICBitmapInterpolationModeFant).ok()?;
            src = sc.cast().ok()?;
        }
        if o > 1 {
            let fr = f.CreateBitmapFlipRotator().ok()?;
            fr.Initialize(&src, transform_for(o)).ok()?;
            src = fr.cast().ok()?;
        }
        let conv = f.CreateFormatConverter().ok()?;
        conv.Initialize(&src, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)
            .ok()?;
        let (mut cw, mut ch) = (0u32, 0u32);
        conv.GetSize(&mut cw, &mut ch).ok()?;
        let mut px = vec![0u8; (cw * ch * 4) as usize];
        conv.CopyPixels(std::ptr::null(), cw * 4, &mut px).ok()?;
        Some(Raw { w: cw, h: ch, px })
    }
}

fn container(dec: &IWICBitmapDecoder) -> GUID {
    unsafe { dec.GetContainerFormat().unwrap_or_default() }
}

fn has_alpha(f: &IWICImagingFactory, frame: &IWICBitmapFrameDecode) -> bool {
    unsafe {
        let Ok(fmt) = frame.GetPixelFormat() else { return true };
        let Ok(info) = f.CreateComponentInfo(&fmt) else { return true };
        let Ok(pf) = info.cast::<IWICPixelFormatInfo2>() else { return true };
        pf.SupportsTransparency().map(|b| b.as_bool()).unwrap_or(true)
    }
}

/// Formats every app accepts are kept byte-for-byte; anything else (AVIF,
/// WebP, HEIC, TIFF, BMP…) is re-encoded: JPEG (0.94) when opaque, PNG otherwise.
pub fn portable(data: &[u8]) -> Result<Portable, Error> {
    if is_svg(data) {
        return Ok(Portable { data: data.to_vec(), mime: "image/svg+xml".into(), ext: "svg".into(), width: None, height: None });
    }
    let f = wic().ok_or(Error::NotAnImage)?;
    let dec = decoder_bytes(&f, data).ok_or(Error::NotAnImage)?;
    let frame = unsafe { dec.GetFrame(0)? };
    let (mut w, mut h) = (0u32, 0u32);
    unsafe { frame.GetSize(&mut w, &mut h)? };
    let fmt = container(&dec);
    if fmt == GUID_ContainerFormatPng {
        return Ok(Portable { data: data.to_vec(), mime: "image/png".into(), ext: "png".into(), width: Some(w), height: Some(h) });
    }
    if fmt == GUID_ContainerFormatJpeg {
        // Mac reports the stored pixel size (before EXIF rotation).
        return Ok(Portable { data: data.to_vec(), mime: "image/jpeg".into(), ext: "jpg".into(), width: Some(w), height: Some(h) });
    }
    if fmt == GUID_ContainerFormatGif {
        return Ok(Portable { data: data.to_vec(), mime: "image/gif".into(), ext: "gif".into(), width: Some(w), height: Some(h) });
    }
    let opaque = !has_alpha(&f, &frame);
    let raw = frame_raw(&f, &frame, None, None).ok_or(Error::NotAnImage)?;
    let out = if opaque { encode_jpeg(&raw, 0.94) } else { encode_png(&raw, None) }.ok_or(Error::NotAnImage)?;
    Ok(Portable {
        data: out,
        mime: if opaque { "image/jpeg" } else { "image/png" }.into(),
        ext: if opaque { "jpg" } else { "png" }.into(),
        width: Some(raw.w),
        height: Some(raw.h),
    })
}

pub fn is_svg_path(path: &Path) -> bool {
    path.extension().map(|e| e.eq_ignore_ascii_case("svg")).unwrap_or(false)
}

/// Downsampled image for tiles and previews. `max_px` caps the longest side.
pub fn thumbnail(path: &Path, max_px: u32) -> Option<Raw> {
    if is_svg_path(path) {
        let data = std::fs::read(path).ok()?;
        return svg_raster(&data, max_px, max_px);
    }
    let f = wic()?;
    let dec = decoder_path(&f, path)?;
    let frame = unsafe { dec.GetFrame(0).ok()? };
    frame_raw(&f, &frame, Some(max_px), None)
}

/// Thumbnail whose *shorter* side is about `side` px (aspect-fill tiles).
pub fn tile_thumbnail(path: &Path, side: u32) -> Option<Raw> {
    if is_svg_path(path) {
        let data = std::fs::read(path).ok()?;
        return svg_raster(&data, side * 2, side * 2);
    }
    let f = wic()?;
    let dec = decoder_path(&f, path)?;
    let frame = unsafe { dec.GetFrame(0).ok()? };
    frame_raw(&f, &frame, Some(side * 4), Some(side))
}

pub fn decode_path(path: &Path) -> Option<Raw> {
    if is_svg_path(path) {
        let data = std::fs::read(path).ok()?;
        return svg_raster(&data, 1024, 1024);
    }
    let f = wic()?;
    let dec = decoder_path(&f, path)?;
    let frame = unsafe { dec.GetFrame(0).ok()? };
    frame_raw(&f, &frame, None, None)
}

pub fn decode_bytes(data: &[u8], max_px: Option<u32>) -> Option<Raw> {
    if is_svg(data) {
        let m = max_px.unwrap_or(1024);
        return svg_raster(data, m, m);
    }
    let f = wic()?;
    let dec = decoder_bytes(&f, data)?;
    let frame = unsafe { dec.GetFrame(0).ok()? };
    frame_raw(&f, &frame, max_px, None)
}

fn meta_u32(reader: &IWICMetadataQueryReader, name: PCWSTR) -> Option<u32> {
    unsafe {
        let mut v = PROPVARIANT::default();
        reader.GetMetadataByName(name, &mut v).ok()?;
        let vt = v.Anonymous.Anonymous.vt;
        let val = match vt.0 {
            17 => Some(v.Anonymous.Anonymous.Anonymous.bVal as u32),
            18 => Some(v.Anonymous.Anonymous.Anonymous.uiVal as u32),
            19 => Some(v.Anonymous.Anonymous.Anonymous.ulVal),
            2 => Some(v.Anonymous.Anonymous.Anonymous.iVal as u32),
            3 => Some(v.Anonymous.Anonymous.Anonymous.lVal as u32),
            _ => None,
        };
        let _ = PropVariantClear(&mut v);
        val
    }
}

fn resize_raw(f: &IWICImagingFactory, raw: &Raw, w: u32, h: u32) -> Option<Raw> {
    if w == raw.w && h == raw.h {
        return Some(raw.clone());
    }
    unsafe {
        let bmp = f.CreateBitmapFromMemory(raw.w, raw.h, &GUID_WICPixelFormat32bppPBGRA, raw.w * 4, &raw.px).ok()?;
        let sc = f.CreateBitmapScaler().ok()?;
        sc.Initialize(&bmp, w, h, WICBitmapInterpolationModeFant).ok()?;
        let mut px = vec![0u8; (w * h * 4) as usize];
        sc.CopyPixels(std::ptr::null(), w * 4, &mut px).ok()?;
        Some(Raw { w, h, px })
    }
}

/// Animated GIF frames for a tile (Imaging.gifFrames): composited onto the
/// logical screen (offsets + disposal), scaled so the longest side ≤ `side`,
/// at most 400 frames. Delays in seconds (≈0 → 0.1 s, like browsers).
/// None for still images.
pub fn gif_frames(path: &Path, side: u32) -> Option<(Vec<Raw>, Vec<f32>)> {
    let f = wic()?;
    let dec = decoder_path(&f, path)?;
    if container(&dec) != GUID_ContainerFormatGif {
        return None;
    }
    let count = unsafe { dec.GetFrameCount().ok()? };
    if count < 2 {
        return None;
    }
    let first = unsafe { dec.GetFrame(0).ok()? };
    let (mut fw, mut fh) = (0u32, 0u32);
    unsafe { first.GetSize(&mut fw, &mut fh).ok()? };
    let (mut sw, mut sh) = (fw, fh);
    if let Ok(r) = unsafe { dec.GetMetadataQueryReader() } {
        if let (Some(w), Some(h)) = (meta_u32(&r, w!("/logscrdesc/Width")), meta_u32(&r, w!("/logscrdesc/Height"))) {
            if w > 0 && h > 0 {
                (sw, sh) = (w, h);
            }
        }
    }
    if sw == 0 || sh == 0 || sw > 8192 || sh > 8192 {
        return None;
    }
    // Longest side ≤ `side` (GIFFrames: 240 px), at most 400 frames.
    let k = (side as f64 / sw.max(sh) as f64).min(1.0);
    let (tw, th) = (((sw as f64 * k).round() as u32).max(1), ((sh as f64 * k).round() as u32).max(1));
    let mut canvas = vec![0u8; (sw * sh * 4) as usize];
    let mut frames = Vec::new();
    let mut delays = Vec::new();
    for i in 0..count.min(400) {
        let Ok(frame) = (unsafe { dec.GetFrame(i) }) else { continue };
        let reader = unsafe { frame.GetMetadataQueryReader().ok() };
        let get = |n: PCWSTR| reader.as_ref().and_then(|r| meta_u32(r, n));
        let left = get(w!("/imgdesc/Left")).unwrap_or(0);
        let top = get(w!("/imgdesc/Top")).unwrap_or(0);
        let disposal = get(w!("/grctlext/Disposal")).unwrap_or(0);
        let delay = get(w!("/grctlext/Delay")).unwrap_or(0) as f32 / 100.0;
        let Some(fr) = frame_raw(&f, &frame, None, None) else { continue };
        let saved = if disposal == 3 { Some(canvas.clone()) } else { None };
        for y in 0..fr.h {
            let cy = top + y;
            if cy >= sh {
                break;
            }
            for x in 0..fr.w {
                let cx = left + x;
                if cx >= sw {
                    break;
                }
                let si = ((y * fr.w + x) * 4) as usize;
                let a = fr.px[si + 3] as u32;
                if a == 0 {
                    continue;
                }
                let di = ((cy * sw + cx) * 4) as usize;
                for c in 0..4 {
                    let d = canvas[di + c] as u32;
                    canvas[di + c] = (fr.px[si + c] as u32 + d * (255 - a) / 255).min(255) as u8;
                }
            }
        }
        let snap = Raw { w: sw, h: sh, px: canvas.clone() };
        if let Some(r) = resize_raw(&f, &snap, tw, th) {
            frames.push(r);
            delays.push(if delay <= 0.011 { 0.1 } else { delay });
        }
        match disposal {
            2 => {
                for y in top..(top + fr.h).min(sh) {
                    let a = ((y * sw + left.min(sw)) * 4) as usize;
                    let b = ((y * sw + (left + fr.w).min(sw)) * 4) as usize;
                    canvas[a..b].fill(0);
                }
            }
            3 => {
                if let Some(s) = saved {
                    canvas = s;
                }
            }
            _ => {}
        }
    }
    if frames.len() > 1 { Some((frames, delays)) } else { None }
}

/// An SVG's intended size from its root `width`/`height` (or `viewBox`).
pub fn svg_size(data: &[u8]) -> Option<(f32, f32)> {
    let text = String::from_utf8_lossy(&data[..data.len().min(16384)]).to_string();
    let lower = text.to_lowercase();
    let start = lower.find("<svg")?;
    let end = start + lower[start..].find('>')?;
    let tag = &text[start..end];
    let attr = |name: &str| -> Option<String> {
        let tl = tag.to_lowercase();
        let mut from = 0;
        while let Some(i) = tl[from..].find(name) {
            let at = from + i;
            let before = tl[..at].chars().last().unwrap_or(' ');
            let rest = tl[at + name.len()..].trim_start();
            if before.is_whitespace() && rest.starts_with('=') {
                let rest = rest[1..].trim_start();
                let q = rest.chars().next()?;
                if q == '"' || q == '\'' {
                    let off = tag.len() - rest.len();
                    let val = &tag[off + 1..];
                    return val.find(q).map(|e| val[..e].to_string());
                }
            }
            from = at + name.len();
        }
        None
    };
    let num = |v: String| v.trim().trim_end_matches("px").trim().parse::<f32>().ok().filter(|n| *n > 0.0);
    if let (Some(w), Some(h)) = (attr("width").and_then(num), attr("height").and_then(num)) {
        return Some((w, h));
    }
    let vb = attr("viewbox")?;
    let parts: Vec<f32> = vb.split([' ', ',']).filter(|p| !p.is_empty()).filter_map(|p| p.parse().ok()).collect();
    if parts.len() == 4 && parts[2] > 0.0 && parts[3] > 0.0 { Some((parts[2], parts[3])) } else { None }
}

/// Full-resolution pixels for the editor plus their scale: Retina-style
/// screenshots record their DPI (144 dpi → 1.5×); SVGs rasterise at 2×.
pub fn decode_for_edit(path: &Path) -> Option<(Raw, f32)> {
    if is_svg_path(path) {
        let data = std::fs::read(path).ok()?;
        let (w, h) = svg_size(&data).unwrap_or((512.0, 512.0));
        let long = w.max(h);
        let side = (long * 2.0).clamp(256.0, 4096.0);
        let k = side / long;
        let raw = svg_raster(&data, ((w * k).round() as u32).max(1), ((h * k).round() as u32).max(1))?;
        return Some((raw, 2.0));
    }
    let f = wic()?;
    let dec = decoder_path(&f, path)?;
    let frame = unsafe { dec.GetFrame(0).ok()? };
    let (mut dx, mut dy) = (96.0f64, 96.0f64);
    unsafe {
        let _ = frame.GetResolution(&mut dx, &mut dy);
    }
    let scale = if dx > 96.5 && dx < 960.0 { (dx / 96.0) as f32 } else { 1.0 };
    Some((frame_raw(&f, &frame, None, None)?, scale))
}

/// Pixel size without decoding.
pub fn size_of(data: &[u8]) -> Option<(u32, u32)> {
    let f = wic()?;
    let dec = decoder_bytes(&f, data)?;
    unsafe {
        let frame = dec.GetFrame(0).ok()?;
        let (mut w, mut h) = (0, 0);
        frame.GetSize(&mut w, &mut h).ok()?;
        Some((w, h))
    }
}

fn encode(raw: &Raw, container: &GUID, quality: Option<f32>, dpi: Option<f64>) -> Option<Vec<u8>> {
    let f = wic()?;
    unsafe {
        let stream = SHCreateMemStream(None)?;
        let enc = f.CreateEncoder(container, std::ptr::null()).ok()?;
        enc.Initialize(&stream, WICBitmapEncoderNoCache).ok()?;
        let mut frame: Option<IWICBitmapFrameEncode> = None;
        let mut bag: Option<IPropertyBag2> = None;
        enc.CreateNewFrame(&mut frame, &mut bag).ok()?;
        let frame = frame?;
        if let (Some(q), Some(bag)) = (quality, bag.as_ref()) {
            let name = crate::util::wide("ImageQuality");
            let opt = PROPBAG2 { pstrName: windows::core::PWSTR(name.as_ptr() as *mut u16), ..Default::default() };
            let v = VARIANT::from(q);
            let _ = bag.Write(1, &opt, &v);
        }
        frame.Initialize(bag.as_ref()).ok()?;
        frame.SetSize(raw.w, raw.h).ok()?;
        if let Some(d) = dpi {
            let _ = frame.SetResolution(d, d);
        }
        let bmp = f
            .CreateBitmapFromMemory(raw.w, raw.h, &GUID_WICPixelFormat32bppPBGRA, raw.w * 4, &raw.px)
            .ok()?;
        let target = if *container == GUID_ContainerFormatJpeg { GUID_WICPixelFormat24bppBGR } else { GUID_WICPixelFormat32bppBGRA };
        let mut pf = target;
        frame.SetPixelFormat(&mut pf).ok()?;
        let conv = f.CreateFormatConverter().ok()?;
        conv.Initialize(&bmp, &pf, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
        frame.WriteSource(&conv, std::ptr::null()).ok()?;
        frame.Commit().ok()?;
        enc.Commit().ok()?;
        read_stream(&stream)
    }
}

pub fn read_stream(stream: &IStream) -> Option<Vec<u8>> {
    unsafe {
        let mut stat = STATSTG::default();
        stream.Stat(&mut stat, STATFLAG_NONAME).ok()?;
        let size = stat.cbSize as usize;
        stream.Seek(0, STREAM_SEEK_SET, None).ok()?;
        let mut out = vec![0u8; size];
        let mut read = 0u32;
        let mut off = 0usize;
        while off < size {
            let chunk = (size - off).min(1 << 20) as u32;
            let hr = stream.Read(out[off..].as_mut_ptr() as *mut _, chunk, Some(&mut read));
            if hr.is_err() || read == 0 {
                break;
            }
            off += read as usize;
        }
        out.truncate(off);
        Some(out)
    }
}

/// PNG; `dpi` records the point size (Mac's rep.size = point size).
pub fn encode_png(raw: &Raw, dpi: Option<f64>) -> Option<Vec<u8>> {
    encode(raw, &GUID_ContainerFormatPng, None, dpi)
}

pub fn encode_jpeg(raw: &Raw, quality: f32) -> Option<Vec<u8>> {
    encode(raw, &GUID_ContainerFormatJpeg, Some(quality), None)
}

/// PNG bytes for the clipboard (keeps PNG files untouched).
pub fn png_bytes(path: &Path) -> Option<Vec<u8>> {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if ext == "png" {
        return std::fs::read(path).ok();
    }
    let raw = decode_path(path)?;
    encode_png(&raw, None)
}

/// CF_DIBV5 (32-bit, straight alpha, bottom-up) for apps that read bitmaps.
pub fn to_dibv5(raw: &Raw) -> Vec<u8> {
    let header = 124usize;
    let mut out = Vec::with_capacity(header + raw.px.len());
    let le32 = |v: u32| v.to_le_bytes();
    out.extend_from_slice(&le32(124));
    out.extend_from_slice(&(raw.w as i32).to_le_bytes());
    out.extend_from_slice(&(raw.h as i32).to_le_bytes()); // positive: bottom-up
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&le32(3)); // BI_BITFIELDS
    out.extend_from_slice(&le32(raw.px.len() as u32));
    out.extend_from_slice(&le32(2835));
    out.extend_from_slice(&le32(2835));
    out.extend_from_slice(&le32(0));
    out.extend_from_slice(&le32(0));
    out.extend_from_slice(&le32(0x00ff0000));
    out.extend_from_slice(&le32(0x0000ff00));
    out.extend_from_slice(&le32(0x000000ff));
    out.extend_from_slice(&le32(0xff000000));
    out.extend_from_slice(&le32(0x7352_4742)); // LCS_sRGB 'sRGB'
    out.resize(header - 4 * 4, 0); // endpoints + gammas
    out.extend_from_slice(&le32(4)); // LCS_GM_IMAGES
    out.extend_from_slice(&le32(0));
    out.extend_from_slice(&le32(0));
    out.extend_from_slice(&le32(0));
    debug_assert_eq!(out.len(), header);
    let stride = (raw.w * 4) as usize;
    for y in (0..raw.h as usize).rev() {
        let row = &raw.px[y * stride..(y + 1) * stride];
        for p in row.chunks_exact(4) {
            let a = p[3] as u32;
            if a == 0 {
                out.extend_from_slice(&[0, 0, 0, 0]);
            } else if a == 255 {
                out.extend_from_slice(p);
            } else {
                let un = |c: u8| ((c as u32 * 255 + a / 2) / a).min(255) as u8;
                out.extend_from_slice(&[un(p[0]), un(p[1]), un(p[2]), p[3]]);
            }
        }
    }
    out
}

/// Clipboard DIB (CF_DIB / CF_DIBV5 bytes) → image, via WIC's BMP decoder.
pub fn from_dib(dib: &[u8]) -> Option<Raw> {
    if dib.len() < 40 {
        return None;
    }
    let header_size = u32::from_le_bytes(dib[0..4].try_into().ok()?) as usize;
    let bit_count = u16::from_le_bytes(dib[14..16].try_into().ok()?) as usize;
    let compression = u32::from_le_bytes(dib[16..20].try_into().ok()?);
    let clr_used = u32::from_le_bytes(dib[32..36].try_into().ok()?) as usize;
    let mut palette = if bit_count <= 8 { if clr_used > 0 { clr_used } else { 1 << bit_count } } else { clr_used } * 4;
    if compression == 3 && header_size == 40 {
        palette += 12;
    }
    let offset = 14 + header_size + palette;
    let mut bmp = Vec::with_capacity(14 + dib.len());
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&((14 + dib.len()) as u32).to_le_bytes());
    bmp.extend_from_slice(&[0, 0, 0, 0]);
    bmp.extend_from_slice(&(offset as u32).to_le_bytes());
    bmp.extend_from_slice(dib);
    let mut raw = decode_bytes(&bmp, None)?;
    // 32-bit BI_RGB DIBs usually leave alpha at 0: treat all-zero alpha as opaque.
    if bit_count == 32 && compression == 0 && raw.px.chunks_exact(4).all(|p| p[3] == 0) {
        for p in raw.px.chunks_exact_mut(4) {
            p[3] = 255;
        }
    }
    Some(raw)
}

/// Rasterise SVG with Direct2D into a `w`×`h` box (aspect-fit, centred).
pub fn svg_raster(data: &[u8], w: u32, h: u32) -> Option<Raw> {
    let f = wic()?;
    unsafe {
        let d2f: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_MULTI_THREADED, None).ok()?;
        let bmp = f.CreateBitmap(w, h, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnLoad).ok()?;
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let rt = d2f.CreateWicBitmapRenderTarget(&bmp, &props).ok()?;
        let dc: ID2D1DeviceContext5 = rt.cast().ok()?;
        let stream = stream_from(data)?;
        let doc = dc.CreateSvgDocument(&stream, D2D_SIZE_F { width: w as f32, height: h as f32 }).ok()?;
        dc.BeginDraw();
        dc.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
        dc.DrawSvgDocument(&doc);
        dc.EndDraw(None, None).ok()?;
        drop(dc);
        drop(rt);
        let mut px = vec![0u8; (w * h * 4) as usize];
        bmp.CopyPixels(std::ptr::null(), w * 4, &mut px).ok()?;
        Some(Raw { w, h, px })
    }
}

/// Simple box downscale to fit `max` (for tiny thumbnails from a Raw).
pub fn scale_raw(raw: &Raw, max: u32) -> Raw {
    let long = raw.w.max(raw.h);
    if long <= max {
        return raw.clone();
    }
    let f = wic();
    if let Some(f) = f {
        unsafe {
            if let Ok(bmp) = f.CreateBitmapFromMemory(raw.w, raw.h, &GUID_WICPixelFormat32bppPBGRA, raw.w * 4, &raw.px) {
                let k = max as f64 / long as f64;
                let (w, h) = (((raw.w as f64 * k).round() as u32).max(1), ((raw.h as f64 * k).round() as u32).max(1));
                if let Ok(sc) = f.CreateBitmapScaler() {
                    if sc.Initialize(&bmp, w, h, WICBitmapInterpolationModeFant).is_ok() {
                        let mut px = vec![0u8; (w * h * 4) as usize];
                        if sc.CopyPixels(std::ptr::null(), w * 4, &mut px).is_ok() {
                            return Raw { w, h, px };
                        }
                    }
                }
            }
        }
    }
    raw.clone()
}

/// Grayscale in place (PlayerIcon.grayscale).
pub fn grayscale(raw: &mut Raw) {
    for p in raw.px.chunks_exact_mut(4) {
        let v = (0.299 * p[2] as f32 + 0.587 * p[1] as f32 + 0.114 * p[0] as f32) as u8;
        p[0] = v;
        p[1] = v;
        p[2] = v;
    }
}

pub fn is_image_path(path: &Path) -> bool {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "jpe" | "jfif" | "gif" | "bmp" | "dib" | "tif" | "tiff" | "webp" | "heic" | "heif" | "avif"
            | "ico" | "svg" | "jxr" | "wdp" | "dds"
    )
}
