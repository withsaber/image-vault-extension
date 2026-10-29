//! OLE drag & drop: a drop target that forwards to the app (island, drag
//! sensor), drag-out of real files through the shell data object (so
//! Explorer, browsers and chat apps all get every selected image), and the
//! drag-image helper so dragged thumbnails render over our windows.

use std::path::PathBuf;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Ole::*;
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::*;
use windows::core::{Interface, Ref, implement};

use crate::clipboard::{MARKER, format_id};

/// Which of our windows a drop event belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Island,
    Sensor,
    Editor(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Enter,
    Over,
    Leave,
    Drop,
}

/// App callback: returns the effect to show.
pub type Handler = fn(Target, Phase, Option<&IDataObject>, POINT) -> DROPEFFECT;

#[implement(IDropTarget)]
pub struct DropTarget {
    target: Target,
    hwnd: HWND,
    handler: Handler,
    helper: Option<IDropTargetHelper>,
}

impl DropTarget {
    pub fn register(hwnd: HWND, target: Target, handler: Handler) -> Option<IDropTarget> {
        let helper: Option<IDropTargetHelper> = unsafe { CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER).ok() };
        let t: IDropTarget = DropTarget { target, hwnd, handler, helper }.into();
        unsafe {
            if RegisterDragDrop(hwnd, &t).is_err() {
                return None;
            }
        }
        Some(t)
    }
}

fn pt(p: &POINTL) -> POINT {
    POINT { x: p.x, y: p.y }
}

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(&self, data: Ref<IDataObject>, _keys: MODIFIERKEYS_FLAGS, p: &POINTL, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
        let d = data.as_ref();
        let e = (self.handler)(self.target, Phase::Enter, d, pt(p));
        unsafe {
            if !effect.is_null() {
                *effect = e;
            }
            if let (Some(h), Some(d)) = (&self.helper, d) {
                let mut pp = pt(p);
                let _ = h.DragEnter(self.hwnd, d, &mut pp, e);
            }
        }
        Ok(())
    }

    fn DragOver(&self, _keys: MODIFIERKEYS_FLAGS, p: &POINTL, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
        let e = (self.handler)(self.target, Phase::Over, None, pt(p));
        unsafe {
            if !effect.is_null() {
                *effect = e;
            }
            if let Some(h) = &self.helper {
                let mut pp = pt(p);
                let _ = h.DragOver(&mut pp, e);
            }
        }
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        (self.handler)(self.target, Phase::Leave, None, POINT::default());
        unsafe {
            if let Some(h) = &self.helper {
                let _ = h.DragLeave();
            }
        }
        Ok(())
    }

    fn Drop(&self, data: Ref<IDataObject>, _keys: MODIFIERKEYS_FLAGS, p: &POINTL, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
        let d = data.as_ref();
        let e = (self.handler)(self.target, Phase::Drop, d, pt(p));
        unsafe {
            if !effect.is_null() {
                *effect = e;
            }
            if let (Some(h), Some(d)) = (&self.helper, d) {
                let mut pp = pt(p);
                let _ = h.Drop(d, &mut pp, e);
            }
        }
        Ok(())
    }
}

/// Our own drags carry the marker (so the island ignores them).
pub fn is_ours(d: &IDataObject) -> bool {
    let f = FORMATETC {
        cfFormat: format_id(MARKER) as u16,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    unsafe { d.QueryGetData(&f) == S_OK }
}

fn set_hglobal(d: &IDataObject, cf: u32, bytes: &[u8]) {
    unsafe {
        let Ok(h) = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)) else { return };
        let p = GlobalLock(h);
        if p.is_null() {
            return;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p as *mut u8, bytes.len());
        let _ = GlobalUnlock(h);
        let f = FORMATETC { cfFormat: cf as u16, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
        let mut m = STGMEDIUM { tymed: TYMED_HGLOBAL.0 as u32, u: STGMEDIUM_0 { hGlobal: h }, pUnkForRelease: std::mem::ManuallyDrop::new(None) };
        if d.SetData(&f, &mut m, true).is_err() {
            let _ = GlobalFree(Some(h));
        }
    }
}

