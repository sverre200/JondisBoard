//! The one native window. Everything inside it is drawn by `view.rs`: no child windows, no popup
//! windows (menus, Settings, dialogs and toasts are drawn in the window, so nothing can ever
//! float over a game or take focus from it).
//!
//! The window's state is a `Box<State>` whose pointer lives in GWLP_USERDATA: that's the only
//! way a window procedure gets at its data. It's created in WM_NCCREATE and freed in WM_NCDESTROY;
//! all access is on the UI thread.

use super::app::{App, WM_START_AUDIO, WM_WAKE};
use super::gfx::{Canvas, Gfx};
use super::logo;
use crate::paths::Paths;
use crate::theme::Theme;
use std::sync::mpsc;
use std::time::Instant;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const TIMER_TOAST: usize = 1;
pub const TIMER_PLAYING: usize = 2;
pub const TIMER_CARET: usize = 3;
pub const TIMER_DRAG: usize = 4;
pub const TIMER_BENCH: usize = 5;
pub const TIMER_SLOW: usize = 6;
pub const TIMER_AUDIO_RETRY: usize = 7;

const CLASS: PCWSTR = w!("JondisBoardMain");
const WM_MOUSELEAVE: u32 = 0x02A3;
const MIN_W: i32 = 640;
const MIN_H: i32 = 480;

pub struct State {
    pub app: Option<App>,
    pub gfx: Gfx,
    pub canvas: Option<Canvas>,
    pub dpi: u32,
    pub shown: bool,
    pub bench: Option<super::bench::Bench>,
    pub tracking: bool,
}

pub fn invalidate(hwnd: HWND) {
    // SAFETY: invalidating our own window.
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

pub fn set_timer(hwnd: HWND, id: usize, ms: u32) {
    // SAFETY: a timer on our own window (re-setting an existing id just restarts it).
    unsafe {
        SetTimer(Some(hwnd), id, ms, None);
    }
}

pub fn kill_timer(hwnd: HWND, id: usize) {
    // SAFETY: as above.
    unsafe {
        let _ = KillTimer(Some(hwnd), id);
    }
}


pub fn is_foreground(hwnd: HWND) -> bool {
    // SAFETY: plain query.
    unsafe { GetForegroundWindow() == hwnd }
}

fn dwm_set<T>(hwnd: HWND, attr: DWMWINDOWATTRIBUTE, v: &T) {
    // SAFETY: v points to a value of the size given.
    unsafe {
        let _ = DwmSetWindowAttribute(hwnd, attr, v as *const T as *const _, std::mem::size_of::<T>() as u32);
    }
}

/// The title bar in the theme's colours, and the window/taskbar icon in the theme's logo
/// (Windows 11). Returns the new icons, which must stay alive while the window uses them.
pub fn apply_theme_chrome(hwnd: HWND, t: &Theme) -> Vec<logo::Icon> {
    dwm_set(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &1i32);
    dwm_set(hwnd, DWMWA_CAPTION_COLOR, &t.bg.colorref());
    dwm_set(hwnd, DWMWA_TEXT_COLOR, &t.text.colorref());
    dwm_set(hwnd, DWMWA_BORDER_COLOR, &t.border.colorref());
    // SAFETY: plain queries and WM_SETICON with icons we keep alive (returned to the caller).
    unsafe {
        let dpi = GetDpiForWindow(hwnd).max(96);
        let small = GetSystemMetricsForDpi(SM_CXSMICON, dpi).max(16) as u32;
        let big = GetSystemMetricsForDpi(SM_CXICON, dpi).max(32) as u32;
        let mut out = Vec::new();
        for (which, px) in [(ICON_SMALL, small), (ICON_BIG, big)] {
            if let Some(ic) = logo::icon(t, px) {
                SendMessageW(hwnd, WM_SETICON, Some(WPARAM(which as usize)), Some(LPARAM(ic.0 .0 as isize)));
                out.push(ic);
            }
        }
        out
    }
}

/// Parse "WxH+X+Y" (the Python app's geometry: client size in DIPs, outer position in pixels).
pub fn parse_geometry(s: &str) -> Option<(i32, i32, i32, i32)> {
    let (size, rest) = s.split_at(s.find(['+', '-'])?);
    let (w, h) = size.split_once('x')?;
    let mut nums = Vec::new();
    let mut cur = String::new();
    for ch in rest.chars() {
        if ch == '+' {
            if !cur.is_empty() {
                nums.push(cur.clone());
            }
            cur.clear();
        } else if ch == '-' && (cur.is_empty() || cur == "-") {
            cur.push('-');
        } else if ch == '-' {
            nums.push(cur.clone());
            cur = "-".into();
        } else {
            cur.push(ch);
        }
    }
    if !cur.is_empty() {
        nums.push(cur);
    }
    if nums.len() != 2 {
        return None;
    }
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?, nums[0].parse().ok()?, nums[1].parse().ok()?))
}

