# JondisBoard: full overview

A guide to what the app is, how it works and how it's built. It's written so a designer (or an AI helping with design) can change the app without breaking it. For build and test commands see [DEVELOPING.md](DEVELOPING.md); for the hard-won technical rules see [CLAUDE.md](CLAUDE.md).

---

## 1. What it is

JondisBoard is a **Windows desktop soundboard** for a small group of friends. Click a sound (or press its hotkey, even inside a fullscreen game) and it plays **into Discord and games as if it came from your microphone**, mixed with your real voice, while you also hear it in your headphones.

```
Your mic ──┐
           ├─► JondisBoard (mixer) ─► VB-CABLE ─► Discord / game  (their mic = "CABLE Output")
Clips ─────┘         └──────────────► your headphones
```

**Who uses it:** a handful of friends gaming together (DayZ, Discord). Not a commercial product. It's shared as a zip; there's no installer, account or server.

**Main jobs:**
1. Get clips in fast: paste an Instagram reel / YouTube link → it becomes a sound (Quick import).
2. Find and fire them fast: big tile grid, groups as tabs, search, global hotkeys.
3. Stay out of the way of the game: no focus stealing, nothing popping over the game, low CPU.

---

## 2. Tech stack

| Part | Library |
|---|---|
| Language | Python 3.12 |
| Window / UI | **customtkinter 6** on top of Tk, plus raw Win32 calls through `ctypes` (rounded popups, shadows, layered windows, screen copies) |
| Images | Pillow (tile cards, icons, logo are drawn as images) |
| Audio | sounddevice (PortAudio, DirectSound host), soundfile, numpy |
| Hotkeys | `keyboard` (global low-level hook, works in games) |
| Imports | yt-dlp (+ yt-dlp-ejs, Node.js for YouTube) and ffmpeg |
| Shipping | PyInstaller one-file `JondisBoard.exe` (~38 MB) + `Uninstall JondisBoard.exe` (~19 MB) |

**Important for design:** this is **not a web app**. There's no HTML/CSS. Everything is Tk widgets, Tk canvases and Pillow images. Rounded corners, shadows, gradients and animation are all done by hand (see §7).

---

## 3. Folder layout

```
JondisBoard/
├─ src/
│  ├─ app.py        ~3,700 lines: the whole UI (window, board, tiles, menus, dialogs, Settings, drag and drop, animation)
│  ├─ engine.py     audio devices and the real-time mixer
│  ├─ importer.py   download (yt-dlp), trim + normalise (ffmpeg), thumbnails, pictures
│  ├─ hotkeys.py    global hotkeys and "press a key to record it"
│  ├─ themes.py     the 6 colour themes (shared with the uninstaller)
│  ├─ logo.py       the waveform logo, drawn per theme and size
│  ├─ paths.py      where data and bundled assets live (source vs exe)
│  └─ uninstall.py  the uninstaller window
├─ assets/          icon.ico / icon.png (bundled into the exe), share_readme.txt ("READ ME FIRST.txt")
├─ scripts/         build.ps1, install_build.ps1, make_icon.py, make_share.py
├─ tests/           UI test suites (run_all.py) driving the real app with fake audio
├─ README.md        user install guide
├─ DEVELOPING.md    developer setup/build/test
└─ CLAUDE.md        technical rules and gotchas
```

**User data** lives next to the exe (not in git):
- `sounds/`: the clips. **A group is a subfolder**; a clip's picture is a `.png` with the same name.
- `config.json`: settings (below).
- `soundboard.log`: errors.

---

## 4. Data model

There's no database. **The file system is the data model:**

```
sounds/
├─ Chinese Rap.wav          ← clip in "No group"
├─ Chinese Rap.png          ← its picture (optional)
├─ THE LION/                ← a group
│  ├─ To the gossiper.wav
│  └─ To the gossiper.png
└─ _deleted/                ← deleted clips go here (folders starting with "_" are hidden)
```

A clip's identity is its path inside `sounds/` (e.g. `"THE LION/To the gossiper.wav"`). Renaming or moving a clip renames/moves files, and the app carries its hotkey and position along.

`config.json` keys:

