"""Clicks (play on release), dragging to re-sort, onto group tabs, Esc, the ⋯ button, saved order,
auto-scroll."""
import sys
from pathlib import Path

from _harness import GROUP, Ev, Suite, center


def main():
    s = Suite("drag and drop")
    A = s.A
    app = s.open()
    root, at, check, played = s.root, s.at, s.check, s.played
    names = lambda: app.dragger._visible()                   # noqa: E731
    tiles = lambda: [app.page.tiles[n] for n in names()]     # noqa: E731
    state = {}

    def press_drag(tile, path, release=True):
        x, y = center(tile)
        tile._press(Ev(x, y))
        for px, py in path:
            tile._drag_motion(Ev(px, py))
            root.update()
        if release:
            tile._release(Ev(*path[-1]) if path else Ev(x, y))

    def clicks():
        t = tiles()[1]
        press_drag(t, [])
        check("a click plays (on release)", played == [t.name])
        app.clip_menu(Ev(*center(t)), t.name); root.update()
        t._press(Ev(*center(t)))
        app._click_anywhere(Ev(widget=t, serial=12345))
        t._release(Ev(*center(t)))
        check("a click that closes a menu doesn't play", played == [t.name] and not app.open_menus)
        x, y = center(t)
        press_drag(t, [(x + 3, y + 2)])
        check("a tiny wobble (under 6 px) still counts as a click", len(played) == 2)
    at(0, clicks)

    def drag_mid():
        state["before"] = names()
        t0, t3 = tiles()[0], tiles()[3]
        press_drag(t0, [(center(t0)[0] + 40, center(t0)[1] + 10), center(t3, 0.2)], release=False)
        state["t0"] = t0
        check("a drag starts: tile goes faint, nothing plays", app.dragger.active and t0.dragging and len(played) == 2)
        check("the drop marker shows", app.dragger.marker is not None and app.dragger.marker.winfo_ismapped())
    at(300, drag_mid)

    def drop():
        b = state["before"]
        state["t0"]._release(Ev(*center(app.page.tiles[b[3]], 0.2)))
        want = b[1:3] + [b[0]] + b[3:]
        check("dropped before the 4th tile: order updated", names() == want)
        check("the order is saved", s.config()["order"][:4] == want[:4])
        check("the drag is over", not app.dragger.active)
    at(800, drop)
    at(1450, lambda: check("lifted copy gone after landing, tile back to normal",
                           not app.dragger.ghost.winfo_viewable() and not state["t0"].dragging))

    def to_end():
        b = names(); t, last = tiles()[0], tiles()[-1]
        x, y = center(last, 0.95)
        press_drag(t, [(x - 20, y), (x + 5, y)])
        check("dropped after the last tile", names() == b[1:] + [b[0]])
    at(1500, to_end)

    def cancel():
        b = names(); t = tiles()[2]
        press_drag(t, [(center(t)[0] + 300, center(t)[1] + 100)], release=False)
        app._escape()
        t._release(Ev(center(t)[0] + 300, center(t)[1] + 100))
        check("Esc cancels: nothing moved, nothing played", names() == b and len(played) == 2 and not app.dragger.active)
    at(2100, cancel)

    def over_tab():
        t = [x for x in tiles() if not A.group_of(x.name)][0]
        state["moving"] = t.name
        tab = app._tabs[GROUP]
        press_drag(t, [(center(t)[0], center(t)[1] - 50), center(tab)], release=False)
        check("a group tab lights up under the dragged sound", tab.drop and app.dragger.tab is tab)
        check("no drop marker while over a tab", not app.dragger.marker.winfo_ismapped())
    at(2700, over_tab)
    at(3100, lambda: app.page.tiles[state["moving"]]._release(Ev(*center(app._tabs[GROUP]))))

    def in_group():
        moved = f"{GROUP}/{Path(state['moving']).name}"
        check("dropped on a group tab: the file moves into the group", moved in app.clips and (A.SOUNDS / moved).exists())
        check("the tab stops being lit", not app._tabs[GROUP].drop)
        state["moved"] = moved
    at(3700, in_group)

    def back_out():
        t = app.page.tiles[state["moved"]]
        press_drag(t, [(center(t)[0], center(t)[1] - 50), center(app._tabs[A.NO_GROUP])])
    at(3800, back_out)
    at(4400, lambda: check("dropped on No group: moved back out", state["moving"] in app.clips))

    def more_button():
        t = tiles()[0]
        t._set_hover(True); root.update()
        bx, by, *_ = t.bbox(t.more)
        x, y = t.winfo_rootx() + bx + 4, t.winfo_rooty() + by + 4
        t._press(Ev(x, y))
        t._down = t._down[:5] + (True,)   # Tk sets 'current' from the real pointer; aim this press at the ⋯
        t._drag_motion(Ev(x + 40, y + 40))
        check("pressing the options button doesn't start a drag", not app.dragger.active)
        t._release(Ev(x, y))
        check("releasing on the options button opens the menu", bool(app.open_menus))
        app.close_menus()
    at(4500, more_button)

    def saved_order():
        b = names()
        app.reload_clips()
        check("the order survives a reload", names() == b)
        victim = b[1]
        new = app._move_clip_files(victim, (A.SOUNDS / victim).parent, "zz renamed for test")
        app.reload_clips()
        check("a renamed sound keeps its place", names()[1] == new)
        app._move_clip_files(new, (A.SOUNDS / new).parent, Path(victim).stem)
        app.reload_clips()
        app.sort_az()
        check("Sort A-Z", names() == sorted(names(), key=lambda n: Path(n).stem.lower()))
    at(4900, saved_order)
    # auto-scroll near the bottom edge, in a window short enough for the board to scroll
    at(5100, lambda: (root.state("normal"), A.tk.Tk.wm_geometry(root, "1300x520")))

    def scroll():
        t = tiles()[0]
        press_drag(t, [(center(t)[0], app.holder.winfo_rooty() + app.holder.winfo_height() - 5)], release=False)
        state["y0"] = app.page.frame._parent_canvas.yview()[0]
    at(6000, scroll)
    at(6600, lambda: (check("dragging near the bottom edge scrolls the board",
                            app.page.frame._parent_canvas.yview()[0] > state["y0"]), app._escape()))
    return s.run()


if __name__ == "__main__":
    sys.exit(main())
