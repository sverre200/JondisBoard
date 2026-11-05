"""Make the benchmark libraries: 20, 100, 500 and 1,000 sounds with groups and pictures.

    python rust/bench/make_libraries.py <out folder>

Each library is <out>/<n>/ laid out like the app folder (sounds/ + config.json). Sounds are
1-4 s 48 kHz stereo 16-bit tones (a typical soundboard clip), spread over 6 groups and No group;
every third sound has a 256x256 PNG picture. Smaller libraries are hard links into the 1,000 one,
so the disk cost is paid once. The configs are silent (all volumes 0) and name devices that don't
exist, so nothing can reach a real mic, cable or speaker.
"""
import json
import os
import struct
import sys
import zlib
from pathlib import Path

GROUPS = ["Memes", "THE LION", "Movies", "Games", "Music", "Voice lines"]
WORDS = ["air", "horn", "bruh", "vine", "boom", "sad", "trombone", "laugh", "drum", "roll", "epic",
         "fail", "win", "scream", "wow", "nope", "yes", "gong", "jazz", "ding", "crickets", "boing"]


def wav(path, seconds, freq):
    import numpy as np
    t = np.arange(int(48000 * seconds)) / 48000
    v = (0.2 * 32767 * np.sin(2 * np.pi * freq * t)).astype("<i2")
    data = np.repeat(v, 2).tobytes()
    hdr = b"RIFF" + struct.pack("<I", 36 + len(data)) + b"WAVEfmt " + struct.pack(
        "<IHHIIHH", 16, 1, 2, 48000, 48000 * 4, 4, 16) + b"data" + struct.pack("<I", len(data))
    path.write_bytes(hdr + data)


def png(path, rgb, size=256):
    raw = b"".join(b"\x00" + bytes(rgb) * size for _ in range(size))

    def chunk(t, d):
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xffffffff)
    path.write_bytes(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 2, 0, 0, 0))
                     + chunk(b"IDAT", zlib.compress(raw, 6)) + chunk(b"IEND", b""))


def name(i):
    a, b, c = WORDS[i % len(WORDS)], WORDS[(i * 7) % len(WORDS)], WORDS[(i * 13) % len(WORDS)]
    return f"{a.title()} {b} {c} {i:04d}"


def main():
    out = Path(sys.argv[1])
    full = out / "1000"
    sounds = full / "sounds"
    rels = []
    if not (full / "done").exists():
        for g in GROUPS:
            (sounds / g).mkdir(parents=True, exist_ok=True)
        for i in range(1000):
            g = GROUPS[i % 7] if i % 7 < len(GROUPS) else ""
            folder = sounds / g if g else sounds
            p = folder / f"{name(i)}.wav"
            wav(p, 1.0 + (i * 37 % 300) / 100.0, 180 + (i * 53) % 900)
            if i % 3 == 0:
                png(p.with_suffix(".png"), ((i * 40) % 255, (i * 90) % 255, (i * 150) % 255))
            print(i, end="\r", flush=True)
        (full / "done").write_text("ok")
    for i in range(1000):
        g = GROUPS[i % 7] if i % 7 < len(GROUPS) else ""
        rels.append(((g + "/") if g else "") + f"{name(i)}.wav")
    for n in (20, 100, 500, 1000):
        lib = out / str(n)
        if n != 1000:
            for g in GROUPS:
                (lib / "sounds" / g).mkdir(parents=True, exist_ok=True)
            for rel in rels[:n]:
                for ext in (".wav", ".png"):
                    src = (sounds / rel).with_suffix(ext)
                    dst = (lib / "sounds" / rel).with_suffix(ext)
                    if src.exists() and not dst.exists():
                        os.link(src, dst)
        cfg = {"mic": "Bench mic (none)", "cable": "Bench cable (none)", "monitor": "Bench out (none)",
               "mic_volume": 0.0, "clip_volume": 0.0, "monitor_volume": 0.0, "mic_passthrough": False,
               "hotkeys": {}, "group": "*", "theme": "Toxic Green", "geometry": "1300x820+60+40"}
        (lib / "config.json").write_text(json.dumps(cfg, indent=2), encoding="utf-8")
    print("libraries ready in", out)


if __name__ == "__main__":
    main()
