# JondisBoard

Plays clips into Discord and games as if they came from your mic. Your real voice gets mixed in, so you can keep talking.

```
Real mic ──┐
           ├─► JondisBoard ─► CABLE Input ─► (Discord/game mic = "CABLE Output")
Clips ─────┘         └──────► your headphones (so you hear the clips too)
```

## One-time setup

1. **Install VB-CABLE** (free): https://vb-audio.com/Cable/. Unzip it, right-click `VBCABLE_Setup_x64.exe` → *Run as administrator* → *Install Driver*, then reboot.
2. Start **`JondisBoard.exe`** (there's a shortcut on the desktop). "Send to (virtual cable)" should fill in with `CABLE Input` on its own. If it doesn't, pick it and click **Restart audio**.
3. **Discord** → Settings → Voice & Video:
   - Input device: **CABLE Output (VB-Audio Virtual Cable)**
   - Turn **Noise Suppression off**, or set it to Standard. Krisp treats clips as "noise" and filters them out.
   - Voice Activity works best. With push-to-talk, a clip only goes through while you're holding PTT.
   - Turning off Echo Cancellation and Automatic Gain Control helps too.
4. **In games**, set the voice-chat input device to **CABLE Output** if the game lets you choose one. If not, see the DayZ section below.

### DayZ

DayZ has no mic picker. It uses the Windows default mic, so:

1. Windows Settings → System → Sound → Input → choose **CABLE Output** as the default.
   The soundboard still picks up your real mic itself (it's listed by name under "Your microphone").
2. DayZ uses push-to-talk (Caps Lock by default), so clips only go out while you hold it.
   Hold Caps Lock and hit the clip's hotkey, or switch to voice activation in DayZ's settings if you'd rather not hold anything.
3. Clips go out on proximity chat, so players near you hear them. The "Clips" slider sets how loud.

To undo it, set your normal mic back as the Windows default.

Don't point Windows' default *output* at CABLE Input, or everything on your PC goes into your mic.

## Using it

- **Click** a button to play its clip. Clicking it again restarts it.
- **Right-click** a button to set a hotkey, rename it or delete it. Hotkeys work while you're in a game. Numpad keys are separate from the normal number keys, so Numpad 1–9 make good hotkeys.
- **Set stop key** gives "Stop all" a hotkey.
- Volume sliders: **Mic** is your voice, **Clips** is how loud clips are for other people, **In your ears** is what you hear.
- **Send my voice too** off means only clips go out.
- If hotkeys don't work in a game that runs as admin, run the soundboard as admin as well.

### Groups and pictures

- **Groups** are the tabs above the sounds. Click **+ New group** to make one, then right-click a sound > **Move to group**. Right-click a tab to rename it, change its colour, or delete it. Each group has its own colour, which tints its tab and its sounds. Deleting a group keeps its sounds; they move to "No group".
- Imports and added files go into the group you're looking at (the import window also has a group picker). Search looks through every group.
- On disk, a group is just a folder inside `sounds`.
- **Pictures**: imports use the video's thumbnail automatically. Right-click a sound > **Choose picture...**, or copy an image (in your browser, right-click > Copy image) and use **Paste picture**. The picture is saved as a `.png` with the same name as the sound.

## Adding sounds

**From Instagram or YouTube:** YouTube links work the same way as reels (YouTube needs Node.js installed, which it is). For Instagram: open the reel, use *Share → Copy link*, then click **+ Import link** in the app. It fills in the link from your clipboard. Optionally give it a name and a start/end time like `0:03.5` → `0:07`. It downloads the reel, cuts out that part, evens out the loudness and adds a button.

Use links to single reels (`instagram.com/reel/...`). A profile link like `instagram.com/thewarrior1010/reels/` won't work.

If Instagram demands a login, log in to Instagram in **Firefox**, then set this in `config.json` (next to app.py):

```json
"cookies_from_browser": "firefox"
```

To import a lot at once, make `reels.txt` with one clip per line (`url | name | start | end`):

```
https://www.instagram.com/reel/AAAA/ | battle cry | 0:01 | 0:04
https://www.instagram.com/reel/BBBB/ | laugh
```

```bash
.venv\Scripts\python.exe importer.py --file reels.txt
```

Then click **Reload**.

**From files:** use **Add files...**, or drop audio/video files (wav, mp3, m4a, mp4, ogg…) into the `sounds` folder and click **Reload**.

## Files

- `app.py`: window, buttons, settings
- `engine.py`: audio mixing and devices
- `hotkeys.py`: global hotkeys
- `importer.py`: reel download and trimming (yt-dlp + ffmpeg)
- `config.json`: your devices, volumes and hotkeys (created automatically)
- `soundboard.log`: errors go here if something acts weird

`JondisBoard.exe` must stay in this folder, since it keeps `sounds` and `config.json` beside it. After changing the code, rebuild it with:

```bash
.venv\Scripts\python.exe -m PyInstaller --noconfirm --onefile --windowed --name JondisBoard --icon icon.ico --add-data "icon.ico;." --add-data "icon.png;." --collect-data customtkinter --collect-all yt_dlp_ejs --distpath build_out app.py
```

```bash
powershell -ExecutionPolicy Bypass -File install_build.ps1 build_out\JondisBoard.exe
```

Don't copy over `JondisBoard.exe` while the app is open: a running copy keeps reading its code from that file and freezes if it changes. `install_build.ps1` waits until the app is closed before swapping the new build in.

Reinstall dependencies with:

```bash
python -m venv .venv
```

```bash
.venv\Scripts\python.exe -m pip install -r requirements.txt
```

ffmpeg must be on PATH (it already is on this PC).

## Sharing with friends

```bash
.venv\Scripts\python.exe make_share.py
```

This makes two zips in `share\`: `JondisBoard.zip` (empty board) and `JondisBoard + sounds.zip` (your sounds, shrunk to Opus). Each one includes ffmpeg and a `READ ME FIRST.txt` with setup steps for your friends. It uses the current `JondisBoard.exe`, so rebuild that first if you changed the code.
