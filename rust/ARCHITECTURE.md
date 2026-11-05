# JondisBoard (Rust): architecture

The native rewrite of the Python app in `src/`. Same features, same data on disk, same look;
no Python, no runtime, one ~1.2 MB exe plus the two helper programs it calls for imports
(`ffmpeg.exe`, `yt-dlp.exe`).

## 1. Choices and why

### GUI: raw Win32 + Direct2D/DirectWrite (no framework)

| Option | For | Against (for this app) |
|---|---|---|
| **Win32 + Direct2D/DirectWrite** (chosen) | Full control of the window: DWM title-bar colours, AppUserModelID, taskbar icon swaps, cloaking, `WS_EX_NOACTIVATE`-style behaviour, occlusion. Zero idle cost (draws only on `WM_PAINT`). ClearType-quality Segoe UI identical to the Python app. ~1.2 MB exe, ~80 ms to first frame. | We write the widgets (buttons, menus, text field, sliders) ourselves. |
| egui / eframe | Fast to build, immediate mode, has text input. | Its own look and text rendering; multi-window/popup control is limited; wgpu/glow backend adds startup time and ~30-60 MB; continuous-repaint habits need care. |
| Slint | Declarative, good tooling. | Styling the exact existing look is fighting the toolkit; window/taskbar control is indirect. |
| iced | Elm-style, nice architecture. | Same window-control and custom-look issues; heavier. |

"GPU is always lighter" isn't assumed: Direct2D's hardware target costs nothing when idle (no
frames are drawn), and while animating a full frame takes ~1-2 ms of CPU. The decisive points
were idle cost, startup, exact control of Windows behaviour, and that the Python app already
depended on Win32 tricks that frameworks hide.

**Everything is drawn in one window.** Menus, the Settings panel, dialogs, toasts and the drag
copy are drawn inside the main window instead of being separate popup windows. That removes the
Python app's whole class of popup problems (focus stealing, floating over the game, layered
window repaint costs, the 1 % opacity warm-up): nothing can float over a game because nothing
exists outside the window. The trade-off is that a menu can't extend past the window edge (it's
kept inside, and the minimum window size is 640x480 DIPs).

### Audio: WASAPI directly (no CPAL, no PortAudio)

Shared-mode, event-driven WASAPI with `AUTOCONVERTPCM | SRC_DEFAULT_QUALITY`: every stream is
float32 48 kHz (stereo out, mono mic) and Windows converts to the device's own format, which is
what the DirectSound host did for the Python app. Endpoint friendly names are the same strings
DirectSound showed, so saved `mic`/`cable`/`monitor` settings still match. Each stream thread is
registered with MMCSS ("Pro Audio"). CPAL would add a layer whose format negotiation and
buffer sizes we'd have to work around; we needed exact control and no allocation in callbacks.

### Decoding: own WAV reader + ffmpeg (no Symphonia)

48 kHz mono/stereo WAV (every imported clip) is read directly and bit-exactly; 16-bit clips stay
16-bit in memory (half the Python app's float32 RAM; the mixer converts with the exact same
x/32768). Everything else (MP3, M4A, Opus, other rates, 5.1) goes through ffmpeg with the tuned
resampler `paths::RESAMPLE` and a proper downmix, exactly as before. Symphonia would decode
MP3/AAC, but resampling would then need our own resampler, which is what the Python history
warns against (the numpy one left -11 dB junk). ffmpeg ships with the app anyway for imports.

### Pictures: WIC (built into Windows)

Decoding (PNG/JPG/WebP/GIF/BMP, EXIF rotation) and the 256x256 square crop + PNG save use WIC.
No image crate needed.

## 2. Layout of the code (`rust/src`)

