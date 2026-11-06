//! The app: its state and everything it does. Drawing is in `view.rs`, mouse/keyboard handling
//! in `input.rs`, the Win32 window in `window.rs`.
//!
//! One `App` lives on the UI thread (owned by the window). Background threads (audio, hotkeys,
//! imports, picture decoding) never touch it: they send a `Msg` and wake the window.

use super::anim::{self, Anim};
use super::gfx::Rect;
use super::shell;
use super::textfield::TextField;
use crate::audio::engine::{Event, Player};
use crate::audio::wasapi::{self, Kind};
use crate::config::{Binding, Config, ConfigFile, ALL, NO_GROUP};
use crate::hotkeys::{Action as HkAction, Hotkeys, Recorded};
use crate::importer;
use crate::library::{self, Library};
use crate::paths::Paths;
use crate::theme::{self, Rgb, ThemeManager, DANGER, GOOD, WARN};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_WAKE: u32 = WM_APP + 1;
pub const WM_START_AUDIO: u32 = WM_APP + 2;

/// A window handle that background threads may post to.
#[derive(Clone, Copy)]
pub struct Waker(pub isize);

impl Waker {
    pub fn wake(&self) {
        // SAFETY: PostMessage is safe to call from any thread with any window handle; a stale
        // handle just fails.
        unsafe {
            let _ = PostMessageW(Some(HWND(self.0 as *mut _)), WM_WAKE, WPARAM(0), LPARAM(0));
        }
    }
}

