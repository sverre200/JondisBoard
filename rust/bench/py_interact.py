"""Python app interaction latencies, measured inside the app (from source, audio stubbed out).

    .venv\\Scripts\\python.exe rust/bench/py_interact.py <library folder>

Copies src/ into the library folder (the app then uses its sounds/ and config.json), replaces the
audio device code with a stand-in that opens nothing, starts the real App and times:
  startup_ms      App() created -> window ready
  group_ms        show_group() -> fully drawn (update()), median of cycling through every tab
  search_ms       typing a query -> drawn, and clearing it again
  resize_ms       window 200 px wider -> laid out and drawn
  theme_ms        switch_theme() -> new window ready (the Python app rebuilds its window)
Prints one JSON line. The window opens on screen: don't run while gaming.
"""
import json
import shutil
import statistics
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
lib = Path(sys.argv[1])
shutil.rmtree(lib / "src", ignore_errors=True)
shutil.copytree(REPO / "src", lib / "src", ignore=shutil.ignore_patterns("__pycache__"))
shutil.copytree(REPO / "assets", lib / "assets", dirs_exist_ok=True)
sys.path.insert(0, str(lib / "src"))
import engine  # noqa: E402
engine.list_devices = lambda kind: [f"Test {kind}"]
engine.find_cable_output_name = lambda: "Test cable"
engine.default_device_name = lambda kind: f"Test {kind}"
engine.is_mapper = lambda name: False


def start(self_, *args):
    self_.cable_open = self_.monitor_open = True
    return []


engine.Engine.start = start
engine.Engine.stop = lambda self_: self_.stop_all()
import app as A  # noqa: E402

A.smooth_timers(True)
A.ctk.set_appearance_mode("dark")
res = {"sounds": lib.name}


def watchdog():
    # the Python app sometimes never finishes its rebuilt window at 1,000 sounds: report what
    # was measured, with the theme switch marked as not finished within the limit
    import os
    import threading
    import time as _t

    def fire():
        res.pop("_theme_t0", None)
        res.setdefault("theme_ms", "did not finish in 240 s")
        print(json.dumps(res), flush=True)
        os._exit(2)
    threading.Timer(240, fire).start()


watchdog()


def run_window(on_ready):
    root = A.ctk.CTk()
    t0 = time.perf_counter()
    app = A.App(root)
    app.when_ready(lambda: root.after(0, lambda: on_ready(root, app, (time.perf_counter() - t0) * 1000)))
    root.mainloop()
    A.close_window(root)
    return app


def measure(root, app, startup):
    res["startup_ms"] = round(startup)

    def go():
        root.update()
        keys = list(app._tabs)
        times = []
        for _ in range(2):
            for k in keys + [A.ALL]:
                t = time.perf_counter()
                app.show_group(k)
                root.update()
                times.append((time.perf_counter() - t) * 1000)
        res["group_ms"] = round(statistics.median(times), 1)
        res["group_max_ms"] = round(max(times), 1)
        t = time.perf_counter()
        app.search.insert("end", "bruh")
        app._search_changed()
        root.update()
        res["search_ms"] = round((time.perf_counter() - t) * 1000, 1)
        t = time.perf_counter()
        app.search.delete(0, "end")
        app._search_changed()
        root.update()
        res["search_clear_ms"] = round((time.perf_counter() - t) * 1000, 1)
        w, h = root.winfo_width(), root.winfo_height()
        t = time.perf_counter()
        root.geometry(f"{w + 200}x{h}")
        root.update()
        res["resize_ms"] = round((time.perf_counter() - t) * 1000, 1)
        res["_theme_t0"] = time.perf_counter()
        app.switch_theme("Ocean Blue")
    root.after(4000, go)   # let the hidden pages pre-build first


app = run_window(measure)
if app.restart:
    t0 = res.pop("_theme_t0")

    def theme_done(root, app2, _):
        res["theme_ms"] = round((time.perf_counter() - t0) * 1000)
        app2.cfg["theme"] = "Toxic Green"
        app2.restart = False
        app2.quit()
    run_window(theme_done)
A.smooth_timers(False)
print(json.dumps(res), flush=True)
shutil.rmtree(lib / "src", ignore_errors=True)
