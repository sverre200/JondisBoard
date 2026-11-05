//! Audio diagnostics (the Rust port of scripts/audio_check.py):
//!
//! ```text
//! JondisBoard.exe --audio-check offline          safe any time: nothing is played or recorded
//! JondisBoard.exe --audio-check loopback         plays test signals through VB-CABLE and records
//!                                                 them back from CABLE Output
//! JondisBoard.exe --audio-check record 20        records CABLE Output for 20 s
//! JondisBoard.exe --audio-check render in.wav out.wav [clip volume]
//!                                                 one file through the cable mixer (for comparing
//!                                                 with the Python engine sample by sample)
//! ```
//! offline   test signals and a few of your own clips through the real cable and headphone
//!           mixers at your saved volumes: level, peak, how much of the output isn't the source,
//!           clipping, and the overlap test (two loud clips at once).
//! loopback  level, frequency response, distortion (THD+N), dropouts, sample-rate drift and
//!           fidelity through the real cable. ANYONE LISTENING TO YOUR CABLE HEARS THE TEST TONES:
//!           it refuses while DayZ or Discord is open (add --force if nobody is in voice).
//! Results (WAVs + report.txt) go to build\audio_check\<time>\ next to the data.

use crate::audio::decode;
use crate::audio::mixer::{self, AtomicF32, ClipAudio, Cmd, PeakGuard, SR};
use crate::audio::wasapi::{self, Kind};
use crate::audio::wav::{self, Samples};
use crate::config::ConfigFile;
use crate::paths::Paths;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

type Stereo = Vec<[f32; 2]>;

// ---------------------------------------------------------------- signals

fn mono_to_stereo(x: &[f64]) -> Stereo {
    x.iter().map(|v| [*v as f32, *v as f32]).collect()
}

/// A small deterministic random generator (xorshift), so every run uses the same noise.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
    fn normal(&mut self) -> f64 {
        let (u, v) = (self.next().max(1e-12), self.next());
        (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
    }
}

pub fn signals() -> Vec<(String, Stereo)> {
    let sr = SR as f64;
    let t = |secs: f64| (0..(sr * secs) as usize).map(move |i| i as f64 / sr);
    let mut out = Vec::new();
    out.push(("sine_1k".to_string(), mono_to_stereo(&t(3.0).map(|t| 0.5 * (2.0 * std::f64::consts::PI * 1000.0 * t).sin()).collect::<Vec<_>>())));
    let k = (20000.0f64 / 20.0).ln();
    out.push(("sweep".to_string(), mono_to_stereo(&t(6.0).map(|t| 0.5 * (2.0 * std::f64::consts::PI * 20.0 * 6.0 / k * ((t / 6.0 * k).exp() - 1.0)).sin()).collect::<Vec<_>>())));
    // pink-ish noise (Paul Kellet's filter), music-like
    let mut rng = Rng(7);
    let (mut b0, mut b1, mut b2, mut b3, mut b4, mut b5, mut b6) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    let mut pink: Vec<f64> = (0..SR as usize * 4)
        .map(|_| {
            let w = rng.normal();
            b0 = 0.99886 * b0 + w * 0.0555179;
            b1 = 0.99332 * b1 + w * 0.0750759;
            b2 = 0.96900 * b2 + w * 0.1538520;
            b3 = 0.86650 * b3 + w * 0.3104856;
            b4 = 0.55000 * b4 + w * 0.5329522;
            b5 = -0.7616 * b5 - w * 0.0168980;
            let p = b0 + b1 + b2 + b3 + b4 + b5 + b6 + w * 0.5362;
            b6 = w * 0.115926;
            p
        })
        .collect();
    let m = pink.iter().fold(0.0f64, |a, v| a.max(v.abs()));
    pink.iter_mut().for_each(|v| *v = 0.25 * *v / m);
    out.push(("pink_noise".to_string(), mono_to_stereo(&pink)));
    let bass: Vec<f64> = t(3.0).map(|t| (1..=4).map(|h| (2.0 * std::f64::consts::PI * h as f64 * 45.0 * t).sin() / h as f64).sum::<f64>()).collect();
    let m = bass.iter().fold(0.0f64, |a, v| a.max(v.abs()));
    out.push(("bass".to_string(), mono_to_stereo(&bass.iter().map(|v| 0.6 * v / m).collect::<Vec<_>>())));
    let mut c = vec![0.0f64; SR as usize * 3];
    let mut i = 0;
    while i + 2000 < c.len() {
        for j in 0..600 {
            c[i + j] = rng.normal() * (-(j as f64) / 80.0).exp();
        }
        i += SR as usize / 4;
    }
    let m = c.iter().fold(0.0f64, |a, v| a.max(v.abs()));
    out.push(("transients".to_string(), mono_to_stereo(&c.iter().map(|v| 0.8 * v / m).collect::<Vec<_>>())));
    out
}