| Key | Meaning |
|---|---|
| `mic`, `cable`, `monitor` | device names: your real mic, the VB-CABLE input, your headphones |
| `mic_volume`, `clip_volume`, `monitor_volume` | the three sliders (0–1+) |
| `mic_passthrough` | "Send my voice" on/off |
| `hotkeys` | clip path → key binding (`{scan, keypad, mods, label}`) |
| `stop_hotkey` | binding for Stop all |
| `group` | the tab that was open last |
| `group_colors` | group → colour (from an 8-colour palette) |
| `order` | the clip order you dragged into (new clips go last) |
| `solo` | "One sound at a time" |
| `toggle_stop` | "Press a playing sound to stop it" |
| `cookies_from_browser` | e.g. `"firefox"` when Instagram demands a login |
| `theme` | theme name |
| `geometry` | window size/position |

---

## 5. How it works at runtime

### Threads
- **UI thread:** Tk main loop. All widgets are touched only here.
- **Audio threads:** PortAudio callbacks (mic in, cable out, headphones out). They only mix numpy arrays under a lock.
- **Keyboard hook thread:** `keyboard` library; a hotkey calls `App.play()`, which hands the tile flash back to the UI thread with `root.after(0, ...)`.
- **Import worker threads:** one per import (download + ffmpeg); results are posted back to the UI thread.

