"""Audio engine: mixes your real mic + sound clips into a virtual cable,
and plays the clips to your headphones so you can hear them too."""
import logging
import subprocess
import threading

import numpy as np
import sounddevice as sd
import soundfile as sf

from paths import RESAMPLE, find_ffmpeg

log = logging.getLogger("soundboard")

SR = 48000
CHANNELS = 2
AUDIO_EXTS = {".wav", ".mp3", ".ogg", ".flac", ".m4a", ".mp4", ".aac", ".opus", ".webm"}
HOST_API = "Windows DirectSound"  # full device names + automatic resampling


# ---------- devices ----------

def _hostapi_index():
    for i, api in enumerate(sd.query_hostapis()):
        if api["name"] == HOST_API:
            return i
    return None


# "follow the Windows default" entries; a mic one becomes CABLE Output once that's
# the Windows default mic (needed for games like DayZ), which would loop clips back in
MAPPER_NAMES = {"Primary Sound Capture Driver", "Primary Sound Driver"}


def is_mapper(name):
    return name in MAPPER_NAMES or (name or "").startswith("Microsoft Sound Mapper")


def list_devices(kind):
    """Return device names for kind 'input' or 'output'."""
    api = _hostapi_index()
    key = "max_input_channels" if kind == "input" else "max_output_channels"
    names = []
    for d in sd.query_devices():
        if d[key] > 0 and (api is None or d["hostapi"] == api) and d["name"] not in names:
            if kind == "input" and is_mapper(d["name"]):
                continue
            names.append(d["name"])
    return names


def find_device(name, kind):
    if not name:
        return None
    api = _hostapi_index()
    key = "max_input_channels" if kind == "input" else "max_output_channels"
    for i, d in enumerate(sd.query_devices()):
        if d[key] > 0 and d["name"] == name and (api is None or d["hostapi"] == api):
            return i
    return None


def find_cable_output_name():
    """The device we send into, i.e. VB-CABLE's 'CABLE Input' speaker."""
    for n in list_devices("output"):
        if "cable input" in n.lower():
            return n
    return None


def default_device_name(kind):
    """A real device name, never a mapper or a CABLE device."""
    key = "default_input_device" if kind == "input" else "default_output_device"
    names = [n for n in list_devices(kind) if not is_mapper(n) and "cable" not in n.lower()]
    # MME's default is the actual device, but its name is cut to 31 chars
    for api in sd.query_hostapis():
        if api["name"] == "MME" and api[key] >= 0:
            short = sd.query_devices(api[key])["name"]
            for n in names:
                if n.startswith(short):
                    return n
    return names[0] if names else None


# ---------- clip loading ----------

def _ffmpeg_decode(path):
    """Decode anything ffmpeg reads to float32 (frames, 2) at SR: a proper stereo downmix (keeps a
    5.1 file's centre channel, where dialogue is) and the high-quality resampler (paths.RESAMPLE)."""
    ffmpeg = find_ffmpeg()
    if not ffmpeg:
        raise RuntimeError("ffmpeg not found (needed for this file type)")
    proc = subprocess.run(
        [ffmpeg, "-v", "error", "-i", str(path), "-vn", "-af", RESAMPLE, "-f", "f32le", "-ac", "2",
         "-ar", str(SR), "-"],
        capture_output=True, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
    )
    if proc.returncode != 0:
        raise RuntimeError(proc.stderr.decode(errors="replace").strip() or "ffmpeg failed")
    return np.frombuffer(proc.stdout, dtype=np.float32).reshape(-1, 2).copy()


def load_audio(path):
    """Load any audio file as float32 (frames, 2) at SR. A 48 kHz mono/stereo file is read as is
    (bit-exact); anything else goes through ffmpeg, never a home-made resampler or downmix."""
    try:
        info = sf.info(str(path))
    except Exception:
        return _ffmpeg_decode(path)
    if info.samplerate != SR or info.channels > 2:
        return _ffmpeg_decode(path)
    data, _ = sf.read(str(path), dtype="float32", always_2d=True)
    if data.shape[1] == 1:
        data = np.repeat(data, 2, axis=1)   # mono: the same signal on both sides, no level change
    return np.ascontiguousarray(data, dtype=np.float32)


# ---------- engine ----------