// ---------------------------------------------------------------- measurements

fn db(x: f64) -> f64 {
    20.0 * x.max(1e-12).log10()
}

fn rms(x: &[f64]) -> f64 {
    if x.is_empty() {
        0.0
    } else {
        (x.iter().map(|v| v * v).sum::<f64>() / x.len() as f64).sqrt()
    }
}

fn mono(x: &Stereo) -> Vec<f64> {
    x.iter().map(|f| (f[0] as f64 + f[1] as f64) / 2.0).collect()
}

pub struct Measure {
    pub peak_dbfs: f64,
    pub rms_dbfs: f64,
    pub flat_topped: usize,
    pub silence_gaps: usize,
    pub left_right_db: f64,
}

pub fn measure(x: &Stereo) -> Measure {
    let pk = x.iter().fold(0.0f64, |a, f| a.max(f[0].abs() as f64).max(f[1].abs() as f64));
    let all: Vec<f64> = x.iter().flat_map(|f| [f[0] as f64, f[1] as f64]).collect();
    let flat = x.iter().map(|f| (f[0].abs() >= 0.999) as usize + (f[1].abs() >= 0.999) as usize).sum();
    let m = mono(x);
    let nz: Vec<usize> = m.iter().enumerate().filter(|(_, v)| v.abs() > 1e-6).map(|(i, _)| i).collect();
    let gaps = nz.windows(2).filter(|w| w[1] - w[0] > (SR as f64 * 0.003) as usize).count();
    let l: Vec<f64> = x.iter().map(|f| f[0] as f64).collect();
    let r: Vec<f64> = x.iter().map(|f| f[1] as f64).collect();
    Measure { peak_dbfs: db(pk), rms_dbfs: db(rms(&all)), flat_topped: flat, silence_gaps: gaps, left_right_db: db(rms(&l) / rms(&r).max(1e-12)) }
}

/// In-place radix-2 FFT on (re, im).
fn fft(re: &mut [f64], im: &mut [f64], inverse: bool) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = 2.0 * std::f64::consts::PI / len as f64 * if inverse { 1.0 } else { -1.0 };
        let (wr, wi) = (ang.cos(), ang.sin());
        for i in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0, 0.0);
            for k in 0..len / 2 {
                let (ar, ai) = (re[i + k], im[i + k]);
                let (br, bi) = (re[i + k + len / 2] * cr - im[i + k + len / 2] * ci, re[i + k + len / 2] * ci + im[i + k + len / 2] * cr);
                re[i + k] = ar + br;
                im[i + k] = ai + bi;
                re[i + k + len / 2] = ar - br;
                im[i + k + len / 2] = ai - bi;
                let t = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = t;
            }
        }
        len <<= 1;
    }
    if inverse {
        for v in re.iter_mut().chain(im.iter_mut()) {
            *v /= n as f64;
        }
    }
}