fn style() -> WINDOW_STYLE {
    WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN
}

/// Where the window opens: the saved spot if it's still on a monitor, else centred on the
/// primary one. Returns the outer rect in pixels.
fn initial_rect(geometry: Option<&str>) -> RECT {
    // SAFETY: monitor/DPI queries and AdjustWindowRectExForDpi on local structs.
    unsafe {
        let (cw, ch, x, y) = geometry.and_then(parse_geometry).unwrap_or((940, 680, i32::MIN, i32::MIN));
        let (cw, ch) = (cw.max(MIN_W), ch.max(MIN_H));
        let probe = POINT { x: if x == i32::MIN { 0 } else { x + 40 }, y: if y == i32::MIN { 0 } else { y + 20 } };
        let mon = MonitorFromPoint(probe, if x == i32::MIN { MONITOR_DEFAULTTOPRIMARY } else { MONITOR_DEFAULTTONULL });
        let (mon, x, y) = if mon.is_invalid() { (MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY), i32::MIN, i32::MIN) } else { (mon, x, y) };
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let s = dx as f32 / 96.0;
        let mut r = RECT { left: 0, top: 0, right: (cw as f32 * s).round() as i32, bottom: (ch as f32 * s).round() as i32 };
        let _ = AdjustWindowRectExForDpi(&mut r, style(), false, WINDOW_EX_STYLE(0), dx);
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let work = mi.rcWork;
        let (w, h) = (w.min(work.right - work.left + 16), h.min(work.bottom - work.top + 8));
        let (x, y) = if x == i32::MIN {
            (work.left + (work.right - work.left - w) / 2, work.top + (work.bottom - work.top - h) / 2)
        } else {
            (x, y)
        };
        RECT { left: x, top: y, right: x + w, bottom: y + h }
    }
}