class PeakGuard:
    """Keeps a mix under CEILING without hard clipping, and passes it through untouched (bit-exact,
    1 ms late) while it stays under, which a single normalised clip at normal volume always does.

    A 1 ms lookahead limiter: the output runs ATTACK samples behind the input, so when a block would
    go over (clips overlapping, a slider above 100 %, mic + clips) the gain has already glided down
    before the loud sample comes out; afterwards it glides back up to 1 over about RELEASE seconds.
    The gain only moves as smooth ramps, never per sample, so it adds no distortion of its own.
    Cost per block: two copies and a max/min, all in preallocated arrays."""
    CEILING = 10 ** (-1 / 20)     # -1 dBFS: headroom for the voice codec's own reconstruction peaks
    ATTACK = 48                   # 1 ms lookahead and gain glide
    RELEASE = 0.3                 # s for the gain to climb a full 1.0 (0.5 -> 1 in 150 ms)

    def __init__(self, sr=SR, channels=CHANNELS):
        self.gain = 1.0
        self.step = 1.0 / (self.RELEASE * sr)
        self.limited_blocks = 0
        self.clipped_samples = 0
        self.channels = channels
        self._work = np.zeros((self.ATTACK + 4096, channels), np.float32)   # tail + this block
        self._ramp = np.empty(4096, np.float32)

    def reset(self):
        self.gain = 1.0
        self._work[:self.ATTACK] = 0

    def process(self, buf):
        """buf: float32 (frames, channels): replaced in place by the limited, 1 ms delayed signal."""
        frames, a = len(buf), self.ATTACK
        if not frames:
            return
        if len(self._work) < a + frames:                     # a bigger block than ever before
            work = np.zeros((a + frames, self.channels), np.float32)
            work[:a] = self._work[:a]
            self._work, self._ramp = work, np.empty(frames, np.float32)
        x = self._work[:a + frames]
        x[a:] = buf                                          # x = last block's tail + this block
        # everything still to come out (incl. the tail) must fit under the gain at the block's end
        peak = max(float(x.max()), -float(x.min()))          # no temporary arrays
        need = self.CEILING / peak if peak > self.CEILING else 1.0
        g0 = self.gain
        target = need if need < g0 else min(1.0, need, g0 + self.step * frames)
        buf[:] = x[:frames]                                  # out: delayed by `a`
        if g0 != 1.0 or target != 1.0:
            self.limited_blocks += 1
            ramp = self._ramp[:frames]
            if target < g0:                                  # attack: down within 1 ms, then hold
                n = min(a, frames)
                ramp[:n] = np.linspace(g0, target, n, endpoint=False, dtype=np.float32)
                ramp[n:] = target
            else:                                            # release: a slow climb
                ramp[:] = np.linspace(g0, target, frames, endpoint=False, dtype=np.float32)
            buf *= ramp[:, None]
            self.gain = target
            if max(float(buf.max()), -float(buf.min())) > 1.0:   # safety net only
                self.clipped_samples += int(np.count_nonzero(np.abs(buf) > 1.0))
                np.clip(buf, -1.0, 1.0, out=buf)
        x[:a] = x[frames:frames + a].copy() if frames < a else x[frames:]   # keep the new tail