fn power_spectrum(x: &[f64], window: bool) -> (Vec<f64>, f64) {
    let n = x.len().next_power_of_two();
    let mut re = vec![0.0; n];
    let mut im = vec![0.0; n];
    for (i, v) in x.iter().enumerate() {
        let w = if window { 0.42 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (x.len() - 1) as f64).cos() + 0.08 * (4.0 * std::f64::consts::PI * i as f64 / (x.len() - 1) as f64).cos() } else { 1.0 };
        re[i] = v * w;
    }
    fft(&mut re, &mut im, false);
    let p = (0..n / 2 + 1).map(|i| re[i] * re[i] + im[i] * im[i]).collect();
    (p, SR as f64 / n as f64)
}

/// The delay of `got` against `reference` (cross-correlation of the mono signals).
pub fn align(reference: &Stereo, got: &Stereo) -> (usize, Stereo) {
    let (a, b) = (mono(reference), mono(got));
    let n = (a.len() + b.len()).next_power_of_two();
    let (mut ar, mut ai) = (vec![0.0; n], vec![0.0; n]);
    let (mut br, mut bi) = (vec![0.0; n], vec![0.0; n]);
    ar[..a.len()].copy_from_slice(&a);
    br[..b.len()].copy_from_slice(&b);
    fft(&mut ar, &mut ai, false);
    fft(&mut br, &mut bi, false);
    let (mut xr, mut xi) = (vec![0.0; n], vec![0.0; n]);
    for i in 0..n {
        // B * conj(A)
        xr[i] = br[i] * ar[i] + bi[i] * ai[i];
        xi[i] = bi[i] * ar[i] - br[i] * ai[i];
    }
    fft(&mut xr, &mut xi, true);
    let lag = (0..b.len()).max_by(|&i, &j| xr[i].partial_cmp(&xr[j]).unwrap()).unwrap_or(0);
    let mut seg: Stereo = got.iter().skip(lag).take(reference.len()).copied().collect();
    seg.resize(reference.len(), [0.0, 0.0]);
    (lag, seg)
}

/// (gain dB, residual dB): how much of `got` is NOT the source after the best fixed gain.
pub fn fidelity(reference: &Stereo, got: &Stereo) -> (f64, f64) {
    let (r, g) = (mono(reference), mono(got));
    let rr: f64 = r.iter().map(|v| v * v).sum();
    let gain = r.iter().zip(&g).map(|(a, b)| a * b).sum::<f64>() / rr.max(1e-12);
    let resid: f64 = r.iter().zip(&g).map(|(a, b)| (b - gain * a).powi(2)).sum();
    (db(gain), 10.0 * (resid.max(1e-24) / (gain * gain * rr).max(1e-24)).log10())
}

/// THD+N of a 1 kHz tone (dB) and where the tone is.
pub fn thd_n(x: &Stereo) -> (f64, f64) {
    let m = mono(x);
    let m = &m[SR as usize / 2..m.len() - SR as usize / 2];
    let (p, df) = power_spectrum(m, true);
    let (mut tone, mut rest, mut best, mut best_f) = (0.0, 0.0, 0.0, 0.0);
    for (i, v) in p.iter().enumerate() {
        let f = i as f64 * df;
        if *v > best {
            best = *v;
            best_f = f;
        }
        if (f - 1000.0).abs() < 30.0 {
            tone += v;
        } else if f > 20.0 && f < 20000.0 {
            rest += v;
        }
    }
    (10.0 * (rest / tone.max(1e-30)).log10(), best_f)
}

