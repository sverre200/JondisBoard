//! Global hotkeys that work while a game has focus: a low-level keyboard hook (WH_KEYBOARD_LL)
//! on its own thread.
//!
//! Bindings match on scan code + numpad flag + modifiers, so Numpad 1 and the normal 1 key are
//! different hotkeys, exactly like the Python app's `hotkeys.py` (same `{scan, keypad, mods,
//! label}` format in config.json). Key autorepeat is ignored. "Record" mode captures the next key
//! press (Esc cancels) and swallows it, so it doesn't also reach the game.
//!
//! A low-level hook callback gets no user pointer, so the hook thread keeps its state in a
//! thread-local (the one piece of per-thread global state in the app; nothing else can reach it).

use crate::config::Binding;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyNameTextW;
use windows::Win32::UI::WindowsAndMessaging::*;

/// What a hotkey does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Clip(u32),
    StopAll,
}

/// Result of record mode: a binding, or None if Esc cancelled it.
pub type Recorded = Option<Binding>;

type Key = (u32, bool, Vec<String>);

pub struct Shared {
    actions: Mutex<HashMap<Key, Action>>,
    recording: AtomicBool,
    /// set while one of the app's own text fields has the keyboard: plain keys then type,
    /// they don't fire sounds
    pub typing: AtomicBool,
    on_action: Box<dyn Fn(Action) + Send + Sync>,
    on_record: Box<dyn Fn(Recorded) + Send + Sync>,
}

pub struct Hotkeys {
    shared: Arc<Shared>,
    thread_id: Arc<AtomicU32>,
    thread: Option<std::thread::JoinHandle<()>>,
}

// (scan code, virtual key, extended) of numpad keys, from the `keyboard` library's table
const KEYPAD: &[(u32, u32, bool)] = &[
    (126, 194, false), (28, 13, true), (53, 111, true), (55, 106, false), (69, 144, true),
    (71, 103, false), (71, 36, false), (72, 104, false), (72, 38, false), (73, 105, false),
    (73, 33, false), (74, 109, false), (75, 100, false), (75, 37, false), (76, 101, false),
    (76, 12, false), (77, 102, false), (77, 39, false), (78, 107, false), (79, 35, false),
    (79, 97, false), (80, 40, false), (80, 98, false), (81, 34, false), (81, 99, false),
    (82, 45, false), (82, 96, false), (83, 110, false), (83, 46, false),
];

pub fn is_keypad(scan: u32, vk: u32, extended: bool) -> bool {
    KEYPAD.contains(&(scan, vk, extended))
}

fn modifier_of(vk: u32) -> Option<&'static str> {
    match vk {
        0x10 | 0xA0 | 0xA1 => Some("shift"),
        0x11 | 0xA2 | 0xA3 => Some("ctrl"),
        0x12 | 0xA4 | 0xA5 => Some("alt"),
        0x5B | 0x5C => Some("windows"),
        _ => None,
    }
}

/// The key's name as the `keyboard` library spells it ("f5", "a", "num 1", "space").
pub fn key_label(scan: u32, vk: u32, extended: bool) -> String {
    let mut buf = [0u16; 64];
    let lp = ((scan as i32) << 16) | ((extended as i32) << 24);
    // SAFETY: buf is a valid, writable buffer of the given length.
    let n = unsafe { GetKeyNameTextW(lp, &mut buf) };
    let mut name = if n > 0 { String::from_utf16_lossy(&buf[..n as usize]).to_lowercase() } else { String::new() };
    if name.is_empty() {
        name = format!("key{scan}");
    }
    if is_keypad(scan, vk, extended) && !name.starts_with("num ") {
        name = format!("num {name}");
    }
    name
}

struct HookState {
    shared: Arc<Shared>,
    down: HashSet<u32>,
    mods: HashSet<&'static str>,
}

thread_local! {
    static HOOK: RefCell<Option<HookState>> = const { RefCell::new(None) };
}

impl Hotkeys {
    pub fn start(on_action: Box<dyn Fn(Action) + Send + Sync>, on_record: Box<dyn Fn(Recorded) + Send + Sync>) -> Hotkeys {
        let shared = Arc::new(Shared {
            actions: Mutex::new(HashMap::new()),
            recording: AtomicBool::new(false),
            typing: AtomicBool::new(false),
            on_action,
            on_record,
        });
        let thread_id = Arc::new(AtomicU32::new(0));
        let (s2, tid2) = (shared.clone(), thread_id.clone());
        let thread = std::thread::Builder::new()
            .name("hotkeys".into())
            .spawn(move || hook_thread(s2, tid2))
            .ok();
        Hotkeys { shared, thread_id, thread }
    }

