//! The audio engine and the shared player.
//!
//! [`Player`] is what the UI thread and the hotkey thread both call: it holds the decoded-clip
//! cache, the playback rules (one sound at a time, press-to-stop) and the running streams. It is
//! `Sync` (short locks, never held by an audio thread), so a hotkey in a game plays its clip
//! straight away, even if the UI thread is busy.
//!
//! Clips are decoded lazily on a background loader thread (below-normal priority), within a
//! memory budget: startup only reads file headers. A clip that is pressed before it has been
//! decoded jumps the queue and starts as soon as it's ready.

use super::decode;
use super::mixer::{self, AtomicF32, ClipAudio, Cmd, MicInput, MicReader, MicRing, OutputControl, PlayState, SR};
use super::wasapi::{self, DeviceInfo, Kind, Stream};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

/// Something the engine tells the UI (delivered by the notifier, which wakes the UI thread).
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// a clip was started (by a click or a hotkey): flash its tile
    Played(u32),
    /// a clip finished decoding: (id, duration in seconds)
    Loaded(u32, f64),
    /// a clip couldn't be decoded
    LoadFailed(u32, String),
    /// a device failed while running (unplugged...)
    DeviceLost,
    /// the stop-all hotkey (or button) was used
    StoppedAll,
}

pub type Notify = Arc<dyn Fn(Event) + Send + Sync>;

/// The mixer settings the audio threads read on every block.
pub struct Levels {
    pub mic: Arc<AtomicF32>,
    pub clips: Arc<AtomicF32>,
    pub monitor: Arc<AtomicF32>,
    pub passthrough: Arc<AtomicBool>,
}

impl Levels {
    fn new() -> Levels {
        Levels {
            mic: Arc::new(AtomicF32::new(1.0)),
            clips: Arc::new(AtomicF32::new(0.8)),
            monitor: Arc::new(AtomicF32::new(0.6)),
            passthrough: Arc::new(AtomicBool::new(true)),
        }
    }
}

struct Output {
    /// None for offline outputs (diagnostics/tests render the mixer themselves)
    _stream: Option<Stream>,
    control: OutputControl,
    stats: Arc<wasapi::StreamStats>,
}

#[derive(Default)]
struct Streams {
    cable: Option<Output>,
    monitor: Option<Output>,
    mic: Option<Stream>,
}

struct CacheEntry {
    path: PathBuf,
    audio: Option<Arc<ClipAudio>>,
    failed: Option<String>,
    last_used: u64,
}

#[derive(Default)]
struct Cache {
    entries: HashMap<u32, CacheEntry>,
    queue: VecDeque<u32>,
    /// clips pressed before they were decoded: play once ready
    play_when_ready: HashMap<u32, Instant>,
    bytes: usize,
    clock: u64,
    shutdown: bool,
}

/// Counters for the log and diagnostics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub cable_underflows: u64,
    pub monitor_underflows: u64,
    pub mic_overflows: u64,
    pub mic_gaps: u64,
    pub mic_skips: u64,
    pub limited_blocks: u64,
    pub clipped_samples: u64,
    pub blocks: u64,
    pub avg_render_us: u64,
    pub max_render_us: u64,
}

pub struct Player {
    pub levels: Levels,
    pub solo: AtomicBool,
    pub toggle_stop: AtomicBool,
    streams: Mutex<Streams>,
    cache: Mutex<Cache>,
    loader_wake: Condvar,
    notify: Notify,
    ffmpeg: Option<PathBuf>,
    budget: usize,
    /// a stand-in for the outputs in tests (no devices): PlayState of a never-run mixer
    idle: Arc<PlayState>,
    pub decode_ns: AtomicU64,
}

impl Player {
    /// budget: bytes of decoded audio to keep in memory.
    pub fn new(ffmpeg: Option<PathBuf>, budget: usize, notify: Notify) -> Arc<Player> {
        let p = Arc::new(Player {
            levels: Levels::new(),
            solo: AtomicBool::new(false),
            toggle_stop: AtomicBool::new(false),
            streams: Mutex::new(Streams::default()),
            cache: Mutex::new(Cache::default()),
            loader_wake: Condvar::new(),
            notify,
            ffmpeg,
            budget,
            idle: PlayState::new(),
            decode_ns: AtomicU64::new(0),
        });
        let weak = Arc::downgrade(&p);
        std::thread::Builder::new()
            .name("clip loader".into())
            .spawn(move || loader(weak))
            .expect("spawn loader");
        p
    }

    // ---------- library ----------

