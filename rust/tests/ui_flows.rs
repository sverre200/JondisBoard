//! UI flow tests: the real window and app, driven with synthetic mouse/keyboard calls.
//!
//! JONDISBOARD_TEST=1 keeps the window cloaked (it never appears on screen, so this is safe to
//! run mid-game), opens no audio devices and swaps the clipboard for a stand-in. Every test
//! works on its own throwaway library of generated tones in %TEMP%.
//!
//! One test function runs the flows in order on one UI thread (windows belong to a thread).

use jondisboard::audio::wav;
use jondisboard::config::{Binding, ALL};
use jondisboard::paths::Paths;
use jondisboard::ui::app::{Action, App, Dialog, DialogKind, HotkeyTarget, MenuItem};
use jondisboard::ui::gfx::Gfx;
use jondisboard::ui::view::Hit;
use jondisboard::ui::window;
use std::path::{Path, PathBuf};
use std::time::Instant;
use windows::Win32::Foundation::HWND;

struct Lib(PathBuf);

impl Drop for Lib {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn tone(path: &Path, secs: f32, freq: f32) {
    let n = (48000.0 * secs) as usize;
    let data: Vec<i16> = (0..n).flat_map(|i| {
        let v = ((i as f32 * freq * 2.0 * std::f32::consts::PI / 48000.0).sin() * 3000.0) as i16;
        [v, v]
    }).collect();
    wav::write_i16(path, 48000, 2, &data).unwrap();
}

fn make_lib() -> Lib {
    let d = std::env::temp_dir().join(format!("jb_ui_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let s = d.join("sounds");
    std::fs::create_dir_all(s.join("Memes")).unwrap();
    for (i, n) in ["Air horn", "Bruh", "Sad trombone", "Vine boom"].iter().enumerate() {
        tone(&s.join("Memes").join(format!("{n}.wav")), 1.0, 300.0 + 40.0 * i as f32);
    }
    for (i, n) in ["Applause", "Boing", "Crickets", "Ding", "Evil laugh", "Fail horn", "Gong", "Jazz with a really long name that has to wrap onto two lines"].iter().enumerate() {
        tone(&s.join(format!("{n}.wav")), 1.5, 220.0 + 30.0 * i as f32);
    }
    std::fs::write(
        d.join("config.json"),
        r#"{"mic": "No such mic", "cable": "No such cable", "monitor": "No such speakers",
            "mic_volume": 0.0, "clip_volume": 0.0, "monitor_volume": 0.0, "mic_passthrough": false,
            "group": "*", "theme": "Toxic Green", "geometry": "1300x820+40+30", "unknown_key": 42}"#,
    )
    .unwrap();
    Lib(d)
}

fn app<R>(h: HWND, f: impl FnOnce(&mut App, &mut Gfx) -> R) -> R {
    window::with_app(h, f).expect("app")
}

/// Screen point (DIPs) of a clip's tile centre, from the last frame.
fn tile_center(a: &App, id: u32) -> (f32, f32) {
    let (_, r) = a.layout.tiles.iter().find(|(i, _)| *i == id).copied().expect("tile on board");
    let scroll = a.scroll.get(&a.layout.view).copied().unwrap_or(0.0);
    (r.cx(), r.cy() + a.layout.board.y - scroll)
}

fn frame(h: HWND) {
    window::pump(30);
    window::invalidate(h);
    window::pump(30);
}

fn click(h: HWND, x: f32, y: f32) {
    app(h, |a, g| {
        a.mouse_move(g, x, y);
        a.mouse_down(g, x, y, false, false);
        a.mouse_up(x, y);
    });
    frame(h);
}

fn check(name: &str, ok: bool) {
    println!("{} {name}", if ok { "PASS" } else { "FAIL" });
    assert!(ok, "{name}");
}

#[test]
fn ui_flows() {
    std::env::set_var("JONDISBOARD_TEST", "1");
    let lib = make_lib();
    let paths = Paths::new(Some(lib.0.clone()));
    let t = Instant::now();
    let h = window::create(paths, t, None).expect("window");
    frame(h);
    frame(h);

    // --- startup
    let (n, groups, tiles) = app(h, |a, _| (a.clips.len(), a.groups.clone(), a.layout.tiles.len()));
    check("loads every sound", n == 12);
    check("groups are folders", groups == vec!["Memes".to_string()]);
    check("All shows every tile", tiles == 12);
    check("first frame was drawn quickly", app(h, |a, _| a.timings.iter().any(|(k, v)| *k == "first frame" && *v < 3000.0)));

    // --- click plays on release; a press that becomes a drag never plays
    let bruh = app(h, |a, _| a.id_of("Memes/Bruh.wav").unwrap());
    let (x, y) = app(h, |a, _| tile_center(a, bruh));
    app(h, |a, g| {
        a.mouse_move(g, x, y);
        a.mouse_down(g, x, y, false, false);
    });
    window::pump(50);
    check("press alone doesn't play", app(h, |a, _| a.played.is_empty()));
    app(h, |a, _| a.mouse_up(x, y));
    window::pump(80);
    check("release plays", app(h, |a, _| a.played == vec![bruh]));

    let first = app(h, |a, _| a.visible()[0]);
    let order_before = app(h, |a, _| a.cfg.order.clone());
    let (fx, fy) = app(h, |a, _| tile_center(a, first));
    let target = app(h, |a, _| a.visible()[5]);
    let (tx, ty) = app(h, |a, _| tile_center(a, target));
    app(h, |a, g| {
        a.played.clear();
        a.mouse_move(g, fx, fy);
        a.mouse_down(g, fx, fy, false, false);
        for i in 1..=20 {
            let k = i as f32 / 20.0;
            a.mouse_move(g, fx + (tx + 5.0 - fx) * k, fy + (ty - fy) * k);
        }
    });
    frame(h);
    check("dragging lifts the tile", app(h, |a, _| a.drag.is_some()));
    app(h, |a, _| println!("  drag slot {:?} origin {:?} tab {:?} board {:?} cols {} target ({tx},{ty})", a.drag.as_ref().unwrap().slot, a.drag.as_ref().unwrap().origin, a.drag.as_ref().unwrap().tab, a.layout.board, a.layout.cols));
    app(h, |a, _| a.mouse_up(tx + 5.0, ty));
    window::pump(100);
    let order_after = app(h, |a, _| a.cfg.order.clone());
    check("a drag never plays", app(h, |a, _| a.played.is_empty()));
    check("dropping re-orders", order_after != order_before);
    let moved_name = app(h, |a, _| a.clip(first).unwrap().name.clone());
    check("the order is saved", std::fs::read_to_string(lib.0.join("config.json")).unwrap().contains(&moved_name));

    // --- drag onto a group tab moves the file
    let ding = app(h, |a, _| a.id_of("Ding.wav").unwrap());
    frame(h);
    let (dx, dy) = app(h, |a, _| tile_center(a, ding));
    let (gx, gy) = app(h, |a, _| a.layout.tabs.iter().find(|(k, _)| k == "Memes").map(|(_, r)| (r.cx(), r.cy())).unwrap());
    app(h, |a, g| {
        a.mouse_move(g, dx, dy);
        a.mouse_down(g, dx, dy, false, false);
        for i in 1..=15 {
            let k = i as f32 / 15.0;
            a.mouse_move(g, dx + (gx - dx) * k, dy + (gy - dy) * k);
        }
        println!("  over tab: {:?}", a.drag.as_ref().map(|d| d.tab.clone()));
        a.mouse_up(gx, gy);
        println!("  dropped");
    });
    frame(h);
    check("drop on a tab moves the clip into that group", lib.0.join("sounds/Memes/Ding.wav").exists() && !lib.0.join("sounds/Ding.wav").exists());
    check("it keeps its id (and so its audio and place)", app(h, |a, _| a.id_of("Memes/Ding.wav") == Some(ding)));

    // --- groups, search
    app(h, |a, _| a.show_group("Memes"));
    frame(h);
    check("a group tab shows only its sounds", app(h, |a, _| a.layout.tiles.len() == 5));
    check("the open group is remembered", std::fs::read_to_string(lib.0.join("config.json")).unwrap().contains("\"group\": \"Memes\""));
    app(h, |a, _| {
        a.key_down(0x46, true, false); // Ctrl+F
        for c in "boom".chars() {
            a.char_typed(c);
        }
    });
    frame(h);
    check("search filters across all groups", app(h, |a, _| a.visible().len() == 1 && a.view_key() == ALL));
    app(h, |a, _| a.key_down(0x1B, false, false));
    frame(h);
    check("Esc clears the search", app(h, |a, _| a.query.is_empty() && a.view_key() == "Memes"));
    app(h, |a, _| {
        a.key_down(0x1B, false, false);
        a.show_group(ALL)
    });

    // --- rename (dialog), hotkey carried along
    let boing = app(h, |a, _| a.id_of("Boing.wav").unwrap());
    let b = Binding { scan: 63, keypad: false, mods: vec!["ctrl".into()], label: "f5".into() };
    app(h, |a, _| a.assign_hotkey(HotkeyTarget::Clip(boing), Some(b.clone())));
    app(h, |a, _| a.run(Action::Rename(boing)));
    frame(h);
    check("rename opens a dialog", app(h, |a, _| a.dialog.is_some()));
    app(h, |a, _| {
        a.key_down(0x41, true, false); // select all
        for c in "Boing two".chars() {
            a.char_typed(c);
        }
        a.key_down(0x0D, false, false); // Enter
    });
    frame(h);
    check("rename moves the file", lib.0.join("sounds/Boing two.wav").exists() && !lib.0.join("sounds/Boing.wav").exists());
    check("the hotkey follows the rename", app(h, |a, _| a.cfg.hotkeys.contains_key("Boing two.wav") && a.id_of("Boing two.wav") == Some(boing)));

    // --- one key = one action
    let gong = app(h, |a, _| a.id_of("Gong.wav").unwrap());
    app(h, |a, _| a.assign_hotkey(HotkeyTarget::Clip(gong), Some(b.clone())));
    check("re-using a key takes it off the other sound", app(h, |a, _| !a.cfg.hotkeys.contains_key("Boing two.wav") && a.cfg.hotkeys.contains_key("Gong.wav")));
    check("and says so", app(h, |a, _| a.toast.as_ref().map(|t| t.text.contains("moved from")).unwrap_or(false)));
    app(h, |a, _| a.assign_hotkey(HotkeyTarget::StopAll, Some(b.clone())));
    check("Stop all can take it too", app(h, |a, _| a.cfg.stop_hotkey.is_some() && !a.cfg.hotkeys.contains_key("Gong.wav")));

    // --- delete (confirm) goes to _deleted
    app(h, |a, _| a.run(Action::Delete(gong)));
    frame(h);
    app(h, |a, _| a.dialog_ok());
    frame(h);
    check("delete moves it to sounds/_deleted", lib.0.join("sounds/_deleted/Gong.wav").exists() && !lib.0.join("sounds/Gong.wav").exists());
    check("it's gone from the board", app(h, |a, _| a.id_of("Gong.wav").is_none() && a.clips.len() == 11));

    // --- new / rename / colour / delete group
    app(h, |a, _| a.new_group("Voice lines", None));
    frame(h);
    check("new group makes a folder and opens it", lib.0.join("sounds/Voice lines").is_dir() && app(h, |a, _| a.cfg.group == "Voice lines"));
    let crickets = app(h, |a, _| a.id_of("Crickets.wav").unwrap());
    app(h, |a, _| a.move_clip(crickets, "Voice lines", false));
    app(h, |a, _| a.rename_group("Voice lines", "Lines"));
    frame(h);
    check("renaming a group renames its folder", lib.0.join("sounds/Lines/Crickets.wav").exists());
    check("and keeps the clip's identity", app(h, |a, _| a.id_of("Lines/Crickets.wav") == Some(crickets)));
    app(h, |a, _| a.set_group_color("Lines", jondisboard::theme::GROUP_COLORS[4]));
    check("group colour is saved", std::fs::read_to_string(lib.0.join("config.json")).unwrap().contains(&jondisboard::theme::GROUP_COLORS[4].hex()));
    app(h, |a, _| a.delete_group("Lines"));
    frame(h);
    check("deleting a group keeps its sounds (in No group)", lib.0.join("sounds/Crickets.wav").exists() && !lib.0.join("sounds/Lines").exists());

    // --- sort A-Z
    app(h, |a, _| a.sort_az());
    let names = app(h, |a, _| a.clips.iter().map(|c| c.stem.to_lowercase()).collect::<Vec<_>>());
    let mut sorted = names.clone();
    sorted.sort();
    check("Sort A–Z", names == sorted);

    // --- settings toggles and volumes
    app(h, |a, _| a.open_settings(true));
    frame(h);
    frame(h);
    let solo_hit = app(h, |a, _| a.hits.iter().find(|(_, h)| *h == Hit::Opt(true)).map(|(r, _)| (r.cx(), r.cy())));
    if let Some((sx, sy)) = solo_hit {
        click(h, sx, sy);
    }
    check("One sound at a time toggles", app(h, |a, _| a.cfg.solo && a.player.solo.load(std::sync::atomic::Ordering::Relaxed)));
    app(h, |a, _| a.key_down(0x1B, false, false));
    frame(h);
    check("Esc closes Settings", app(h, |a, _| !a.settings_open));
    app(h, |a, _| a.set_volume(1, 1.2));
    check("Clips volume reaches the mixer", app(h, |a, _| (a.player.levels.clips.get() - 1.2).abs() < 1e-6));

    // --- menus: right-click a tile, Esc closes
    let ding_xy = {
        frame(h);
        app(h, |a, _| tile_center(a, ding))
    };
    app(h, |a, _| a.right_click(ding_xy.0, ding_xy.1));
    frame(h);
    check("right-click opens the clip menu", app(h, |a, _| a.menus.len() == 1));
    let sub = app(h, |a, _| a.menus[0].rows.get(5).map(|r| (r.cx(), r.cy())));
    if let Some((mx, my)) = sub {
        app(h, |a, g| a.mouse_move(g, mx, my));
        frame(h);
        check("hovering 'Move to group' opens its submenu", app(h, |a, _| a.menus.len() == 2));
    }
    app(h, |a, _| a.key_down(0x1B, false, false));
    frame(h);
    check("Esc closes menus", app(h, |a, _| a.menus.is_empty()));
    // a click that only closes a menu doesn't play
    app(h, |a, _| {
        a.played.clear();
        a.right_click(ding_xy.0, ding_xy.1);
    });
    frame(h);
    // click another tile, outside the menu
    let other = app(h, |a, _| {
        let m = a.menus[0].rect;
        a.layout.tiles.iter().map(|(id, _)| (*id, tile_center(a, *id))).find(|(_, (x, y))| !m.contains(*x, *y) && *y < a.layout.board.bottom()).unwrap()
    });
    click(h, other.1 .0, other.1 .1);
    window::pump(60);
    check("a click that closes a menu doesn't play", app(h, |a, _| a.played.is_empty() && a.menus.is_empty()));

    // --- theme: instant, nothing lost
    let before = app(h, |a, _| (a.cfg.group.clone(), a.clips.len(), a.query.clone()));
    let t0 = Instant::now();
    app(h, |a, _| a.set_theme("Ocean Blue"));
    frame(h);
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    check("theme switches in place", app(h, |a, _| a.theme().name == "Ocean Blue" && a.cfg.theme == "Ocean Blue"));
    check("nothing is lost on a theme switch", app(h, |a, _| (a.cfg.group.clone(), a.clips.len(), a.query.clone())) == before);
    println!("  theme switch + 2 pumped frames: {ms:.1} ms");

    // --- quick import: no link, then a link (finished by hand: no network in tests)
    jondisboard::ui::shell::set_test_clipboard("nothing useful");
    app(h, |a, _| a.quick_import());
    check("Quick import without a link says what to do", app(h, |a, _| a.toast.as_ref().map(|t| t.text.contains("Copy an Instagram")).unwrap_or(false) && a.jobs.is_empty()));
    jondisboard::ui::shell::set_test_clipboard("look https://youtu.be/abc123 lol");
    app(h, |a, _| a.show_group(ALL));
    app(h, |a, _| {
        a.paths.exe_dir = std::env::temp_dir().join("jb_no_tools_here");
        a.quick_import()
    });
    frame(h);
    check("a pending tile appears straight away", app(h, |a, _| a.jobs.len() == 1 && a.layout.jobs == 1));
    window::pump(1500);
    // (with no yt-dlp on this PC's PATH it fails; with one it may still be running)
    let failed = app(h, |a, _| a.jobs.first().map(|j| j.error.is_some()).unwrap_or(false));
    if failed {
        let id = app(h, |a, _| a.jobs[0].id);
        app(h, |a, _| a.dismiss_job(id));
        check("a failed import can be dismissed", app(h, |a, _| a.jobs.is_empty()));
    } else {
        // finish it by hand with a file, like a download would
        let id = app(h, |a, _| a.jobs[0].id);
        let out = lib.0.join("sounds/Imported.wav");
        tone(&out, 1.0, 500.0);
        app(h, |a, _| a.job_done(id, Ok(out.clone())));
        check("a finished import becomes a tile", app(h, |a, _| a.jobs.is_empty() && a.id_of("Imported.wav").is_some()));
    }

    // --- Import a sound: the Group dropdown, a new group made from the dialog, the result shown
    app(h, |a, _| a.show_group("Memes"));
    app(h, |a, _| a.import_dialog());
    frame(h);
    check("the import dialog starts on the open group", app(h, |a, _| matches!(&a.dialog, Some(Dialog { kind: DialogKind::Import { group, .. }, .. }) if group == "Memes")));
    let gr = app(h, |a, _| a.layout.dialog_group);
    click(h, gr.cx(), gr.cy());
    let (labels, groups) = app(h, |a, _| {
        let labels: Vec<String> = a.menus.first().map(|m| m.items.iter().filter_map(|it| match it {
            MenuItem::Item { text, .. } => Some(text.clone()),
            _ => None,
        }).collect()).unwrap_or_default();
        (labels, a.groups.clone())
    });
    check("the Group list opens", !labels.is_empty());
    check("it lists No group, every group and New group…", labels.first().map(String::as_str) == Some("No group") && groups.iter().all(|g| labels.contains(g)) && labels.last().map(String::as_str) == Some("New group…"));
    let row = |a: &App, label: &str| -> (f32, f32) {
        let m = &a.menus[0];
        let i = m.items.iter().position(|it| matches!(it, MenuItem::Item { text, .. } if text == label)).unwrap();
        (m.rows[i].cx(), m.rows[i].cy())
    };
    let (rx, ry) = app(h, |a, _| row(a, "No group"));
    check("the list is drawn over the dialog (its rows take clicks)", app(h, |a, _| matches!(a.hit_at(rx, ry), Some(Hit::Menu(0, _)))));
    click(h, rx, ry);
    check("picking a row sets the group and closes the list", app(h, |a, _| a.menus.is_empty() && matches!(&a.dialog, Some(Dialog { kind: DialogKind::Import { group, .. }, .. }) if group.is_empty())));
    click(h, gr.cx(), gr.cy());
    click(h, gr.cx(), gr.cy());
    check("clicking the Group control again closes its list", app(h, |a, _| a.menus.is_empty() && a.dialog.is_some()));
    click(h, gr.cx(), gr.cy());
    let (nx, ny) = app(h, |a, _| row(a, "New group…"));
    click(h, nx, ny);
    check("New group… turns the control into a focused name field", app(h, |a, _| matches!(&a.dialog, Some(d) if d.fields.len() == 5 && d.focus == 4 && matches!(d.kind, DialogKind::Import { new_group: true, .. }))));
    app(h, |a, _| {
        for c in "Fresh finds".chars() {
            a.char_typed(c);
        }
        a.dialog.as_mut().unwrap().fields[0].set("https://youtu.be/abc123");
        a.dialog_ok();
    });
    frame(h);
    check("importing makes the new group's folder", lib.0.join("sounds/Fresh finds").is_dir());
    check("and its tab, straight away", app(h, |a, _| a.groups.iter().any(|g| g == "Fresh finds")));
    check("the import goes into it", app(h, |a, _| a.import_target.as_deref() == Some("Fresh finds")));
    // finish it by hand, as a download would
    let out = lib.0.join("sounds/Fresh finds/Fresh clip.wav");
    tone(&out, 1.0, 640.0);
    app(h, |a, _| a.dialog_import_done(Ok(out.clone())));
    frame(h);
    check("the dialog closes on success", app(h, |a, _| a.dialog.is_none()));
    check("the board switches to the sound's group", app(h, |a, _| a.view_key() == "Fresh finds"));
    check("the new sound is on the board", app(h, |a, _| a.id_of("Fresh finds/Fresh clip.wav").map(|id| a.visible().contains(&id)).unwrap_or(false)));
    check("and the toast says where it went", app(h, |a, _| a.toast.as_ref().map(|t| t.text.contains("to Fresh finds")).unwrap_or(false)));
    window::pump(300);

    // --- menus work from the keyboard
    app(h, |a, _| a.show_group(ALL));
    frame(h);
    let gong_like = app(h, |a, _| a.visible()[0]);
    let (gx, gy) = app(h, |a, _| tile_center(a, gong_like));
    app(h, |a, _| a.right_click(gx, gy));
    frame(h);
    const DOWN: u32 = 0x28;
    const ENTER: u32 = 0x0D;
    app(h, |a, _| a.key_down(DOWN, false, false));
    check("Down lights the first row of a menu", app(h, |a, _| a.menus.first().and_then(|m| m.hover).is_some()));
    let first_label = app(h, |a, _| match a.menus[0].hover.and_then(|i| a.menus[0].items.get(i)) {
        Some(MenuItem::Item { text, .. }) => text.clone(),
        _ => String::new(),
    });
    app(h, |a, _| a.played.clear());
    if first_label == "Play" {
        app(h, |a, _| a.key_down(ENTER, false, false));
        window::pump(80);
        check("Enter runs the lit row", app(h, |a, _| a.menus.is_empty() && a.played == vec![gong_like]));
    } else {
        app(h, |a, _| a.close_menus());
        println!("  (first clip-menu row is '{first_label}', Enter check skipped)");
    }

    // --- a name dialog doesn't vanish on an empty name
    app(h, |a, _| a.run(Action::Rename(gong_like)));
    app(h, |a, _| {
        a.dialog.as_mut().unwrap().fields[0].set("   ");
        a.dialog_ok();
    });
    check("an empty name keeps the dialog open and says why", app(h, |a, _| a.dialog.as_ref().map(|d| d.message.is_some()).unwrap_or(false)));
    app(h, |a, _| a.close_dialog());
    frame(h);

    // --- sounds added in Explorer show up when the window comes back
    let cfg_before = std::fs::metadata(lib.0.join("config.json")).unwrap().modified().unwrap();
    app(h, |a, _| a.refresh_if_changed());
    check("nothing changed on disk: no reload, no config write", std::fs::metadata(lib.0.join("config.json")).unwrap().modified().unwrap() == cfg_before);
    tone(&lib.0.join("sounds/Dropped in.wav"), 0.5, 700.0);
    app(h, |a, _| a.refresh_if_changed());
    check("a file added outside the app appears", app(h, |a, _| a.id_of("Dropped in.wav").is_some()));
    std::fs::remove_file(lib.0.join("sounds/Dropped in.wav")).unwrap();
    app(h, |a, _| a.refresh_if_changed());
    check("and goes when it's removed", app(h, |a, _| a.id_of("Dropped in.wav").is_none()));

    // --- Add files / files dropped from Explorer: into the open group, with feedback
    let outside = std::env::temp_dir().join(format!("jb_ui_drop_{}", std::process::id()));
    std::fs::create_dir_all(&outside).unwrap();
    tone(&outside.join("Picked.wav"), 0.5, 520.0);
    std::fs::write(outside.join("notes.txt"), b"not a sound").unwrap();
    app(h, |a, _| {
        a.show_group("Memes");
        a.add_paths(&[outside.join("Picked.wav"), outside.join("notes.txt")]);
    });
    check("an added sound goes into the open group", lib.0.join("sounds/Memes/Picked.wav").exists() && app(h, |a, _| a.id_of("Memes/Picked.wav").is_some()));
    check("a non-sound file isn't copied in", !lib.0.join("sounds/Memes/notes.txt").exists());
    check("and the toast says both", app(h, |a, _| a.toast.as_ref().map(|t| t.text.contains("Added 1 to Memes") && t.text.contains("notes.txt")).unwrap_or(false)));
    let _ = std::fs::remove_dir_all(&outside);

    // --- the config keeps keys this version doesn't know
    check("unknown config keys survive", std::fs::read_to_string(lib.0.join("config.json")).unwrap().contains("unknown_key"));

    window::close(h);
    window::pump(50);
    check("closing saves the window spot", std::fs::read_to_string(lib.0.join("config.json")).unwrap().contains("geometry"));
}
