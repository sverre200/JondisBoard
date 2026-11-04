"""Audio engine: numbers, not just "it plays". No windows and no devices: the callbacks are fed
plain arrays, so this suite is safe to run any time (even mid-game, it's a second of CPU).

Covers loading (bit-exact 48 kHz, mono, resampling quality, 5.1 downmix), both output paths and
their volumes, mic passthrough, peak protection vs hard clipping, the mic jitter buffer, stream
restart, allocation and callback cost, and the importer's linear loudness normalisation."""
import shutil
import subprocess
import sys
import tempfile
import time
import tracemalloc
from pathlib import Path

import numpy as np
import soundfile as sf

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))
import engine  # noqa: E402
from engine import SR, Engine, MicBuffer, PeakGuard  # noqa: E402

results = []


def check(name, ok, detail=""):
    results.append(bool(ok))
    print(("PASS " if ok else "FAIL ") + name + (f"  ({detail})" if detail else ""), flush=True)


class Status:
    output_underflow = input_overflow = False


def db(x):
    return 20 * np.log10(max(float(x), 1e-12))


def sine(f, secs, amp=0.5, sr=SR):
    t = np.arange(int(sr * secs)) / sr
    return (amp * np.sin(2 * np.pi * f * t)).astype(np.float32)


def spur_db(y, f, sr=SR):
    """Everything that isn't the tone (up to 20 kHz), relative to the tone: lower is cleaner."""
    y = y[2000:-2000].astype(np.float64)
    S = np.abs(np.fft.rfft(y * np.blackman(len(y)))) ** 2
    fr = np.fft.rfftfreq(len(y), 1 / sr)
    tone = np.abs(fr - f) < 40
    return 10 * np.log10(S[(fr < 20000) & ~tone].sum() / S[tone].sum())


def open_engine(**vol):
    e = Engine()
    e.cable_open = e.monitor_open = True     # as if start() had opened both streams
    for k, v in vol.items():
        setattr(e, k, v)
    return e


LAG = PeakGuard.ATTACK   # the peak guard's 1 ms lookahead: every output is this many frames late


def run(e, path, frames_total, block=480):
    """Pull frames_total frames (+ the 1 ms lookahead) out of one output path, block by block like
    PortAudio does, and line the result up with the input again."""
    frames_total += LAG
    cb = e._cable_cb if path == "cable" else e._monitor_cb
    out = []
    for i in range(0, frames_total, block):
        buf = np.zeros((min(block, frames_total - i), 2), np.float32)
        cb(buf, len(buf), None, Status())
        out.append(buf)
    return np.concatenate(out)[LAG:]


def main():
    tmp = Path(tempfile.mkdtemp(prefix="jondisboard-audio-"))
    try:
        loading(tmp)
        paths()
        peaks()
        mic_buffer()
        restart()
        cost()
        loudness(tmp)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    passed = sum(results)
    print(f"audio: {passed}/{len(results)} passed", flush=True)
    return 0 if results and passed == len(results) else 1


# ---------- loading ----------

def loading(tmp):
    st = np.stack([sine(440, 1, 0.6), sine(660, 1, 0.3)], 1)
    sf.write(tmp / "a.wav", st, SR, subtype="PCM_16")
    ref, _ = sf.read(tmp / "a.wav", dtype="float32", always_2d=True)
    got = engine.load_audio(tmp / "a.wav")
    check("48 kHz stereo WAV loads bit-exact", got.dtype == np.float32 and np.array_equal(got, ref))

    m = sine(440, 1, 0.5)
    sf.write(tmp / "m.wav", m, SR, subtype="FLOAT")
    got = engine.load_audio(tmp / "m.wav")
    check("mono goes to both sides unchanged (no level drop)",
          got.shape == (len(m), 2) and np.array_equal(got[:, 0], m) and np.array_equal(got[:, 1], m))

    if not engine.find_ffmpeg():
        check("ffmpeg available for resampling checks", False)
        return
    worst = []
    for f in (1000, 9000, 15000):
        x = sine(f, 2, 0.5, 44100)
        sf.write(tmp / "r.wav", np.stack([x, x], 1), 44100, subtype="FLOAT")
        y = engine.load_audio(tmp / "r.wav")
        worst.append(spur_db(y[:, 0], f))
        if f == 1000:
            n_ok = abs(len(y) - round(len(x) * SR / 44100)) <= 64
            lvl = db(np.sqrt(np.mean(y[4000:-4000, 0] ** 2)) / (0.5 / np.sqrt(2)))
            check("44.1 kHz file becomes 48 kHz at the right length and level", n_ok and abs(lvl) < 0.05,
                  f"{len(y)} frames, level {lvl:+.3f} dB")
    check("resampling junk under -90 dB (old linear interpolation: -11 to -62 dB)", max(worst) < -90,
          ", ".join(f"{w:.0f} dB" for w in worst))

    six = np.zeros((SR, 6), np.float32)
    six[:, 2] = sine(300, 1, 0.5)                       # dialogue lives in the centre channel
    sf.write(tmp / "s.wav", six, SR, subtype="FLOAT")
    y = engine.load_audio(tmp / "s.wav")
    lvl = db(np.abs(y).max() / 0.5)
    check("5.1 file keeps its centre channel (old code dropped it)", y.shape[1] == 2 and lvl > -6,
          f"centre arrives at {lvl:+.1f} dB")


