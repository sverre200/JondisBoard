"""JondisBoard: play clips into Discord / games through a virtual audio cable."""
import ctypes
import json
import logging
import os
import re
import shutil
import sys
import tempfile
import threading
import time
import tkinter as tk
import tkinter.font as tkfont
from collections import OrderedDict
from contextlib import contextmanager
from pathlib import Path
from tkinter import filedialog

import customtkinter as ctk
from PIL import Image, ImageDraw, ImageGrab, ImageOps, ImageTk

import engine
import importer
import logo
from hotkeys import HotkeyManager, binding_label

ROOT = importer.APP_DIR
SOUNDS = ROOT / "sounds"
TRASH = SOUNDS / "_deleted"
CONFIG = ROOT / "config.json"
ASSETS = Path(getattr(sys, "_MEIPASS", ROOT))  # bundled files when running as the exe
TILE_WIDTH = 250   # narrowest a tile gets (columns stretch to fill)
TILE_HEIGHT = 80
ART = 52            # the picture square on each tile
ALL, NO_GROUP = "*", ""   # special group views

logging.basicConfig(filename=ROOT / "soundboard.log", level=logging.INFO,
                    format="%(asctime)s %(levelname)s %(message)s")
log = logging.getLogger("soundboard")

DEFAULTS = {
    "mic": None, "cable": None, "monitor": None,
    "mic_volume": 1.0, "clip_volume": 0.8, "monitor_volume": 0.6,
    "mic_passthrough": True,
    "hotkeys": {},             # clip path inside sounds/ ("Group/clip.wav") -> binding
    "stop_hotkey": None,
    "group": ALL,              # group shown last time
    "group_colors": {},        # group -> "#rrggbb"
    "order": [],               # clip names in the order you dragged them into (new ones go last)
    "solo": False,             # one sound at a time: starting one stops the others
    "toggle_stop": False,      # pressing a sound that's playing stops it (instead of restarting it)
    "cookies_from_browser": None,
    "theme": "Toxic Green",
    "geometry": None,          # window size/position last time
}

# ---------- look ----------
# Colour themes. apply_theme() copies one into the module-level names below; the UI reads
# those names while it's built, so switching theme rebuilds the window (see App.switch_theme).
THEMES = {
    "Toxic Green": dict(   # matches the icon
        BG="#080d09", SURFACE="#0e1610", CARD="#142018", CARD_HOVER="#1b2b20", CARD_PLAYING="#1d3d23",
        BORDER="#22362a", TEXT="#e6f5e9", MUTED="#7f9a86", ACCENT="#45e33f", ACCENT_HOVER="#36c431",
        ACCENT_TEXT="#04120a"),
    "Midnight Purple": dict(
        BG="#0f0f14", SURFACE="#16161e", CARD="#1e1e29", CARD_HOVER="#262634", CARD_PLAYING="#271f4d",
        BORDER="#2b2b3a", TEXT="#ececf3", MUTED="#8a8aa0", ACCENT="#7c5cff", ACCENT_HOVER="#6948f0",
        ACCENT_TEXT="#ffffff"),
    "Ocean Blue": dict(
        BG="#0a0f17", SURFACE="#0f1723", CARD="#16202f", CARD_HOVER="#1d2a3d", CARD_PLAYING="#16305a",
        BORDER="#22324a", TEXT="#e6eef9", MUTED="#7e91ab", ACCENT="#3b8cff", ACCENT_HOVER="#2a74e0",
        ACCENT_TEXT="#ffffff"),
    "Sunset Orange": dict(
        BG="#120c09", SURFACE="#1a120e", CARD="#241914", CARD_HOVER="#30211a", CARD_PLAYING="#4a2814",
        BORDER="#3a2a20", TEXT="#f8ece4", MUTED="#a8907f", ACCENT="#ff8a2b", ACCENT_HOVER="#e8741a",
        ACCENT_TEXT="#1a0d04"),
    "Crimson": dict(
        BG="#110a0c", SURFACE="#191013", CARD="#23161a", CARD_HOVER="#2e1d23", CARD_PLAYING="#4a1622",
        BORDER="#3a232a", TEXT="#f7e8eb", MUTED="#a8868d", ACCENT="#ff3d5e", ACCENT_HOVER="#e02848",
        ACCENT_TEXT="#ffffff"),
    "Graphite": dict(
        BG="#0e0e0f", SURFACE="#151517", CARD="#1d1d20", CARD_HOVER="#26262a", CARD_PLAYING="#2e2e34",
        BORDER="#2c2c31", TEXT="#ededef", MUTED="#8b8b93", ACCENT="#e6e6ea", ACCENT_HOVER="#c9c9cf",
        ACCENT_TEXT="#111113"),
}
GOOD = "#3ddc97"
WARN = "#ffb547"
DANGER = "#f04566"
DANGER_HOVER = "#d63655"
FONT = "Segoe UI"

# Motion: every open, close and hover in the app uses these, so it all moves the same way.
MOTION_OPEN = 170      # menus, popovers, toasts and dialogs appearing
MOTION_CLOSE = 110     # ...and going away (quicker: closing should never hold you up)
MOTION_HOVER = 90      # a highlight coming on
MOTION_UNHOVER = 170   # ...and fading off (slower, so sweeping across a list leaves a soft trail)
MOTION_FLASH = 360     # a tile lighting up when its sound fires
SLIDE = 7              # px a popup glides while it fades in (and half that on the way out)


def mix(a, b, t):
    """Blend hex colour a toward b by t (0..1)."""
    ca = [int(a[i:i + 2], 16) for i in (1, 3, 5)]
    cb = [int(b[i:i + 2], 16) for i in (1, 3, 5)]
    return "#" + "".join(f"{round(x + (y - x) * t):02x}" for x, y in zip(ca, cb))


def apply_theme(name):
    t = THEMES.get(name) or THEMES["Toxic Green"]
    globals().update(t)
    # derived from each theme the same way, so every theme gets them
    globals().update(
        ELEVATED=mix(t["SURFACE"], t["CARD"], 0.6),          # menus, popovers, toasts
        EDGE=mix(t["BORDER"], t["TEXT"], 0.12),              # their 1 px outline (Windows draws it)
        ACCENT_SOFT=mix(t["CARD"], t["ACCENT"], 0.16),       # a quiet button that's switched on
        ACCENT_SOFT_HOVER=mix(t["CARD"], t["ACCENT"], 0.26),
        ACCENT_EDGE=mix(t["BORDER"], t["ACCENT"], 0.6),
    )


apply_theme("Toxic Green")


def theme_icon():
    """The logo in the current theme as a .ico, for the title bar and taskbar. Drawn once per theme
    into the temp folder (Windows needs a file); the name has the colours in it, so a changed
    theme gets a fresh one."""
    path = Path(tempfile.gettempdir()) / "JondisBoard" / f"logo1_{ACCENT[1:]}_{BG[1:]}.ico"
    if not path.exists():
        try:
            path.parent.mkdir(parents=True, exist_ok=True)
            logo.save_ico(path, logo.tile_color(globals()), ACCENT)
        except Exception:
            log.exception("could not write the theme icon")
            return ASSETS / "icon.ico"
    return path


def crisp_icons(win, path):
    """Tk hands Windows one icon and lets it scale (soft in the taskbar). Give it the .ico's own
    frames at the exact sizes the title bar (small) and taskbar / Alt+Tab (big) use."""
    try:
        user32 = ctypes.windll.user32
        user32.LoadImageW.restype = ctypes.c_void_p
        user32.SendMessageW.argtypes = (ctypes.c_void_p, ctypes.c_uint, ctypes.c_size_t, ctypes.c_void_p)
        hwnd = int(win.wm_frame(), 16)
        for which, metric in ((0, 49), (1, 11)):   # ICON_SMALL / SM_CXSMICON, ICON_BIG / SM_CXICON
            px = user32.GetSystemMetrics(metric)
            h = user32.LoadImageW(None, str(path), 1, px, px, 0x10)   # IMAGE_ICON, LR_LOADFROMFILE
            if h:
                user32.SendMessageW(hwnd, 0x0080, which, h)       # WM_SETICON
    except Exception:
        log.exception("could not set window icons")


def font(size=13, weight="normal"):
    return ctk.CTkFont(family=FONT, size=size, weight=weight)


def load_config():
    cfg = dict(DEFAULTS)
    if CONFIG.exists():
        try:
            cfg.update(json.loads(CONFIG.read_text(encoding="utf-8")))
        except Exception:
            log.exception("config.json unreadable, using defaults")
    return cfg


def pretty_key(binding):
    return binding_label(binding).title()


def group_of(name):
    """'Memes/clip.wav' -> 'Memes', 'clip.wav' -> ''."""
    parent = Path(name).parent.as_posix()
    return "" if parent == "." else parent


GROUP_COLORS = ["#7c5cff", "#3b82f6", "#14b8a6", "#22c55e", "#eab308", "#f97316", "#ef4444", "#ec4899"]


_dots = {}


def dot_image(color, size=10):
    if (color, size) not in _dots:
        im = Image.new("RGBA", (size * 4, size * 4), (0, 0, 0, 0))
        ImageDraw.Draw(im).ellipse((0, 0, size * 4 - 1, size * 4 - 1), fill=color)
        _dots[(color, size)] = ctk.CTkImage(im, size=(size, size))
    return _dots[(color, size)]


