"""JondisBoard: play clips into Discord / games through a virtual audio cable."""
import ctypes
import json
import logging
import os
import shutil
import sys
import threading
import tkinter as tk
from contextlib import contextmanager
from pathlib import Path
from tkinter import filedialog

import customtkinter as ctk
from PIL import Image, ImageDraw, ImageGrab, ImageOps

import engine
import importer
from hotkeys import HotkeyManager, binding_label

ROOT = importer.APP_DIR
SOUNDS = ROOT / "sounds"
TRASH = SOUNDS / "_deleted"
CONFIG = ROOT / "config.json"
ASSETS = Path(getattr(sys, "_MEIPASS", ROOT))  # bundled files when running as the exe
TILE_WIDTH = 210
THUMB = 56
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


def apply_theme(name):
    globals().update(THEMES.get(name) or THEMES["Toxic Green"])


apply_theme("Toxic Green")


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


def mix(a, b, t):
    """Blend hex colour a toward b by t (0..1)."""
    ca = [int(a[i:i + 2], 16) for i in (1, 3, 5)]
    cb = [int(b[i:i + 2], 16) for i in (1, 3, 5)]
    return "#" + "".join(f"{round(x + (y - x) * t):02x}" for x, y in zip(ca, cb))


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


MENU_KEY = "#010203"   # painted see-through, so the menu window gets rounded corners
MENU_WIDTH = 220


WM_SETREDRAW = 0x000B
# RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_UPDATENOW | RDW_FRAME
RDW_REPAINT_ALL = 0x0001 | 0x0004 | 0x0080 | 0x0100 | 0x0400


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
    """Dark rounded right-click menu with hover highlights and side submenus."""

    def __init__(self, app, parent=None):
        self.app, self.parent = app, parent
        self.child = self.child_owner = None
        self.hovered = None
        self.had_focus = False
        self.opening = False
        app.open_menus.append(self)
        self.win = ctk.CTkToplevel(app.root, fg_color=MENU_KEY)
        self.win.withdraw()
        self.win.overrideredirect(True)
        self.win.attributes("-topmost", True)
        self.win.attributes("-transparentcolor", MENU_KEY)
        self.win.attributes("-alpha", 0.0)   # faded in by show()
        box = ctk.CTkFrame(self.win, fg_color=SURFACE, bg_color=MENU_KEY, corner_radius=12,
                           border_width=1, border_color=BORDER)
        box.pack()
        self.inner = ctk.CTkFrame(box, fg_color="transparent", width=MENU_WIDTH)
        self.inner.pack(padx=6, pady=6)
        self.win.bind("<Escape>", lambda e: self.close_all())

    @property
    def alive(self):
        return self in self.app.open_menus and self.win.winfo_exists()

    # --- building ---
    def header(self, text):
        ctk.CTkLabel(self.inner, text=text, font=font(12, "bold"), text_color=MUTED, anchor="w",
                     justify="left", width=MENU_WIDTH - 24, wraplength=MENU_WIDTH - 24).pack(
            fill="x", padx=12, pady=(4, 6))

    def _row(self, text, on_click, color=None, dot=None, enabled=True, arrow=False):
        """One menu line. Built from a frame + labels, so the highlight covers the whole row."""
        color = color or TEXT
        row = ctk.CTkFrame(self.inner, fg_color=SURFACE, corner_radius=8, height=32, width=MENU_WIDTH)
        row.lit = row.goal = 0.0   # highlight amount, animated by _paint
        row.anim = None
        row.pack_propagate(False)
        row.pack(fill="x", pady=1)
        if dot:
            ctk.CTkLabel(row, text="", image=dot_image(dot), width=10).pack(side="left", padx=(12, 0))
        ctk.CTkLabel(row, text=text, font=font(13), text_color=color if enabled else MUTED, anchor="w",
                     height=30).pack(side="left", fill="x", expand=True, padx=(10 if dot else 12, 0))
        if arrow:
            ctk.CTkLabel(row, text="›", font=font(17), text_color=MUTED, width=14, height=30).pack(
                side="right", padx=(0, 10))
        if enabled:
            # self.alive: a closing menu is still on screen while it fades out
            for w in [row, *row.winfo_children()]:
                w.bind("<Enter>", lambda e: self.alive and self._enter(row), add="+")
                w.bind("<Leave>", lambda e: self.alive and self._leave(row, e), add="+")
                w.bind("<Button-1>", lambda e: self.alive and on_click(), add="+")
        return row

    def item(self, text, command, danger=False, dot=None, enabled=True):
        def click():
            self.close_all()
            command()
        row = self._row(text, click, DANGER if danger else TEXT, dot, enabled)
        row.submenu = None

    def submenu(self, text, build):
        row = self._row(text, lambda: self._open_child(row), arrow=True)
        row.submenu = build

    def separator(self):
        # plain tk frame: a 1px CTkFrame doesn't draw anything
        tk.Frame(self.inner, height=1, bg=BORDER, bd=0, highlightthickness=0).pack(fill="x", padx=8, pady=5)

    # --- hover ---
    def _paint(self, row, on):
        row.goal = 1.0 if on else 0.0
        if row.anim is None and row.lit != row.goal:
            self._fade_row(row)

    def _fade_row(self, row):
        """Ease the row's highlight toward its goal: ~50 ms in, a bit slower out."""
        if not row.winfo_exists():
            return
        step = 0.34 if row.goal > row.lit else 0.25
        row.lit = min(row.goal, row.lit + step) if row.goal > row.lit else max(row.goal, row.lit - step)
        row.configure(fg_color=mix(SURFACE, CARD_HOVER, row.lit))
        row.anim = self.win.after(16, lambda: self._fade_row(row)) if row.lit != row.goal else None

    def _enter(self, row):
        if self.hovered is not row:
            self.hovered = row
            self._repaint()
        if row.submenu:
            self.app.root.after(120, lambda: self._hover_open(row))
        elif self.child and not self.opening:
            self._close_child()

    def _leave(self, row, e):
        under = row.winfo_containing(e.x_root, e.y_root)
        if under is not None and str(under).startswith(str(row)):
            return   # moved between the row's own labels
        if self.hovered is row:
            self.hovered = None
        self._repaint()   # the row stays lit while its submenu is open

    def _hover_open(self, row):
        if self.alive and self.hovered is row:
            self._open_child(row)

    # --- showing / closing ---
    def show(self, x, y, flip_x=None):
        self.win.update_idletasks()
        w, h = self.win.winfo_reqwidth(), self.win.winfo_reqheight()
        sw, sh = self.win.winfo_screenwidth(), self.win.winfo_screenheight()
        if x + w > sw:
            x = flip_x - w if flip_x is not None else sw - w - 4
        y = max(0, min(y, sh - h - 48))
        self.win.geometry(f"+{int(x)}+{int(y)}")
        self.win.deiconify()
        self.win.lift()
        self.win.focus_force()
        # submenus slide in from the side, menus drop down a touch
        self._animate_in(int(x), int(y), (-8, 0) if self.parent else (0, -6), 0)
        if not self.parent:
            self.app.root.after(150, self._watch_focus)

    def _animate_in(self, x, y, offset, i, frames=8):
        if not self.alive:   # closed mid-animation; _fade_out has it now
            return
        t = (i + 1) / frames
        ease = 1 - (1 - t) ** 3   # ease-out cubic
        self.win.attributes("-alpha", ease)
        self.win.geometry(f"+{x + round(offset[0] * (1 - ease))}+{y + round(offset[1] * (1 - ease))}")
        if i + 1 < frames:
            self.win.after(14, lambda: self._animate_in(x, y, offset, i + 1, frames))

    def contains(self, widget):
        return any(str(widget).startswith(str(m.win)) for m in self.app.open_menus)

    def _watch_focus(self):
        """Close once another program (e.g. the game) comes to the front."""
        if not self.alive:
            return
        if app_has_focus():
            self.had_focus = True
        elif self.had_focus:
            self.close_all()
            return
        self.app.root.after(150, self._watch_focus)

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
            child.show(self.win.winfo_rootx() + self.win.winfo_width() + 2, row.winfo_rooty() - 7,
                       flip_x=self.win.winfo_rootx() - 2)
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
        if self.win.winfo_exists():
            self.win.focus_force()

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
        if self.win.winfo_exists():
            self._fade_out(1.0)

    def _fade_out(self, alpha):
        if not self.win.winfo_exists():
            return
        alpha -= 0.25
        if alpha <= 0:
            self.win.destroy()
            return
        self.win.attributes("-alpha", min(alpha, float(self.win.attributes("-alpha"))))
        self.win.after(14, lambda: self._fade_out(alpha))

    def close_all(self):
        self.app.close_menus()   # every menu window, so none can be left behind


