//! Notification-area icon (StatusItem.swift): left-click opens / closes the
//! island; right-click shows Open, Copy Latest Image, Take Screenshot,
//! Recent (copy the last five), Music, capture toggles, stats and Quit.

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

use crate::app::{self, App};
use crate::icons::Icon;
use crate::island::{self, State};
use crate::menu::Menu;
use crate::theme::{Color, Weight};
use crate::{hotkeys, imaging, ui, vault};

#[derive(Default)]
pub struct StatusItem {
    icon: Option<HICON>,
    added: bool,
}

pub fn taskbar_created() -> u32 {
    use std::sync::OnceLock;
    static ID: OnceLock<u32> = OnceLock::new();
    *ID.get_or_init(|| unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) })
}

fn light_taskbar() -> bool {
    unsafe {
        let mut v = 0u32;
        let mut size = 4u32;
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut v as *mut _ as *mut _),
            Some(&mut size),
        )
        .is_ok()
            && v == 1
    }
}

fn hicon(raw: &crate::imaging::Raw) -> Option<HICON> {
    unsafe {
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
        let color = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        std::ptr::copy_nonoverlapping(raw.px.as_ptr(), bits as *mut u8, raw.px.len());
        let mask = CreateBitmap(raw.w as i32, raw.h as i32, 1, 1, None);
        let info = ICONINFO { fIcon: TRUE, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info).ok();
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}

fn make_icon() -> Option<HICON> {
    let g = crate::gfx::gfx();
    let px = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16) as u32;
    let color = if light_taskbar() { Color::black(1.0) } else { Color::white(1.0) };
    let raw = crate::icons::rasterize(&g, Icon::PhotoOnRectangle, px, px as f32 * 0.82, Weight::Regular, color)?;
    hicon(&raw)
}

fn data(icon: Option<HICON>) -> NOTIFYICONDATAW {
    let mut d = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: ui::msg_hwnd(),
        uID: 1,
        uFlags: NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP | if icon.is_some() { NIF_ICON } else { NOTIFY_ICON_DATA_FLAGS(0) },
        uCallbackMessage: ui::WM_APP_TRAY,
        hIcon: icon.unwrap_or_default(),
        ..Default::default()
    };
    let tip = crate::util::wide("Image Vault — click to open, right-click for settings");
    for (i, c) in tip.iter().take(127).enumerate() {
        d.szTip[i] = *c;
    }
    d
}

pub fn install(app: &mut App) {
    unsafe {
        if let Some(old) = app.status.icon.take() {
            let _ = DestroyIcon(old);
        }
        let icon = make_icon();
        app.status.icon = icon;
        let mut d = data(icon);
        if app.status.added {
            let _ = Shell_NotifyIconW(NIM_DELETE, &d);
        }
        app.status.added = Shell_NotifyIconW(NIM_ADD, &d).as_bool();
        d.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &d);
    }
}

pub fn remove(app: &mut App) {
    unsafe {
        let d = data(None);
        let _ = Shell_NotifyIconW(NIM_DELETE, &d);
        if let Some(i) = app.status.icon.take() {
            let _ = DestroyIcon(i);
        }
        app.status.added = false;
    }
}

/// Notification-icon callback (NOTIFYICON_VERSION_4 events).
pub fn event(ev: u32) {
    const NIN_SELECT: u32 = WM_USER;
    const NIN_KEYSELECT: u32 = WM_USER + 1;
    match ev {
        NIN_SELECT | NIN_KEYSELECT | WM_LBUTTONUP => {
            app::with(|a| {
                if a.island.state == State::Expanded {
                    island::collapse(a);
                } else {
                    island::reassert(a);
                    island::expand(a, false);
                }
            });
        }
        WM_CONTEXTMENU | WM_RBUTTONUP => {
            let Some(m) = app::with(build_menu) else { return };
            app::popup_menu(m, None);
        }
        _ => {}
    }
}

fn build_menu(app: &mut App) -> Menu {
    let mut m = Menu::new();
    m.action("Open Image Vault", || {
        app::with(|a| {
            island::reassert(a);
            island::expand(a, false);
        });
    });
    m.action_key("Copy Latest Image", &hotkeys::label(app, hotkeys::COPY), || {
        app::with(app::copy_latest);
    });
    m.action_key("Take Screenshot", &hotkeys::label(app, hotkeys::SHOT), || {
        ui::after(0.15, || ui::defer(crate::capture::take_screenshot));
    });

    let mut recent = Menu::new();
    let scale = app.island.scale;
    let px = (16.0 * scale).round() as u32;
    for item in app.model.items.iter().take(5) {
        let title = if item.name.chars().count() > 44 { item.name.chars().take(43).collect::<String>() + "…" } else { item.name.clone() };
        let icon = imaging::thumbnail(&vault::file_path(item), px * 2).map(|r| square(&r, px));
        let it = item.clone();
        recent.item(title, "", false, icon, move || {
            app::with(|a| app::copy_item(a, it));
        });
    }
    if recent.is_empty() {
        recent.disabled("No images yet");
    }
    m.sub("Recent", recent);

    let mut music = Menu::new();
    if let Some(t) = &app.np.track {
        music.disabled(format!("{} — {}", t.title, t.subtitle()));
        music.sep();
    }
    music.action("Play / Pause", || {
        app::with(crate::nowplaying::play_pause);
    });
    music.action("Next", || {
        app::with(crate::nowplaying::next);
    });
    music.action("Previous", || {
        app::with(crate::nowplaying::previous);
    });
    m.sub("Music", music);
    m.sep();
    m.toggle("Save Copied Images", app.settings.save_clipboard, || {
        app::with(|a| {
            a.settings.save_clipboard = !a.settings.save_clipboard;
            a.settings.save();
        });
    });
    m.toggle("Save New Screenshots", app.settings.save_screenshots, || {
        app::with(|a| {
            a.settings.save_screenshots = !a.settings.save_screenshots;
            a.settings.save();
        });
    });
    m.toggle("Music Controls", app.settings.music_controls, || {
        app::with(|a| {
            let on = !a.settings.music_controls;
            crate::nowplaying::set_enabled(a, on);
        });
    });
    m.sep();
    m.action("Open Vault Folder", || crate::util::shell_open(&vault::images_dir().display().to_string()));
    m.sep();
    m.disabled(app::stats_title(app));
    m.action("Quit Image Vault", app::quit);
    m
}

/// Aspect-fill crop to a `px` square (menu thumbnails).
fn square(r: &crate::imaging::Raw, px: u32) -> crate::imaging::Raw {
    let k = (px as f32 / r.w as f32).max(px as f32 / r.h as f32);
    let ox = (r.w as f32 * k - px as f32) / 2.0;
    let oy = (r.h as f32 * k - px as f32) / 2.0;
    let mut out = vec![0u8; (px * px * 4) as usize];
    for y in 0..px {
        for x in 0..px {
            let sx = (((x as f32 + ox) / k) as u32).min(r.w - 1);
            let sy = (((y as f32 + oy) / k) as u32).min(r.h - 1);
            let si = ((sy * r.w + sx) * 4) as usize;
            let di = ((y * px + x) * 4) as usize;
            out[di..di + 4].copy_from_slice(&r.px[si..si + 4]);
        }
    }
    crate::imaging::Raw { w: px, h: px, px: out }
}
