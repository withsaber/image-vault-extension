//! Image Vault for Windows — a Dynamic-Island-style image vault.
//! Native Rust: Win32 + Direct2D + DirectComposition + DirectWrite + WIC.

#![windows_subsystem = "windows"]

mod anim;
mod app;
mod capture;
mod clipboard;
mod cliphistory;
mod dragdrop;
mod editor;
mod gfx;
mod hotkeys;
mod icons;
mod imaging;
mod ingest;
mod island;
mod menu;
mod monitors;
mod nowplaying;
mod ocr;
mod screenocr;
mod settings;
mod snapshot;
mod statusitem;
mod textfield;
mod theme;
mod thumbs;
mod tooltip;
mod tray;
mod ui;
mod util;
mod vault;
mod views;

use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
use windows::Win32::System::Ole::OleInitialize;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::core::w;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // Developer aids (print to the launching console).
    if let Some(i) = args.iter().position(|a| a == "--ocr") {
        unsafe {
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        }
        let langs = settings::Settings::load().ocr_extra_languages;
        match args.get(i + 1).and_then(|p| ocr::cli(p, &langs)) {
            Some(t) => {
                println!("{t}");
                std::process::exit(0);
            }
            None => {
                println!("NO TEXT");
                std::process::exit(1);
            }
        }
    }
    if let Some(i) = args.iter().position(|a| a == "--snapshot") {
        unsafe {
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let _ = OleInitialize(None);
        }
        ui::init_with(ui::Hooks { tick: app::tick, mouse: app::mouse, message: app::message }, false);
        snapshot::run(args.get(i + 1).map(String::as_str).unwrap_or("snapshots"));
        std::process::exit(0);
    }
    if let Some(i) = args.iter().position(|a| a == "--capture-demo") {
        unsafe {
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let _ = OleInitialize(None);
        }
        ui::init_with(ui::Hooks { tick: app::tick, mouse: app::mouse, message: app::message }, false);
        snapshot::capture_demo(args.get(i + 1).map(String::as_str).unwrap_or(""), args.get(i + 2).map(String::as_str).unwrap_or("capture-demo"));
        std::process::exit(0);
    }
    if let Some(i) = args.iter().position(|a| a == "--blur-test") {
        snapshot::blur_test(&args[i + 1], &args[i + 2]);
        std::process::exit(0);
    }
    if args.iter().any(|a| a == "--quit") {
        app::quit_running_instance();
        return;
    }
    if args.iter().any(|a| a == "--nowplaying") {
        unsafe {
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        }
        nowplaying::print_now_playing();
        std::process::exit(0);
    }

    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _mutex = CreateMutexW(None, true, w!("Local\\ImageVault.Singleton"));
        if windows::Win32::Foundation::GetLastError() == windows::Win32::Foundation::ERROR_ALREADY_EXISTS {
            if app::signal_running_instance() {
                return;
            }
        }
        let _ = OleInitialize(None);
        std::panic::set_hook(Box::new(|info| {
            util::log(&format!("panic: {info}"));
        }));
        menu::enable_dark();
        ui::init(ui::Hooks { tick: app::tick, mouse: app::mouse, message: app::message });
        app::init();
        ui::run_loop();
        app::shutdown();
        std::mem::forget(_mutex);
    }
}
