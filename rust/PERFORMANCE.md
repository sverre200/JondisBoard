# Performance and audio: before (Python) and after (Rust)

Measured on the development PC (Windows 11, AMD GPU, 144 Hz display) on 2026-11-05, with
throwaway libraries from `bench/make_libraries.py`: 20, 100, 500 and 1,000 sounds of 1-4 s
(48 kHz stereo 16-bit), spread over 6 groups and No group, every third with a picture. Both
builds' configs name audio devices that don't exist, so no stream opens in either. Raw results:
`bench/results/`. Python = the shipped PyInstaller `JondisBoard.exe` (startup/idle/memory) and
the app from source with its audio stubbed (interactions). Rust = the release build.

## Startup, idle and memory (from outside the process: `bench/measure_exe.py`)

Window on screen (ms after launch), cold run / warm runs:

| Sounds | Python | Rust |
|---|---|---|
| 20 | 2,911 / 2,269 | 112 / 27–47 |
| 100 | 2,772 / 2,623 | 177 / 40–43 |
| 500 | 5,386 / 4,727 | 396 / 97–104 |
| 1,000 | 7,313 / 8,032 | 505 / 146–150 |

Settled (stopped using CPU), warm: Python 3.1 s / 4.1 s / 7.3 s / 13.0 s; Rust 0.33 s / 0.34 s /
0.40 s / 0.70 s. Inside the Rust process the first frame is drawn 88 / 101 / 158 / 212 ms after
`main()` starts (most of the 1,000-sound time is reading 1,000 WAV headers for durations).

Idle CPU (one core, 10 s after settling, window open): Python 3.0–6.1 %; Rust 0.2–1.1 % in
the outside harness, which still catches the end of the startup toast and the pointer passing
over the window. With nothing happening at all, the Rust process used **0 ms of CPU in 20 s**
(500 sounds, normal and hidden mode, measured with the process's own CPU counter): no frames,
no timers, no polling while idle.

Memory (working set / private, after loading):

| Sounds | Python | Rust | of which decoded audio (Rust) |
|---|---|---|---|
| 20 | 101 / 554 MB | 58 / 72 MB | 9 MB |
| 100 | 183 / 637 MB | 98 / 116 MB | 46 MB |
| 500 | 592 / 1,049 MB | 292 / 311 MB | 228 MB |
| 1,000 | 1,100 / 1,562 MB | 533 / 554 MB | 457 MB |

Rust keeps 16-bit clips as 16-bit (half of Python's float32) and caps decoded audio at 1 GiB
(clips past it are decoded when pressed), so memory is the clips plus ~50 MB. Decoding runs on
a background thread after the window is up: 12 / 50 / 242 / 455 ms of thread time.

## Interactions (inside the app)

Rust: `JondisBoard.exe --bench` changes state and draws one frame. "build" is the CPU time to
draw it; "frame" adds handing it to Windows, which waits for the next refresh (6.9 ms on this
144 Hz screen). Python: the same change followed by Tk's `update()` (until drawn).

| | Python 20 / 100 / 500 / 1,000 | Rust build (median, max) 20 / 1,000 | Rust frame 1,000 |
|---|---|---|---|
| Group switch | 32 / 64 / 60 / 97 ms (max 190) | 0.17 / 1.47 ms (max 1.7) | 5.9 ms |
| Search (typing) | 26 / 69 / 281 / 723 ms | 0.19 / 1.71 ms (max 2.0) | 7.5 ms |
| Clear search | 37 / 76 / 300 / 860 ms | — | 0.01–0.02 ms build |
| Resize step | 265 / 520 / 1,594 / 7,605 ms | — | 3.9 / 5.0 ms per step (incl. present) |
| Theme switch | 1,332 / 2,053 / 5,458 / 9,735 ms (rebuilds the window) | — | **6.9–7.6 ms click to new frame** |
| Scroll (wheel) | not measured | 0.77 / 1.46 ms | 7.0 ms |
| Drag (60 pointer moves) | not measured | 0.76 / 1.43 ms | 7.0 ms = 144 fps (display-limited) |

Every interaction is under 2 ms of CPU at 1,000 sounds, so 60 fps (16.7 ms) has >8x headroom and
the display's own refresh is the limit. Nothing paints in: each change is one complete frame.

Audio callback cost (`OutputMixer::render`, 8 clips at once, 480-frame block = 10 ms of audio):
**1.6 µs median, 2.3 µs p95, ~25 µs max** (the Python budget was < 500 µs). On the real
headphones stream: 3 µs average, 34 µs max, 0 underflows in 200 blocks (`tests/wasapi_smoke.rs`).

## Audio verification

| Check | Result |
|---|---|
| Rust vs Python engine, sample by sample (`bench/compare_engines.py`), quiet tone 100 % | **bit-exact** |
| same, music-like noise at 80 % | **bit-exact** |
| same, loud signal at 150 % (peak guard working) | max difference 1.2e-7 (one float step), same peak −1.0 dBFS, 0 clipped |
| `--audio-check offline` on a copy of the real library (Clips 78 %, ears 43 %) | every signal and clip = source x one fixed gain (residual −151 to −155 dB), 0 flat-topped samples, headphones = cable sample for sample |
| Overlap test (two copies of the transients) | a hard clip would clip 2 samples (78 %) / 100 samples (150 %); the engine peaks at −1.0 dBFS with 0 clipped |
| Unit tests | guard bit-exact under the ceiling and 1 ms late; loud mixes under −1 dBFS without clipping; gain recovers; mic cushion, gap fade, drift skip; restart, stop, solo/toggle rules; i16 mixing exact; finished clips freed off the audio thread |
| Imports | real ffmpeg: 44.1 kHz mono → 48 kHz stereo WAV, cut, two-pass linear loudnorm |
| **Loopback through the real VB-CABLE** | **not run**: Discord was open all session and the check refuses then. Run `JondisBoard.exe --audio-check loopback` when nobody is in voice. The Python app's loopback on the same endpoints measured flat 31 Hz–19 kHz, THD+N −89 dB, 0 dropouts, 0 ppm drift; the Rust streams use the same shared-mode conversion path. |

## Targets

| Target | Result |
|---|---|
| Near-zero idle CPU, event-driven | 0 ms CPU in 20 s idle |
| 60 fps interactions, no paint-in or flicker | ≤ 2 ms per frame at 1,000 sounds; one frame per change, display-rate presents |
| Instant group switches and search | ~1.5 ms + one refresh (vs 97 ms / 723 ms) |
| Fast startup, lazy loading | window in 0.03–0.5 s (vs 2.3–8 s); headers only at startup, decode in background |
| Predictable memory | clips (16-bit) + ~50 MB, capped at 1 GiB of decoded audio |
| No hundreds of native child windows | one window, zero child windows |
| Theme switch ≈ 100 ms, nothing lost | ~7 ms |
