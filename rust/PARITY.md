# Feature parity: Rust vs the Python app

Legend: **auto** = covered by an automated test (`tests/ui_flows.rs`, `tests/compat_real.rs`,
unit tests); **visual** = checked in offscreen snapshots / a screenshot of the real window;
**manual** = needs a person (real devices, a game, the network); **not tested** = implemented but
not exercised yet. Nothing below was tested against the real VB-CABLE: Discord was running the
whole time, and the loopback refuses then (see "Not done / not verified").

## Board and tiles

| Feature | Status | Evidence |
|---|---|---|
| Tile grid, 250x80 minimum, columns stretch | done | visual, auto (tile count/layout) |
| Picture / tinted placeholder with note, 2-line clamped title, duration, group (in All), hotkey chip | done | visual |
| Play on mouse release only; a press that becomes a drag never plays | done | auto |
| A click that closes a menu doesn't play | done | auto |
| Playing look: tint, accent outline, progress bar, equalizer over the picture | done | visual (state forced) |
| Flash when a sound fires (click or hotkey) | done | visual |
| ⋯ button on hover opens the clip menu; right-click menu | done | auto (right-click), visual |
| Empty board screen with Quick import / Import link buttons | done | visual |
| "This group is empty…" / "No sounds match your search." | done | visual |
| Smooth wheel scrolling, scrollbar (drag + click on track), per-tab scroll position | done | bench (scroll), not tested by hand |

## Clips

| Feature | Status | Evidence |
|---|---|---|
| Stop all (button + hotkey, label under the button) | done | auto (hotkey assignment) |
| Add files (copies into the open group, safe names, numbered duplicates) | done | unit (library), manual for the picker |
| Rename (dialog), hotkey/order/picture follow | done | auto |
| Delete to `sounds\_deleted` (confirm dialog), picture follows | done | auto |
| Move to group (menu, drop on tab), New group… from the menu | done | auto |
| Picture: Choose… / Paste (clipboard image or copied file) / Remove | done | manual (picker, clipboard); Remove now goes to the **Recycle Bin** (Python deleted it) |
| Picture saving: 256x256 centre crop PNG, EXIF rotation | done | not tested with a rotated photo |
| Sort A–Z with slide | done | auto |
| Custom order (drag) saved in `order`, carried across renames/moves | done | auto |
| Drag with tiles sliding out of the way live, drop on group tabs, Esc cancels, edge auto-scroll | done | auto (reorder, tab drop), visual |
| Search (Ctrl+F, Esc clears, searches every group) | done | auto |

## Groups

| Feature | Status | Evidence |
|---|---|---|
| All / custom groups / No group tabs with counts and colour dots | done | visual, auto |
| Create, rename (folder + hotkeys + order + colour carried), delete (sounds move to No group, leftovers to `_deleted`) | done | auto |
| Colour (8-colour palette, unused first) | done | auto |
| Clicking the open tab / its chevron opens its menu | done | not tested by hand |
| Selected group remembered | done | auto |
| Tabs wrap onto more rows when there are many | new | visual (not with dozens of groups) |

## Audio

| Feature | Status | Evidence |
|---|---|---|
| Mic + clips → VB-CABLE, clips → headphones | done | offline mixer tests; **not run on real devices** |
| Three independent volumes (Mic 0–150 %, Clips 0–150 %, In your ears 0–100 %) | done | auto (value reaches the mixer) |
| Send my voice | done | unit (mixer) |
| One sound at a time / Press a playing sound to stop it | done | auto (toggle), unit (rules in Player) |
| Float32 48 kHz stereo, 1 ms lookahead peak guard at −1 dBFS, no hard clip | done | unit + sample-by-sample vs Python (bit-exact; limiter within 1.2e-7) |
| Resampling of non-48 kHz / >2 ch files via ffmpeg `RESAMPLE` | done | unit (real ffmpeg convert) |
| Mic jitter cushion ring buffer, drift skip | done | unit |
| No allocation / I/O / locks in callbacks | done | by construction (see ARCHITECTURE.md); allocation not instrumented |
| Device selection (Settings), Restart audio, automatic first-run picks | done | not tested with real device switching |
| Refuse mic = CABLE, monitor = cable | done | by code review |
| VB-CABLE missing: help box with Get VB-CABLE | done | visual (not with the driver really missing) |
| Device unplugged while running → warning | new | not tested |
| Dropout / limiter counters in `soundboard.log` | done | not tested |
| Lazy decoding in the background within a memory budget | new | auto (compat: all real clips decode) |

