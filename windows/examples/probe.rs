// Probe: does a WS_EX_LAYERED|WS_EX_TRANSPARENT|WS_EX_NOREDIRECTIONBITMAP window render
// DirectComposition content, and is it skipped by WindowFromPoint?
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

unsafe extern "system" fn wnd_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(h, m, w, l) }
}

fn pump(ms: u32) {
    unsafe {
        let start = windows::Win32::System::SystemInformation::GetTickCount();
        let mut msg = MSG::default();
        while windows::Win32::System::SystemInformation::GetTickCount() - start < ms {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).into() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

fn main() -> Result<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let hinstance = GetModuleHandleW(None)?;
        RegisterClassExW(&WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance.into(),
            lpszClassName: w!("Probe"),
            ..Default::default()
        });
        let layered = std::env::args().any(|a| a == "layered");
        let mut ex = WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW | WS_EX_TOPMOST;
        if layered {
            ex |= WS_EX_LAYERED;
        }
        let hwnd = CreateWindowExW(ex, w!("Probe"), w!("probe"), WS_POPUP, 200, 200, 300, 200, None, None, Some(hinstance.into()), None)?;
        if layered {
            // Layered windows need attributes set before they show anything.
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
        }
        let mut device: Option<ID3D11Device> = None;
        D3D11CreateDevice(None::<&IDXGIAdapter>, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, None)?;
        let device = device.unwrap();
        let dxgi: IDXGIDevice = device.cast()?;
        let factory: IDXGIFactory2 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))?;
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: 300, Height: 200, Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT, BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH, SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED, ..Default::default()
        };
        let sc = factory.CreateSwapChainForComposition(&device, &desc, None)?;
        let d2d = D2D1CreateDevice(&dxgi, None)?;
        let ctx = d2d.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
        let surface: IDXGISurface = sc.GetBuffer(0)?;
        let props = D2D1_BITMAP_PROPERTIES1 {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0, dpiY: 96.0,
            bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW, ..Default::default()
        };
        let bmp = ctx.CreateBitmapFromDxgiSurface(&surface, Some(&props))?;
        ctx.SetTarget(&bmp);
        let dcomp: IDCompositionDevice = DCompositionCreateDevice(&dxgi)?;
        let target = dcomp.CreateTargetForHwnd(hwnd, true)?;
        let visual = dcomp.CreateVisual()?;
        visual.SetContent(&sc)?;
        target.SetRoot(&visual)?;
        ctx.BeginDraw();
        ctx.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
        let brush = ctx.CreateSolidColorBrush(&D2D1_COLOR_F { r: 1.0, g: 0.0, b: 0.0, a: 1.0 }, None)?;
        ctx.FillRectangle(&D2D_RECT_F { left: 0.0, top: 0.0, right: 150.0, bottom: 200.0 }, &brush);
        ctx.EndDraw(None, None)?;
        sc.Present(1, DXGI_PRESENT(0)).ok()?;
        dcomp.Commit()?;
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        pump(700);
        let dc = GetDC(None);
        let painted = GetPixel(dc, 250, 300); // inside red half
        let clear = GetPixel(dc, 420, 300); // transparent half
        println!("layered={layered} red-half pixel={:06x} clear-half pixel={:06x}", painted.0, clear.0);
        let hit = WindowFromPoint(POINT { x: 250, y: 300 });
        let hit2 = WindowFromPoint(POINT { x: 420, y: 300 });
        println!("hit red={} hit clear={}", hit == hwnd, hit2 == hwnd);
        // Now make it click-through.
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | WS_EX_TRANSPARENT.0 as isize | if layered { 0 } else { 0 });
        pump(300);
        let hit = WindowFromPoint(POINT { x: 250, y: 300 });
        let painted = GetPixel(dc, 250, 300);
        println!("after TRANSPARENT: hit red={} pixel={:06x}", hit == hwnd, painted.0);
        ReleaseDC(None, dc);
    }
    Ok(())
}
