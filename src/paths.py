"""Where the app's files live, both from source and as JondisBoard.exe."""
import shutil
import sys
from pathlib import Path

FROZEN = getattr(sys, "frozen", False)
# Your sounds, config.json and the log: next to JondisBoard.exe (not in its unpack folder), or,
# run from source, in the project folder above src/, so both use the same sounds and settings.
APP_DIR = Path(sys.executable).resolve().parent if FROZEN else Path(__file__).resolve().parent.parent
# The app's own pictures (icon.ico, icon.png): unpacked with the code in the exe, assets/ from source.
ASSETS_DIR = Path(getattr(sys, "_MEIPASS", "")) if FROZEN else APP_DIR / "assets"


def find_ffmpeg():
    """ffmpeg.exe shipped next to the app (shared copies), else the one on PATH."""
    local = APP_DIR / "ffmpeg.exe"
    return str(local) if local.exists() else shutil.which("ffmpeg")
