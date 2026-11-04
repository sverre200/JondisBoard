# Developing JondisBoard

Python 3.12 + customtkinter (Tk) for the window, sounddevice/soundfile/numpy for audio, `keyboard` for global hotkeys, and yt-dlp + ffmpeg for imports. It ships as a single `JondisBoard.exe` built with PyInstaller.

## Files

- `app.py` - the whole window: board, tiles, menus, Settings, dialogs, themes, drag and drop
- `engine.py` - audio devices and mixing
- `hotkeys.py` - global hotkeys
- `importer.py` - downloads (yt-dlp), trimming and loudness (ffmpeg), thumbnails
- `logo.py` - the waveform logo, drawn per theme; `make_icon.py` regenerates `icon.ico` / `icon.png`
- `paths.py`, `install_build.ps1`, `make_share.py`, `share_readme.txt` - see below
- `tests/` - the UI test suites
- Next to the exe (not in git): `sounds/` (a group is a folder; a picture is a same-name `.png`), `config.json`, `soundboard.log` (errors go here)

## Set up

```bash
python -m venv .venv
```

```bash
.venv\Scripts\python.exe -m pip install -r requirements.txt
```

ffmpeg must be on PATH (or `ffmpeg.exe` next to the app). Run from source with `.venv\Scripts\pythonw.exe app.py`.

## Test

```bash
.venv\Scripts\python.exe tests\run_all.py
```

Runs every suite (about a minute), each in its own process on a throwaway copy of the app with generated sounds and a stand-in audio engine, so nothing reaches your mic, cable, speakers or clipboard. The app's windows do open on screen while it runs, so don't run it mid-game. Add a word to run only some suites, e.g. `tests\run_all.py slide`.

## Build and install

```bash
.venv\Scripts\python.exe -m PyInstaller --noconfirm --onefile --windowed --name JondisBoard --icon ..\icon.ico --add-data "..\icon.ico;." --add-data "..\icon.png;." --collect-data customtkinter --collect-all yt_dlp_ejs --specpath build --distpath build_out app.py
```

```bash
powershell -ExecutionPolicy Bypass -File install_build.ps1 build_out\JondisBoard.exe
```

Never copy over `JondisBoard.exe` while it's running: the one-file exe reads its code from that file and freezes if it changes. `install_build.ps1` waits until the app is closed before swapping the new build in. (`--specpath build` keeps PyInstaller's spec file in `build/`, which is why the icon paths start with `..\`.)

## Share with friends

```bash
.venv\Scripts\python.exe make_share.py
```

Makes `share\JondisBoard.zip` (empty board) and `share\JondisBoard + sounds.zip` (your sounds, shrunk to Opus), each with ffmpeg and `READ ME FIRST.txt` (from `share_readme.txt`). It packs the current `JondisBoard.exe`, so build first if you changed the code.

## Bulk import and Instagram logins

Put one clip per line in `reels.txt` (`url | name | start | end`):

```
https://www.instagram.com/reel/AAAA/ | battle cry | 0:01 | 0:04
https://www.instagram.com/reel/BBBB/ | laugh
```

```bash
.venv\Scripts\python.exe importer.py --file reels.txt
```

Then click **Reload** in the ⋯ menu. If Instagram demands a login, log in to Instagram in Firefox and set `"cookies_from_browser": "firefox"` in `config.json`.
