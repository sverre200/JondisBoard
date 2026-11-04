"""Playback settings: the rules for clicks and hotkeys, and the switches."""
import sys
import threading

from _harness import Suite


class RecordingEngine:
    """Remembers what the app asked for; 'playing' is what you'd hear."""
    def __init__(self):
        self.playing, self.log = set(), []

    def play(self, name, data):
        self.playing.add(name); self.log.append(("play", name))

    def stop_clip(self, name):
        self.playing.discard(name); self.log.append(("stop", name))

    def stop_all(self):
        self.playing.clear(); self.log.append(("stop_all",))

    def progress(self):
        return {n: 0.5 for n in self.playing}


def main():
    s = Suite("settings", stub_play=False)
    A = s.A
    app = s.open()
    root, at, check = s.root, s.at, s.check
    real = app.engine
    fake = app.engine = RecordingEngine()
    a, b = list(app.clips)[:2]

    def reset(solo, toggle):
        fake.playing.clear(); fake.log.clear()
        app.cfg["solo"], app.cfg["toggle_stop"] = solo, toggle

    def rules():
        reset(False, False)
        app.play(a); app.play(b); app.play(a)
        check("off: sounds overlap, pressing again restarts", fake.playing == {a, b} and
              fake.log.count(("play", a)) == 2 and ("stop_all",) not in fake.log)
        reset(True, False)
        app.play(a); app.play(b)
        check("one at a time: starting b stops a", fake.playing == {b} and fake.log[-2:] == [("stop_all",), ("play", b)])
        reset(False, True)
        app.play(a); app.play(b); app.play(a)
        check("press a playing sound: the second press stops it, others keep playing",
              fake.playing == {b} and fake.log[-1] == ("stop", a))
        app.play(a)
        check("...and a third press starts it again", fake.playing == {a, b})
        reset(True, True)
        app.play(a); app.play(b); app.play(b)
        check("both on: b replaces a, then pressing b stops it", fake.playing == set())
        reset(True, True)

        def hotkeys(left):   # from another thread, like the keyboard listener (never join it from here)
            if left:
                threading.Thread(target=app.play, args=(left[0],), daemon=True).start()
                root.after(60, lambda: hotkeys(left[1:]))
            else:
                check("hotkeys follow the same rules", fake.playing == set() and fake.log[-1] == ("stop", b))
        hotkeys([a, b, b])
    at(300, rules)
    at(700, lambda: app.toggle_settings(True))

    def switches():
        root.update()
        sw = [w for w in app.settings.winfo_children() if isinstance(w, A.Toggle)]
        check("two switches in the Playback section", len(sw) == 2)
        sw[0].toggle()
        check("switching 'One sound at a time' on saves it", app.cfg["solo"] is True and s.config()["solo"] is True)
        words = [w for f in app.settings.winfo_children() if isinstance(f, A.ctk.CTkFrame)
                 for w in f.winfo_children() if isinstance(w, A.ctk.CTkLabel)
                 and w.cget("text") == "Press a playing sound to stop it"]
        words[0]._label.event_generate("<Button-1>")
        check("clicking a setting's words flips it", app.cfg["toggle_stop"] is True and s.config()["toggle_stop"] is True)
        check("the header button is called Settings", app.settings_btn.cget("text") == "Settings")
        root.after(400, lambda: check("the switch has slid all the way across", sw[0].t == 1.0))
        app.engine = real
    at(1200, switches)
    at(1900, lambda: app.toggle_settings(False))
    return s.run()


if __name__ == "__main__":
    sys.exit(main())