```
main.rs         entry: AppUserModelID, logging, panic hook, --data-dir / --bench / --audio-check
lib.rs          the library crate (everything below is testable without the binary)
config.rs       config.json: lenient per-key reading, unknown keys kept, atomic save + one backup
library.rs      sounds/: scan, groups, order merge, move/rename/delete, group ops (pure file work)
paths.rs        data dir (next to the exe, or --data-dir / JONDISBOARD_HOME), helper tools
theme.rs        the 6 themes, derived tokens, ThemeManager
hotkeys.rs      WH_KEYBOARD_LL hook thread: scan code + numpad + mods, record mode
importer.rs     yt-dlp + ffmpeg pipeline, two-pass linear loudnorm, link/time parsing
diag.rs         --audio-check offline | loopback | record | render
log.rs          soundboard.log
audio/
  wav.rs        WAV reader/writer
  decode.rs     load(): direct WAV or ffmpeg; quiet_command() for helper processes
  mixer.rs      real-time core: OutputMixer, PeakGuard, MicRing, PlayState
  spsc.rs       lock-free single-producer/single-consumer queue
  wasapi.rs     devices, render and capture threads
  engine.rs     Player: decoded-clip cache + background loader, playback rules, streams
ui/
  window.rs     the Win32 window, message dispatch, DPI, icons/title bar, timers
  app.rs        App: all state and actions (no drawing)
  view.rs       drawing + layout + hit regions (one Direct2D frame)
  input.rs      mouse/keyboard, menus, drag and drop
  gfx.rs        Direct2D/DirectWrite wrapper, text-layout cache, offscreen canvas
  anim.rs       clock-timed tweens, motion tokens
  textfield.rs  single-line text field (caret, selection, clipboard)
  icons.rs      the line icons, as vectors
  logo.rs       the waveform logo: geometry, rasteriser, HICONs
  images.rs     WIC: picture loading thread, square-crop PNG save
  shell.rs      clipboard, file pickers, ShellExecute, Recycle Bin
  bench.rs      --bench scripted performance run
```

## 3. Threads and data flow

```
UI thread        window + App (owns all UI state). Never blocks on audio or files for long.
hotkey thread    low-level keyboard hook; calls Player::press directly (a hotkey plays even
                 if the UI thread is busy), the tile flash comes back as an event
audio threads    cable out (+ mic in), headphones out: OutputMixer::render only
clip loader      decodes queued clips at below-normal priority within a 1 GiB budget
picture loader   decodes tile pictures at their on-screen size
import workers   one per import: yt-dlp, ffmpeg, picture
```

Background threads never touch `App`: they send a `Msg` on a channel and `PostMessage` the
window (`WM_WAKE`), which drains it on the UI thread.

**Real-time rules** (`mixer.rs`): `render()` does no allocation, no I/O, no locks. Commands
(play/stop) arrive on an SPSC queue; a finished clip's `Arc` goes back on another queue and is
freed on the control side; progress is published in atomics; the mic arrives through `MicRing`
(atomic indices, f32 bits in relaxed atomics, jitter cushion, one-time skip on drift). Volumes
are atomics read each block.

**Lazy loading**: startup only reads WAV headers (for durations). The loader decodes clips in
board order (hotkeyed ones first) until the memory budget; a clip pressed before it's decoded
jumps the queue and plays as soon as it's ready (if within 3 s).

## 4. The UI model

One `paint()` builds a whole frame: every clickable thing registers its rect (`App::hits`) while
it's drawn; mouse input is tested against the last frame's hits (topmost last). There are no
widget objects to keep in sync, so a theme switch, group switch or search is "change the state,
draw one frame".

Repaint policy (measured, see PERFORMANCE.md):
- idle: no frames at all, no timers (0 % CPU);
- short animations (hover fades, slides, menu/dialog fades, flashes): frames at the display rate
  until they settle;