    pub fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    pub fn set_bindings(&self, pairs: &[(Binding, Action)]) {
        let map = pairs.iter().map(|(b, a)| (b.key(), *a)).collect();
        *self.shared.actions.lock().unwrap() = map;
    }

    pub fn record(&self) {
        self.shared.recording.store(true, Ordering::Relaxed);
    }

    pub fn cancel_record(&self) {
        self.shared.recording.store(false, Ordering::Relaxed);
    }

    pub fn is_recording(&self) -> bool {
        self.shared.recording.load(Ordering::Relaxed)
    }
}

impl Drop for Hotkeys {
    fn drop(&mut self) {
        let tid = self.thread_id.load(Ordering::Acquire);
        if tid != 0 {
            // SAFETY: posting WM_QUIT to the hook thread's message queue.
            unsafe {
                let _ = PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn hook_thread(shared: Arc<Shared>, tid: Arc<AtomicU32>) {
    HOOK.with(|h| *h.borrow_mut() = Some(HookState { shared, down: HashSet::new(), mods: HashSet::new() }));
    // SAFETY: standard hook install + message loop on this thread; the hook is removed before
    // the thread ends.
    unsafe {
        tid.store(GetCurrentThreadId(), Ordering::Release);
        let hinst = GetModuleHandleW(None).ok().map(|h| windows::Win32::Foundation::HINSTANCE(h.0));
        let Ok(hook) = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), hinst, 0) else {
            crate::log::error("could not install the keyboard hook: hotkeys won't work");
            return;
        };
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let _ = UnhookWindowsHookEx(hook);
    }
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for HC_ACTION, lparam points to a KBDLLHOOKSTRUCT owned by the system.
        let k = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let msg = wparam.0 as u32;
        let down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let injected = k.flags.0 & LLKHF_INJECTED.0 != 0;
        let swallow = if injected { false } else { HOOK.with(|h| h.borrow_mut().as_mut().map(|s| on_key(s, k, down)).unwrap_or(false)) };
        if swallow {
            return LRESULT(1);
        }
    }
    // SAFETY: passing the event on, as required.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// One key event. Returns true to swallow it (only in record mode).
fn on_key(s: &mut HookState, k: &KBDLLHOOKSTRUCT, down: bool) -> bool {
    let vk = k.vkCode;
    let extended = k.flags.0 & LLKHF_EXTENDED.0 != 0;
    let scan = k.scanCode;
    if !down {
        s.down.remove(&scan);
        if let Some(m) = modifier_of(vk) {
            s.mods.remove(m);
        }
        return false;
    }
    if !s.down.insert(scan) {
        return s.shared.recording.load(Ordering::Relaxed); // autorepeat
    }
    if let Some(m) = modifier_of(vk) {
        s.mods.insert(m);
        return false;
    }
    let mods: Vec<String> = ["ctrl", "shift", "alt"].iter().filter(|m| s.mods.contains(*m)).map(|m| m.to_string()).collect();
    let keypad = is_keypad(scan, vk, extended);
    if s.shared.recording.swap(false, Ordering::Relaxed) {
        let label = key_label(scan, vk, extended);
        let b = if vk == 0x1B { None } else { Some(Binding { scan, keypad, mods, label }) };
        (s.shared.on_record)(b);
        return true;
    }
    if s.shared.typing.load(Ordering::Relaxed) && !mods.iter().any(|m| m == "ctrl" || m == "alt") {
        return false;
    }
    let action = s.shared.actions.lock().unwrap().get(&(scan, keypad, {
        let mut m = mods.clone();
        m.sort();
        m
    })).copied();
    if let Some(a) = action {
        (s.shared.on_action)(a);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keypad_table_tells_numpad_from_main_keys() {
        assert!(is_keypad(79, 97, false)); // Num 1
        assert!(is_keypad(28, 13, true)); // numpad Enter
        assert!(!is_keypad(28, 13, false)); // main Enter
        assert!(!is_keypad(2, 0x31, false)); // main 1
    }

    #[test]
    fn binding_keys_ignore_label_and_mod_order() {
        let a = Binding { scan: 63, keypad: false, mods: vec!["shift".into(), "ctrl".into()], label: "f5".into() };
        let b = Binding { scan: 63, keypad: false, mods: vec!["ctrl".into(), "shift".into()], label: "F5".into() };
        assert!(a.same(&b));
        let c = Binding { keypad: true, ..b.clone() };
        assert!(!a.same(&c));
    }
}
