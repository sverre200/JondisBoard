r"""Audio diagnostics for developers: what JondisBoard sends, and what actually comes out of VB-CABLE.

    .venv\Scripts\python.exe scripts\audio_check.py                 offline (safe any time)
    .venv\Scripts\python.exe scripts\audio_check.py loopback        plays test signals through the cable
    .venv\Scripts\python.exe scripts\audio_check.py record 20       records CABLE Output for 20 s

offline   Runs test signals and your own clips through the real engine's two outputs (no devices) at
          your saved volumes and compares them with the source and with the old engine (sum + hard
          clip). Nothing is played or recorded.
loopback  Starts the real engine on "CABLE Input", plays each test signal and records it back from
          "CABLE Output": level, frequency response, distortion, dropouts, sample-rate drift and how
          close it is to the source. ANYONE LISTENING TO YOUR CABLE HEARS THE TEST TONES, so it won't
          run while DayZ or Discord is open (add --force if you're sure nobody is in voice).
record    Records CABLE Output (what the game hears) while you press sounds in JondisBoard, then
          measures it. Same warning: don't use it with friends in voice unless they know.

Everything goes to build\audio_check\<time>\ as WAV files (listen to source vs cable side by side)
plus report.txt. Nothing here runs inside the app; normal use is unaffected."""
import json
import subprocess
import sys
import threading
import time
from pathlib import Path

import numpy as np
import soundfile as sf

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "src"))
import engine  # noqa: E402
from engine import SR, Engine  # noqa: E402

OUT = ROOT / "build" / "audio_check" / time.strftime("%Y%m%d-%H%M%S")
lines = []


def say(text=""):
    print(text, flush=True)
    lines.append(text)


class Status:
    output_underflow = input_overflow = False


# ---------- test signals ----------

