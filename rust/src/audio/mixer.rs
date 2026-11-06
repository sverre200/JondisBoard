//! The real-time mixing core, shared by the WASAPI threads and the offline diagnostics/tests:
//!
//! ```text
//! cable   (VB-CABLE "CABLE Input", what Discord / the game hear)
//!         = clips x clip_volume + mic x mic_volume (only with Send my voice)
//! monitor (your headphones) = clips x monitor_volume
//! ```
//! each through its own [`PeakGuard`]. Float32, 48 kHz, interleaved stereo. Nothing in
//! `OutputMixer::render` allocates, locks, or does I/O: commands arrive through an SPSC queue,
//! finished clips leave through another one (so their memory is freed off the audio thread), and
//! progress is published through atomics.

use super::spsc::{self, Consumer, Producer};
use super::wav::Samples;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

pub const SR: u32 = 48_000;
pub const MAX_VOICES: usize = 64;
/// The biggest block processed in one go; bigger requests are processed in pieces.
pub const MAX_BLOCK: usize = 4096;

/// Decoded clip audio, shared between the cache and the voices playing it.
pub struct ClipAudio {
    pub id: u32,
    pub samples: Samples,
}

impl ClipAudio {
    pub fn frames(&self) -> usize {
        self.samples.frames()
    }
}

/// An f32 that can be shared between threads (stored as bits).
#[derive(Default)]
pub struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub fn new(v: f32) -> Self {
        AtomicF32(AtomicU32::new(v.to_bits()))
    }
    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
    pub fn set(&self, v: f32) {
        self.0.store(v.to_bits(), Ordering::Relaxed)
    }
}

/// Keeps a mix under CEILING without hard clipping, and passes it through untouched (bit-exact,
/// 1 ms late) while it stays under, which a single normalised clip at normal volume always does.
///
/// A 1 ms lookahead limiter: the output runs ATTACK frames behind the input, so when a block
/// would go over (clips overlapping, a slider above 100 %, mic + clips) the gain has already
/// glided down before the loud sample comes out; afterwards it glides back up to 1 over about
/// RELEASE seconds. The gain only moves in smooth ramps, never per sample.
pub struct PeakGuard {
    pub gain: f32,
    step: f32,
    pub limited_blocks: u64,
    pub clipped_samples: u64,
    /// last block's tail (ATTACK frames) followed by this block, interleaved stereo
    work: Box<[f32]>,
    ramp: Box<[f32]>,
}

impl PeakGuard {
    /// -1 dBFS: headroom for the voice codec's own reconstruction peaks
    pub const CEILING: f32 = 0.891_250_9; // 10^(-1/20)
    /// 1 ms lookahead and gain glide
    pub const ATTACK: usize = 48;
    /// seconds for the gain to climb a full 1.0
    pub const RELEASE: f32 = 0.3;

    pub fn new() -> Self {
        PeakGuard {
            gain: 1.0,
            step: 1.0 / (Self::RELEASE * SR as f32),
            limited_blocks: 0,
            clipped_samples: 0,
            work: vec![0.0; (Self::ATTACK + MAX_BLOCK) * 2].into_boxed_slice(),
            ramp: vec![0.0; MAX_BLOCK].into_boxed_slice(),
        }
    }

    pub fn reset(&mut self) {
        self.gain = 1.0;
        self.work[..Self::ATTACK * 2].fill(0.0);
    }

    /// buf: interleaved stereo, replaced in place by the limited, 1 ms delayed signal.
    pub fn process(&mut self, buf: &mut [f32]) {
        for chunk in buf.chunks_mut(MAX_BLOCK * 2) {
            self.process_block(chunk);
        }
    }

