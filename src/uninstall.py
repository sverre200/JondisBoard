r"""Uninstall JondisBoard: remove the app's files and shortcuts from this computer.

Built as "Uninstall JondisBoard.exe", which sits next to JondisBoard.exe. It only removes files it
knows are JondisBoard's, never a whole folder (the install folder can be a code checkout too), sends
your sounds and settings to the Recycle Bin only if you tick the box, and leaves VB-CABLE alone (a
separate driver other apps can use)."""
import ctypes
import json
import os
import subprocess
import sys
import tempfile
import uuid
from ctypes import wintypes
from pathlib import Path

FROZEN = getattr(sys, "frozen", False)
APP_DIR = Path(sys.executable).resolve().parent if FROZEN else Path(__file__).resolve().parent.parent
APP_EXE = "JondisBoard.exe"
UNINSTALLER = "Uninstall JondisBoard.exe"
# the app's own files next to the exe (READ ME FIRST.txt and ffmpeg.exe come with the share zips)
PROGRAM_FILES = (APP_EXE, "JondisBoard.update.exe", "ffmpeg.exe", "soundboard.log", "READ ME FIRST.txt")
DATA = ("sounds", "config.json")   # yours: only with the box ticked, and to the Recycle Bin
NO_WINDOW = 0x08000000


# ---------- finding what's there ----------
def _known_folder(guid):
    class GUID(ctypes.Structure):
        _fields_ = [("a", ctypes.c_uint32), ("b", ctypes.c_uint16), ("c", ctypes.c_uint16), ("d", ctypes.c_ubyte * 8)]
    u = uuid.UUID(guid)
    g = GUID(u.time_low, u.time_mid, u.time_hi_version, (ctypes.c_ubyte * 8)(*u.bytes[8:]))
    out = ctypes.c_wchar_p()
    try:
        if ctypes.windll.shell32.SHGetKnownFolderPath(ctypes.byref(g), 0, None, ctypes.byref(out)) == 0:
            return Path(out.value)
    except OSError:
        pass
    finally:
        ctypes.windll.ole32.CoTaskMemFree(out)
    return None


def shortcut_folders():
    """(folders to look in directly, folders to look through with their subfolders) for shortcuts:
    your desktop and the shared one, the Start menu (yours and everyone's), taskbar pins."""
    desktop = _known_folder("B4BFCC3A-DB2C-424C-B029-7FE99A87C641")          # (follows OneDrive)
    public_desktop = _known_folder("C4AA340D-F20F-4863-AFEF-F87EF2E6BA25")
    programs = _known_folder("A77F5D77-2E2B-44C3-A6A2-ABA601054A51")
    common_programs = _known_folder("0139D44E-6AFE-49F2-8690-3DAFCAE6FFB8")
    pins = Path(os.environ.get("APPDATA", "")) / "Microsoft" / "Internet Explorer" / "Quick Launch" / "User Pinned" / "TaskBar"
    return [d for d in (desktop, public_desktop, pins) if d], [d for d in (programs, common_programs) if d]


def shortcut_targets(flat, deep):
    """[(shortcut, what it opens)] for every .lnk in those folders (Windows' own shortcut reader)."""
    script = (
        "$sh = New-Object -ComObject WScript.Shell; "
        "$d = $env:JB_DIRS | ConvertFrom-Json; "
        "$files = @(); "
        "foreach ($f in $d.flat) { if (Test-Path -LiteralPath $f) { $files += Get-ChildItem -LiteralPath $f -Filter *.lnk -ErrorAction SilentlyContinue } }; "
        "foreach ($f in $d.deep) { if (Test-Path -LiteralPath $f) { $files += Get-ChildItem -LiteralPath $f -Filter *.lnk -Recurse -ErrorAction SilentlyContinue } }; "
        "foreach ($l in $files) { try { $l.FullName + '|' + $sh.CreateShortcut($l.FullName).TargetPath } catch {} }")
    env = {**os.environ, "JB_DIRS": json.dumps({"flat": [str(p) for p in flat], "deep": [str(p) for p in deep]})}
    try:
        out = subprocess.run(["powershell", "-NoProfile", "-NonInteractive", "-Command", script], capture_output=True,
                             text=True, encoding="utf-8", errors="replace", env=env, timeout=60,
                             creationflags=NO_WINDOW).stdout
    except (OSError, subprocess.SubprocessError):
        return []
    pairs = []
    for line in out.splitlines():
        if "|" in line:
            lnk, target = line.split("|", 1)
            pairs.append((Path(lnk.strip()), target.strip()))
    return pairs


