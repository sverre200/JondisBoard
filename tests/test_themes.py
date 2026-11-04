"""Switching through every theme after using the board (drag, sort, a setting): each window has to
close cleanly and the next one open, with no errors. (Closing used to fail after animations.)"""
import sys

from _harness import Ev, Suite, center


def main():
    s = Suite("themes")
    A = s.A
    themes = list(A.THEMES)[1:] + [list(A.THEMES)[0]]   # all of them, ending back on the first

    def use_then_switch(app, root, next_theme):
        def use():
            vis = app.dragger._visible()
            t0, t5 = app.page.tiles[vis[0]], app.page.tiles[vis[5]]
            t0._press(Ev(*center(t0))); t0._drag_motion(Ev(center(t0)[0] + 40, center(t0)[1]))
            t0._drag_motion(Ev(*center(t5, 0.2)))
            root.after(150, lambda: t0._release(Ev(*center(t5, 0.2))))
            root.after(700, app.sort_az)
            root.after(1300, lambda: app.toggle_settings(True))
            root.after(1700, lambda: app.option_vars["solo"].set(not app.option_vars["solo"].get()))
            root.after(2100, lambda: app.toggle_settings(False))
            for t in list(app.page.tiles.values())[:4]:
                t._set_hover(True); t._set_hover(False)

        def switch():
            if next_theme:
                app.switch_theme(next_theme)
            else:
                app.restart = False
                app.quit()
        root.after(10, lambda: app.when_ready(lambda: (root.after(300, use), root.after(2600, switch))))

    for k, nxt in enumerate([*themes, None]):
        app = s.open()
        current = app.cfg["theme"]
        use_then_switch(app, s.root, nxt)
        s.loop()
        if nxt:
            check_ok = app.restart and s.config()["theme"] == nxt
            s.check(f"{current} -> {nxt}: closed cleanly and asked for the next theme", check_ok)
            if not check_ok:
                break
    s.check("no UI errors in any window", not s.errors)
    return s.done()


if __name__ == "__main__":
    sys.exit(main())