    fn process_block(&mut self, buf: &mut [f32]) {
        let frames = buf.len() / 2;
        if frames == 0 {
            return;
        }
        let a = Self::ATTACK;
        let x = &mut self.work[..(a + frames) * 2];
        x[a * 2..].copy_from_slice(buf);
        // everything still to come out (incl. the tail) must fit under the gain at the block's end
        let mut hi = f32::MIN;
        let mut lo = f32::MAX;
        for &v in x.iter() {
            hi = hi.max(v);
            lo = lo.min(v);
        }
        let peak = hi.max(-lo);
        let need = if peak > Self::CEILING { Self::CEILING / peak } else { 1.0 };
        let g0 = self.gain;
        let target = if need < g0 { need } else { 1.0f32.min(need).min(g0 + self.step * frames as f32) };
        buf.copy_from_slice(&x[..frames * 2]);
        if g0 != 1.0 || target != 1.0 {
            self.limited_blocks += 1;
            let ramp = &mut self.ramp[..frames];
            if target < g0 {
                // attack: down within 1 ms, then hold
                let n = a.min(frames);
                for (i, r) in ramp[..n].iter_mut().enumerate() {
                    *r = (g0 as f64 + (target as f64 - g0 as f64) * i as f64 / n as f64) as f32;
                }
                ramp[n..].fill(target);
            } else {
                // release: a slow climb
                for (i, r) in ramp.iter_mut().enumerate() {
                    *r = (g0 as f64 + (target as f64 - g0 as f64) * i as f64 / frames as f64) as f32;
                }
            }
            let mut over = false;
            for (f, r) in buf.chunks_exact_mut(2).zip(ramp.iter()) {
                f[0] *= r;
                f[1] *= r;
                over |= f[0].abs() > 1.0 || f[1].abs() > 1.0;
            }
            self.gain = target;
            if over {
                // safety net only
                for v in buf.iter_mut() {
                    if v.abs() > 1.0 {
                        self.clipped_samples += 1;
                        *v = v.clamp(-1.0, 1.0);
                    }
                }
            }
        }
        // keep the new tail
        x.copy_within(frames * 2..(frames + a) * 2, 0);
    }
}

impl Default for PeakGuard {
    fn default() -> Self {
        Self::new()
    }
}

/// Hands mic samples from the capture thread to the cable thread: a lock-free ring buffer (the
/// indices only ever grow; samples are f32 bits in relaxed atomics, so it needs no `unsafe`)
/// with a small jitter cushion.
///
/// The two streams run on different device clocks and block sizes, so the cable side sometimes
/// asks before the mic side has delivered. It waits until it holds a cushion (the biggest mic
/// block + the biggest read seen, plus 5 ms) before playing, fades out and back in at a real gap,
/// and when clock drift piles up delay past `max_frames` it drops back to the cushion once (one
/// tiny skip instead of steady crackle).
pub struct MicRing {
    buf: Box<[AtomicU32]>,
    mask: u64,
    written: AtomicU64,
    big_write: AtomicU64,
    pub overflows: AtomicU64,
}

pub struct MicReader {
    ring: Arc<MicRing>,
    read: u64,
    primed: bool,
    big_read: u64,
    max_frames: u64,
    pub underruns: u64,
    pub drops: u64,
    fade_in: [f32; MicReader::FADE],
}

impl MicRing {
    /// A ring of at least `capacity` mono samples.
    pub fn new(capacity: usize) -> Arc<MicRing> {
        let cap = capacity.next_power_of_two();
        Arc::new(MicRing {
            buf: (0..cap).map(|_| AtomicU32::new(0)).collect(),
            mask: cap as u64 - 1,
            written: AtomicU64::new(0),
            big_write: AtomicU64::new(0),
            overflows: AtomicU64::new(0),
        })
    }

    /// Capture side: append mono samples.
    pub fn write(&self, x: &[f32]) {
        let n = x.len() as u64;
        if n > self.big_write.load(Ordering::Relaxed) {
            self.big_write.store(n, Ordering::Relaxed);
        }
        let w = self.written.load(Ordering::Relaxed);
        for (i, &v) in x.iter().enumerate() {
            self.buf[((w + i as u64) & self.mask) as usize].store(v.to_bits(), Ordering::Relaxed);
        }
        self.written.store(w + n, Ordering::Release);
    }
}

impl MicReader {
    const FADE: usize = 96; // 2 ms