def plus_image(color, size=12):
    """A crisp bold plus for '+ Import link' style buttons (a text '+' sits small and low)."""
    key = ("plus", color, size)
    if key not in _dots:
        n = size * 4
        im = Image.new("RGBA", (n, n), (0, 0, 0, 0))
        d = ImageDraw.Draw(im)
        w = round(n * 0.2)
        d.rounded_rectangle(((n - w) // 2, 0, (n + w) // 2, n - 1), radius=w // 2, fill=color)
        d.rounded_rectangle((0, (n - w) // 2, n - 1, (n + w) // 2), radius=w // 2, fill=color)
        _dots[key] = ctk.CTkImage(im, size=(size, size))
    return _dots[key]


def icon(name, color, size=14):
    """One family of line icons, drawn at 4x and scaled down so they're crisp at any DPI and all
    share the same stroke weight. (Text glyphs like '+' or '✓' sit at odd heights in Segoe UI.)"""
    key = ("icon", name, color, size)
    if key not in _dots:
        _dots[key] = ctk.CTkImage(icon_pil(name, color, size), size=(size, size))
    return _dots[key]


def icon_pil(name, color, size=14):
    """icon() as a PIL image at 4x size (for drawing into other pictures)."""
    n = size * 4
    im = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    w = max(3, round(n * 0.11))

    def p(x, y):
        return x * n, y * n

    def line(*pts):
        pts = [p(*q) for q in pts]
        d.line(pts, fill=color, width=w, joint="curve")
        r = w / 2   # round caps
        for x, y in (pts[0], pts[-1]):
            d.ellipse((x - r, y - r, x + r, y + r), fill=color)

    if name == "search":
        d.ellipse(p(.1, .1) + p(.68, .68), outline=color, width=w)
        line((.64, .64), (.88, .88))
    elif name == "sliders":
        for y, k in ((.22, .66), (.5, .32), (.78, .58)):
            line((.1, y), (.9, y))
            d.ellipse(p(k - .12, y - .12) + p(k + .12, y + .12), fill=color)
    elif name == "folder":
        line((.08, .8), (.08, .2), (.38, .2), (.5, .33), (.92, .33), (.92, .8), (.08, .8))
    elif name == "refresh":
        d.arc(p(.13, .13) + p(.87, .87), start=40, end=320, fill=color, width=w)
        # arrow head where the arc ends (top right), pointing on round the circle
        tip, base = (.93, .43), [(.65, .34), (.88, .15)]
        d.polygon([p(*tip)] + [p(*b) for b in base], fill=color)
    elif name == "note":
        line((.64, .7), (.64, .12), (.88, .24))
        d.ellipse(p(.2, .56) + p(.64, .9), fill=color)
    elif name == "stop":
        d.rounded_rectangle(p(.18, .18) + p(.82, .82), radius=n * .14, fill=color)
    elif name == "check":
        line((.16, .54), (.4, .76), (.86, .26))
    elif name in ("chevron-down", "chevron-up"):
        a, b = (.36, .66) if name == "chevron-down" else (.66, .36)
        line((.18, a), (.5, b), (.82, a))
    elif name == "chevron-right":
        line((.36, .18), (.66, .5), (.36, .82))
    elif name == "bolt":
        d.polygon([p(.6, .04), p(.18, .57), p(.46, .57), p(.4, .96), p(.82, .42), p(.54, .42)], fill=color)
    elif name == "link":
        for a in (.04, .42):
            d.rounded_rectangle(p(a, .3) + p(a + .54, .7), radius=n * .2, outline=color, width=w)
    elif name == "x":
        line((.24, .24), (.76, .76))
        line((.76, .24), (.24, .76))
    elif name == "more":
        for x in (.18, .5, .82):
            d.ellipse(p(x - .1, .4) + p(x + .1, .6), fill=color)
    return im


MENU_WIDTH = 220


WM_SETREDRAW = 0x000B
# RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_UPDATENOW | RDW_FRAME
RDW_REPAINT_ALL = 0x0001 | 0x0004 | 0x0080 | 0x0100 | 0x0400


def later(widget, ms, fn):
    """widget.after(ms, fn), but the timer lives on the main window and only runs fn if widget still
    exists. A timer set on a widget itself breaks when the widget is destroyed first, or destroyed from
    inside that very timer (a fade ending in destroy()): tkinter frees the callback's name early, a new
    callback can get the same name, and tkinter's clean-up then deletes the wrong one. That left tiles
    holding dead callbacks (switching theme crashed while closing them) and made stale timers call
    unrelated handlers with no event."""
    def run():
        try:
            if widget.winfo_exists():
                fn()
        except tk.TclError:
            pass
    return widget._root().after(ms, run)


def animate(widget, duration_ms, step, done=None, delay_ms=0, warmup=False):
    """Run step(eased) with eased going 0 -> 1 over duration_ms, then done().
    Timed by the clock, not by counting frames: a late frame just lands further along instead of
    stretching the animation or making it jump, so it stays even. Ease-out cubic (fast, then soft).
    step() returning False stops it there, without done() (something newer took over).
    warmup: for a window that just appeared; its widgets draw right after the first tick (which can
    take ~20 ms), so the clock only starts on the tick after that, or the fade would jump."""
    # warmup: wait until idle (every pending event handled, including the widgets drawing
    # themselves), then keep it at an invisible 1 % until frames arrive on time. Windows spends
    # its first few frames (~15-20 ms each) setting up a brand-new see-through window; doing that
    # while invisible means the visible fade runs evenly from its first frame. Capped at 12 frames.
    state = {"warmup": warmup, "frames": 0, "last": None, "streak": 0, "lead": 0.007 if warmup else 0.0}

    def tick():
        try:
            if not widget.winfo_exists():
                return
            if state["warmup"]:
                now = time.perf_counter()
                # a timed frame (not the quick hop after idle) that arrived within 11 ms
                if state["frames"] >= 2 and now - state["last"] < 0.011:
                    state["streak"] += 1
                else:
                    state["streak"] = 0
                on_time = state["streak"] >= 2
                state["last"] = now
                state["frames"] += 1
                # 1 %, not 0: Windows doesn't compose a fully transparent window at all, so the
                # setup cost would only hit on the first visible frame
                if step(0.01) is False:
                    return
                if state["frames"] == 1:
                    host.after_idle(lambda: host.after(0, tick))
                    return
                if not (on_time or state["frames"] >= 12):
                    host.after(7, tick)
                    return
                state["warmup"] = False
            now = time.perf_counter()
            # after a warmup, start one frame in: stepping back to exactly 0 would make Windows
            # drop the window's composition and pay that ~35 ms again on the next frame
            start = state.setdefault("start", now - state["lead"])
            t = min(1.0, (now - start) * 1000 / duration_ms)
            if step(1 - (1 - t) ** 3) is False:
                return
            if t < 1.0:
                host.after(7, tick)   # ~140 ticks/s; needs the 1 ms system timer (smooth_timers)
            elif done:
                done()
        except tk.TclError:
            pass   # the window went away mid-animation

    # timers on the main window, never on the widget: see later() (a fade often ends by destroying it)
    host = widget._root()
    host.after(delay_ms, tick)


def smooth_timers(on):
    """Windows' timer only ticks every 15.6 ms by default, so after(7) really fires every ~16 ms
    and animations step unevenly. Ask for 1 ms timers while the app runs (what browsers and games
    do), and hand it back on exit."""
    try:
        (ctypes.windll.winmm.timeBeginPeriod if on else ctypes.windll.winmm.timeEndPeriod)(1)
    except Exception:
        pass


def fade_to(owner, name, colors, apply, ms=MOTION_HOVER):
    """Ease a widget's colours to `colors` (a tuple of hex strings), calling apply(*now) each frame.
    Starts from wherever the last fade under this name got to, so a hover that flips half-way
    turns around smoothly instead of jumping. The first call just sets them."""
    cur = owner.__dict__.setdefault("_fade_now", {})
    runs = owner.__dict__.setdefault("_fade_run", {})
    token = runs[name] = object()
    frm = cur.get(name)
    if frm is None or frm == colors or ms <= 0:
        cur[name] = colors
        apply(*colors)
        return
    last = [frm]

    def step(e):
        if runs.get(name) is not token:
            return False   # a newer fade took over
        now = tuple(mix(a, b, e) for a, b in zip(frm, colors))
        cur[name] = now
        if now != last[0]:   # redrawing a customtkinter widget isn't free; skip identical frames
            last[0] = now
            apply(*now)
    animate(owner, ms, step)


def colorref(h):
    r, g, b = (int(h[i:i + 2], 16) for i in (1, 3, 5))
    return r | (g << 8) | (b << 16)


def win_pos(win):
    """Screen position (x, y) of a toplevel's outer frame, as geometry() places it."""
    m = re.search(r"([+-]-?\d+)([+-]-?\d+)$", win.geometry())
    return (int(m.group(1).replace("+", "")), int(m.group(2).replace("+", ""))) if m else (0, 0)


def unlayer(win):
    """Turn a window back into a normal one after fading it. Tk leaves it 'layered' (needed for
    see-through), and Windows then repaints the whole layered window whenever something changes
    on top of it, like a menu fading in, so it jitters."""
    try:
        user32 = ctypes.windll.user32
        hwnd = int(win.wm_frame(), 16)
        style = user32.GetWindowLongW(hwnd, -20)                 # GWL_EXSTYLE
        if style & 0x80000:                                      # WS_EX_LAYERED
            user32.SetWindowLongW(hwnd, -20, style & ~0x80000)
            user32.RedrawWindow(hwnd, None, None, RDW_REPAINT_ALL)
    except Exception:
        log.exception("could not unlayer a window")


def relayer(win):
    """Undo unlayer() before fading a window again. Tk doesn't know unlayer() took the style off,
    so it still thinks the window is see-through-capable: its -alpha calls then silently do nothing
    and the window stays fully visible (restoring from minimised painted in piece by piece)."""
    try:
        user32 = ctypes.windll.user32
        hwnd = int(win.wm_frame(), 16)
        style = user32.GetWindowLongW(hwnd, -20)
        if not style & 0x80000:
            user32.SetWindowLongW(hwnd, -20, style | 0x80000)
    except Exception:
        log.exception("could not re-layer a window")


class _MenuWindow(ctk.CTkToplevel):
    # popups have no title bar, so skip customtkinter's title bar recolouring: it hides the
    # window and runs a full update() pass, which made every menu slow to appear
    _deactivate_windows_window_header_manipulation = True


def popup_window(master, fill=None):
    """A borderless floating window (menu, popover, toast), hidden until reveal().
    On Windows 11 the system draws its rounded corners (anti-aliased, unlike a see-through colour
    key), a 1 px outline in EDGE and the soft drop shadow menus have everywhere else in Windows."""
    win = _MenuWindow(master, fg_color=fill or ELEVATED)
    win.motion = None   # the reveal()/fade_away() running on it
    win.withdraw()
    win.overrideredirect(True)
    win.attributes("-alpha", 0.0)
    win.update_idletasks()
    try:
        user32, dwm = ctypes.windll.user32, ctypes.windll.dwmapi
        hwnd = int(win.wm_frame(), 16)
        for attr, value in ((33, 2), (34, colorref(EDGE))):   # corner preference = round, border colour
            v = ctypes.c_int(value)
            dwm.DwmSetWindowAttribute(hwnd, attr, ctypes.byref(v), ctypes.sizeof(v))
        # Never take focus: an activated popup turns the main window 'inactive', and Windows
        # repaints its title bar in the inactive colour. Esc is handled by the main window.
        user32.SetWindowLongW(hwnd, -20, user32.GetWindowLongW(hwnd, -20) | 0x08000000)  # WS_EX_NOACTIVATE
    except Exception:
        log.exception("popup window setup")
    return win


def owned_by(win, owner):
    """Keep a popup just above `owner` (and behind whatever is in front of it, like a game) instead
    of above everything: Windows stacks owned windows over their owner and hides them with it."""
    try:
        user32 = ctypes.windll.user32
        user32.SetWindowLongPtrW.argtypes = (ctypes.c_void_p, ctypes.c_int, ctypes.c_void_p)
        user32.SetWindowLongPtrW(int(win.wm_frame(), 16), -8, int(owner.wm_frame(), 16))  # GWLP_HWNDPARENT
    except Exception:
        log.exception("popup owner")


def reveal(win, x, y, dx=0, dy=0, topmost=False, ms=MOTION_OPEN, done=None):
    """Show a popup_window at screen x, y. It starts dx, dy away, invisible, and glides into place
    while it fades in. The first frames run while it's still invisible (animate's warm-up), so its
    contents are fully drawn and Windows has set the window up before anything shows."""
    token = win.motion = object()
    x, y = int(x), int(y)
    win.geometry(f"+{x + dx}+{y + dy}")
    win.deiconify()
    try:   # deiconify() puts it on top without activating it (WS_EX_NOACTIVATE); topmost on request
        flags = 0x0001 | 0x0002 | 0x0010 | 0x0040   # no size, no move, no activate, show
        ctypes.windll.user32.SetWindowPos(int(win.wm_frame(), 16), -1 if topmost else 0, 0, 0, 0, 0, flags)
    except Exception:
        pass

    def step(e):
        if win.motion is not token:
            return False   # closed (or re-opened) mid-way: that animation has it now
        win.attributes("-alpha", e)
        if dx or dy:
            win.geometry(f"+{x + round(dx * (1 - e))}+{y + round(dy * (1 - e))}")
    animate(win, ms, step, done=done, warmup=True)


def fade_away(win, dx=0, dy=0, ms=MOTION_CLOSE, done=None):
    """Fade a window out from wherever it is (it may still be fading in), drifting dx, dy, then
    done(). Re-showing it with reveal() mid-way takes over cleanly."""
    if not win.winfo_exists():
        return
    token = win.motion = object()
    start = float(win.attributes("-alpha"))
    x0, y0 = win_pos(win)

    def step(e):
        if win.motion is not token:
            return False
        win.attributes("-alpha", start * (1 - e))
        if dx or dy:
            win.geometry(f"+{x0 + round(dx * e)}+{y0 + round(dy * e)}")
    animate(win, ms, step, done=done)


def title_bar_colors(win):
    """Give a window's title bar the theme's colours (Windows 11). Otherwise Windows paints it
    in the system accent colour (brown on this PC), which clashes with every theme."""
    try:
        win.update_idletasks()
        hwnd = ctypes.windll.user32.GetParent(win.winfo_id())
        dwm = ctypes.windll.dwmapi
        for attr, color in ((35, BG), (36, TEXT), (34, BORDER)):   # caption, caption text, border
            c = ctypes.c_int(colorref(color))
            dwm.DwmSetWindowAttribute(hwnd, attr, ctypes.byref(c), ctypes.sizeof(c))
    except Exception:
        pass   # older Windows: keeps the default title bar


def monitor_work_area(x, y):
    """(left, top, right, bottom) of the work area of the monitor showing screen point x, y.
    winfo_screenwidth() only knows the primary monitor, which pulls menus onto the main screen."""
    class RECT(ctypes.Structure):
        _fields_ = [("left", ctypes.c_long), ("top", ctypes.c_long),
                    ("right", ctypes.c_long), ("bottom", ctypes.c_long)]

    class MONITORINFO(ctypes.Structure):
        _fields_ = [("cbSize", ctypes.c_ulong), ("rcMonitor", RECT), ("rcWork", RECT), ("dwFlags", ctypes.c_ulong)]

    try:
        user32 = ctypes.windll.user32
        user32.MonitorFromPoint.restype = ctypes.c_void_p
        point = ctypes.c_longlong((int(y) << 32) | (int(x) & 0xFFFFFFFF))   # POINT passed by value
        mon = user32.MonitorFromPoint(point, 2)   # MONITOR_DEFAULTTONEAREST
        info = MONITORINFO()
        info.cbSize = ctypes.sizeof(MONITORINFO)
        if user32.GetMonitorInfoW(ctypes.c_void_p(mon), ctypes.byref(info)):
            r = info.rcWork
            return r.left, r.top, r.right, r.bottom
    except Exception:
        log.exception("monitor lookup failed")
    return 0, 0, 1920, 1040


def app_has_focus():
    """True while one of our windows is the foreground window (not the game)."""
    try:
        user32 = ctypes.windll.user32
        pid = ctypes.c_ulong()
        user32.GetWindowThreadProcessId(user32.GetForegroundWindow(), ctypes.byref(pid))
        return pid.value == os.getpid()
    except Exception:
        return True


class PopupMenu:
    """Dark rounded right-click menu with eased hover highlights and side submenus."""

    def __init__(self, app, parent=None, master=None, width=None):
        """master: the window it belongs to (a dialog, so the dialog's click grab lets it work);
        width: in unscaled pixels, e.g. to match a dropdown field."""
        self.app, self.parent = app, parent
        self.master = master or (parent.master if parent else app.root)
        self.width = width or (parent.width if parent else MENU_WIDTH)
        self.child = self.child_owner = None
        self.hovered = None
        self.opening = False
        self.on_close = None   # called once, as soon as the menu starts closing
        self.retreat = (0, -SLIDE // 2)   # where it drifts while fading out
        app.open_menus.append(self)
        self.win = popup_window(self.master)
        self.inner = ctk.CTkFrame(self.win, fg_color="transparent", width=self.width)
        self.inner.pack(padx=5, pady=5)

    @property
    def alive(self):
        return self in self.app.open_menus and self.win.winfo_exists()

    # --- building ---
    def header(self, text):
        ctk.CTkLabel(self.inner, text=text, font=font(12, "bold"), text_color=MUTED, anchor="w",
                     justify="left", width=self.width - 24, wraplength=self.width - 24).pack(
            fill="x", padx=12, pady=(5, 5))

    def _row(self, text, on_click, color=None, dot=None, enabled=True, arrow=False, checked=False):
        """One menu line. Built from a frame + labels, so the highlight covers the whole row."""
        color = color or TEXT
        row = ctk.CTkFrame(self.inner, fg_color=ELEVATED, corner_radius=7, height=32, width=self.width)
        row.pack_propagate(False)
        row.pack(fill="x", pady=1)
        fade_to(row, "lit", (ELEVATED,), lambda c: row.configure(fg_color=c))
        if dot:
            ctk.CTkLabel(row, text="", image=dot_image(dot), width=10).pack(side="left", padx=(12, 0))
        right = arrow or checked
        ctk.CTkLabel(row, text=text, font=font(13), text_color=color if enabled else MUTED, anchor="w",
                     height=30).pack(side="left", fill="x", expand=True, padx=(10 if dot else 12, 0 if right else 12))
        if right:
            img = icon("chevron-right", MUTED, 12) if arrow else icon("check", ACCENT if enabled else MUTED, 14)
            ctk.CTkLabel(row, text="", image=img, width=16, height=30).pack(side="right", padx=(0, 10))
        if enabled and not checked:   # the current choice reads normally but does nothing
            # self.alive: a closing menu is still on screen while it fades out
            for w in [row, *row.winfo_children()]:
                w.bind("<Enter>", lambda e=None: self.alive and self._enter(row), add="+")
                w.bind("<Leave>", lambda e=None: self.alive and self._leave(row, e), add="+")
                w.bind("<Button-1>", lambda e=None: self.alive and on_click(), add="+")
        return row

    FADE_OUT_MS = MOTION_CLOSE

    def after_close(self, command):
        """Close the menu, then run command once its fade-out has finished. Most actions keep the
        app busy for a moment (moving files, reloading, opening a popup), which would freeze the
        fade halfway so the menu seemed to vanish abruptly."""
        app = self.app
        self.close_all()
        app.root.after(self.FADE_OUT_MS + 15, command)

    def item(self, text, command, danger=False, dot=None, enabled=True, instant=False, checked=False):
        """instant: run straight away (e.g. Play, which should never wait).
        checked: the current choice (drawn with a tick, and not clickable)."""
        def click():
            if instant:
                self.close_all()
                command()
            else:
                self.after_close(command)
        row = self._row(text, click, DANGER if danger else TEXT, dot, enabled, checked=checked)
        row.submenu = None

    def submenu(self, text, build):
        row = self._row(text, lambda: self._open_child(row), arrow=True)
        row.submenu = build

    def separator(self):
        # plain tk frame: a 1px CTkFrame doesn't draw anything
        tk.Frame(self.inner, height=1, bg=mix(ELEVATED, BORDER, 0.8), bd=0, highlightthickness=0).pack(
            fill="x", padx=10, pady=5)

    # --- hover ---
    def _paint(self, row, on):
        fade_to(row, "lit", (CARD_HOVER if on else ELEVATED,), lambda c: row.configure(fg_color=c),
                MOTION_HOVER if on else MOTION_UNHOVER)

    def _enter(self, row):
        if self.hovered is not row:
            self.hovered = row
            self._repaint()
        if row.submenu:
            self.app.root.after(120, lambda: self._hover_open(row))
        elif self.child and not self.opening:
            self._close_child()

    def _leave(self, row, e=None):
        if pointer_inside(row, e):
            return   # moved between the row's own labels
        if self.hovered is row:
            self.hovered = None
        self._repaint()   # the row stays lit while its submenu is open

    def _hover_open(self, row):
        if self.alive and self.hovered is row:
            self._open_child(row)

    # --- showing / closing ---
    def show(self, x, y, flip_x=None, align_right=False):
        """Open at screen x, y (its top-left; with align_right, x is where its right edge goes).
        flip_x: for submenus, where to put the right edge if there's no room on the right."""
        self.win.update_idletasks()
        w, h = self.win.winfo_reqwidth(), self.win.winfo_reqheight()
        if align_right:
            x -= w
        # keep it on the monitor where it was opened (any of them, including ones left of
        # or above the main screen, which have negative coordinates)
        left, top, right, bottom = monitor_work_area(x if flip_x is None else flip_x, y)
        leftwards = False
        if x + w > right:
            leftwards = flip_x is not None
            x = flip_x - w if leftwards else right - w - 4
        x = max(left, x)
        y = max(top, min(y, bottom - h - 4))
        # submenus glide out of their parent, menus drop down a touch
        if self.parent:
            dx, dy = (SLIDE if leftwards else -SLIDE), 0
        else:
            dx, dy = 0, -SLIDE
        self.retreat = (dx // 2, dy // 2)
        reveal(self.win, x, y, dx, dy, topmost=True)
        if not self.parent:
            self.app.watch_focus()

    def contains(self, widget):
        return any(str(widget).startswith(str(m.win)) for m in self.app.open_menus)

    def _open_child(self, row):
        # building a CTkToplevel runs pending events, which can call back in here; one at a time
        if not self.alive or self.opening or (self.child and self.child_owner is row):
            return
        self.opening = True
        try:
            self._close_child()
            child = PopupMenu(self.app, parent=self)
            self.child, self.child_owner = child, row
            row.submenu(child)
            child.show(self.win.winfo_rootx() + self.win.winfo_width() + 4, row.winfo_rooty() - 5,
                       flip_x=self.win.winfo_rootx() - 4)
        finally:
            self.opening = False
        self._repaint()
        # the mouse may have moved on while this was opening
        if self.hovered is not None and self.hovered is not row:
            if self.hovered.submenu:
                self._open_child(self.hovered)
            else:
                self._close_child()

    def _close_child(self):
        # every menu below this one, even one that got lost while opening
        for m in list(self.app.open_menus):
            p = m.parent
            while p is not None and p is not self:
                p = p.parent
            if p is self:
                m._destroy()
        self.child = self.child_owner = None
        self._repaint()

    def _repaint(self):
        """Highlight exactly the hovered row and the row whose submenu is open."""
        for w in self.inner.winfo_children():
            if hasattr(w, "submenu"):
                self._paint(w, w is self.hovered or w is self.child_owner)

    def _destroy(self):
        if self.child:
            self.child._destroy()
            self.child = None
        if self in self.app.open_menus:
            self.app.open_menus.remove(self)
        if self.on_close:
            on_close, self.on_close = self.on_close, None
            on_close()
        if self.win.winfo_exists():
            win = self.win
            fade_away(win, *self.retreat, done=win.destroy)

    def close_all(self):
        self.app.close_menus()   # every menu window, so none can be left behind


_thumbs = {}   # (picture, mtime, tint, frame) -> CTkImage


def tile_art(picture, tint, frame=None):
    """art_pil() as a CTkImage (for labels)."""
    try:
        mtime = picture.stat().st_mtime if picture and picture.is_file() else None
    except OSError:
        mtime = None
    key = (str(picture) if mtime else None, mtime, tint, frame)
    if key not in _thumbs:
        _thumbs[key] = ctk.CTkImage(art_pil(picture, tint, frame), size=(ART, ART))
    return _thumbs[key]


def art_pil(picture, tint, frame=None):
    """The square on the left of every tile: the clip's picture, or a placeholder in its group's
    colour with a note on it, so every title lines up. frame 0-2: the 'playing' look (darkened,
    with equalizer bars at one of three heights)."""
    try:
        mtime = picture.stat().st_mtime if picture and picture.is_file() else None
    except OSError:
        mtime = None
    n = ART * 2   # sharp on high-DPI screens
    base = None
    if mtime:
        try:
            base = ImageOps.fit(Image.open(picture).convert("RGBA"), (n, n), Image.LANCZOS)
        except Exception:
            log.exception("bad picture %s", picture)
    if base is None and frame is not None:   # playing: just the bars, no note under them
        base = Image.new("RGBA", (n, n), mix(CARD, tint, 0.28))
    if base is None:
        base = Image.new("RGBA", (n, n), mix(CARD, tint, 0.28))
        g = round(n * 0.42)
        glyph = icon_pil("note", mix(tint, "#ffffff", 0.3), 32).resize((g, g), Image.LANCZOS)
        base.alpha_composite(glyph, ((n - g) // 2, (n - g) // 2))
    if frame is not None:
        base = Image.alpha_composite(base, Image.new("RGBA", (n, n), (0, 0, 0, 120)))
        d = ImageDraw.Draw(base)
        bw, gap = n * 0.11, n * 0.07
        x0, bottom = (n - 3 * bw - 2 * gap) / 2, n * 0.7
        for i, h in enumerate(((.45, .8, .6), (.75, .5, .9), (.6, .95, .4))[frame]):
            x = x0 + i * (bw + gap)
            d.rounded_rectangle((x, bottom - h * n * 0.4, x + bw, bottom), radius=bw / 2, fill="#ffffff")
    mask = Image.new("L", (n, n), 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, n - 1, n - 1), radius=round(n * 0.19), fill=255)
    out = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    out.paste(base, (0, 0), mask)
    return out


def fmt_duration(secs):
    """4.2s, 41s, 4:19."""
    if secs < 10:
        return f"{secs:.1f}s"
    if secs < 60:
        return f"{secs:.0f}s"
    m, s = divmod(round(secs), 60)
    return f"{m}:{s:02d}"


def clamp_lines(text, f, width, lines=2):
    """Wrap text into at most `lines` lines of `width` px, ending in '…' if the rest doesn't fit."""
    words, out, cur, i = text.split(), [], "", 0
    while i < len(words) and len(out) < lines:
        t = f"{cur} {words[i]}".strip()
        if f.measure(t) <= width:
            cur, i = t, i + 1
            continue
        if not cur:   # a single word wider than the line: break it
            cut = words[i]
            while len(cut) > 1 and f.measure(cut) > width:
                cut = cut[:-1]
            words[i] = words[i][len(cut):]
            if not words[i]:
                i += 1
            cur = cut
        out.append(cur)
        cur = ""
    if cur and len(out) < lines:
        out.append(cur)
        cur = ""
    if out and (i < len(words) or cur):
        last = out[-1]
        while last and f.measure(last + "…") > width:
            last = last[:-1]
        out[-1] = last.rstrip() + "…"
    return "\n".join(out)


def set_hover_color(b, color):
    """Change a button() hover colour (its pressed look returns to this)."""
    b.hover_base = color
    b.configure(hover_color=color)


def button(master, text, command, kind="secondary", icon_name=None, width=None, height=36, **kw):
    """The app's one button. Sized to its label with the same side padding everywhere (unless a
    width is given), an optional icon in the label's colour, and a pressed look, so a click feels
    like it landed."""
    styles = {
        "primary": dict(fg_color=ACCENT, hover_color=ACCENT_HOVER, text_color=ACCENT_TEXT),
        "secondary": dict(fg_color=CARD, hover_color=CARD_HOVER, text_color=TEXT,
                          border_width=1, border_color=BORDER),
        "ghost": dict(fg_color="transparent", hover_color=CARD, text_color=MUTED),
        "danger": dict(fg_color=DANGER, hover_color=DANGER_HOVER, text_color="#ffffff"),
    }
    f = font(13, "bold" if kind == "primary" else "normal")
    opts = dict(height=height, corner_radius=10, font=f)
    opts.update(styles[kind])
    if icon_name:
        opts.update(image=icon(icon_name, opts["text_color"], 14), compound="left")
    opts.update(kw)
    if width is None:
        has_image = opts.get("image") is not None
        width = (f.measure(text) if text else 0) + (22 if has_image and text else 14 if has_image else 0) + \
            (32 if text else 22)
    b = ctk.CTkButton(master, text=text, command=command, width=width, **opts)
    b.hover_base = opts["hover_color"]
    base = b.hover_base if b.hover_base != "transparent" else CARD
    pressed = mix(base, "#000000", 0.16) if kind in ("primary", "danger") else mix(base, TEXT, 0.07)

    def press(e):
        if b.cget("state") != "disabled":
            b.configure(hover_color=pressed)
    b.bind("<Button-1>", press, add="+")
    b.bind("<ButtonRelease-1>", lambda e=None: b.winfo_exists() and b.configure(hover_color=b.hover_base), add="+")
    return b


class Toggle(ctk.CTkCanvas):
    """An on/off switch whose knob glides across while the track fades to the accent colour
    (customtkinter's switch jumps). Drives a BooleanVar; setting the variable from elsewhere
    (e.g. clicking the setting's words) animates it too."""
    W, H = 40, 22

    def __init__(self, master, variable, command=None, bg=None):
        self.s = s = master._get_widget_scaling()
        super().__init__(master, width=round(self.W * s), height=round(self.H * s), bg=bg or BG,
                         highlightthickness=0, bd=0)
        self.var, self.command = variable, command
        self.t = 1.0 if variable.get() else 0.0   # 0 = off, 1 = on (animated in between)
        self.hover = False
        self.track = self.create_image(0, 0, anchor="nw")
        self.knob = self.create_image(0, 0, anchor="nw")
        self.bind("<Button-1>", lambda e=None: self.toggle())
        self.bind("<Enter>", lambda e=None: self._set_hover(True))
        self.bind("<Leave>", lambda e=None: self._set_hover(False))
        variable.trace_add("write", lambda *a: self.winfo_exists() and self._slide_to(1.0 if self.var.get() else 0.0))
        self._draw()

    def toggle(self):
        self.var.set(not self.var.get())
        if self.command:
            self.command()

    def _set_hover(self, on):
        self.hover = on
        self._draw()

    def _slide_to(self, goal):
        start = self.t
        token = self._run = object()

        def step(e):
            if self._run is not token:
                return False
            self.t = start + (goal - start) * e
            self._draw()
        animate(self, MOTION_OPEN, step)

    def _draw(self):
        s, t = self.s, self.t
        w, h = round(self.W * s), round(self.H * s)
        off = mix(BORDER, TEXT, 0.1) if self.hover else BORDER
        on = mix(ACCENT, "#ffffff", 0.12) if self.hover else ACCENT
        track = mix(off, on, t)
        self.itemconfigure(self.track, image=photo(("toggle", w, h, track), lambda: pill_pil(w, h, track), (w, h)))
        d, pad = h - round(6 * s), round(3 * s)
        knob = mix(mix(TEXT, BG, 0.15), "#ffffff", t)
        self.itemconfigure(self.knob, image=photo(("knob", d, knob), lambda: pill_pil(d, d, knob), d))
        self.coords(self.knob, round(pad + (w - d - 2 * pad) * t), pad)


def labelled_toggle(master, text, variable, command=None, bg=None):
    """A Toggle with its words to the right (clicking the words flips it too). Returns the frame."""
    row = ctk.CTkFrame(master, fg_color="transparent")
    Toggle(row, variable, command, bg=bg).pack(side="left")
    words = ctk.CTkLabel(row, text=text, font=font(13), text_color=TEXT, height=22)
    words.pack(side="left", padx=(10, 0))
    words.bind("<Button-1>", lambda e=None: (variable.set(not variable.get()), command and command()))
    return row


class Select(ctk.CTkFrame):
    """A dropdown field that opens JondisBoard's own dark menu.
    (CTkOptionMenu opens Windows' native menu, which looks out of place.)"""

    def __init__(self, master, app, variable, values=(), command=None, height=34):
        super().__init__(master, fg_color=CARD, corner_radius=10, border_width=1, border_color=BORDER,
                         height=height)
        self.app, self.var, self.values, self.command = app, variable, list(values), command
        self.pack_propagate(False)
        self.label = ctk.CTkLabel(self, text="", font=font(13), text_color=TEXT, anchor="w", height=height - 6)
        self.label.pack(side="left", fill="x", expand=True, padx=(12, 4), pady=3)
        self.chev = ctk.CTkLabel(self, text="", image=icon("chevron-down", MUTED, 12), width=14, height=height - 6)
        self.chev.pack(side="right", padx=(0, 12), pady=3)
        self.hover = self.is_open = False
        for w in (self, self.label, self.chev):
            w.bind("<Enter>", lambda e=None: self._hover(True), add="+")
            w.bind("<Leave>", self._leave, add="+")
            w.bind("<Button-1>", self.open, add="+")
        variable.trace_add("write", lambda *a: self._sync())
        self._sync()
        self._look()

    def set_values(self, values):
        self.values = list(values)

    def _sync(self):
        self.label.configure(text=self.var.get() or "Choose...")

    def _look(self):
        """Eased hover, and an accent outline while its list is open."""
        fg = CARD_HOVER if self.hover or self.is_open else CARD
        border = ACCENT_EDGE if self.is_open else BORDER
        fade_to(self, "look", (fg, border), lambda f, b: self.configure(fg_color=f, border_color=b),
                MOTION_HOVER if self.hover or self.is_open else MOTION_UNHOVER)

    def _hover(self, on):
        if on != self.hover:
            self.hover = on
            self._look()

    def _leave(self, e):
        under = self.winfo_containing(e.x_root, e.y_root)
        if under is None or not str(under).startswith(str(self)):
            self._hover(False)

    def _set_open(self, on):
        if self.winfo_exists() and on != self.is_open:
            self.is_open = on
            self.chev.configure(image=icon("chevron-up" if on else "chevron-down", TEXT if on else MUTED, 12))
            self._look()

    def open(self, event=None):
        app = self.app
        if app.open_menus and getattr(app.active_menu, "owner", None) is self:
            app.close_menus()   # clicking the field again closes it
            return
        s = self._get_widget_scaling()
        current = self.var.get()
        values = self.values or [""]
        # as wide as the field (5 px padding each side), or wider if an item needs it:
        # 12 px text inset on both sides of the row plus room for the tick, so the highlight keeps its margins
        f = font(13)
        need = max(f.measure(v) for v in values) + 24 + 30
        width = max(round(self.winfo_width() / s) - 10, need)
        m = app._popup(event, master=self.winfo_toplevel(), width=width, keep_settings=True)
        m.owner = self
        for v in values:
            m.item(v, lambda v=v: self._pick(v), enabled=bool(v), checked=bool(v) and v == current)
        m.on_close = lambda: self._set_open(False)
        self._set_open(True)
        m.show(self.winfo_rootx(), self.winfo_rooty() + self.winfo_height() + round(6 * s))

    def _pick(self, value):
        self.var.set(value)
        if self.command:
            self.command(value)


def pointer_event(widget):
    """Stand-in for a mouse event: where the pointer is now."""
    x, y = widget.winfo_pointerxy()
    return type("PointerEvent", (), {"x_root": x, "y_root": y, "serial": None})()


def pointer_inside(widget, e=None):
    """Is the pointer (at event e, or now) over widget or one of its children?"""
    x, y = (e.x_root, e.y_root) if e is not None else widget.winfo_pointerxy()
    under = widget.winfo_containing(x, y)
    return under is not None and str(under).startswith(str(widget))


class _BMI(ctypes.Structure):
    _fields_ = [("size", ctypes.c_uint32), ("w", ctypes.c_int32), ("h", ctypes.c_int32), ("planes", ctypes.c_uint16),
                ("bits", ctypes.c_uint16), ("comp", ctypes.c_uint32), ("rest", ctypes.c_uint32 * 5)]


def screen_grab(x, y, w, h):
    """A copy of what's on screen in that rectangle (a straight screen copy: ~15 ms for the board,
    where PIL's ImageGrab takes ~70 ms and PrintWindow ~35 ms)."""
    user32, gdi32 = ctypes.windll.user32, ctypes.windll.gdi32
    sdc = user32.GetDC(0)
    mdc = gdi32.CreateCompatibleDC(sdc)
    bmp = gdi32.CreateCompatibleBitmap(sdc, w, h)
    old = gdi32.SelectObject(mdc, bmp)
    try:
        gdi32.BitBlt(mdc, 0, 0, w, h, sdc, x, y, 0x00CC0020)   # SRCCOPY
        bmi = _BMI(ctypes.sizeof(_BMI), w, -h, 1, 32, 0)
        buf = ctypes.create_string_buffer(w * h * 4)
        gdi32.GetDIBits(mdc, bmp, 0, h, buf, ctypes.byref(bmi), 0)
        return Image.frombuffer("RGBA", (w, h), buf, "raw", "BGRA", 0, 1).convert("RGB")
    finally:
        gdi32.SelectObject(mdc, old)
        gdi32.DeleteObject(bmp)
        gdi32.DeleteDC(mdc)
        user32.ReleaseDC(0, sdc)


def _tile_grid(tile):
    """The layout every tile shares: picture | two-line title over (duration, group, hotkey) | bar."""
    tile.grid_propagate(False)
    tile.columnconfigure(2, weight=1)
    tile.rowconfigure(0, weight=1)


_photos = {}   # canvas images: key -> ImageTk.PhotoImage (they belong to one Tk window; cleared with _thumbs)
_tkfonts = {}


def tk_font(size, weight="normal", scale=1.0):
    """A plain Tk font at the screen's scaling (canvas text isn't scaled by customtkinter)."""
    key = (size, weight, scale)
    if key not in _tkfonts:
        _tkfonts[key] = tkfont.Font(family=FONT, size=-round(size * scale), weight=weight)
    return _tkfonts[key]


def photo(key, make, px):
    """Cache a PIL picture as a canvas image of px x px (or (w, h)) real pixels."""
    if key not in _photos:
        im = make()
        size = px if isinstance(px, tuple) else (px, px)
        _photos[key] = ImageTk.PhotoImage(im.resize(size, Image.LANCZOS) if im.size != size else im)
    return _photos[key]


def pill_pil(w, h, color, radius=None):
    """An anti-aliased rounded rectangle (drawn at 3x, so its edges are smooth once scaled down)."""
    k = 3
    im = Image.new("RGBA", (w * k, h * k), (0, 0, 0, 0))
    ImageDraw.Draw(im).rounded_rectangle((0, 0, w * k - 1, h * k - 1), radius=(radius or h / 2) * k, fill=color)
    return im.resize((w, h), Image.LANCZOS)


_card_masks = {}
_card_photos = OrderedDict()   # (w, h, fill, border, scale) -> PhotoImage, most recent last


def card_photo(w, h, fill, border, scale):
    """A tile's rounded card (fill + 1 px outline, anti-aliased) as ONE canvas image. customtkinter
    draws rounded corners from ~20 font glyphs, and those made every repaint of a tile about twice as
    expensive, which a slide of many tiles felt. The masks are made once per size; a colour pair is
    two pastes, so hover fades stay cheap. Kept to the last few hundred colour pairs."""
    key = (w, h, fill, border, scale)
    photo_ = _card_photos.get(key)
    if photo_ is not None:
        _card_photos.move_to_end(key)
        return photo_
    mk = (w, h, scale)
    if mk not in _card_masks:
        k, r, b = 3, 14 * scale, max(1, round(scale))
        outer = Image.new("L", (w * k, h * k), 0)
        ImageDraw.Draw(outer).rounded_rectangle((0, 0, w * k - 1, h * k - 1), radius=r * k, fill=255)
        inner = Image.new("L", (w * k, h * k), 0)
        ImageDraw.Draw(inner).rounded_rectangle((b * k, b * k, (w - b) * k - 1, (h - b) * k - 1),
                                                radius=max(0, r - b) * k, fill=255)
        _card_masks[mk] = (outer.resize((w, h), Image.LANCZOS), inner.resize((w, h), Image.LANCZOS))
    outer, inner = _card_masks[mk]
    im = Image.new("RGBA", (w, h), border)
    im.putalpha(outer)
    im.paste(Image.new("RGBA", (w, h), fill), (0, 0), inner)
    photo_ = _card_photos[key] = ImageTk.PhotoImage(im)
    if len(_card_photos) > 400:
        _card_photos.popitem(last=False)
    return photo_


class Tile(ctk.CTkCanvas):
    """One sound pad: click to play, right-click (or the ⋯ that shows on hover) for options.

    Drawn on ONE canvas, not built from frames and labels: each customtkinter widget is one or two
    real windows, so a board of tiles was hundreds of them, and Windows repaints those one at a
    time whenever a page comes to the front (switching groups showed tiles filling in). Colour
    changes are now just recolouring canvas items, too."""

    def __init__(self, master, app, name, show_group=False):
        self.s = s = master._get_widget_scaling()   # (master: the page's scroll area, a customtkinter frame)
        super().__init__(master, width=round(160 * s), height=round(TILE_HEIGHT * s), bg=BG,
                         highlightthickness=0, bd=0)
        self.app, self.name = app, name
        self.show_group = show_group   # in All, say which group it's in
        self.playing = False
        self.hover = False
        self.art_frame = None          # which equalizer frame the picture shows while playing
        self.progress = 0.0
        self.w = 0
        self.card = self.create_image(0, 0, anchor="nw")   # the rounded card, drawn as one image (card_photo)
        self.f_title, self.f_meta, self.f_chip = tk_font(13, "bold", s), tk_font(12, "normal", s), tk_font(11, "bold", s)
        p = lambda v: round(v * s)   # noqa: E731
        self.pic = self.create_image(p(12), p(12), anchor="nw")
        self.title = self.create_text(p(12 + ART + 12), p(11), anchor="nw", font=self.f_title, fill=TEXT)
        self.meta = self.create_text(0, 0, anchor="sw", font=self.f_meta, fill=MUTED)
        self.gdot = self.create_image(0, 0, anchor="w")
        self.gname = self.create_text(0, 0, anchor="sw", font=self.f_meta, fill=MUTED)
        self.chip_bg = self.create_image(0, 0, anchor="se")
        self.chip = self.create_text(0, 0, anchor="center", font=self.f_chip, fill=TEXT)
        self.track = self.create_rectangle(0, 0, 0, 0, width=0, fill=CARD)
        self.fill = self.create_rectangle(0, 0, 0, 0, width=0, fill=CARD)
        self.more_bg = self.create_image(0, 0, anchor="ne", state="hidden", tags=("more",))
        self.more = self.create_image(0, 0, anchor="ne", state="hidden", tags=("more",),
                                      image=photo(("more", MUTED, s), lambda: icon_pil("more", MUTED, 16), p(16)))

        # (e=None: Tk has been seen calling handlers with no event; nothing below needs one)
        self.bind("<Button-1>", self._press)
        self.bind("<B1-Motion>", self._drag_motion)
        self.bind("<ButtonRelease-1>", self._release)
        self.bind("<Button-3>", lambda e=None: app.clip_menu(e or pointer_event(self), self.name))
        self.bind("<Enter>", lambda e=None: self._set_hover(True))
        self.bind("<Leave>", self._on_leave)
        self.bind("<Configure>", lambda e=None: self._layout())
        self.tag_bind("more", "<Enter>", lambda e=None: self.itemconfigure(self.more_bg, state="normal"))
        self.tag_bind("more", "<Leave>", lambda e=None: self.itemconfigure(self.more_bg, state="hidden"))
        self.refresh()
        self._paint()

    # --- drawing ---
    def _layout(self):
        """Place everything for the tile's current width (its column stretches with the window)."""
        w = self.winfo_width()
        if w <= 1 or w == self.w:
            return
        self.w = w
        s, h = self.s, round(TILE_HEIGHT * self.s)
        p = lambda v: round(v * s)   # noqa: E731
        self.tag_lower(self.card)
        base = p(12 + ART)   # the info row lines up with the picture's bottom edge
        self.coords(self.meta, p(76), base + p(1))
        self.coords(self.chip_bg, w - p(12), base + p(2))
        self.coords(self.track, p(12), h - p(10), w - p(12), h - p(7))
        self.coords(self.more, w - p(13), p(12))
        self.coords(self.more_bg, w - p(8), p(8))
        self._place_group()
        self._place_chip()
        self._fit_title()
        self._show_progress()
        self._recolor(*getattr(self, "_fade_now", {}).get("look", self._look()[:3]))

    def _fit_title(self):
        width = round(self.w / self.s) - (12 + ART + 12 + 30) if self.w > 1 else TILE_WIDTH - (ART + 54)
        self.itemconfigure(self.title, text=clamp_lines(Path(self.name).stem, font(13, "bold"), width))

    def _place_group(self):
        x1 = (self.bbox(self.meta) or (0, 0, round(76 * self.s), 0))[2]
        y = self.coords(self.meta)[1] if self.coords(self.meta) else 0
        self.coords(self.gdot, x1 + round(10 * self.s), y - round(7 * self.s))
        self.coords(self.gname, x1 + round(21 * self.s), y)

    def _place_chip(self):
        text = self.itemcget(self.chip, "text")
        if not text:
            self.itemconfigure(self.chip_bg, state="hidden")
            return
        s = self.s
        cw, ch = round(self.f_chip.measure(text) + 14 * s), round(20 * s)
        x, y = self.coords(self.chip_bg) or (0, 0)
        color = mix(CARD, TEXT, 0.09)
        self.itemconfigure(self.chip_bg, state="normal",
                           image=photo(("chip", cw, ch, color), lambda: pill_pil(cw, ch, color, 6 * s), (cw, ch)))
        self.coords(self.chip, x - cw / 2, y - ch / 2)

    def refresh(self):
        self.w = 0   # re-lay out on the next pass, even at the same width
        self._show_art()
        self.itemconfigure(self.meta, text=fmt_duration(len(self.app.clips[self.name]) / engine.SR))
        g = group_of(self.name)
        if self.show_group and g:
            c = self.app.group_color(g) or MUTED
            d = round(7 * self.s)
            self.itemconfigure(self.gdot, state="normal", image=photo(("dot", c, d), lambda: pill_pil(d, d, c), d))
            self.itemconfigure(self.gname, text=g, state="normal")
        else:
            self.itemconfigure(self.gdot, state="hidden")
            self.itemconfigure(self.gname, text="", state="hidden")
        key = pretty_key(self.app.cfg["hotkeys"].get(self.name))
        self.itemconfigure(self.chip, text=key or "")
        self._layout()

    def _tint(self):
        return self.app.group_color(group_of(self.name)) or ACCENT

    def _show_art(self):
        pic, tint, frame, px = importer.picture_path(SOUNDS / self.name), self._tint(), self.art_frame, round(ART * self.s)
        try:
            stamp = pic.stat().st_mtime if pic.is_file() else None
        except OSError:
            stamp = None
        dim = self.dragging

        def make():
            im = art_pil(pic, tint, frame)
            if dim:   # faded toward the background, keeping the rounded corners
                veil = Image.new("RGBA", im.size, BG + "a0")
                faded = Image.alpha_composite(im, veil)
                faded.putalpha(im.getchannel("A"))
                return faded
            return im
        self.itemconfigure(self.pic, image=photo(("art", str(pic), stamp, tint, frame, px, dim), make, px))

    def set_dragging(self, on):
        """Its lifted copy is under the pointer: the spot it left goes faint (or comes back)."""
        if on == self.dragging:
            return
        self.dragging = on
        self.itemconfigure(self.title, fill=mix(TEXT, BG, 0.6) if on else TEXT)
        for item in (self.meta, self.gname):
            self.itemconfigure(item, fill=mix(MUTED, BG, 0.5) if on else MUTED)
        self._show_art()
        self._paint()

    def tick_art(self, frame):
        """Step the equalizer on the picture while it plays."""
        frame = frame if self.playing else None
        if frame != self.art_frame:
            self.art_frame = frame
            self._show_art()

    def _show_progress(self):
        x1, y1, x2, y2 = self.coords(self.track) or (0, 0, 0, 0)
        self.coords(self.fill, x1, y1, x1 + (x2 - x1) * (self.progress if self.playing else 0), y2)

    # --- states ---
    dragging = False   # it's being dragged (Dragger dims it)
    _down = None       # where the button went down: (x_root, y_root, x, y, menus were open, on ⋯)

    def _press(self, e=None):
        e = e or pointer_event(self)
        on_more = bool(self.find_withtag("current")) and "more" in self.gettags("current")
        # menus close on this same press (click-outside), so remember they were open
        self._down = (e.x_root, e.y_root, e.x_root - self.winfo_rootx(), e.y_root - self.winfo_rooty(),
                      bool(self.app.open_menus), on_more)

    def _drag_motion(self, e=None):
        if self._down is None:
            return
        e = e or pointer_event(self)
        drag = self.app.dragger
        if not drag.active:
            x0, y0, gx, gy, _, on_more = self._down
            if on_more or max(abs(e.x_root - x0), abs(e.y_root - y0)) < Dragger.THRESHOLD * self.s:
                return
            drag.begin(self, e.x_root, e.y_root, gx, gy)
        elif drag.tile is self:
            drag.move(e.x_root, e.y_root)

    def _release(self, e=None):
        """Plays on release, not press, so a press that turns into a drag never fires a sound."""
        down, self._down = self._down, None
        e = e or pointer_event(self)
        drag = self.app.dragger
        if drag.tile is self:
            drag.drop(e.x_root, e.y_root)
            return
        if down is None:
            return
        menus_were_open, on_more = down[4], down[5]
        if on_more:
            self.app.clip_menu(e, self.name)
        elif not menus_were_open and pointer_inside(self, e):   # a click that closed a menu doesn't play
            self.app.play(self.name)

    def _on_leave(self, e=None):
        if not pointer_inside(self, e):
            self._set_hover(False)

    def _set_hover(self, on):
        if on != self.hover:
            self.hover = on
            self.itemconfigure("more", state="normal" if on else "hidden")
            if on:
                self.itemconfigure(self.more_bg, state="hidden")
            self._paint()

    def set_progress(self, frac):
        playing = frac is not None
        if not playing and not self.playing:
            return   # idle: nothing to redraw
        self.progress = frac or 0
        if playing != self.playing:
            self.playing = playing
            if not playing:
                self.tick_art(None)
            self._paint()
        self._show_progress()

    _flash_until = 0.0

    def _look(self):
        """(card, outline, progress bar, accent) for the tile's current state. Cards are neutral;
        a group's colour shows in the picture, the dot and while playing."""
        accent = self._tint()
        if self.dragging:   # its lifted copy is under the pointer; the slot it left stays faint
            fg = mix(CARD, BG, 0.6)
            border = mix(BORDER, ACCENT, 0.25)
        elif self.playing:
            fg = mix(CARD, accent, 0.14 if self.hover else 0.1)
            border = accent
        else:
            fg = CARD_HOVER if self.hover else CARD
            border = mix(BORDER, TEXT, 0.12) if self.hover else BORDER
        return fg, border, accent if self.playing else fg, accent

    def _recolor(self, fg, border, progress):
        if self.w:
            self.itemconfigure(self.card, image=card_photo(self.w, round(TILE_HEIGHT * self.s), fg, border, self.s))
        self.itemconfigure(self.track, fill=fg if not self.playing else mix(fg, TEXT, 0.08))
        self.itemconfigure(self.fill, fill=progress)
        if self.w:   # the ⋯ hover backdrop follows the card
            s, d = self.s, round(26 * self.s)
            c = mix(fg, TEXT, 0.1)
            self.itemconfigure(self.more_bg, image=photo(("more_bg", c, d), lambda: pill_pil(d, round(24 * s), c, 7 * s),
                                                         (d, round(24 * s))))

    def _paint(self):
        fg, border, progress, _ = self._look()
        # a fade that's still settling after a flash keeps its slower pace
        left = (self._flash_until - time.perf_counter()) * 1000
        ms = max(left, MOTION_HOVER if self.hover or self.playing else MOTION_UNHOVER)
        fade_to(self, "look", (fg, border, progress), self._recolor, ms)

    def flash(self, strength=0.32):
        """Light up for a moment when its sound fires (click or hotkey), then settle. A dropped
        sound arriving in its new spot gets a softer one (strength)."""
        fg, border, progress, accent = self._look()
        lit = mix(fg, accent, strength)
        fade_to(self, "look", (lit, accent, progress if self.playing else lit), self._recolor, 0)
        self._flash_until = time.perf_counter() + MOTION_FLASH / 1000
        self._paint()


LINK = re.compile(r"https?://(?:www\.|m\.|music\.)?(?:instagram\.com|youtube\.com|youtu\.be)/\S+", re.I)


def find_link(text):
    """The first Instagram / YouTube link in text, else None."""
    m = LINK.search(text or "")
    return m.group(0) if m else None


class ImportJob:
    """One Quick import: a link being downloaded in the background."""
    _ids = 0

    def __init__(self, url):
        ImportJob._ids += 1
        self.id, self.url = ImportJob._ids, url
        self.source = "Instagram reel" if "instagram" in url.lower() else "YouTube video"
        self.status = "Starting"
        self.error = None
        self.full_error = None


def short_error(text):
    """yt-dlp / ffmpeg errors are long; keep the part that says what went wrong."""
    text = (text or "Something went wrong").strip().splitlines()[0]
    text = re.sub(r"^(ERROR:\s*)?(\[[^\]]+\]\s*)?([\w-]+:\s*)?", "", text)
    if "login" in text.lower() or "cookies" in text.lower():
        return "Instagram wants a login (see the README)"
    return text[:90] + ("…" if len(text) > 90 else "")


class PendingTile(ctk.CTkFrame):
    """Stands in the grid while a Quick import downloads, showing what it's doing; if it fails it
    says why, with Retry and Dismiss. Same size and layout as a Tile."""

    def __init__(self, master, app, job):
        super().__init__(master, fg_color=CARD, corner_radius=14, border_width=1, border_color=BORDER,
                         height=TILE_HEIGHT)
        self.app, self.job = app, job
        _tile_grid(self)
        self.art = ctk.CTkFrame(self, width=ART, height=ART, corner_radius=10, fg_color=CARD_HOVER)
        self.art.grid(row=0, column=0, rowspan=2, sticky="nw", padx=12, pady=(12, 0))
        self.art.pack_propagate(False)
        self.glyph = ctk.CTkLabel(self.art, text="", image=icon("link", MUTED, 18))
        self.glyph.pack(expand=True)
        self.title = ctk.CTkLabel(self, text="", font=font(13, "bold"), text_color=TEXT, anchor="nw",
                                  justify="left", height=36)
        self.title.grid(row=0, column=1, columnspan=3, sticky="nw", pady=(11, 0), padx=(0, 12))
        self.meta = ctk.CTkLabel(self, text="", font=font(12), text_color=MUTED, anchor="w", height=18)
        self.meta.grid(row=1, column=1, columnspan=3, sticky="sw", padx=(0, 12))
        # Retry and Dismiss sit in the corner (where a tile's ⋯ goes), so the reason gets the whole line
        self.actions = ctk.CTkFrame(self, fg_color="transparent")
        for name, cmd in (("refresh", lambda: app.retry_job(job)), ("x", lambda: app.dismiss_job(job))):
            button(self.actions, "", cmd, "ghost", icon_name=name, width=28, height=26,
                   text_color=TEXT).pack(side="left", padx=(2, 0))
        for w in (self, self.art, self.glyph, self.title, self.meta):   # the full reason, if it was cut short
            w.bind("<Button-1>", lambda e=None: job.error and app.toast(f"Couldn't import: {job.full_error}", "bad"))
        self.bar = ctk.CTkProgressBar(self, height=3, corner_radius=2, mode="indeterminate",
                                      fg_color=CARD_HOVER, progress_color=ACCENT, indeterminate_speed=0.6)
        self.bar.grid(row=2, column=0, columnspan=4, sticky="ew", padx=12, pady=(6, 7))
        self._shown = None
        self.update_view()
        self._pulse(True)

    def _pulse(self, up):
        """The picture square breathes while it works."""
        if not self.winfo_exists() or self.job.error:
            return
        color = mix(CARD_HOVER, ACCENT, 0.16) if up else CARD_HOVER
        fade_to(self.art, "pulse", (color,), lambda c: self.art.configure(fg_color=c), 700)
        later(self, 760, lambda: self._pulse(not up))

    def update_view(self):
        job = self.job
        failed = job.error is not None
        title = "Import failed" if failed else "Importing…"
        meta = job.error if failed else f"{job.source}  ·  {job.status}"
        self.title.configure(text=clamp_lines(title, font(13, "bold"), TILE_WIDTH - ART - 100))
        self.meta.configure(text=clamp_lines(meta, font(12), TILE_WIDTH - ART - 40, 1),
                            text_color=mix(MUTED, DANGER, 0.6) if failed else MUTED)
        if failed == self._shown:
            return
        self._shown = failed
        if failed:
            self.bar.stop()
            self.bar.grid_remove()
            self.actions.place(relx=1.0, x=-8, y=8, anchor="ne")
            self.configure(border_color=mix(BORDER, DANGER, 0.5))
            fade_to(self.art, "pulse", (mix(CARD, DANGER, 0.2),), lambda c: self.art.configure(fg_color=c), 0)
            self.glyph.configure(image=icon("x", DANGER, 16))
        else:
            self.actions.place_forget()
            self.bar.grid()
            self.bar.start()
            self.configure(border_color=BORDER)
            self.glyph.configure(image=icon("link", MUTED, 18))
            self._pulse(True)


class GroupTab(ctk.CTkFrame):
    """A group in the strip above the board: colour dot, name and a quieter count. The chosen one
    fills with a tint of its colour. Your own groups have a chevron that opens their options
    (clicking the chosen tab again does too); right-click works as well."""

    def __init__(self, master, app, key, label):
        super().__init__(master, fg_color=BG, corner_radius=16, border_width=1, border_color=BORDER)
        self.app, self.key = app, key
        self.custom = key not in (ALL, NO_GROUP)
        self.on = self.hover = False
        self.dot = None
        # pady: the labels are opaque, so they'd paint over the outline if they touched it
        if key != ALL:
            self.dot = ctk.CTkLabel(self, text="", width=8, height=28)
            self.dot.pack(side="left", padx=(13, 0), pady=2)
        self.name = ctk.CTkLabel(self, text=label, font=font(13), text_color=TEXT, height=28)
        self.name.pack(side="left", padx=(8 if self.dot else 15, 0), pady=2)
        self.count = ctk.CTkLabel(self, text="", font=font(12), text_color=MUTED, height=28)
        self.count.pack(side="left", padx=(7, 6 if self.custom else 15), pady=2)
        parts = [self, self.name, self.count] + ([self.dot] if self.dot else [])
        self.chev = None
        if self.custom:
            # always there (not only on the chosen tab), so choosing one never changes its width
            self.chev = ctk.CTkLabel(self, text="", image=icon("chevron-down", MUTED, 10), width=18, height=22,
                                     corner_radius=6)
            self.chev.pack(side="left", padx=(0, 7), pady=2)
            self.chev.bind("<Button-1>", self._menu)
            self.chev.bind("<Enter>", lambda e=None: self.chev.configure(fg_color=mix(self.cget("fg_color"), TEXT, 0.1)))
            self.chev.bind("<Leave>", lambda e=None: self.chev.configure(fg_color="transparent"))
            parts.append(self.chev)
        for w in parts:
            if w is not self.chev:
                w.bind("<Button-1>", self._click)
            if self.custom:
                w.bind("<Button-3>", lambda e=None: app.group_menu(e or pointer_event(self), key))
            w.bind("<Enter>", lambda e=None: self._set_hover(True))
            w.bind("<Leave>", self._on_leave)
        self.look = None

    drop = False   # a dragged sound is over it

    def set_drop(self, on):
        if on != self.drop:
            self.drop = on
            self._paint(MOTION_HOVER if on else MOTION_UNHOVER)

    def _click(self, e=None):
        e = e or pointer_event(self)
        if self.on and self.custom:
            self._menu(e)
        else:
            self.app.show_group(self.key)

    def _menu(self, e):
        s = self._get_widget_scaling()
        self.app.group_menu(e, self.key, at=(self.winfo_rootx(), self.winfo_rooty() + self.winfo_height() + round(6 * s)))
        return "break"

    def _on_leave(self, e=None):
        if not pointer_inside(self, e):
            self._set_hover(False)

    def _set_hover(self, on):
        if on != self.hover:
            self.hover = on
            self._paint(MOTION_HOVER if on else MOTION_UNHOVER)

    def set(self, label, n, on, fade=True):
        color = self.app.group_color(self.key) or (ACCENT if self.key == ALL else MUTED)
        look = (label, n, on, color)
        if look == self.look:
            return
        self.look, self.on, self.color = look, on, color
        self.name.configure(text=label)
        self.count.configure(text=str(n))
        if self.dot:
            self.dot.configure(image=dot_image(color, 8))
        self._paint(MOTION_OPEN if fade else 0)

    def _paint(self, ms):
        if self.drop:
            fg, border, count = mix(CARD, self.color, 0.42), self.color, TEXT
        elif self.on:
            fg, border, count = mix(CARD, self.color, 0.24), mix(BORDER, self.color, 0.75), mix(MUTED, TEXT, 0.55)
        elif self.hover:
            fg, border, count = CARD, mix(BORDER, TEXT, 0.12), MUTED
        else:
            fg, border, count = BG, BORDER, MUTED
        fade_to(self, "look", (fg, border, count),
                lambda f, b, c: (self.configure(fg_color=f, border_color=b), self.count.configure(text_color=c)), ms)


def move_in_order(order, name, anchor, after=False):
    """order with name moved to just before (or after) anchor."""
    out = [n for n in order if n != name]
    i = out.index(anchor) + (1 if after else 0) if anchor in out else len(out)
    out.insert(i, name)
    return out


class Dragger:
    """Drag a tile to re-order the board, or onto a group tab to move it into that group.

    While you drag, a lifted copy of the tile (its own popup window, with the system shadow)
    follows the pointer, the tile itself dims, and an accent marker shows where it will land.
    Dropping glides the copy into the tile's new spot. Esc cancels."""

    THRESHOLD = 6   # px the pointer moves before a press becomes a drag (so clicks stay clicks)

    def __init__(self, app):
        self.app = app
        self.tile = None
        self.ghost = None
        self.marker = None
        self.slot = None      # where it would land on the board: (index, visible names)
        self.tab = None       # the group tab it's over, if any
        self.pos = (0, 0)
        self.snap = None      # copies of the board's tiles, taken when the drag started
        self._scroll_job = None

    @property
    def active(self):
        return self.tile is not None

    # --- the lifted copy ---
    GHOST_ART = 34

    def _ghost(self):
        """A compact chip (picture + name). It hangs just below-right of the pointer, so it never
        covers the drop marker or the tab you're aiming at."""
        if self.ghost is None or not self.ghost.winfo_exists():
            win = popup_window(self.app.root)
            win.pic = ctk.CTkLabel(win, text="", width=self.GHOST_ART, height=self.GHOST_ART)
            win.pic.pack(side="left", padx=(8, 10), pady=8)
            win.title = ctk.CTkLabel(win, text="", font=font(13, "bold"), text_color=TEXT, anchor="w")
            win.title.pack(side="left", padx=(0, 14))
            self.ghost = win
        return self.ghost

    def _offset(self):
        return round(16 * self.tile.s) if self.tile is not None else 16

    def begin(self, tile, x, y, grab_x, grab_y):
        """tile was pressed at (grab_x, grab_y) inside it and the pointer has moved to x, y."""
        app = self.app
        app.close_menus()
        if app.page is not None:
            app.page.stop_slide()
        self.tile, self.grab = tile, (grab_x, grab_y)
        # copies of the tiles for the slide when it's dropped, taken now, before the chip or the faint
        # tile are drawn (skipped if a menu was just open: it could still be fading over the board)
        tile._set_hover(False)
        fade_to(tile, "look", tile._look()[:3], tile._recolor, 0)   # straight to its resting look
        self.snap = app.page.snapshot(settle=True) if app.page is not None and not (tile._down and tile._down[4]) \
            else None
        tile.set_dragging(True)
        win = self._ghost()
        a = self.GHOST_ART
        win.pic.configure(image=ctk.CTkImage(art_pil(importer.picture_path(SOUNDS / tile.name), tile._tint()),
                                             size=(a, a)))
        win.title.configure(text=clamp_lines(Path(tile.name).stem, font(13, "bold"), 200, 1))
        owned_by(win, app.root)
        off = self._offset()
        reveal(win, x + off, y + off, ms=MOTION_HOVER)   # fades in; move() keeps it with the pointer
        self.move(x, y)

    def move(self, x, y):
        if not self.active:
            return
        self.pos = (x, y)
        off = self._offset()
        self.ghost.geometry(f"+{x + off}+{y + off}")
        tab = self._tab_at(x, y)
        if tab is not self.tab:
            if self.tab is not None:
                self.tab.set_drop(False)
            self.tab = tab
            if tab is not None:
                tab.set_drop(True)
        if tab is not None:
            self.slot = None
            self._hide_marker()
        else:
            self.slot = self._slot_at(x, y)
            self._show_marker()
        self._auto_scroll(y)

    def drop(self, x, y):
        if not self.active:
            return
        self.move(x, y)
        tile, tab, slot = self.tile, self.tab, self.slot
        self._finish()
        app = self.app
        if tab is not None:   # into a group: drift towards the tab while fading, then move the files
            g = self.ghost
            tx = tab.winfo_rootx() + tab.winfo_width() // 2 - g.winfo_width() // 2
            dx, dy = (tx - win_pos(g)[0]) // 4, (tab.winfo_rooty() - win_pos(g)[1]) // 4
            fade_away(g, dx, dy, MOTION_OPEN, done=g.withdraw)
            tab.set_drop(False)
            tile.set_dragging(False)
            group = "" if tab.key == NO_GROUP else tab.key
            name = tile.name
            app.root.after(MOTION_OPEN + 20, lambda: app.move_clip(name, group, announce=True))
            return
        if slot is not None:
            index, visible = slot
            name = tile.name
            moved, snaps, self.snap = True, self.snap, None
            if index < len(visible) and visible[index] != name and (index == 0 or visible[index - 1] != name):
                app.set_order(move_in_order(app.cfg["order"], name, visible[index]), dropped=name, snaps=snaps)
            elif index == len(visible) and visible and visible[-1] != name:
                app.set_order(move_in_order(app.cfg["order"], name, visible[-1], after=True), dropped=name,
                              snaps=snaps)
            else:
                moved = False
            self._land(tile.name, moved)
            return
        self._land(tile.name)

    def cancel(self):
        self.snap = None
        if self.active:
            name = self.tile.name
            self._finish()
            self._land(name)

    def _finish(self):
        if self.tab is not None:
            self.tab.set_drop(False)
        self.tab = self.slot = None
        self._hide_marker()
        if self._scroll_job:
            self.app.root.after_cancel(self._scroll_job)
            self._scroll_job = None
        self.tile = None   # (the tile stays faint until its copy has landed on it: _land)

    def _land(self, name, moved=False):
        """The sound is in its (new) spot: it comes back at once (lighting up if it moved) while the
        lifted copy fades out, drifting toward it, the same way a drop onto a group tab ends.
        (Flying the smaller copy onto the tile never lined up, and reading the new layout first
        painted the board in its new order before anything moved.)"""
        app, g = self.app, self.ghost
        page = app.page
        tile = page.tiles.get(name) if page else None
        for t in app.all_tiles():   # every copy of it (All and its group page) comes back
            if t.name == name:
                t.set_dragging(False)
        # (winfo_manager, not winfo_ismapped: a tile just placed for its slide maps only at idle)
        if tile is None or not tile.winfo_exists() or not tile.winfo_manager():
            fade_away(g, done=g.withdraw)
            return
        if moved:
            if page.overlay is not None:   # it's under the slide's canvas: light up as that lifts
                page.after_slide = (*page.after_slide, lambda: tile.flash(0.16))
            else:
                tile.flash(0.16)
        # where the tile is headed, without asking Tk for the new layout (that would draw it)
        tx, ty = page.targets.get(name) or (tile.winfo_x(), tile.winfo_y())
        cx = page.frame.winfo_rootx() + tx + tile.winfo_width() // 2
        cy = page.frame.winfo_rooty() + ty + tile.winfo_height() // 2
        gx, gy = win_pos(g)
        dx = (cx - (gx + g.winfo_width() // 2)) // 3
        dy = (cy - (gy + g.winfo_height() // 2)) // 3
        fade_away(g, dx, dy, MOTION_OPEN, done=g.withdraw)

    # --- where it would land ---
    def _tab_at(self, x, y):
        """A group tab it could be moved into (not All, not the group it's already in)."""
        current = group_of(self.tile.name)
        for key, tab in self.app._tabs.items():
            if key == ALL or (key == NO_GROUP and not current) or key == current:
                continue
            tx, ty = tab.winfo_rootx(), tab.winfo_rooty()
            if tx - 4 <= x <= tx + tab.winfo_width() + 4 and ty - 6 <= y <= ty + tab.winfo_height() + 6:
                return tab
        return None

    def _visible(self):
        page = self.app.page
        return [n for (_, n) in sorted((pos, n) for n, pos in page.placed.items() if not isinstance(n, tuple))]

    def _slot_at(self, x, y):
        """(index into the visible tiles it would go before, visible names), or None if the pointer
        isn't over the board."""
        page, holder = self.app.page, self.app.holder
        hx, hy = holder.winfo_rootx(), holder.winfo_rooty()
        if not (hx <= x <= hx + holder.winfo_width() and hy - 20 <= y <= hy + holder.winfo_height() + 20):
            return None
        visible = self._visible()
        px, py = x - page.frame.winfo_rootx(), y - page.frame.winfo_rooty()
        gap = round(6 * self.tile.s)
        for i, n in enumerate(visible):
            t = page.tiles[n]
            tx, ty, tw, th = t.winfo_x(), t.winfo_y(), t.winfo_width(), t.winfo_height()
            if py < ty - gap or (py <= ty + th + gap and px < tx + tw / 2):
                return i, visible
        return len(visible), visible

    def _show_marker(self):
        if self.slot is None:
            self._hide_marker()
            return
        page = self.app.page
        index, visible = self.slot
        if not visible:
            return self._hide_marker()
        s = self.tile.s
        gap = round(6 * s)
        if self.marker is None or not self.marker.winfo_exists() or self.marker.master is not page.frame:
            if self.marker is not None and self.marker.winfo_exists():
                self.marker.destroy()
            self.marker = tk.Frame(page.frame, bg=ACCENT, width=max(3, round(3 * s)), bd=0, highlightthickness=0)
        # at the end of a row (or the board): after the tile before it; otherwise: before this tile
        prev = page.tiles[visible[index - 1]] if index > 0 else None
        nxt = page.tiles[visible[index]] if index < len(visible) else None
        if prev is not None and (nxt is None or nxt.winfo_y() != prev.winfo_y()):
            x, t = prev.winfo_x() + prev.winfo_width() + gap, prev
        else:
            x, t = nxt.winfo_x() - gap, nxt
        w = max(3, round(3 * s))
        tk.Frame.place(self.marker, x=x - w // 2, y=t.winfo_y() + round(8 * s), width=w,
                       height=t.winfo_height() - round(16 * s))
        self.marker.lift()

    def _hide_marker(self):
        if self.marker is not None and self.marker.winfo_exists():
            self.marker.place_forget()

    def _auto_scroll(self, y):
        """Near the top or bottom edge of the board, scroll it (faster the closer you get)."""
        holder = self.app.holder
        top, bottom = holder.winfo_rooty(), holder.winfo_rooty() + holder.winfo_height()
        edge = round(48 * self.tile.s)
        speed = -(top + edge - y) if y < top + edge else (y - (bottom - edge)) if y > bottom - edge else 0
        if speed and self._scroll_job is None:
            self._scroll_job = self.app.root.after(16, lambda: self._scroll_tick(speed))

    def _scroll_tick(self, speed):
        self._scroll_job = None
        if not self.active:
            return
        page = self.app.page
        canvas = page.frame._parent_canvas
        total = max(1, page.frame.winfo_height())
        top = canvas.yview()[0]
        canvas.yview_moveto(min(1.0, max(0.0, top + max(-24, min(24, speed / 2)) / total)))
        self.move(*self.pos)   # the slot under the pointer changed


class Page:
    """One view of the board (All, a group, or No group) with its own scroll area and tiles.
    All pages sit stacked in the same spot, fully drawn, so switching groups only raises one."""

    def __init__(self, app, key):
        self.app, self.key = app, key
        self.frame = ctk.CTkScrollableFrame(app.holder, fg_color="transparent", scrollbar_button_color=CARD,
                                            scrollbar_button_hover_color=CARD_HOVER)
        self.frame.grid(row=0, column=0, sticky="nsew")
        self.frame._parent_frame.lower()   # new pages start underneath
        self.tiles = {}       # filename -> Tile
        self.pending = {}     # Quick import job id -> PendingTile (on All and No group)
        self.placed = {}      # filename or ("job", id) -> (row, col)
        self.columns = 0
        self.query = None
        self.built = False
        self.dirty = True
        self.msg = ctk.CTkLabel(self.frame, text="", font=font(13), text_color=MUTED)
        self.empty = None
        self.geom, self.targets = {}, {}
        tk.Misc.bind(self.frame, "<Configure>", lambda e=None: self._reflow(), add="+")
        if key == ALL:   # first-run screen
            self.empty = ctk.CTkFrame(self.frame, fg_color="transparent")
            ctk.CTkLabel(self.empty, text="", image=tile_art(None, ACCENT)).pack(pady=(64, 14))
            ctk.CTkLabel(self.empty, text="No sounds yet", font=font(18, "bold"), text_color=TEXT).pack(pady=(0, 4))
            ctk.CTkLabel(self.empty, text="Copy an Instagram or YouTube link, then press Quick import (or Ctrl+V).",
                         font=font(13), text_color=MUTED).pack(pady=(0, 16))
            row = ctk.CTkFrame(self.empty, fg_color="transparent")
            row.pack()
            button(row, "Quick import", app.quick_import, "primary", icon_name="bolt").pack(side="left", padx=(0, 8))
            button(row, "Import link…", app.import_dialog, "secondary", icon_name="link").pack(side="left")

    def wants(self, name):
        return self.key == ALL or group_of(name) == self.key

    def sync(self, moved=None):
        """Match tiles to app.clips, keeping existing tile widgets so nothing flickers."""
        moved = moved or {}
        old, self.tiles = self.tiles, {}
        for name in self.app.clips:
            if not self.wants(name):
                continue
            tile = old.pop(moved.get(name, name), None) or old.pop(name, None)
            if tile is None:
                tile = Tile(self.frame, self.app, name, show_group=self.key == ALL)
            else:
                if tile.name != name:
                    if tile.name in self.placed:
                        self.placed[name] = self.placed.pop(tile.name)
                    if tile.name in self.geom:
                        self.geom[name] = self.geom.pop(tile.name)
                    tile.name = name
                tile.refresh()
                tile._paint()
            self.tiles[name] = tile
        for name, t in old.items():
            self.placed.pop(name, None)
            self.geom.pop(name, None)
            t.destroy()
        self.built = self.dirty = True

    def reorder(self):
        """Put the tiles in app.clips order (a drag changed it). Nothing about a tile changed, so
        nothing is redrawn; layout() just moves the ones whose spot changed."""
        self.tiles = {n: self.tiles[n] for n in self.app.clips if n in self.tiles}
        self.dirty = True

    # --- placing tiles ---
    # Tiles are placed at exact cells (x, y, width) worked out here, not gridded. Moving a widget
    # between Tk's geometry managers hides and re-shows it, and every tile flashed when a slide
    # started and ended; with place() throughout, a slide only ever moves them.
    _slide = None      # the running slide (a token), if any
    _moves = ()        # its (widget, x0, y0, x1, y1)
    _width = 0         # scroll area width the cells were worked out for
    slide_from = None  # (cells before, dropped name): set by App.set_order, used by the next layout
    targets = {}       # where each tile sits (or is sliding to): key -> (x, y) in the scroll area
    geom = {}          # where each shown widget is placed: key -> (x, y, width)

    def cells(self):
        """{name: (x, y)} of the tiles' cells now, in the page's scroll area."""
        return {n: g[:2] for n, g in self.geom.items() if not isinstance(n, tuple) and n in self.tiles}

    def _metrics(self, cols):
        s = self.frame._get_widget_scaling()
        width = self.frame.winfo_width()
        if width <= 1:   # not on screen yet: the board's width less the scrollbar
            width = max(1, self.app.holder.winfo_width() - round(24 * s))
        gap = round(6 * s)
        return width, gap, width / cols, round(TILE_HEIGHT * s) + 2 * gap

    overlay = None     # the canvas a slide is drawn on (see _slide_on_canvas)
    after_slide = ()   # things to do once it lifts

    def snapshot(self, settle=False):
        """Copies of the tiles exactly as they look on screen now, for drawing a slide on one canvas:
        {key: (image, dx, dy)}, dx, dy being where a copy starts inside its tile (a tile cut off by
        the edge of the board is copied in part). None if the board isn't on screen.
        settle: something on it just changed; draw that and wait for it to reach the screen first."""
        if settle:
            self.frame.update_idletasks()
            try:
                ctypes.windll.dwmapi.DwmFlush()   # until the next frame Windows puts on screen
            except Exception:
                pass
        c = self.frame._parent_canvas
        vx, vy, vw, vh = c.winfo_rootx(), c.winfo_rooty(), c.winfo_width(), c.winfo_height()
        if vw <= 1 or vh <= 1:
            return None
        try:
            im = screen_grab(vx, vy, vw, vh)
        except Exception:
            log.exception("board snapshot")
            return None
        out = {}
        widgets = {**{("job", k): w for k, w in self.pending.items()}, **self.tiles}
        for key, w in widgets.items():
            if key not in self.geom:
                continue
            x, y = w.winfo_rootx() - vx, w.winfo_rooty() - vy
            x0, y0 = max(0, x), max(0, y)
            x1, y1 = min(vw, x + w.winfo_width()), min(vh, y + w.winfo_height())
            if x1 > x0 and y1 > y0:
                out[key] = (ImageTk.PhotoImage(im.crop((x0, y0, x1, y1))), x0 - x, y0 - y)
        return out

    def stop_slide(self):
        """End a slide where it's headed."""
        if self._slide is None:
            return
        self._slide = None
        for w, _, _, x1, y1 in self._moves:
            if w.winfo_exists():
                tk.Place.place_configure(w, x=x1, y=y1)
        self._moves = ()
        self._lift_overlay()

    def _lift_overlay(self):
        ov, self.overlay = self.overlay, None
        if ov is not None and ov.winfo_exists():
            ov.destroy()
        later, self.after_slide = self.after_slide, ()
        for fn in later:
            try:
                fn()
            except Exception:
                log.exception("after slide")

    def _slide_on_canvas(self, snaps, before, geom, dropped, ms=220):
        """Draw the slide on ONE canvas over the board, from copies of the tiles, and move the real
        tiles to their new cells underneath straight away. Each tile is its own small window, and
        with many moving at once Windows put frames on screen while some were repainted and others
        weren't (torn cards); a single canvas is drawn off screen and shown in one go, so every
        frame is whole. Returns False (nothing done) if a tile on screen has no copy."""
        c = self.frame._parent_canvas
        top, vw, vh = round(c.canvasy(0)), c.winfo_width(), c.winfo_height()
        th = round(TILE_HEIGHT * self.frame._get_widget_scaling())
        on_screen = lambda y: y + th > top and y < top + vh   # noqa: E731
        widgets = {**{("job", k): w for k, w in self.pending.items()}, **self.tiles}
        for key in geom:
            if key != dropped and key not in snaps and (on_screen(geom[key][1]) or on_screen(before.get(key, geom[key])[1])):
                return False
        ov = tk.Canvas(self.frame, width=vw, height=vh, bg=BG, highlightthickness=0, bd=0)
        items, keep = [], []
        for key, (ph, dx, dy) in snaps.items():
            if key == dropped or key not in geom:
                continue
            x0, y0 = before.get(key, geom[key][:2])
            x1, y1 = geom[key][:2]
            keep.append(ph)
            items.append((ov.create_image(x0 + dx, y0 + dy - top, image=ph, anchor="nw"),
                          x0 + dx, y0 + dy - top, x1 + dx, y1 + dy - top))
        for it, x0, y0, x1, y1 in items:   # tiles changing rows glide over the others, not under them
            if y0 != y1:
                ov.tag_raise(it)
        if dropped in geom:   # it's already in its new spot (the copy was taken before it went faint)
            x, y, w = geom[dropped]
            if dropped in snaps:
                ph, dx, dy = snaps[dropped]
                ov.create_image(x + dx, y + dy - top, image=ph, anchor="nw")
            else:
                ph = card_photo(w, th, CARD, BORDER, self.frame._get_widget_scaling())
                ov.create_image(x, y - top, image=ph, anchor="nw")
            keep.append(ph)
        ov.keep = keep
        tk.Place.place_configure(ov, x=0, y=top, width=vw, height=vh)
        tk.Misc.lift(ov)   # (Canvas.lift raises canvas items; this raises the widget)
        ov.update_idletasks()   # on screen, showing exactly what was there, before anything moves
        self.overlay = ov
        for key, w in widgets.items():   # the real tiles go straight to their new cells, underneath
            if key in geom:
                x, y, cw = geom[key]
                tk.Place.place_configure(w, x=x, y=y, width=cw)
        token = self._slide = object()
        self._moves = ()

        def step(e):
            if self._slide is not token:
                return False
            for it, x0, y0, x1, y1 in items:
                ov.coords(it, round(x0 + (x1 - x0) * e), round(y0 + (y1 - y0) * e))

        def done():
            if self._slide is token:
                self._slide = None
                self._lift_overlay()   # the real tiles are already there
        animate(ov, ms, step, done=done)
        return True

    def _reflow(self):
        """The scroll area changed width (window resized): re-work the cells."""
        if self.built and self.columns and self.frame.winfo_width() != self._width:
            self.dirty = True
            self.layout(self.columns, self.query or "")

    def layout(self, cols, query=""):
        self.stop_slide()
        slide, self.slide_from = self.slide_from, None
        width, gap, colw, rowh = self._metrics(cols)
        if not self.dirty and cols == self.columns and query == self.query and width == self._width:
            return
        self.dirty = False
        self.columns, self.query, self._width = cols, query, width
        s = self.frame._get_widget_scaling()
        # imports still downloading go first, where you'll see them; Quick import adds to No group
        jobs = list(self.app.jobs.values()) if self.key in (ALL, NO_GROUP) and not query else []
        for j in jobs:
            if j.id not in self.pending:
                self.pending[j.id] = PendingTile(self.frame, self.app, j)
        items = [(("job", j.id), self.pending[j.id]) for j in jobs] + \
                [(n, t) for n, t in self.tiles.items() if query in Path(n).stem.lower()]
        tk.Place.place_forget(self.msg)
        if self.empty is not None:
            tk.Place.place_forget(self.empty)
        height = 0
        if not items:
            note = self.empty if self.empty is not None and not self.tiles else self.msg
            if note is self.msg:
                self.msg.configure(text="No sounds match your search." if query else
                                   "This group is empty. Right-click a sound > Move to group, "
                                   "or import straight into it.")
            top = 0 if note is self.empty else round(60 * s)
            tk.Place.place_configure(note, relx=0.5, y=top, anchor="n")
            height = top + note.winfo_reqheight() + round(40 * s)
        # cells: equal columns across the scroll area, a gap around every tile
        want, geom = {}, {}
        for i, (key, _) in enumerate(items):
            r, c = divmod(i, cols)
            x0, x1 = round(c * colw), round((c + 1) * colw)
            want[key] = (r, c)
            geom[key] = (x0 + gap, r * rowh + gap, x1 - x0 - 2 * gap)
        if items:
            height = -(-len(items) // cols) * rowh + gap
        before, dropped, snaps = slide if slide else ({}, None, None)
        if snaps and any(k in before and before[k] != g[:2] for k, g in geom.items() if k != dropped):
            if self._slide_on_canvas(snaps, before, geom, dropped):
                self.placed, self.geom = want, geom
                self.targets = {k: g[:2] for k, g in geom.items()}
                tk.Frame.configure(self.frame, height=max(1, height))
                return
        moves = []
        widgets = {**{("job", k): w for k, w in self.pending.items()}, **self.tiles}
        for key, w in widgets.items():
            g = geom.get(key)
            if g is None:
                if key in self.geom:
                    tk.Place.place_forget(w)
                continue
            if self.geom.get(key) == g:
                continue
            x, y, cw = g
            if key in before and key != dropped and before[key] != (x, y):
                # a re-sort: start where it was and glide to its new cell (the one you dropped
                # goes straight to its new cell)
                tk.Place.place_configure(w, x=before[key][0], y=before[key][1], width=cw)
                moves.append((w, before[key][0], before[key][1], x, y))
            else:
                tk.Place.place_configure(w, x=x, y=y, width=cw)
        self.placed, self.geom = want, geom
        self.targets = {k: g[:2] for k, g in geom.items()}
        # nothing in the scroll area is gridded, so its height is ours to set
        tk.Frame.configure(self.frame, height=max(1, height))
        if moves:
            self._slide_tiles(moves)

    def _slide_tiles(self, moves, ms=220):
        token = self._slide = object()
        self._moves = moves

        frame = self.frame

        def step(e):
            if self._slide is not token:
                return False
            for w, x0, y0, x1, y1 in moves:
                tk.Place.place_configure(w, x=round(x0 + (x1 - x0) * e), y=round(y0 + (y1 - y0) * e))

        def done():
            if self._slide is token:
                self._slide, self._moves = None, ()
        animate(frame, ms, step, done=done)

    def drop_job(self, job):
        w = self.pending.pop(job.id, None)
        if w is not None:
            self.placed.pop(("job", job.id), None)
            self.geom.pop(("job", job.id), None)
            w.destroy()
            self.dirty = True


    hidden = False

    def show(self):
        if self.hidden:
            self.frame.grid()   # grid remembers its options from grid_remove()
            self.hidden = False
        self.frame._parent_frame.lift()

    def hide(self):
        """Off screen but still drawn: Windows only repaints what's mapped, so restoring a
        minimised window (or anything covering it) repaints just the visible page."""
        if not self.hidden:
            self.frame.grid_remove()
            self.hidden = True

    def destroy(self):
        self.frame._parent_frame.destroy()


class App:
    def __init__(self, root):
        self.root = root
        self.cfg = load_config()
        apply_theme(self.cfg.get("theme"))
        self.restart = False       # True when the window closes only to come back in a new theme
        _dots.clear()              # cached images belong to the previous window
        _thumbs.clear()
        _photos.clear()
        _tkfonts.clear()
        _card_photos.clear()
        self.engine = engine.Engine()
        self.hotkeys = HotkeyManager()
        self.clips = {}            # filename -> float32 array
        self.pages = {}            # view key (ALL, group, NO_GROUP) -> Page
        self.page = None           # the Page on top
        self.groups = []           # folder names inside sounds/
        self._stats = {}           # filename -> (size, mtime) of the loaded audio
        self._moved = {}           # new filename -> old, until the next reload
        self._tabs = {}            # group key -> tab button
        self._freeze_depth = 0
        self.active_menu = None    # open PopupMenu
        self.open_menus = []       # that menu plus its submenus
        self._ready = False        # True once the window has faded in at startup
        self._on_ready = []        # things waiting for that (see when_ready)
        self._watching = False     # focus watcher running (see watch_focus)
        self.jobs = {}             # Quick imports in progress: url -> ImportJob
        self.dragger = Dragger(self)
        self._polls = 0
        self.columns = 0
        SOUNDS.mkdir(exist_ok=True)

        root.attributes("-alpha", 0.0)   # invisible while it's built, then faded in (end of __init__)
        root.title("JondisBoard")
        root.geometry(self.cfg.get("geometry") or "940x680")
        root.minsize(640, 480)
        root.configure(fg_color=BG)
        root.protocol("WM_DELETE_WINDOW", self.quit)
        self._menu_serial = None
        # customtkinter already has an all-widgets <Button-1> binding, which outranks a plain
        # <ButtonPress> one, so hook each button explicitly
        for seq in ("<Button-1>", "<Button-2>", "<Button-3>"):
            root.bind_all(seq, self._click_anywhere, add="+")
        root.bind("<Configure>", lambda e=None: e is not None and e.widget is root and self._root_configured(), add="+")
        root.bind("<Unmap>", self._on_minimise, add="+")
        root.bind("<Map>", self._on_restore, add="+")
        self._set_icon(root)
        title_bar_colors(root)
        self._build_ui()
        self.reload_clips()
        self.start_audio()
        self._poll_playing()
        root.update_idletasks()
        animate(root, 220, lambda e: root.attributes("-alpha", e), done=self._became_ready, delay_ms=30,
                warmup=True)
        root.after(800, self._prerender_settings)

    def _root_configured(self):
        """The window moved or resized: menus would be left floating, and Settings may need
        to get narrower (or can grow back) to fit."""
        if self.open_menus:
            self.close_menus()
        if self.settings_open:
            self._place_settings()
        toast = getattr(self, "_toast", None)
        if toast is not None and toast.winfo_exists() and toast.motion is not None:
            self._toast = None
            fade_away(toast, done=toast.destroy)

    def _unlayer(self):
        unlayer(self.root)

    def _set_icon(self, win):
        icon = theme_icon()   # the logo in this theme's colours
        if icon.exists():
            # set it straight away (no flash of the default icon); customtkinter only swaps in
            # its own icon if iconbitmap() hasn't been called, but re-set it later to be sure
            win.iconbitmap(str(icon))
            crisp_icons(win, icon)
            later(win, 250, lambda: (win.iconbitmap(str(icon)), crisp_icons(win, icon)))

    # ---------- UI ----------
    def _build_ui(self):
        root = self.root
        root.columnconfigure(0, weight=1)
        root.rowconfigure(4, weight=1)

        # header
        head = ctk.CTkFrame(root, fg_color="transparent")
        head.grid(row=0, column=0, sticky="ew", padx=24, pady=(20, 6))
        head.columnconfigure(2, weight=1)
        self.head = head
        mark = logo.draw(36 * 3, logo.tile_color(globals()), ACCENT)   # 3x: sharp at any scaling
        ctk.CTkLabel(head, image=ctk.CTkImage(mark, size=(36, 36)), text="").grid(
            row=0, column=0, rowspan=2, padx=(0, 12))
        ctk.CTkLabel(head, text="JondisBoard", font=font(22, "bold"), text_color=TEXT,
                     height=26, anchor="w").grid(row=0, column=1, sticky="w")
        self.count_label = ctk.CTkLabel(head, text="", font=font(12), text_color=MUTED, height=16, anchor="w")
        self.count_label.grid(row=1, column=1, sticky="w")
        self.status_pill = ctk.CTkLabel(head, text="", font=font(12, "bold"), corner_radius=15,
                                        fg_color=CARD, height=30, compound="left")
        self.status_pill.grid(row=0, column=3, rowspan=2, padx=(0, 10))
        self.theme_btn = button(head, "Theme", None, "secondary", image=dot_image(ACCENT, 12), compound="left")
        self.theme_btn.configure(command=self.theme_menu)
        self.theme_btn.grid(row=0, column=4, rowspan=2, padx=(0, 8))
        self.settings_btn = button(head, "Settings", self.toggle_settings, "secondary", icon_name="sliders")
        self.settings_btn.grid(row=0, column=5, rowspan=2)
        fade_to(self.settings_btn, "on", (CARD, BORDER),
                lambda f, b: self.settings_btn.configure(fg_color=f, border_color=b))

        # Settings: a popover that glides down from its button (see toggle_settings). Its own
        # window, so it can fade and move without the board underneath redrawing.
        self.settings_win = popup_window(root)
        self.settings = ctk.CTkFrame(self.settings_win, fg_color="transparent")
        self.settings.pack(fill="both", expand=True, padx=6, pady=6)
        # popups never take focus, so the main window handles Esc for them too
        root.bind("<Escape>", self._escape, add="+")
        root.bind("<Control-f>", lambda e=None: self.search.focus_set(), add="+")
        root.bind("<Control-v>", self._paste, add="+")
        root.bind("<Control-V>", self._paste, add="+")
        self.settings.columnconfigure(1, weight=1)
        ctk.CTkLabel(self.settings, text="Audio", font=font(14, "bold"), text_color=TEXT, anchor="w").grid(
            row=0, column=0, columnspan=3, sticky="w", padx=14, pady=(10, 2))
        self.mic_var = tk.StringVar(value=self.cfg["mic"] or "")
        self.cable_var = tk.StringVar(value=self.cfg["cable"] or "")
        self.monitor_var = tk.StringVar(value=self.cfg["monitor"] or "")
        rows = [("Your microphone", self.mic_var, "input"),
                ("Send to (virtual cable)", self.cable_var, "output"),
                ("Hear clips on", self.monitor_var, "output")]
        self.menus = []
        for r, (label, var, kind) in enumerate(rows, start=1):
            pad = (8, 12 if r == 3 else 0)
            ctk.CTkLabel(self.settings, text=label, font=font(13), text_color=MUTED, anchor="w").grid(
                row=r, column=0, sticky="w", padx=(14, 14), pady=pad)
            menu = Select(self.settings, self, var, command=lambda _: self.start_audio())
            menu.kind = kind
            menu.grid(row=r, column=1, sticky="ew", pady=pad)
            self.menus.append(menu)
        button(self.settings, "Restart audio", self.start_audio, "secondary", icon_name="refresh").grid(
            row=1, column=2, rowspan=3, padx=14)
        # first-run help for people who got the app shared and have no virtual cable yet
        self.cable_help = ctk.CTkFrame(self.settings, fg_color=mix(CARD, DANGER, 0.12), corner_radius=10,
                                       border_width=1, border_color=mix(BORDER, DANGER, 0.45))
        self.cable_help.columnconfigure(0, weight=1)
        ctk.CTkLabel(self.cable_help, text="VB-CABLE isn't installed. It's the free virtual mic that carries "
                     "the sounds into games. Install it, restart your PC, then open the soundboard again.",
                     font=font(12), text_color=TEXT, justify="left", wraplength=430, anchor="w").grid(
            row=0, column=0, sticky="w", padx=14, pady=10)
        button(self.cable_help, "Get VB-CABLE", lambda: os.startfile("https://vb-audio.com/Cable/"),
               "primary", width=120).grid(row=0, column=1, padx=12)
        # (row 4 is the VB-CABLE help, when it shows)
        tk.Frame(self.settings, height=1, bg=mix(ELEVATED, BORDER, 0.8), bd=0, highlightthickness=0).grid(
            row=5, column=0, columnspan=3, sticky="ew", padx=14, pady=(2, 8))
        ctk.CTkLabel(self.settings, text="Playback", font=font(14, "bold"), text_color=TEXT, anchor="w").grid(
            row=6, column=0, columnspan=3, sticky="w", padx=14, pady=(0, 2))
        self.option_vars = {}
        for r, (key, title, about) in enumerate((
                ("solo", "One sound at a time", "Starting a sound stops the others."),
                ("toggle_stop", "Press a playing sound to stop it",
                 "A second click or hotkey stops it, instead of starting it over.")), start=7):
            self._option_row(r, key, title, about, last=r == 8)
        self.settings_open = False

        # toolbar
        bar = ctk.CTkFrame(root, fg_color="transparent")
        bar.grid(row=2, column=0, sticky="ew", padx=24, pady=(10, 10))
        bar.columnconfigure(4, weight=1)
        # Quick import is the fast path (link on the clipboard -> sound, no questions), so it leads;
        # Import link... is for naming, trimming or picking a group first
        self.quick_btn = button(bar, "Quick import", self.quick_import, "primary", icon_name="bolt")
        self.quick_btn.grid(row=0, column=0, padx=(0, 8))
        button(bar, "Import link…", self.import_dialog, "secondary", icon_name="link").grid(
            row=0, column=1, padx=(0, 8))
        button(bar, "Add files", self.add_files, "secondary", icon_name="note").grid(row=0, column=2, padx=(0, 8))
        self.more_btn = button(bar, "", None, "ghost", icon_name="more", width=36)
        self.more_btn.configure(command=self.more_menu)
        self.more_btn.grid(row=0, column=3)
        # a search field with an icon: the entry sits borderless inside a frame that draws the
        # outline, which turns accent while you type in it
        box = ctk.CTkFrame(bar, fg_color=CARD, corner_radius=10, border_width=1, border_color=BORDER,
                           width=240, height=34)
        box.grid(row=0, column=5, sticky="e")
        box.pack_propagate(False)
        ctk.CTkLabel(box, text="", image=icon("search", MUTED, 14), width=16).pack(side="left", padx=(11, 0))
        # no textvariable: CTkEntry hides its placeholder when one is set
        self.search = ctk.CTkEntry(box, placeholder_text="Search sounds   Ctrl+F", height=30, border_width=0,
                                   corner_radius=8, fg_color=CARD, text_color=TEXT,
                                   placeholder_text_color=MUTED, font=font(13))
        self.search.pack(side="left", fill="both", expand=True, padx=(4, 6), pady=2)
        self.search.bind("<KeyRelease>", lambda e=None: self._search_changed())
        self.search.bind("<Escape>", self._clear_search, add="+")
        fade_to(box, "ring", (BORDER,), lambda c: box.configure(border_color=c))
        self.search.bind("<FocusIn>", lambda e=None: fade_to(box, "ring", (ACCENT_EDGE,),
                                                        lambda c: box.configure(border_color=c)), add="+")
        self.search.bind("<FocusOut>", lambda e=None: fade_to(box, "ring", (BORDER,),
                                                         lambda c: box.configure(border_color=c),
                                                         MOTION_UNHOVER), add="+")
        for w in (box, *box.winfo_children()):
            w.bind("<Button-1>", lambda e=None: self.search.focus_set(), add="+")

        # group tabs, filled in by _build_group_bar
        self.group_bar = ctk.CTkFrame(root, fg_color="transparent")
        self.group_bar.grid(row=3, column=0, sticky="ew", padx=24, pady=(0, 6))

        # sound grid
        holder = ctk.CTkFrame(root, fg_color="transparent")
        holder.grid(row=4, column=0, sticky="nsew", padx=(16, 10))
        holder.rowconfigure(0, weight=1)
        holder.columnconfigure(0, weight=1)
        holder.bind("<Configure>", lambda e=None: self._on_resize())
        self.holder = holder   # pages (one per group) are stacked in here

        # bottom bar, with a hairline between it and the board
        foot = ctk.CTkFrame(root, fg_color=SURFACE, corner_radius=0, border_width=0)
        foot.grid(row=5, column=0, sticky="ew")
        self.foot = foot
        tk.Frame(foot, height=1, bg=BORDER, bd=0, highlightthickness=0).pack(fill="x")
        inner = ctk.CTkFrame(foot, fg_color="transparent")
        inner.pack(fill="x", padx=24, pady=14)
        self.vol_labels = {}
        col = 0
        for key, label, top in [("mic_volume", "Mic", 1.5), ("clip_volume", "Clips", 1.5),
                                ("monitor_volume", "In your ears", 1.0)]:
            box = ctk.CTkFrame(inner, fg_color="transparent")
            box.grid(row=0, column=col, sticky="ew", padx=(0, 24))
            inner.columnconfigure(col, weight=1)
            box.columnconfigure(0, weight=1)
            ctk.CTkLabel(box, text=label, font=font(12), text_color=MUTED, height=16, anchor="w").grid(
                row=0, column=0, sticky="w")
            self.vol_labels[key] = ctk.CTkLabel(box, text="", font=font(12, "bold"), text_color=TEXT,
                                                height=16, anchor="e")
            self.vol_labels[key].grid(row=0, column=1, sticky="e")
            s = ctk.CTkSlider(box, from_=0, to=top, height=18, button_length=0, button_corner_radius=9,
                              button_color=TEXT, button_hover_color="#ffffff", progress_color=ACCENT,
                              fg_color=mix(SURFACE, BORDER, 0.9),
                              command=lambda v, k=key: self._set_volume(k, float(v)))
            s.set(self.cfg[key])
            s.grid(row=1, column=0, columnspan=2, sticky="ew", pady=(6, 0))
            self._set_volume(key, self.cfg[key])
            col += 1
        self.pass_var = tk.BooleanVar(value=self.cfg["mic_passthrough"])
        labelled_toggle(inner, "Send my voice", self.pass_var, self._set_passthrough, bg=SURFACE).grid(
            row=0, column=col, padx=(0, 20))
        self._set_passthrough()
        stop = ctk.CTkFrame(inner, fg_color="transparent")
        stop.grid(row=0, column=col + 1)
        self.stop_btn = button(stop, "Stop all", self.engine.stop_all, "danger", width=124, height=36,
                               image=icon("stop", "#ffffff", 12), compound="left")
        self.stop_btn.pack()
        self.stop_key_btn = ctk.CTkButton(stop, text="", command=self.set_stop_hotkey, fg_color="transparent",
                                          hover_color=CARD, text_color=MUTED, font=font(11), height=18,
                                          width=124, corner_radius=6)
        self.stop_key_btn.pack(pady=(4, 0))

    def _option_row(self, row, key, title, about, last=False):
        """One setting in the Playback section: what it does on the left, its switch on the right.
        Clicking the words flips it too."""
        var = self.option_vars[key] = tk.BooleanVar(value=bool(self.cfg.get(key)))

        def changed():
            self.cfg[key] = var.get()
            self.save_config()
        text = ctk.CTkFrame(self.settings, fg_color="transparent")
        text.grid(row=row, column=0, columnspan=2, sticky="w", padx=14, pady=(6, 14 if last else 4))
        words = [ctk.CTkLabel(text, text=title, font=font(13), text_color=TEXT, anchor="w", height=18),
                 ctk.CTkLabel(text, text=about, font=font(12), text_color=MUTED, anchor="w", height=16)]
        for w in words:
            w.pack(anchor="w")
            w.bind("<Button-1>", lambda e=None: (var.set(not var.get()), changed()))
        Toggle(self.settings, var, changed, bg=ELEVATED).grid(row=row, column=2, sticky="e", padx=(0, 14),
                                                              pady=(6, 14 if last else 4))

    def _escape(self, e=None):
        """Esc closes the topmost thing: a drag, a menu, else Settings."""
        if self.dragger.active:
            self.dragger.cancel()
        elif self.open_menus:
            self.close_menus()
        else:
            self.toggle_settings(False)

    def _clear_search(self, e=None):
        if self.search.get():
            self.search.delete(0, "end")
            self._search_changed()
            return "break"   # just clear it; a second Esc closes popups as usual

    def toggle_settings(self, show=None):
        """Settings glides down from its button in its own window, over the board. (Opening it
        inside the window made the board redraw underneath, which is what made it jittery.)"""
        open_ = (not self.settings_open) if show is None else show
        if open_ == self.settings_open and show is not None:
            return
        if open_ and not self._ready:   # at startup: once the window is on screen
            self.when_ready(lambda: self.toggle_settings(True))
            return
        self.settings_open = open_
        win = self.settings_win
        if open_:
            self.close_menus()   # one thing open at a time (e.g. the Theme menu)
            x, y = self._settings_spot()
            owned_by(win, self.root)
            reveal(win, x, y, 0, -SLIDE)
            self.watch_focus()
        else:
            self.close_menus()   # its device lists go with it
            fade_away(win, 0, -SLIDE // 2, done=lambda: win.withdraw())
        # open: tinted with the accent and its own (brighter) hover; closed: a normal button
        b = self.settings_btn
        set_hover_color(b, ACCENT_SOFT_HOVER if open_ else CARD_HOVER)
        fade_to(b, "on", (ACCENT_SOFT, ACCENT_EDGE) if open_ else (CARD, BORDER),
                lambda f, c: b.configure(fg_color=f, border_color=c), MOTION_OPEN if open_ else MOTION_UNHOVER)

    SETTINGS_WIDTH = 660

    def _settings_spot(self):
        """Size Settings for the window (narrower only if the window is) and return where it
        goes: right-aligned under its button."""
        win, b = self.settings_win, self.settings_btn
        scale = self.root._get_window_scaling()
        width = min(round(self.SETTINGS_WIDTH * scale), self.root.winfo_width() - round(48 * scale))
        win.update_idletasks()
        # customtkinter scales a toplevel's WxH itself, so hand it unscaled numbers
        win.geometry(f"{round(width / scale)}x{round(self.settings.winfo_reqheight() / scale) + 12}")
        x = b.winfo_rootx() + b.winfo_width() - width
        y = b.winfo_rooty() + b.winfo_height() + round(8 * scale)
        return x, y

    def _place_settings(self):
        """Keep the open Settings under its button (the window moved, or its contents changed)."""
        if self.settings_open:
            x, y = self._settings_spot()
            if self.settings_win.motion is None or float(self.settings_win.attributes("-alpha")) >= 1:
                self.settings_win.geometry(f"+{x}+{y}")

    def _prerender_settings(self):
        """Draw the Settings popover once out of sight, so its first opening is as quick as the rest."""
        if not self.settings_open:
            win = self.settings_win
            self._settings_spot()
            win.geometry("+-4000+0")
            win.deiconify()
            self.root.update_idletasks()
            if not self.settings_open:
                win.withdraw()

    def set_status(self, text, level="good"):
        color = {"good": GOOD, "warn": WARN, "bad": DANGER}[level]
        self.root.after(0, lambda: self.status_pill.configure(
            text=f"  {text}   ", text_color=color, fg_color=mix(BG, color, 0.12), image=dot_image(color, 8)))

    def toast(self, text, level="info"):
        """Short message that rises in over the bottom bar, then fades away. Its own window, so it
        can fade; it's owned by the main window, so it never floats over a game."""
        if not self._ready:
            self.when_ready(lambda: self.toast(text, level))
            return
        old = getattr(self, "_toast", None)
        if old is not None and old.winfo_exists():
            fade_away(old, 0, SLIDE // 2, done=old.destroy)
        win = self._toast = popup_window(self.root)
        color = {"info": ACCENT, "warn": WARN, "bad": DANGER}.get(level, ACCENT)
        row = ctk.CTkFrame(win, fg_color="transparent")
        row.pack(padx=16, pady=10)
        ctk.CTkLabel(row, text="", image=dot_image(color, 8), width=8).pack(side="left", padx=(0, 10))
        ctk.CTkLabel(row, text=text, font=font(13), text_color=TEXT, justify="left", wraplength=560).pack(side="left")
        for w in (win, row, *row.winfo_children()):   # click to dismiss
            w.bind("<Button-1>", lambda e=None: win.winfo_exists() and fade_away(win, 0, SLIDE // 2, done=win.destroy))
        win.update_idletasks()
        root, s = self.root, self.root._get_window_scaling()
        x = root.winfo_rootx() + (root.winfo_width() - win.winfo_reqwidth()) // 2
        y = root.winfo_rooty() + self.foot.winfo_y() - win.winfo_reqheight() - round(14 * s)
        owned_by(win, root)
        reveal(win, x, y, 0, SLIDE + 3)
        later(win, 4200, lambda: fade_away(win, 0, SLIDE // 2, MOTION_UNHOVER,
                                                                 done=win.destroy))

    def when_ready(self, fn):
        """Run fn once the window has faded in at startup (popups shouldn't beat it to the screen)."""
        if self._ready:
            fn()
        else:
            self._on_ready.append(fn)

    def _became_ready(self):
        self._unlayer()
        self._ready = True
        pending, self._on_ready = self._on_ready, []
        for fn in pending:
            fn()

    def watch_focus(self):
        """While a menu or popover is open, close it once another program (e.g. the game) comes
        to the front. One watcher for all of them."""
        if not self._watching:
            self._watching = True
            self._had_focus = False
            self._watch_id = self.root.after(150, self._watch_focus)

    def _watch_focus(self):
        if not (self.open_menus or self.settings_open):
            self._watching = False
            return
        if app_has_focus():
            self._had_focus = True
        elif self._had_focus:
            self._watching = False
            self.close_menus()
            self.toggle_settings(False)
            return
        self._watch_id = self.root.after(150, self._watch_focus)

    # ---------- audio ----------
    def refresh_devices(self):
        for m in self.menus:
            m.set_values(engine.list_devices(m.kind))
        if engine.is_mapper(self.mic_var.get()):
            self.mic_var.set("")
        if not self.cable_var.get():
            self.cable_var.set(engine.find_cable_output_name() or "")
        if not self.mic_var.get():
            self.mic_var.set(engine.default_device_name("input") or "")
        if not self.monitor_var.get():
            self.monitor_var.set(engine.default_device_name("output") or "")

    def start_audio(self):
        self.refresh_devices()
        self.cfg.update(mic=self.mic_var.get() or None, cable=self.cable_var.get() or None,
                        monitor=self.monitor_var.get() or None)
        self.save_config()
        try:
            warnings = self.engine.start(self.cfg["mic"], self.cfg["cable"], self.cfg["monitor"])
        except Exception as ex:
            log.exception("audio start failed")
            self.cable_help.grid_forget()
            self.set_status("Audio error", "bad")
            self.toast(f"Audio error: {ex}", "bad")
            self.toggle_settings(True)
            self._place_settings()
            return
        has_cable = bool(engine.find_cable_output_name())
        if has_cable:
            self.cable_help.grid_forget()
        else:
            self.cable_help.grid(row=4, column=0, columnspan=3, sticky="ew", padx=8, pady=(0, 8))
        if not has_cable:
            self.set_status("VB-CABLE not installed", "bad")
            self.toggle_settings(True)
        elif warnings:
            self.set_status("Check audio setup", "warn")
            self.toast(warnings[0], "warn")
            self.toggle_settings(True)
        else:
            self.set_status("Live", "good")
        self._place_settings()   # its contents may have changed (the VB-CABLE help)

    def _set_volume(self, key, value):
        self.cfg[key] = round(value, 3)
        setattr(self.engine, key, value)
        if key in getattr(self, "vol_labels", {}):
            self.vol_labels[key].configure(text=f"{round(value * 100)}%")

    def _set_passthrough(self):
        self.cfg["mic_passthrough"] = self.engine.mic_passthrough = self.pass_var.get()

    # ---------- clips ----------
    def reload_clips(self):
        # groups are folders inside sounds/ (one level); "_" folders like _deleted are ours
        is_audio = lambda p: p.is_file() and p.suffix.lower() in engine.AUDIO_EXTS  # noqa: E731
        files, self.groups = [], []
        for p in SOUNDS.iterdir():
            if p.is_dir() and not p.name.startswith("_"):
                self.groups.append(p.name)
                files += [f for f in p.iterdir() if is_audio(f)]
            elif is_audio(p):
                files.append(p)
        self.groups.sort(key=str.lower)
        files.sort(key=lambda p: p.stem.lower())
        # reuse already-decoded audio for unchanged (or just moved/renamed) files
        old_clips, old_stats, moved = self.clips, self._stats, self._moved
        self.clips, self._stats, self._moved = {}, {}, {}
        errors = []
        for p in files:
            name = p.relative_to(SOUNDS).as_posix()
            st = p.stat()
            stat = (st.st_size, st.st_mtime_ns)
            prev = moved.get(name, name)
            try:
                if prev in old_clips and old_stats.get(prev) == stat:
                    self.clips[name] = old_clips[prev]
                else:
                    self.clips[name] = engine.load_audio(p)
                self._stats[name] = stat
            except Exception as ex:
                log.exception("failed to load %s", p)
                errors.append(f"{p.name}: {ex}")
        # the board's order: the saved one (renamed / moved clips keep their place), new clips last
        renamed = {old: new for new, old in moved.items()}
        order = [renamed.get(n, n) for n in self.cfg.get("order") or []]
        order = list(dict.fromkeys(n for n in order if n in self.clips))
        order += [n for n in self.clips if n not in set(order)]
        self.cfg["order"] = order
        self.clips = {n: self.clips[n] for n in order}
        # forget hotkeys of clips that no longer exist
        self.cfg["hotkeys"] = {k: v for k, v in self.cfg["hotkeys"].items() if k in self.clips}
        if self.cfg["group"] not in (ALL, NO_GROUP, *self.groups):
            self.cfg["group"] = ALL
        self._assign_group_colors()
        self.save_config()
        with self._frozen():
            self._build_group_bar()
            self._sync_pages(moved)
        self._apply_hotkeys()
        n = len(self.clips)
        self.count_label.configure(text=f"{n} sound{'s' if n != 1 else ''}")
        if errors:
            self.toast("Couldn't load: " + "; ".join(errors), "warn")

    # ---------- groups ----------
    def _build_group_bar(self):
        counts = {}
        for n in self.clips:
            counts[group_of(n)] = counts.get(group_of(n), 0) + 1
        tabs = [(ALL, "All", len(self.clips))] + [(g, g, counts.get(g, 0)) for g in self.groups]
        if self.groups and counts.get(NO_GROUP):
            tabs.append((NO_GROUP, "No group", counts[NO_GROUP]))
        same = list(self._tabs) == [k for k, _, _ in tabs]
        if not same:   # tabs added/removed: rebuild; otherwise update them in place
            for w in self.group_bar.winfo_children():
                w.destroy()
            self._tabs = {}
            for key, label, _ in tabs:
                t = self._tabs[key] = GroupTab(self.group_bar, self, key, label)
                t.pack(side="left", padx=(0, 8), pady=2)
            button(self.group_bar, "New group", self.new_group, "ghost", height=32,
                   image=plus_image(MUTED, 10), compound="left").pack(side="left")
        for key, label, n in tabs:
            # the tab you pick fills in while the old one fades back
            self._tabs[key].set(label, n, key == self.cfg["group"], fade=same)

    # ---------- quick import ----------
    def _paste(self, e=None):
        """Ctrl+V on the board is Quick import (typing in a text box pastes as usual)."""
        if isinstance(self.root.focus_get(), tk.Entry):
            return
        self.quick_import()

    def quick_import(self):
        """Turn the link on the clipboard into a sound, straight away: no group, the video's title
        as its name, loudness evened out. It shows up as a 'working' tile until it's done."""
        try:
            link = find_link(self.root.clipboard_get())
        except tk.TclError:   # nothing (or no text) on the clipboard
            link = None
        if not link:
            self.toast("Copy an Instagram or YouTube link first, then press Quick import.", "warn")
            return
        if link in self.jobs:
            self.toast("That link is already importing.", "warn")
            return
        job = self.jobs[link] = ImportJob(link)
        self._jobs_changed()
        if self._view_key() not in (ALL, NO_GROUP):   # you'd otherwise not see it working
            self.toast("Importing… it'll appear under All (and No group).")
        self._run_job(job)

    def _run_job(self, job):
        steps = (("Downloading", "Downloading"), ("Cutting", "Cutting audio"), ("Saved", "Finishing"))

        def progress(text):
            for prefix, label in steps:
                if text.startswith(prefix):
                    self.root.after(0, lambda label=label: self._job_status(job, label))

        def work():
            try:
                out = importer.import_clip(job.url, normalize=True, log=progress, folder=SOUNDS,
                                           cookies_from_browser=self.cfg.get("cookies_from_browser"))
            except Exception as ex:
                log.exception("quick import failed: %s", job.url)
                full = str(ex).strip() or type(ex).__name__   # (ex is gone once this block ends)
                self.root.after(0, lambda: self._job_failed(job, short_error(full), full[:300]))
                return
            self.root.after(0, lambda: self._job_done(job, out))
        threading.Thread(target=work, daemon=True).start()

    def _job_widgets(self, job):
        return [p.pending[job.id] for p in self.pages.values() if job.id in p.pending]

    def _job_status(self, job, status):
        if self.jobs.get(job.url) is job and job.error is None:
            job.status = status
            for w in self._job_widgets(job):
                w.update_view()

    def _job_failed(self, job, text, full=None):
        if self.jobs.get(job.url) is not job:
            return   # dismissed meanwhile
        job.error, job.full_error = text, full or text
        for w in self._job_widgets(job):
            w.update_view()
        if self._view_key() not in (ALL, NO_GROUP):
            self.toast(f"Couldn't import: {text}", "bad")

    def _job_done(self, job, out):
        self._drop_job(job)
        self.reload_clips()   # the new tile takes the working tile's place in the same frame
        name = out.relative_to(SOUNDS).as_posix()
        for t in self.tiles_for(name):
            t.flash()
        tile = self.page.tiles.get(name) if self.page else None
        if tile is not None:
            self.root.after(30, lambda: self._scroll_into_view(tile))
        self.toast(f"Added '{out.stem}'")

    def retry_job(self, job):
        job.error, job.status = None, "Starting"
        for w in self._job_widgets(job):
            w.update_view()
        self._run_job(job)

    def dismiss_job(self, job):
        self._drop_job(job)
        self._show_page()

    def _drop_job(self, job):
        if self.jobs.get(job.url) is job:
            del self.jobs[job.url]
        for p in self.pages.values():
            p.drop_job(job)

    def _jobs_changed(self):
        for p in self.pages.values():
            p.dirty = True
        self._show_page()
        if self.page:
            self.page.frame._parent_canvas.yview_moveto(0)   # working tiles are at the top

    def _scroll_into_view(self, tile):
        """Scroll the board so tile is on screen (it may have landed far down the list)."""
        if not tile.winfo_exists():
            return
        canvas = self.page.frame._parent_canvas
        total = max(1, self.page.frame.winfo_height())
        top, bottom = (f * total for f in canvas.yview())
        y = tile.winfo_y()
        if y < top or y + tile.winfo_height() > bottom:
            canvas.yview_moveto(max(0.0, (y - 12) / total))

    def show_group(self, key):
        if key == self.cfg["group"]:
            return
        self.cfg["group"] = key
        self._build_group_bar()   # restyles just the two tabs that changed
        self._show_page()
        self.save_config()

    def _search_changed(self):
        query = self.search.get().strip().lower()
        if query == getattr(self, "_last_query", ""):
            return   # arrow keys, shift etc.
        self._last_query = query
        self._show_page()
        self.pages[ALL].frame._parent_canvas.yview_moveto(0)

    # ---------- pages ----------
    def _view_key(self):
        """Which page should be on top: All while searching, else the chosen group."""
        return ALL if self.search.get().strip() else self.cfg["group"]

    def _cols(self):
        width = self.holder.winfo_width() / self.holder._get_widget_scaling() - 24  # minus scrollbar
        return max(1, int(width // TILE_WIDTH)) if width > 1 else 4

    def all_tiles(self):
        for p in self.pages.values():
            yield from p.tiles.values()

    def tiles_for(self, name):
        return [p.tiles[name] for p in self.pages.values() if name in p.tiles]

    def _sync_pages(self, moved=None):
        keys = [ALL] + self.groups + ([NO_GROUP] if self.groups else [])
        for k in list(self.pages):
            if k not in keys:
                if self.pages[k] is self.page:
                    self.page = None
                self.pages.pop(k).destroy()
        for k in keys:
            if k not in self.pages:
                self.pages[k] = Page(self, k)
        view = self._view_key()
        for p in self.pages.values():
            if p.built or p.key == view:
                p.sync(moved)
        self._show_page()
        self.root.after(200, self._prebuild)

    def _show_page(self):
        page = self.pages.get(self._view_key()) or self.pages[ALL]
        if not page.built:
            page.sync()
        page.layout(self._cols(), self.search.get().strip().lower() if page.key == ALL else "")
        if page is not self.page:
            self._swap_to(page)
            self.page = page

    def _swap_to(self, page):
        """Bring an already-drawn page to the front and paint it straight away.
        (No WM_SETREDRAW here: switching it back on can leave black, unpainted patches.)"""
        page.show()
        self.root.update_idletasks()

    # While the app is in use every page stays mapped, stacked under the visible one, so a group
    # switch only raises an already-drawn page. Minimised, the hidden pages are taken off screen:
    # restoring then repaints just the page you see (all of them made restoring jittery), and the
    # others are mapped back underneath once the window is up.
    def _on_minimise(self, e):
        if e.widget is self.root and self.root.state() == "iconic":
            self.close_menus()
            self.toggle_settings(False)
            for p in self.pages.values():
                if p is not self.page and p.built:
                    p.hide()
            # Restoring repaints ~100 small child windows (~200 ms) while the window is already
            # showing, so it visibly fills in. Go invisible now; _on_restore fades it back in once
            # it has repainted, like at startup.
            relayer(self.root)
            self.root.attributes("-alpha", 0.0)
            self._restoring = True

    def _on_restore(self, e):
        if e.widget is self.root and getattr(self, "_restoring", False):
            self._restoring = False
            root = self.root
            animate(root, 180, lambda v: root.attributes("-alpha", v), done=self._unlayer, warmup=True)
            root.after(500, self._untuck)

    def _untuck(self):
        for p in self.pages.values():
            if p.hidden and p is not self.page:
                p.frame.grid()
                p.frame._parent_frame.lower()
                p.hidden = False

    def _prebuild(self):
        """Draw the hidden pages ahead of time, one per tick, so the first switch is instant too."""
        for p in self.pages.values():
            if not p.built:
                p.sync()
                p.layout(self._cols())
                self.root.after(30, self._prebuild)
                return

    def _on_resize(self):
        cols = self._cols()
        if cols != self.columns:
            self.columns = cols
            for p in self.pages.values():
                if p.built:
                    p.layout(cols, p.query or "")

    @contextmanager
    def _frozen(self, settle=True):
        """Pause painting while the board changes, then show the finished result in one frame.
        Without this, Windows paints every tile as it moves, which looks glitchy."""
        self._freeze_depth += 1
        hwnd = None
        # (re-enabling redraw also makes a window visible, so skip it while still hidden)
        if self._freeze_depth == 1 and self.root.winfo_viewable():
            try:
                hwnd = int(self.root.wm_frame(), 16)
                ctypes.windll.user32.SendMessageW(hwnd, WM_SETREDRAW, 0, 0)
            except Exception:
                hwnd = None
        try:
            yield
            if hwnd:
                self.root.update_idletasks()
                if settle:   # new/resized widgets: customtkinter draws them on <Configure> events
                    self.root.update()
        finally:
            self._freeze_depth -= 1
            if hwnd:
                user32 = ctypes.windll.user32
                user32.SendMessageW(hwnd, WM_SETREDRAW, 1, 0)
                user32.RedrawWindow(hwnd, None, None, RDW_REPAINT_ALL)

    def group_color(self, group):
        return self.cfg["group_colors"].get(group) if group not in (ALL, NO_GROUP) else None

    def _assign_group_colors(self):
        """Give every group a colour (unused ones first) and forget colours of gone groups."""
        colors = {g: c for g, c in self.cfg["group_colors"].items() if g in self.groups}
        for g in self.groups:
            if g not in colors:
                free = [c for c in GROUP_COLORS if c not in colors.values()] or GROUP_COLORS
                colors[g] = free[0]
        self.cfg["group_colors"] = colors

    def set_group_color(self, group, color):
        self.cfg["group_colors"][group] = color
        self.save_config()
        self._build_group_bar()
        for t in self.all_tiles():
            if group_of(t.name) == group:   # placeholder picture, dot and playing colour
                t.refresh()
                t._paint()

    def close_menus(self):
        for m in list(self.open_menus):
            m._destroy()
        self.active_menu = None

    def _popup(self, event, master=None, width=None, keep_settings=False):
        """keep_settings: the menu belongs to Settings (its device dropdowns); any other menu
        closes that panel, so the two never overlap."""
        self.close_menus()
        if not keep_settings:
            self.toggle_settings(False)
        self.active_menu = PopupMenu(self, master=master, width=width)
        self._menu_serial = getattr(event, "serial", None)
        return self.active_menu

    def _click_anywhere(self, event):
        """Clicking outside an open menu closes it (tiles handle their own clicks first), and
        clicking outside Settings closes that panel."""
        try:
            top = event.widget.winfo_toplevel()
        except Exception:
            top = None
        in_settings = top is self.settings_win
        # a click in any menu window, including one that's just closing after a pick
        in_menu = isinstance(top, _MenuWindow) and not in_settings and top is not getattr(self, "_toast", None)
        if self.open_menus and event.serial != self._menu_serial and not in_menu:
            self.close_menus()   # including a device list, when you click elsewhere in Settings
        if self.settings_open and not (in_menu or in_settings):
            # the Settings button toggles it itself
            if not str(event.widget).startswith(str(self.settings_btn)):
                self.toggle_settings(False)

    def group_menu(self, event, group, at=None):
        """at: (x, y) to open it at (under its tab) instead of at the pointer."""
        m = self._popup(event)
        m.header(group)
        m.item("Rename group...", lambda: self.rename_group(group))
        m.separator()
        ctk.CTkLabel(m.inner, text="Colour", font=font(12), text_color=MUTED, anchor="w", height=20).pack(
            fill="x", padx=12, pady=(0, 2))
        swatches = ctk.CTkFrame(m.inner, fg_color="transparent")
        swatches.pack(fill="x", padx=10, pady=(2, 6))
        current = self.group_color(group)
        for i, c in enumerate(GROUP_COLORS):
            ctk.CTkButton(swatches, text="", width=26, height=26, corner_radius=13,
                          fg_color=c, hover_color=mix(c, "#ffffff", 0.2),
                          border_width=3 if c == current else 0, border_color=TEXT,
                          command=lambda c=c: m.after_close(lambda: self.set_group_color(group, c))).grid(
                row=0, column=i, padx=(0, 0 if i == len(GROUP_COLORS) - 1 else 6))
        m.separator()
        m.item("Delete group", lambda: self.delete_group(group), danger=True)
        m.show(*(at or (event.x_root, event.y_root)))

    def new_group(self, move=None):
        def ok(text):
            g = importer.safe_name(text)
            (SOUNDS / g).mkdir(exist_ok=True)
            if move:
                self._move_clip_files(move, SOUNDS / g)
                self.toast(f"Moved '{Path(move).stem}' to {g}")
            else:
                self.cfg["group"] = g
            self.reload_clips()
        self._ask_text("New group", "", "Create", ok, placeholder="Group name, e.g. Memes")

    def rename_group(self, group):
        def ok(text):
            new = importer.safe_name(text)
            if new == group:
                return
            if (SOUNDS / new).exists():
                self.toast(f"There's already a group called {new}", "warn")
                return
            self.engine.stop_all()
            (SOUNDS / group).rename(SOUNDS / new)
            prefix = group + "/"
            self.cfg["hotkeys"] = {(new + "/" + k[len(prefix):] if k.startswith(prefix) else k): v
                                   for k, v in self.cfg["hotkeys"].items()}
            if self.cfg["group"] == group:
                self.cfg["group"] = new
            if group in self.cfg["group_colors"]:
                self.cfg["group_colors"][new] = self.cfg["group_colors"].pop(group)
            self.reload_clips()
        self._ask_text("Rename group", group, "Save", ok)

    def delete_group(self, group):
        def do():
            folder = SOUNDS / group
            for name in [n for n in self.clips if group_of(n) == group]:
                self._move_clip_files(name, SOUNDS)
            leftovers = [p for p in folder.iterdir()]
            if leftovers:
                TRASH.mkdir(exist_ok=True)
                for p in leftovers:
                    shutil.move(str(p), str(importer.unique_path(TRASH, p.stem, p.suffix)))
            folder.rmdir()
            self.reload_clips()
        self._confirm("Delete group?", f"The sounds in '{group}' aren't deleted. They move to No group.",
                      "Delete", do)

    def move_clip(self, name, group, announce=False):
        if name not in self.clips:
            return
        self._move_clip_files(name, SOUNDS / group if group else SOUNDS)
        self.reload_clips()
        if announce:
            self.toast(f"Moved '{Path(name).stem}' to {group or 'No group'}")

    def set_order(self, order, dropped=None, snaps=None):
        """Show the board in this order and remember it. The tiles you can see slide to their new
        spots; dropped (the sound you just let go of) goes straight to its new one instead."""
        self.cfg["order"] = [n for n in order if n in self.clips]
        self.clips = {n: self.clips[n] for n in self.cfg["order"]}
        self.save_config()
        page = self.page
        if page is not None:
            page.stop_slide()   # a slide still running from the last drop ends where it was headed
            page.slide_from = (page.cells(), dropped, snaps)
        # only the order changed: move tiles, don't re-render them (a full sync of every page plus a
        # whole-window repaint held the drop up ~200 ms before its copy could glide in)
        for p in self.pages.values():
            if p.built:
                p.reorder()
        self._show_page()   # the page you see slides; the others catch up when shown

    def sort_az(self):
        # a copy of the board first, so the whole board glides into order on one canvas (moving every
        # tile window at once tore and dragged)
        snaps = self.page.snapshot(settle=True) if self.page is not None else None
        self.set_order(sorted(self.clips, key=lambda n: Path(n).stem.lower()), snaps=snaps)
        self.toast("Sorted A–Z")

    def _move_clip_files(self, name, folder, new_stem=None):
        """Move a clip (and its picture) to folder, keeping its hotkey. Returns the new name."""
        src = SOUNDS / name
        folder.mkdir(exist_ok=True)
        dst = importer.unique_path(folder, new_stem or src.stem, src.suffix)
        self.engine.stop_clip(name)
        src.rename(dst)
        pic = importer.picture_path(src)
        if pic.exists():
            pic.replace(importer.picture_path(dst))
        new = dst.relative_to(SOUNDS).as_posix()
        self._moved[new] = name   # lets reload_clips keep its audio and tile
        if name in self.cfg["hotkeys"]:
            self.cfg["hotkeys"][new] = self.cfg["hotkeys"].pop(name)
        return new

    # ---------- pictures ----------
    def _clipboard_picture(self):
        """An image (or image file) on the clipboard, else None."""
        try:
            got = ImageGrab.grabclipboard()
        except Exception:
            return None
        if isinstance(got, Image.Image):
            return got
        if isinstance(got, list):
            for p in got:
                if Path(p).suffix.lower() in (".png", ".jpg", ".jpeg", ".webp", ".gif", ".bmp"):
                    return p
        return None

    def set_picture(self, name, image):
        try:
            importer.save_picture(image, SOUNDS / name)
        except Exception as ex:
            log.exception("picture failed")
            self.toast(f"Couldn't use that picture: {ex}", "bad")
            return
        for t in self.tiles_for(name):
            t.refresh()

    def choose_picture(self, name):
        path = filedialog.askopenfilename(
            title="Choose a picture", parent=self.root,
            filetypes=[("Pictures", "*.png *.jpg *.jpeg *.webp *.gif *.bmp"), ("All", "*.*")])
        if path:
            self.set_picture(name, path)

    def remove_picture(self, name):
        importer.picture_path(SOUNDS / name).unlink(missing_ok=True)
        for t in self.tiles_for(name):
            t.refresh()

    def play(self, name):
        """A click or a hotkey. Playback settings: with toggle_stop, pressing a sound that's playing
        stops it; with solo, starting a sound stops whatever else is playing first."""
        data = self.clips.get(name)
        if data is None:
            return
        if self.cfg.get("toggle_stop") and name in self.engine.progress():
            self.engine.stop_clip(name)   # its tile settles back on the next progress poll
            return
        if self.cfg.get("solo"):
            self.engine.stop_all()
        self.engine.play(name, data)
        # hotkeys call this from the keyboard thread: light the tile up on the UI thread
        self.root.after(0, lambda: [t.flash() for t in self.tiles_for(name) if t.winfo_exists()])

    def _poll_playing(self):
        progress = self.engine.progress()
        self._polls += 1
        frame = (self._polls // 3) % 3   # equalizer steps every 150 ms
        for tile in self.all_tiles():
            tile.set_progress(progress.get(tile.name))
            if tile.playing:
                tile.tick_art(frame)
        self._poll_id = self.root.after(50, self._poll_playing)

    def clip_menu(self, event, name):
        m = self._popup(event)
        current = group_of(name)
        m.header(Path(name).stem)
        m.item("Play", lambda: self.play(name), instant=True)
        key = pretty_key(self.cfg["hotkeys"].get(name))
        m.item(f"Change hotkey  ({key})..." if key else "Set hotkey...", lambda: self.set_clip_hotkey(name))
        if key:
            m.item("Clear hotkey", lambda: self._assign(name, None))
        m.separator()

        def groups(sub):
            for g in self.groups:
                sub.item(g, lambda g=g: self.move_clip(name, g), dot=self.group_color(g), checked=g == current)
            if current:
                sub.item("No group", lambda: self.move_clip(name, NO_GROUP), dot=MUTED)
            if self.groups:
                sub.separator()
            sub.item("New group...", lambda: self.new_group(move=name))

        clip_pic = self._clipboard_picture()
        has_pic = importer.picture_path(SOUNDS / name).exists()

        def pictures(sub):
            sub.item("Choose picture...", lambda: self.choose_picture(name))
            sub.item("Paste picture", lambda: self.set_picture(name, clip_pic), enabled=clip_pic is not None)
            if has_pic:
                sub.separator()
                sub.item("Remove picture", lambda: self.remove_picture(name), danger=True)

        m.submenu("Move to group", groups)
        m.submenu("Picture", pictures)
        m.separator()
        m.item("Rename...", lambda: self.rename_clip(name))
        m.item("Delete", lambda: self.delete_clip(name), danger=True)
        m.show(event.x_root, event.y_root)

    # ---------- dialogs ----------
    def _dialog(self, title, width, height=None):
        """A popup that appears in one smooth fade. It stays hidden while customtkinter recolours
        its title bar (a hide/show flash) and while the caller fills it in, then fades in.
        height=None: as tall as its contents (win.fit() re-fits it after the contents change)."""
        self.close_menus()
        self.toggle_settings(False)   # one thing open at a time
        win = ctk.CTkToplevel(self.root, fg_color=BG)
        # customtkinter already made the title bar dark while creating the window. Stop it doing
        # that again: resizable() below would schedule another hide/show, which can land after our
        # reveal and leave an invisible popup holding the click grab, so the whole app looks frozen.
        win._deactivate_windows_window_header_manipulation = True
        win.withdraw()   # also stops customtkinter re-showing it after its first title bar trick
        win.title(title)
        win.resizable(False, False)
        win.transient(self.root)
        icon = theme_icon()
        if icon.exists():
            win.iconbitmap(str(icon))   # now, so customtkinter doesn't swap its own icon in later
            crisp_icons(win, icon)
        title_bar_colors(win)
        self.root.update_idletasks()
        scale = self.root._get_window_scaling()

        def place(h):
            # centred across the main window, a little above the middle
            x = self.root.winfo_rootx() + (self.root.winfo_width() - int(width * scale)) // 2
            y = self.root.winfo_rooty() + max(0, (self.root.winfo_height() - int(h * scale)) // 3)
            win.geometry(f"{width}x{h}+{x}+{y}")

        def fit():
            """Size the popup to its contents (they're packed, so their requested height is it)."""
            if height is None and win.winfo_exists():
                win.update_idletasks()
                place(round(win.winfo_reqheight() / scale))
        win.fit = fit
        place(height or 300)

        def show():
            if not win.winfo_exists():
                return
            fit()
            win.attributes("-alpha", 0.0)
            x, y = win_pos(win)
            rise = round(10 * scale)
            win.geometry(f"+{x}+{y + rise}")
            win.deiconify()
            win.lift()
            win.focus_force()
            win.update_idletasks()
            win.grab_set()
            token = win.motion = object()

            def step(e):   # rises into place while it fades in
                if win.motion is not token:
                    return False
                win.attributes("-alpha", e)
                win.geometry(f"+{x}+{y + round(rise * (1 - e))}")
            # the first frames run invisible (animate's warm-up), so it's fully drawn before it shows
            animate(win, MOTION_OPEN + 20, step, warmup=True)
            later(win, 400, make_sure_visible)

        def close(then=None):
            """Fade out, then destroy, then run then(). The app is clickable again straight away."""
            if getattr(win, "closing", False) or not win.winfo_exists():
                return
            win.closing = True
            try:
                win.grab_release()
            except tk.TclError:
                pass

            def gone():
                win.destroy()
                if then:
                    then()
            fade_away(win, 0, round(6 * scale), done=gone)
        win.close = close
        win.motion = None
        win.protocol("WM_DELETE_WINDOW", close)
        win.bind("<Escape>", lambda e=None: close(), add="+")

        def make_sure_visible():
            # never leave a hidden popup holding the click grab
            if win.winfo_exists() and not win.winfo_viewable() and not getattr(win, "closing", False):
                log.warning("dialog %r was hidden after showing; showing it again", title)
                win.deiconify()
                win.lift()
                win.attributes("-alpha", 1.0)

        later(win, 30, show)   # the caller builds the contents first
        return win

    def _confirm(self, title, text, ok_text, on_ok, kind="danger"):
        win = self._dialog(title, 380, 170)
        ctk.CTkLabel(win, text=title, font=font(16, "bold"), text_color=TEXT).pack(anchor="w", padx=22, pady=(20, 2))
        ctk.CTkLabel(win, text=text, font=font(13), text_color=MUTED, justify="left",
                     wraplength=330).pack(anchor="w", padx=22)
        row = ctk.CTkFrame(win, fg_color="transparent")
        row.pack(side="bottom", fill="x", padx=22, pady=18)
        button(row, ok_text, lambda: win.close(on_ok), kind, width=100).pack(side="right")
        button(row, "Cancel", win.close, "ghost", width=80).pack(side="right", padx=8)

    def _ask_text(self, title, initial, ok_text, on_ok, placeholder=""):
        """Small dialog with one text box; on_ok(text) runs if the text isn't empty."""
        win = self._dialog(title, 380, 170)
        ctk.CTkLabel(win, text=title, font=font(16, "bold"), text_color=TEXT).pack(
            anchor="w", padx=22, pady=(20, 8))
        entry = ctk.CTkEntry(win, placeholder_text=placeholder, height=36, corner_radius=10, fg_color=CARD,
                             border_color=BORDER, text_color=TEXT, placeholder_text_color=MUTED, font=font(13))
        if initial:
            entry.insert(0, initial)
        entry.pack(fill="x", padx=22)
        later(win, 150, lambda: (entry.focus_set(), entry.select_range(0, "end")))

        def ok(*_):
            text = entry.get().strip()
            win.close((lambda: on_ok(text)) if text else None)

        entry.bind("<Return>", ok)
        row = ctk.CTkFrame(win, fg_color="transparent")
        row.pack(side="bottom", fill="x", padx=22, pady=18)
        button(row, ok_text, ok, "primary", width=90).pack(side="right")
        button(row, "Cancel", win.close, "ghost", width=80).pack(side="right", padx=8)

    def rename_clip(self, name):
        def ok(new):
            if new != Path(name).stem:
                self._move_clip_files(name, (SOUNDS / name).parent, importer.safe_name(new))
                self.reload_clips()
        self._ask_text("Rename sound", Path(name).stem, "Save", ok)

    def delete_clip(self, name):
        def do():
            TRASH.mkdir(exist_ok=True)
            src = SOUNDS / name
            dst = importer.unique_path(TRASH, src.stem, src.suffix)
            shutil.move(str(src), str(dst))
            if importer.picture_path(src).exists():
                shutil.move(str(importer.picture_path(src)), str(importer.picture_path(dst)))
            self.engine.stop_clip(name)
            self.reload_clips()
        self._confirm("Delete sound?", f"'{Path(name).stem}' is moved to sounds\\_deleted, so you can "
                      "get it back later.", "Delete", do)

    def _current_folder(self):
        """Where new sounds go: the group being shown, else sounds/ itself."""
        g = self.cfg["group"]
        return SOUNDS / g if g not in (ALL, NO_GROUP) else SOUNDS

    def add_files(self):
        paths = filedialog.askopenfilenames(
            title="Add sounds", parent=self.root,
            filetypes=[("Audio/video", " ".join("*" + e for e in sorted(engine.AUDIO_EXTS))), ("All", "*.*")])
        folder = self._current_folder()
        for p in paths:
            p = Path(p)
            shutil.copy2(p, importer.unique_path(folder, importer.safe_name(p.stem), p.suffix.lower()))
        if paths:
            self.reload_clips()

    # ---------- hotkeys ----------
    def _apply_hotkeys(self):
        pairs = [(b, lambda n=name: self.play(n)) for name, b in self.cfg["hotkeys"].items()]
        pairs.append((self.cfg["stop_hotkey"], self.engine.stop_all))
        self.hotkeys.set_bindings(pairs)
        stop = pretty_key(self.cfg["stop_hotkey"])
        self.stop_key_btn.configure(text=f"Hotkey: {stop}" if stop else "Set hotkey")
        for t in self.all_tiles():
            t.refresh()

    def _record(self, what, on_binding):
        win = self._dialog("Hotkey", 360, 190)
        ctk.CTkLabel(win, text=what, font=font(13), text_color=MUTED).pack(pady=(24, 4))
        ctk.CTkLabel(win, text="Press a key", font=font(22, "bold"), text_color=TEXT).pack()
        ctk.CTkLabel(win, text="Ctrl / Shift / Alt combos work  ·  Esc to cancel", font=font(12),
                     text_color=MUTED).pack(pady=(6, 0))

        def got(binding):
            def finish():
                if win.winfo_exists() and not getattr(win, "closing", False):
                    win.close((lambda: on_binding(binding)) if binding else None)
                elif binding:
                    on_binding(binding)
            self.root.after(0, finish)

        def cancel(*_):
            self.hotkeys.cancel_record()
            win.close()
        win.protocol("WM_DELETE_WINDOW", cancel)
        win.bind("<Escape>", cancel)
        self.hotkeys.record(got)

    def _same(self, a, b):
        return bool(a and b) and (a["scan"], a.get("keypad"), sorted(a.get("mods", []))) == \
            (b["scan"], b.get("keypad"), sorted(b.get("mods", [])))

    def _assign(self, name, binding):
        if binding:
            # one key = one action
            self.cfg["hotkeys"] = {k: v for k, v in self.cfg["hotkeys"].items() if not self._same(v, binding)}
            if self._same(self.cfg["stop_hotkey"], binding):
                self.cfg["stop_hotkey"] = None
            self.cfg["hotkeys"][name] = binding
        else:
            self.cfg["hotkeys"].pop(name, None)
        self.save_config()
        self._apply_hotkeys()

    def set_clip_hotkey(self, name):
        self._record(f"Hotkey for '{Path(name).stem}'", lambda b: self._assign(name, b))

    def set_stop_hotkey(self):
        def done(b):
            self.cfg["hotkeys"] = {k: v for k, v in self.cfg["hotkeys"].items() if not self._same(v, b)}
            self.cfg["stop_hotkey"] = b
            self.save_config()
            self._apply_hotkeys()
        self._record("Hotkey for 'Stop all'", done)

    # ---------- import ----------
    def import_dialog(self):
        win = self._dialog("Import", 480)
        ctk.CTkLabel(win, text="Import a sound", font=font(18, "bold"), text_color=TEXT).pack(
            anchor="w", padx=24, pady=(22, 2))
        ctk.CTkLabel(win, text="From an Instagram reel or a YouTube video. Only the link is needed.",
                     font=font(12), text_color=MUTED).pack(anchor="w", padx=24, pady=(0, 16))

        def caption(parent, text):
            return ctk.CTkLabel(parent, text=text, font=font(11, "bold"), text_color=MUTED, anchor="w", height=16)

        def field(parent, placeholder):
            e = ctk.CTkEntry(parent, placeholder_text=placeholder, height=38, corner_radius=10, fg_color=CARD,
                             border_width=1, border_color=BORDER, text_color=TEXT, placeholder_text_color=MUTED,
                             font=font(13))
            # the field you're typing in gets an accent outline
            e.bind("<FocusIn>", lambda _: e.configure(border_color=ACCENT), add="+")
            e.bind("<FocusOut>", lambda _: e.configure(border_color=BORDER), add="+")
            return e

        caption(win, "Link").pack(fill="x", padx=26, pady=(0, 4))
        url = field(win, "https://www.instagram.com/reel/...  or  https://youtu.be/...")
        url.pack(fill="x", padx=24)
        try:
            link = find_link(self.root.clipboard_get())
            if link:
                url.insert(0, link)
        except tk.TclError:
            pass
        caption(win, "Name").pack(fill="x", padx=26, pady=(14, 4))
        name = field(win, "Leave empty to use the video's title")
        name.pack(fill="x", padx=24)

        # start / end / group on one row
        opts = ctk.CTkFrame(win, fg_color="transparent")
        opts.pack(fill="x", padx=24, pady=(14, 0))
        opts.columnconfigure((0, 1), weight=1, uniform="t")
        opts.columnconfigure(2, weight=2, uniform="t")
        caption(opts, "Start").grid(row=0, column=0, sticky="ew", padx=(2, 6), pady=(0, 4))
        caption(opts, "End").grid(row=0, column=1, sticky="ew", padx=(8, 6), pady=(0, 4))
        caption(opts, "Group").grid(row=0, column=2, sticky="ew", padx=(8, 0), pady=(0, 4))
        start = field(opts, "0:03.5")
        start.grid(row=1, column=0, sticky="ew", padx=(0, 6))
        end = field(opts, "0:07")
        end.grid(row=1, column=1, sticky="ew", padx=(6, 6))
        no_group = "No group"
        g = self.cfg["group"]
        group_var = tk.StringVar(value=g if g not in (ALL, NO_GROUP) else no_group)
        Select(opts, self, group_var, [no_group] + self.groups, height=38).grid(row=1, column=2, sticky="ew",
                                                                              padx=(6, 0))

        norm = tk.BooleanVar(value=True)
        labelled_toggle(win, "Even out loudness", norm, bg=BG).pack(anchor="w", padx=24, pady=(16, 0))
        msg = ctk.CTkLabel(win, text="", font=font(12), text_color=MUTED, wraplength=430, justify="left")
        spinner = ctk.CTkProgressBar(win, mode="indeterminate", height=3, corner_radius=2, progress_color=ACCENT,
                                     fg_color=CARD)
        line = tk.Frame(win, height=1, bg=BORDER, bd=0, highlightthickness=0)
        line.pack(fill="x", pady=(18, 0))
        row = ctk.CTkFrame(win, fg_color="transparent")
        row.pack(fill="x", padx=24, pady=16)
        go = button(row, "Import", None, "primary", width=120, height=38)
        go.pack(side="right")
        button(row, "Cancel", win.close, "ghost", width=88, height=38).pack(side="right", padx=8)

        def show_msg(text, color):
            # the message (and the loading bar) sit above the line; the popup grows to fit them
            msg.configure(text=text, text_color=color)
            if text:
                msg.pack(anchor="w", padx=24, pady=(12, 0), before=line)
            else:
                msg.pack_forget()
            win.fit()

        def ui(text, color=MUTED):
            # progress lines from the download ("Downloading ...", "Cutting audio ...")
            self.root.after(0, lambda: show_msg(text, color) if win.winfo_exists() else None)

        def worker(v):
            try:
                out = importer.import_clip(v["url"], v["name"] or None, v["start"] or None, v["end"] or None,
                                           normalize=norm.get(),
                                           cookies_from_browser=self.cfg.get("cookies_from_browser"),
                                           log=ui, folder=v["folder"])
            except Exception as ex:
                log.exception("import failed")
                text = str(ex)
                if "login" in text.lower() or "cookies" in text.lower():
                    text += ("\n\nInstagram wants a login. Set \"cookies_from_browser\": \"firefox\" "
                             "in config.json (while logged in to Instagram in Firefox).")

                def fail():
                    if win.winfo_exists():
                        spinner.stop()
                        spinner.pack_forget()
                        go.configure(state="normal")
                        show_msg("Failed: " + text, DANGER)
                self.root.after(0, fail)
                return

            def done():
                def added():
                    self.reload_clips()
                    self.toast(f"Added '{out.stem}'")
                if win.winfo_exists() and not getattr(win, "closing", False):
                    win.close(added)
                else:
                    added()
            self.root.after(0, done)

        def begin(*_):
            v = {"url": url.get().strip(), "name": name.get().strip(),
                 "start": start.get().strip(), "end": end.get().strip(),
                 "folder": SOUNDS if group_var.get() == no_group else SOUNDS / group_var.get()}
            if not v["url"]:
                show_msg("Paste an Instagram or YouTube link first.", WARN)
                url.focus_set()
                return
            try:
                importer.parse_time(v["start"])
                importer.parse_time(v["end"])
            except ValueError:
                show_msg("Times look like 7, 0:07 or 0:07.5", WARN)
                return
            go.configure(state="disabled")
            spinner.pack(fill="x", padx=24, pady=(12, 0), before=line)
            spinner.start()
            win.fit()
            threading.Thread(target=worker, args=(v,), daemon=True).start()

        go.configure(command=begin)
        url.bind("<Return>", begin)
        later(win, 150, url.focus_set)

    # ---------- misc ----------
    def save_config(self):
        try:
            CONFIG.write_text(json.dumps(self.cfg, indent=2), encoding="utf-8")
        except Exception:
            log.exception("could not save config")

    # ---------- themes ----------
    def _button_menu(self, b, build):
        """A menu that drops from a toolbar button; clicking the button again closes it."""
        if self.open_menus and getattr(self.active_menu, "owner", None) is b:
            self.close_menus()
            return
        # open after this click is fully handled, or the click-outside check closes it at once
        self.root.after(1, build)

    def theme_menu(self):
        self._button_menu(self.theme_btn, self._theme_menu)

    def more_menu(self):
        self._button_menu(self.more_btn, self._more_menu)

    def _more_menu(self):
        b = self.more_btn
        m = self._popup(None)
        m.owner = b
        m.item("Open sounds folder", lambda: os.startfile(SOUNDS))
        m.item("Reload sounds", self.reload_clips)
        m.item("Sort sounds A–Z", self.sort_az)
        m.separator()
        stop = pretty_key(self.cfg["stop_hotkey"])
        m.item(f"Stop all hotkey  ({stop})..." if stop else "Set Stop all hotkey...", self.set_stop_hotkey)
        s = self.root._get_window_scaling()
        m.show(b.winfo_rootx(), b.winfo_rooty() + b.winfo_height() + round(8 * s))

    def _theme_menu(self):
        b = self.theme_btn
        m = self._popup(None)
        m.owner = b
        m.header("Theme")
        for name, t in THEMES.items():
            on = name == self.cfg.get("theme")
            m.item(name, lambda n=name: self.switch_theme(n), dot=t["ACCENT"], checked=on)
        s = self.root._get_window_scaling()
        m.show(b.winfo_rootx() + b.winfo_width(), b.winfo_rooty() + b.winfo_height() + round(8 * s), align_right=True)

    def switch_theme(self, name):
        """Colours are read while widgets are built, so rebuild the window: close it and let main()
        open a fresh one in the new theme, at the same size and spot."""
        self.cfg["theme"] = name
        self.restart = True
        self.quit()

    def quit(self):
        if self.root.state() == "normal":
            self.cfg["geometry"] = self.root.geometry()
        self.save_config()
        self.close_menus()
        for job in (getattr(self, "_poll_id", None), getattr(self, "_watch_id", None)):
            if job:
                self.root.after_cancel(job)   # don't let it fire into the next window
        self.hotkeys.close()
        self.engine.stop()
        if self.restart:   # theme switch: fade out, the new window fades in at the same spot
            relayer(self.root)
            for a in (0.7, 0.4, 0.15):
                self.root.attributes("-alpha", a)
                self.root.update_idletasks()
                time.sleep(0.014)
        # stop the main loop; _run_windows destroys the window once it has returned. Destroying it here,
        # from inside a menu's timer, tore down tkinter's callback bookkeeping mid-callback
        self.root.quit()


APP_ID = "JondisBoard.Soundboard"


def main():
    # Our own taskbar identity, before any window exists. Started from a shortcut (the desktop one),
    # Windows otherwise ties the taskbar button to that shortcut and shows its icon (the exe's,
    # always green) instead of the window's, so the taskbar ignored the theme.
    try:
        ctypes.windll.shell32.SetCurrentProcessExplicitAppUserModelID(APP_ID)
    except Exception:
        log.exception("could not set the app id")
    ctk.set_appearance_mode("dark")
    smooth_timers(True)
    try:
        _run_windows()
    finally:
        smooth_timers(False)


def _run_windows():
    while True:
        root = ctk.CTk()
        root.report_callback_exception = lambda *a: log.error("UI error", exc_info=a)
        app = App(root)
        root.mainloop()
        try:
            root.destroy()
        except tk.TclError:
            log.exception("closing the window")
        if not app.restart:
            break


if __name__ == "__main__":
    main()
