// Test helper: optional mouse moves/clicks, then a screenshot of a region.
// usage: snap <x> <y> <w> <h> <out.png> [cmd...]
// cmds: move:X,Y  wait:MS  click  rclick  down  up  key:VK  ctrlkey:VK  text:STRING  wheel:N
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

fn send_mouse(flags: MOUSE_EVENT_FLAGS, x: i32, y: i32, data: i32) {
    unsafe {
        let (sw, sh) = (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN));
        let (sx, sy) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
        let nx = ((x - sx) as i64 * 65535 / (sw - 1).max(1) as i64) as i32;
        let ny = ((y - sy) as i64 * 65535 / (sh - 1).max(1) as i64) as i32;
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT { dx: nx, dy: ny, mouseData: data as u32, dwFlags: flags | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK, time: 0, dwExtraInfo: 0 },
            },
        };
        SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
}

fn key(vk: u16, up: bool) {
    unsafe {
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, time: 0, dwExtraInfo: 0 } },
        };
        SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
}

fn unicode(c: u16) {
    unsafe {
        let mk = |up: bool| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0), wScan: c, dwFlags: KEYEVENTF_UNICODE | if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, time: 0, dwExtraInfo: 0 } },
        };
        SendInput(&[mk(false), mk(true)], std::mem::size_of::<INPUT>() as i32);
    }
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().collect();
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let (x, y, w, h): (i32, i32, i32, i32) = (a[1].parse().unwrap(), a[2].parse().unwrap(), a[3].parse().unwrap(), a[4].parse().unwrap());
    let out = &a[5];
    let mut cur = unsafe {
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        (p.x, p.y)
    };
    for cmd in &a[6..] {
        let (c, v) = cmd.split_once(':').unwrap_or((cmd.as_str(), ""));
        match c {
            "move" => {
                let (mx, my) = v.split_once(',').unwrap();
                // Glide in steps so hover logic sees motion.
                let (tx, ty): (i32, i32) = (mx.parse().unwrap(), my.parse().unwrap());
                for i in 1..=8 {
                    let px = cur.0 + (tx - cur.0) * i / 8;
                    let py = cur.1 + (ty - cur.1) * i / 8;
                    send_mouse(MOUSEEVENTF_MOVE, px, py, 0);
                    std::thread::sleep(std::time::Duration::from_millis(12));
                }
                cur = (tx, ty);
                std::thread::sleep(std::time::Duration::from_millis(20));
                let mut p = POINT::default();
                unsafe { let _ = GetCursorPos(&mut p); }
                eprintln!("move to {tx},{ty} -> cursor {},{}", p.x, p.y);
            }
            "wait" => std::thread::sleep(std::time::Duration::from_millis(v.parse().unwrap())),
            "click" => {
                send_mouse(MOUSEEVENTF_LEFTDOWN, cur.0, cur.1, 0);
                std::thread::sleep(std::time::Duration::from_millis(40));
                send_mouse(MOUSEEVENTF_LEFTUP, cur.0, cur.1, 0);
            }
            "dclick" => {
                for _ in 0..2 {
                    send_mouse(MOUSEEVENTF_LEFTDOWN, cur.0, cur.1, 0);
                    std::thread::sleep(std::time::Duration::from_millis(30));
                    send_mouse(MOUSEEVENTF_LEFTUP, cur.0, cur.1, 0);
                    std::thread::sleep(std::time::Duration::from_millis(60));
                }
            }
            "rclick" => {
                send_mouse(MOUSEEVENTF_RIGHTDOWN, cur.0, cur.1, 0);
                std::thread::sleep(std::time::Duration::from_millis(40));
                send_mouse(MOUSEEVENTF_RIGHTUP, cur.0, cur.1, 0);
            }
            "down" => send_mouse(MOUSEEVENTF_LEFTDOWN, cur.0, cur.1, 0),
            "up" => send_mouse(MOUSEEVENTF_LEFTUP, cur.0, cur.1, 0),
            "wheel" => send_mouse(MOUSEEVENTF_WHEEL, cur.0, cur.1, v.parse::<i32>().unwrap()),
            "key" => {
                let vk: u16 = u16::from_str_radix(v, 16).unwrap();
                key(vk, false);
                key(vk, true);
            }
            "ctrlkey" => {
                let vk: u16 = u16::from_str_radix(v, 16).unwrap();
                key(0x11, false);
                key(vk, false);
                key(vk, true);
                key(0x11, true);
            }
            "combo" => {
                // combo:11+12+54 (hex VKs pressed in order, released reversed)
                let vks: Vec<u16> = v.split('+').map(|k| u16::from_str_radix(k, 16).unwrap()).collect();
                for k in &vks {
                    key(*k, false);
                }
                for k in vks.iter().rev() {
                    key(*k, true);
                }
            }
            "text" => {
                for c in v.encode_utf16() {
                    unicode(c);
                    std::thread::sleep(std::time::Duration::from_millis(15));
                }
            }
            _ => {}
        }
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    if out == "-" {
        return Ok(());
    }
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bmp.into());
        let _ = BitBlt(mem, 0, 0, w, h, Some(screen), x, y, SRCCOPY | CAPTUREBLT);
        SelectObject(mem, old);
        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() },
            ..Default::default()
        };
        let mut px = vec![0u8; (w * h * 4) as usize];
        GetDIBits(mem, bmp, 0, h as u32, Some(px.as_mut_ptr() as *mut _), &mut bi, DIB_RGB_COLORS);
        for p in px.chunks_exact_mut(4) {
            p[3] = 255;
        }
        let f: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let stream = f.CreateStream()?;
        let wout: Vec<u16> = out.encode_utf16().chain(Some(0)).collect();
        stream.InitializeFromFilename(PCWSTR(wout.as_ptr()), 0x40000000)?;
        let enc = f.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        enc.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let mut frame = None;
        let mut bag = None;
        enc.CreateNewFrame(&mut frame, &mut bag)?;
        let frame = frame.unwrap();
        frame.Initialize(bag.as_ref())?;
        frame.SetSize(w as u32, h as u32)?;
        let mut pf = GUID_WICPixelFormat32bppBGRA;
        frame.SetPixelFormat(&mut pf)?;
        frame.WritePixels(h as u32, (w * 4) as u32, &px)?;
        frame.Commit()?;
        enc.Commit()?;
    }
    Ok(())
}