/// Shell data object for real files (CF_HDROP, shell ID lists, file contents).
pub fn files_object(paths: &[PathBuf]) -> Option<IDataObject> {
    unsafe {
        let pidls: Vec<*const ITEMIDLIST> = paths
            .iter()
            .map(|p| {
                let w = crate::util::wide_path(p);
                ILCreateFromPathW(windows::core::PCWSTR(w.as_ptr())) as *const ITEMIDLIST
            })
            .filter(|p| !p.is_null())
            .collect();
        if pidls.is_empty() {
            return None;
        }
        let obj: Option<IDataObject> = SHCreateDataObject(None, Some(&pidls), None::<&IDataObject>).ok();
        for p in pidls {
            ILFree(Some(p));
        }
        let obj = obj?;
        set_hglobal(&obj, format_id(MARKER), &[1]);
        set_hglobal(&obj, format_id("Preferred DropEffect"), &1u32.to_le_bytes());
        Some(obj)
    }
}

/// Text drag (clipboard history cards).
pub fn text_object(text: &str) -> Option<IDataObject> {
    unsafe {
        let obj: IDataObject = SHCreateDataObject(None, None, None::<&IDataObject>).ok()?;
        let w: Vec<u8> = crate::util::wide(text).iter().flat_map(|c| c.to_le_bytes()).collect();
        set_hglobal(&obj, CF_UNICODETEXT.0 as u32, &w);
        set_hglobal(&obj, format_id(MARKER), &[1]);
        Some(obj)
    }
}

/// Attach a drag image (premultiplied BGRA) with the cursor at `offset`.
pub fn set_drag_image(obj: &IDataObject, raw: &crate::imaging::Raw, offset: POINT) {
    unsafe {
        let Ok(helper) = CoCreateInstance::<_, IDragSourceHelper>(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER) else { return };
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: raw.w as i32,
                biHeight: -(raw.h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let Ok(hbm) = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else { return };
        if bits.is_null() {
            return;
        }
        std::ptr::copy_nonoverlapping(raw.px.as_ptr(), bits as *mut u8, raw.px.len());
        let img = SHDRAGIMAGE {
            sizeDragImage: SIZE { cx: raw.w as i32, cy: raw.h as i32 },
            ptOffset: offset,
            hbmpDragImage: hbm,
            crColorKey: COLORREF(0xFFFFFFFF),
        };
        if helper.InitializeFromBitmap(&img, obj).is_err() {
            let _ = DeleteObject(hbm.into());
        }
    }
}

#[implement(IDropSource)]
struct Source;

impl IDropSource_Impl for Source_Impl {
    fn QueryContinueDrag(&self, esc: windows::core::BOOL, keys: MODIFIERKEYS_FLAGS) -> windows::core::HRESULT {
        use windows::Win32::System::SystemServices::MK_LBUTTON;
        if esc.as_bool() {
            return DRAGDROP_S_CANCEL;
        }
        if keys.0 & MK_LBUTTON.0 == 0 {
            return DRAGDROP_S_DROP;
        }
        S_OK
    }
    fn GiveFeedback(&self, _e: DROPEFFECT) -> windows::core::HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

/// Modal drag; returns the effect (DROPEFFECT_NONE when cancelled).
/// Never call while app state is borrowed.
pub fn run(obj: &IDataObject) -> DROPEFFECT {
    unsafe {
        let src: IDropSource = Source.into();
        let mut effect = DROPEFFECT_NONE;
        let hr = DoDragDrop(obj, &src, DROPEFFECT_COPY, &mut effect);
        if hr == DRAGDROP_S_DROP { effect } else { DROPEFFECT_NONE }
    }
}

pub fn effect_copy() -> DROPEFFECT {
    DROPEFFECT_COPY
}

pub fn effect_none() -> DROPEFFECT {
    DROPEFFECT_NONE
}

pub fn unregister(hwnd: HWND) {
    unsafe {
        let _ = RevokeDragDrop(hwnd);
    }
}

