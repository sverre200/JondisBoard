"""The board's layout: clean rows and columns, resizing, search, an empty group, a group page."""
import sys

from _harness import GROUP, GROUPED, Suite


def main():
    s = Suite("layout")
    A = s.A
    # every opacity the main window is given while it opens: hidden while it's built, then shown
    # at once (no fade-in: that was taken out on request)
    alphas = []
    real_attributes = A.ctk.CTk.attributes

    def attributes(self, *args):
        if len(args) == 2 and args[0] == "-alpha":
            alphas.append(round(float(args[1]), 3))
        return real_attributes(self, *args)
    A.ctk.CTk.attributes = attributes
    app = s.open()
    root, at, check = s.root, s.at, s.check
    state = {}

    def opening():
        A.ctk.CTk.attributes = real_attributes
        # 0 while built, an invisible 1 % while it paints (Windows won't draw a 0 % window), then 100 %
        seen = sorted(set(alphas))
        check("the window appears at once, no fade-in", seen == [0.0, 0.01, 1.0] and alphas[-1] == 1.0
              and app._ready)
    at(0, opening)

    def clean(page):
        ts = [page.tiles[n] for n in app.dragger._visible()]
        cols = page.columns
        xs = {i % cols: t.winfo_x() for i, t in enumerate(ts)}
        rows = {t.winfo_y() for t in ts}
        widths = {t.winfo_width() for t in ts}
        return (all(t.winfo_x() == xs[i % cols] for i, t in enumerate(ts)) and len(rows) == -(-len(ts) // cols)
                and max(widths) - min(widths) <= 1), max(t.winfo_x() + t.winfo_width() for t in ts)

    def board():
        root.update_idletasks()
        ok, right = clean(app.page)
        check("tiles sit in clean columns and rows, all the same width", ok)
        sc = app.page.frame._get_widget_scaling()
        check("the last column ends one gap in from the edge", 0 < app.page.frame.winfo_width() - right <= round(8 * sc))
        rows = -(-len(app.page.tiles) // app.page.columns)
        gap = round(6 * sc)
        want = rows * (round(A.TILE_HEIGHT * sc) + 2 * gap) + gap
        check("the scroll area is exactly as tall as its rows", abs(app.page.frame.winfo_height() - want) <= 2)
        state["cols"] = app.page.columns
        A.tk.Tk.wm_geometry(root, "900x820")
    at(500, board)

    def narrow():
        root.update_idletasks()
        ok, _ = clean(app.page)
        check(f"a narrower window: fewer columns ({state['cols']} -> {app.page.columns}), still clean",
              app.page.columns < state["cols"] and ok)
        A.tk.Tk.wm_geometry(root, "1300x820")
    at(1100, narrow)

    def search():
        root.update_idletasks()
        app.search.insert(0, "bo"); app._search_changed(); root.update_idletasks()
        shown = list(app.page.geom)
        check("search shows only matches", shown and all("bo" in A.Path(n).stem.lower() for n in shown))
        app.search.delete(0, "end"); app._search_changed()
        app.search.insert(0, "zzzz"); app._search_changed(); root.update_idletasks()
        check("no matches: a message instead", app.page.msg.winfo_manager() == "place" and not app.page.geom)
        app.search.delete(0, "end"); app._search_changed()
    at(1700, search)

    def groups():
        (A.SOUNDS / "Empty test").mkdir(exist_ok=True)
        app.reload_clips(); app.show_group("Empty test"); root.update_idletasks()
        check("an empty group shows its message", app.page.key == "Empty test" and app.page.msg.winfo_manager() == "place")
        app.show_group(GROUP); root.update_idletasks()
        ok, _ = clean(app.page)
        check("a group's page is laid out cleanly", ok and len(app.page.tiles) == len(GROUPED))
        app.show_group(A.ALL)
    at(2300, groups)

    def reloaded():
        (A.SOUNDS / "Empty test").rmdir()
        app.reload_clips(); root.update_idletasks()
        ok, _ = clean(app.page)
        check("after a reload the board is still clean", ok)
    at(2900, reloaded)
    return s.run()


if __name__ == "__main__":
    sys.exit(main())