    /// Tell the cache which clips exist (id -> file). Unchanged ids keep their decoded audio;
    /// clips that are gone are dropped. Everything not decoded yet is queued, in `order`.
    pub fn set_clips(&self, clips: &[(u32, PathBuf, bool)]) {
        let mut c = self.cache.lock().unwrap();
        let keep: HashMap<u32, (&PathBuf, bool)> = clips.iter().map(|(id, p, changed)| (*id, (p, *changed))).collect();
        let mut freed = 0;
        c.entries.retain(|id, e| match keep.get(id) {
            Some((_, false)) => true,
            _ => {
                freed += e.audio.as_ref().map(|a| a.samples.bytes()).unwrap_or(0);
                false
            }
        });
        c.bytes -= freed;
        for (id, path, _) in clips {
            let e = c.entries.entry(*id).or_insert_with(|| CacheEntry { path: path.clone(), audio: None, failed: None, last_used: 0 });
            e.path = path.clone();
        }
        let queue: VecDeque<u32> = clips
            .iter()
            .filter(|(id, _, _)| c.entries.get(id).map(|e| e.audio.is_none() && e.failed.is_none()).unwrap_or(false))
            .map(|(id, _, _)| *id)
            .collect();
        c.queue = queue;
        c.play_when_ready.retain(|id, _| keep.contains_key(id));
        drop(c);
        self.loader_wake.notify_all();
    }

    /// Decode these first (e.g. clips with hotkeys, or the ones on screen).
    pub fn prioritise(&self, ids: &[u32]) {
        let mut c = self.cache.lock().unwrap();
        for id in ids.iter().rev() {
            if let Some(i) = c.queue.iter().position(|q| q == id) {
                c.queue.remove(i);
                c.queue.push_front(*id);
            }
        }
        drop(c);
        self.loader_wake.notify_all();
    }

    pub fn is_loaded(&self, id: u32) -> bool {
        self.cache.lock().unwrap().entries.get(&id).map(|e| e.audio.is_some()).unwrap_or(false)
    }

    pub fn cached_bytes(&self) -> usize {
        self.cache.lock().unwrap().bytes
    }