class MicBuffer:
    """Hands mic samples from the input stream to the cable stream: a ring buffer (no allocation
    while running) with a small jitter cushion.

    The two streams run on different device clocks and in different block sizes, so the cable side
    sometimes asks before the mic side has delivered. The old version then filled the gap with
    zeros on every such read (crackly voice). Now it waits until it holds a cushion (the biggest
    mic block + the biggest read seen, plus 5 ms) before playing, fades out and back in at a real
    gap, and when clock drift piles up delay past max_frames it drops back to the cushion once
    (one tiny skip instead of steady crackle)."""
    FADE = 96   # 2 ms

    def __init__(self, max_frames=int(SR * 0.12), capacity=SR):
        self.buf = np.zeros(capacity, np.float32)
        self.cap = capacity
        self.max_frames = max_frames
        self.r = self.count = 0
        self.primed = False
        self.big_write = self.big_read = 0
        self.underruns = self.drops = 0
        self.lock = threading.Lock()
        self._fade_in = np.linspace(0, 1, self.FADE, dtype=np.float32)
        self._fade_out = self._fade_in[::-1].copy()

    @property
    def size(self):
        return self.count

    def cushion(self):
        return min(self.max_frames, self.big_write + self.big_read + int(SR * 0.005))

    def write(self, x):
        n = len(x)
        with self.lock:
            self.big_write = max(self.big_write, n)
            if n >= self.cap:
                x, n = x[-self.cap:], self.cap
            if self.count + n > self.cap:                    # never in practice: keep the newest
                drop = self.count + n - self.cap
                self.r = (self.r + drop) % self.cap
                self.count -= drop
            w = (self.r + self.count) % self.cap
            first = min(n, self.cap - w)
            self.buf[w:w + first] = x[:first]
            self.buf[:n - first] = x[first:]
            self.count += n
            if self.count > self.max_frames:                 # drift built up delay: skip back once
                drop = self.count - self.cushion()
                self.r = (self.r + drop) % self.cap
                self.count -= drop
                self.drops += 1

    def read_into(self, out):
        """Fill out (float32, n) with mic samples, or silence while (re)building the cushion."""
        n = len(out)
        with self.lock:
            self.big_read = max(self.big_read, n)
            fade_in = False
            if not self.primed:
                if self.count < self.cushion():
                    out[:] = 0
                    return
                self.primed = fade_in = True
            take = min(n, self.count)
            first = min(take, self.cap - self.r)
            out[:first] = self.buf[self.r:self.r + first]
            out[first:take] = self.buf[:take - first]
            self.r = (self.r + take) % self.cap
            self.count -= take
            if take < n:                                     # ran dry: fade out, wait for cushion
                self.underruns += 1
                self.primed = False
                f = min(self.FADE, take)
                if f:
                    out[take - f:take] *= self._fade_out[self.FADE - f:]
                out[take:] = 0
        if fade_in:
            f = min(self.FADE, n)
            out[:f] *= self._fade_in[:f]

    def read(self, n):
        out = np.zeros(n, np.float32)
        self.read_into(out)
        return out


