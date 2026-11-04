"""Audio engine: mixes your real mic + sound clips into a virtual cable,
and plays the clips to your headphones so you can hear them too."""
import subprocess
import threading
from collections import deque

import numpy as np
import sounddevice as sd
import soundfile as sf

from paths import find_ffmpeg

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
    ffmpeg = find_ffmpeg()
    if not ffmpeg:
        raise RuntimeError("ffmpeg not found (needed for this file type)")
    proc = subprocess.run(
        [ffmpeg, "-v", "error", "-i", str(path), "-f", "f32le", "-ac", "2", "-ar", str(SR), "-"],
        capture_output=True, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
    )
    if proc.returncode != 0:
        raise RuntimeError(proc.stderr.decode(errors="replace").strip() or "ffmpeg failed")
    return np.frombuffer(proc.stdout, dtype=np.float32).reshape(-1, 2).copy()


def load_audio(path):
    """Load any audio file as float32 (frames, 2) at SR."""
    try:
        data, sr = sf.read(str(path), dtype="float32", always_2d=True)
    except Exception:
        return _ffmpeg_decode(path)
    if data.shape[1] == 1:
        data = np.repeat(data, 2, axis=1)
    elif data.shape[1] > 2:
        data = data[:, :2]
    if sr != SR:
        n_out = int(round(len(data) * SR / sr))
        x_old = np.arange(len(data))
        x_new = np.linspace(0, len(data) - 1, n_out)
        data = np.stack([np.interp(x_new, x_old, data[:, c]) for c in range(2)], axis=1)
    return np.ascontiguousarray(data, dtype=np.float32)


# ---------- engine ----------

class MicBuffer:
    """Hands mic samples from the input stream to the output stream.
    Capped so that clock drift between the two devices can't build up delay."""

    def __init__(self, max_frames=int(SR * 0.12)):
        self.chunks = deque()
        self.size = 0
        self.max_frames = max_frames
        self.lock = threading.Lock()

    def write(self, x):
        with self.lock:
            self.chunks.append(x.copy())
            self.size += len(x)
            while self.size > self.max_frames and self.chunks:
                self.size -= len(self.chunks.popleft())

    def read(self, n):
        out = np.zeros(n, np.float32)
        i = 0
        with self.lock:
            while i < n and self.chunks:
                c = self.chunks[0]
                take = min(n - i, len(c))
                out[i:i + take] = c[:take]
                if take == len(c):
                    self.chunks.popleft()
                else:
                    self.chunks[0] = c[take:]
                self.size -= take
                i += take
        return out


class Engine:
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
        self.cable_open = False
        self.monitor_open = False

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
        for s in self._streams:
            try:
                s.stop()
                s.close()
            except Exception:
                pass
        self._streams = []
        self._mic_buf = None
        self.cable_open = self.monitor_open = False
        self.stop_all()

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

    # --- audio callbacks ---
    def _mix(self, voices, frames):
        out = np.zeros((frames, CHANNELS), np.float32)
        with self.lock:
            alive = []
            for v in voices:
                _, data, pos = v
                chunk = data[pos:pos + frames]
                out[:len(chunk)] += chunk
                v[2] = pos + frames
                if v[2] < len(data):
                    alive.append(v)
            voices[:] = alive
        return out

    def _mic_cb(self, indata, frames, t, status):
        buf = self._mic_buf
        if buf is not None:
            buf.write(indata[:, 0])

    def _cable_cb(self, outdata, frames, t, status):
        out = self._mix(self.cable_voices, frames) * self.clip_volume
        buf = self._mic_buf
        if buf is not None:
            mic = buf.read(frames)  # always drain, even when muted
            if self.mic_passthrough:
                out += (mic * self.mic_volume)[:, None]
        np.clip(out, -1.0, 1.0, out=out)
        outdata[:] = out

    def _monitor_cb(self, outdata, frames, t, status):
        out = self._mix(self.monitor_voices, frames) * self.monitor_volume
        np.clip(out, -1.0, 1.0, out=out)
        outdata[:] = out
