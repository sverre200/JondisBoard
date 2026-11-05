"""Sample-by-sample comparison of the Python engine and the Rust mixer (offline, no devices).

    .venv\\Scripts\\python.exe rust/bench/compare_engines.py <JondisBoard.exe> <work folder>

Renders test signals through the Python engine's cable output (its real callback, in 480-frame
blocks, 1 ms lookahead removed) and through `JondisBoard.exe --audio-check render` (the Rust
cable mixer, same blocks), then reports the largest difference and the peaks. Cases: a quiet
tone (pass-through must be bit-exact), music-like noise at 80 %, and a loud overlap that the
peak guard has to catch (clips at 150 %, two copies).
"""
import json
import subprocess
import sys
from pathlib import Path

import numpy as np
import soundfile as sf

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "src"))
import engine  # noqa: E402

SR = 48000
exe, work = Path(sys.argv[1]), Path(sys.argv[2])
work.mkdir(parents=True, exist_ok=True)


class Status:
    output_underflow = input_overflow = False


def py_render(x, vol, copies=1):
    e = engine.Engine()
    e.cable_open = e.monitor_open = True
    e.clip_volume, e.mic_passthrough = vol, False
    for i in range(copies):
        e.play(f"x{i}", x.copy())
    n, pad = len(x), engine.PeakGuard.ATTACK
    out = np.zeros((n + pad, 2), np.float32)
    for i in range(0, n + pad, 480):
        e._cable_cb(out[i:i + 480], len(out[i:i + 480]), None, Status())
    return out[pad:]


def rust_render(x, vol, name):
    src, dst = work / f"{name}_in.wav", work / f"{name}_rust.wav"
    sf.write(src, x, SR, subtype="FLOAT")
    subprocess.run([str(exe), "--audio-check", "render", str(src), str(dst), str(vol)], check=True, timeout=60)
    y, sr = sf.read(dst, dtype="float32", always_2d=True)
    assert sr == SR
    return y


rng = np.random.default_rng(3)
t = np.arange(SR * 3) / SR
tone = (0.3 * np.sin(2 * np.pi * 440 * t)).astype(np.float32)
noise = rng.standard_normal(SR * 3).astype(np.float32)
noise = (0.3 * noise / np.abs(noise).max()).astype(np.float32)
loud = (0.9 * np.sin(2 * np.pi * 120 * t) * (0.5 + 0.5 * np.sin(2 * np.pi * 0.7 * t))).astype(np.float32)
cases = [
    ("quiet_tone_100", np.stack([tone, tone], 1), 1.0, 1),
    ("noise_80", np.stack([noise, -noise], 1), 0.8, 1),
    ("loud_150", np.stack([loud, loud], 1), 1.5, 1),
]
results = {}
for name, x, vol, copies in cases:
    a = py_render(x, vol, copies)
    b = rust_render(x, vol, name)
    n = min(len(a), len(b))
    d = np.abs(a[:n] - b[:n])
    results[name] = {
        "max_abs_diff": float(d.max()),
        "bit_exact": bool(np.array_equal(a[:n], b[:n])),
        "python_peak": float(np.abs(a).max()),
        "rust_peak": float(np.abs(b).max()),
        "python_clipped": int(np.count_nonzero(np.abs(a) >= 0.999)),
        "rust_clipped": int(np.count_nonzero(np.abs(b) >= 0.999)),
        "frames": n,
    }
print(json.dumps(results, indent=2))
