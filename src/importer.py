"""Turn Instagram reels (or anything yt-dlp supports) into soundboard clips.

GUI uses import_clip(); you can also batch-import from the command line:

    python importer.py https://www.instagram.com/reel/XXXX/ --name scream --start 0:03 --end 0:06.5
    python importer.py --file reels.txt

reels.txt has one clip per line:   url | name | start | end   (name/start/end optional)
"""
import argparse
import re
import shutil
import subprocess
import sys
import tempfile
import urllib.request
from io import BytesIO
from pathlib import Path

import yt_dlp
from PIL import Image, ImageOps

from paths import APP_DIR, find_ffmpeg

SOUNDS_DIR = APP_DIR / "sounds"
PICTURE_SIZE = 256


def picture_path(audio_path):
    """A clip's picture is a PNG next to it with the same name."""
    return Path(audio_path).with_suffix(".png")


def save_picture(image, audio_path):
    """Save image (a path, bytes or PIL Image) as audio_path's square picture."""
    if isinstance(image, bytes):
        image = BytesIO(image)
    if not isinstance(image, Image.Image):
        image = Image.open(image)
    image = ImageOps.exif_transpose(image).convert("RGB")
    ImageOps.fit(image, (PICTURE_SIZE, PICTURE_SIZE), Image.LANCZOS).save(picture_path(audio_path))


def _save_thumbnail(info, audio_path, log):
    url = info.get("thumbnail")
    if not url:
        return
    try:
        req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
        with urllib.request.urlopen(req, timeout=15) as r:
            save_picture(r.read(), audio_path)
    except Exception as ex:
        log(f"No picture: {ex}")


def parse_time(s):
    """'83', '1:23', '1:23.5', '0:01:23' -> seconds (float). Empty -> None."""
    if s is None or str(s).strip() == "":
        return None
    parts = str(s).strip().split(":")
    if len(parts) > 3:
        raise ValueError(f"Bad time: {s!r}")
    secs = 0.0
    for p in parts:
        secs = secs * 60 + float(p)
    return secs


def safe_name(name):
    name = re.sub(r'[<>:"/\\|?*\x00-\x1f]', "", name).strip(" .")
    return name[:80] or "clip"


def unique_path(folder, name, ext=".wav"):
    p = folder / f"{name}{ext}"
    n = 2
    while p.exists():
        p = folder / f"{name} ({n}){ext}"
        n += 1
    return p


def import_clip(url, name=None, start=None, end=None, normalize=True,
                cookies_from_browser=None, log=print, folder=None):
    """Download url, cut [start, end], save as 48 kHz stereo WAV in folder (default sounds/),
    with the video's thumbnail as its picture. Returns the Path."""
    folder = Path(folder or SOUNDS_DIR)
    ffmpeg = find_ffmpeg()
    if not ffmpeg:
        raise RuntimeError("ffmpeg not found (put ffmpeg.exe next to the app)")
    start_s, end_s = parse_time(start), parse_time(end)
    if start_s is not None and end_s is not None and end_s <= start_s:
        raise ValueError("End time must be after start time")

    folder.mkdir(parents=True, exist_ok=True)
    tmp = Path(tempfile.mkdtemp(prefix="soundboard_"))
    try:
        log(f"Downloading {url} ...")
        opts = {
            "format": "bestaudio/best",
            "outtmpl": str(tmp / "%(id)s.%(ext)s"),
            "noplaylist": True,
            "quiet": True,
            "no_warnings": True,
            "noprogress": True,
            # YouTube needs a JS runtime to unlock its audio formats; Node is on this PC
            "js_runtimes": {"deno": {}, "node": {}},
        }
        if cookies_from_browser:
            opts["cookiesfrombrowser"] = (cookies_from_browser,)
        with yt_dlp.YoutubeDL(opts) as ydl:
            info = ydl.extract_info(url, download=True)
        files = [f for f in tmp.iterdir() if f.is_file()]
        if not files:
            raise RuntimeError("Download produced no file")
        src = max(files, key=lambda f: f.stat().st_size)

        if not name:
            # Instagram: the reel caption without hashtags; elsewhere (YouTube...): the title
            if info.get("extractor_key", "").lower().startswith("instagram"):
                caption = re.sub(r"#\S+", "", info.get("description") or "").split("\n")[0].strip()
            else:
                caption = info.get("title") or ""
            name = caption[:50].strip() or \
                f"{info.get('channel') or info.get('uploader_id') or 'clip'}_{info.get('id', '')}"
        out = unique_path(folder, safe_name(name))

        cmd = [ffmpeg, "-y", "-hide_banner", "-loglevel", "error"]
        if start_s is not None:
            cmd += ["-ss", f"{start_s:.3f}"]
        cmd += ["-i", str(src)]
        if end_s is not None:
            cmd += ["-t", f"{end_s - (start_s or 0):.3f}"]
        cmd += ["-vn", "-ac", "2", "-ar", "48000"]
        if normalize:
            cmd += ["-af", "loudnorm=I=-16:TP=-1.5:LRA=11"]
        cmd += [str(out)]
        log("Cutting audio ...")
        proc = subprocess.run(cmd, capture_output=True, text=True,
                              creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
        if proc.returncode != 0 or not out.exists():
            raise RuntimeError(proc.stderr.strip() or "ffmpeg failed")
        _save_thumbnail(info, out, log)
        log(f"Saved {out.name}")
        return out
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


def _main():
    ap = argparse.ArgumentParser(description="Import reels as soundboard clips")
    ap.add_argument("url", nargs="?")
    ap.add_argument("--name")
    ap.add_argument("--start")
    ap.add_argument("--end")
    ap.add_argument("--file", help="text file: url | name | start | end per line")
    ap.add_argument("--group", help="put the clips in this group (a folder inside sounds)")
    ap.add_argument("--no-normalize", action="store_true")
    ap.add_argument("--cookies-from-browser", help="e.g. firefox, if Instagram demands a login")
    a = ap.parse_args()

    jobs = []
    if a.file:
        for line in Path(a.file).read_text(encoding="utf-8").splitlines():
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            parts = [p.strip() for p in line.split("|")] + [""] * 3
            jobs.append((parts[0], parts[1] or None, parts[2] or None, parts[3] or None))
    elif a.url:
        jobs.append((a.url, a.name, a.start, a.end))
    else:
        ap.error("give a url or --file")

    failed = 0
    for url, name, start, end in jobs:
        try:
            import_clip(url, name, start, end, normalize=not a.no_normalize,
                        cookies_from_browser=a.cookies_from_browser,
                        folder=SOUNDS_DIR / safe_name(a.group) if a.group else None)
        except Exception as ex:
            failed += 1
            print(f"FAILED {url}: {ex}", file=sys.stderr)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    _main()