_thumbs = {}   # (path, mtime) -> CTkImage


def load_thumb(path):
    """A clip's picture as a rounded-corner CTkImage, or None."""
    try:
        key = (str(path), path.stat().st_mtime)
    except OSError:
        return None
    if key not in _thumbs:
        try:
            size = THUMB * 2   # sharp on high-DPI screens
            im = ImageOps.fit(Image.open(path).convert("RGBA"), (size, size), Image.LANCZOS)
            mask = Image.new("L", (size, size), 0)
            ImageDraw.Draw(mask).rounded_rectangle((0, 0, size - 1, size - 1), radius=size // 6, fill=255)
            im.putalpha(mask)
            _thumbs[key] = ctk.CTkImage(im, size=(THUMB, THUMB))
        except Exception:
            log.exception("bad picture %s", path)
            return None
    return _thumbs[key]


def button(master, text, command, kind="secondary", **kw):
    styles = {
        "primary": dict(fg_color=ACCENT, hover_color=ACCENT_HOVER, text_color=ACCENT_TEXT),
        "secondary": dict(fg_color=CARD, hover_color=CARD_HOVER, text_color=TEXT,
                          border_width=1, border_color=BORDER),
        "ghost": dict(fg_color="transparent", hover_color=CARD, text_color=MUTED),
        "danger": dict(fg_color=DANGER, hover_color=DANGER_HOVER, text_color="white"),
    }
    opts = dict(height=34, corner_radius=10, font=font(13, "bold" if kind == "primary" else "normal"))
    opts.update(styles[kind])
    opts.update(kw)
    return ctk.CTkButton(master, text=text, command=command, **opts)


class Tile(ctk.CTkFrame):
    """One sound pad: click to play, right-click for options."""

    def __init__(self, master, app, name):
        super().__init__(master, fg_color=CARD, corner_radius=14, border_width=1,
                         border_color=BORDER, height=96)
        self.app, self.name = app, name
        self.playing = False
        self.hover = False
        self.grid_propagate(False)
        self.columnconfigure(1, weight=1)
        self.rowconfigure(0, weight=1)

        # column 0 holds the picture, when the clip has one
        self.pic = ctk.CTkLabel(self, text="", width=THUMB, height=THUMB)
        # fixed two-line height so every tile is the same size
        self.title = ctk.CTkLabel(self, text=Path(name).stem, font=font(13, "bold"), text_color=TEXT,
                                  anchor="nw", justify="left", height=40)
        self.title.grid(row=0, column=1, columnspan=2, sticky="new", pady=(12, 0))
        self.meta = ctk.CTkLabel(self, text="", font=font(11), text_color=MUTED, anchor="w", height=18)
        self.meta.grid(row=1, column=1, sticky="w")
        self.chip = ctk.CTkLabel(self, text="", font=font(11, "bold"), text_color=TEXT, fg_color=BORDER,
                                 corner_radius=6, height=20)
        self.chip.grid(row=1, column=2, sticky="e", padx=12)
        self.bar = ctk.CTkProgressBar(self, height=3, corner_radius=2, fg_color=CARD, progress_color=CARD)
        self.bar.set(0)
        self.bar.grid(row=2, column=0, columnspan=3, sticky="ew", padx=14, pady=(6, 10))

        for w in (self, self.pic, self.title, self.meta, self.chip, self.bar):
            # self.name, not name: a tile keeps its widget when its sound is moved or renamed
            w.bind("<Button-1>", lambda e: app.tile_click(self.name))
            w.bind("<Button-3>", lambda e: app.clip_menu(e, self.name))
            w.bind("<Enter>", lambda e: self._set_hover(True))
            w.bind("<Leave>", self._on_leave)
        self.refresh()
        self._paint()

    def refresh(self):
        self.title.configure(text=Path(self.name).stem)
        thumb = load_thumb(importer.picture_path(SOUNDS / self.name))
        if thumb:
            self.pic.configure(image=thumb)
            self.pic.grid(row=0, column=0, rowspan=2, sticky="nw", padx=(12, 10), pady=(12, 0))
            self.title.configure(wraplength=TILE_WIDTH - 50 - THUMB)
            self.title.grid_configure(padx=(0, 12))
            self.meta.grid_configure(padx=(0, 0))
        else:
            self.pic.grid_remove()
            self.title.configure(wraplength=TILE_WIDTH - 40)
            self.title.grid_configure(padx=(14, 12))
            self.meta.grid_configure(padx=(14, 0))
        secs = len(self.app.clips[self.name]) / engine.SR
        self.meta.configure(text=f"{secs:.0f}s" if secs >= 10 else f"{secs:.1f}s")
        key = pretty_key(self.app.cfg["hotkeys"].get(self.name))
        self.chip.configure(text=f" {key} " if key else "")
        self.chip.configure(fg_color=BORDER if key else "transparent")

    def _on_leave(self, e):
        under = self.winfo_containing(e.x_root, e.y_root)
        if under is None or not str(under).startswith(str(self)):
            self._set_hover(False)

    def _set_hover(self, on):
        if on != self.hover:
            self.hover = on
            self._paint()

    def set_progress(self, frac):
        playing = frac is not None
        if not playing and not self.playing:
            return   # idle: nothing to redraw
        self.bar.set(frac or 0)
        if playing != self.playing:
            self.playing = playing
            self._paint()

    def _paint(self):
        color = self.app.group_color(group_of(self.name))
        if color:   # tinted with its group's colour
            if self.playing:
                fg, border = mix(CARD, color, 0.38), color
            else:
                fg, border = mix(CARD, color, 0.24 if self.hover else 0.14), mix(BORDER, color, 0.45)
            accent = mix(color, "#ffffff", 0.2)
        else:
            if self.playing:
                fg, border = CARD_PLAYING, ACCENT
            else:
                fg, border = (CARD_HOVER if self.hover else CARD), BORDER
            accent = ACCENT
        self.configure(fg_color=fg, border_color=border)
        # an idle bar still draws a dot at 0%, so colour it away
        self.bar.configure(fg_color=fg, progress_color=accent if self.playing else fg)
        if not self.chip.cget("text"):
            self.chip.configure(fg_color="transparent")


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
        self.placed = {}      # filename -> (row, col)
        self.columns = 0
        self.query = None
        self.built = False
        self.dirty = True
        self.msg = ctk.CTkLabel(self.frame, text="", font=font(13), text_color=MUTED)
        self.empty = None
        if key == ALL:   # first-run screen
            self.empty = ctk.CTkFrame(self.frame, fg_color="transparent")
            ctk.CTkLabel(self.empty, text="No sounds yet", font=font(18, "bold"), text_color=TEXT).pack(
                pady=(70, 4))
            ctk.CTkLabel(self.empty, text="Paste an Instagram or YouTube link to make your first button.",
                         font=font(13), text_color=MUTED).pack(pady=(0, 14))
            button(self.empty, "Import link", app.import_dialog, "primary", width=150, height=38,
                   image=plus_image(ACCENT_TEXT), compound="left").pack()

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
                tile = Tile(self.frame, self.app, name)
            else:
                if tile.name != name:
                    if tile.name in self.placed:
                        self.placed[name] = self.placed.pop(tile.name)
                    tile.name = name
                tile.refresh()
                tile._paint()
            self.tiles[name] = tile
        for name, t in old.items():
            self.placed.pop(name, None)
            t.destroy()
        self.built = self.dirty = True

    def layout(self, cols, query=""):
        if not self.dirty and cols == self.columns and query == self.query:
            return
        self.dirty = False
        for c in range(max(cols, self.columns)):
            self.frame.columnconfigure(c, weight=1 if c < cols else 0, uniform="tiles" if c < cols else "")
        self.columns, self.query = cols, query
        shown = [n for n in self.tiles if query in Path(n).stem.lower()]
        self.msg.grid_forget()
        if self.empty is not None:
            self.empty.grid_forget()
        if not shown:
            if self.empty is not None and not self.tiles:
                self.empty.grid(row=0, column=0, columnspan=cols)
            else:
                self.msg.configure(text="No sounds match your search." if query else
                                   "This group is empty. Right-click a sound > Move to group, "
                                   "or import straight into it.")
                self.msg.grid(row=0, column=0, columnspan=cols, pady=60)
        # only touch tiles whose spot changed
        want = {name: (i // cols, i % cols) for i, name in enumerate(shown)}
        for name, t in self.tiles.items():
            pos = want.get(name)
            if pos is None:
                if name in self.placed:
                    t.grid_forget()
            elif self.placed.get(name) != pos:
                t.grid(row=pos[0], column=pos[1], sticky="ew", padx=6, pady=6)
        self.placed = want

    def raise_(self):
        self.frame._parent_frame.lift()

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
        self.columns = 0
        SOUNDS.mkdir(exist_ok=True)

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
        root.bind("<Configure>", lambda e: e.widget is root and self.open_menus and self.close_menus(), add="+")
        self._set_icon(root)
        self._build_ui()
        self.reload_clips()
        self.start_audio()
        self._poll_playing()

    def _set_icon(self, win):
        icon = ASSETS / "icon.ico"
        if icon.exists():
            # CustomTkinter puts its own icon on new windows after ~200 ms; set ours after that
            win.after(250, lambda: win.iconbitmap(str(icon)))

    # ---------- UI ----------
    def _build_ui(self):
        root = self.root
        root.columnconfigure(0, weight=1)
        root.rowconfigure(4, weight=1)

        # header
        head = ctk.CTkFrame(root, fg_color="transparent")
        head.grid(row=0, column=0, sticky="ew", padx=24, pady=(20, 6))
        head.columnconfigure(2, weight=1)
        png = ASSETS / "icon.png"
        if png.exists():
            img = ctk.CTkImage(Image.open(png), size=(34, 34))
            ctk.CTkLabel(head, image=img, text="").grid(row=0, column=0, rowspan=2, padx=(0, 12))
        ctk.CTkLabel(head, text="JondisBoard", font=font(22, "bold"), text_color=TEXT,
                     height=26, anchor="w").grid(row=0, column=1, sticky="w")
        self.count_label = ctk.CTkLabel(head, text="", font=font(12), text_color=MUTED, height=16, anchor="w")
        self.count_label.grid(row=1, column=1, sticky="w")
        self.status_pill = ctk.CTkLabel(head, text="", font=font(12, "bold"), corner_radius=12,
                                        fg_color=CARD, height=28)
        self.status_pill.grid(row=0, column=3, rowspan=2, padx=(0, 8))
        self.theme_btn = button(head, "Theme", None, "secondary", width=86, image=dot_image(ACCENT, 12),
                                compound="left")
        self.theme_btn.configure(command=self.theme_menu)
        self.theme_btn.grid(row=0, column=4, rowspan=2, padx=(0, 8))
        self.settings_btn = button(head, "Audio setup", self.toggle_settings, "secondary", width=110)
        self.settings_btn.grid(row=0, column=5, rowspan=2)

        # audio setup (collapsible)
        self.settings = ctk.CTkFrame(root, fg_color=SURFACE, corner_radius=14, border_width=1, border_color=BORDER)
        self.settings.columnconfigure(1, weight=1)
        self.mic_var = tk.StringVar(value=self.cfg["mic"] or "")
        self.cable_var = tk.StringVar(value=self.cfg["cable"] or "")
        self.monitor_var = tk.StringVar(value=self.cfg["monitor"] or "")
        rows = [("Your microphone", self.mic_var, "input"),
                ("Send to (virtual cable)", self.cable_var, "output"),
                ("Hear clips on", self.monitor_var, "output")]
        self.menus = []
        for r, (label, var, kind) in enumerate(rows):
            ctk.CTkLabel(self.settings, text=label, font=font(13), text_color=MUTED, anchor="w").grid(
                row=r, column=0, sticky="w", padx=(18, 12), pady=(14 if r == 0 else 6, 14 if r == 2 else 0))
            menu = ctk.CTkOptionMenu(self.settings, variable=var, values=[""], height=34, corner_radius=10,
                                     dynamic_resizing=False, font=font(13), dropdown_font=font(13),
                                     fg_color=CARD, button_color=CARD, button_hover_color=CARD_HOVER,
                                     text_color=TEXT, dropdown_fg_color=CARD, dropdown_hover_color=CARD_HOVER,
                                     dropdown_text_color=TEXT, command=lambda _: self.start_audio())
            menu.kind = kind
            menu.grid(row=r, column=1, sticky="ew", pady=(14 if r == 0 else 6, 14 if r == 2 else 0))
            self.menus.append(menu)
        button(self.settings, "Restart audio", self.start_audio, "secondary", width=120).grid(
            row=0, column=2, rowspan=3, padx=18)
        # first-run help for people who got the app shared and have no virtual cable yet
        self.cable_help = ctk.CTkFrame(self.settings, fg_color=CARD, corner_radius=10)
        self.cable_help.columnconfigure(0, weight=1)
        ctk.CTkLabel(self.cable_help, text="VB-CABLE isn't installed. It's the free virtual mic that carries "
                     "the sounds into games. Install it, restart your PC, then open the soundboard again.",
                     font=font(12), text_color=TEXT, justify="left", wraplength=520, anchor="w").grid(
            row=0, column=0, sticky="w", padx=14, pady=10)
        button(self.cable_help, "Get VB-CABLE", lambda: os.startfile("https://vb-audio.com/Cable/"),
               "primary", width=120).grid(row=0, column=1, padx=12)
        self.settings_open = False

        # toolbar
        bar = ctk.CTkFrame(root, fg_color="transparent")
        bar.grid(row=2, column=0, sticky="ew", padx=24, pady=(10, 8))
        bar.columnconfigure(4, weight=1)
        button(bar, "Import link", self.import_dialog, "primary", width=134,
               image=plus_image(ACCENT_TEXT), compound="left").grid(row=0, column=0, padx=(0, 8))
        button(bar, "Add files", self.add_files, "secondary", width=96).grid(row=0, column=1, padx=(0, 4))
        button(bar, "Open folder", lambda: os.startfile(SOUNDS), "ghost", width=96).grid(row=0, column=2)
        button(bar, "Reload", self.reload_clips, "ghost", width=70).grid(row=0, column=3)
        # no textvariable: CTkEntry hides its placeholder when one is set
        self.search = ctk.CTkEntry(bar, placeholder_text="Search sounds", width=220, height=34,
                                   corner_radius=10, fg_color=CARD, border_color=BORDER, text_color=TEXT,
                                   placeholder_text_color=MUTED, font=font(13))
        self.search.grid(row=0, column=5, sticky="e")
        self.search.bind("<KeyRelease>", lambda e: self._search_changed())

        # group tabs, filled in by _build_group_bar
        self.group_bar = ctk.CTkFrame(root, fg_color="transparent")
        self.group_bar.grid(row=3, column=0, sticky="ew", padx=24, pady=(0, 4))

        # sound grid
        holder = ctk.CTkFrame(root, fg_color="transparent")
        holder.grid(row=4, column=0, sticky="nsew", padx=(16, 10))
        holder.rowconfigure(0, weight=1)
        holder.columnconfigure(0, weight=1)
        holder.bind("<Configure>", lambda e: self._on_resize())
        self.holder = holder   # pages (one per group) are stacked in here

        # bottom bar
        foot = ctk.CTkFrame(root, fg_color=SURFACE, corner_radius=0, border_width=0)
        foot.grid(row=5, column=0, sticky="ew")
        inner = ctk.CTkFrame(foot, fg_color="transparent")
        inner.pack(fill="x", padx=24, pady=14)
        self.vol_labels = {}
        col = 0
        for key, label, top in [("mic_volume", "Mic", 1.5), ("clip_volume", "Clips", 1.5),
                                ("monitor_volume", "In your ears", 1.0)]:
            box = ctk.CTkFrame(inner, fg_color="transparent")
            box.grid(row=0, column=col, sticky="ew", padx=(0, 22))
            inner.columnconfigure(col, weight=1)
            box.columnconfigure(0, weight=1)
            ctk.CTkLabel(box, text=label, font=font(12), text_color=MUTED, height=16, anchor="w").grid(
                row=0, column=0, sticky="w")
            self.vol_labels[key] = ctk.CTkLabel(box, text="", font=font(12, "bold"), text_color=TEXT,
                                                height=16, anchor="e")
            self.vol_labels[key].grid(row=0, column=1, sticky="e")
            s = ctk.CTkSlider(box, from_=0, to=top, height=16, button_color=TEXT, button_hover_color="white",
                              progress_color=ACCENT, fg_color=BORDER,
                              command=lambda v, k=key: self._set_volume(k, float(v)))
            s.set(self.cfg[key])
            s.grid(row=1, column=0, columnspan=2, sticky="ew", pady=(4, 0))
            self._set_volume(key, self.cfg[key])
            col += 1
        self.pass_var = tk.BooleanVar(value=self.cfg["mic_passthrough"])
        ctk.CTkSwitch(inner, text="Send my voice", variable=self.pass_var, command=self._set_passthrough,
                      font=font(13), text_color=TEXT, progress_color=ACCENT, fg_color=BORDER,
                      button_color=TEXT, button_hover_color="white").grid(row=0, column=col, padx=(0, 18))
        self._set_passthrough()
        stop = ctk.CTkFrame(inner, fg_color="transparent")
        stop.grid(row=0, column=col + 1)
        self.stop_btn = button(stop, "Stop all", self.engine.stop_all, "danger", width=120, height=36)
        self.stop_btn.pack()
        self.stop_key_btn = ctk.CTkButton(stop, text="", command=self.set_stop_hotkey, fg_color="transparent",
                                          hover_color=CARD, text_color=MUTED, font=font(11), height=18,
                                          width=120, corner_radius=6)
        self.stop_key_btn.pack(pady=(4, 0))

    def toggle_settings(self, show=None):
        self.settings_open = (not self.settings_open) if show is None else show
        if self.settings_open:
            self.settings.grid(row=1, column=0, sticky="ew", padx=24, pady=(10, 0))
        else:
            self.settings.grid_forget()
        self.settings_btn.configure(fg_color=CARD_HOVER if self.settings_open else CARD)

    def set_status(self, text, level="good"):
        color = {"good": GOOD, "warn": WARN, "bad": DANGER}[level]
        self.root.after(0, lambda: self.status_pill.configure(text=f"  ●  {text}  ", text_color=color))

    def toast(self, text):
        """Short message under the header that fades away."""
        if getattr(self, "_toast", None):
            self._toast.destroy()
        self._toast = ctk.CTkLabel(self.root, text=f"  {text}  ", font=font(12), fg_color=CARD,
                                   text_color=TEXT, corner_radius=10, height=30)
        self._toast.place(relx=0.5, rely=1.0, y=-96, anchor="s")
        t = self._toast
        self.root.after(4000, lambda: t.destroy() if t.winfo_exists() else None)

    # ---------- audio ----------
    def refresh_devices(self):
        for m in self.menus:
            m.configure(values=engine.list_devices(m.kind) or [""])
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
            self.toast(f"Audio error: {ex}")
            self.toggle_settings(True)
            return
        has_cable = bool(engine.find_cable_output_name())
        if has_cable:
            self.cable_help.grid_forget()
        else:
            self.cable_help.grid(row=3, column=0, columnspan=3, sticky="ew", padx=14, pady=(0, 14))
        if not has_cable:
            self.set_status("VB-CABLE not installed", "bad")
            self.toggle_settings(True)
        elif warnings:
            self.set_status("Check audio setup", "warn")
            self.toast(warnings[0])
            self.toggle_settings(True)
        else:
            self.set_status("Live", "good")

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
            self.toast("Couldn't load: " + "; ".join(errors))

    # ---------- groups ----------
    def _build_group_bar(self):
        counts = {}
        for n in self.clips:
            counts[group_of(n)] = counts.get(group_of(n), 0) + 1
        tabs = [(ALL, "All", len(self.clips))] + [(g, g, counts.get(g, 0)) for g in self.groups]
        if self.groups and counts.get(NO_GROUP):
            tabs.append((NO_GROUP, "No group", counts[NO_GROUP]))
        same = list(self._tabs) == [k for k, _, _ in tabs]
        if not same:   # tabs added/removed: rebuild; otherwise restyle the buttons in place
            for w in self.group_bar.winfo_children():
                w.destroy()
            self._tabs = {}
        for key, label, n in tabs:
            on = key == self.cfg["group"]
            color = self.group_color(key) or ACCENT
            if on:
                fg, hover, border = color, mix(color, "#000000", 0.15), color
                text = ACCENT_TEXT if color == ACCENT else "white"
            elif key in (ALL, NO_GROUP):
                fg, hover, border, text = CARD, CARD_HOVER, BORDER, TEXT
            else:
                fg, hover, border, text = mix(CARD, color, 0.14), mix(CARD, color, 0.26), mix(BORDER, color, 0.5), TEXT
            look = (f"{label}   {n}", on, fg, hover, text, border)
            style = dict(text=look[0], font=font(12, "bold" if on else "normal"), fg_color=fg,
                         hover_color=hover, text_color=text, border_color=border)
            if same:
                b = self._tabs[key]
                if b.look != look:   # restyling a CTkButton redraws it, so skip unchanged tabs
                    b.configure(**style)
                    b.look = look
                continue
            b = ctk.CTkButton(self.group_bar, width=20, height=30, corner_radius=15, border_width=1,
                              command=lambda k=key: self.show_group(k), **style)
            b.pack(side="left", padx=(0, 6), pady=2)
            if key not in (ALL, NO_GROUP):
                b.bind("<Button-3>", lambda e, g=key: self.group_menu(e, g))
            b.look = look
            self._tabs[key] = b
        if not same:
            button(self.group_bar, "New group", self.new_group, "ghost", width=20, height=30,
                   image=plus_image(MUTED, 10), compound="left").pack(side="left")

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
        page.raise_()
        self.root.update_idletasks()

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
            t._paint()

    def close_menus(self):
        for m in list(self.open_menus):
            m._destroy()
        self.active_menu = None

    def _popup(self, event):
        self.close_menus()
        self.active_menu = PopupMenu(self)
        self._menu_serial = getattr(event, "serial", None)
        return self.active_menu

    def _click_anywhere(self, event):
        """Clicking outside an open menu closes it (tiles handle their own clicks first)."""
        if self.open_menus and event.serial != self._menu_serial and \
                not self.open_menus[0].contains(event.widget):
            self.close_menus()

    def tile_click(self, name):
        # a click that only closes an open menu shouldn't also fire a sound into the game
        if self.open_menus:
            self.close_menus()
            return
        self.play(name)

    def group_menu(self, event, group):
        m = self._popup(event)
        m.header(group)
        m.item("Rename group...", lambda: self.rename_group(group))
        m.separator()
        swatches = ctk.CTkFrame(m.inner, fg_color="transparent")
        swatches.pack(fill="x", padx=8, pady=(2, 4))
        current = self.group_color(group)
        for i, c in enumerate(GROUP_COLORS):
            ctk.CTkButton(swatches, text="", width=26, height=26, corner_radius=13,
                          fg_color=c, hover_color=mix(c, "#ffffff", 0.2),
                          border_width=3 if c == current else 0, border_color="white",
                          command=lambda c=c: (m.close_all(), self.set_group_color(group, c))).grid(
                row=0, column=i, padx=(0, 0 if i == len(GROUP_COLORS) - 1 else 6))
        m.separator()
        m.item("Delete group", lambda: self.delete_group(group), danger=True)
        m.show(event.x_root, event.y_root)

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
                self.toast(f"There's already a group called {new}")
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

    def move_clip(self, name, group):
        self._move_clip_files(name, SOUNDS / group if group else SOUNDS)
        self.reload_clips()

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
            self.toast(f"Couldn't use that picture: {ex}")
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
        data = self.clips.get(name)
        if data is not None:
            self.engine.play(name, data)

    def _poll_playing(self):
        progress = self.engine.progress()
        for tile in self.all_tiles():
            tile.set_progress(progress.get(tile.name))
        self._poll_id = self.root.after(50, self._poll_playing)

    def clip_menu(self, event, name):
        m = self._popup(event)
        current = group_of(name)
        m.header(Path(name).stem)
        m.item("Play", lambda: self.play(name))
        key = pretty_key(self.cfg["hotkeys"].get(name))
        m.item(f"Change hotkey  ({key})..." if key else "Set hotkey...", lambda: self.set_clip_hotkey(name))
        if key:
            m.item("Clear hotkey", lambda: self._assign(name, None))
        m.separator()

        def groups(sub):
            for g in self.groups:
                sub.item(g + ("   ✓" if g == current else ""), lambda g=g: self.move_clip(name, g),
                         dot=self.group_color(g), enabled=g != current)
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
    def _dialog(self, title, width, height):
        """A popup that appears in one smooth fade. It stays hidden while customtkinter recolours
        its title bar (a hide/show flash) and while the caller fills it in, then fades in."""
        win = ctk.CTkToplevel(self.root, fg_color=BG)
        # customtkinter already made the title bar dark while creating the window. Stop it doing
        # that again: resizable() below would schedule another hide/show, which can land after our
        # reveal and leave an invisible popup holding the click grab, so the whole app looks frozen.
        win._deactivate_windows_window_header_manipulation = True
        win.withdraw()   # also stops customtkinter re-showing it after its first title bar trick
        win.title(title)
        win.resizable(False, False)
        win.transient(self.root)
        icon = ASSETS / "icon.ico"
        if icon.exists():
            win.iconbitmap(str(icon))   # now, so customtkinter doesn't swap its own icon in later
        self.root.update_idletasks()
        scale = self.root._get_window_scaling()
        x = self.root.winfo_rootx() + (self.root.winfo_width() - int(width * scale)) // 2
        y = self.root.winfo_rooty() + (self.root.winfo_height() - int(height * scale)) // 3
        win.geometry(f"{width}x{height}+{x}+{y}")

        def reveal():
            if not win.winfo_exists():
                return
            win.attributes("-alpha", 0.0)
            win.deiconify()
            win.lift()
            win.focus_force()
            win.update_idletasks()
            win.grab_set()
            win.after(16, lambda: fade(0))   # one frame for the widgets to draw, still invisible
            win.after(400, make_sure_visible)

        def make_sure_visible():
            # never leave a hidden popup holding the click grab
            if win.winfo_exists() and not win.winfo_viewable():
                log.warning("dialog %r was hidden after showing; showing it again", title)
                win.deiconify()
                win.lift()
                win.attributes("-alpha", 1.0)

        def fade(i, frames=7):
            if win.winfo_exists():
                win.attributes("-alpha", 1 - (1 - (i + 1) / frames) ** 3)
                if i + 1 < frames:
                    win.after(14, lambda: fade(i + 1))

        win.after(30, reveal)   # the caller builds the contents first
        return win

    def _confirm(self, title, text, ok_text, on_ok, kind="danger"):
        win = self._dialog(title, 380, 170)
        ctk.CTkLabel(win, text=title, font=font(16, "bold"), text_color=TEXT).pack(anchor="w", padx=22, pady=(20, 2))
        ctk.CTkLabel(win, text=text, font=font(13), text_color=MUTED, justify="left",
                     wraplength=330).pack(anchor="w", padx=22)
        row = ctk.CTkFrame(win, fg_color="transparent")
        row.pack(side="bottom", fill="x", padx=22, pady=18)
        button(row, ok_text, lambda: (win.destroy(), on_ok()), kind, width=100).pack(side="right")
        button(row, "Cancel", win.destroy, "ghost", width=80).pack(side="right", padx=8)

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
        win.after(150, lambda: (entry.focus_set(), entry.select_range(0, "end")))

        def ok(*_):
            text = entry.get().strip()
            win.destroy()
            if text:
                on_ok(text)

        entry.bind("<Return>", ok)
        row = ctk.CTkFrame(win, fg_color="transparent")
        row.pack(side="bottom", fill="x", padx=22, pady=18)
        button(row, ok_text, ok, "primary", width=90).pack(side="right")
        button(row, "Cancel", win.destroy, "ghost", width=80).pack(side="right", padx=8)

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
                if win.winfo_exists():
                    win.destroy()
                if binding:
                    on_binding(binding)
            self.root.after(0, finish)

        win.protocol("WM_DELETE_WINDOW", lambda: (self.hotkeys.cancel_record(), win.destroy()))
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
        win = self._dialog("Import", 480, 396)
        ctk.CTkLabel(win, text="Import a sound", font=font(18, "bold"), text_color=TEXT).pack(
            anchor="w", padx=24, pady=(22, 2))
        ctk.CTkLabel(win, text="Paste an Instagram or YouTube link. Times are optional.", font=font(12),
                     text_color=MUTED).pack(anchor="w", padx=24, pady=(0, 12))

        def field(parent, placeholder, width=None):
            e = ctk.CTkEntry(parent, placeholder_text=placeholder, height=36, corner_radius=10, fg_color=CARD,
                             border_color=BORDER, text_color=TEXT, font=font(13),
                             **({"width": width} if width else {}))
            return e

        url = field(win, "Instagram or YouTube link")
        url.pack(fill="x", padx=24)
        try:
            clip = self.root.clipboard_get().strip()
            if any(site in clip for site in ("instagram.com", "youtube.com", "youtu.be")):
                url.insert(0, clip)
        except tk.TclError:
            pass
        name = field(win, "Name (leave empty to use the title)")
        name.pack(fill="x", padx=24, pady=(10, 0))
        times = ctk.CTkFrame(win, fg_color="transparent")
        times.pack(fill="x", padx=24, pady=(10, 0))
        times.columnconfigure((0, 1), weight=1)
        start = field(times, "Start, e.g. 0:03.5")
        start.grid(row=0, column=0, sticky="ew", padx=(0, 5))
        end = field(times, "End, e.g. 0:07")
        end.grid(row=0, column=1, sticky="ew", padx=(5, 0))
        no_group = "No group"
        g = self.cfg["group"]
        group_var = tk.StringVar(value=g if g not in (ALL, NO_GROUP) else no_group)
        ctk.CTkOptionMenu(win, variable=group_var, values=[no_group] + self.groups, height=36,
                          corner_radius=10, dynamic_resizing=False, font=font(13), dropdown_font=font(13),
                          fg_color=CARD, button_color=CARD, button_hover_color=CARD_HOVER, text_color=TEXT,
                          dropdown_fg_color=CARD, dropdown_hover_color=CARD_HOVER,
                          dropdown_text_color=TEXT).pack(fill="x", padx=24, pady=(10, 0))
        norm = tk.BooleanVar(value=True)
        ctk.CTkSwitch(win, text="Even out loudness", variable=norm, font=font(13), text_color=TEXT,
                      progress_color=ACCENT, fg_color=BORDER, button_color=TEXT).pack(anchor="w", padx=24, pady=12)
        msg = ctk.CTkLabel(win, text="", font=font(12), text_color=MUTED, wraplength=430, justify="left")
        msg.pack(anchor="w", padx=24)
        spinner = ctk.CTkProgressBar(win, mode="indeterminate", height=3, progress_color=ACCENT, fg_color=BG)
        row = ctk.CTkFrame(win, fg_color="transparent")
        row.pack(side="bottom", fill="x", padx=24, pady=20)
        go = button(row, "Import", None, "primary", width=110)
        go.pack(side="right")
        button(row, "Cancel", win.destroy, "ghost", width=80).pack(side="right", padx=8)

        def ui(text, color=MUTED):
            self.root.after(0, lambda: msg.configure(text=text, text_color=color) if win.winfo_exists() else None)

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
                        msg.configure(text="Failed: " + text, text_color=DANGER)
                self.root.after(0, fail)
                return

            def done():
                self.reload_clips()
                self.toast(f"Added '{out.stem}'")
                if win.winfo_exists():
                    win.destroy()
            self.root.after(0, done)

        def begin(*_):
            v = {"url": url.get().strip(), "name": name.get().strip(),
                 "start": start.get().strip(), "end": end.get().strip(),
                 "folder": SOUNDS if group_var.get() == no_group else SOUNDS / group_var.get()}
            if not v["url"]:
                msg.configure(text="Paste a reel link first.", text_color=WARN)
                return
            try:
                importer.parse_time(v["start"])
                importer.parse_time(v["end"])
            except ValueError:
                msg.configure(text="Times look like 7, 0:07 or 0:07.5", text_color=WARN)
                return
            go.configure(state="disabled")
            spinner.pack(fill="x", padx=24, pady=(8, 0), before=row)
            spinner.start()
            threading.Thread(target=worker, args=(v,), daemon=True).start()

        go.configure(command=begin)
        url.bind("<Return>", begin)
        win.after(150, url.focus_set)

    # ---------- misc ----------
    def save_config(self):
        try:
            CONFIG.write_text(json.dumps(self.cfg, indent=2), encoding="utf-8")
        except Exception:
            log.exception("could not save config")

    # ---------- themes ----------
    def theme_menu(self):
        # open after this click is fully handled, or the click-outside check closes it at once
        self.root.after(1, self._theme_menu)

    def _theme_menu(self):
        b = self.theme_btn
        m = self._popup(None)
        m.header("Theme")
        for name, t in THEMES.items():
            on = name == self.cfg.get("theme")
            m.item(name + ("   ✓" if on else ""), lambda n=name: self.switch_theme(n), dot=t["ACCENT"], enabled=not on)
        m.show(b.winfo_rootx() + b.winfo_width() - 232, b.winfo_rooty() + b.winfo_height() + 6)

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
        if getattr(self, "_poll_id", None):
            self.root.after_cancel(self._poll_id)   # don't let it fire into the next window
        self.hotkeys.close()
        self.engine.stop()
        self.root.destroy()


def main():
    ctk.set_appearance_mode("dark")
    while True:
        root = ctk.CTk()
        root.report_callback_exception = lambda *a: log.error("UI error", exc_info=a)
        app = App(root)
        root.mainloop()
        if not app.restart:
            break


if __name__ == "__main__":
    main()