    pub fn new(ring: Arc<MicRing>, max_frames: u64) -> MicReader {
        let mut fade_in = [0.0; Self::FADE];
        for (i, f) in fade_in.iter_mut().enumerate() {
            *f = i as f32 / (Self::FADE - 1) as f32;
        }
        let read = ring.written.load(Ordering::Acquire);
        MicReader { ring, read, primed: false, big_read: 0, max_frames, underruns: 0, drops: 0, fade_in }
    }

    pub fn cushion(&self) -> u64 {
        self.max_frames.min(self.ring.big_write.load(Ordering::Relaxed) + self.big_read + (SR as u64 * 5 / 1000))
    }

    pub fn available(&self) -> u64 {
        self.ring.written.load(Ordering::Acquire) - self.read
    }

    /// Fill `out` (mono) with mic samples, or silence while (re)building the cushion.
    pub fn read_into(&mut self, out: &mut [f32]) {
        let n = out.len() as u64;
        self.big_read = self.big_read.max(n);
        let cap = self.ring.mask + 1;
        let mut count = self.available();
        if count > cap {
            // the writer lapped us (we weren't reading at all): start over from the newest samples
            self.read = self.ring.written.load(Ordering::Acquire) - self.cushion();
            count = self.cushion();
            self.ring.overflows.fetch_add(1, Ordering::Relaxed);
        }
        if count > self.max_frames {
            // drift built up delay: skip back to the cushion once
            let drop = count - self.cushion();
            self.read += drop;
            count -= drop;
            self.drops += 1;
        }
        let mut fade_in = false;
        if !self.primed {
            if count < self.cushion() {
                out.fill(0.0);
                return;
            }
            self.primed = true;
            fade_in = true;
        }
        let take = n.min(count) as usize;
        for (i, o) in out[..take].iter_mut().enumerate() {
            *o = f32::from_bits(self.ring.buf[((self.read + i as u64) & self.ring.mask) as usize].load(Ordering::Relaxed));
        }
        self.read += take as u64;
        if take < out.len() {
            // ran dry: fade out, wait for the cushion again
            self.underruns += 1;
            self.primed = false;
            let f = Self::FADE.min(take);
            for i in 0..f {
                // fade_out[FADE - f + i] == fade_in[f - 1 - i]
                out[take - f + i] *= self.fade_in[f - 1 - i];
            }
            out[take..].fill(0.0);
        }
        if fade_in {
            let f = Self::FADE.min(out.len());
            for i in 0..f {
                out[i] *= self.fade_in[i];
            }
        }
    }
}

/// A command for an output's mixer.
pub enum Cmd {
    /// start (or restart) a clip
    Play(Arc<ClipAudio>),
    Stop(u32),
    StopAll,
    /// a playing clip starts over by itself at its end (or stops doing so)
    Loop(u32, bool),
}

struct Voice {
    clip: Arc<ClipAudio>,
    pos: usize,
    looped: bool,
}

/// What a mixer publishes for the UI: which clips play and how far along they are.
pub struct PlayState {
    ids: [AtomicU32; MAX_VOICES],
    pos: [AtomicU32; MAX_VOICES],
    len: [AtomicU32; MAX_VOICES],
    pub count: AtomicU32,
    pub underflows: AtomicU64,
    pub limited_blocks: AtomicU64,
    pub clipped_samples: AtomicU64,
    pub blocks: AtomicU64,
    /// total nanoseconds spent in render (for the cost measurement)
    pub render_ns: AtomicU64,
    pub max_render_ns: AtomicU64,
    pub mic_gaps: AtomicU64,
    pub mic_skips: AtomicU64,
}

impl PlayState {
    pub fn new() -> Arc<PlayState> {
        Arc::new(PlayState {
            ids: std::array::from_fn(|_| AtomicU32::new(0)),
            pos: std::array::from_fn(|_| AtomicU32::new(0)),
            len: std::array::from_fn(|_| AtomicU32::new(0)),
            count: AtomicU32::new(0),
            underflows: AtomicU64::new(0),
            limited_blocks: AtomicU64::new(0),
            clipped_samples: AtomicU64::new(0),
            blocks: AtomicU64::new(0),
            render_ns: AtomicU64::new(0),
            max_render_ns: AtomicU64::new(0),
            mic_gaps: AtomicU64::new(0),
            mic_skips: AtomicU64::new(0),
        })
    }