# ---------- the two outputs ----------

def paths():
    rng = np.random.default_rng(1)
    clip = (rng.standard_normal((SR, 2)) * 0.15).astype(np.float32)      # wide-band, peaks ~0.7
    e = open_engine(clip_volume=0.8, monitor_volume=0.43, mic_passthrough=False)
    e.play("c", clip)
    for block in (480, 441):
        e.stop_all(); e.play("c", clip)
        cable = run(e, "cable", len(clip), block)
        e.stop_all(); e.play("c", clip)
        mon = run(e, "monitor", len(clip), block)
        if block == 480:
            check("cable output is the clip x Clips volume, bit-exact", np.array_equal(cable, clip * 0.8))
            check("headphones are the clip x In your ears volume, bit-exact", np.array_equal(mon, clip * 0.43))
        else:
            check("odd block size (441) is still bit-exact", np.array_equal(cable, clip * 0.8))
    check("cable and headphones carry the same signal (only the volume differs)",
          np.allclose(cable / 0.8, mon / 0.43, atol=1e-6))
    check("output stays float32 stereo", cable.dtype == np.float32 and cable.shape == clip.shape)

    # mic + clip into the cable; the headphones never get the mic
    e = open_engine(clip_volume=0.5, mic_volume=0.7, monitor_volume=1.0, mic_passthrough=True)
    e._mic_buf = MicBuffer()
    voice = sine(200, 1, 0.3)
    c = sine(1000, 1, 0.3)
    e.play("c", np.stack([c, c], 1))
    out = []
    for i in range(0, SR, 480):
        e._mic_cb(voice[i:i + 480, None], 480, None, Status())
        b = np.zeros((480, 2), np.float32)
        e._cable_cb(b, 480, None, Status())
        out.append(b)
    out = np.concatenate(out)[LAG:]
    # the mic arrives a cushion late: find that delay, then compare sample for sample
    res = out[:len(c) - LAG, 0] - c[:len(c) - LAG] * 0.5
    lag = int(np.argmax(np.abs(res) > 1e-6))
    while lag > 0 and not np.allclose(res[lag + 200:lag + 400], voice[200:400] * 0.7, atol=1e-6):
        lag -= 1
    mic_part = res[lag + 200:]
    expect = voice[200:len(mic_part) + 200] * 0.7
    check("Send my voice: cable = clips x Clips + mic x Mic", np.allclose(mic_part, expect, atol=1e-6),
          f"mic {lag / SR * 1000:.0f} ms behind")
    e.mic_passthrough = False
    b = np.zeros((480, 2), np.float32)
    e._mic_cb(voice[:480, None], 480, None, Status())
    before = e._mic_buf.size
    e._cable_cb(b, 480, None, Status())
    check("Send my voice off: no mic in the cable, but the mic buffer is still drained",
          np.allclose(b[:, 0], 0, atol=1e-7) or e._mic_buf.size < before)


# ---------- peaks ----------