- long indicators (a playing sound's progress bar, import spinners): ~30 fps on a timer;
- covered by a game or minimised (`CheckWindowState` = occluded): at most 4 checks a second.

Time-based logic (toast timeout, playback polling, drag auto-scroll) also runs at the start of
each frame, because continuous `WM_PAINT`s starve `WM_TIMER` (a bug found and fixed in testing).

The window state is a `RefCell` behind `GWLP_USERDATA`. Windows calls the window procedure
re-entrantly (releasing capture, modal file pickers, `WM_SETICON`); a nested call that finds the
state borrowed defers instead of aliasing it (paint later, re-post resizes). Mouse capture and
bench resizes are applied after the handler returns.

Drag and drop: a press becomes a drag after 6 DIPs; the dragged tile's slot follows the pointer
and the other tiles slide out of the way live (220 ms); over a group tab the board returns to
its order and the tab lights up; Esc cancels. Tiles play on release only, and a click that
closes a menu never plays.

## 5. The theme system

- `theme.rs` holds the 6 themes (the same 11 tokens: BG, SURFACE, CARD, CARD_HOVER,
  CARD_PLAYING, BORDER, TEXT, MUTED, ACCENT, ACCENT_HOVER, ACCENT_TEXT), the derived ones
  (ELEVATED, EDGE, ACCENT_SOFT, ACCENT_SOFT_HOVER, ACCENT_EDGE, computed with the Python
  `mix()` formula, unit-tested against its values), the fixed GOOD `#3ddc97`, WARN `#ffb547`,
  DANGER `#f04566` (hover `#d63655`) and the 8 group colours.
- `ThemeManager` is the one runtime owner of the current theme, with a generation counter.
- Nothing caches a colour: every frame reads `App::theme()`. Switching = `ThemeManager::set` +
  title-bar colours (DWM caption/text/border attributes) + new window/taskbar icons
  (`WM_SETICON`, logo rasterised per theme and size) + one repaint. On a new generation the
  colour tweens are dropped, so the change is instant instead of fading.
- Nothing else changes: group, search, scroll position (per tab), window position, playback,
  devices, hotkeys and open menus' state are untouched. Measured: see PERFORMANCE.md.
- The taskbar icon follows because the process sets its own AppUserModelID before any window
  exists (a shortcut launch would otherwise pin the exe's icon).

## 6. `unsafe` code

All of it is FFI (Win32/COM) or the SPSC queue; there's no global mutable state except the
hook thread's thread-local (a low-level hook callback has no user pointer). Every block has a
`// SAFETY:` comment. Where it lives:

| File | What |
|---|---|
| `audio/spsc.rs` | the queue's slots (`UnsafeCell<MaybeUninit<T>>`, ordered by head/tail atomics) |
| `audio/wasapi.rs` | COM device/stream calls; the device buffer slice for exactly `frames` frames |
| `hotkeys.rs` | hook install/remove, `KBDLLHOOKSTRUCT` read, `PostThreadMessage` |
| `ui/window.rs` | window class/creation/dispatch, `GWLP_USERDATA` box, DWM attributes, capture |
| `ui/gfx.rs` | Direct2D/DirectWrite calls |
| `ui/images.rs` | WIC decode/encode |
| `ui/shell.rs` | clipboard (GlobalLock), IFileOpenDialog, ShellExecute, SHFileOperation |
| `ui/logo.rs` | DIB section + CreateIconIndirect |
| `ui/bench.rs`, `diag.rs`, `main.rs`, `ui/app.rs` | single queries (memory, DPI, console, process list) |

`mixer.rs` (the real-time core) has no `unsafe` at all.

## 7. Testing

- `cargo test --lib`: 51 unit tests (config, library ops, mixer/peak guard/mic ring, WAV,
  importer parsing + a real ffmpeg convert, spsc, anim, text field, logo, DIB, geometry).
- `tests/ui_flows.rs`: the real window and app driven with synthetic input (46 checks).
- `tests/ui_snapshots.rs`: every UI state rendered offscreen to PNGs.
- `tests/compat_real.rs`: a copy of a real Python install opens unchanged.

`JONDISBOARD_TEST=1` (set by those tests) keeps the window cloaked (never on screen), opens no
audio devices, replaces the clipboard with a stand-in, never grabs the mouse and doesn't install
the keyboard hook, so the tests are safe to run mid-game.