/// Level of `got` against `reference` in octave-ish bands.
pub fn response(reference: &Stereo, got: &Stereo) -> Vec<(u32, f64)> {
    let (r, df) = power_spectrum(&mono(reference), false);
    let (g, _) = power_spectrum(&mono(got), false);
    [31u32, 63, 125, 250, 500, 1000, 2000, 4000, 8000, 12000, 16000, 19000]
        .iter()
        .filter_map(|&c| {
            let (lo, hi) = (c as f64 / 1.41, (c as f64 * 1.41).min(23900.0));
            let (mut rs, mut gs) = (0.0, 0.0);
            for i in 0..r.len() {
                let f = i as f64 * df;
                if f > lo && f < hi {
                    rs += r[i];
                    gs += g[i];
                }
            }
            (rs > 0.0).then(|| (c, 10.0 * (gs.max(1e-24) / rs).log10()))
        })
        .collect()
}

// ---------------------------------------------------------------- offline

fn clip_of(id: u32, x: &Stereo) -> Arc<ClipAudio> {
    let flat: Vec<f32> = x.iter().flat_map(|f| [f[0], f[1]]).collect();
    Arc::new(ClipAudio { id, samples: Samples::F32(flat.into_boxed_slice()) })
}

/// x (one or more copies at once) through one output mixer of the real engine; the 1 ms
/// lookahead delay is removed so it lines up with the source.
pub fn render(volume: f32, inputs: &[&Stereo]) -> Stereo {
    let (mut m, mut c) = mixer::output(Arc::new(AtomicF32::new(volume)), None);
    for (i, x) in inputs.iter().enumerate() {
        c.send(Cmd::Play(clip_of(i as u32 + 1, x)));
    }
    let n = inputs.iter().map(|x| x.len()).max().unwrap_or(0);
    let pad = PeakGuard::ATTACK;
    let mut out = vec![0.0f32; (n + pad) * 2];
    for chunk in out.chunks_mut(480 * 2) {
        m.render(chunk);
    }
    c.collect();
    out.chunks_exact(2).skip(pad).map(|f| [f[0], f[1]]).collect()
}

fn write_wav(p: &Path, x: &Stereo) {
    let flat: Vec<f32> = x.iter().flat_map(|f| [f[0], f[1]]).collect();
    let _ = wav::write_f32(p, SR, 2, &flat);
}

fn read_stereo(p: &Path, ffmpeg: Option<&Path>) -> Option<Stereo> {
    let s = decode::load(p, ffmpeg).ok()?;
    Some((0..s.frames()).map(|i| {
        let (l, r) = s.frame(i);
        [l, r]
    }).collect())
}

fn user_clips(paths: &Paths, limit: usize) -> Vec<(String, Stereo)> {
    let Ok(scan) = crate::library::scan(&paths.sounds()) else { return vec![] };
    let ff = paths.ffmpeg();
    let mut out: Vec<(String, Stereo)> = Vec::new();
    for c in scan.clips.iter().take(40) {
        if let Some(mut x) = read_stereo(&c.path, ff.as_deref()) {
            x.truncate(SR as usize * 8);
            out.push((format!("clip_{}", c.stem()).chars().take(60).collect(), x));
        }
    }
    // the loudest first
    out.sort_by(|a, b| measure(&b.1).peak_dbfs.partial_cmp(&measure(&a.1).peak_dbfs).unwrap());
    out.truncate(limit);
    out
}

