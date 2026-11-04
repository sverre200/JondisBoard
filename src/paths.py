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


# ffmpeg's resampler set up for transparent sample-rate changes (aliasing ~ -120 dB; its default is
# ~ -95 dB). This ffmpeg build has no soxr. Used for every conversion to the app's 48 kHz.
RESAMPLE = "aresample=48000:filter_size=64:phase_shift=10:cutoff=0.97:filter_type=kaiser:kaiser_beta=12"


def find_ffmpeg():
    """ffmpeg.exe shipped next to the app (shared copies), else the one on PATH."""
    local = APP_DIR / "ffmpeg.exe"
    return str(local) if local.exists() else shutil.which("ffmpeg")
