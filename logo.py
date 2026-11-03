"""The JondisBoard logo: a matte rounded tile with a waveform in the theme's accent colour.
Drawn in code (not a picture file), so every theme gets its own version and every size is drawn
for its own pixel grid instead of being shrunk from a big one."""
from PIL import Image, ImageDraw

ICO_SIZES = (16, 20, 24, 32, 40, 48, 64, 128, 256)


def _mix(a, b, t):
    ca = [int(a[i:i + 2], 16) for i in (1, 3, 5)]
    cb = [int(b[i:i + 2], 16) for i in (1, 3, 5)]
    return "#" + "".join(f"{round(x + (y - x) * t):02x}" for x, y in zip(ca, cb))


def tile_color(theme):
    """The tile behind the mark, from a theme dict (BG, CARD, ACCENT...)."""
    return _mix(theme["BG"], theme["CARD"], 0.7)


def draw(size, tile, mark, tile_visible=True):
    """The logo at size x size px. Small sizes (taskbar, title bar) get three thicker bars so they
    stay crisp; from 32 px up it's the full five-bar waveform."""
    k = 4   # supersample, then scale down for smooth edges
    n = size * k
    im = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    if tile_visible:
        inset = round(n * 0.04)
        d.rounded_rectangle((inset, inset, n - inset - 1, n - inset - 1), radius=n * 0.225, fill=tile)
    if size <= 24:   # bolder, so it still reads in the title bar and taskbar
        heights, bar, gap, span = (0.55, 1.0, 0.55), max(2, round(size * 0.125)), 2, size * 0.68
    else:
        heights, bar, span = (0.34, 0.62, 0.86, 0.62, 0.34), max(2, round(size * 0.1)), size * 0.56
        gap = max(1, round(size * 0.05))
    # whole pixels at the target size, so the bars don't blur at 16-24 px
    total = len(heights) * bar + (len(heights) - 1) * gap
    x = (size - total) // 2
    for h in heights:
        hh = max(bar, round(span * h))
        y = (size - hh) / 2
        d.rounded_rectangle((x * k, y * k, (x + bar) * k - 1, (y + hh) * k - 1), radius=bar * k / 2, fill=mark)
        x += bar + gap
    return im.resize((size, size), Image.LANCZOS)


def save_ico(path, tile, mark):
    """A Windows .ico with every size drawn separately."""
    frames = [draw(s, tile, mark) for s in ICO_SIZES]
    frames[-1].save(path, format="ICO", sizes=[(s, s) for s in ICO_SIZES], append_images=frames[:-1])
