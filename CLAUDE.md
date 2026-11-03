# JondisBoard

Windows desktop soundboard: plays clips (plus the user's real mic) into VB-CABLE so games (DayZ) and Discord hear them. Python 3.12 + customtkinter (Tk), sounddevice/soundfile/numpy audio, `keyboard` global hotkeys, yt-dlp + ffmpeg imports. Shipped as a PyInstaller one-file exe.

## Layout
- `logo.py` - the waveform logo, drawn per theme and per size (`theme_icon()` writes the window/taskbar .ico to %TEMP%\JondisBoard). `make_icon.py` regenerates `icon.ico`/`icon.png` (Toxic Green; only Explorer and the shortcut use them).
- `app.py` - all UI (App, Page per group, Tile, PopupMenu, Select, dialogs, themes). `engine.py` - audio streams/mixing. `importer.py` - yt-dlp download, ffmpeg trim, thumbnails, pictures. `hotkeys.py`, `paths.py`.
- User data next to the exe (not in git): `sounds/` (groups = subfolders, picture = same-name `.png`), `config.json`, `soundboard.log`.

## Build and install
```
.venv\Scripts\python.exe -m PyInstaller --noconfirm --onefile --windowed --name JondisBoard --icon icon.ico --add-data "icon.ico;." --add-data "icon.png;." --collect-data customtkinter --collect-all yt_dlp_ejs --distpath build_out app.py
powershell -ExecutionPolicy Bypass -File install_build.ps1 build_out\JondisBoard.exe
```
Never copy over `JondisBoard.exe` while it runs: the one-file exe reads its code from that file lazily and freezes/fails imports. `install_build.ps1` stages the build and swaps it in after the app closes.

## Testing / verification
- No unit-test suite yet. Verify with scripted runs of a **test copy** (copy the .py files and a few sounds to a scratch folder, `config.json` with all volumes 0 and `mic_passthrough: false`) so nothing reaches the user's mic, and drive the App object from a script.
- Measure instead of eyeballing: frame gaps for animations, counts of `_draw` calls for redraw cost, `GetForegroundWindow` for focus.
- Capture windows with `ImageGrab` (on screen) or `PrintWindow` (behind other windows). Don't make test windows topmost or move the user's mouse; the user is often in a fullscreen game.

## Gotchas (all bit us before)
- Edit source with the Edit/Write tools or Python using UTF-8. PowerShell 5.1 `Get-Content`/`Set-Content` round-trips mangle non-ASCII (`›` became `â€º`).
- customtkinter: `resizable()` schedules a title-bar hide/show (set `_deactivate_windows_window_header_manipulation` on popups); its `place()` refuses `width=` (use `tk.Frame.place`); it already binds `<Button-1>` on "all", so bind `<Button-1>` explicitly, not `<ButtonPress>`.
- After fading a window with `-alpha`, clear `WS_EX_LAYERED` (`_unlayer`) or Windows repaints the whole window under every popup.
- Menus must not take focus (`WS_EX_NOACTIVATE`) or the main title bar flips to inactive; Esc is handled by the main window.
- Use `monitor_work_area()` for placement; `winfo_screenwidth()` only knows the primary monitor.
- Animations go through `animate()` (clock-timed, 1 ms system timer from `smooth_timers`, warm-up while invisible). Durations come from the `MOTION_*` tokens; colour changes use `fade_to()` (picks up mid-fade), window motion uses `reveal()` / `fade_away()`.
- Every floating window (menus, Audio setup popover, toasts) comes from `popup_window()`: Win11 draws its rounded corners, outline and shadow (DWM corner preference), so no see-through colour key. Menus are topmost; the popover and toasts are `owned_by(root)` so they never float over a game. Nothing pops up before `when_ready()`.
- Dialogs close with `win.close(then)`: fade out, destroy, then run the follow-up (so heavy work doesn't freeze the fade).
- `CTkFrame.bind` binds its inner `_canvas`; a synthetic `event_generate` must target that canvas.
- Buttons come from `button()` (auto width from the label, `icon_name=`, pressed look); change a button's hover with `set_hover_color()`. Tiles share `_tile_grid()` (picture | 2-line `clamp_lines` title | info row | bar); `PendingTile` is the Quick import stand-in and must keep the same size.
- Tiles are single `CTkCanvas`es (DrawEngine card + canvas items), not frames of labels: every customtkinter widget is 1-2 real windows, and hundreds of them made group switches paint in visibly. Keep the board's window count low; recolour with `itemconfig` (CTkCanvas's, for the corner glyphs).
- The board is laid out with `place()` (Page.layout works out every cell), never `grid()`: switching a widget between geometry managers hides and re-shows it, which made every tile flash after a drop. Re-sorts slide tiles between cells (`slide_from`).
- Re-sort slides are drawn on ONE canvas over the board (`Page._slide_on_canvas`) from a screen copy taken when the drag starts (`Page.snapshot`, raw BitBlt ~15 ms; ImageGrab ~70, PrintWindow ~35, child-window BitBlt is blank). Moving many tile windows at once tears no matter how paints are ordered. Tile cards are one image (`card_photo`), not customtkinter's glyph corners (2x cheaper repaints). Check tearing with frame-by-frame screen recordings, not frame timings.
- `main()` sets an explicit AppUserModelID first: launched from a shortcut, Windows otherwise shows the shortcut's (exe's) icon on the taskbar instead of the themed window icon.
- Tiles play on mouse *release* (a press that becomes a drag must never fire a sound into the game). `Dragger` handles re-ordering (`cfg["order"]`, carried across renames/moves in `reload_clips`) and drops onto group tabs; test it with synthetic events on `Tile._press/_drag_motion/_release`, never by moving the real mouse.
- `unlayer()` takes WS_EX_LAYERED off behind Tk's back; call `relayer()` before any later `-alpha` change on that window or it silently does nothing.
- Tk has been seen calling `<Enter>`/`<Button-1>` handlers with no event (cause not found): tile/tab handlers take `e=None` and use `pointer_event()` / `pointer_inside()`.
- Quick import (`quick_import`, Ctrl+V) is testable with a fake `importer.import_clip` and a stubbed `root.clipboard_get`; never touch the real clipboard in tests.
- Don't use `WM_SETREDRAW` on partial areas: it left black patches.