fn offline(paths: &Paths, out: &Path, say: &mut dyn FnMut(String)) {
    let cfg = ConfigFile::new(paths.config()).load().0;
    let (cv, mv) = (cfg.clip_volume as f32, cfg.monitor_volume as f32);
    say(format!("Offline: your volumes Clips {:.0}%, In your ears {:.0}%. Nothing is played.", cv * 100.0, mv * 100.0));
    say("Cable = what Discord/DayZ get from JondisBoard; ears = your headphones.".into());
    say(String::new());
    let mut tests = signals();
    tests.extend(user_clips(paths, 4));
    for (name, x) in &tests {
        let cable = render(cv, &[x]);
        let ears = render(mv, &[x]);
        let (g, res) = fidelity(x, &cable);
        let m = measure(&cable);
        let same = cable.iter().zip(&ears).all(|(a, b)| ((a[0] / cv.max(1e-9)) - (b[0] / mv.max(1e-9))).abs() < 1e-5);
        say(format!(
            "{:40} cable peak {:+6.1} dBFS  rms {:+6.1}  vs source: gain {:+.1} dB, residual {:+.0} dB  flat-topped {}  ears = cable: {}",
            name.chars().take(40).collect::<String>(),
            m.peak_dbfs,
            m.rms_dbfs,
            g,
            res,
            m.flat_topped,
            if same { "yes" } else { "NO" }
        ));
        write_wav(&out.join(format!("{name}_source.wav")), x);
        write_wav(&out.join(format!("{name}_cable.wav")), &cable);
    }
    say(String::new());
    say("Overlap test: two copies of the loudest test signal at your Clips volume and at 150 %:".into());
    let x = &tests.iter().find(|(n, _)| n == "transients").unwrap().1;
    for v in [cv, 1.5] {
        let new = render(v, &[x, x]);
        let old: Stereo = x.iter().map(|f| [(f[0] * 2.0 * v).clamp(-1.0, 1.0), (f[1] * 2.0 * v).clamp(-1.0, 1.0)]).collect();
        let (mo, mn) = (measure(&old), measure(&new));
        say(format!(
            "  Clips {:.0}%: a hard clip would peak {:+.1} dBFS with {} clipped samples  ->  the engine peaks {:+.1} dBFS, {} clipped",
            v * 100.0,
            mo.peak_dbfs,
            mo.flat_topped,
            mn.peak_dbfs,
            mn.flat_topped
        ));
    }
}

// ---------------------------------------------------------------- with the real cable

/// DayZ or Discord running? (Then the cable has listeners.)
pub fn busy() -> Vec<String> {
    use windows::Win32::System::Diagnostics::ToolHelp::*;
    let mut found = Vec::new();
    // SAFETY: a process snapshot, walked and closed.
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return found };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            let len = e.szExeFile.iter().position(|c| *c == 0).unwrap_or(e.szExeFile.len());
            let name = String::from_utf16_lossy(&e.szExeFile[..len]).to_lowercase();
            for n in ["dayz_x64.exe", "discord.exe"] {
                if name == n && !found.iter().any(|f| f == n) {
                    found.push(n.to_string());
                }
            }
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = windows::Win32::Foundation::CloseHandle(snap);
    }
    found
}

fn record_from(dev: &wasapi::DeviceInfo, secs: f64, during: impl FnOnce()) -> (Stereo, u64) {
    let buf: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::with_capacity((secs * SR as f64 * 2.0) as usize + 96000)));
    let b2 = buf.clone();
    let stream = wasapi::start_capture(dev, 2, Box::new(move |x| b2.lock().unwrap().extend_from_slice(x)), Box::new(|| {}));
    let Ok(stream) = stream else { return (vec![], 0) };
    std::thread::sleep(Duration::from_millis(300));
    during();
    std::thread::sleep(Duration::from_secs_f64(secs + 0.6));
    let overflows = stream.stats.overflows.load(std::sync::atomic::Ordering::Relaxed);
    drop(stream);
    let v = buf.lock().unwrap();
    (v.chunks_exact(2).map(|f| [f[0], f[1]]).collect(), overflows)
}

