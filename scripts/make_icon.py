r"""Write assets/icon.ico and assets/icon.png (the Toxic Green logo) for the exe, and build/icon_preview.png
with every theme.

    .venv\Scripts\python.exe scripts\make_icon.py

The running app draws its own icon in the chosen theme (see logo.py); these files are only what
Explorer and the desktop shortcut show, which can't change with the theme."""
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "src"))
from PIL import Image  # noqa: E402

import logo  # noqa: E402
from app import THEMES  # noqa: E402

assets = ROOT / "assets"
green = THEMES["Toxic Green"]
logo.save_ico(assets / "icon.ico", logo.tile_color(green), green["ACCENT"])
logo.draw(256, logo.tile_color(green), green["ACCENT"]).save(assets / "icon.png")

preview = Image.new("RGBA", (90 + 300 * len(THEMES) // 2, 330), (20, 20, 24, 255))
for k, (name, t) in enumerate(THEMES.items()):
    x, y = 12 + (k % 3) * 300, 12 + (k // 3) * 160
    for size in (128, 48, 32, 24, 16):
        preview.alpha_composite(logo.draw(size, logo.tile_color(t), t["ACCENT"]), (x, y + 128 - size))
        x += size + 10
(ROOT / "build").mkdir(exist_ok=True)
preview.save(ROOT / "build" / "icon_preview.png")   # build/ is git-ignored scratch space
print("wrote assets/icon.ico, assets/icon.png and build/icon_preview.png")
