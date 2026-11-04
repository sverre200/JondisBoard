# JondisBoard

A soundboard for Windows that plays sound clips into **Discord and games** as if they came from your microphone. Your own voice still goes through, so you can talk over the clips, and you hear them in your headphones too.

```
Your mic ──┐
           ├─► JondisBoard ─► VB-CABLE ─► Discord / game  (their mic = "CABLE Output")
Clips ─────┘         └──────► your headphones
```

## Install

1. **Unzip `JondisBoard.zip`** into a folder you'll keep, for example `Documents\JondisBoard`. Keep everything in it together (the app saves your sounds and settings next to itself).
2. **Install VB-CABLE** (free virtual mic): download it from https://vb-audio.com/Cable/, unzip it, right-click `VBCABLE_Setup_x64.exe` → **Run as administrator** → **Install Driver**, then **restart your PC**.
3. **Open `JondisBoard.exe`.** If Windows says "Windows protected your PC", click **More info** → **Run anyway** (it's a homemade app without a paid signature). Click **Settings** and check that **Your microphone** is your real mic. **Send to** should already say `CABLE Input`.
4. **Discord** → Settings → Voice & Video: set **Input Device** to **CABLE Output**, and turn **Noise Suppression** off (or to Standard), or it filters the clips out.
5. **Games:** set the voice chat input to **CABLE Output**. Games without a mic choice (like DayZ) use the Windows default mic, so set **CABLE Output** as the default input in Windows Settings → System → Sound → Input.

Optional: YouTube imports need [Node.js](https://nodejs.org) installed. Instagram works without it.

## Using it

- **Add sounds:** copy an Instagram reel or YouTube link, then press **Quick import** (or **Ctrl+V** in the app). **Import link…** lets you name it, trim it or pick a group first. **Add files** adds sounds from your PC.
- **Play:** click a sound. **Right-click** it to set a hotkey (works in-game; numpad keys are good), rename it, give it a picture, move it to a group or delete it. **Stop all** stops everything.
- **Organise:** drag sounds to reorder them, or drop one on a group tab to move it there. **+ New group** makes a group; click a group's tab again for its colour, name and delete.
- **Settings:** your audio devices, plus two switches: **One sound at a time**, and **Press a playing sound to stop it**.
- **Sliders:** **Mic** is your voice, **Clips** is how loud the clips are for others, **In your ears** is what you hear. **Send my voice** off means only clips go out.
- **Theme** changes the colours (and the app's icon).

## Best sound quality

JondisBoard sends the cable the same clean signal you hear in your headphones. What your friends hear after that is decided by Discord or the game:

- **Discord** → Voice & Video: **Noise Suppression off** (Krisp treats music as noise), **Echo Cancellation off**, **Automatic Gain Control off**, and set the input sensitivity by hand (or use push-to-talk).
- **Windows** → Sound → CABLE Output → Properties: turn **Audio enhancements off** and leave the format at **48000 Hz**. In Sound → More sound settings → Communications, pick **Do nothing**.
- **DayZ:** voice is mono, compressed and positional: it gets quieter and duller with distance and through helmets or gas masks. The server's `vonCodecQuality` sets how good it can sound (higher is better). If one friend suddenly hears you much worse than the others, them restarting their game helps (a known DayZ bug).
- Keep **Clips** around 80–100 %: louder doesn't get through clearer, the game just squashes it.

## If something's off

- **Nobody hears the clips:** check that Discord's / the game's input (or the Windows default mic) is still **CABLE Output**. Some headsets switch it back when they reconnect.
- **Push-to-talk games:** clips only go out while you hold your push-to-talk key.
- **Hotkeys don't work in a game:** run JondisBoard as administrator (right-click → Run as administrator).
- **Sounds play twice:** two copies of the app are open; close one.

## Uninstall

Run **`Uninstall JondisBoard.exe`** in the JondisBoard folder. It removes the app, its log and its shortcuts (desktop, Start menu, taskbar). Your sounds and settings stay unless you tick the box, and then they go to the Recycle Bin. VB-CABLE is a separate driver: remove it in Windows Settings → Apps if nothing else needs it.

Building the app, running the tests and making the share zips: see [DEVELOPING.md](DEVELOPING.md).