    /// (clip id, fraction played) for each playing clip.
    pub fn snapshot(&self, out: &mut Vec<(u32, f32)>) {
        out.clear();
        let n = (self.count.load(Ordering::Acquire) as usize).min(MAX_VOICES);
        for i in 0..n {
            let id = self.ids[i].load(Ordering::Relaxed);
            let len = self.len[i].load(Ordering::Relaxed).max(1);
            if id != 0 {
                out.push((id, (self.pos[i].load(Ordering::Relaxed) as f32 / len as f32).min(1.0)));
            }
        }
    }

    pub fn is_playing(&self, id: u32) -> bool {
        let n = (self.count.load(Ordering::Acquire) as usize).min(MAX_VOICES);
        (0..n).any(|i| self.ids[i].load(Ordering::Relaxed) == id)
    }
}

/// One output's mixer (owned by its audio thread).
pub struct OutputMixer {
    voices: Vec<Voice>,
    cmds: Consumer<Cmd>,
    garbage: Producer<Arc<ClipAudio>>,
    pub guard: PeakGuard,
    pub volume: Arc<AtomicF32>,
    pub state: Arc<PlayState>,
    /// cable only: the mic and its controls
    pub mic: Option<MicInput>,
    mic_tmp: Box<[f32]>,
}

pub struct MicInput {
    pub reader: MicReader,
    pub volume: Arc<AtomicF32>,
    pub passthrough: Arc<AtomicBool>,
}

/// The control side of an output: send commands, collect finished clips.
pub struct OutputControl {
    pub cmds: Producer<Cmd>,
    pub garbage: Consumer<Arc<ClipAudio>>,
    pub state: Arc<PlayState>,
}

impl OutputControl {
    pub fn send(&mut self, cmd: Cmd) {
        self.collect();
        let _ = self.cmds.push(cmd);
    }
    /// Free clips the audio thread has finished with (here, not on the audio thread).
    pub fn collect(&mut self) {
        while self.garbage.pop().is_some() {}
    }
}

pub fn output(volume: Arc<AtomicF32>, mic: Option<MicInput>) -> (OutputMixer, OutputControl) {
    let (cp, cc) = spsc::channel(256);
    let (gp, gc) = spsc::channel(512);
    let state = PlayState::new();
    (
        OutputMixer {
            voices: Vec::with_capacity(MAX_VOICES),
            cmds: cc,
            garbage: gp,
            guard: PeakGuard::new(),
            volume,
            state: state.clone(),
            mic,
            mic_tmp: vec![0.0; MAX_BLOCK].into_boxed_slice(),
        },
        OutputControl { cmds: cp, garbage: gc, state },
    )
}

impl OutputMixer {
    fn discard(&mut self, clip: Arc<ClipAudio>) {
        if let Err(c) = self.garbage.push(clip) {
            // never in practice (the control side drains it); leaking beats freeing here
            std::mem::forget(c);
        }
    }

    fn apply_commands(&mut self) {
        while let Some(cmd) = self.cmds.pop() {
            match cmd {
                Cmd::Play(clip) => {
                    if let Some(i) = self.voices.iter().position(|v| v.clip.id == clip.id) {
                        let old = self.voices.remove(i);
                        self.discard(old.clip);
                    }
                    if self.voices.len() == MAX_VOICES {
                        let old = self.voices.remove(0);
                        self.discard(old.clip);
                    }
                    self.voices.push(Voice { clip, pos: 0, looped: false });
                }
                Cmd::Stop(id) => {
                    while let Some(i) = self.voices.iter().position(|v| v.clip.id == id) {
                        let old = self.voices.remove(i);
                        self.discard(old.clip);
                    }
                }
                Cmd::StopAll => {
                    while let Some(v) = self.voices.pop() {
                        self.discard(v.clip);
                    }
                }
                Cmd::Loop(id, on) => {
                    for v in self.voices.iter_mut().filter(|v| v.clip.id == id) {
                        v.looped = on;
                    }
                }
            }
        }
    }

