//! Displays: bounds, work area, DPI scale and friendly names.

use windows::Win32::Devices::Display::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::*;

#[derive(Debug, Clone)]
pub struct Monitor {
    pub handle: HMONITOR,
    pub device: String,
    pub rect: RECT,
    pub work: RECT,
    pub scale: f32,
    pub primary: bool,
    pub name: String,
}

unsafe extern "system" fn enum_proc(h: HMONITOR, _dc: HDC, _r: *mut RECT, data: LPARAM) -> windows::core::BOOL {
    unsafe {
        let list = &mut *(data.0 as *mut Vec<Monitor>);
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(h, &mut info as *mut _ as *mut MONITORINFO).as_bool() {
            let mut dx = 96u32;
            let mut dy = 96u32;
            let _ = GetDpiForMonitor(h, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
            let device = crate::util::from_wide(&info.szDevice);
            list.push(Monitor {
                handle: h,
                device: device.clone(),
                rect: info.monitorInfo.rcMonitor,
                work: info.monitorInfo.rcWork,
                scale: dx as f32 / 96.0,
                primary: info.monitorInfo.dwFlags & 1 != 0,
                name: String::new(),
            });
        }
        windows::core::BOOL(1)
    }
}

/// Monitor friendly names ("DELL U2720Q") keyed by GDI device name.
fn friendly_names() -> Vec<(String, String)> {
    let mut out = Vec::new();
    unsafe {
        let mut np = 0u32;
        let mut nm = 0u32;
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm) != ERROR_SUCCESS {
            return out;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
        if QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), None) != ERROR_SUCCESS {
            return out;
        }
        for p in paths.iter().take(np as usize) {
            let mut src = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            src.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
            src.header.size = std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
            src.header.adapterId = p.sourceInfo.adapterId;
            src.header.id = p.sourceInfo.id;
            if DisplayConfigGetDeviceInfo(&mut src.header) != 0 {
                continue;
            }
            let mut tgt = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
            tgt.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
            tgt.header.size = std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
            tgt.header.adapterId = p.targetInfo.adapterId;
            tgt.header.id = p.targetInfo.id;
            if DisplayConfigGetDeviceInfo(&mut tgt.header) != 0 {
                continue;
            }
            let dev = crate::util::from_wide(&src.viewGdiDeviceName);
            let mut name = crate::util::from_wide(&tgt.monitorFriendlyDeviceName);
            if name.is_empty() {
                name = if tgt.outputTechnology == DISPLAYCONFIG_OUTPUT_TECHNOLOGY_INTERNAL { "Built-in Display".into() } else { String::new() };
            }
            out.push((dev, name));
        }
    }
    out
}

/// All displays, primary first then left to right.
pub fn all() -> Vec<Monitor> {
    let mut list: Vec<Monitor> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(enum_proc), LPARAM(&mut list as *mut _ as isize));
    }
    let names = friendly_names();
    let n = list.len();
    for (i, m) in list.iter_mut().enumerate() {
        let friendly = names.iter().find(|(d, _)| d.eq_ignore_ascii_case(&m.device)).map(|(_, n)| n.clone()).unwrap_or_default();
        m.name = if !friendly.is_empty() {
            friendly
        } else if n > 1 {
            format!("Display {}", i + 1)
        } else {
            "Display".into()
        };
    }
    list.sort_by(|a, b| b.primary.cmp(&a.primary).then(a.rect.left.cmp(&b.rect.left)));
    // Disambiguate identical model names.
    let names: Vec<String> = list.iter().map(|m| m.name.clone()).collect();
    for (i, m) in list.iter_mut().enumerate() {
        if names.iter().filter(|x| **x == names[i]).count() > 1 {
            let k = names[..=i].iter().filter(|x| **x == names[i]).count();
            m.name = format!("{} ({k})", m.name);
        }
    }
    list
}

pub fn at_point(x: i32, y: i32) -> Option<Monitor> {
    let h = unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST) };
    all().into_iter().find(|m| m.handle == h)
}

pub fn of_window(hwnd: HWND) -> Option<Monitor> {
    let h = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    all().into_iter().find(|m| m.handle == h)
}

pub fn cursor() -> (i32, i32) {
    let mut p = POINT::default();
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p);
    }
    (p.x, p.y)
}
