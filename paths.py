"""Where the app's files live, both from source and as JondisBoard.exe."""
import shutil
import sys
from pathlib import Path

# as JondisBoard.exe, data lives next to the exe rather than in its unpack folder
APP_DIR = Path(sys.executable if getattr(sys, "frozen", False) else __file__).resolve().parent


def find_ffmpeg():
    """ffmpeg.exe shipped next to the app (shared copies), else the one on PATH."""
    local = APP_DIR / "ffmpeg.exe"
    return str(local) if local.exists() else shutil.which("ffmpeg")