    /// Fill `out` (interleaved stereo) with the next block of this output.
    pub fn render(&mut self, out: &mut [f32]) {
        let t0 = std::time::Instant::now();
        self.apply_commands();
        for chunk in out.chunks_mut(MAX_BLOCK * 2) {
            self.render_block(chunk);
        }
        // publish progress
        let st = &*self.state;
        for (i, v) in self.voices.iter().enumerate() {
            st.ids[i].store(v.clip.id, Ordering::Relaxed);
            st.pos[i].store(v.pos.min(u32::MAX as usize) as u32, Ordering::Relaxed);
            st.len[i].store(v.clip.frames().min(u32::MAX as usize) as u32, Ordering::Relaxed);
        }
        st.count.store(self.voices.len() as u32, Ordering::Release);
        st.limited_blocks.store(self.guard.limited_blocks, Ordering::Relaxed);
        st.clipped_samples.store(self.guard.clipped_samples, Ordering::Relaxed);
        if let Some(m) = &self.mic {
            st.mic_gaps.store(m.reader.underruns, Ordering::Relaxed);
            st.mic_skips.store(m.reader.drops, Ordering::Relaxed);
        }
        st.blocks.fetch_add(1, Ordering::Relaxed);
        let ns = t0.elapsed().as_nanos() as u64;
        st.render_ns.fetch_add(ns, Ordering::Relaxed);
        if ns > st.max_render_ns.load(Ordering::Relaxed) {
            st.max_render_ns.store(ns, Ordering::Relaxed);
        }
    }