def peaks():
    s = sine(1000, 2, 0.84)                               # a normalised clip peaks at about -1.5 dBFS
    clip = np.stack([s, s], 1)

    e = open_engine(clip_volume=1.0)
    e.play("a", clip)
    out = run(e, "cable", len(clip))
    check("one normalised clip at 100 % passes untouched", np.array_equal(out, clip)
          and e.cable_guard.limited_blocks == 0)

    # the old engine: sum, then hard clip at +/-1
    hard = np.clip(clip * 1.5 * 2, -1, 1)
    e = open_engine(clip_volume=1.5)
    e.play("a", clip); e.play("b", clip.copy())
    out = run(e, "cable", len(clip))
    body = out[4800:-4800, 0]
    peak = np.abs(out).max()
    flat = np.count_nonzero(np.abs(out) >= 0.9999)
    check("two loud clips at 150 %: no hard clipping", peak <= 1.0 and flat == 0,
          f"peak {db(peak):+.2f} dBFS, {flat} flat-topped samples")
    check("...and stay under the -1 dBFS ceiling from the very first sample",
          peak <= PeakGuard.CEILING + 1e-6, f"{db(peak):+.2f} dBFS")
    old, new = spur_db(hard[4800:-4800, 0], 1000), spur_db(body, 1000)
    check("distortion: peak guard is clean where the old hard clip wasn't", new < -60 and new < old - 30,
          f"old {old:.0f} dB, now {new:.0f} dB")

    # recovery: once the overlap ends, a quiet clip comes out untouched again
    q = sine(500, 1, 0.2)
    e.play("q", np.stack([q, q], 1))
    rest = run(e, "cable", int(SR * 0.6))
    tail = rest[int(SR * 0.4):, 0]            # run() already lined the output up with the input
    check("gain is back to 1 within 0.4 s and clean audio is bit-exact again",
          e.cable_guard.gain == 1.0 and np.array_equal(tail, (q * np.float32(1.5))[int(SR * 0.4):int(SR * 0.6)]))


# ---------- mic jitter buffer ----------

def mic_buffer():
    def simulate(write_block, read_block, drift=0.0, secs=20):
        """Writer and reader on their own clocks, events in time order. Input is a ramp, so
        every sample's value says where it came from."""
        b = MicBuffer()
        ramp = np.arange(int(SR * secs * 1.01), dtype=np.float32)
        w_rate = SR * (1 + drift)
        tw = tr = 0.0
        wpos = 0
        outs = []
        while tr < secs:
            if tw <= tr:
                b.write(ramp[wpos:wpos + write_block] / 1e6)
                wpos += write_block
                tw += write_block / w_rate
            else:
                o = np.zeros(read_block, np.float32)
                b.read_into(o)
                outs.append(o)
                tr += read_block / SR
        return b, np.concatenate(outs)

    b, out = simulate(441, 480)
    live = out[np.argmax(out > 0):]
    steps = np.diff(np.round(live[200:] * 1e6))
    check("mic: different block sizes, same clock: no gaps after start-up",
          b.underruns == 0 and b.drops == 0 and np.all(steps == 1),
          f"{b.underruns} gaps, cushion {b.cushion() / SR * 1000:.1f} ms")

    b, out = simulate(1764, 480)                          # big DirectSound-style mic blocks
    check("mic: 37 ms mic blocks vs 10 ms reads: no gaps", b.underruns == 0)

    b, _ = simulate(441, 480, drift=+0.0005)              # mic clock 500 ppm fast
    check("mic: fast mic clock: only rare single skips, no gaps", b.underruns == 0 and b.drops <= 3,
          f"{b.drops} skips in 20 s")
    b, _ = simulate(441, 480, drift=-0.0005)              # mic clock 500 ppm slow
    check("mic: slow mic clock: only rare re-fills", b.underruns <= 3, f"{b.underruns} in 20 s")


# ---------- restart ----------

def restart():
    opened = []

    class FakeStream:
        def __init__(self, **kw):
            self.kw, self.closed = kw, False
            opened.append(self)

        def start(self):
            pass

        def stop(self):
            pass

        def close(self):
            self.closed = True

    real = (engine.sd.OutputStream, engine.sd.InputStream, engine.find_device)
    engine.sd.OutputStream = engine.sd.InputStream = FakeStream
    engine.find_device = lambda name, kind: {"mic": 1, "cable": 2, "phones": 3}.get(name)
    try:
        e = Engine()
        e.start("mic", "cable", "phones")
        fmt = all(s.kw["samplerate"] == SR and s.kw["dtype"] == "float32" for s in opened)
        chans = [s.kw["channels"] for s in opened]
        check("streams: 48 kHz float32, mic mono, cable and headphones stereo",
              fmt and chans == [1, 2, 2], f"{chans}")
        e.cable_guard.gain = 0.5
        e.underflows["cable"] = 7
        e.play("x", np.zeros((SR, 2), np.float32))
        first = list(opened)
        e.start("mic", "cable", "phones")
        check("restart closes the old streams and starts clean",
              all(s.closed for s in first) and e.cable_guard.gain == 1.0
              and e.underflows["cable"] == 0 and not e.cable_voices and len(e._streams) == 3)
        st = Status()
        st.output_underflow = True
        e._cable_cb(np.zeros((480, 2), np.float32), 480, None, st)
        check("dropouts are counted", e.stats()["cable_underflows"] == 1)
        e.start("CABLE Output (VB-Audio Virtual Cable)", "cable", "cable")
        check("refuses a CABLE mic and headphones = cable (no feedback loop)",
              e._mic_buf is None and not e.monitor_open)
        e.stop()
    finally:
        engine.sd.OutputStream, engine.sd.InputStream, engine.find_device = real


