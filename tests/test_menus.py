"""Menus, the Settings popover, dialogs, minimise/restore and tile fades."""
import sys

from _harness import Ev, Suite


def main():
    s = Suite("menus", synthetic_pointer=False, stub_play=False)
    A = s.A
    # popups asked for before the window is up must wait for it
    build = A.App._build_ui

    def build_and_ask(self):
        build(self)
        self.toggle_settings(True)
        self.toast("early toast")
    A.App._build_ui = build_and_ask
    app = s.open()
    A.App._build_ui = build
    root, at, check = s.root, s.at, s.check
    vis = lambda w: bool(w.winfo_exists() and w.winfo_viewable())   # noqa: E731

    check("Settings waits until the window is up", not app.settings_open and not vis(app.settings_win))
    at(0, lambda: check("...then opens", app._ready and app.settings_open and vis(app.settings_win)))
    at(0, lambda: check("an early toast shows once the window is up", app._toast is not None and vis(app._toast)))
    # clicks inside the popover keep it; on the board close it
    at(1000, lambda: app._click_anywhere(Ev(widget=app.menus[0].label, serial=999)))
    at(1010, lambda: check("click inside Settings keeps it open", app.settings_open))
    at(1020, lambda: app._click_anywhere(Ev(widget=app.holder, serial=999)))
    at(1030, lambda: check("click on the board closes Settings", not app.settings_open))
    at(1300, lambda: check("closed popover is hidden after its fade", not vis(app.settings_win)))
    # open, close, open within a few ms: ends open at full opacity
    at(1400, lambda: app.toggle_settings(True))
    at(1430, lambda: app.toggle_settings(False))
    at(1450, lambda: app.toggle_settings(True))
    at(1900, lambda: check("rapid toggling ends open and opaque", app.settings_open and vis(app.settings_win)
                           and float(app.settings_win.attributes("-alpha")) > 0.99))
    # a device list inside it: clicking elsewhere in the popover closes only the list
    at(2000, lambda: app.menus[0].open(Ev(widget=app.menus[0], serial=5)))
    at(2300, lambda: check("device list open, its field marked open", app.open_menus and app.menus[0].is_open))
    at(2310, lambda: app._click_anywhere(Ev(widget=app.settings, serial=999)))
    at(2320, lambda: check("click in the popover closes only the list", not app.open_menus and app.settings_open))
    at(2330, lambda: check("the field un-marks when its list closes", not app.menus[0].is_open))
    # Esc: menus first, then the popover
    at(2500, lambda: app.menus[1].open(Ev(widget=app.menus[1], serial=6)))
    at(2800, app._escape)
    at(2810, lambda: check("Esc closes the menu first", not app.open_menus and app.settings_open))
    at(2820, app._escape)
    at(2830, lambda: check("a second Esc closes Settings", not app.settings_open))
    # a menu item's command runs once the menu has faded out
    ran = []

    def menu_cmd():
        m = app._popup(None)
        m.item("x", lambda: ran.append(A.time.perf_counter()))
        m.show(root.winfo_rootx() + 200, root.winfo_rooty() + 200)
        ran.append(m)
    at(3000, menu_cmd)

    def click_item():
        row = [w for w in ran[0].inner.winfo_children() if hasattr(w, "submenu")][0]
        ran.append(A.time.perf_counter())
        row._canvas.event_generate("<Button-1>", x=5, y=5)   # (CTkFrame.bind binds its inner canvas)
    at(3300, click_item)
    at(3700, lambda: check("menu action runs after the fade", len(ran) == 3 and 0.1 < ran[2] - ran[1] < 0.3))
    at(3710, lambda: check("menu window destroyed after its fade", not ran[0].win.winfo_exists()))
    # dialogs: Esc fades and closes; OK runs its follow-up after closing
    done = []
    dialogs = lambda: [w for w in root.winfo_children() if hasattr(w, "close") and not isinstance(w, A._MenuWindow)]  # noqa: E731
    at(3800, lambda: app._confirm("Test?", "text", "OK", lambda: done.append(1)))
    at(4400, lambda: check("confirm dialog shows", dialogs() and vis(dialogs()[0])))
    at(4410, lambda: dialogs()[0].event_generate("<Escape>"))
    at(4700, lambda: check("Esc closes a dialog without running OK", not dialogs() and not done))
    at(4800, lambda: app._ask_text("Name", "abc", "Save", lambda t: done.append(t)))

    def press_enter():
        entry = [c for c in dialogs()[0].winfo_children() if isinstance(c, A.ctk.CTkEntry)][0]
        entry.event_generate("<Return>")
    at(5400, press_enter)
    at(5700, lambda: check("text dialog's OK runs after it closes", done == ["abc"] and not dialogs()))
    # minimising closes the popover; restoring fades back to fully opaque
    at(5800, lambda: app.toggle_settings(True))
    at(6100, root.iconify)
    at(6400, lambda: check("minimising closes Settings", not app.settings_open))
    at(6500, root.deiconify)
    at(7000, lambda: check("restored window is fully opaque again", float(root.attributes("-alpha")) > 0.99))
    # a tile's flash and hover fade settle on its idle look
    tiles = []

    def tile_fades():
        t = list(app.page.tiles.values())[3]
        t.flash(); t._set_hover(True); t._set_hover(False)
        tiles.append(t)
    at(7100, tile_fades)
    at(7700, lambda: check("tile settles on its idle look", tiles[0]._fade_now["look"] == tiles[0]._look()[:3]))
    return s.run()


if __name__ == "__main__":
    sys.exit(main())
