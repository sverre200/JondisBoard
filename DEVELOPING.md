# Developing JondisBoard

Python 3.12 + customtkinter (Tk) for the window, sounddevice/soundfile/numpy for audio, `keyboard` for global hotkeys, and yt-dlp + ffmpeg for imports. It ships as a single `JondisBoard.exe` built with PyInstaller.

## Folders

```
src/       the app: app.py (window, board, menus, Settings, dialogs, themes, drag and drop),
           engine.py (audio devices and mixing), hotkeys.py, importer.py (downloads, trimming,
           thumbnails), logo.py (the waveform logo, per theme), themes.py (colours),
           uninstall.py ("Uninstall JondisBoard.exe"), paths.py
assets/    icon.ico and icon.png (bundled into the exe), share_readme.txt
scripts/   build.ps1, install_build.ps1, make_icon.py, make_share.py
tests/     the UI test suites (run_all.py runs them)
```

Your own files live in the project folder, next to `JondisBoard.exe`, and aren't in git: `sounds/` (a group is a folder; a picture is a same-name `.png`), `config.json`, and `soundboard.log` (errors go here). Running from source uses the same folder.

## Set up

```bash
python -m venv .venv
```

```bash
.venv\Scripts\python.exe -m pip install -r requirements.txt
```

ffmpeg must be on PATH (or `ffmpeg.exe` next to the app). Run from source with `.venv\Scripts\pythonw.exe src\app.py`.

## Test

```bash
.venv\Scripts\python.exe tests\run_all.py
```

Runs every suite (about a minute), each in its own process on a throwaway copy of the app with generated sounds and a stand-in audio engine, so nothing reaches your mic, cable, speakers or clipboard. The app's windows do open on screen while it runs, so don't run it mid-game. Add a word to run only some suites, e.g. `tests\run_all.py slide`.

## Build and install

```bash
powershell -ExecutionPolicy Bypass -File scripts\build.ps1
```

Builds `JondisBoard.exe` and `Uninstall JondisBoard.exe` from `src\` (at low priority, so a game doesn't stutter) and puts them in the project folder. If the app is open, the new exe waits and swaps in when you close it: never copy over `JondisBoard.exe` while it's running, because the one-file exe reads its code from that file and freezes if it changes. To change the icon, edit `src\logo.py` and run `scripts\make_icon.py`.

## Share with friends

```bash
.venv\Scripts\python.exe scripts\make_share.py
```

Makes `share\JondisBoard.zip` (empty board) and `share\JondisBoard + sounds.zip` (your sounds, shrunk to Opus), each with ffmpeg, the uninstaller and `READ ME FIRST.txt` (from `assets\share_readme.txt`). It packs the current `JondisBoard.exe`, so build first if you changed the code.

## Bulk import and Instagram logins

Put one clip per line in `reels.txt` (`url | name | start | end`):

```
https://www.instagram.com/reel/AAAA/ | battle cry | 0:01 | 0:04
https://www.instagram.com/reel/BBBB/ | laugh
```

```bash
.venv\Scripts\python.exe src\importer.py --file reels.txt
```

Then click **Reload** in the ⋯ menu. If Instagram demands a login, log in to Instagram in Firefox and set `"cookies_from_browser": "firefox"` in `config.json`.
