"""Turn round artwork on a white background into icon.png / icon.ico with a see-through outside.

    python make_icon.py ART.webp OUT_DIR [left top right bottom]

The optional box is the circle's edge in the artwork (measured, default for icon_art.webp)."""
import sys
from pathlib import Path

from PIL import Image, ImageDraw

src, out = Path(sys.argv[1]), Path(sys.argv[2])
box = tuple(int(v) for v in sys.argv[3:7]) if len(sys.argv) >= 7 else (0, -1, 1240, 1239)
im = Image.open(src).convert("RGBA").crop(box)
n = im.width
inset, big = 3, 4   # shave the white fringe; supersample the mask for a smooth edge
mask = Image.new("L", (n * big, n * big), 0)
ImageDraw.Draw(mask).ellipse((inset * big, inset * big, (n - inset) * big - 1, (n - inset) * big - 1), fill=255)
im.putalpha(mask.resize((n, n), Image.LANCZOS))

icon = im.resize((256, 256), Image.LANCZOS)
icon.save(out / "icon.png")
icon.save(out / "icon.ico", sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
preview = Image.new("RGBA", (420, 280), (15, 15, 20, 255))
x = 10
for size in (256, 64, 32, 16):
    preview.alpha_composite(im.resize((size, size), Image.LANCZOS), (x, 10))
    x += size + 12
preview.save(out / "icon_preview.png")

