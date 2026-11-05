//! `JondisBoard.exe --bench <out.json>`: drives the real window through the interactions the
//! performance work is judged on and writes the timings as JSON, then closes. Used with a
//! throwaway library (bench/make_libraries.py); never with the real sounds.
//!
//! Each step changes something, then forces one synchronous repaint (RedrawWindow with
//! RDW_UPDATENOW) and times it. "build" is the CPU time to draw the frame; "frame" also includes
//! handing it to Windows (EndDraw waits for the next screen refresh, so up to ~16.7 ms of it is
//! just waiting for vsync).

use super::app::App;
use super::gfx::Gfx;
use crate::audio::mixer::{self, AtomicF32, ClipAudio, Cmd};
use crate::audio::wav::Samples;
use serde_json::{json, Map, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use super::gfx::Canvas;
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};

pub struct Bench {
    /// window sizes (px) to step through; done by the window with its state released
    pub resize_jobs: Vec<(i32, i32)>,
    pub resize_times: Vec<f64>,
    out: PathBuf,
    step: usize,
    started: Option<Instant>,
    results: Map<String, Value>,
}

fn stats(v: &[f64]) -> Value {
    if v.is_empty() {
        return Value::Null;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = s[s.len() / 2];
    let p95 = s[((s.len() as f64 * 0.95) as usize).min(s.len() - 1)];
    json!({"median": (med * 100.0).round() / 100.0, "p95": (p95 * 100.0).round() / 100.0, "max": (s[s.len() - 1] * 100.0).round() / 100.0, "n": s.len()})
}

/// Draw one frame now; returns (ms including handing it to Windows, ms to build it).
fn frame(app: &mut App, c: &mut Canvas, g: &mut Gfx) -> (f64, f64) {
    app.frame_times.clear();
    let t = Instant::now();
    app.paint(c, g);
    let total = t.elapsed().as_secs_f64() * 1000.0;
    let build = app.frame_times.first().copied().unwrap_or(0.0) as f64;
    (total, build)
}

fn memory() -> (f64, f64) {
    let mut m = PROCESS_MEMORY_COUNTERS_EX { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32, ..Default::default() };
    // SAFETY: valid struct and size for our own process.
    unsafe {
        let _ = GetProcessMemoryInfo(GetCurrentProcess(), &mut m as *mut _ as *mut _, m.cb);
    }
    (m.WorkingSetSize as f64 / 1048576.0, m.PrivateUsage as f64 / 1048576.0)
}

/// The mixer's cost per 10 ms block with 8 clips playing at once (the real render code).
pub fn mixer_cost() -> Value {
    let (mut m, mut c) = mixer::output(Arc::new(AtomicF32::new(0.8)), None);
    for id in 1..=8u32 {
        let data: Vec<i16> = (0..48000 * 10 * 2).map(|i| ((i as f32 * 0.01 * id as f32).sin() * 8000.0) as i16).collect();
        c.send(Cmd::Play(Arc::new(ClipAudio { id, samples: Samples::I16(data.into_boxed_slice()) })));
    }
    let mut out = vec![0.0f32; 480 * 2];
    let mut times = Vec::new();
    for _ in 0..2000 {
        let t = Instant::now();
        m.render(&mut out);
        times.push(t.elapsed().as_secs_f64() * 1e6);
    }
    let v = stats(&times);
    json!({"voices": 8, "block_frames": 480, "us": v})
}

impl Bench {
    pub fn new(out: PathBuf) -> Bench {
        Bench { resize_jobs: Vec::new(), resize_times: Vec::new(), out, step: 0, started: None, results: Map::new() }
    }

    pub fn start(&mut self, hwnd: HWND) {
        self.started = Some(Instant::now());
        super::window::set_timer(hwnd, super::window::TIMER_BENCH, 50);
    }

    pub fn after_frame(&mut self, _app: &mut App, _g: &mut Gfx) {}

    pub fn tick(&mut self, hwnd: HWND, app: &mut App, c: &mut Canvas, g: &mut Gfx) {
        let Some(t0) = self.started else { return };
        match self.step {
            0 => {
                // let the first frame, audio start and visible pictures settle
                if t0.elapsed() < Duration::from_millis(1500) {
                    return;
                }
                let timings: Map<String, Value> = app.timings.iter().map(|(k, v)| (k.to_string(), json!((v * 10.0).round() / 10.0))).collect();
                self.results.insert("startup".into(), Value::Object(timings));
                let (ws, private) = memory();
                self.results.insert("memory_at_start_mb".into(), json!({"working_set": ws.round(), "private": private.round()}));
                self.results.insert("sounds".into(), json!(app.clips.len()));
                self.results.insert("groups".into(), json!(app.groups.len()));
            }
            1 => {
                // group switches: every tab, twice
                let keys: Vec<String> = app.tabs().into_iter().map(|t| t.0).collect();
                let (mut fr, mut bu) = (Vec::new(), Vec::new());
                frame(app, c, g);
                for _ in 0..2 {
                    for k in keys.iter().chain(std::iter::once(&crate::config::ALL.to_string())) {
                        app.show_group(k);
                        let (f, b) = frame(app, c, g);
                        fr.push(f);
                        bu.push(b);
                    }
                }
                self.results.insert("group_switch_ms".into(), json!({"frame": stats(&fr), "build": stats(&bu)}));
            }
            2 => {
                // search: typing a word, then clearing it
                let (mut fr, mut bu) = (Vec::new(), Vec::new());
                app.search_focused = true;
                for ch in "bruh".chars() {
                    app.char_typed(ch);
                    let (f, b) = frame(app, c, g);
                    fr.push(f);
                    bu.push(b);
                }
                app.key_down(0x1B, false, false);
                let (f, b) = frame(app, c, g);
                app.search_focused = false;
                self.results.insert("search_ms".into(), json!({"frame": stats(&fr), "build": stats(&bu), "clear_frame": f, "clear_build": b}));
            }
            3 => {
                // scrolling the All view
                app.show_group(crate::config::ALL);
                let (mut fr, mut bu) = (Vec::new(), Vec::new());
                for i in 0..40 {
                    app.wheel(if i < 20 { -120.0 } else { 120.0 });
                    let (f, b) = frame(app, c, g);
                    fr.push(f);
                    bu.push(b);
                }
                self.results.insert("scroll_ms".into(), json!({"frame": stats(&fr), "build": stats(&bu)}));
            }
            4 => {
                // drag: press a tile, move across the board in 60 steps, drop where it started
                let tiles = app.layout.tiles.clone();
                let board = app.layout.board;
                let scroll = app.scroll.get(&app.layout.view).copied().unwrap_or(0.0);
                if let Some((id, r)) = tiles.first().copied() {
                    let (x0, y0) = (r.cx(), r.cy() + board.y - scroll);
                    app.down = Some(super::view::Hit::Tile(id));
                    app.down_pos = (x0, y0);
                    let (mut fr, mut bu) = (Vec::new(), Vec::new());
                        for i in 0..=60 {
                        let k = i as f32 / 60.0;
                        let x = x0 + (board.right() - 60.0 - x0) * k;
                        let y = y0 + (board.y + board.h * 0.6 - y0) * k;
                        app.mouse_move(g, x, y);
                        let (f, b) = frame(app, c, g);
                        fr.push(f);
                        bu.push(b);
                    }
                    app.cancel_drag();
                    let mut settle = 0;
                    while app.dirty && settle < 30 {
                        frame(app, c, g);
                        settle += 1;
                    }
                    let fps = 1000.0 / (fr.iter().sum::<f64>() / fr.len() as f64);
                    self.results.insert("drag_ms".into(), json!({"frame": stats(&fr), "build": stats(&bu), "fps": fps.round()}));
                }
            }
            5 => {
                // theme switches: every theme once, back to the first
                let names: Vec<&str> = crate::theme::all().iter().map(|t| t.name).collect();
                let start = app.theme().name;
                let (mut fr, mut bu) = (Vec::new(), Vec::new());
                for n in names.iter().filter(|n| **n != start).chain(std::iter::once(&start)) {
                    let t = Instant::now();
                    app.set_theme(n);
                    let (_, b) = frame(app, c, g);
                    fr.push(t.elapsed().as_secs_f64() * 1000.0);
                    bu.push(b);
                }
                self.results.insert("theme_switch_ms".into(), json!({"click_to_frame": stats(&fr), "build": stats(&bu)}));
            }
            6 => {
                // resizing: 10 steps wider, 10 back
                let (w, h) = (app.layout.w, app.layout.h);
                let s = app.layout_scale();
                for i in 0..20 {
                    let dw = if i < 10 { i * 30 } else { (20 - i) * 30 };
                    self.resize_jobs.push((((w + dw as f32) * s) as i32 + 16, (h * s) as i32 + 39));
                }
            }
            7 => {
                let fr = std::mem::take(&mut self.resize_times);
                self.results.insert("resize_step_ms".into(), stats(&fr));
            }
            8 => {
                self.results.insert("mixer_cost".into(), mixer_cost());
                // wait for the background decoder, then the memory it settled at
                let t = Instant::now();
                app.player.wait_idle(Duration::from_secs(60));
                self.results.insert("decode_all_ms".into(), json!((t.elapsed().as_secs_f64() * 1000.0).round()));
                self.results.insert("decode_thread_ms".into(), json!((app.player.decode_ns.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6).round()));
                self.results.insert("decoded_mb".into(), json!((app.player.cached_bytes() as f64 / 1048576.0).round()));
                let (ws, private) = memory();
                self.results.insert("memory_after_decode_mb".into(), json!({"working_set": ws.round(), "private": private.round()}));
            }
            _ => {
                super::window::kill_timer(hwnd, super::window::TIMER_BENCH);
                let text = serde_json::to_string_pretty(&Value::Object(std::mem::take(&mut self.results))).unwrap_or_default();
                let _ = std::fs::write(&self.out, text);
                // SAFETY: closing our own window.
                unsafe {
                    let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
                return;
            }
        }
        self.step += 1;
    }
}