/// "WxH+X+Y" for the window's normal (not minimised/maximised) spot.
fn current_geometry(hwnd: HWND) -> Option<String> {
    // SAFETY: queries on our own window.
    unsafe {
        let mut wp = WINDOWPLACEMENT { length: std::mem::size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
        GetWindowPlacement(hwnd, &mut wp).ok()?;
        let r = wp.rcNormalPosition;
        let dpi = GetDpiForWindow(hwnd).max(96);
        let mut nc = RECT::default();
        let _ = AdjustWindowRectExForDpi(&mut nc, style(), false, WINDOW_EX_STYLE(0), dpi);
        let s = dpi as f32 / 96.0;
        let cw = ((r.right - r.left) - (nc.right - nc.left)) as f32 / s;
        let ch = ((r.bottom - r.top) - (nc.bottom - nc.top)) as f32 / s;
        // rcNormalPosition is in workspace coordinates; with a top/left taskbar that differs from
        // screen coordinates by the work area's offset
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let (ox, oy) = (mi.rcWork.left - mi.rcMonitor.left, mi.rcWork.top - mi.rcMonitor.top);
        Some(format!("{}x{}+{}+{}", cw.round() as i32, ch.round() as i32, r.left + ox, r.top + oy))
    }
}

/// Create the window, run the app, return when it closes.
pub fn run(paths: Paths, started: Instant, bench: Option<super::bench::Bench>) -> windows::core::Result<()> {
    create(paths, started, bench)?;
    // SAFETY: the standard message loop.
    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

/// Tests: handle pending messages for up to `ms`.
pub fn pump(ms: u64) {
    let end = Instant::now() + std::time::Duration::from_millis(ms);
    // SAFETY: a non-blocking message loop on the UI thread.
    unsafe {
        let mut msg = MSG::default();
        loop {
            while Instant::now() < end && PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if Instant::now() >= end {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
}

/// Tests: run f with the window's app and graphics.
pub fn with_app<R>(hwnd: HWND, f: impl FnOnce(&mut App, &mut Gfx) -> R) -> Option<R> {
    let win = win_of(hwnd)?;
    let r = {
        let mut st = win.state.try_borrow_mut().ok()?;
        let st = &mut *st;
        let app = st.app.as_mut()?;
        f(app, &mut st.gfx)
    };
    after(hwnd, win);
    Some(r)
}

/// Tests: draw the current state into a PNG (offscreen: nothing appears on screen).
pub fn snapshot(hwnd: HWND, path: &std::path::Path) -> Result<(), String> {
    snapshot_at(hwnd, path, None)
}

/// Tests: like `snapshot`, drawn as if the screen were at `dpi` (e.g. 144 = 150 %).
pub fn snapshot_at(hwnd: HWND, path: &std::path::Path, dpi: Option<u32>) -> Result<(), String> {
    let win = win_of(hwnd).ok_or("no window")?;
    let mut st = win.state.try_borrow_mut().map_err(|_| "busy")?;
    let st = &mut *st;
    let dpi = dpi.unwrap_or(st.dpi);
    let mut rc = RECT::default();
    // SAFETY: plain query on our window.
    unsafe {
        let _ = GetClientRect(hwnd, &mut rc);
    }
    let k = dpi as f32 / st.dpi as f32;
    let (w, h) = (((rc.right - rc.left) as f32 * k) as u32, ((rc.bottom - rc.top) as f32 * k) as u32);
    let mut c = Canvas::offscreen(&st.gfx, w.max(1), h.max(1), dpi).map_err(|e| e.message())?;
    let app = st.app.as_mut().ok_or("no app")?;
    // the pictures it has decoded so far
    app.pic_failed.clear();
    app.pic_pending.clear();
    app.paint(&mut c, &mut st.gfx);
    c.save_png(path)
}

/// Tests: close the window (saves and stops everything, like the close button).
pub fn close(hwnd: HWND) {
    // SAFETY: sending WM_CLOSE to our own window.
    unsafe {
        SendMessageW(hwnd, WM_CLOSE, None, None);
    }
}

/// The window's data: the state behind a RefCell, because Windows calls the window procedure
/// re-entrantly (releasing mouse capture, a modal file picker, WM_SETICON...). A nested call that
/// finds the state already borrowed doesn't touch it: it defers what it can (`busy`).
struct Win {
    state: std::cell::RefCell<State>,
    missed_paint: std::cell::Cell<bool>,
}

/// Create the window and its app (shown, but cloaked until the first frame is drawn).
pub fn create(paths: Paths, started: Instant, bench: Option<super::bench::Bench>) -> windows::core::Result<HWND> {
    // SAFETY: standard window class registration and creation. The Win box is handed to the
    // window in CreateWindowExW and reclaimed in WM_NCDESTROY.
    unsafe {
        let hinst: HINSTANCE = GetModuleHandleW(None)?.into();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hIcon: LoadIconW(Some(hinst), PCWSTR(1 as _)).unwrap_or_default(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let gfx = Gfx::new()?;
        let state = State { app: None, gfx, canvas: None, dpi: 96, shown: false, bench, tracking: false };
        let win = Box::new(Win { state: std::cell::RefCell::new(state), missed_paint: std::cell::Cell::new(false) });
        // the saved spot comes from the config, read early just for this
        let cfg = crate::config::ConfigFile::new(paths.config()).load().0;
        let r = initial_rect(cfg.geometry.as_deref());
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            w!("JondisBoard"),
            style(),
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            None,
            None,
            Some(hinst),
            Some(Box::into_raw(win) as *const _),
        )?;
        let win = win_of(hwnd).expect("window state");
        // hidden from view (cloaked) until the first frame is drawn: it appears in one step
        dwm_set(hwnd, DWMWA_CLOAK, &1i32);
        let (tx, rx) = mpsc::channel();
        let mut app = App::new(hwnd, paths, tx, rx, started);
        app.icons = apply_theme_chrome(hwnd, app.themes.get());
        app.mark("window created");
        {
            let mut st = win.state.borrow_mut();
            st.dpi = GetDpiForWindow(hwnd).max(96);
            if st.bench.is_some() {
                app.record_frames = true;
            }
            st.app = Some(app);
        }
        // tests: shown (so it paints) but cloaked for good: it never appears on screen
        let _ = ShowWindow(hwnd, if super::app::test_mode() { SW_SHOWNOACTIVATE } else { SW_SHOWNORMAL });
        let _ = UpdateWindow(hwnd);
        Ok(hwnd)
    }
}

fn win_of(hwnd: HWND) -> Option<&'static Win> {
    // SAFETY: GWLP_USERDATA holds the Win box pointer from WM_NCCREATE until WM_NCDESTROY, and is
    // only used on this (the window's) thread; mutation goes through its RefCell.
    unsafe {
        let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Win;
        p.as_ref()
    }
}

fn lparam_xy(l: LPARAM, scale: f32) -> (f32, f32) {
    let x = (l.0 & 0xffff) as i16 as f32;
    let y = ((l.0 >> 16) & 0xffff) as i16 as f32;
    (x / scale, y / scale)
}

fn key_down(vk: VIRTUAL_KEY) -> bool {
    // SAFETY: plain query.
    unsafe { GetKeyState(vk.0 as i32) < 0 }
}

fn paint(hwnd: HWND, st: &mut State) {
    let Some(app) = st.app.as_mut() else { return };
    if st.canvas.is_none() {
        match Canvas::new(&st.gfx, hwnd, st.dpi) {
            Ok(c) => st.canvas = Some(c),
            Err(e) => {
                crate::log::error(&format!("could not create the render target: {e}"));
                return;
            }
        }
    }
    let c = st.canvas.as_mut().unwrap();
    app.paint(c, &mut st.gfx);
    // covered by a fullscreen game (or minimised): Windows doesn't show our frames, so don't
    // keep drawing them at full rate; check back 4 times a second
    // SAFETY: plain query on a valid render target (it reflects the last present).
    let occluded = !super::app::test_mode() && c.occluded();
    if occluded && (app.dirty || app.slow_anim) {
        app.dirty = false;
        app.slow_anim = false;
        set_timer(hwnd, TIMER_SLOW, 250);
    }
    if c.gen == u64::MAX {
        st.canvas = None; // device lost: recreate on the next paint
        invalidate(hwnd);
    }
    if !st.shown {
        st.shown = true;
        if !super::app::test_mode() {
            dwm_set(hwnd, DWMWA_CLOAK, &0i32);
        }
        app.mark("first frame");
        app.ready = true;
        // audio after the window is up (a slow driver never delays the first frame)
        // SAFETY: posting to our own window.
        unsafe {
            let _ = PostMessageW(Some(hwnd), WM_START_AUDIO, WPARAM(0), LPARAM(0));
        }
    }
    if app.dirty {
        invalidate(hwnd);
    } else if app.slow_anim {
        set_timer(hwnd, TIMER_SLOW, 33);
    } else {
        kill_timer(hwnd, TIMER_SLOW);
    }
}

/// After a message: things that must happen with the state released (they can call back into
/// the window procedure).
fn after(hwnd: HWND, win: &Win) {
    let (capture, resizes) = match win.state.try_borrow_mut() {
        Ok(mut st) => {
            let cap = st.app.as_mut().and_then(|a| a.capture_req.take());
            let rs = st.bench.as_mut().map(|b| std::mem::take(&mut b.resize_jobs)).unwrap_or_default();
            (cap, rs)
        }
        Err(_) => return,
    };
    // (tests never capture the mouse: that would grab the real one)
    if let Some(on) = capture.filter(|_| !super::app::test_mode()) {
        // SAFETY: mouse capture for our own window.
        unsafe {
            if on {
                SetCapture(hwnd);
            } else {
                let _ = ReleaseCapture();
            }
        }
    }
    if !resizes.is_empty() {
        let mut times = Vec::new();
        for (w, h) in resizes {
            let t = Instant::now();
            // SAFETY: resizing our own window; WM_SIZE paints synchronously.
            unsafe {
                let _ = SetWindowPos(hwnd, None, 0, 0, w, h, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
            }
            times.push(t.elapsed().as_secs_f64() * 1000.0);
        }
        if let Ok(mut st) = win.state.try_borrow_mut() {
            if let Some(b) = st.bench.as_mut() {
                b.resize_times.extend(times);
            }
        }
    }
    if win.missed_paint.take() {
        invalidate(hwnd);
    }
}

/// A nested call while the state is in use: paint later, re-queue what can be re-queued.
fn busy(hwnd: HWND, win: &Win, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // SAFETY: plain Win32 calls on our own window.
    unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                BeginPaint(hwnd, &mut ps);
                let _ = EndPaint(hwnd, &ps);
                win.missed_paint.set(true);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_SIZE | WM_WAKE | WM_START_AUDIO => {
                let _ = PostMessageW(Some(hwnd), msg, wp, lp);
                LRESULT(0)
            }
            WM_DPICHANGED => {
                let r = &*(lp.0 as *const RECT);
                let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                let _ = PostMessageW(Some(hwnd), WM_DPI_LATER, wp, LPARAM(0));
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

const WM_DPI_LATER: u32 = WM_APP + 9;

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        // SAFETY: for WM_NCCREATE lp is a CREATESTRUCTW whose lpCreateParams is our Win box.
        unsafe {
            let cs = &*(lp.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
    }
    if msg == WM_NCDESTROY {
        // SAFETY: reclaim the box created in create(); nothing uses the pointer afterwards (the
        // state is never borrowed here: WM_CLOSE releases it before DestroyWindow).
        unsafe {
            let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Win;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            if !p.is_null() {
                drop(Box::from_raw(p));
            }
            if !super::app::test_mode() {
                PostQuitMessage(0);
            }
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
    }
    let Some(win) = win_of(hwnd) else {
        // SAFETY: default handling.
        return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
    };
    let Ok(mut guard) = win.state.try_borrow_mut() else {
        return busy(hwnd, win, msg, wp, lp);
    };
    let (result, destroy) = handle(hwnd, &mut guard, msg, wp, lp);
    drop(guard);
    if destroy {
        // SAFETY: destroying our own window, with the state released.
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        return LRESULT(0);
    }
    after(hwnd, win);
    match result {
        Some(r) => r,
        // SAFETY: default handling.
        None => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// One message with the state borrowed. Returns (result, or None for default handling; destroy).
fn handle(hwnd: HWND, st: &mut State, msg: u32, wp: WPARAM, lp: LPARAM) -> (Option<LRESULT>, bool) {
    let scale = st.dpi as f32 / 96.0;
    let done = |r: isize| (Some(LRESULT(r)), false);
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            // SAFETY: Begin/EndPaint pair on our window validates the update region.
            unsafe {
                BeginPaint(hwnd, &mut ps);
                let _ = EndPaint(hwnd, &ps);
            }
            paint(hwnd, st);
            done(0)
        }
        WM_ERASEBKGND => done(1),
        WM_SIZE => {
            let (w, h) = ((lp.0 & 0xffff) as u32, ((lp.0 >> 16) & 0xffff) as u32);
            if let Some(c) = st.canvas.as_mut() {
                c.resize(w, h);
            }
            if let Some(app) = st.app.as_mut() {
                if !app.menus.is_empty() {
                    app.close_menus();
                }
                app.invalidate();
            }
            // draw straight away while resizing (no stretched old frame)
            if st.shown && w > 0 && h > 0 {
                paint(hwnd, st);
            }
            done(0)
        }
        WM_MOVE => {
            if let Some(app) = st.app.as_mut() {
                if !app.menus.is_empty() {
                    app.close_menus();
                }
            }
            (None, false)
        }
        WM_GETMINMAXINFO => {
            // SAFETY: lp is a MINMAXINFO for this message.
            unsafe {
                let mm = &mut *(lp.0 as *mut MINMAXINFO);
                let dpi = GetDpiForWindow(hwnd).max(96);
                let mut r = RECT { left: 0, top: 0, right: (MIN_W as f32 * dpi as f32 / 96.0) as i32, bottom: (MIN_H as f32 * dpi as f32 / 96.0) as i32 };
                let _ = AdjustWindowRectExForDpi(&mut r, style(), false, WINDOW_EX_STYLE(0), dpi);
                mm.ptMinTrackSize = POINT { x: r.right - r.left, y: r.bottom - r.top };
            }
            done(0)
        }
        WM_DPICHANGED | WM_DPI_LATER => {
            let dpi = (wp.0 & 0xffff) as u32;
            st.dpi = dpi;
            if let Some(c) = st.canvas.as_mut() {
                c.set_dpi(dpi);
            }
            if msg == WM_DPICHANGED {
                // SAFETY: lp is the suggested RECT for this message. (The WM_SIZE this causes
                // arrives while the state is borrowed and is re-queued.)
                unsafe {
                    let r = &*(lp.0 as *const RECT);
                    let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                }
            }
            if let Some(app) = st.app.as_mut() {
                app.icons = apply_theme_chrome(hwnd, app.themes.get());
                app.pic_failed.clear(); // pictures are decoded at the new size
                app.invalidate();
            }
            done(0)
        }
        WM_MOUSEMOVE => {
            if !st.tracking {
                st.tracking = true;
                let mut t = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                // SAFETY: valid struct for our window.
                unsafe {
                    let _ = TrackMouseEvent(&mut t);
                }
            }
            let (x, y) = lparam_xy(lp, scale);
            if let Some(app) = st.app.as_mut() {
                app.mouse_move(&mut st.gfx, x, y);
            }
            done(0)
        }
        WM_MOUSELEAVE => {
            st.tracking = false;
            if let Some(app) = st.app.as_mut() {
                app.mouse_leave();
            }
            done(0)
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            let (x, y) = lparam_xy(lp, scale);
            if let Some(app) = st.app.as_mut() {
                app.mouse_down(&mut st.gfx, x, y, key_down(VK_SHIFT), msg == WM_LBUTTONDBLCLK);
            }
            done(0)
        }
        WM_LBUTTONUP => {
            let (x, y) = lparam_xy(lp, scale);
            if let Some(app) = st.app.as_mut() {
                app.mouse_up(x, y);
            }
            done(0)
        }
        WM_RBUTTONUP => {
            let (x, y) = lparam_xy(lp, scale);
            if let Some(app) = st.app.as_mut() {
                app.right_click(x, y);
            }
            done(0)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wp.0 >> 16) & 0xffff) as i16 as f32;
            if let Some(app) = st.app.as_mut() {
                app.wheel(delta);
            }
            done(0)
        }
        WM_CAPTURECHANGED => {
            // someone else took the mouse mid-press (our own release comes after mouse_up has
            // already finished the press, so this changes nothing then)
            if let Some(app) = st.app.as_mut() {
                if app.drag.is_some() && lp.0 != hwnd.0 as isize {
                    app.cancel_drag();
                }
                app.slider_drag = None;
                app.scrollbar_drag = None;
            }
            (None, false)
        }
        WM_SETCURSOR => {
            if (lp.0 & 0xffff) as u32 == HTCLIENT {
                let text = st.app.as_ref().map(|a| a.over_text()).unwrap_or(false);
                // SAFETY: standard cursors.
                unsafe {
                    SetCursor(LoadCursorW(None, if text { IDC_IBEAM } else { IDC_ARROW }).ok());
                }
                return done(1);
            }
            (None, false)
        }
        WM_KEYDOWN => {
            if let Some(app) = st.app.as_mut() {
                if app.key_down(wp.0 as u32, key_down(VK_CONTROL), key_down(VK_SHIFT)) {
                    return done(0);
                }
            }
            (None, false)
        }
        WM_CHAR => {
            if let Some(app) = st.app.as_mut() {
                if let Some(ch) = char::from_u32(wp.0 as u32) {
                    if !ch.is_control() {
                        app.char_typed(ch);
                    }
                }
            }
            done(0)
        }
        WM_ACTIVATEAPP => {
            if let Some(app) = st.app.as_mut() {
                if wp.0 == 0 {
                    app.deactivated();
                } else {
                    app.update_typing();
                    // sounds added, removed or edited in Explorer meanwhile show up now
                    app.refresh_if_changed();
                }
            }
            (None, false)
        }
        WM_TIMER => {
            if let Some(app) = st.app.as_mut() {
                match wp.0 {
                    TIMER_TOAST => {
                        kill_timer(hwnd, TIMER_TOAST);
                        app.close_toast();
                    }
                    TIMER_PLAYING => {
                        if !app.poll_progress() {
                            kill_timer(hwnd, TIMER_PLAYING);
                        }
                    }
                    TIMER_CARET => {
                        let typing = app.search_focused || app.dialog.as_ref().map(|d| !d.fields.is_empty()).unwrap_or(false);
                        if typing {
                            app.invalidate();
                        } else {
                            kill_timer(hwnd, TIMER_CARET);
                        }
                    }
                    TIMER_DRAG => {
                        if !app.drag_tick() {
                            kill_timer(hwnd, TIMER_DRAG);
                        }
                    }
                    TIMER_BENCH => {
                        if let (Some(b), Some(c)) = (st.bench.as_mut(), st.canvas.as_mut()) {
                            b.tick(hwnd, app, c, &mut st.gfx);
                        }
                    }
                    TIMER_SLOW => {
                        kill_timer(hwnd, TIMER_SLOW);
                        app.invalidate();
                    }
                    TIMER_AUDIO_RETRY => {
                        kill_timer(hwnd, TIMER_AUDIO_RETRY);
                        app.probe_audio();
                    }
                    _ => {}
                }
            }
            done(0)
        }
        WM_WAKE => {
            if let Some(app) = st.app.as_mut() {
                app.drain();
                if app.player.is_playing_any() {
                    set_timer(hwnd, TIMER_PLAYING, 33);
                }
            }
            done(0)
        }
        WM_START_AUDIO => {
            if let Some(app) = st.app.as_mut() {
                app.start_audio();
            }
            if let Some(b) = st.bench.as_mut() {
                b.start(hwnd);
            }
            done(0)
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED => {
            if let Some(app) = st.app.as_mut() {
                app.icons = apply_theme_chrome(hwnd, app.themes.get());
            }
            (None, false)
        }
        WM_CLOSE => {
            if let Some(mut app) = st.app.take() {
                let g = current_geometry(hwnd);
                // SAFETY: plain query.
                let zoomed = unsafe { IsZoomed(hwnd).as_bool() };
                app.shutdown(g, zoomed);
            }
            (Some(LRESULT(0)), true)
        }
        _ => (None, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_python_geometry() {
        assert_eq!(parse_geometry("1918x1000+-7+0"), Some((1918, 1000, -7, 0)));
        assert_eq!(parse_geometry("940x680+100+50"), Some((940, 680, 100, 50)));
        assert_eq!(parse_geometry("940x680-5-10"), Some((940, 680, -5, -10)));
        assert_eq!(parse_geometry("garbage"), None);
    }
}
