"""Shared set-up for the UI tests.

Every suite runs the real app in its own process, on a throwaway copy of the code in a temp folder,
with its own generated sounds (short sine tones), a silent config, and the audio engine's device
code replaced by a stand-in that opens nothing, so no sound reaches a mic, cable or speaker and the
tests run on any PC (VB-CABLE or not). Pointer events are synthetic, so the real mouse is never
moved, and the clipboard is stubbed where a suite needs one. The app's windows do open on screen
while a suite runs (Tk needs them); don't run the tests while you're in a game."""
import json
import logging
import shutil
import sys
import tempfile
import traceback
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
GROUP = "Memes"
GROUPED = ["Air horn", "Bruh", "Sad trombone", "Vine boom", "Wow",
           "Drum roll with a really long name that has to wrap onto two lines"]
LOOSE = ["Applause", "Boing", "Crickets", "Ding", "Evil laugh", "Fail horn", "Gong", "Jazz",
         "Laugh track", "Record scratch"]
PICTURE = "Jazz"   # the one sound with a picture


def make_copy():
    """A temp folder laid out like the project (src/, assets/, sounds/, config.json), with the test
    sounds and a silent config."""
    import numpy as np
    import soundfile as sf
    from PIL import Image
    d = Path(tempfile.mkdtemp(prefix="jondisboard-test-"))
    shutil.copytree(REPO / "src", d / "src", ignore=shutil.ignore_patterns("__pycache__"))
    shutil.copytree(REPO / "assets", d / "assets")
    sounds = d / "sounds"
    (sounds / GROUP).mkdir(parents=True)
    t = np.linspace(0, 1.5, int(48000 * 1.5), endpoint=False)
    for i, name in enumerate(GROUPED + LOOSE):
        tone = (0.05 * np.sin(2 * np.pi * (220 + 30 * i) * t)).astype("float32")
        folder = sounds / GROUP if name in GROUPED else sounds
        sf.write(folder / f"{name}.wav", np.stack([tone, tone], 1), 48000)
    Image.new("RGB", (64, 64), (180, 60, 200)).save(sounds / f"{PICTURE}.png")
    (d / "config.json").write_text(json.dumps({
        "mic_volume": 0.0, "clip_volume": 0.0, "monitor_volume": 0.0, "mic_passthrough": False,
        "hotkeys": {}, "group": "*", "group_colors": {GROUP: "#14b8a6"}, "theme": "Toxic Green",
        "geometry": "1300x820+40+30"}), encoding="utf-8")
    return d


class Ev:
    """A stand-in mouse event at screen point x, y."""
    def __init__(self, x=0, y=0, widget=None, serial=0):
        self.x_root, self.y_root, self.widget, self.serial = int(x), int(y), widget, serial


def center(w, fx=0.5, fy=0.5):
    return w.winfo_rootx() + w.winfo_width() * fx, w.winfo_rooty() + w.winfo_height() * fy


class Suite:
    """One test suite: Suite(name), then open() for the app, at(ms, fn) to schedule steps (counted
    from when the window is on screen and focused), check(name, ok) inside them, and run() at the end."""

    def __init__(self, name, synthetic_pointer=True, stub_play=True):
        self.name = name
        self.dir = make_copy()
        self.cfg_path = self.dir / "config.json"
        sys.path.insert(0, str(self.dir / "src"))
        import engine   # the copy's engine: no devices
        engine.list_devices = lambda kind: [f"Test {kind}"]
        engine.find_cable_output_name = lambda: "Test cable"
        engine.default_device_name = lambda kind: f"Test {kind}"
        engine.is_mapper = lambda name: False

        def start(self_, *args):   # "opens" the cable and monitor without any streams: plays are recorded, silent
            self_.cable_open = self_.monitor_open = True
            return []
        engine.Engine.start = start
        engine.Engine.stop = lambda self_: self_.stop_all()
        import app as A
        self.A = A
        self.results, self.errors, self.steps, self.played = [], [], [], []
        self.stub_play = stub_play
        A.smooth_timers(True)
        A.ctk.set_appearance_mode("dark")
        if synthetic_pointer:
            # pointer_inside() asks Tk what's under the real pointer; use the stand-in event's spot
            A.pointer_inside = lambda w, e=None: e is not None and \
                w.winfo_rootx() <= e.x_root < w.winfo_rootx() + w.winfo_width() and \
                w.winfo_rooty() <= e.y_root < w.winfo_rooty() + w.winfo_height()

    def open(self):
        A = self.A
        self.root = root = A.ctk.CTk()
        root.report_callback_exception = self._ui_error
        self.app = A.App(root)
        if self.stub_play:
            self.app.play = lambda n: self.played.append(n)   # count plays, make no sound
        return self.app

    def _ui_error(self, *exc):
        self.errors.append(exc)
        print("UI ERROR", "".join(traceback.format_exception(*exc))[-900:], flush=True)

    def check(self, name, ok):
        self.results.append(bool(ok))
        print(("PASS " if ok else "FAIL ") + name, flush=True)

    def at(self, ms, fn):
        self.steps.append((ms, fn))

    def config(self):
        return json.loads(self.cfg_path.read_text(encoding="utf-8"))

    # --- running ---
    def run(self, settle_ms=600):
        last = max((ms for ms, _ in self.steps), default=0)
        self.at(last + settle_ms, self._finish)
        # steps start once the window is on screen AND has focus (like a real launch), since
        # menus close themselves when another program has the focus
        root = self.root

        def ready():
            root.lift()
            root.focus_force()
            root.after(150, lambda: [root.after(ms, fn) for ms, fn in self.steps])
        root.after(10, lambda: self.app.when_ready(ready))
        self.loop()
        return self.done()

    def loop(self):
        """The main loop, then closing the window (the way the app's _run_windows does)."""
        self.root.mainloop()
        try:
            self.A.close_window(self.root)
        except Exception as ex:
            self.check(f"window closes cleanly ({ex})", False)

    def _finish(self):
        self.check("no UI errors", not self.errors)
        self.app.restart = False
        self.app.quit()

    def done(self):
        self.A.smooth_timers(False)
        passed, total = sum(self.results), len(self.results)
        print(f"{self.name}: {passed}/{total} passed", flush=True)
        logging.shutdown()   # let go of the copy's log file, then remove the copy
        shutil.rmtree(self.dir, ignore_errors=True)
        return 0 if total and passed == total else 1