    fn render_block(&mut self, out: &mut [f32]) {
        let frames = out.len() / 2;
        out.fill(0.0);
        let mut ended = false;
        for v in self.voices.iter_mut() {
            let total = v.clip.frames();
            // a looping clip wraps to its start inside the block (no gap); others end here
            let mut done = 0;
            loop {
                let n = (frames - done).min(total.saturating_sub(v.pos));
                let dst = &mut out[done * 2..(done + n) * 2];
                match &v.clip.samples {
                    Samples::F32(d) => {
                        for (o, s) in dst.iter_mut().zip(&d[v.pos * 2..(v.pos + n) * 2]) {
                            *o += *s;
                        }
                    }
                    Samples::I16(d) => {
                        for (o, s) in dst.iter_mut().zip(&d[v.pos * 2..(v.pos + n) * 2]) {
                            *o += *s as f32 / 32768.0;
                        }
                    }
                }
                v.pos += n;
                done += n;
                if done >= frames || !v.looped || total == 0 {
                    break;
                }
                v.pos = 0;
            }
            ended |= v.pos >= total;
        }
        if ended {
            let mut i = 0;
            while i < self.voices.len() {
                if self.voices[i].pos >= self.voices[i].clip.frames() {
                    let v = self.voices.remove(i);
                    self.discard(v.clip);
                } else {
                    i += 1;
                }
            }
        }
        let vol = self.volume.get();
        if vol != 1.0 {
            for o in out.iter_mut() {
                *o *= vol;
            }
        }
        if let Some(m) = &mut self.mic {
            let mic = &mut self.mic_tmp[..frames];
            m.reader.read_into(mic); // always drain, even when muted, so the delay stays small
            let mv = m.volume.get();
            if m.passthrough.load(Ordering::Relaxed) && mv != 0.0 {
                for (f, s) in out.chunks_exact_mut(2).zip(mic.iter()) {
                    let x = *s * mv;
                    f[0] += x;
                    f[1] += x;
                }
            }
        }
        self.guard.process(out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(id: u32, data: Vec<f32>) -> Arc<ClipAudio> {
        Arc::new(ClipAudio { id, samples: Samples::F32(data.into_boxed_slice()) })
    }

    fn sine(freq: f32, frames: usize, amp: f32) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let v = amp * (2.0 * std::f32::consts::PI * freq * i as f32 / SR as f32).sin();
                [v, v]
            })
            .collect()
    }

    #[test]
    fn guard_is_bit_exact_under_the_ceiling_and_1ms_late() {
        let mut g = PeakGuard::new();
        let x = sine(1000.0, 4800, 0.5);
        let mut out = Vec::new();
        for block in x.chunks(480 * 2) {
            let mut b = block.to_vec();
            g.process(&mut b);
            out.extend(b);
        }
        let a = PeakGuard::ATTACK * 2;
        assert!(out[..a].iter().all(|v| *v == 0.0));
        assert_eq!(&out[a..], &x[..x.len() - a]);
        assert_eq!(g.limited_blocks, 0);
    }

    #[test]
    fn guard_keeps_loud_mix_under_ceiling_without_clipping() {
        let mut g = PeakGuard::new();
        let x = sine(200.0, 48000, 1.8);
        let mut peak = 0f32;
        for block in x.chunks(441 * 2) {
            let mut b = block.to_vec();
            g.process(&mut b);
            peak = b.iter().fold(peak, |p, v| p.max(v.abs()));
        }
        assert!(peak <= PeakGuard::CEILING + 1e-6, "peak {peak}");
        assert_eq!(g.clipped_samples, 0);
        assert!(g.limited_blocks > 0);
    }

    #[test]
    fn guard_recovers_after_a_burst() {
        let mut g = PeakGuard::new();
        let mut loud = sine(300.0, 4800, 1.5);
        g.process(&mut loud);
        assert!(g.gain < 0.7);
        for _ in 0..40 {
            let mut quiet = sine(300.0, 480, 0.2);
            g.process(&mut quiet);
        }
        assert_eq!(g.gain, 1.0);
    }

    #[test]
    fn a_looped_clip_starts_over_without_a_gap_and_stops_on_request() {
        let (mut m, mut c) = output(Arc::new(AtomicF32::new(1.0)), None);
        // 300 frames of a ramp, blocks of 128: the loop point falls inside a block
        let ramp: Vec<f32> = (0..300).flat_map(|i| [i as f32 / 1000.0, i as f32 / 1000.0]).collect();
        c.send(Cmd::Play(clip(7, ramp.clone())));
        c.send(Cmd::Loop(7, true));
        let mut got = Vec::new();
        let mut out = vec![0.0f32; 256];
        for _ in 0..8 {
            m.render(&mut out);
            got.extend(out.iter().step_by(2).copied());
        }
        // 1024 frames (after the guard's 1 ms = 48 frame delay): the ramp three times over and
        // a bit, sample for sample
        for (i, s) in got.iter().enumerate().skip(48) {
            assert_eq!(*s, ((i - 48) % 300) as f32 / 1000.0, "frame {i}");
        }
        assert!(c.state.is_playing(7));
        // looping off: it ends at the end of the current pass
        c.send(Cmd::Loop(7, false));
        for _ in 0..4 {
            m.render(&mut out);
        }
        assert!(!c.state.is_playing(7));
    }

    #[test]
    fn mixer_sums_voices_applies_volume_and_restarts() {
        let vol = Arc::new(AtomicF32::new(0.5));
        let (mut m, mut c) = output(vol, None);
        c.send(Cmd::Play(clip(1, vec![0.2; 2000])));
        c.send(Cmd::Play(clip(2, vec![0.4; 600])));
        let mut out = vec![0.0; 960];
        m.render(&mut out);
        assert_eq!(m.state.count.load(Ordering::Relaxed), 1); // clip 2 (300 frames) finished
        // 1 ms delay from the guard; then (0.2 + 0.4) * 0.5
        assert_eq!(out[96], (0.2f32 + 0.4) * 0.5);
        assert!(m.state.is_playing(1) && !m.state.is_playing(2));
        c.send(Cmd::Play(clip(1, vec![0.2; 2000]))); // restart: still one voice
        m.render(&mut out);
        let mut snap = Vec::new();
        m.state.snapshot(&mut snap);
        assert_eq!(snap.len(), 1);
        assert!((snap[0].1 - 480.0 / 1000.0).abs() < 1e-6);
        c.send(Cmd::StopAll);
        m.render(&mut out);
        assert_eq!(m.state.count.load(Ordering::Relaxed), 0);
        c.collect();
    }

    #[test]
    fn finished_clips_are_freed_off_the_audio_thread() {
        let (mut m, mut c) = output(Arc::new(AtomicF32::new(1.0)), None);
        let a = clip(7, vec![0.1; 20]);
        c.send(Cmd::Play(a.clone()));
        let mut out = vec![0.0; 64];
        m.render(&mut out);
        // the voice ended inside render; its Arc went to the garbage queue, not dropped there
        assert_eq!(Arc::strong_count(&a), 2);
        c.collect();
        assert_eq!(Arc::strong_count(&a), 1);
    }

    #[test]
    fn i16_clips_mix_exactly() {
        let (mut m, mut c) = output(Arc::new(AtomicF32::new(1.0)), None);
        let s: Vec<i16> = vec![16384; 400];
        c.send(Cmd::Play(Arc::new(ClipAudio { id: 3, samples: Samples::I16(s.into_boxed_slice()) })));
        let mut out = vec![0.0; 400];
        m.render(&mut out);
        assert_eq!(out[100], 0.5);
    }

    #[test]
    fn mic_ring_cushion_gap_and_drift() {
        let ring = MicRing::new(48000);
        let mut r = MicReader::new(ring.clone(), 48000 * 12 / 100);
        let mut out = vec![1.0; 480];
        r.read_into(&mut out);
        assert!(out.iter().all(|v| *v == 0.0)); // nothing yet: silence
        let block = vec![0.5f32; 480];
        for _ in 0..4 {
            ring.write(&block);
        }
        r.read_into(&mut out);
        assert_eq!(out[0], 0.0); // fades in
        assert_eq!(out[200], 0.5);
        // starve it: an underrun, faded out
        for _ in 0..4 {
            r.read_into(&mut out);
        }
        assert!(r.underruns >= 1);
        // drift: far more than max_frames waiting -> one skip back to the cushion
        for _ in 0..40 {
            ring.write(&block);
        }
        r.read_into(&mut out);
        assert_eq!(r.drops, 1);
        assert!(r.available() <= r.cushion());
    }

    #[test]
    fn cable_adds_mic_only_with_passthrough() {
        let ring = MicRing::new(48000);
        let pass = Arc::new(AtomicBool::new(false));
        let mic = MicInput {
            reader: MicReader::new(ring.clone(), 5760),
            volume: Arc::new(AtomicF32::new(1.0)),
            passthrough: pass.clone(),
        };
        let (mut m, _c) = output(Arc::new(AtomicF32::new(1.0)), Some(mic));
        let mut out = vec![0.0; 960];
        for _ in 0..10 {
            ring.write(&[0.25; 480]);
            m.render(&mut out);
        }
        assert!(out.iter().all(|v| *v == 0.0));
        pass.store(true, Ordering::Relaxed);
        for _ in 0..3 {
            ring.write(&[0.25; 480]);
            m.render(&mut out);
        }
        assert_eq!(out[500], 0.25);
    }

    #[test]
    fn render_cost_is_tiny() {
        let (mut m, mut c) = output(Arc::new(AtomicF32::new(0.8)), None);
        for id in 1..=8 {
            c.send(Cmd::Play(clip(id, sine(100.0 * id as f32, 48000 * 5, 0.3))));
        }
        let mut out = vec![0.0; 960];
        let t = std::time::Instant::now();
        for _ in 0..400 {
            m.render(&mut out);
        }
        let per_block = t.elapsed().as_secs_f64() / 400.0 * 1000.0;
        assert!(per_block < 0.5, "{per_block} ms per 10 ms block");
    }
}
