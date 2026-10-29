//! UI-thread dispatcher: a message-only window that runs closures posted from
//! worker threads, deferred work (run after the current handler returns, so no
//! state is borrowed while Win32 pumps nested messages), one-shot timers
//! (DispatchQueue.main.asyncAfter), the vsync frame clock and the global
//! low-level mouse hook.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

pub const WM_APP_RUN: u32 = WM_APP + 1;
pub const WM_APP_TICK: u32 = WM_APP + 2;
pub const WM_APP_MOUSE: u32 = WM_APP + 3;
pub const WM_APP_TRAY: u32 = WM_APP + 4;
pub const WM_APP_SHOW: u32 = WM_APP + 5;
pub const WM_APP_QUIT: u32 = WM_APP + 6;

static MSG_HWND: AtomicIsize = AtomicIsize::new(0);
type Job = Box<dyn FnOnce() + Send>;
static POSTED: Mutex<Vec<Job>> = Mutex::new(Vec::new());
static RUN_PENDING: AtomicBool = AtomicBool::new(false);

pub fn msg_hwnd() -> HWND {
    HWND(MSG_HWND.load(Ordering::SeqCst) as *mut _)
}

/// Run `f` on the UI thread (callable from any thread).
pub fn post(f: impl FnOnce() + Send + 'static) {
    POSTED.lock().unwrap_or_else(|e| e.into_inner()).push(Box::new(f));
    if !RUN_PENDING.swap(true, Ordering::SeqCst) {
        unsafe {
            let _ = PostMessageW(Some(msg_hwnd()), WM_APP_RUN, WPARAM(0), LPARAM(0));
        }
    }
}

fn run_posted() {
    RUN_PENDING.store(false, Ordering::SeqCst);
    let jobs: Vec<Job> = std::mem::take(&mut *POSTED.lock().unwrap_or_else(|e| e.into_inner()));
    for j in jobs {
        j();
        drain();
    }
}

thread_local! {
    static DEFERRED: RefCell<VecDeque<Box<dyn FnOnce()>>> = RefCell::new(VecDeque::new());
    static TIMERS: RefCell<Timers> = RefCell::new(Timers::default());
}

/// Run `f` as soon as the current message handler has returned.
pub fn defer(f: impl FnOnce() + 'static) {
    DEFERRED.with(|d| d.borrow_mut().push_back(Box::new(f)));
}

/// Execute deferred work (called after every top-level message).
pub fn drain() {
    loop {
        let next = DEFERRED.with(|d| d.borrow_mut().pop_front());
        match next {
            Some(f) => f(),
            None => break,
        }
    }
}

#[derive(Default)]
struct Timers {
    next_id: u64,
    order: BTreeMap<(u64, u64), ()>,
    jobs: HashMap<u64, (u64, Box<dyn FnOnce()>)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TimerId(pub u64);

fn now_us() -> u64 {
    (crate::util::mono() * 1_000_000.0) as u64
}

/// One-shot timer on the UI thread.
pub fn after(secs: f64, f: impl FnOnce() + 'static) -> TimerId {
    let due = now_us() + (secs.max(0.0) * 1_000_000.0) as u64;
    let id = TIMERS.with(|t| {
        let mut t = t.borrow_mut();
        t.next_id += 1;
        let id = t.next_id;
        t.order.insert((due, id), ());
        t.jobs.insert(id, (due, Box::new(f)));
        id
    });
    reschedule();
    TimerId(id)
}

pub fn cancel(id: TimerId) {
    TIMERS.with(|t| {
        let mut t = t.borrow_mut();
        if let Some((due, _)) = t.jobs.remove(&id.0) {
            t.order.remove(&(due, id.0));
        }
    });
}

pub fn pending(id: TimerId) -> bool {
    TIMERS.with(|t| t.borrow().jobs.contains_key(&id.0))
}

fn reschedule() {
    let next = TIMERS.with(|t| t.borrow().order.keys().next().map(|(d, _)| *d));
    unsafe {
        match next {
            Some(due) => {
                let now = now_us();
                let ms = if due > now { ((due - now) / 1000).max(1) as u32 } else { 1 };
                SetTimer(Some(msg_hwnd()), 1, ms.max(USER_TIMER_MINIMUM), None);
            }
            None => {
                let _ = KillTimer(Some(msg_hwnd()), 1);
            }
        }
    }
}

fn fire_timers() {
    loop {
        let now = now_us() + 2000; // fire anything within 2 ms
        let job = TIMERS.with(|t| {
            let mut t = t.borrow_mut();
            let first = t.order.keys().next().copied();
            match first {
                Some((due, id)) if due <= now => {
                    t.order.remove(&(due, id));
                    t.jobs.remove(&id).map(|(_, f)| f)
                }
                _ => None,
            }
        });
        match job {
            Some(f) => {
                f();
                drain();
            }
            None => break,
        }
    }
    reschedule();
}

// ---------------------------------------------------------------------------
// Frame clock
// ---------------------------------------------------------------------------

static ANIMATING: AtomicBool = AtomicBool::new(false);
static TICK_PENDING: AtomicBool = AtomicBool::new(false);
static WAKE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());
pub static FRAME_COUNT: AtomicU64 = AtomicU64::new(0);

/// Keep vsync ticks coming while `on`.
pub fn animate(on: bool) {
    let was = ANIMATING.swap(on, Ordering::SeqCst);
    if on && !was {
        let (m, c) = &WAKE;
        *m.lock().unwrap_or_else(|e| e.into_inner()) = true;
        c.notify_all();
    }
}