def _same(a, b):
    return os.path.normcase(os.path.normpath(str(a))) == os.path.normcase(os.path.normpath(str(b)))


def icon_cache():
    return Path(tempfile.gettempdir()) / "JondisBoard"   # the themed window icons (see app.theme_icon)


def find(folder=APP_DIR, folders=None, cache=None):
    """What there is to remove: {"program": [...], "data": [...], "shortcuts": [...], "cache": path or None}."""
    folder = Path(folder)
    program = [folder / n for n in PROGRAM_FILES if (folder / n).exists()]
    data = [folder / n for n in DATA if (folder / n).exists()]
    flat, deep = folders if folders is not None else shortcut_folders()
    ours = (folder / APP_EXE, folder / UNINSTALLER)
    shortcuts = [lnk for lnk, target in shortcut_targets(flat, deep) if any(_same(target, o) for o in ours)]
    cache = icon_cache() if cache is None else Path(cache)
    return {"program": program, "data": data, "shortcuts": shortcuts, "cache": cache if cache.exists() else None}


def count_sounds(folder=APP_DIR):
    sounds = Path(folder) / "sounds"
    exts = {".wav", ".mp3", ".ogg", ".flac", ".m4a", ".mp4", ".aac", ".opus", ".webm"}
    if not sounds.is_dir():
        return 0
    return sum(1 for p in sounds.rglob("*") if p.suffix.lower() in exts and "_deleted" not in p.parts)


def app_running():
    try:
        out = subprocess.run(["tasklist", "/FI", f"IMAGENAME eq {APP_EXE}", "/NH"], capture_output=True, text=True,
                             timeout=15, creationflags=NO_WINDOW).stdout
    except (OSError, subprocess.SubprocessError):
        return False
    return APP_EXE.lower() in out.lower()


# ---------- removing ----------
def recycle_bin(paths):
    """Send files / folders to the Recycle Bin (Windows' own delete, undoable). Returns the ones it couldn't."""
    if not paths:
        return []

    class SHFILEOPSTRUCTW(ctypes.Structure):
        _fields_ = [("hwnd", wintypes.HWND), ("wFunc", wintypes.UINT), ("pFrom", wintypes.LPCWSTR),
                    ("pTo", wintypes.LPCWSTR), ("fFlags", ctypes.c_ushort), ("fAnyOperationsAborted", wintypes.BOOL),
                    ("hNameMappings", ctypes.c_void_p), ("lpszProgressTitle", wintypes.LPCWSTR)]
    op = SHFILEOPSTRUCTW(None, 3, "\0".join(str(p) for p in paths) + "\0\0", None,
                         0x0040 | 0x0010 | 0x0004 | 0x0400)   # FO_DELETE; ALLOWUNDO | NOCONFIRMATION | SILENT | NOERRORUI
    ctypes.windll.shell32.SHFileOperationW(ctypes.byref(op))
    return [p for p in paths if Path(p).exists()]


def remove(found, with_data, recycle=recycle_bin):
    """Remove what find() found (data only with_data). Returns what's still there."""
    left = []
    for p in [*found["program"], *found["shortcuts"]]:
        try:
            Path(p).unlink()
        except OSError:
            left.append(p)
    if found["cache"] is not None:
        for f in Path(found["cache"]).glob("*.ico"):   # only our icons, then the folder if that empties it
            try:
                f.unlink()
            except OSError:
                left.append(f)
        try:
            Path(found["cache"]).rmdir()
        except OSError:
            pass
    if with_data:
        left += recycle(found["data"])
    return left


def remove_self_later(folder=APP_DIR):
    """Once this window has closed: delete the uninstaller itself, then the folder if that leaves it
    empty (rmdir refuses a folder with anything left in it, like your sounds or a code checkout)."""
    if not FROZEN:
        return
    exe = Path(sys.executable)
    cmd = f'ping -n 3 127.0.0.1 >nul & del /f /q "{exe}" & rmdir "{Path(folder)}"'
    # one string, as cmd expects it: a list would get Python's \" escaping, which cmd doesn't read
    subprocess.Popen(f'cmd /c "{cmd}"', creationflags=NO_WINDOW | 0x00000008, close_fds=True)   # DETACHED_PROCESS