    /// Wait until the loader has nothing left to do (tests and diagnostics).
    pub fn wait_idle(&self, timeout: std::time::Duration) -> bool {
        let t = Instant::now();
        while t.elapsed() < timeout {
            {
                let c = self.cache.lock().unwrap();
                if c.queue.is_empty() && c.play_when_ready.is_empty() {
                    return true;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        false
    }

    // ---------- playback ----------

    fn state(&self) -> Arc<PlayState> {
        let s = self.streams.lock().unwrap();
        s.cable.as_ref().or(s.monitor.as_ref()).map(|o| o.control.state.clone()).unwrap_or_else(|| self.idle.clone())
    }

    pub fn is_playing_any(&self) -> bool {
        self.state().count.load(Ordering::Acquire) > 0
    }

    pub fn is_playing(&self, id: u32) -> bool {
        self.state().is_playing(id)
    }

    /// (clip id, fraction played) of everything playing.
    pub fn progress(&self, out: &mut Vec<(u32, f32)>) {
        self.state().snapshot(out)
    }

    /// A click or a hotkey: the playback rules, then play.
    pub fn press(&self, id: u32) {
        if self.toggle_stop.load(Ordering::Relaxed) && self.is_playing(id) {
            self.stop(id);
            return;
        }
        if self.solo.load(Ordering::Relaxed) {
            self.stop_all_quiet();
        }
        self.play(id);
    }

    /// Start (or restart) a clip on both outputs.
    pub fn play(&self, id: u32) {
        let audio = {
            let mut c = self.cache.lock().unwrap();
            c.clock += 1;
            let clock = c.clock;
            match c.entries.get_mut(&id) {
                Some(e) => {
                    e.last_used = clock;
                    match &e.audio {
                        Some(a) => Some(a.clone()),
                        None => {
                            if e.failed.is_none() {
                                c.play_when_ready.insert(id, Instant::now());
                                if let Some(i) = c.queue.iter().position(|q| *q == id) {
                                    c.queue.remove(i);
                                }
                                c.queue.push_front(id);
                            }
                            None
                        }
                    }
                }
                None => None,
            }
        };
        match audio {
            Some(a) => {
                let mut g = self.streams.lock().unwrap();
                let s = &mut *g;
                for o in [s.cable.as_mut(), s.monitor.as_mut()].into_iter().flatten() {
                    o.control.send(Cmd::Play(a.clone()));
                }
                drop(g);
                (self.notify)(Event::Played(id));
            }
            None => self.loader_wake.notify_all(),
        }
    }

    pub fn stop(&self, id: u32) {
        self.cache.lock().unwrap().play_when_ready.remove(&id);
        let mut g = self.streams.lock().unwrap();
        let s = &mut *g;
        for o in [s.cable.as_mut(), s.monitor.as_mut()].into_iter().flatten() {
            o.control.send(Cmd::Stop(id));
        }
    }

    fn stop_all_quiet(&self) {
        self.cache.lock().unwrap().play_when_ready.clear();
        let mut g = self.streams.lock().unwrap();
        let s = &mut *g;
        for o in [s.cable.as_mut(), s.monitor.as_mut()].into_iter().flatten() {
            o.control.send(Cmd::StopAll);
        }
    }

    pub fn stop_all(&self) {
        self.stop_all_quiet();
        (self.notify)(Event::StoppedAll);
    }

    /// Free clip memory the audio threads have finished with.
    pub fn collect_garbage(&self) {
        let mut g = self.streams.lock().unwrap();
        let s = &mut *g;
        for o in [s.cable.as_mut(), s.monitor.as_mut()].into_iter().flatten() {
            o.control.collect();
        }
    }

    // ---------- devices ----------

    pub fn outputs_open(&self) -> (bool, bool) {
        let s = self.streams.lock().unwrap();
        (s.cable.is_some(), s.monitor.is_some())
    }

    pub fn stats(&self) -> Stats {
        let s = self.streams.lock().unwrap();
        let mut st = Stats::default();
        let mut ns = 0;
        for (o, is_cable) in [(s.cable.as_ref(), true), (s.monitor.as_ref(), false)] {
            let Some(o) = o else { continue };
            let ps = &o.control.state;
            let under = o.stats.underflows.load(Ordering::Relaxed);
            if is_cable {
                st.cable_underflows = under;
                st.mic_gaps = ps.mic_gaps.load(Ordering::Relaxed);
                st.mic_skips = ps.mic_skips.load(Ordering::Relaxed);
            } else {
                st.monitor_underflows = under;
            }
            st.limited_blocks += ps.limited_blocks.load(Ordering::Relaxed);
            st.clipped_samples += ps.clipped_samples.load(Ordering::Relaxed);
            st.blocks += ps.blocks.load(Ordering::Relaxed);
            ns += ps.render_ns.load(Ordering::Relaxed);
            st.max_render_us = st.max_render_us.max(ps.max_render_ns.load(Ordering::Relaxed) / 1000);
        }
        if let Some(m) = &s.mic {
            st.mic_overflows = m.stats.overflows.load(Ordering::Relaxed);
        }
        if st.blocks > 0 {
            st.avg_render_us = ns / st.blocks / 1000;
        }
        st
    }

    /// Close every stream.
    pub fn stop_streams(&self) -> Stats {
        let st = self.stats();
        let mut s = self.streams.lock().unwrap();
        *s = Streams::default();
        st
    }

    /// (Re)open the streams. Returns warnings (shown in the app); Err if a device failed to open.
    pub fn start(&self, mic: Option<&str>, cable: Option<&str>, monitor: Option<&str>) -> Result<Vec<String>, String> {
        self.stop_streams();
        let mut warnings = Vec::new();
        let outs = wasapi::list(Kind::Output);
        let ins = wasapi::list(Kind::Input);
        let find = |list: &[DeviceInfo], name: Option<&str>| name.and_then(|n| list.iter().find(|d| d.name == n).cloned());
        let cable_dev = find(&outs, cable);
        let mut monitor_dev = find(&outs, monitor);
        let mut mic_dev = find(&ins, mic);
        if cable_dev.is_none() {
            warnings.push("No virtual cable selected - clips won't reach Discord/games.".to_string());
        }
        if mic.map(|m| m.to_lowercase().contains("cable")).unwrap_or(false) {
            warnings.push("Mic is set to a CABLE device - that would loop audio, so mic is disabled.".to_string());
            mic_dev = None;
        }
        if cable_dev.is_some() && cable_dev == monitor_dev {
            warnings.push("Monitor and cable are the same device - monitor disabled.".to_string());
            monitor_dev = None;
        }
        let mut streams = Streams::default();
        let notify = self.notify.clone();
        let lost = move || (notify)(Event::DeviceLost);
        let lost: Arc<dyn Fn() + Send + Sync> = Arc::new(lost);
        if let Some(dev) = &cable_dev {
            let mic_input = match &mic_dev {
                Some(m) => {
                    let ring = MicRing::new(SR as usize);
                    let l = lost.clone();
                    match wasapi::start_input(m, ring.clone(), Box::new(move || l())) {
                        Ok(stream) => {
                            streams.mic = Some(stream);
                            Some(MicInput {
                                reader: MicReader::new(ring, SR as u64 * 12 / 100),
                                volume: self.levels.mic.clone(),
                                passthrough: self.levels.passthrough.clone(),
                            })
                        }
                        Err(e) => {
                            warnings.push(format!("Couldn't open the microphone: {e}"));
                            None
                        }
                    }
                }
                None => None,
            };
            let (mixer, control) = mixer::output(self.levels.clips.clone(), mic_input);
            let l = lost.clone();
            let stream = wasapi::start_output(dev, mixer, Box::new(move || l())).map_err(|e| format!("{}: {e}", dev.name))?;
            let stats = stream.stats.clone();
            streams.cable = Some(Output { _stream: Some(stream), control, stats });
        }
        if let Some(dev) = &monitor_dev {
            let (mixer, control) = mixer::output(self.levels.monitor.clone(), None);
            let l = lost.clone();
            match wasapi::start_output(dev, mixer, Box::new(move || l())) {
                Ok(stream) => {
                    let stats = stream.stats.clone();
                    streams.monitor = Some(Output { _stream: Some(stream), control, stats });
                }
                Err(e) => warnings.push(format!("Couldn't open {}: {e}", dev.name)),
            }
        }
        *self.streams.lock().unwrap() = streams;
        Ok(warnings)
    }

    pub fn shutdown(&self) {
        self.stop_streams();
        self.cache.lock().unwrap().shutdown = true;
        self.loader_wake.notify_all();
    }

    /// Offline outputs with no devices behind them: the caller renders the two mixers itself
    /// (the audio diagnostics and tests use this to run the exact engine code).
    pub fn attach_offline(&self, with_mic: Option<Arc<MicRing>>) -> (mixer::OutputMixer, mixer::OutputMixer) {
        let mic = with_mic.map(|ring| MicInput {
            reader: MicReader::new(ring, SR as u64 * 12 / 100),
            volume: self.levels.mic.clone(),
            passthrough: self.levels.passthrough.clone(),
        });
        let (cm, cc) = mixer::output(self.levels.clips.clone(), mic);
        let (mm, mc) = mixer::output(self.levels.monitor.clone(), None);
        let mut s = self.streams.lock().unwrap();
        s.cable = Some(Output { _stream: None, control: cc, stats: Default::default() });
        s.monitor = Some(Output { _stream: None, control: mc, stats: Default::default() });
        (cm, mm)
    }
}

/// The background decoder: decodes queued clips (front first) within the memory budget.
fn loader(player: std::sync::Weak<Player>) {
    loop {
        let Some(p) = player.upgrade() else { return };
        let job = {
            let mut c = p.cache.lock().unwrap();
            loop {
                if c.shutdown {
                    return;
                }
                // over budget: only decode clips someone is waiting to play
                let urgent = c.queue.iter().position(|id| c.play_when_ready.contains_key(id));
                let next = match urgent {
                    Some(i) => c.queue.remove(i),
                    None if c.bytes < p.budget => c.queue.pop_front(),
                    None => None,
                };
                if let Some(id) = next {
                    if let Some(e) = c.entries.get(&id) {
                        if e.audio.is_none() && e.failed.is_none() {
                            break Some((id, e.path.clone()));
                        }
                    }
                    continue;
                }
                let (g, _) = p.loader_wake.wait_timeout(c, std::time::Duration::from_secs(5)).unwrap();
                c = g;
                if player.strong_count() == 0 {
                    return;
                }
            }
        };
        let Some((id, path)) = job else { continue };
        let t = Instant::now();
        let r = decode::load(&path, p.ffmpeg.as_deref());
        p.decode_ns.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        let mut play_now = false;
        let event = {
            let mut c = p.cache.lock().unwrap();
            let waiting = c.play_when_ready.remove(&id);
            match r {
                Ok(samples) => {
                    let bytes = samples.bytes();
                    let frames = samples.frames();
                    let audio = Arc::new(ClipAudio { id, samples });
                    if let Some(e) = c.entries.get_mut(&id) {
                        e.audio = Some(audio);
                        c.bytes += bytes;
                        // pressed less than 3 s ago: still worth playing
                        play_now = waiting.map(|t| t.elapsed().as_secs_f32() < 3.0).unwrap_or(false);
                    }
                    evict(&mut c, p.budget, id);
                    Event::Loaded(id, frames as f64 / SR as f64)
                }
                Err(e) => {
                    if let Some(en) = c.entries.get_mut(&id) {
                        en.failed = Some(e.clone());
                    }
                    Event::LoadFailed(id, e)
                }
            }
        };
        (p.notify)(event);
        if play_now {
            p.play(id);
        }
    }
}

/// Over budget: drop the least recently played decoded clips (not `keep`). A clip that's playing
/// keeps its memory until its voice ends (the voice holds a reference).
fn evict(c: &mut Cache, budget: usize, keep: u32) {
    while c.bytes > budget {
        let victim = c
            .entries
            .iter()
            .filter(|(id, e)| **id != keep && e.audio.is_some())
            .min_by_key(|(_, e)| e.last_used)
            .map(|(id, _)| *id);
        let Some(id) = victim else { break };
        if let Some(e) = c.entries.get_mut(&id) {
            if let Some(a) = e.audio.take() {
                c.bytes -= a.samples.bytes();
            }
        }
    }
}