fn vsync_thread() {
    unsafe {
        let factory: Option<IDXGIFactory1> = CreateDXGIFactory1().ok();
        let output: Option<IDXGIOutput> = factory.as_ref().and_then(|f| f.EnumAdapters1(0).ok()).and_then(|a| a.EnumOutputs(0).ok());
        loop {
            if !ANIMATING.load(Ordering::SeqCst) {
                let (m, c) = &WAKE;
                let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
                while !*g && !ANIMATING.load(Ordering::SeqCst) {
                    g = c.wait(g).unwrap_or_else(|e| e.into_inner());
                }
                *g = false;
                continue;
            }
            let waited = match &output {
                Some(o) => o.WaitForVBlank().is_ok(),
                None => false,
            };
            if !waited {
                std::thread::sleep(std::time::Duration::from_millis(16));
            }
            if !TICK_PENDING.swap(true, Ordering::SeqCst) {
                let _ = PostMessageW(Some(msg_hwnd()), WM_APP_TICK, WPARAM(0), LPARAM(0));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Global mouse (WH_MOUSE_LL on its own thread, coalesced to the UI thread)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MouseEv {
    Move(i32, i32),
    LDown(i32, i32),
    LUp(i32, i32),
    RDown(i32, i32),
    RUp(i32, i32),
}

static MOUSE_EVENTS: Mutex<Vec<MouseEv>> = Mutex::new(Vec::new());
static MOUSE_PENDING: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn ll_mouse(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        if code >= 0 {
            let info = &*(l.0 as *const MSLLHOOKSTRUCT);
            let (x, y) = (info.pt.x, info.pt.y);
            let ev = match w.0 as u32 {
                WM_MOUSEMOVE => Some(MouseEv::Move(x, y)),
                WM_LBUTTONDOWN => Some(MouseEv::LDown(x, y)),
                WM_LBUTTONUP => Some(MouseEv::LUp(x, y)),
                WM_RBUTTONDOWN => Some(MouseEv::RDown(x, y)),
                WM_RBUTTONUP => Some(MouseEv::RUp(x, y)),
                _ => None,
            };
            if let Some(ev) = ev {
                let mut q = MOUSE_EVENTS.lock().unwrap_or_else(|e| e.into_inner());
                // Coalesce consecutive moves.
                if let (Some(MouseEv::Move(..)), MouseEv::Move(..)) = (q.last(), ev) {
                    q.pop();
                }
                if q.len() < 256 {
                    q.push(ev);
                }
                drop(q);
                if !MOUSE_PENDING.swap(true, Ordering::SeqCst) {
                    let _ = PostMessageW(Some(msg_hwnd()), WM_APP_MOUSE, WPARAM(0), LPARAM(0));
                }
            }
        }
        CallNextHookEx(None, code, w, l)
    }
}

fn hook_thread() {
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let _hook = SetWindowsHookExW(WH_MOUSE_LL, Some(ll_mouse), Some(hinst.into()), 0);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

pub fn take_mouse_events() -> Vec<MouseEv> {
    MOUSE_PENDING.store(false, Ordering::SeqCst);
    std::mem::take(&mut *MOUSE_EVENTS.lock().unwrap_or_else(|e| e.into_inner()))
}

// ---------------------------------------------------------------------------
// Message window
// ---------------------------------------------------------------------------

/// Handlers the app installs for dispatcher messages.
pub struct Hooks {
    pub tick: fn(),
    pub mouse: fn(Vec<MouseEv>),
    pub message: fn(HWND, u32, WPARAM, LPARAM) -> Option<LRESULT>,
}

thread_local! {
    static HOOKS: RefCell<Option<Hooks>> = const { RefCell::new(None) };
}

unsafe extern "system" fn msg_proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_APP_RUN => {
                run_posted();
                LRESULT(0)
            }
            WM_TIMER if w.0 == 1 => {
                fire_timers();
                LRESULT(0)
            }
            WM_APP_TICK => {
                TICK_PENDING.store(false, Ordering::SeqCst);
                FRAME_COUNT.fetch_add(1, Ordering::Relaxed);
                let tick = HOOKS.with(|h| h.borrow().as_ref().map(|h| h.tick));
                if let Some(t) = tick {
                    t();
                }
                drain();
                LRESULT(0)
            }
            WM_APP_MOUSE => {
                let evs = take_mouse_events();
                let f = HOOKS.with(|h| h.borrow().as_ref().map(|h| h.mouse));
                if let Some(f) = f {
                    f(evs);
                }
                drain();
                LRESULT(0)
            }
            _ => {
                let f = HOOKS.with(|h| h.borrow().as_ref().map(|h| h.message));
                if let Some(f) = f {
                    if let Some(r) = f(hwnd, msg, w, l) {
                        drain();
                        return r;
                    }
                }
                DefWindowProcW(hwnd, msg, w, l)
            }
        }
    }
}

/// Creates the dispatcher window (a hidden top-level window, so it also gets
/// broadcasts like WM_DISPLAYCHANGE and TaskbarCreated) and the helper threads.
pub fn init(hooks: Hooks) -> HWND {
    init_with(hooks, true)
}

pub fn init_with(hooks: Hooks, threads: bool) -> HWND {
    HOOKS.with(|h| *h.borrow_mut() = Some(hooks));
    unsafe {
        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(msg_proc),
            hInstance: hinst.into(),
            lpszClassName: w!("ImageVault.Dispatcher"),
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            w!("ImageVault.Dispatcher"),
            w!("Image Vault"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(hinst.into()),
            None,
        )
        .expect("dispatcher window");
        MSG_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
        if threads {
        std::thread::Builder::new().name("vsync".into()).spawn(vsync_thread).ok();
        std::thread::Builder::new().name("mouse-hook".into()).spawn(hook_thread).ok();
        }
        hwnd
    }
}

pub fn run_loop() {
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
            drain();
        }
    }
}

pub fn quit() {
    unsafe { PostQuitMessage(0) };
}