class Engine:
    """Two outputs from the same clip data, sample for sample:
        cable   (VB-CABLE "CABLE Input", what Discord/the game hear)
                = clips x clip_volume + mic x mic_volume (only with Send my voice)
        monitor (your headphones) = clips x monitor_volume
    Each then passes its own PeakGuard. Everything is float32 at 48 kHz stereo; the callbacks
    mix straight into PortAudio's buffer and allocate nothing."""

    def __init__(self):
        self.lock = threading.Lock()
        self.cable_voices = []    # [name, data, pos]
        self.monitor_voices = []
        self.mic_volume = 1.0
        self.clip_volume = 0.8
        self.monitor_volume = 0.6
        self.mic_passthrough = True
        self.latency = "low"
        self._streams = []
        self._mic_buf = None
        self._mic_tmp = np.zeros(SR, np.float32)
        self.cable_guard = PeakGuard()
        self.monitor_guard = PeakGuard()
        self.cable_open = False
        self.monitor_open = False
        self.underflows = {"cable": 0, "monitor": 0, "mic": 0}
        self.tap = None   # diagnostics only: callable(path, block) gets what each output sends

    # --- lifecycle ---
    def start(self, mic_name, cable_name, monitor_name):
        """Open the streams. Returns a list of warning strings."""
        self.stop()
        warnings = []
        cable = find_device(cable_name, "output")
        monitor = find_device(monitor_name, "output")
        mic = find_device(mic_name, "input")
        if cable is None:
            warnings.append("No virtual cable selected - clips won't reach Discord/games.")
        if mic_name and "cable" in mic_name.lower():
            warnings.append("Mic is set to a CABLE device - that would loop audio, so mic is disabled.")
            mic = None
        if cable is not None and cable == monitor:
            warnings.append("Monitor and cable are the same device - monitor disabled.")
            monitor = None

        self.cable_guard = PeakGuard()
        self.monitor_guard = PeakGuard()
        self.underflows = dict.fromkeys(self.underflows, 0)
        try:
            if cable is not None:
                if mic is not None:
                    self._mic_buf = MicBuffer()
                    s = sd.InputStream(device=mic, samplerate=SR, channels=1, dtype="float32",
                                       latency=self.latency, callback=self._mic_cb)
                    s.start()
                    self._streams.append(s)
                s = sd.OutputStream(device=cable, samplerate=SR, channels=CHANNELS, dtype="float32",
                                    latency=self.latency, callback=self._cable_cb)
                s.start()
                self._streams.append(s)
                self.cable_open = True
            if monitor is not None:
                s = sd.OutputStream(device=monitor, samplerate=SR, channels=CHANNELS, dtype="float32",
                                    latency=self.latency, callback=self._monitor_cb)
                s.start()
                self._streams.append(s)
                self.monitor_open = True
        except Exception:
            self.stop()
            raise
        return warnings

    def stop(self):
        had_streams = bool(self._streams)
        for s in self._streams:
            try:
                s.stop()
                s.close()
            except Exception:
                pass
        if had_streams:
            st = self.stats()
            if any(v for k, v in st.items() if k != "limited_blocks"):
                log.info("audio stats: %s", st)   # dropouts / mic gaps show up in soundboard.log
        self._streams = []
        self._mic_buf = None
        self.cable_open = self.monitor_open = False
        self.stop_all()

    def stats(self):
        """Counters since start(): dropouts, mic gaps, peak-guard work. Cheap to read any time."""
        mic = self._mic_buf
        return {"cable_underflows": self.underflows["cable"],
                "monitor_underflows": self.underflows["monitor"],
                "mic_overflows": self.underflows["mic"],
                "mic_gaps": mic.underruns if mic else 0, "mic_skips": mic.drops if mic else 0,
                "limited_blocks": self.cable_guard.limited_blocks + self.monitor_guard.limited_blocks,
                "clipped_samples": self.cable_guard.clipped_samples + self.monitor_guard.clipped_samples}

    # --- playback control (thread-safe, called from GUI and hotkey thread) ---
    def play(self, name, data):
        """Start a clip. Pressing a clip that's already playing restarts it."""
        with self.lock:
            for voices, is_open in ((self.cable_voices, self.cable_open),
                                    (self.monitor_voices, self.monitor_open)):
                voices[:] = [v for v in voices if v[0] != name]
                if is_open:
                    voices.append([name, data, 0])

    def stop_clip(self, name):
        with self.lock:
            for voices in (self.cable_voices, self.monitor_voices):
                voices[:] = [v for v in voices if v[0] != name]

    def stop_all(self):
        with self.lock:
            self.cable_voices.clear()
            self.monitor_voices.clear()

    def progress(self):
        """{clip name: 0..1 played} for clips currently playing."""
        with self.lock:
            src = self.cable_voices if self.cable_open else self.monitor_voices
            return {v[0]: min(1.0, v[2] / max(1, len(v[1]))) for v in src}

    # --- audio callbacks (real time: no allocation, no I/O, no waiting beyond the short lock) ---
    def _mix_into(self, out, voices):
        """Write the sum of every playing voice's next block into out (frames, 2)."""
        frames = len(out)
        out.fill(0)
        with self.lock:
            ended = False
            for v in voices:
                data, pos = v[1], v[2]
                chunk = data[pos:pos + frames]
                out[:len(chunk)] += chunk
                v[2] = pos + frames
                ended |= v[2] >= len(data)
            if ended:
                voices[:] = [v for v in voices if v[2] < len(v[1])]

    def _mic_cb(self, indata, frames, t, status):
        if status.input_overflow:
            self.underflows["mic"] += 1
        buf = self._mic_buf
        if buf is not None:
            buf.write(indata[:, 0])

    def _cable_cb(self, outdata, frames, t, status):
        if status.output_underflow:
            self.underflows["cable"] += 1
        self._mix_into(outdata, self.cable_voices)
        if self.clip_volume != 1.0:
            outdata *= self.clip_volume
        buf = self._mic_buf
        if buf is not None:
            if len(self._mic_tmp) < frames:
                self._mic_tmp = np.zeros(frames, np.float32)
            mic = self._mic_tmp[:frames]
            buf.read_into(mic)          # always drain, even when muted, so the delay stays small
            if self.mic_passthrough and self.mic_volume:
                mic *= self.mic_volume
                outdata += mic[:, None]
        self.cable_guard.process(outdata)
        if self.tap:
            self.tap("cable", outdata)

    def _monitor_cb(self, outdata, frames, t, status):
        if status.output_underflow:
            self.underflows["monitor"] += 1
        self._mix_into(outdata, self.monitor_voices)
        if self.monitor_volume != 1.0:
            outdata *= self.monitor_volume
        self.monitor_guard.process(outdata)
        if self.tap:
            self.tap("monitor", outdata)