# ---------- cost ----------

def cost():
    rng = np.random.default_rng(2)
    e = open_engine(clip_volume=0.8, mic_volume=0.6, mic_passthrough=True)
    e._mic_buf = MicBuffer()
    for i in range(6):
        e.play(str(i), (rng.standard_normal((SR * 30, 2)) * 0.05).astype(np.float32))
    buf = np.zeros((480, 2), np.float32)
    voice = np.zeros((480, 1), np.float32)
    for _ in range(50):                                   # warm up
        e._mic_cb(voice, 480, None, Status()); e._cable_cb(buf, 480, None, Status())
    tracemalloc.start()
    t0 = time.perf_counter()
    for _ in range(1000):
        e._mic_cb(voice, 480, None, Status())
        e._cable_cb(buf, 480, None, Status())
        e._monitor_cb(buf, 480, None, Status())
    took = (time.perf_counter() - t0) / 1000
    _, peak = tracemalloc.get_traced_memory()
    tracemalloc.stop()
    check("6 clips + mic: both callbacks under 0.5 ms per 10 ms block", took < 0.0005,
          f"{took * 1e6:.0f} us = {took / 0.01 * 100:.1f} % of real time")
    check("no per-block allocations (steady state)", peak < 16_000, f"peak {peak} bytes over 1000 blocks")


# ---------- importer loudness ----------

def loudness(tmp):
    ffmpeg = engine.find_ffmpeg()
    if not ffmpeg:
        return
    import importer
    quiet, loud = sine(440, 3, 0.03), sine(440, 3, 0.3)   # a 20 dB jump inside one clip
    x = np.concatenate([quiet, loud])
    sf.write(tmp / "dyn.wav", np.stack([x, x], 1), SR, subtype="FLOAT")
    af = importer.loudness_filter(ffmpeg, ["-i", str(tmp / "dyn.wav")])
    subprocess.run([ffmpeg, "-v", "error", "-y", "-i", str(tmp / "dyn.wav"), "-af", af, "-ac", "2",
                    "-ar", "48000", str(tmp / "out.wav")], check=True)
    y, sr = sf.read(tmp / "out.wav", dtype="float32", always_2d=True)
    rms = lambda a: np.sqrt(np.mean(a ** 2))  # noqa: E731
    a = db(rms(y[int(SR * 0.5):int(SR * 2.5), 0]) / rms(quiet[int(SR * 0.5):int(SR * 2.5)]))
    b = db(rms(y[int(SR * 3.5):int(SR * 5.5), 0]) / rms(loud[int(SR * 0.5):int(SR * 2.5)]))
    check("import normalising is one fixed gain (dynamics kept)", "linear=true" in af and abs(a - b) < 0.5,
          f"quiet part {a:+.1f} dB, loud part {b:+.1f} dB")
    check("import output is 48 kHz", sr == SR)

    # the whole import (cut + normalise + save) with the download swapped for a local file
    src = sine(440, 4, 0.05, 44100)
    sf.write(tmp / "dl.wav", np.stack([src, src], 1), 44100, subtype="PCM_16")

    class FakeYDL:
        def __init__(self, opts):
            self.dest = Path(opts["outtmpl"].replace("%(id)s.%(ext)s", "x.wav"))

        def __enter__(self):
            return self

        def __exit__(self, *a):
            pass

        def extract_info(self, url, download=True):
            shutil.copy(tmp / "dl.wav", self.dest)
            return {"title": "Test tone", "id": "x"}

    real = importer.yt_dlp.YoutubeDL
    importer.yt_dlp.YoutubeDL = FakeYDL
    importer._save_thumbnail = lambda *a: None
    try:
        out = importer.import_clip("https://youtu.be/x", start="0:01", end="0:03", folder=tmp / "imp", log=lambda m: None)
    finally:
        importer.yt_dlp.YoutubeDL = real
    y, sr = sf.read(out, dtype="float32", always_2d=True)
    lvl = db(np.sqrt(np.mean(y[SR // 4:-SR // 4] ** 2)))
    check("a full import: trimmed to 2 s, 48 kHz stereo, at about -16 LUFS, clean",
          sr == SR and y.shape[1] == 2 and abs(len(y) / SR - 2) < 0.02 and -19 < lvl < -13
          and spur_db(y[:, 0], 440) < -60 and np.abs(y).max() < 0.9,
          f"{len(y) / SR:.2f} s, rms {lvl:+.1f} dBFS, junk {spur_db(y[:, 0], 440):.0f} dB")


if __name__ == "__main__":
    sys.exit(main())
