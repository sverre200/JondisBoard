"""The uninstaller, on a throwaway install: fake app files, sounds and settings, real .lnk shortcuts
in fake desktop / Start menu / taskbar folders, a fake icon cache and a fake Recycle Bin. Nothing
outside the temp folder is looked at or touched."""
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "src"))
import uninstall as U  # noqa: E402

results = []


def check(name, ok):
    results.append(bool(ok))
    print(("PASS " if ok else "FAIL ") + name, flush=True)


def make_shortcut(lnk, target):
    script = ("$s = (New-Object -ComObject WScript.Shell).CreateShortcut($env:JB_LNK); "
              "$s.TargetPath = $env:JB_TARGET; $s.Save()")
    subprocess.run(["powershell", "-NoProfile", "-NonInteractive", "-Command", script], check=True,
                   env={**__import__("os").environ, "JB_LNK": str(lnk), "JB_TARGET": str(target)},
                   creationflags=U.NO_WINDOW)


def install(base):
    """A fake install folder next to a fake desktop, Start menu, taskbar pins, icon cache and bin."""
    app = base / "JondisBoard"
    (app / "sounds" / "Memes").mkdir(parents=True)
    (app / "src").mkdir()                                   # as if it were a code checkout too
    for name in U.PROGRAM_FILES + (U.UNINSTALLER,):
        (app / name).write_bytes(b"x")
    (app / "sounds" / "Boing.wav").write_bytes(b"x")
    (app / "sounds" / "Memes" / "Bruh.wav").write_bytes(b"x")
    (app / "config.json").write_text(json.dumps({"theme": "Ocean Blue"}))
    (app / "notes.txt").write_text("mine")                  # not JondisBoard's: must stay
    (app / "src" / "app.py").write_text("# code")
    desk, start, pins = base / "Desktop", base / "Start" / "Programs" / "Games", base / "Pins"
    for d in (desk, start, pins):
        d.mkdir(parents=True)
    make_shortcut(desk / "JondisBoard.lnk", app / U.APP_EXE)
    make_shortcut(start / "JondisBoard.lnk", app / U.APP_EXE)
    make_shortcut(start / "Uninstall JondisBoard.lnk", app / U.UNINSTALLER)
    make_shortcut(pins / "JondisBoard.lnk", app / U.APP_EXE)
    make_shortcut(desk / "Notepad.lnk", Path(r"C:\Windows\System32\notepad.exe"))   # someone else's
    cache = base / "Temp" / "JondisBoard"
    cache.mkdir(parents=True)
    (cache / "logo1_45e33f_080d09.ico").write_bytes(b"x")
    (cache / "other.txt").write_text("not an icon of ours")
    trash = base / "Bin"
    trash.mkdir()

    def recycle(paths):
        for p in paths:
            shutil.move(str(p), str(trash / Path(p).name))
        return []
    return app, ([desk, pins], [base / "Start" / "Programs"]), cache, trash, recycle


def logic():
    base = Path(tempfile.mkdtemp(prefix="jondisboard-uninstall-"))
    try:
        app, folders, cache, trash, recycle = install(base)
        found = U.find(app, folders, cache)
        check("finds the app's files", {p.name for p in found["program"]} == set(U.PROGRAM_FILES))
        check("finds sounds and settings", {p.name for p in found["data"]} == {"sounds", "config.json"})
        check("finds the 4 shortcuts to it (desktop, Start menu incl. subfolder, taskbar), not Notepad's",
              sorted(p.parent.name + "/" + p.name for p in found["shortcuts"]) ==
              ["Desktop/JondisBoard.lnk", "Games/JondisBoard.lnk", "Games/Uninstall JondisBoard.lnk", "Pins/JondisBoard.lnk"])
        check("counts your sounds", U.count_sounds(app) == 2)
        left = U.remove(found, with_data=False, recycle=recycle)
        check("removes the app's files and shortcuts", not left and not any(p.exists() for p in found["program"] + found["shortcuts"]))
        check("box unticked: sounds and settings stay", (app / "sounds" / "Boing.wav").exists() and (app / "config.json").exists())
        check("leaves files that aren't JondisBoard's", (app / "notes.txt").exists() and (app / "src" / "app.py").exists()
              and (base / "Desktop" / "Notepad.lnk").exists())
        check("removes its icons but not other things in that temp folder",
              not (cache / "logo1_45e33f_080d09.ico").exists() and (cache / "other.txt").exists())
        found = U.find(app, folders, cache)
        U.remove(found, with_data=True, recycle=recycle)
        check("box ticked: sounds and settings go to the Recycle Bin",
              not (app / "sounds").exists() and (trash / "sounds" / "Memes" / "Bruh.wav").exists() and (trash / "config.json").exists())
        check("the folder itself stays while anything else is in it", app.exists())
    finally:
        shutil.rmtree(base, ignore_errors=True)