## Hotkeys

| Feature | Status | Evidence |
|---|---|---|
| Global low-level hook, scan code + numpad flag + mods, same config format | done | unit (keypad table, binding equality), compat (format) |
| Works in fullscreen games | done | **manual: not tested in DayZ** |
| Record mode ("Press a key", Esc cancels), key swallowed while recording | done | manual: not tested |
| Conflict handling: one key = one action, toast says where it moved from | done | auto |
| Hotkeys don't fire while typing in the app's own text fields | new | not tested |

## Imports

| Feature | Status | Evidence |
|---|---|---|
| Quick import (button / Ctrl+V): link from clipboard, pending tile, status, Retry/Dismiss, full error on click | done | auto (no link, pending tile, failure, dismiss) |
| Import link… dialog: link, name, start/end, group, Even out loudness, progress, errors | done | visual; validation logic unit-tested |
| yt-dlp (standalone exe) download, Instagram caption / YouTube title naming, thumbnail → picture | done | bundled (official release, SHA-256 verified); **manual: no real download tested yet** |
| Two-pass linear loudnorm to −16 LUFS, cut, 48 kHz stereo WAV | done | unit (real ffmpeg) |
| `cookies_from_browser` | done | not tested |
| YouTube needs a JS runtime (deno/node used if installed) | same as before | not tested |
| Bulk import from `reels.txt`: `JondisBoard.exe --import-file reels.txt [--group G]` | done | unit (line parsing); no network run |

## Window, look, settings

| Feature | Status | Evidence |
|---|---|---|
| 6 themes, instant switch, nothing lost (group, search, scroll, window, playback, devices, hotkeys) | done | auto, bench |
| Title bar colours + window and taskbar icon follow the theme, in place | done | screenshot of title bar; taskbar not checked by eye |
| Waveform logo per theme and size (3 bars ≤24 px) | done | unit, visual |
| Settings panel (Audio, Playback) with toggles; Esc / click outside closes | done | auto, visual |
| Status pill (Live / Check audio setup / VB-CABLE not installed / Audio error / Starting) | done | visual |
| Toasts (4.2 s, click to dismiss) | done | auto (text), visual |
| Dialogs (rename, new/rename group, confirm, hotkey, import), Enter/Esc/Tab | done | auto, visual |
| Keyboard: Ctrl+V (Quick import / paste in a field), Ctrl+F, Esc | done | auto |
| Window size/position saved (Python's "WxH+X+Y" format), restored on the right monitor | done | auto (saved); restore not tested on a second monitor |
| Per-monitor DPI (v2), DPI change on monitor move | done | **not tested at 125/150 %, or across monitors** |
| Menus/popovers closed when another app comes to the front or the window moves | done | not tested by hand |
| Explicit AppUserModelID | done | by code |
| No opening fade-in (appears fully drawn: cloaked until the first frame) | done | screenshot |

## Data compatibility

| | Status | Evidence |
|---|---|---|
| Reads the Python `config.json` and `sounds\` as is (UTF-8 names, groups, `_deleted`, order, colours, volumes, devices, theme, last group, geometry) | done | compat test on a copy of the real install |
| Unknown config keys kept; wrong types fall back per key | done | unit |
| First save makes `config.backup.json`; atomic writes | done | unit, compat |
| The Python app can still read what the Rust app writes | done | unit (round trip), compat |

## Not done / not verified

- **Real audio path not exercised**: no stream has been opened on the real CABLE, mic or
  headphones (Discord was running; the loopback refuses). `--audio-check loopback` is ready to
  run when nobody is in voice. The Python loopback result (flat 31 Hz–19 kHz, −89 dB THD+N, 0
  dropouts, 0 ppm) was measured on the same WASAPI-converted endpoints.
- yt-dlp.exe is bundled (official build, checksum verified) but no real import has been run with it.
- **Uninstaller not ported**: the release README says to delete the folder. The Python
  `Uninstall JondisBoard.exe` doesn't know the new files (`yt-dlp.exe`, `config.backup.json`).
- No keyboard navigation of the grid (same as Python).
- Menus can't extend past the window edge (they're drawn inside it).
