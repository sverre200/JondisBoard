# JondisBoard (Rust)

The native Windows version of JondisBoard: one `JondisBoard.exe`, no Python. Friends get a
ZIP (`JondisBoard.exe`, `ffmpeg.exe`, `yt-dlp.exe`, `READ ME FIRST.txt`) and unzip it; it reads
the same `sounds\` folder and `config.json` as the Python version, so it can replace it in place.

- [ARCHITECTURE.md](ARCHITECTURE.md): stack choices, modules, threads, the theme system, `unsafe`
- [PARITY.md](PARITY.md): feature checklist against the Python app, tested vs not
- [PERFORMANCE.md](PERFORMANCE.md): before/after measurements and audio verification

## Build from source

Needs the Rust toolchain (`rustup`, stable, MSVC target) and the Windows SDK's `rc.exe`
(installed with Visual Studio Build Tools, "Desktop development with C++").

```bash
cd rust
```

```bash
cargo build --release
```

The exe is `rust\target\release\JondisBoard.exe`. Run it from the project folder with your data:

```bash
target\release\JondisBoard.exe --data-dir ..
```

or put it next to `sounds\` and `config.json`. Don't copy it over a running `JondisBoard.exe`.

## Test

```bash
cargo test
```

Unit tests plus the UI suites (`tests\ui_flows.rs`, `tests\ui_snapshots.rs`). The UI suites run
the real window cloaked (never on screen), with no audio devices, a stand-in clipboard and no
keyboard hook, so they're safe while a game is running. Snapshots go to `%TEMP%\jb_snapshots`
(or `JB_SNAP_DIR`). Compatibility with a real install (on a COPY of it):

```bash
set JB_REAL_COPY=C:\path\to\copy && cargo test --release --test compat_real -- --nocapture
```

## Audio diagnostics

```bash
JondisBoard.exe --audio-check offline
```

Safe any time (nothing is played). `--audio-check loopback` and `--audio-check record 20` use the
real VB-CABLE and refuse while DayZ or Discord is running (`--force` if nobody is in voice).
Results go to `build\audio_check\<time>\`. `rust\bench\compare_engines.py` compares the Rust
mixer with the Python engine sample by sample.

## Performance runs

```bash
python rust\bench\make_libraries.py %TEMP%\jb_bench
```

then `rust\bench\measure_exe.py <exe> <library>` (startup, idle CPU, memory, from outside) and
`JondisBoard.exe --data-dir <library> --bench out.json` (group switch, search, scroll, drag,
theme, resize, mixer cost, decode). Windows open on screen: not while gaming.

## Release ZIP

Put the standalone `yt-dlp.exe` (from https://github.com/yt-dlp/yt-dlp/releases) in
`rust\vendor\`, then:

```bash
powershell -ExecutionPolicy Bypass -File rust\release\make_release.ps1
```

It writes `rust\dist\JondisBoard-<version>.zip`. ffmpeg comes from `rust\vendor\ffmpeg.exe` or PATH.

## Command line

| | |
|---|---|
| `--data-dir <folder>` | use that folder's `sounds\` and `config.json` (also `JONDISBOARD_HOME`) |
| `--bench <out.json>` | scripted performance run, then exit |
| `--audio-check offline\|loopback\|record <s>\|render <in> <out> [vol]` | audio diagnostics |
