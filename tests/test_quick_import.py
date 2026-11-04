"""Quick import (with a stand-in importer and clipboard), tile alignment, the ⋯ and group menus."""
import shutil
import sys
import time
from pathlib import Path

from _harness import GROUP, Ev, Suite

GOOD = "https://www.instagram.com/reel/ABC123/?igsh=xyz"
BAD = "https://www.instagram.com/reel/FAIL999/"
NEW = "Quick import test clip with a rather long name for wrapping"


def main():
    s = Suite("quick import")
    A = s.A
    state = {"fail_once": True}

    def fake_import(url, name=None, start=None, end=None, normalize=True, cookies_from_browser=None,
                    log=print, folder=None):
        log(f"Downloading {url} ...")
        time.sleep(0.9)
        if "FAIL" in url and state["fail_once"]:
            state["fail_once"] = False
            raise RuntimeError("ERROR: [Instagram] DXyz123: Requested content is not available, rate-limit reached")
        log("Cutting audio ...")
        time.sleep(0.5)
        dst = A.importer.unique_path(Path(folder), NEW)
        shutil.copy(s.dir / "sounds" / "Applause.wav", dst)
        log(f"Saved {dst.name}")
        return dst
    A.importer.import_clip = fake_import
    app = s.open()
    root, at, check = s.root, s.at, s.check
    clip = [""]
    root.clipboard_get = lambda: clip[0]   # never touch the real clipboard
    toasts = []
    toast = app.toast
    app.toast = lambda text, level="info": (toasts.append((text, level)), toast(text, level))
    imported = lambda: [n for n in app.clips if Path(n).stem.startswith("Quick import test")]   # noqa: E731

    def aligned():
        ts = list(app.pages[A.ALL].tiles.values())
        rows = {}
        for t in ts:
            rows.setdefault(t.winfo_y(), []).append(t)
        check("pictures, titles and info lines line up in every row", all(
            len({(t.coords(t.title)[1], t.coords(t.meta)[1], t.coords(t.pic)[0]) for t in r}) == 1 for r in rows.values()))
        check("every tile has a picture square", all(t.itemcget(t.pic, "image") for t in ts))
        check("long titles clamp to two lines", all(t.itemcget(t.title, "text").count("\n") <= 1 for t in ts))
    at(100, aligned)
    # nothing useful on the clipboard
    at(200, lambda: clip.__setitem__(0, "just some text"))
    at(210, app.quick_import)
    at(250, lambda: check("no link on the clipboard: no import, a warning", not app.jobs and toasts and toasts[-1][1] == "warn"))
    # a link inside other text starts at once
    at(300, lambda: clip.__setitem__(0, f"look at this {GOOD} lol"))
    at(310, app._paste)
    at(360, lambda: check("link found inside the clipboard text, import started", list(app.jobs) == [GOOD]))
    at(380, lambda: check("its working tile comes first on All", app.page.placed.get(("job", app.jobs[GOOD].id)) == (0, 0)))
    at(400, app._paste)
    at(430, lambda: check("the same link twice is ignored", len(app.jobs) == 1))

    def search_paste():
        app.search.focus_set(); root.update()
        n = len(app.jobs); app._paste()
        check("Ctrl+V in the search box doesn't import", len(app.jobs) == n)
        root.focus_set()
    at(700, search_paste)

    def finished():
        names = imported()
        check("import finished: job gone, sound added", not app.jobs and len(names) == 1)
        check("working tile removed from every page", all(not p.pending for p in app.pages.values()))
        t = app.pages[A.ALL].tiles.get(names[0]) if names else None
        check("the new tile's title is cut short with an ellipsis", t is not None and t.itemcget(t.title, "text").endswith("…"))
        check("a success toast", any(text.startswith("Added") for text, _ in toasts))
    at(2300, finished)
    # a failing link keeps its tile, with a short reason and Retry / Dismiss
    at(2400, lambda: clip.__setitem__(0, BAD))
    at(2410, app.quick_import)

    def failed():
        job = app.jobs.get(BAD)
        w = app.pages[A.ALL].pending.get(job.id) if job else None
        check("a failed import keeps its tile with a short reason", w is not None and job.error and
              "not available" in job.error and "ERROR" not in job.error)
        check("Retry and Dismiss shown", w is not None and w.actions.winfo_ismapped())
    at(3600, failed)
    at(3800, lambda: app.retry_job(app.jobs[BAD]))
    at(3900, lambda: check("Retry goes back to working", app.jobs[BAD].error is None and
                           not app.pages[A.ALL].pending[app.jobs[BAD].id].actions.winfo_ismapped()))
    at(5600, lambda: check("the retry succeeds", BAD not in app.jobs and len(imported()) == 2))

    def fail_again():
        state["fail_once"] = True
        clip[0] = BAD + "x"
        app.quick_import()
    at(5700, fail_again)
    at(6900, lambda: app.dismiss_job(app.jobs[BAD + "x"]))
    at(7000, lambda: check("Dismiss removes the failed tile", not app.jobs and all(not p.pending for p in app.pages.values())))
    # importing while looking at a group says where it goes
    at(7100, lambda: app.show_group(GROUP))
    at(7200, lambda: clip.__setitem__(0, "https://youtu.be/dQw4w9WgXcQ"))
    at(7210, app.quick_import)
    at(7250, lambda: check("importing from a group view says where it goes", "All" in toasts[-1][0]))

    def hover():
        t = list(app.page.tiles.values())[0]
        t._set_hover(True); root.update_idletasks()
        check("the options button shows on hover", t.itemcget(t.more, "state") == "normal")
    at(7500, hover)
    at(7600, lambda: app._tabs[GROUP]._click(Ev(serial=77)))
    at(8000, lambda: check("clicking the chosen group's tab opens its options", bool(app.open_menus)))
    at(8100, app.close_menus)
    at(8200, lambda: app.show_group(A.ALL))
    at(9200, app.more_menu)
    at(9650, lambda: check("the More menu opens", bool(app.open_menus)))
    at(9700, app.more_menu)
    at(9800, lambda: check("clicking More again closes it", not app.open_menus))
    return s.run(settle_ms=2000)   # (the YouTube import above finishes meanwhile)


if __name__ == "__main__":
    sys.exit(main())
