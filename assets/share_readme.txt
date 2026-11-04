JONDISBOARD - play sounds through your mic in games and Discord
=================================================================

SETUP (once, about 5 minutes)

1. Unzip this whole folder somewhere permanent, for example Documents\JondisBoard.
   Keep JondisBoard.exe, ffmpeg.exe and the sounds folder together.

2. Install VB-CABLE (free virtual mic): https://vb-audio.com/Cable/
   Unzip it, right-click VBCABLE_Setup_x64.exe > Run as administrator > Install Driver.
   Then RESTART your PC.

3. Windows Settings > System > Sound:
   - Output: your normal headphones/speakers (the installer sometimes switches this to CABLE Input - change it back)
   - Input:  CABLE Output (VB-Audio Virtual Cable)
   Games like DayZ use this default mic, so the sounds and your voice both go through it.

4. Open JondisBoard.exe.
   If Windows says "Windows protected your PC": click More info > Run anyway.
   (It's a homemade app with no paid signature, which is why Windows warns.)
   Click "Settings" and check that "Your microphone" is your real mic.

USING IT

- Click a sound to play it. Right-click it to set a hotkey (numpad keys work well), rename or delete it.
- Drag sounds to reorder them, or drop one on a group tab to move it there.
- Give "Stop all" a hotkey too (click the small text under the red button).
- Settings also has "One sound at a time" and "Press a playing sound to stop it".
- "Quick import" (or Ctrl+V): copy an Instagram reel or YouTube link first, and it becomes a sound.
  "Import link..." lets you name it, trim it or pick a group first.
  YouTube links need Node.js installed (https://nodejs.org); Instagram works without it.
- In DayZ, hold push-to-talk (Caps Lock) while you press a sound's hotkey, or switch DayZ to voice activation.
- In Discord, set Input Device to CABLE Output and turn Noise Suppression off.

UNINSTALL

- Run "Uninstall JondisBoard.exe" in this folder. Your sounds stay unless you tick the box
  (then they go to the Recycle Bin). VB-CABLE: Windows Settings > Apps.

TROUBLESHOOTING

- Nobody hears the sounds: check the Windows default mic is still CABLE Output.
  Some headsets switch it back when they reconnect. Restart the game after fixing it.
- Hotkeys don't work in a game: right-click JondisBoard.exe > Run as administrator.
- Sounds play twice: two copies of the app are open; close one.