### Audio (`engine.py`)
- Everything runs at **48 kHz stereo float32**, DirectSound host (full device names, automatic resampling), low latency.
- Clips are fully decoded into memory when sounds load (`load_audio`, ffmpeg for odd formats).
- Two voice lists: **cable** and **monitor**. `play(name)` adds the clip to both (restarting it if it's already playing).
- Cable callback = `clips × clip_volume + mic × mic_volume` (if Send my voice is on). Headphone callback = `clips × monitor_volume` (you don't hear yourself). Each output then goes through its own `PeakGuard`, a 1 ms lookahead limiter at −1 dBFS that does nothing (bit-exact) unless overlapping clips or high volumes would go over. There's no hard clipping.
- Files that aren't 48 kHz (or have more than 2 channels) are converted by ffmpeg's high-quality resampler/downmix (`paths.RESAMPLE`). The mic reaches the cable through a ring buffer with a small jitter cushion (`MicBuffer`). Callbacks allocate nothing and count dropouts (`Engine.stats()`, written to the log).
- `scripts/audio_check.py` measures all of this (offline, or through the real cable with `loopback` / `record`).
- Safety: a mic set to a CABLE device is refused (feedback loop); monitor = cable is refused.
- `progress()` returns how far each playing clip is; the UI polls it to draw the play bar on tiles.

### Playback rules (`App.play`)
1. If **Press a playing sound to stop it** is on and the clip is playing → stop it.
2. Else if **One sound at a time** is on → stop everything first.
3. Play; flash the tile.

Tiles play on **mouse release**, not press, so a press that turns into a drag never fires a sound into the game.

### Hotkeys (`hotkeys.py`)
Global hook on scan codes (keypad keys distinguished, modifiers included, autorepeat ignored). "Record" mode captures the next key press; Esc cancels. Hotkeys work in games; some games need JondisBoard run as administrator.

### Importing (`importer.py`)
1. yt-dlp downloads the best audio.
2. ffmpeg cuts `[start, end]`, converts to 48 kHz stereo WAV and loudness-normalises to −16 LUFS as one fixed gain change (two-pass linear `loudnorm`, so the clip's dynamics stay intact).
3. The video thumbnail becomes the clip's picture.
4. Name: Instagram caption without hashtags, or the YouTube title.

**Quick import** (button or Ctrl+V): reads the link from the clipboard, shows a **PendingTile** in the grid right away (spinner → done / failed with Retry / Dismiss), and saves the clip into **No group**. **Import link…** opens a dialog for name, trim times and group.

---

## 6. The UI, piece by piece

```
┌──────────────────────────────────────────────────────────────────────────┐
│ [logo] JondisBoard  23 sounds   (status pill)          [● Theme] [Settings]│  header
├──────────────────────────────────────────────────────────────────────────┤
│ [⚡ Quick import] [Import link…] [♪ Add files] [⋯]      [🔍 Search  Ctrl+F] │  toolbar
├──────────────────────────────────────────────────────────────────────────┤
│ (All) (No group) (● THE LION) (● Memes) (+ New group)                     │  group tabs
├──────────────────────────────────────────────────────────────────────────┤
│ ┌──────────────┐ ┌──────────────┐ ┌──────────────┐ ┌──────────────┐       │
│ │[pic] Name    │ │[pic] Name    │ │[pic] Name    │ │[pic] Name    │       │  sound grid
│ │      0:04 F5 │ │      0:12    │ │      0:03 N1 │ │      0:07    │       │  (tiles)
│ │▬▬▬▬▬▬▬▬▬▬▬▬▬ │ │              │ │              │ │              │       │
│ └──────────────┘ └──────────────┘ └──────────────┘ └──────────────┘       │
├──────────────────────────────────────────────────────────────────────────┤
│ Mic ──●──── 60%   Clips ───●── 80%   In your ears ──●── 43%  [Send my voice]  [Stop all]│ footer
│                                                                    F12    │
└──────────────────────────────────────────────────────────────────────────┘
```

### Header
Waveform logo (recoloured per theme), title, sound count, a status pill (green/amber/red: audio OK, warnings, errors), **Theme** button (menu of 6 themes with colour dots) and **Settings** button.

### Settings (popover)
Its own window that glides down from the Settings button.
- **Audio:** Your microphone, Send to (CABLE Input), Your headphones (custom `Select` dropdowns), Restart audio. If VB-CABLE isn't installed: a help box with a "Get VB-CABLE" button.
- **Playback:** two animated toggle switches: One sound at a time; Press a playing sound to stop it.

### Toolbar
Quick import (primary, accent colour), Import link…, Add files, ⋯ menu (Open sounds folder, Reload sounds, Sort sounds A–Z, Stop all hotkey…), and a search field (Ctrl+F; Esc clears).

### Group tabs
**All**, **No group**, one tab per group with its colour dot, **+ New group**. Clicking the open tab again opens its menu (colour, rename, delete). Dropping a dragged tile on a tab moves the clip into that group.

### Sound tiles
250×80 px minimum (columns stretch to fill). Layout: **picture** (52 px square, or a tinted placeholder) | **name** (up to 2 lines, clamped with …) / **info row** (duration, hotkey) | **progress bar** at the bottom while playing. States: normal, hover, pressed, playing (`CARD_PLAYING` + bar), flash when fired, dragging (lifted), drop target. Right-click menu: Play, Set / Change hotkey, Clear hotkey, Move to group (groups, No group, New group…), Picture (Choose / Paste / Remove), Rename…, Delete.

### Footer
Three sliders with live % labels: **Mic** (your voice to others), **Clips** (clip loudness to others), **In your ears** (what you hear). **Send my voice** toggle. **Stop all** (red) with its hotkey shown underneath (click to set).

### Other windows
- **Dialogs** (`App._dialog`): Import link, rename, new group, confirm delete, "press a key" for hotkeys. Themed, fade in/out.
- **Toasts:** small messages centred just above the footer, owned by the main window (never over a game).
- **Menus** (`PopupMenu`): rounded, shadowed, keyboard-free, hover trails, check marks, colour dots, separators, headers.

### Drag and drop (`Dragger`)
Press and move a tile past a few px → it lifts and follows the mouse; other tiles slide to make room; drop to re-order (saved in `order`), or drop on a group tab to move it. Sort A–Z re-orders with the same slide animation.

---

## 7. Design system

### Colour tokens (`themes.py`)
Each theme defines the same 11 tokens:

| Token | Use |
|---|---|
| `BG` | window background |
| `SURFACE` | bars, popovers, menus |
| `CARD` / `CARD_HOVER` / `CARD_PLAYING` | tiles and their states |
| `BORDER` | hairlines, outlines |
| `TEXT` / `MUTED` | primary / secondary text |
| `ACCENT` / `ACCENT_HOVER` / `ACCENT_TEXT` | primary buttons, focus, highlights, logo |

Plus fixed colours: `GOOD #3ddc97`, `WARN #ffb547`, `DANGER #f04566` (hover `#d63655`), and 8 group colours (`#7c5cff #3b82f6 #14b8a6 #22c55e #eab308 #f97316 #ef4444 #ec4899`).

Themes: **Toxic Green** (default, matches the icon), Midnight Purple, Ocean Blue, Sunset Orange, Crimson, Graphite. All are dark. Switching theme **rebuilds the window** (`App.switch_theme`), because widgets read the colours when they're built. The window icon, taskbar icon and title bar colour follow the theme.

### Type
Segoe UI everywhere (`font(size, weight)`): title 22 bold, section headers 14 bold, body 13, small/meta 12.

### Components (reuse these, don't hand-roll)
- `button(master, text, command, kind, icon_name=…)`: kinds `primary`, `secondary`, `ghost`, `danger`; auto width; pressed look; height 36.
- `icon(name, color, size)`: hand-drawn icons: bolt, check, folder, link, note, refresh, search, sliders, stop, x, plus the ⋯ "more" glyph.
- `Toggle` / `labelled_toggle`: animated switch.
- `Select`: dropdown that opens a `PopupMenu`.
- `PopupMenu`: every menu.
- `popup_window()`: every floating window (Win11 rounded corners + shadow via DWM).
- `Tile`, `PendingTile`, `GroupTab`.

### Motion tokens
| Token | ms | Use |
|---|---|---|
| `MOTION_OPEN` | 170 | menus, popovers, toasts, dialogs appearing |
| `MOTION_CLOSE` | 110 | going away (quicker than opening) |
| `MOTION_HOVER` | 90 | highlight on |
| `MOTION_UNHOVER` | 170 | highlight off (soft trail) |
| `MOTION_FLASH` | 360 | tile flash when a sound fires |
| `SLIDE` | 7 px | popup glide distance while fading in |

Helpers: `animate()` (clock-timed, 1 ms system timer), `fade_to()` (colour tweens that pick up mid-fade), `reveal()` / `fade_away()` (window fade + glide).

---

## 8. Technical constraints a redesign must respect

These are why the code looks the way it does. Each one was a real bug.

1. **Keep the number of real windows low.** Every customtkinter widget is 1–2 native windows. Hundreds of them made group switching paint in visibly. That's why each **tile is one canvas with its card drawn as a single image** (`card_photo`), not a frame full of labels. A tile redesign should stay "one canvas, drawn items/images".
2. **The board uses `place()` only** (`Page.layout` computes every cell). Mixing in `grid()`/`pack()` made every tile flash after a drop.
3. **Slides are drawn on one overlay canvas** from a screen copy taken when the drag starts. Moving many tile windows at once tears.
4. **Popups never take focus** (`WS_EX_NOACTIVATE`), or the main title bar flickers to inactive and the game loses focus. The main window handles Esc for them.
5. **Nothing floats over the game.** Toasts and the Settings popover are owned by the main window; only menus (which you open yourself) are topmost.
6. **Animations go through `animate()` with the `MOTION_*` tokens**, never ad-hoc `after()` loops.
7. **Layered windows:** after fading with `-alpha`, the app removes `WS_EX_LAYERED` (`unlayer`) for speed; call `relayer()` before any later alpha change.
8. **Timers:** schedule with `later(widget, ms, fn)`; never destroy a widget inside its own `after` callback (it crashed theme switching).
9. **Clicks play on release**, never on press.
10. **Low CPU while gaming:** builds run at low priority; nothing polls faster than needed.
11. Save source files as **UTF-8** (Windows PowerShell 5.1 mangles them).

---

## 9. Testing

`tests/run_all.py` runs 8 suites, each in its own process on a throwaway copy with generated tones and a fake audio engine (nothing touches the real mic, cable, speakers or clipboard):

| Suite | Covers |
|---|---|
| `test_menus` | menus open/close, focus, Esc |
| `test_quick_import` | clipboard link → pending tile → clip (fake importer) |
| `test_drag_drop` | re-order, drop on group tabs, play-on-release |
| `test_slide` | slide animation, snapshot overlay |
| `test_layout` | grid layout, resize, columns |
| `test_settings` | toggles, solo / toggle-stop rules |
| `test_themes` | switching every theme without crashing |
| `test_uninstall` | the uninstaller on a fake install |

The windows open on screen, so tests aren't run while a game is in the foreground. Visual issues (tearing, flicker) are checked with frame-by-frame screen recordings, not just timings.

---

## 10. Build and distribution

- `scripts\build.ps1` builds both exes with PyInstaller (low priority) and swaps the new exe in after the running app closes (never overwrite the exe while it runs).
- `scripts\make_share.py` makes two friend zips: empty board, and board + sounds (re-encoded to Opus), each with ffmpeg, the uninstaller and "READ ME FIRST.txt".
- Friends install **VB-CABLE** themselves (free driver), set Discord/game input to **CABLE Output**, and run the exe (SmartScreen warns because it's unsigned).
- **Uninstall JondisBoard.exe** removes only known JondisBoard files and shortcuts; sounds and settings only if ticked, and to the Recycle Bin.

---

## 11. Known limitations / ideas for design work

- Startup takes 1–2 s to settle (pages for every group are pre-built).
- The group tab row doesn't wrap or scroll with many groups.
- No keyboard navigation of the grid (mouse + hotkeys only).
- Only dark themes; no light theme.
- A pinned taskbar icon keeps the icon it was pinned with.
- The drag snapshot would include anything sitting on top of the app (e.g. an overlay).
- No volume per clip, no favourites, no "recently played".
- Single big `app.py`: UI pieces aren't split into modules yet.

When proposing design changes, say which component/token they touch, keep the existing tokens and helpers, and check them against §8.