# ---------- the window ----------
def describe(found, sounds):
    """What will go, in plain words (one line each)."""
    lines = []
    names = [p.name for p in found["program"]]
    if names:
        lines.append(", ".join(names))
    if found["shortcuts"]:
        n = len(found["shortcuts"])
        lines.append(f"{n} shortcut{'s' if n != 1 else ''} (desktop, Start menu or taskbar)")
    if found["cache"] is not None:
        lines.append("the themed icons it keeps in your temp folder")
    lines.append("this uninstaller")
    return lines


def main(folder=APP_DIR, folders=None, cache=None, recycle=recycle_bin, self_remove=True):
    import customtkinter as ctk
    from PIL import Image

    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import logo
    from themes import DANGER, DANGER_HOVER, THEMES

    try:
        theme = json.loads((Path(folder) / "config.json").read_text(encoding="utf-8")).get("theme")
    except (OSError, ValueError):
        theme = None
    t = THEMES.get(theme) or THEMES["Toxic Green"]
    ctk.set_appearance_mode("dark")
    root = ctk.CTk(fg_color=t["BG"])
    root.title("Uninstall JondisBoard")
    root.resizable(False, False)
    # the app's waveform logo and title bar colours (Windows otherwise paints the system accent)
    ico = Path(tempfile.gettempdir()) / "JondisBoard-uninstaller.ico"   # (not in the icon cache it removes)
    try:
        logo.save_ico(ico, logo.tile_color(t), t["ACCENT"])
        root.iconbitmap(str(ico))
        root.after(250, lambda: root.winfo_exists() and root.iconbitmap(str(ico)))   # (customtkinter sets its own at 200 ms)
        root.update_idletasks()
        hwnd = ctypes.windll.user32.GetParent(root.winfo_id())
        for attr, color in ((35, t["BG"]), (36, t["TEXT"]), (34, t["BORDER"])):   # caption, caption text, border
            r, gr, b = (int(color[i:i + 2], 16) for i in (1, 3, 5))
            c = ctypes.c_int(r | (gr << 8) | (b << 16))
            ctypes.windll.dwmapi.DwmSetWindowAttribute(hwnd, attr, ctypes.byref(c), ctypes.sizeof(c))
    except Exception:
        pass   # older Windows: the default title bar
    state = {"found": find(folder, folders, cache), "done": False}
    sounds = count_sounds(folder)

    def font(size, weight="normal"):
        return ctk.CTkFont(family="Segoe UI", size=size, weight=weight)

    body = ctk.CTkFrame(root, fg_color="transparent")
    body.pack(fill="both", expand=True, padx=26, pady=(22, 20))
    head = ctk.CTkFrame(body, fg_color="transparent")
    head.pack(fill="x")
    mark = logo.draw(40 * 3, logo.tile_color(t), t["ACCENT"])
    ctk.CTkLabel(head, image=ctk.CTkImage(mark, size=(40, 40)), text="").pack(side="left", padx=(0, 12))
    titles = ctk.CTkFrame(head, fg_color="transparent")
    titles.pack(side="left", fill="x")
    ctk.CTkLabel(titles, text="Uninstall JondisBoard", font=font(18, "bold"), text_color=t["TEXT"], anchor="w").pack(anchor="w")
    ctk.CTkLabel(titles, text=str(folder), font=font(12), text_color=t["MUTED"], anchor="w").pack(anchor="w")

    content = ctk.CTkFrame(body, fg_color="transparent")
    content.pack(fill="x", pady=(18, 0))
    keep_var = ctk.BooleanVar(value=False)
    status = ctk.CTkLabel(body, text="", font=font(12), text_color="#ffb547", anchor="w", justify="left", wraplength=400)
    buttons = ctk.CTkFrame(body, fg_color="transparent")
    buttons.pack(fill="x", side="bottom", pady=(18, 0))

    def button(parent, text, cmd, danger=False):
        return ctk.CTkButton(parent, text=text, command=cmd, height=36, corner_radius=10, width=110,
                             font=font(13, "bold" if danger else "normal"),
                             fg_color=DANGER if danger else "transparent", hover_color=DANGER_HOVER if danger else t["CARD"],
                             text_color="#ffffff" if danger else t["MUTED"])
    go = button(buttons, "Uninstall", lambda: uninstall(), danger=True)
    go.pack(side="right")
    cancel = button(buttons, "Cancel", root.destroy)
    cancel.pack(side="right", padx=(0, 8))

    def fit():
        root.update_idletasks()
        w, h = root.winfo_reqwidth(), root.winfo_reqheight()
        s = root._get_window_scaling()
        x = (root.winfo_screenwidth() - w) // 2
        y = (root.winfo_screenheight() - h) // 3
        root.geometry(f"{round(w / s)}x{round(h / s)}+{x}+{y}")

    def show_plan():
        for w in content.winfo_children():
            w.destroy()
        ctk.CTkLabel(content, text="This removes:", font=font(13, "bold"), text_color=t["TEXT"], anchor="w").pack(anchor="w")
        for line in describe(state["found"], sounds):
            ctk.CTkLabel(content, text=f"•  {line}", font=font(13), text_color=t["TEXT"], anchor="w", justify="left",
                         wraplength=400).pack(anchor="w", padx=(4, 0), pady=(3, 0))
        if state["found"]["data"]:
            what = f"your {sounds} sound{'s' if sounds != 1 else ''} and settings" if sounds else "your settings"
            ctk.CTkCheckBox(content, text=f"Also remove {what} (they go to the Recycle Bin)", variable=keep_var,
                            font=font(13), text_color=t["TEXT"], fg_color=t["ACCENT"], hover_color=t["ACCENT_HOVER"],
                            border_color=t["BORDER"], checkmark_color=t["ACCENT_TEXT"], corner_radius=6).pack(
                anchor="w", pady=(14, 0))
            ctk.CTkLabel(content, text="Leave it unticked to keep them in this folder.", font=font(12),
                         text_color=t["MUTED"], anchor="w").pack(anchor="w", padx=(30, 0))
        ctk.CTkLabel(content, text="VB-CABLE stays installed: it's a separate driver. Remove it in Windows Settings → "
                     "Apps if nothing else needs it.", font=font(12), text_color=t["MUTED"], anchor="w",
                     justify="left", wraplength=400).pack(anchor="w", pady=(14, 0))

    def watch_app():
        """The app can't be removed while it's open: say so, and wait for it to close."""
        if state["done"] or not root.winfo_exists():
            return
        running = app_running()
        if running:
            status.configure(text="JondisBoard is open. Close it first, then click Uninstall.")
            status.pack(fill="x", pady=(14, 0), before=buttons)
            go.configure(state="disabled")
        else:
            status.pack_forget()
            go.configure(state="normal")
        root.after(1000, watch_app)

    def uninstall():
        if app_running():
            return
        go.configure(state="disabled"); cancel.configure(state="disabled")
        root.update()
        left = remove(state["found"], keep_var.get(), recycle)
        state["done"] = True
        for w in content.winfo_children():
            w.destroy()
        status.pack_forget()
        ctk.CTkLabel(content, text="JondisBoard is uninstalled.", font=font(15, "bold"), text_color=t["TEXT"],
                     anchor="w").pack(anchor="w")
        if left:
            ctk.CTkLabel(content, text="Couldn't remove (delete these yourself):\n" + "\n".join(f"•  {p}" for p in left),
                         font=font(12), text_color="#ffb547", anchor="w", justify="left", wraplength=400).pack(anchor="w", pady=(8, 0))
        elif not keep_var.get() and state["found"]["data"]:
            ctk.CTkLabel(content, text="Your sounds and settings are still in this folder.", font=font(12),
                         text_color=t["MUTED"], anchor="w").pack(anchor="w", pady=(6, 0))
        go.pack_forget()
        cancel.configure(state="normal", text="Close", command=close)
        fit()

    def close():
        quit_window()
        if self_remove:
            remove_self_later(folder)

    def quit_window():
        root.destroy()
        try:
            ico.unlink()
        except OSError:
            pass

    cancel.configure(command=quit_window)
    root.protocol("WM_DELETE_WINDOW", lambda: close() if state["done"] else quit_window())
    show_plan()
    fit()
    root.after(200, watch_app)
    root.uninstaller = {"state": state, "keep": keep_var, "go": go, "cancel": cancel, "content": content, "status": status}
    return root


if __name__ == "__main__":
    main().mainloop()
