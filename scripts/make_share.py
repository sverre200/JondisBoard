r"""Build zips to share the soundboard with friends.

    .venv\Scripts\python.exe scripts\make_share.py

Makes share\JondisBoard.zip (empty board) and share\JondisBoard + sounds.zip
(your sounds, shrunk from WAV to Opus). Uses the JondisBoard.exe already in the
project folder, so rebuild that first if you changed the code.
"""
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))
from paths import APP_DIR, find_ffmpeg  # noqa: E402

SHARE = APP_DIR / "share"
AUDIO_EXTS = {".wav", ".mp3", ".ogg", ".flac", ".m4a", ".mp4", ".aac", ".opus", ".webm"}


def stage(folder, with_sounds, ffmpeg):
    folder.mkdir(parents=True)
    shutil.copy2(APP_DIR / "JondisBoard.exe", folder / "JondisBoard.exe")
    shutil.copy2(ffmpeg, folder / "ffmpeg.exe")
    shutil.copy2(APP_DIR / "assets" / "share_readme.txt", folder / "READ ME FIRST.txt")
    sounds = folder / "sounds"
    sounds.mkdir()
    if not with_sounds:
        return
    src_root = APP_DIR / "sounds"
    for src in sorted(src_root.rglob("*")):
        rel = src.relative_to(src_root)
        if not src.is_file() or rel.parts[0].startswith("_"):   # skip _deleted
            continue
        out = sounds / rel
        out.parent.mkdir(parents=True, exist_ok=True)
        if src.suffix.lower() == ".png":    # a clip's picture
            shutil.copy2(src, out)
        elif src.suffix.lower() in AUDIO_EXTS:
            out = out.with_suffix(".ogg")
            subprocess.run([ffmpeg, "-v", "error", "-y", "-i", str(src), "-c:a", "libopus", "-b:a", "128k",
                            str(out)], check=True)
            print(f"  {rel} -> {out.name}")


def main():
    exe = APP_DIR / "JondisBoard.exe"
    ffmpeg = find_ffmpeg()
    if not exe.exists() or not ffmpeg:
        sys.exit("Need JondisBoard.exe in the project folder and ffmpeg on PATH")
    SHARE.mkdir(exist_ok=True)
    for zip_name, with_sounds in (("JondisBoard", False), ("JondisBoard + sounds", True)):
        print(f"Building {zip_name}.zip ...")
        with tempfile.TemporaryDirectory() as tmp:
            stage(Path(tmp) / "JondisBoard", with_sounds, ffmpeg)
            zip_path = shutil.make_archive(str(SHARE / zip_name), "zip", tmp, "JondisBoard")
        print(f"  {zip_path}  ({Path(zip_path).stat().st_size / 1e6:.0f} MB)")


if __name__ == "__main__":
    main()
