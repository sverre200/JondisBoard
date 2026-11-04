"""Re-sort slides (one canvas, frame timing, landing), group colour changes, quick repeated drops."""
import sys
import time

from _harness import GROUP, Ev, Suite, center


def main():
    s = Suite("slide")
    A = s.A
    app = s.open()
    root, at, check = s.root, s.at, s.check
    gaps, state = [], {}
    animate = A.animate

    def timed(widget, ms, step, done=None, delay_ms=0, warmup=False):   # frame gaps of the 220 ms slide
        if ms != 220:
            return animate(widget, ms, step, done, delay_ms, warmup)
        last = [None]

        def st(e):
            r = step(e)
            now = time.perf_counter()
            if last[0] is not None and 0 < e < 1:
                gaps.append((now - last[0]) * 1000)
            last[0] = now
            return r
        return animate(widget, ms, st, done, delay_ms, warmup)
    A.animate = timed

    def colour():
        t = next(t for t in app.pages[A.ALL].tiles.values() if A.group_of(t.name) == GROUP)
        before = (t.itemcget(t.pic, "image"), t.itemcget(t.gdot, "image"))
        app.set_group_color(GROUP, "#ef4444")
        check("a group colour change redraws its tiles' picture and dot",
              (t.itemcget(t.pic, "image"), t.itemcget(t.gdot, "image")) != before)
        check("the tile's tint follows the new colour", t._tint() == "#ef4444")
        g = app.pages.get(GROUP)
        if g and g.built:
            check("...on the group's own page too", next(iter(g.tiles.values()))._tint() == "#ef4444")
        app.set_group_color(GROUP, "#14b8a6")
    at(400, colour)

    def drop():
        vis = app.dragger._visible()
        state["before"] = vis
        t0, t9 = app.page.tiles[vis[0]], app.page.tiles[vis[9]]
        t0._press(Ev(*center(t0))); t0._drag_motion(Ev(center(t0)[0] + 40, center(t0)[1]))
        t0._drag_motion(Ev(*center(t9, 0.2))); root.update()
        state["cells"] = app.page.cells()
        t0._release(Ev(*center(t9, 0.2)))
        page = app.page
        check("the slide is drawn on one canvas over the board", page.overlay is not None)
        d = page.tiles[vis[0]]
        check("the dropped tile is already in its new cell",
              (int(d.place_info()["x"]), int(d.place_info()["y"])) == page.targets[vis[0]])
        check("the dropped tile is no longer faint", not d.dragging)
        root.after(300, lambda: (check("the slide canvas is gone afterwards", page.overlay is None),
                                 check("the dropped tile lights up as the slide ends", d._flash_until > time.perf_counter())))
    at(900, drop)

    def settled():
        page = app.page
        b = state["before"]
        check("the final order is right", app.dragger._visible() == b[1:9] + [b[0]] + b[9:])
        root.update_idletasks()
        cells = sorted(state["cells"].values(), key=lambda p: (p[1], p[0]))
        check("tiles end exactly on the board's cells",
              [(page.tiles[n].winfo_x(), page.tiles[n].winfo_y()) for n in app.dragger._visible()] == cells)
        if gaps:
            print(f"  slide: {len(gaps)} frames, avg {sum(gaps) / len(gaps):.1f} ms, max {max(gaps):.1f} ms", flush=True)
        check("the slide runs smoothly (no frame over 20 ms)", gaps and max(gaps) < 20)
    at(1600, settled)

    def twice():
        for _ in range(2):
            vis = app.dragger._visible()
            t0, t5 = app.page.tiles[vis[0]], app.page.tiles[vis[5]]
            t0._press(Ev(*center(t0))); t0._drag_motion(Ev(center(t0)[0] + 40, center(t0)[1]))
            t0._drag_motion(Ev(*center(t5, 0.2))); t0._release(Ev(*center(t5, 0.2)))
        root.after(500, lambda: check("two quick drops end with a clean board",
                                      app.page._slide is None and app.page.overlay is None))
    at(2000, twice)
    return s.run()


if __name__ == "__main__":
    sys.exit(main())