/// Messages from background threads.
pub enum Msg {
    Engine(Event),
    Hotkey(HkAction),
    Recorded(Recorded),
    JobStatus(u64, &'static str),
    JobDone(u64, Result<PathBuf, String>),
    DialogImport(Result<PathBuf, String>),
    DialogImportStatus(&'static str),
    Picture(super::images::Decoded),
    AudioStarted(Result<Vec<String>, String>, Devices, [Option<String>; 3]),
    /// after a device was lost: whether every configured device is present again
    AudioProbe(bool),
}

#[derive(Clone, Debug, Default)]
pub struct Devices {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub cable_installed: bool,
}

#[derive(Clone, Debug)]
pub struct Clip {
    pub name: String,
    pub id: u32,
    pub path: PathBuf,
    pub stem: String,
    pub group: String,
    pub duration: Option<f64>,
    pub size: u64,
    pub mtime: Option<std::time::SystemTime>,
    /// the picture file and its modification stamp (changes when it's replaced)
    pub picture: Option<(PathBuf, u64)>,
    pub failed: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Good,
    Warn,
    Bad,
    Info,
}

impl Level {
    pub fn color(self, t: &theme::Theme) -> Rgb {
        match self {
            Level::Good => GOOD,
            Level::Warn => WARN,
            Level::Bad => DANGER,
            Level::Info => t.accent,
        }
    }
}

pub struct Toast {
    pub text: String,
    pub level: Level,
    pub born: Instant,
    pub closing: Option<Instant>,
}

/// One Quick import in progress (or failed).
pub struct Job {
    pub id: u64,
    pub url: String,
    pub source: &'static str,
    pub status: &'static str,
    pub error: Option<String>,
    pub full_error: Option<String>,
    pub started: Instant,
}

/// Everything a menu item can do.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Play(u32),
    /// loop it (true) or let it end (false)
    Loop(u32, bool),
    SetHotkey(u32),
    ClearHotkey(u32),
    MoveTo(u32, String),
    NewGroupWith(u32),
    ChoosePicture(u32),
    PastePicture(u32),
    RemovePicture(u32),
    Rename(u32),
    Delete(u32),
    RenameGroup(String),
    DeleteGroup(String),
    GroupColor(String, Rgb),
    OpenFolder,
    Reload,
    SortAz,
    StopHotkey,
    Theme(String),
    Device(DeviceSlot, String),
    ImportGroup(String),
    /// the import dialog's "New group…": type a name in place of the dropdown
    ImportNewGroup,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DeviceSlot {
    Mic,
    Cable,
    Monitor,
}

#[derive(Clone, Debug)]
pub enum MenuItem {
    Header(String),
    Item { text: String, action: Action, danger: bool, dot: Option<Rgb>, enabled: bool, checked: bool, instant: bool },
    Submenu { text: String, items: Vec<MenuItem> },
    Separator,
    Label(String),
    Swatches { group: String, current: Option<Rgb> },
}

impl MenuItem {
    pub fn item(text: impl Into<String>, action: Action) -> MenuItem {
        MenuItem::Item { text: text.into(), action, danger: false, dot: None, enabled: true, checked: false, instant: false }
    }
    pub fn danger(text: impl Into<String>, action: Action) -> MenuItem {
        MenuItem::Item { text: text.into(), action, danger: true, dot: None, enabled: true, checked: false, instant: false }
    }
}

/// Who opened a menu (clicking that button again closes it).
#[derive(Clone, Debug, PartialEq)]
pub enum MenuOwner {
    Context,
    Theme,
    More,
    Select(DeviceSlot),
    ImportGroup,
    Tab(String),
}

pub struct Menu {
    pub items: Vec<MenuItem>,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub born: Instant,
    pub owner: MenuOwner,
    /// for submenus: the parent's row index
    pub parent_row: Option<usize>,
    /// glide direction while it appears
    pub dx: f32,
    pub dy: f32,
    /// rows' rects (from the last frame)
    pub rows: Vec<Rect>,
    pub rect: Rect,
    pub hover: Option<usize>,
}

pub struct ClosingMenu {
    pub menu: Menu,
    pub since: Instant,
    pub alpha: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TextAction {
    NewGroup(Option<u32>),
    RenameGroup(String),
    RenameClip(u32),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConfirmAction {
    DeleteClip(u32),
    DeleteGroup(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum HotkeyTarget {
    Clip(u32),
    StopAll,
}

pub enum DialogKind {
    Confirm { title: String, text: String, ok: String, action: ConfirmAction },
    Text { title: String, ok: String, action: TextAction },
    Hotkey { what: String, target: HotkeyTarget },
    /// `new_group`: the Group control is a text field (fields[4]) for a group made on import
    Import { normalize: bool, group: String, new_group: bool, busy: bool, started: Option<Instant> },
}

pub struct Dialog {
    pub kind: DialogKind,
    pub fields: Vec<TextField>,
    pub focus: usize,
    pub born: Instant,
    pub closing: Option<Instant>,
    pub message: Option<(String, Rgb)>,
    pub width: f32,
    pub rect: Rect,
}

pub struct Drag {
    pub id: u32,
    pub grab: (f32, f32),
    pub pos: (f32, f32),
    pub born: Instant,
    /// index in the visible list it would go to (live, while over the board)
    pub slot: Option<usize>,
    /// group tab it's over
    pub tab: Option<String>,
    pub origin: usize,
}

/// A lifted copy fading out after a drop.
pub struct Landing {
    pub id: u32,
    pub from: (f32, f32),
    pub to: (f32, f32),
    pub since: Instant,
}

pub struct App {
    pub hwnd: HWND,
    pub waker: Waker,
    pub paths: Paths,
    pub cfg: Config,
    cfg_file: ConfigFile,
    pub themes: ThemeManager,
    pub lib: Library,
    pub clips: Vec<Clip>,
    pub by_id: HashMap<u32, usize>,
    pub groups: Vec<String>,
    next_id: u32,
    moved: HashMap<String, String>,
    pub player: Arc<Player>,
    pub hotkeys: Option<Hotkeys>,
    pub tx: Sender<Msg>,
    pub rx: Receiver<Msg>,
    pub anim: Anim,
    // view state
    pub search: TextField,
    pub search_focused: bool,
    pub query: String,
    pub scroll: HashMap<String, f32>,
    pub scroll_target: HashMap<String, f32>,
    pub menus: Vec<Menu>,
    pub closing_menus: Vec<ClosingMenu>,
    pub settings_open: bool,
    pub settings_changed: Instant,
    pub dialog: Option<Dialog>,
    pub closing_dialog: Option<Dialog>,
    pub toast: Option<Toast>,
    pub old_toasts: Vec<Toast>,
    pub drag: Option<Drag>,
    pub landing: Option<Landing>,
    pub jobs: Vec<Job>,
    /// how far the sidebar's group list is scrolled
    pub side_scroll: f32,
    /// the view the sidebar last scrolled into sight (it follows the open group)
    pub side_follow: Option<String>,
    next_job: u64,
    /// the group the import dialog's running import goes into ("" = No group)
    pub import_target: Option<String>,
    pub status: (String, Level),
    pub devices: Devices,
    pub audio_starting: bool,
    /// a device stopped; the app reconnects by itself once it's back
    pub audio_lost: bool,
    /// the running start_audio is a quiet reconnect attempt
    audio_retry: bool,
    pub progress: Vec<(u32, f32)>,
    pub playing: HashMap<u32, f32>,
    /// clips set to loop (shown on the tiles that are playing)
    pub looping: std::collections::HashSet<u32>,
    pub flash: HashMap<u32, (Instant, f32)>,
    pub slide_until: Instant,
    pub caret_since: Instant,
    // pictures
    pub pic_tx: Option<Sender<super::images::Request>>,
    pub pic_pending: HashSet<u64>,
    pub pic_ready: HashMap<u64, (u32, Vec<u8>)>,
    pub pic_failed: HashSet<u64>,
    // input
    pub mouse: (f32, f32),
    pub mouse_in: bool,
    pub down: Option<super::view::Hit>,
    pub down_pos: (f32, f32),
    pub down_menus_open: bool,
    /// the press was the second click of a double-click
    pub down_double: bool,
    pub slider_drag: Option<u8>,
    pub scrollbar_drag: Option<(f32, f32)>,
    pub hits: Vec<(Rect, super::view::Hit)>,
    pub hover: Option<super::view::Hit>,
    pub layout: super::view::Layout,
    pub ready: bool,
    pub started: Instant,
    pub timings: Vec<(&'static str, f64)>,
    pub frame_times: Vec<f32>,
    pub record_frames: bool,
    pub quitting: bool,
    pub dirty: bool,
    /// something on screen wants frames at ~30 fps (see Anim::keep_slow)
    pub slow_anim: bool,
    /// the window/taskbar icons in use (kept alive while the window shows them)
    pub icons: Vec<super::logo::Icon>,
    /// clips started (click or hotkey), newest last: for tests and diagnostics
    pub played: Vec<u32>,
    /// mouse capture to take (true) or release (false) once the current message is handled
    pub capture_req: Option<bool>,
}

pub fn clip_id_key(id: u32) -> u64 {
    id as u64
}

impl App {
    pub fn new(hwnd: HWND, paths: Paths, tx: Sender<Msg>, rx: Receiver<Msg>, started: Instant) -> App {
        let cfg_file = ConfigFile::new(paths.config());
        let (cfg, err) = cfg_file.load();
        if let Some(e) = err {
            crate::log::error(&e);
        }
        let waker = Waker(hwnd.0 as isize);
        let notify_tx = tx.clone();
        let notify: crate::audio::engine::Notify = Arc::new(move |e| {
            let _ = notify_tx.send(Msg::Engine(e));
            waker.wake();
        });
        let budget = cfg_budget(&cfg);
        let player = Player::new(paths.ffmpeg(), budget, notify);
        player.solo.store(cfg.solo, std::sync::atomic::Ordering::Relaxed);
        player.toggle_stop.store(cfg.toggle_stop, std::sync::atomic::Ordering::Relaxed);
        player.levels.mic.set(cfg.mic_volume as f32);
        player.levels.clips.set(cfg.clip_volume as f32);
        player.levels.monitor.set(cfg.monitor_volume as f32);
        player.levels.passthrough.store(cfg.mic_passthrough, std::sync::atomic::Ordering::Relaxed);
        let themes = ThemeManager::new(&cfg.theme);
        let lib = Library::new(paths.sounds());
        let now = Instant::now();
        let mut app = App {
            hwnd,
            waker,
            paths,
            cfg,
            cfg_file,
            themes,
            lib,
            clips: Vec::new(),
            by_id: HashMap::new(),
            groups: Vec::new(),
            next_id: 1,
            moved: HashMap::new(),
            player,
            hotkeys: None,
            tx,
            rx,
            anim: Anim::new(),
            search: TextField::new("Search sounds   Ctrl+F"),
            search_focused: false,
            query: String::new(),
            scroll: HashMap::new(),
            scroll_target: HashMap::new(),
            menus: Vec::new(),
            closing_menus: Vec::new(),
            settings_open: false,
            settings_changed: now - Duration::from_secs(5),
            dialog: None,
            closing_dialog: None,
            toast: None,
            old_toasts: Vec::new(),
            drag: None,
            landing: None,
            jobs: Vec::new(),
            side_scroll: 0.0,
            side_follow: None,
            import_target: None,
            next_job: 1,
            status: ("Starting audio…".into(), Level::Info),
            devices: Devices::default(),
            audio_starting: true,
            audio_lost: false,
            audio_retry: false,
            progress: Vec::new(),
            playing: HashMap::new(),
            looping: std::collections::HashSet::new(),
            flash: HashMap::new(),
            slide_until: now,
            caret_since: now,
            pic_tx: None,
            pic_pending: HashSet::new(),
            pic_ready: HashMap::new(),
            pic_failed: HashSet::new(),
            mouse: (-1.0, -1.0),
            mouse_in: false,
            down: None,
            down_pos: (0.0, 0.0),
            down_menus_open: false,
            down_double: false,
            slider_drag: None,
            scrollbar_drag: None,
            hits: Vec::new(),
            hover: None,
            layout: Default::default(),
            ready: false,
            started,
            timings: Vec::new(),
            frame_times: Vec::new(),
            record_frames: false,
            quitting: false,
            dirty: true,
            slow_anim: false,
            icons: Vec::new(),
            played: Vec::new(),
            capture_req: None,
        };
        app.reload();
        app.mark("library scanned");
        let (tx3, w3) = (app.tx.clone(), waker);
        let p = app.player.clone();
        // tests don't hook the real keyboard
        app.hotkeys = (!test_mode()).then(|| Hotkeys::start(
            // straight from the hook thread: a hotkey in a game plays even if the window is busy
            // (the tile flash comes back through the engine's Played event)
            Box::new(move |a| match a {
                HkAction::Clip(id) => p.press(id),
                HkAction::StopAll => p.stop_all(),
            }),
            Box::new(move |r| {
                let _ = tx3.send(Msg::Recorded(r));
                w3.wake();
            }),
        ));
        app.apply_hotkeys();
        let (ptx, pw) = (app.tx.clone(), waker);
        let (rtx, rrx) = std::sync::mpsc::channel();
        app.pic_tx = Some(super::images::start_loader(rtx, Box::new(move || pw.wake())));
        // forward decoded pictures into the main channel
        std::thread::Builder::new()
            .name("picture relay".into())
            .spawn(move || {
                while let Ok(d) = rrx.recv() {
                    if ptx.send(Msg::Picture(d)).is_err() {
                        return;
                    }
                }
            })
            .ok();
        app
    }

    pub fn mark(&mut self, what: &'static str) {
        self.timings.push((what, self.started.elapsed().as_secs_f64() * 1000.0));
    }

    /// DIPs -> pixels for this window.
    pub fn layout_scale(&self) -> f32 {
        // SAFETY: plain query on our window.
        unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(self.hwnd).max(96) as f32 / 96.0 }
    }

    pub fn theme(&self) -> theme::Theme {
        *self.themes.get()
    }

    pub fn invalidate(&mut self) {
        self.dirty = true;
        super::window::invalidate(self.hwnd);
    }

    pub fn save_config(&mut self) {
        if let Err(e) = self.cfg_file.save(&self.cfg) {
            crate::log::error(&format!("could not save config: {e}"));
        }
    }

    // ---------- library ----------

    pub fn clip(&self, id: u32) -> Option<&Clip> {
        self.by_id.get(&id).map(|i| &self.clips[*i])
    }

    pub fn id_of(&self, name: &str) -> Option<u32> {
        self.clips.iter().find(|c| c.name == name).map(|c| c.id)
    }

    /// Re-read sounds/: keeps ids (and so decoded audio, pictures, positions) of unchanged and
    /// just moved/renamed clips.
    pub fn reload(&mut self) {
        let scan = match library::scan(&self.lib.sounds) {
            Ok(s) => s,
            Err(e) => {
                self.toast(&format!("Couldn't read the sounds folder: {e}"), Level::Bad);
                library::Scan::default()
            }
        };
        let moved = std::mem::take(&mut self.moved);
        library::reconcile(&mut self.cfg, &scan, &moved);
        let old: HashMap<String, Clip> = self.clips.drain(..).map(|c| (c.name.clone(), c)).collect();
        let mut by_name: HashMap<String, Clip> = HashMap::new();
        let mut cache_list = Vec::new();
        for f in &scan.clips {
            let prev = moved.get(&f.name).and_then(|o| old.get(o)).or_else(|| old.get(&f.name));
            let changed = prev.map(|p| p.size != f.size || p.mtime != f.mtime).unwrap_or(true);
            let id = match prev {
                Some(p) => p.id,
                None => {
                    self.next_id += 1;
                    self.next_id
                }
            };
            let pic_path = f.picture();
            let picture = std::fs::metadata(&pic_path).ok().map(|m| {
                let stamp = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos() as u64).unwrap_or(1);
                (pic_path.clone(), stamp ^ m.len())
            });
            let duration = match prev {
                Some(p) if !changed => p.duration,
                _ => crate::audio::decode::quick_duration(&f.path),
            };
            cache_list.push((id, f.path.clone(), changed && prev.is_some()));
            by_name.insert(
                f.name.clone(),
                Clip {
                    name: f.name.clone(),
                    id,
                    path: f.path.clone(),
                    stem: f.stem().to_string(),
                    group: f.group().to_string(),
                    duration,
                    size: f.size,
                    mtime: f.mtime,
                    picture,
                    failed: prev.filter(|_| !changed).and_then(|p| p.failed.clone()),
                },
            );
        }
        self.clips = self.cfg.order.iter().filter_map(|n| by_name.remove(n)).collect();
        self.reindex();
        self.groups = scan.groups;
        // the cache gets them in board order, hotkeyed ones first
        let order: HashMap<u32, usize> = self.clips.iter().enumerate().map(|(i, c)| (c.id, i)).collect();
        cache_list.sort_by_key(|(id, _, _)| order.get(id).copied().unwrap_or(usize::MAX));
        self.player.set_clips(&cache_list);
        let hot: Vec<u32> = self.cfg.hotkeys.keys().filter_map(|k| self.id_of(k)).collect();
        self.player.prioritise(&hot);
        self.save_config();
        self.apply_hotkeys();
        self.invalidate();
    }

    fn reindex(&mut self) {
        self.by_id = self.clips.iter().enumerate().map(|(i, c)| (c.id, i)).collect();
    }

    /// The tab shown: All while searching, else the chosen group.
    /// Re-read sounds/ only if files or folders changed on disk (cheap: one directory scan, no
    /// config write), e.g. when the window comes back to the front.
    pub fn refresh_if_changed(&mut self) {
        if self.drag.is_some() {
            return;
        }
        let Ok(scan) = library::scan(&self.lib.sounds) else { return };
        let same = scan.groups == self.groups
            && scan.clips.len() == self.clips.len()
            && scan.clips.iter().all(|f| self.id_of(&f.name).and_then(|id| self.clip(id)).map(|c| c.size == f.size && c.mtime == f.mtime).unwrap_or(false));
        if !same {
            crate::log::info("sounds folder changed on disk; reloading");
            self.reload();
        }
    }

    pub fn view_key(&self) -> String {
        if !self.query.is_empty() {
            ALL.to_string()
        } else {
            self.cfg.group.clone()
        }
    }

    /// The clips on the board right now (in order).
    pub fn visible(&self) -> Vec<u32> {
        let key = self.view_key();
        let q = self.query.to_lowercase();
        self.clips
            .iter()
            .filter(|c| key == ALL || c.group == key)
            .filter(|c| q.is_empty() || c.stem.to_lowercase().contains(&q))
            .map(|c| c.id)
            .collect()
    }

    pub fn group_color(&self, group: &str) -> Option<Rgb> {
        if group == ALL || group == NO_GROUP {
            return None;
        }
        self.cfg.group_colors.get(group).and_then(|c| Rgb::parse(c))
    }

    pub fn tint(&self, clip: &Clip) -> Rgb {
        self.group_color(&clip.group).unwrap_or(self.theme().accent)
    }

    /// (key, label, count) for every tab.
    pub fn tabs(&self) -> Vec<(String, String, usize)> {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for c in &self.clips {
            *counts.entry(c.group.as_str()).or_default() += 1;
        }
        let mut tabs = vec![(ALL.to_string(), "All".to_string(), self.clips.len())];
        for g in &self.groups {
            tabs.push((g.clone(), g.clone(), counts.get(g.as_str()).copied().unwrap_or(0)));
        }
        if !self.groups.is_empty() && counts.get(NO_GROUP).copied().unwrap_or(0) > 0 {
            tabs.push((NO_GROUP.to_string(), "No group".to_string(), counts[NO_GROUP]));
        }
        tabs
    }

    pub fn show_group(&mut self, key: &str) {
        if key == self.cfg.group && self.query.is_empty() {
            return;
        }
        self.cfg.group = key.to_string();
        if !self.query.is_empty() {
            self.search.set("");
            self.query.clear();
        }
        self.save_config();
        self.invalidate();
    }

    pub fn search_changed(&mut self) {
        let q = self.search.text.trim().to_lowercase();
        if q != self.query {
            self.query = q;
            self.scroll.insert(ALL.into(), 0.0);
            self.scroll_target.insert(ALL.into(), 0.0);
        }
        self.invalidate();
    }

    // ---------- playback ----------

    pub fn press(&mut self, id: u32) {
        let Some(c) = self.clip(id) else { return };
        if let Some(e) = &c.failed {
            let msg = format!("Couldn't load {}: {e}", c.stem);
            self.toast(&msg, Level::Warn);
            return;
        }
        self.player.press(id);
    }

    /// Loop a sound (double-click, or its menu), or let it end at the end of this pass.
    pub fn set_loop(&mut self, id: u32, on: bool) {
        if let Some(c) = self.clip(id) {
            if let Some(e) = &c.failed {
                let msg = format!("Couldn't load {}: {e}", c.stem);
                return self.toast(&msg, Level::Warn);
            }
        }
        self.player.set_loop(id, on);
        if on {
            self.looping.insert(id);
            let stem = self.clip(id).map(|c| c.stem.clone()).unwrap_or_default();
            self.toast(&format!("Looping '{stem}'. Click it again (or its hotkey) to stop."), Level::Info);
        } else {
            self.looping.remove(&id);
        }
        super::window::set_timer(self.hwnd, super::window::TIMER_PLAYING, 33);
        self.invalidate();
    }

    pub fn flash_tile(&mut self, id: u32, strength: f32) {
        self.flash.insert(id, (Instant::now(), strength));
        self.invalidate();
    }

    pub fn stop_all(&mut self) {
        self.player.stop_all();
    }

    /// Poll the engine's progress (while anything plays).
    pub fn poll_progress(&mut self) -> bool {
        let mut buf = std::mem::take(&mut self.progress);
        self.player.progress(&mut buf);
        let now: HashMap<u32, f32> = buf.iter().copied().collect();
        let mut ids = Vec::new();
        self.player.looping_ids(&mut ids);
        let looping: std::collections::HashSet<u32> = ids.into_iter().collect();
        let changed = now != self.playing || looping != self.looping;
        self.playing = now;
        self.looping = looping;
        self.progress = buf;
        self.player.collect_garbage();
        if changed || !self.playing.is_empty() {
            self.invalidate();
        }
        !self.playing.is_empty()
    }

    // ---------- toasts ----------

    pub fn toast(&mut self, text: &str, level: Level) {
        if let Some(mut t) = self.toast.take() {
            t.closing = Some(Instant::now());
            self.old_toasts.push(t);
        }
        self.toast = Some(Toast { text: text.to_string(), level, born: Instant::now(), closing: None });
        super::window::set_timer(self.hwnd, super::window::TIMER_TOAST, 4200);
        self.invalidate();
    }

    pub fn close_toast(&mut self) {
        if let Some(mut t) = self.toast.take() {
            t.closing = Some(Instant::now());
            self.old_toasts.push(t);
            self.invalidate();
        }
    }

    pub fn set_status(&mut self, text: &str, level: Level) {
        self.status = (text.to_string(), level);
        self.invalidate();
    }

    // ---------- audio ----------

    pub fn device_name(&self, slot: DeviceSlot) -> Option<String> {
        match slot {
            DeviceSlot::Mic => self.cfg.mic.clone(),
            DeviceSlot::Cable => self.cfg.cable.clone(),
            DeviceSlot::Monitor => self.cfg.monitor.clone(),
        }
    }

    /// Re-read the device lists and (re)open the streams, on a worker thread so the window never
    /// waits for a slow driver.
    pub fn start_audio(&mut self) {
        if test_mode() {
            // tests never open a real mic, cable or speaker
            self.audio_starting = false;
            self.devices = Devices { inputs: vec!["Test mic".into()], outputs: vec!["Test cable input".into(), "Test speakers".into()], cable_installed: true };
            self.set_status("Live", Level::Good);
            return;
        }
        self.audio_starting = true;
        self.set_status("Starting audio…", Level::Info);
        let stats = self.player.stats();
        if stats != Default::default() && (stats.cable_underflows + stats.monitor_underflows + stats.mic_gaps + stats.mic_skips + stats.clipped_samples) > 0 {
            crate::log::info(&format!("audio stats: {stats:?}"));
        }
        let player = self.player.clone();
        let (tx, w) = (self.tx.clone(), self.waker);
        let mut cfg = (self.cfg.mic.clone(), self.cfg.cable.clone(), self.cfg.monitor.clone());
        std::thread::Builder::new()
            .name("audio start".into())
            .spawn(move || {
                let _com = wasapi::ComScope::mta();
                let ins = wasapi::list(Kind::Input);
                let outs = wasapi::list(Kind::Output);
                let cable_dev = outs.iter().find(|d| d.name.to_lowercase().contains("cable input")).map(|d| d.name.clone());
                let not_cable = |n: &&wasapi::DeviceInfo| !n.name.to_lowercase().contains("cable");
                let default_in = wasapi::default_device(Kind::Input).filter(|d| !d.name.to_lowercase().contains("cable")).or_else(|| ins.iter().find(not_cable).cloned());
                let default_out = wasapi::default_device(Kind::Output).filter(|d| !d.name.to_lowercase().contains("cable")).or_else(|| outs.iter().find(not_cable).cloned());
                if cfg.1.is_none() {
                    cfg.1 = cable_dev.clone();
                }
                if cfg.0.is_none() {
                    cfg.0 = default_in.map(|d| d.name);
                }
                if cfg.2.is_none() {
                    cfg.2 = default_out.map(|d| d.name);
                }
                let devices = Devices {
                    inputs: ins.iter().map(|d| d.name.clone()).collect(),
                    outputs: outs.iter().map(|d| d.name.clone()).collect(),
                    cable_installed: cable_dev.is_some(),
                };
                let r = player.start(cfg.0.as_deref(), cfg.1.as_deref(), cfg.2.as_deref());
                let _ = tx.send(Msg::AudioStarted(r, devices, [cfg.0, cfg.1, cfg.2]));
                w.wake();
            })
            .ok();
    }

    /// A device stopped: look (off the UI thread) whether the configured devices are all there
    /// again. Only then are the streams reopened, so a mic that still works isn't cut every few
    /// seconds while the headphones are unplugged.
    pub fn probe_audio(&mut self) {
        if !self.audio_lost || self.audio_starting || test_mode() {
            return;
        }
        let want = (self.cfg.mic.clone(), self.cfg.cable.clone(), self.cfg.monitor.clone());
        let (tx, w) = (self.tx.clone(), self.waker);
        std::thread::Builder::new()
            .name("audio probe".into())
            .spawn(move || {
                let _com = wasapi::ComScope::mta();
                let ins = wasapi::list(Kind::Input);
                let outs = wasapi::list(Kind::Output);
                let has = |list: &[wasapi::DeviceInfo], n: &Option<String>| n.as_ref().map(|n| list.iter().any(|d| &d.name == n)).unwrap_or(true);
                let present = has(&ins, &want.0) && has(&outs, &want.1) && has(&outs, &want.2);
                let _ = tx.send(Msg::AudioProbe(present));
                w.wake();
            })
            .ok();
    }

    pub fn audio_started(&mut self, r: Result<Vec<String>, String>, devices: Devices, chosen: [Option<String>; 3]) {
        self.audio_starting = false;
        self.devices = devices;
        if std::mem::take(&mut self.audio_retry) {
            // a quiet reconnect: it keeps trying only while the streams fail to open
            match &r {
                Err(_) => {
                    self.set_status("Reconnecting audio…", Level::Warn);
                    super::window::set_timer(self.hwnd, super::window::TIMER_AUDIO_RETRY, AUDIO_RETRY_MS);
                    return self.invalidate();
                }
                Ok(w) => {
                    self.audio_lost = false;
                    crate::log::info("audio reconnected");
                    if w.is_empty() {
                        self.set_status("Live", Level::Good);
                        self.toast("Audio is back.", Level::Info);
                        return self.invalidate();
                    }
                    // opened, with a setup warning: shown the usual way below (once)
                }
            }
        }
        if r.is_ok() {
            self.audio_lost = false; // Restart audio (or a new device pick) fixed it
        }
        match r {
            Err(e) => {
                crate::log::error(&format!("audio start failed: {e}"));
                self.set_status("Audio error", Level::Bad);
                self.toast(&format!("Audio error: {e}"), Level::Bad);
                self.open_settings(true);
            }
            Ok(warnings) => {
                // devices picked automatically (first run) are remembered
                let [mic, cable, monitor] = chosen;
                self.cfg.mic = mic;
                self.cfg.cable = cable;
                self.cfg.monitor = monitor;
                self.save_config();
                if !self.devices.cable_installed {
                    self.set_status("VB-CABLE not installed", Level::Bad);
                    self.open_settings(true);
                } else if let Some(w) = warnings.first() {
                    self.set_status("Check audio setup", Level::Warn);
                    let w = w.clone();
                    self.toast(&w, Level::Warn);
                    self.open_settings(true);
                } else {
                    self.set_status("Live", Level::Good);
                }
            }
        }
        self.mark("audio started");
    }

    pub fn set_device(&mut self, slot: DeviceSlot, name: String) {
        match slot {
            DeviceSlot::Mic => self.cfg.mic = Some(name),
            DeviceSlot::Cable => self.cfg.cable = Some(name),
            DeviceSlot::Monitor => self.cfg.monitor = Some(name),
        }
        self.save_config();
        self.start_audio();
    }

    pub fn set_volume(&mut self, which: u8, v: f64) {
        let v = (v * 1000.0).round() / 1000.0;
        match which {
            0 => {
                self.cfg.mic_volume = v;
                self.player.levels.mic.set(v as f32);
            }
            1 => {
                self.cfg.clip_volume = v;
                self.player.levels.clips.set(v as f32);
            }
            _ => {
                self.cfg.monitor_volume = v;
                self.player.levels.monitor.set(v as f32);
            }
        }
        self.invalidate();
    }

    pub fn set_passthrough(&mut self, on: bool) {
        self.cfg.mic_passthrough = on;
        self.player.levels.passthrough.store(on, std::sync::atomic::Ordering::Relaxed);
        self.save_config();
        self.invalidate();
    }

    pub fn set_option(&mut self, solo: bool, value: bool) {
        if solo {
            self.cfg.solo = value;
            self.player.solo.store(value, std::sync::atomic::Ordering::Relaxed);
        } else {
            self.cfg.toggle_stop = value;
            self.player.toggle_stop.store(value, std::sync::atomic::Ordering::Relaxed);
        }
        self.save_config();
        self.invalidate();
    }

    // ---------- hotkeys ----------

    pub fn apply_hotkeys(&mut self) {
        let Some(h) = &self.hotkeys else { return };
        let mut pairs: Vec<(Binding, HkAction)> = self
            .cfg
            .hotkeys
            .iter()
            .filter_map(|(name, b)| self.clips.iter().find(|c| &c.name == name).map(|c| (b.clone(), HkAction::Clip(c.id))))
            .collect();
        if let Some(b) = &self.cfg.stop_hotkey {
            pairs.push((b.clone(), HkAction::StopAll));
        }
        h.set_bindings(&pairs);
    }

    pub fn hotkey_of(&self, id: u32) -> Option<&Binding> {
        self.clip(id).and_then(|c| self.cfg.hotkeys.get(&c.name))
    }

    /// One key = one action: assigning a key takes it off whatever had it (and says so).
    pub fn assign_hotkey(&mut self, target: HotkeyTarget, b: Option<Binding>) {
        let mut taken_from: Option<String> = None;
        if let Some(b) = &b {
            let before: Vec<String> = self.cfg.hotkeys.iter().filter(|(_, v)| v.same(b)).map(|(k, _)| k.clone()).collect();
            for k in &before {
                if HotkeyTarget::Clip(self.id_of(k).unwrap_or(0)) != target {
                    taken_from = Some(format!("'{}'", library::stem_of(k)));
                }
                self.cfg.hotkeys.remove(k);
            }
            if self.cfg.stop_hotkey.as_ref().map(|s| s.same(b)).unwrap_or(false) {
                if target != HotkeyTarget::StopAll {
                    taken_from = Some("Stop all".into());
                }
                self.cfg.stop_hotkey = None;
            }
        }
        let what = match &target {
            HotkeyTarget::Clip(id) => {
                let Some(name) = self.clip(*id).map(|c| c.name.clone()) else { return };
                match &b {
                    Some(b) => {
                        self.cfg.hotkeys.insert(name.clone(), b.clone());
                    }
                    None => {
                        self.cfg.hotkeys.remove(&name);
                    }
                }
                format!("'{}'", library::stem_of(&name))
            }
            HotkeyTarget::StopAll => {
                self.cfg.stop_hotkey = b.clone();
                "Stop all".to_string()
            }
        };
        self.save_config();
        self.apply_hotkeys();
        if let (Some(b), Some(from)) = (&b, taken_from) {
            self.toast(&format!("{} moved from {from} to {what}", b.pretty()), Level::Info);
        }
        if let HotkeyTarget::Clip(id) = target {
            self.player.prioritise(&[id]);
        }
        self.invalidate();
    }

    pub fn record_hotkey(&mut self, target: HotkeyTarget) {
        let what = match &target {
            HotkeyTarget::Clip(id) => format!("Hotkey for '{}'", self.clip(*id).map(|c| c.stem.clone()).unwrap_or_default()),
            HotkeyTarget::StopAll => "Hotkey for 'Stop all'".to_string(),
        };
        self.open_dialog(DialogKind::Hotkey { what, target }, vec![], 360.0);
        if let Some(h) = &self.hotkeys {
            h.record();
        }
    }

    // ---------- groups & files ----------

    fn carry_move(&mut self, old: &str, new: &str) {
        library::carry(&mut self.cfg, old, new);
        self.moved.insert(new.to_string(), old.to_string());
    }

    pub fn move_clip(&mut self, id: u32, group: &str, announce: bool) {
        let Some(c) = self.clip(id).cloned() else { return };
        if c.group == group {
            return;
        }
        self.player.stop(id);
        let folder = self.lib.folder_of_group(group);
        match self.lib.move_clip(&c.name, &folder, None) {
            Ok(new) => {
                self.carry_move(&c.name, &new);
                self.reload();
                if announce {
                    self.toast(&format!("Moved '{}' to {}", c.stem, if group.is_empty() { "No group" } else { group }), Level::Info);
                }
            }
            Err(e) => self.toast(&format!("Couldn't move '{}': {e}", c.stem), Level::Bad),
        }
    }

    pub fn rename_clip(&mut self, id: u32, new_stem: &str) {
        let Some(c) = self.clip(id).cloned() else { return };
        let stem = library::safe_name(new_stem);
        if stem == c.stem {
            return;
        }
        let folder = c.path.parent().map(PathBuf::from).unwrap_or_else(|| self.lib.sounds.clone());
        match self.lib.move_clip(&c.name, &folder, Some(&stem)) {
            Ok(new) => {
                self.carry_move(&c.name, &new);
                self.reload();
            }
            Err(e) => self.toast(&format!("Couldn't rename: {e}"), Level::Bad),
        }
    }

    pub fn delete_clip(&mut self, id: u32) {
        let Some(c) = self.clip(id).cloned() else { return };
        self.player.stop(id);
        match self.lib.delete_clip(&c.name) {
            Ok(_) => self.reload(),
            Err(e) => self.toast(&format!("Couldn't delete '{}': {e}", c.stem), Level::Bad),
        }
    }

    pub fn new_group(&mut self, name: &str, with: Option<u32>) {
        let g = match self.lib.create_group(name) {
            Ok(g) => g,
            Err(e) => return self.toast(&format!("Couldn't create the group: {e}"), Level::Bad),
        };
        match with {
            Some(id) => {
                let stem = self.clip(id).map(|c| c.stem.clone()).unwrap_or_default();
                self.move_clip(id, &g, false);
                self.toast(&format!("Moved '{stem}' to {g}"), Level::Info);
            }
            None => {
                self.cfg.group = g;
                self.reload();
            }
        }
    }

    pub fn rename_group(&mut self, group: &str, name: &str) {
        self.player.stop_all();
        let mut cfg = self.cfg.clone();
        match self.lib.rename_group(&mut cfg, group, name) {
            Ok(_) => {
                self.cfg = cfg;
                // the clips keep their ids through the rename
                let new = library::safe_name(name);
                for c in &self.clips {
                    if c.group == group {
                        let rest = &c.name[group.len() + 1..];
                        self.moved.insert(format!("{new}/{rest}"), c.name.clone());
                    }
                }
                self.reload();
            }
            Err(e) => self.toast(&e.to_string(), Level::Warn),
        }
    }

    pub fn delete_group(&mut self, group: &str) {
        let names: Vec<String> = self.clips.iter().map(|c| c.name.clone()).collect();
        let mut cfg = self.cfg.clone();
        match self.lib.delete_group(&mut cfg, group, &names) {
            Ok(moved) => {
                self.cfg = cfg;
                self.moved.extend(moved);
                self.reload();
            }
            Err(e) => {
                self.toast(&format!("Couldn't delete the group: {e}"), Level::Bad);
                self.reload();
            }
        }
    }

    pub fn set_group_color(&mut self, group: &str, color: Rgb) {
        self.cfg.group_colors.insert(group.to_string(), color.hex());
        self.save_config();
        self.invalidate();
    }

    /// Show the board in this order and remember it; the tiles slide to their new spots.
    pub fn set_order(&mut self, order: Vec<String>) {
        let have: HashSet<&String> = self.clips.iter().map(|c| &c.name).collect();
        self.cfg.order = order.into_iter().filter(|n| have.contains(n)).collect();
        let mut by_name: HashMap<String, Clip> = self.clips.drain(..).map(|c| (c.name.clone(), c)).collect();
        self.clips = self.cfg.order.iter().filter_map(|n| by_name.remove(n)).collect();
        self.clips.extend(by_name.into_values());
        self.reindex();
        self.slide_until = Instant::now() + Duration::from_millis(anim::MOTION_SLIDE as u64 + 50);
        self.save_config();
        self.invalidate();
    }

    pub fn sort_az(&mut self) {
        let mut names: Vec<(String, String)> = self.clips.iter().map(|c| (c.stem.to_lowercase(), c.name.clone())).collect();
        names.sort();
        self.set_order(names.into_iter().map(|(_, n)| n).collect());
        self.toast("Sorted A–Z", Level::Info);
    }

    /// Where new sounds go: the group being shown, else sounds/ itself.
    pub fn current_folder(&self) -> PathBuf {
        self.lib.folder_of_group(&self.cfg.group)
    }

    pub fn add_files(&mut self) {
        let exts: String = library::AUDIO_EXTS.iter().map(|e| format!("*.{e}")).collect::<Vec<_>>().join(";");
        let files = shell::open_files(self.hwnd, "Add sounds", &[("Audio/video", &exts), ("All", "*.*")], true);
        if files.is_empty() {
            return;
        }
        self.add_paths(&files);
    }

    /// Copy picked files into the open group (No group on All) and show the result.
    pub fn add_paths(&mut self, files: &[PathBuf]) {
        let (audio, other): (Vec<PathBuf>, Vec<PathBuf>) = files.iter().cloned().partition(|p| library::is_audio(p));
        let folder = self.current_folder();
        let results = self.lib.add_files(&audio, &folder);
        let mut errors: Vec<String> = results.iter().filter_map(|r| r.as_ref().err().cloned()).collect();
        if !other.is_empty() {
            let names: Vec<String> = other.iter().filter_map(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).collect();
            errors.push(format!("{} isn't a sound file", names.join(", ")));
        }
        let added: Vec<String> = results.into_iter().filter_map(Result::ok).collect();
        self.reload();
        for n in &added {
            if let Some(id) = self.id_of(n) {
                self.flash_tile(id, 0.32);
            }
        }
        if let Some(id) = added.last().and_then(|n| self.id_of(n)) {
            self.scroll_to_clip(id);
        }
        let g = self.cfg.group.clone();
        let place = if g == ALL || g == NO_GROUP { String::new() } else { format!(" to {g}") };
        match (added.len(), errors.is_empty()) {
            (_, false) => {
                let lead = if added.is_empty() { String::new() } else { format!("Added {}{place}. ", added.len()) };
                self.toast(&format!("{lead}Couldn't add: {}", errors.join("; ")), Level::Warn)
            }
            (0, true) => {}
            (1, true) => self.toast(&format!("Added '{}'{place}", library::stem_of(&added[0])), Level::Info),
            (n, true) => self.toast(&format!("Added {n} sounds{place}"), Level::Info),
        }
    }

    // ---------- pictures ----------

    pub fn choose_picture(&mut self, id: u32) {
        let exts: String = library::PICTURE_EXTS.iter().map(|e| format!("*.{e}")).collect::<Vec<_>>().join(";");
        let files = shell::open_files(self.hwnd, "Choose a picture", &[("Pictures", &exts), ("All", "*.*")], false);
        if let Some(f) = files.first() {
            let f = f.clone();
            self.set_picture(id, |dst| super::images::save_picture_from_file(&f, dst));
        }
    }

    pub fn paste_picture(&mut self, id: u32) {
        match shell::clipboard_image(self.hwnd) {
            Some(shell::ClipImage::Pixels(w, h, px)) => self.set_picture(id, |dst| super::images::save_picture_from_pixels(w, h, &px, dst)),
            Some(shell::ClipImage::File(p)) => self.set_picture(id, |dst| super::images::save_picture_from_file(&p, dst)),
            None => self.toast("There's no picture on the clipboard.", Level::Warn),
        }
    }

    fn set_picture(&mut self, id: u32, save: impl FnOnce(&std::path::Path) -> Result<(), String>) {
        let Some(c) = self.clip(id).cloned() else { return };
        let dst = library::picture_path(&c.path);
        match save(&dst) {
            Ok(()) => self.reload(),
            Err(e) => {
                crate::log::error(&format!("picture failed: {e}"));
                self.toast(&format!("Couldn't use that picture: {e}"), Level::Bad);
            }
        }
    }

    pub fn remove_picture(&mut self, id: u32) {
        let Some(c) = self.clip(id).cloned() else { return };
        let p = library::picture_path(&c.path);
        if p.exists() && !shell::recycle(&p) {
            self.toast("Couldn't remove the picture.", Level::Bad);
        }
        self.reload();
    }

    // ---------- theme ----------

    pub fn set_theme(&mut self, name: &str) {
        let t0 = Instant::now();
        if !self.themes.set(name) {
            return;
        }
        self.cfg.theme = self.themes.get().name.to_string();
        self.save_config();
        self.icons = super::window::apply_theme_chrome(self.hwnd, self.themes.get());
        self.invalidate();
        self.timings.push(("theme chrome", t0.elapsed().as_secs_f64() * 1000.0));
    }

    // ---------- quick import ----------

    pub fn quick_import(&mut self) {
        let link = shell::clipboard_text(self.hwnd).and_then(|t| importer::find_link(&t));
        let Some(link) = link else {
            return self.toast("Copy an Instagram or YouTube link first, then press Quick import.", Level::Warn);
        };
        if self.jobs.iter().any(|j| j.url == link) {
            return self.toast("That link is already importing.", Level::Warn);
        }
        let id = self.next_job;
        self.next_job += 1;
        self.jobs.push(Job {
            id,
            url: link.clone(),
            source: importer::source_of(&link),
            status: "Starting",
            error: None,
            full_error: None,
            started: Instant::now(),
        });
        let v = self.view_key();
        if v != ALL && v != NO_GROUP {
            self.toast("Importing… it'll appear under All (and No group).", Level::Info);
        }
        self.scroll_target.insert(self.view_key(), 0.0);
        self.run_job(id);
        self.invalidate();
    }

    pub fn run_job(&mut self, id: u64) {
        let Some(job) = self.jobs.iter().find(|j| j.id == id) else { return };
        let req = importer::Request {
            url: job.url.clone(),
            name: None,
            start: None,
            end: None,
            normalize: true,
            cookies_from_browser: self.cfg.cookies_from_browser.clone(),
            folder: self.lib.sounds.clone(),
        };
        let tools = importer::Tools { yt_dlp: self.paths.yt_dlp(), ffmpeg: self.paths.ffmpeg() };
        let (tx, w) = (self.tx.clone(), self.waker);
        std::thread::Builder::new()
            .name("quick import".into())
            .spawn(move || {
                let _com = wasapi::ComScope::mta();
                let tx2 = tx.clone();
                let r = importer::import_clip(&req, &tools, &|s| {
                    let label = match s {
                        "Downloading" => "Downloading",
                        "Cutting audio" => "Cutting audio",
                        _ => "Finishing",
                    };
                    let _ = tx2.send(Msg::JobStatus(id, label));
                    w.wake();
                });
                let r = r.map(|imp| finish_import(imp));
                let _ = tx.send(Msg::JobDone(id, r));
                w.wake();
            })
            .ok();
    }

    pub fn job_done(&mut self, id: u64, r: Result<PathBuf, String>) {
        let Some(i) = self.jobs.iter().position(|j| j.id == id) else { return }; // dismissed
        match r {
            Ok(path) => {
                self.jobs.remove(i);
                self.reload();
                let name = path.strip_prefix(&self.lib.sounds).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
                if let Some(cid) = self.id_of(&name) {
                    self.flash_tile(cid, 0.32);
                    self.scroll_to_clip(cid);
                }
                let stem = library::stem_of(&name).to_string();
                self.toast(&format!("Added '{stem}'"), Level::Info);
            }
            Err(e) => {
                crate::log::error(&format!("quick import failed: {}: {e}", self.jobs[i].url));
                let j = &mut self.jobs[i];
                j.error = Some(importer::short_error(&e));
                j.full_error = Some(e.chars().take(300).collect());
                let v = self.view_key();
                if v != ALL && v != NO_GROUP {
                    let msg = format!("Couldn't import: {}", self.jobs[i].error.clone().unwrap_or_default());
                    self.toast(&msg, Level::Bad);
                }
                self.invalidate();
            }
        }
    }

    pub fn retry_job(&mut self, id: u64) {
        if let Some(j) = self.jobs.iter_mut().find(|j| j.id == id) {
            j.error = None;
            j.full_error = None;
            j.status = "Starting";
            j.started = Instant::now();
        }
        self.run_job(id);
        self.invalidate();
    }

    pub fn dismiss_job(&mut self, id: u64) {
        self.jobs.retain(|j| j.id != id);
        self.invalidate();
    }

    pub fn scroll_to_clip(&mut self, id: u32) {
        self.layout.scroll_to = Some(id);
        self.invalidate();
    }

    // ---------- dialogs ----------

    pub fn open_dialog(&mut self, kind: DialogKind, fields: Vec<TextField>, width: f32) {
        self.close_menus();
        self.open_settings(false);
        if let Some(d) = self.dialog.take() {
            self.closing_dialog = Some(Dialog { closing: Some(Instant::now()), ..d });
        }
        self.dialog = Some(Dialog { kind, fields, focus: 0, born: Instant::now(), closing: None, message: None, width, rect: Rect::default() });
        self.caret_since = Instant::now();
        self.update_typing();
        self.invalidate();
    }

    pub fn close_dialog(&mut self) {
        if let Some(d) = self.dialog.take() {
            if matches!(d.kind, DialogKind::Hotkey { .. }) {
                if let Some(h) = &self.hotkeys {
                    h.cancel_record();
                }
            }
            self.closing_dialog = Some(Dialog { closing: Some(Instant::now()), ..d });
        }
        self.update_typing();
        self.invalidate();
    }

    pub fn confirm(&mut self, title: &str, text: &str, ok: &str, action: ConfirmAction) {
        self.open_dialog(DialogKind::Confirm { title: title.into(), text: text.into(), ok: ok.into(), action }, vec![], 380.0);
    }

    pub fn ask_text(&mut self, title: &str, initial: &str, ok: &str, placeholder: &str, action: TextAction) {
        let f = TextField::with_text(placeholder, initial);
        self.open_dialog(DialogKind::Text { title: title.into(), ok: ok.into(), action }, vec![f], 380.0);
    }

    pub fn import_dialog(&mut self) {
        let link = shell::clipboard_text(self.hwnd).and_then(|t| importer::find_link(&t)).unwrap_or_default();
        let mut url = TextField::new("https://www.instagram.com/reel/...  or  https://youtu.be/...");
        url.set(&link);
        let fields = vec![url, TextField::new("Leave empty to use the video's title"), TextField::new("0:00"), TextField::new("Full length")];
        let g = self.cfg.group.clone();
        let group = if g == ALL || g == NO_GROUP { String::new() } else { g };
        self.open_dialog(DialogKind::Import { normalize: true, group, new_group: false, busy: false, started: None }, fields, 480.0);
    }

    /// The import dialog's Group: an existing group (`Some`, "" = No group), or a new one typed
    /// in place of the dropdown (`None`).
    pub fn set_import_group(&mut self, choice: Option<String>) {
        let Some(d) = &mut self.dialog else { return };
        let DialogKind::Import { group, new_group, busy, .. } = &mut d.kind else { return };
        if *busy {
            return;
        }
        match choice {
            Some(g) => {
                *group = g;
                if *new_group {
                    *new_group = false;
                    d.fields.truncate(4);
                    d.focus = d.focus.min(3);
                }
            }
            None => {
                if !*new_group {
                    *new_group = true;
                    d.fields.truncate(4);
                    d.fields.push(TextField::new("New group name"));
                }
                d.focus = 4;
                self.caret_since = Instant::now();
            }
        }
        d.message = None;
        self.invalidate();
    }

    /// The dialog's OK.
    pub fn dialog_ok(&mut self) {
        let Some(d) = &mut self.dialog else { return };
        match &mut d.kind {
            DialogKind::Confirm { action, .. } => {
                let a = action.clone();
                self.close_dialog();
                match a {
                    ConfirmAction::DeleteClip(id) => self.delete_clip(id),
                    ConfirmAction::DeleteGroup(g) => self.delete_group(&g),
                }
            }
            DialogKind::Text { action, .. } => {
                let text = d.fields[0].text.trim().to_string();
                if text.is_empty() {
                    d.message = Some(("Type a name first.".into(), WARN));
                    return self.invalidate();
                }
                let a = action.clone();
                self.close_dialog();
                match a {
                    TextAction::NewGroup(with) => self.new_group(&text, with),
                    TextAction::RenameGroup(g) => self.rename_group(&g, &text),
                    TextAction::RenameClip(id) => self.rename_clip(id, &text),
                }
            }
            DialogKind::Hotkey { .. } => self.close_dialog(),
            DialogKind::Import { normalize, group, new_group, busy, started } => {
                if *busy {
                    return;
                }
                let url = d.fields[0].text.trim().to_string();
                let name = d.fields[1].text.trim().to_string();
                if url.is_empty() {
                    d.message = Some(("Paste an Instagram or YouTube link first.".into(), WARN));
                    d.focus = 0;
                    return self.invalidate();
                }
                let (start, end) = match (importer::parse_time(&d.fields[2].text), importer::parse_time(&d.fields[3].text)) {
                    (Ok(s), Ok(e)) => (s, e),
                    _ => {
                        d.message = Some(("Times look like 7, 0:07 or 0:07.5".into(), WARN));
                        return self.invalidate();
                    }
                };
                let mut created = false;
                if *new_group {
                    // the group is made now (an import that's cancelled leaves no empty folder)
                    let typed = d.fields.get(4).map(|f| f.text.trim().to_string()).unwrap_or_default();
                    if typed.is_empty() {
                        d.message = Some(("Type a name for the new group, or pick an existing one.".into(), WARN));
                        d.focus = 4;
                        return self.invalidate();
                    }
                    let safe = library::safe_name(&typed);
                    // an existing group with that name (any case) is the same folder on Windows
                    let existing = self.groups.iter().find(|g| g.eq_ignore_ascii_case(&safe)).cloned();
                    match existing.map(Ok).unwrap_or_else(|| self.lib.create_group(&typed)) {
                        Ok(g) => {
                            created = !self.groups.contains(&g);
                            *group = g;
                            *new_group = false;
                            d.fields.truncate(4);
                            d.focus = d.focus.min(3);
                        }
                        Err(e) => {
                            d.message = Some((format!("Couldn't create the group: {e}"), DANGER));
                            return self.invalidate();
                        }
                    }
                }
                *busy = true;
                *started = Some(Instant::now());
                d.message = None;
                self.import_target = Some(group.clone());
                let req = importer::Request {
                    url,
                    name: if name.is_empty() { None } else { Some(name) },
                    start,
                    end,
                    normalize: *normalize,
                    cookies_from_browser: self.cfg.cookies_from_browser.clone(),
                    folder: self.lib.folder_of_group(group),
                };
                let tools = importer::Tools { yt_dlp: self.paths.yt_dlp(), ffmpeg: self.paths.ffmpeg() };
                let (tx, w) = (self.tx.clone(), self.waker);
                std::thread::Builder::new()
                    .name("import".into())
                    .spawn(move || {
                        let _com = wasapi::ComScope::mta();
                        let tx2 = tx.clone();
                        let r = importer::import_clip(&req, &tools, &|s| {
                            let label = match s {
                                "Downloading" => "Downloading…",
                                "Cutting audio" => "Cutting audio…",
                                _ => "Finishing…",
                            };
                            let _ = tx2.send(Msg::DialogImportStatus(label));
                            w.wake();
                        });
                        let _ = tx.send(Msg::DialogImport(r.map(finish_import)));
                        w.wake();
                    })
                    .ok();
                if created {
                    self.reload(); // its tab shows up now, not only once the import finishes
                }
                self.invalidate();
            }
        }
    }

    pub fn dialog_import_done(&mut self, r: Result<PathBuf, String>) {
        match r {
            Ok(path) => {
                if matches!(self.dialog.as_ref().map(|d| &d.kind), Some(DialogKind::Import { .. })) {
                    self.close_dialog();
                }
                let target = self.import_target.take().unwrap_or_default();
                // show where it went: stay on All, or on the group it was imported into
                let v = self.view_key();
                if v != ALL && v != target {
                    self.cfg.group = if target.is_empty() { NO_GROUP.to_string() } else { target.clone() };
                    self.save_config();
                }
                self.reload();
                let name = path.strip_prefix(&self.lib.sounds).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
                if let Some(cid) = self.id_of(&name) {
                    self.flash_tile(cid, 0.32);
                    self.scroll_to_clip(cid);
                }
                let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let place = if target.is_empty() { String::new() } else { format!(" to {target}") };
                self.toast(&format!("Added '{stem}'{place}"), Level::Info);
            }
            Err(e) => {
                crate::log::error(&format!("import failed: {e}"));
                let mut text = e.clone();
                if text.to_lowercase().contains("login") || text.to_lowercase().contains("cookies") {
                    text.push_str("\n\nInstagram wants a login. Set \"cookies_from_browser\": \"firefox\" in config.json (while logged in to Instagram in Firefox).");
                }
                match &mut self.dialog {
                    Some(d) if matches!(d.kind, DialogKind::Import { .. }) => {
                        if let DialogKind::Import { busy, started, .. } = &mut d.kind {
                            *busy = false;
                            *started = None;
                        }
                        d.message = Some((format!("Failed: {text}"), DANGER));
                    }
                    _ => self.toast(&format!("Couldn't import: {}", importer::short_error(&e)), Level::Bad),
                }
                self.invalidate();
            }
        }
    }

    // ---------- menus & settings ----------

    pub fn close_menus(&mut self) {
        let now = Instant::now();
        for m in self.menus.drain(..) {
            self.closing_menus.push(ClosingMenu { menu: m, since: now, alpha: 1.0 });
        }
        self.invalidate();
    }

    pub fn open_settings(&mut self, open: bool) {
        if open == self.settings_open {
            return;
        }
        self.close_menus();
        self.settings_open = open;
        self.settings_changed = Instant::now();
        self.invalidate();
    }

    /// Run a menu action (after the menu closes).
    pub fn run(&mut self, a: Action) {
        match a {
            Action::Play(id) => self.press(id),
            Action::Loop(id, on) => self.set_loop(id, on),
            Action::SetHotkey(id) => self.record_hotkey(HotkeyTarget::Clip(id)),
            Action::ClearHotkey(id) => self.assign_hotkey(HotkeyTarget::Clip(id), None),
            Action::MoveTo(id, g) => self.move_clip(id, &g, false),
            Action::NewGroupWith(id) => self.ask_text("New group", "", "Create", "Group name, e.g. Memes", TextAction::NewGroup(Some(id))),
            Action::ChoosePicture(id) => self.choose_picture(id),
            Action::PastePicture(id) => self.paste_picture(id),
            Action::RemovePicture(id) => self.remove_picture(id),
            Action::Rename(id) => {
                let stem = self.clip(id).map(|c| c.stem.clone()).unwrap_or_default();
                self.ask_text("Rename sound", &stem, "Save", "", TextAction::RenameClip(id));
            }
            Action::Delete(id) => {
                let stem = self.clip(id).map(|c| c.stem.clone()).unwrap_or_default();
                self.confirm("Delete sound?", &format!("'{stem}' is moved to sounds\\_deleted, so you can get it back later."), "Delete", ConfirmAction::DeleteClip(id));
            }
            Action::RenameGroup(g) => self.ask_text("Rename group", &g.clone(), "Save", "", TextAction::RenameGroup(g)),
            Action::DeleteGroup(g) => {
                self.confirm("Delete group?", &format!("The sounds in '{g}' aren't deleted. They move to No group."), "Delete", ConfirmAction::DeleteGroup(g))
            }
            Action::GroupColor(g, c) => self.set_group_color(&g, c),
            Action::OpenFolder => shell::open(&self.lib.sounds.to_string_lossy()),
            Action::Reload => self.reload(),
            Action::SortAz => self.sort_az(),
            Action::StopHotkey => self.record_hotkey(HotkeyTarget::StopAll),
            Action::Theme(n) => self.set_theme(&n),
            Action::Device(slot, name) => self.set_device(slot, name),
            Action::ImportGroup(g) => self.set_import_group(Some(g)),
            Action::ImportNewGroup => self.set_import_group(None),
        }
    }

    /// Tell the hotkey hook whether one of our text fields has the keyboard.
    pub fn update_typing(&self) {
        let typing = self.search_focused || self.dialog.as_ref().map(|d| !d.fields.is_empty()).unwrap_or(false);
        if let Some(h) = &self.hotkeys {
            h.shared().typing.store(typing && super::window::is_foreground(self.hwnd), std::sync::atomic::Ordering::Relaxed);
        }
    }

    // ---------- background messages ----------

    pub fn drain(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Engine(Event::Played(id)) => {
                    self.played.push(id);
                    self.flash_tile(id, 0.32);
                    super::window::set_timer(self.hwnd, super::window::TIMER_PLAYING, 33);
                    self.poll_progress();
                }
                Msg::Engine(Event::Loaded(id, secs)) => {
                    if let Some(i) = self.by_id.get(&id).copied() {
                        if self.clips[i].duration.is_none() {
                            self.clips[i].duration = Some(secs);
                            self.invalidate();
                        }
                    }
                }
                Msg::Engine(Event::LoadFailed(id, e)) => {
                    if let Some(i) = self.by_id.get(&id).copied() {
                        crate::log::error(&format!("failed to load {}: {e}", self.clips[i].name));
                        self.clips[i].failed = Some(e);
                        self.invalidate();
                    }
                }
                Msg::Engine(Event::DeviceLost) => {
                    if !self.audio_lost {
                        self.audio_lost = true;
                        crate::log::warn("an audio device stopped; reconnecting when it's back");
                        self.set_status("Reconnecting audio…", Level::Warn);
                        self.toast("An audio device stopped (unplugged?). It reconnects by itself when it's back.", Level::Warn);
                    }
                    super::window::set_timer(self.hwnd, super::window::TIMER_AUDIO_RETRY, AUDIO_RETRY_MS);
                }
                Msg::AudioProbe(present) => {
                    if !self.audio_lost || self.audio_starting {
                        // fixed by hand meanwhile, or a start is already running
                    } else if present {
                        self.audio_retry = true;
                        self.start_audio();
                    } else {
                        super::window::set_timer(self.hwnd, super::window::TIMER_AUDIO_RETRY, AUDIO_RETRY_MS);
                    }
                }
                Msg::Engine(Event::StoppedAll) => {
                    self.poll_progress();
                }
                Msg::Hotkey(HkAction::Clip(id)) => {
                    // the hook thread hands it to us; play straight away
                    self.press(id);
                }
                Msg::Hotkey(HkAction::StopAll) => self.stop_all(),
                Msg::Recorded(b) => {
                    if let Some(Dialog { kind: DialogKind::Hotkey { target, .. }, .. }) = &self.dialog {
                        let t = target.clone();
                        self.close_dialog();
                        if let Some(b) = b {
                            self.assign_hotkey(t, Some(b));
                        }
                    }
                }
                Msg::JobStatus(id, s) => {
                    if let Some(j) = self.jobs.iter_mut().find(|j| j.id == id && j.error.is_none()) {
                        j.status = s;
                        self.invalidate();
                    }
                }
                Msg::JobDone(id, r) => self.job_done(id, r),
                Msg::DialogImportStatus(s) => {
                    if let Some(d) = &mut self.dialog {
                        if matches!(d.kind, DialogKind::Import { busy: true, .. }) {
                            d.message = Some((s.to_string(), self.themes.get().muted));
                            self.invalidate();
                        }
                    }
                }
                Msg::DialogImport(r) => self.dialog_import_done(r),
                Msg::Picture(d) => {
                    self.pic_pending.remove(&d.key);
                    match d.pixels {
                        Some(p) => {
                            self.pic_ready.insert(d.key, (d.px, p));
                        }
                        None => {
                            self.pic_failed.insert(d.key);
                        }
                    }
                    self.invalidate();
                }
                Msg::AudioStarted(r, devs, chosen) => self.audio_started(r, devs, chosen),
            }
        }
    }

    /// Before the window closes: remember the window, stop everything.
    pub fn shutdown(&mut self, geometry: Option<String>, maximized: bool) {
        if let Some(g) = geometry {
            self.cfg.geometry = Some(g);
        }
        let _ = maximized;
        self.save_config();
        let st = self.player.stop_streams();
        if st.cable_underflows + st.monitor_underflows + st.mic_gaps + st.mic_skips + st.clipped_samples > 0 {
            crate::log::info(&format!("audio stats: {st:?}"));
        }
        self.player.shutdown();
        self.hotkeys = None;
    }
}

/// After a download: the thumbnail becomes the clip's picture, then the temp folder goes.
fn finish_import(imp: importer::Imported) -> PathBuf {
    if let Some(t) = &imp.thumbnail {
        if let Err(e) = super::images::save_picture_from_file(t, &library::picture_path(&imp.audio)) {
            crate::log::warn(&format!("No picture: {e}"));
        }
    }
    let _ = std::fs::remove_dir_all(&imp.temp);
    imp.audio
}

/// JONDISBOARD_TEST=1: no devices, a stand-in clipboard, the window stays hidden.
pub fn test_mode() -> bool {
    std::env::var_os("JONDISBOARD_TEST").is_some()
}

/// How often a lost audio device is looked for.
const AUDIO_RETRY_MS: u32 = 3000;

fn cfg_budget(_cfg: &Config) -> usize {
    // decoded audio kept in memory: 1 GiB (about 90 minutes of 16-bit stereo). Clips past the
    // budget are decoded when pressed.
    1 << 30
}