def signals():
    rng = np.random.default_rng(7)
    t = lambda s: np.arange(int(SR * s)) / SR  # noqa: E731
    out = {}
    out["sine_1k"] = 0.5 * np.sin(2 * np.pi * 1000 * t(3))
    tt = t(6)                                                        # log sweep 20 Hz -> 20 kHz
    k = np.log(20000 / 20)
    out["sweep"] = 0.5 * np.sin(2 * np.pi * 20 * 6 / k * (np.exp(tt / 6 * k) - 1))
    w = rng.standard_normal(SR * 4)                                  # pink-ish noise (music-like)
    f = np.fft.rfft(w); f[1:] /= np.sqrt(np.arange(1, len(f))); f[0] = 0
    p = np.fft.irfft(f, len(w)); out["pink_noise"] = 0.25 * p / np.abs(p).max()
    b = sum(np.sin(2 * np.pi * h * 45 * t(3)) / h for h in (1, 2, 3, 4))  # bass, 45 Hz + harmonics
    out["bass"] = 0.6 * b / np.abs(b).max()
    c = np.zeros(SR * 3)                                             # sharp transients
    for i in range(0, len(c) - 2000, SR // 4):
        c[i:i + 600] = rng.standard_normal(600) * np.exp(-np.arange(600) / 80)
    out["transients"] = 0.8 * c / np.abs(c).max()
    stereo = {}
    for name, x in out.items():
        x = x.astype(np.float32)
        stereo[name] = np.stack([x, x], 1)
    return stereo


def user_clips(limit=4):
    """A few of your own sounds: speech, music, the loudest and the widest-stereo one."""
    sounds = ROOT / "sounds"
    files = [p for p in sounds.rglob("*") if p.suffix.lower() in engine.AUDIO_EXTS and "_deleted" not in p.parts]
    clips = {}
    for p in sorted(files)[:40]:
        try:
            clips[p.stem] = engine.load_audio(p)[: SR * 8]
        except Exception:
            pass
    if not clips:
        return {}
    by_peak = max(clips, key=lambda n: np.abs(clips[n]).max())
    by_width = min(clips, key=lambda n: corr(clips[n]))
    picks = list(dict.fromkeys([by_peak, by_width] + list(clips)))[:limit]
    return {f"clip_{n}"[:60]: clips[n] for n in picks}


# ---------- measurements ----------

def db(x):
    return 20 * np.log10(max(float(x), 1e-12))


def corr(x):
    if x.shape[1] < 2 or x[:, 0].std() == 0 or x[:, 1].std() == 0:
        return 1.0
    return float(np.corrcoef(x[:, 0], x[:, 1])[0, 1])


def measure(x):
    """Levels and the problems a listener would hear."""
    pk = np.abs(x).max()
    rms = np.sqrt(np.mean(x.astype(np.float64) ** 2)) if len(x) else 0
    mono = x.mean(1)
    flat = int(np.count_nonzero(np.abs(x) >= 0.999))
    # dropouts: runs of exact digital silence (>= 3 ms) inside the signal
    nz = np.nonzero(np.abs(mono) > 1e-6)[0]
    gaps = 0
    if len(nz) > 1:
        gaps = int(np.count_nonzero(np.diff(nz) > SR * 0.003))
    lr = db(np.sqrt(np.mean(x[:, 0] ** 2)) / max(np.sqrt(np.mean(x[:, 1] ** 2)), 1e-12)) if x.shape[1] > 1 else 0
    return {"peak_dbfs": round(db(pk), 2), "rms_dbfs": round(db(rms), 2), "flat_topped": flat,
            "silence_gaps": gaps, "left_right_db": round(lr, 2), "lr_correlation": round(corr(x), 2),
            "mono_loss_db": round(db(np.sqrt(np.mean(mono ** 2)) / max(rms, 1e-12)), 2)}


def align(ref, got):
    """Delay of got against ref (mono, FFT cross-correlation) and got trimmed to match."""
    a, b = ref.mean(1), got.mean(1)
    n = 1 << int(np.ceil(np.log2(len(a) + len(b))))
    xc = np.fft.irfft(np.fft.rfft(b, n) * np.conj(np.fft.rfft(a, n)), n)
    lag = int(np.argmax(xc[: len(b)]))
    seg = got[lag:lag + len(ref)]
    if len(seg) < len(ref):
        seg = np.vstack([seg, np.zeros((len(ref) - len(seg), seg.shape[1]), seg.dtype)])
    return lag, seg


def fidelity(ref, got):
    """How much of got is NOT the source (after the best fixed gain): lower is cleaner."""
    r, g = ref.mean(1).astype(np.float64), got.mean(1).astype(np.float64)
    gain = np.dot(g, r) / max(np.dot(r, r), 1e-12)
    resid = g - gain * r
    return round(db(gain), 2), round(10 * np.log10(max(np.sum(resid ** 2), 1e-24) / max(np.sum((gain * r) ** 2), 1e-24)), 1)


def thd_n(x, f=1000):
    y = x.mean(1)[SR // 2:-SR // 2].astype(np.float64)
    S = np.abs(np.fft.rfft(y * np.blackman(len(y)))) ** 2
    fr = np.fft.rfftfreq(len(y), 1 / SR)
    tone = np.abs(fr - f) < 30
    band = (fr > 20) & (fr < 20000)
    return round(10 * np.log10(S[band & ~tone].sum() / S[tone].sum()), 1), float(fr[np.argmax(S)])


def response(ref, got):
    """Level of got vs ref in octave bands: what the path does to bass and treble."""
    R, G = np.abs(np.fft.rfft(ref.mean(1))) ** 2, np.abs(np.fft.rfft(got.mean(1))) ** 2
    fr = np.fft.rfftfreq(len(ref), 1 / SR)
    bands = {}
    for c in (31, 63, 125, 250, 500, 1000, 2000, 4000, 8000, 12000, 16000, 19000):
        m = (fr > c / 1.41) & (fr < min(c * 1.41, 23900))
        if R[m].sum() > 0:
            bands[c] = round(10 * np.log10(max(G[m].sum(), 1e-24) / R[m].sum()), 1)
    return bands


# ---------- offline ----------

def render(e, path, x, block=480):
    """x through one output of the real engine (callbacks only, no devices)."""
    e.stop_all()
    e.play("x", x)
    cb = e._cable_cb if path == "cable" else e._monitor_cb
    n, pad = len(x), engine.PeakGuard.ATTACK
    out = np.zeros((n + pad, 2), np.float32)
    for i in range(0, n + pad, block):
        cb(out[i:i + block], len(out[i:i + block]), None, Status())
    return out[pad:]


def offline(cfg):
    clip_v, mon_v = float(cfg.get("clip_volume", 0.8)), float(cfg.get("monitor_volume", 0.6))
    say(f"Offline: your volumes Clips {clip_v:.0%}, In your ears {mon_v:.0%}. Nothing is played.")
    say("Cable = what Discord/DayZ get from JondisBoard; ears = your headphones.")
    say()
    e = Engine()
    e.cable_open = e.monitor_open = True
    e.clip_volume, e.monitor_volume, e.mic_passthrough = clip_v, mon_v, False
    tests = {**signals(), **user_clips()}
    for name, x in tests.items():
        cable = render(e, "cable", x)
        ears = render(e, "monitor", x)
        g, res = fidelity(x, cable)
        m = measure(cable)
        same = np.allclose(cable / max(clip_v, 1e-9), ears / max(mon_v, 1e-9), atol=1e-5)
        say(f"{name[:40]:40} cable peak {m['peak_dbfs']:+6.1f} dBFS  rms {m['rms_dbfs']:+6.1f}  "
            f"vs source: gain {g:+.1f} dB, residual {res:+.0f} dB  flat-topped {m['flat_topped']}  "
            f"mono loss {m['mono_loss_db']:+.1f} dB  ears = cable: {'yes' if same else 'NO'}")
        sf.write(OUT / f"{name}_source.wav", x, SR, subtype="FLOAT")
        sf.write(OUT / f"{name}_cable.wav", cable, SR, subtype="FLOAT")
    say()
    say("Overlap test: two copies of the loudest test signal at your Clips volume and at 150 %:")
    x = tests["transients"]
    for v in (clip_v, 1.5):
        e.clip_volume = v
        e.stop_all(); e.play("a", x); e.play("b", x.copy())
        n, pad = len(x), engine.PeakGuard.ATTACK
        new = np.zeros((n + pad, 2), np.float32)
        for i in range(0, n + pad, 480):
            e._cable_cb(new[i:i + 480], len(new[i:i + 480]), None, Status())
        new = new[pad:]
        old = np.clip(x * 2 * v, -1, 1)
        mo, mn = measure(old), measure(new)
        say(f"  Clips {v:.0%}: old engine peak {mo['peak_dbfs']:+.1f} dBFS, {mo['flat_topped']} clipped samples"
            f"  ->  now peak {mn['peak_dbfs']:+.1f} dBFS, {mn['flat_topped']} clipped")
    e.clip_volume = clip_v


# ---------- with the real cable ----------

def busy():
    try:
        out = subprocess.run(["tasklist", "/fo", "csv", "/nh"], capture_output=True, text=True,
                             creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0)).stdout.lower()
    except Exception:
        return []
    return [n for n in ("dayz_x64.exe", "discord.exe") if n in out]


def cable_names():
    outs = engine.find_cable_output_name()
    ins = next((n for n in engine.list_devices("input") if "cable output" in n.lower()), None)
    return outs, ins


class Recorder:
    def __init__(self, device):
        import sounddevice as sd
        self.blocks, self.overflows = [], 0
        self.stream = sd.InputStream(device=engine.find_device(device, "input"), samplerate=SR, channels=2,
                                     dtype="float32", latency="low", callback=self._cb)

    def _cb(self, indata, frames, t, status):
        if status.input_overflow:
            self.overflows += 1
        self.blocks.append(indata.copy())

    def __enter__(self):
        self.stream.start()
        return self

    def __exit__(self, *a):
        self.stream.stop()
        self.stream.close()

    def take(self):
        return np.concatenate(self.blocks) if self.blocks else np.zeros((0, 2), np.float32)


def loopback(cfg):
    send, back = cable_names()
    if not send or not back:
        say("VB-CABLE not found.")
        return
    clip_v = float(cfg.get("clip_volume", 0.8))
    say(f"Loopback: engine -> {send} -> {back}, Clips at 100 % (your setting {clip_v:.0%}), mic off.")
    try:
        import sounddevice as sd
        api = next(a for a in sd.query_hostapis() if a["name"] == "Windows WASAPI")
        for i in api["devices"]:
            d = sd.query_devices(i)
            if "CABLE" in d["name"]:
                say(f"  Windows format of {d['name']}: {int(d['default_samplerate'])} Hz, "
                    f"{max(d['max_input_channels'], d['max_output_channels'])} ch")
    except Exception:
        pass
    say()
    e = Engine()
    e.clip_volume, e.mic_passthrough = 1.0, False
    e.start(None, send, None)
    try:
        tests = {**signals(), **user_clips(2)}
        for name, x in tests.items():
            with Recorder(back) as rec:
                time.sleep(0.3)
                e.play(name, x)
                time.sleep(len(x) / SR + 0.6)
            got = rec.take()
            lag, seg = align(x, got)
            g, res = fidelity(x, seg)
            m = measure(seg)
            extra = ""
            if name == "sine_1k":
                thd, peak_f = thd_n(seg)
                extra = f"  THD+N {thd:+.0f} dB, tone at {peak_f:.1f} Hz (should be 1000)"
            say(f"{name[:40]:40} latency {lag / SR * 1000:5.0f} ms  gain {g:+.2f} dB  residual {res:+.0f} dB  "
                f"peak {m['peak_dbfs']:+.1f}  gaps {m['silence_gaps']}  L/R {m['left_right_db']:+.1f} dB"
                f"  overflows {rec.overflows}{extra}")
            if name == "sweep":
                say("    response vs source (dB): " + "  ".join(f"{k} Hz {v:+.1f}" for k, v in response(x, seg).items()))
                # a sweep never repeats, so its first and last second each line up in one place:
                # any difference from the expected spacing is a sample-rate mismatch on the way
                l1, _ = align(x[:SR], got)
                l2, _ = align(x[-SR:], got)
                drift = l2 - (l1 + len(x) - SR)
                say(f"    sample-rate drift over {len(x) / SR:.0f} s: {drift:+d} samples "
                    f"({drift / (len(x) - SR) * 1e6:+.0f} ppm; 0 = same clock)")
            sf.write(OUT / f"{name}_source.wav", x, SR, subtype="FLOAT")
            sf.write(OUT / f"{name}_from_cable.wav", seg, SR, subtype="FLOAT")
        say()
        say(f"engine counters: {e.stats()}")
    finally:
        e.stop()


def record(cfg, secs):
    _, back = cable_names()
    if not back:
        say("VB-CABLE not found.")
        return
    say(f"Recording {back} for {secs:.0f} s: press sounds in JondisBoard now.")
    with Recorder(back) as rec:
        time.sleep(secs)
    got = rec.take()
    sf.write(OUT / "cable_output.wav", got, SR, subtype="FLOAT")
    m = measure(got)
    say(f"  {json.dumps(m)}  overflows {rec.overflows}")
    say(f"  saved {OUT / 'cable_output.wav'}")


def main():
    args = sys.argv[1:]
    mode = args[0] if args else "offline"
    cfg = {}
    try:
        cfg = json.loads((ROOT / "config.json").read_text(encoding="utf-8"))
    except Exception:
        pass
    if mode in ("loopback", "record") and "--force" not in args:
        b = busy()
        if b:
            print(f"Not running: {', '.join(b)} is open, and anyone listening to your cable would hear "
                  "this. Close it, or add --force if nobody is in voice.")
            return 1
    OUT.mkdir(parents=True, exist_ok=True)
    if mode == "offline":
        offline(cfg)
    elif mode == "loopback":
        loopback(cfg)
    elif mode == "record":
        record(cfg, float(args[1]) if len(args) > 1 and args[1][0].isdigit() else 15)
    else:
        print(__doc__)
        return 1
    (OUT / "report.txt").write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"\nWAVs and report: {OUT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
