//! Visual snapshots: the real app drawn offscreen into PNGs, one per state, for looking at
//! (nothing appears on screen; no audio, no clipboard). Set JB_SNAP_DIR to keep them somewhere;
//! by default they go to %TEMP%\jb_snapshots.
//!
//!     cargo test --test ui_snapshots -- --nocapture

use jondisboard::audio::wav;
use jondisboard::paths::Paths;
use jondisboard::ui::app::{Action, App, DialogKind, HotkeyTarget, Job};
use jondisboard::ui::gfx::Gfx;
use jondisboard::ui::window;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::HWND;

fn tone(path: &Path, secs: f32) {
    let data: Vec<i16> = (0..(48000.0 * secs) as usize).flat_map(|i| {
        let v = ((i as f32 * 0.05).sin() * 2000.0) as i16;
        [v, v]
    }).collect();
    wav::write_i16(path, 48000, 2, &data).unwrap();
}

fn png(path: &Path, rgb: [u8; 3]) {
    // a tiny valid PNG: 1x1 pixel, scaled up by the app
    let mut raw = vec![0u8];
    raw.extend_from_slice(&rgb);
    // zlib "stored" block
    let mut z = vec![0x78, 0x01, 1, raw.len() as u8, 0, !(raw.len() as u8), 0xff];
    z.extend_from_slice(&raw);
    let (mut a, mut b) = (1u32, 0u32);
    for x in &raw {
        a = (a + *x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    fn crc(data: &[u8]) -> u32 {
        let mut c = 0xffffffffu32;
        for &x in data {
            c ^= x as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xedb88320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    let chunk = |t: &[u8], d: &[u8]| {
        let mut v = (d.len() as u32).to_be_bytes().to_vec();
        let mut td = t.to_vec();
        td.extend_from_slice(d);
        v.extend_from_slice(&td);
        v.extend_from_slice(&crc(&td).to_be_bytes());
        v
    };
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    out.extend(chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0]));
    out.extend(chunk(b"IDAT", &z));
    out.extend(chunk(b"IEND", &[]));
    std::fs::write(path, out).unwrap();
}

fn app<R>(h: HWND, f: impl FnOnce(&mut App, &mut Gfx) -> R) -> R {
    window::with_app(h, f).expect("app")
}

fn settle(h: HWND, ms: u64) {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        window::invalidate(h);
        window::pump(20);
    }
}

fn snap(h: HWND, dir: &Path, name: &str) {
    settle(h, 260);
    let p = dir.join(format!("{name}.png"));
    window::snapshot(h, &p).expect("snapshot");
    println!("snapshot {}", p.display());
}

#[test]
fn snapshots() {
    std::env::set_var("JONDISBOARD_TEST", "1");
    let dir = std::env::var_os("JB_SNAP_DIR").map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("jb_snapshots"));
    std::fs::create_dir_all(&dir).unwrap();
    let lib = std::env::temp_dir().join(format!("jb_snaplib_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&lib);
    let s = lib.join("sounds");
    for g in ["Memes", "THE LION"] {
        std::fs::create_dir_all(s.join(g)).unwrap();
    }
    let names = [
        ("Memes", "Air horn"), ("Memes", "Bruh"), ("Memes", "Vine boom"), ("THE LION", "To the gossiper"), ("THE LION", "Roar"),
        ("", "Applause"), ("", "Chinese Rap"), ("", "Drum roll with a really long name that has to wrap onto two lines"), ("", "Evil laugh"), ("", "Jazz"),
    ];
    for (i, (g, n)) in names.iter().enumerate() {
        let p = if g.is_empty() { s.join(format!("{n}.wav")) } else { s.join(g).join(format!("{n}.wav")) };
        tone(&p, 1.0 + i as f32 * 3.7);
        if i % 3 == 0 {
            png(&p.with_extension("png"), [(i * 60 % 255) as u8, 120, 200]);
        }
    }
    std::fs::write(lib.join("config.json"), r##"{"group": "*", "theme": "Toxic Green", "geometry": "1180x760+40+30",
        "group_colors": {"Memes": "#3b82f6", "THE LION": "#ef4444"},
        "hotkeys": {"Memes/Bruh.wav": {"scan": 63, "keypad": false, "mods": [], "label": "f5"},
                    "Jazz.wav": {"scan": 79, "keypad": true, "mods": ["ctrl"], "label": "num 1"}},
        "stop_hotkey": {"scan": 88, "keypad": false, "mods": [], "label": "f12"}}"##).unwrap();
    let h = window::create(Paths::new(Some(lib.clone())), Instant::now(), None).expect("window");
    settle(h, 600);
    snap(h, &dir, "01_board");

    // playing + flash
    let bruh = app(h, |a, _| a.id_of("Memes/Bruh.wav").unwrap());
    app(h, |a, _| {
        a.playing.insert(bruh, 0.42);
        a.flash_tile(bruh, 0.32);
    });
    window::pump(60);
    let p = dir.join("02_playing_flash.png");
    app(h, |a, _| a.playing.insert(bruh, 0.42));
    window::snapshot(h, &p).unwrap();
    app(h, |a, _| a.playing.clear());

    // the clip menu with its submenu
    let jazz = app(h, |a, _| a.id_of("Jazz.wav").unwrap());
    let (x, y) = app(h, |a, _| {
        let (_, r) = a.layout.tiles.iter().find(|(i, _)| *i == jazz).copied().unwrap();
        (r.cx(), r.cy() + a.layout.board.y)
    });
    app(h, |a, _| a.right_click(x, y));
    settle(h, 200);
    let row = app(h, |a, _| a.menus[0].rows[5]);
    app(h, |a, g| a.mouse_move(g, row.cx(), row.cy()));
    snap(h, &dir, "03_clip_menu");
    app(h, |a, _| a.close_menus());

    // settings
    app(h, |a, _| a.open_settings(true));
    snap(h, &dir, "04_settings");
    app(h, |a, _| a.open_settings(false));

    // dialogs
    app(h, |a, _| a.run(Action::Rename(jazz)));
    snap(h, &dir, "05_rename");
    app(h, |a, _| a.close_dialog());
    app(h, |a, _| a.import_dialog());
    snap(h, &dir, "06_import");
    app(h, |a, _| {
        if let Some(d) = &mut a.dialog {
            d.fields[0].set("https://youtu.be/abc");
            if let DialogKind::Import { busy, started, .. } = &mut d.kind {
                *busy = true;
                *started = Some(Instant::now());
            }
            d.message = Some(("Downloading…".into(), a.themes.get().muted));
        }
    });
    snap(h, &dir, "07_import_busy");
    app(h, |a, _| a.close_dialog());
    app(h, |a, _| a.record_hotkey(HotkeyTarget::Clip(jazz)));
    snap(h, &dir, "08_hotkey");
    app(h, |a, _| a.close_dialog());
    app(h, |a, _| a.run(Action::Delete(jazz)));
    snap(h, &dir, "09_confirm_delete");
    app(h, |a, _| a.close_dialog());

    // pending imports (one running, one failed) + a toast
    app(h, |a, _| {
        a.jobs.push(Job { id: 900, url: "https://www.instagram.com/reel/x/".into(), source: "Instagram reel", status: "Downloading", error: None, full_error: None, started: Instant::now() });
        a.jobs.push(Job { id: 901, url: "https://youtu.be/y".into(), source: "YouTube video", status: "Starting", error: Some("Video unavailable".into()), full_error: Some("ERROR: Video unavailable".into()), started: Instant::now() });
        a.toast("Added 'Chinese Rap'", jondisboard::ui::app::Level::Info);
    });
    snap(h, &dir, "10_pending_and_toast");
    app(h, |a, _| {
        a.jobs.clear();
        a.close_toast();
    });

    // drag in progress, over the board and over a tab
    let air = app(h, |a, _| a.id_of("Memes/Air horn.wav").unwrap());
    let (ax, ay) = app(h, |a, _| {
        let (_, r) = a.layout.tiles.iter().find(|(i, _)| *i == air).copied().unwrap();
        (r.cx(), r.cy() + a.layout.board.y)
    });
    app(h, |a, g| {
        a.mouse_move(g, ax, ay);
        a.mouse_down(g, ax, ay, false, false);
        for i in 1..=10 {
            a.mouse_move(g, ax + 60.0 * i as f32, ay + 30.0 * i as f32);
        }
    });
    snap(h, &dir, "11_drag_board");
    let tab = app(h, |a, _| a.layout.tabs.iter().find(|(k, _)| k == "THE LION").map(|(_, r)| *r).unwrap());
    app(h, |a, g| a.mouse_move(g, tab.cx(), tab.cy()));
    snap(h, &dir, "12_drag_tab");
    app(h, |a, _| a.cancel_drag());

    // the theme menu, then every theme
    app(h, |a, g| {
        let r = a.layout.button(&jondisboard::ui::view::Hit::Theme).unwrap();
        a.mouse_move(g, r.cx(), r.cy());
        a.mouse_down(g, r.cx(), r.cy(), false, false);
        a.mouse_up(r.cx(), r.cy());
    });
    snap(h, &dir, "13_theme_menu");
    app(h, |a, _| a.close_menus());
    for (i, t) in jondisboard::theme::all().iter().enumerate() {
        app(h, |a, _| a.set_theme(t.name));
        snap(h, &dir, &format!("14_theme_{i}_{}", t.name.replace(' ', "_")));
    }
    app(h, |a, _| a.set_theme("Toxic Green"));

    // a group, search with no results, the empty board
    app(h, |a, _| a.show_group("THE LION"));
    snap(h, &dir, "15_group");
    app(h, |a, _| {
        a.key_down(0x46, true, false);
        for c in "zzz".chars() {
            a.char_typed(c);
        }
    });
    snap(h, &dir, "16_search_none");
    window::close(h);
    window::pump(50);

    let empty = std::env::temp_dir().join(format!("jb_snapempty_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&empty);
    std::fs::create_dir_all(empty.join("sounds")).unwrap();
    std::fs::write(empty.join("config.json"), r#"{"geometry": "1000x700+40+30"}"#).unwrap();
    let h2 = window::create(Paths::new(Some(empty.clone())), Instant::now(), None).expect("window");
    settle(h2, 400);
    snap(h2, &dir, "17_empty");
    window::close(h2);
    window::pump(50);
    let _ = std::fs::remove_dir_all(&lib);
    let _ = std::fs::remove_dir_all(&empty);
}