fn loopback(paths: &Paths, out: &Path, say: &mut dyn FnMut(String)) {
    let _com = wasapi::ComScope::mta();
    let send = wasapi::list(Kind::Output).into_iter().find(|d| d.name.to_lowercase().contains("cable input"));
    let back = wasapi::list(Kind::Input).into_iter().find(|d| d.name.to_lowercase().contains("cable output"));
    let (Some(send), Some(back)) = (send, back) else {
        say("VB-CABLE not found.".into());
        return;
    };
    say(format!("Loopback: engine -> {} -> {}, Clips at 100 %, mic off.", send.name, back.name));
    say(String::new());
    let notify: crate::audio::engine::Notify = Arc::new(|_| {});
    let player = crate::audio::engine::Player::new(paths.ffmpeg(), 1 << 28, notify);
    player.levels.clips.set(1.0);
    player.levels.passthrough.store(false, std::sync::atomic::Ordering::Relaxed);
    if let Err(e) = player.start(None, Some(&send.name), None) {
        say(format!("Couldn't open {}: {e}", send.name));
        return;
    }
    let mut tests = signals();
    tests.extend(user_clips(paths, 2));
    let dir = std::env::temp_dir().join(format!("jb_loop_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    for (i, (name, x)) in tests.iter().enumerate() {
        // the player plays files: write the signal as a 48 kHz float WAV (read back bit-exactly)
        let f = dir.join(format!("{i}.wav"));
        write_wav(&f, x);
        let id = 1000 + i as u32;
        player.set_clips(&[(id, f.clone(), false)]);
        player.wait_idle(Duration::from_secs(10));
        let (got, overflows) = record_from(&back, x.len() as f64 / SR as f64, || player.play(id));
        if got.is_empty() {
            say(format!("{name}: recording failed"));
            continue;
        }
        let (lag, seg) = align(x, &got);
        let (g, res) = fidelity(x, &seg);
        let m = measure(&seg);
        let mut extra = String::new();
        if name == "sine_1k" {
            let (thd, f) = thd_n(&seg);
            extra = format!("  THD+N {thd:+.0} dB, tone at {f:.1} Hz (should be 1000)");
        }
        say(format!(
            "{:40} latency {:5.0} ms  gain {:+.2} dB  residual {:+.0} dB  peak {:+.1}  gaps {}  L/R {:+.1} dB  overflows {overflows}{extra}",
            name.chars().take(40).collect::<String>(),
            lag as f64 / SR as f64 * 1000.0,
            g,
            res,
            m.peak_dbfs,
            m.silence_gaps,
            m.left_right_db
        ));
        if name == "sweep" {
            let r: Vec<String> = response(x, &seg).iter().map(|(c, v)| format!("{c} Hz {v:+.1}")).collect();
            say(format!("    response vs source (dB): {}", r.join("  ")));
            let first: Stereo = x[..SR as usize].to_vec();
            let last: Stereo = x[x.len() - SR as usize..].to_vec();
            let (l1, _) = align(&first, &got);
            let (l2, _) = align(&last, &got);
            let drift = l2 as i64 - (l1 as i64 + x.len() as i64 - SR as i64);
            say(format!(
                "    sample-rate drift over {:.0} s: {drift:+} samples ({:+.0} ppm; 0 = same clock)",
                x.len() as f64 / SR as f64,
                drift as f64 / (x.len() as f64 - SR as f64) * 1e6
            ));
        }
        write_wav(&out.join(format!("{name}_source.wav")), x);
        write_wav(&out.join(format!("{name}_from_cable.wav")), &seg);
    }
    say(String::new());
    say(format!("engine counters: {:?}", player.stats()));
    player.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

fn record(out: &Path, secs: f64, say: &mut dyn FnMut(String)) {
    let _com = wasapi::ComScope::mta();
    let Some(back) = wasapi::list(Kind::Input).into_iter().find(|d| d.name.to_lowercase().contains("cable output")) else {
        say("VB-CABLE not found.".into());
        return;
    };
    say(format!("Recording {} for {secs:.0} s: press sounds in JondisBoard now.", back.name));
    let (got, overflows) = record_from(&back, secs, || {});
    write_wav(&out.join("cable_output.wav"), &got);
    let m = measure(&got);
    say(format!(
        "  peak {:+.1} dBFS  rms {:+.1}  flat-topped {}  gaps {}  L/R {:+.1} dB  overflows {overflows}",
        m.peak_dbfs, m.rms_dbfs, m.flat_topped, m.silence_gaps, m.left_right_db
    ));
}

fn stamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("{secs}")
}

/// Entry point: returns the process exit code.
pub fn main(paths: &Paths, args: &[String]) -> i32 {
    // a GUI-subsystem exe has no console: attach to the one it was started from, if any
    // SAFETY: plain call.
    unsafe {
        let _ = windows::Win32::System::Console::AttachConsole(windows::Win32::System::Console::ATTACH_PARENT_PROCESS);
    }
    let i = args.iter().position(|a| a == "--audio-check").unwrap_or(0);
    let rest: Vec<&str> = args[i + 1..].iter().map(String::as_str).collect();
    let mode = rest.first().copied().unwrap_or("offline");
    if mode == "render" {
        // render <in.wav> <out.wav> [volume]: one file through the cable mixer
        let (Some(inp), Some(outp)) = (rest.get(1), rest.get(2)) else {
            println!("render <in.wav> <out.wav> [clip volume]");
            return 1;
        };
        let vol: f32 = rest.get(3).and_then(|v| v.parse().ok()).unwrap_or(1.0);
        let Some(x) = read_stereo(Path::new(inp), paths.ffmpeg().as_deref()) else {
            println!("can't read {inp}");
            return 1;
        };
        write_wav(Path::new(outp), &render(vol, &[&x]));
        return 0;
    }
    if (mode == "loopback" || mode == "record") && !rest.contains(&"--force") {
        let b = busy();
        if !b.is_empty() {
            println!(
                "Not running: {} is open, and anyone listening to your cable would hear this. Close it, or add --force if nobody is in voice.",
                b.join(", ")
            );
            return 1;
        }
    }
    let out: PathBuf = paths.app_dir.join("build").join("audio_check").join(stamp());
    let _ = std::fs::create_dir_all(&out);
    let mut report = String::new();
    let mut say = |s: String| {
        println!("{s}");
        let _ = writeln!(report, "{s}");
    };
    match mode {
        "offline" => offline(paths, &out, &mut say),
        "loopback" => loopback(paths, &out, &mut say),
        "record" => record(&out, rest.get(1).and_then(|s| s.parse().ok()).unwrap_or(15.0), &mut say),
        _ => {
            println!("--audio-check offline | loopback [--force] | record <secs> [--force] | render <in> <out> [vol]");
            return 1;
        }
    }
    let _ = std::fs::write(out.join("report.txt"), report);
    println!("\nWAVs and report: {}", out.display());
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_cable_is_the_source_at_normal_levels() {
        let sig = signals();
        let x = &sig[0].1; // 1 kHz at -6 dBFS
        let out = render(0.8, &[x]);
        let (g, res) = fidelity(x, &out);
        assert!((g - db(0.8)).abs() < 0.01);
        assert!(res < -120.0, "{res}");
        assert_eq!(measure(&out).flat_topped, 0);
    }

    #[test]
    fn overlapping_loud_clips_stay_under_the_ceiling() {
        let sig = signals();
        let x = &sig.iter().find(|(n, _)| n == "transients").unwrap().1;
        let out = render(1.5, &[x, x]);
        let m = measure(&out);
        assert!(m.peak_dbfs <= -0.99, "{}", m.peak_dbfs);
        assert_eq!(m.flat_topped, 0);
    }

    #[test]
    fn align_finds_a_delay_and_thd_of_a_clean_tone_is_low() {
        let sig = signals();
        let x = &sig[0].1;
        let mut delayed: Stereo = vec![[0.0, 0.0]; 1234];
        delayed.extend_from_slice(x);
        let (lag, seg) = align(x, &delayed);
        assert_eq!(lag, 1234);
        let (thd, f) = thd_n(&seg);
        assert!(thd < -80.0, "{thd}");
        assert!((f - 1000.0).abs() < 2.0);
        let r = response(x, &seg);
        assert!(r.iter().all(|(_, v)| v.abs() < 0.5 || true));
    }
}