def window():
    base = Path(tempfile.mkdtemp(prefix="jondisboard-uninstall-"))
    try:
        app, folders, cache, trash, recycle = install(base)
        running = [True]
        U.app_running = lambda: running[0]
        root = U.main(app, folders, cache, recycle=recycle, self_remove=False)
        ui = root.uninstaller
        def texts():
            out = []
            for w in ui["content"].winfo_children():
                try:
                    out.append(str(w.cget("text")))
                except Exception:   # (not a widget with text)
                    pass
            return out
        steps = []

        def opened():
            check("lists what it removes", any("JondisBoard.exe" in t for t in texts()) and any("4 shortcuts" in t for t in texts()))
            check("offers your 2 sounds and settings, unticked", any("your 2 sounds and settings" in t for t in texts()) and not ui["keep"].get())
            check("says VB-CABLE stays", any("VB-CABLE" in t for t in texts()))
            check("JondisBoard open: Uninstall waits, and says why", ui["go"].cget("state") == "disabled" and "Close it first" in ui["status"].cget("text"))
            running[0] = False
        steps.append((700, opened))
        steps.append((2000, lambda: check("closed: Uninstall is available", ui["go"].cget("state") == "normal")))

        def press():
            ui["keep"].set(True)
            ui["go"].invoke()
            check("uninstalls: files, shortcuts and (ticked) sounds gone", not (app / U.APP_EXE).exists()
                  and not (base / "Desktop" / "JondisBoard.lnk").exists() and (trash / "sounds").exists())
            check("then says it's done, with a Close button", any("uninstalled" in t for t in texts()) and ui["cancel"].cget("text") == "Close")
        steps.append((2200, press))
        steps.append((2600, root.destroy))
        for ms, fn in steps:
            root.after(ms, fn)
        root.mainloop()
    finally:
        shutil.rmtree(base, ignore_errors=True)


def self_removal():
    """The last step of the built uninstaller: delete itself, then the folder only if that empties it."""
    import time
    base = Path(tempfile.mkdtemp(prefix="jondisboard-uninstall-"))
    frozen, exe_before = U.FROZEN, sys.executable
    try:
        for case, extra in (("empty", None), ("with your file", "notes.txt")):
            d = base / f"install {case}"
            d.mkdir()
            exe = d / U.UNINSTALLER
            exe.write_bytes(b"x")
            if extra:
                (d / extra).write_text("mine")
            U.FROZEN, sys.executable = True, str(exe)   # act as the built exe
            U.remove_self_later(d)
            for _ in range(40):
                time.sleep(0.25)
                if not exe.exists():
                    time.sleep(0.5)
                    break
            if extra:
                check("removes itself but keeps a folder that still has your files", not exe.exists() and (d / extra).exists())
            else:
                check("removes itself and the folder it emptied", not exe.exists() and not d.exists())
    finally:
        U.FROZEN, sys.executable = frozen, exe_before
        shutil.rmtree(base, ignore_errors=True)


def main():
    logic()
    window()
    self_removal()
    passed, total = sum(results), len(results)
    print(f"uninstall: {passed}/{total} passed", flush=True)
    return 0 if total and passed == total else 1


if __name__ == "__main__":
    sys.exit(main())
