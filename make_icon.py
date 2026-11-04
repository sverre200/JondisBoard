"""Write icon.ico / icon.png (the Toxic Green logo) for the exe, and build/icon_preview.png with every theme.

    python make_icon.py

The running app draws its own icon in the chosen theme (see logo.py); these files are only what
Explorer and the desktop shortcut show, which can't change with the theme."""
from pathlib import Path

from PIL import Image

import logo
from app import THEMES

here = Path(__file__).resolve().parent
green = THEMES["Toxic Green"]
logo.save_ico(here / "icon.ico", logo.tile_color(green), green["ACCENT"])
logo.draw(256, logo.tile_color(green), green["ACCENT"]).save(here / "icon.png")

preview = Image.new("RGBA", (90 + 300 * len(THEMES) // 2, 330), (20, 20, 24, 255))
for k, (name, t) in enumerate(THEMES.items()):
    x, y = 12 + (k % 3) * 300, 12 + (k // 3) * 160
    for size in (128, 48, 32, 24, 16):
        preview.alpha_composite(logo.draw(size, logo.tile_color(t), t["ACCENT"]), (x, y + 128 - size))
        x += size + 10
(here / "build").mkdir(exist_ok=True)
preview.save(here / "build" / "icon_preview.png")   # build/ is git-ignored scratch space
print("wrote icon.ico, icon.png and build/icon_preview.png")
