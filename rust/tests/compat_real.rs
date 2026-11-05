//! Compatibility with an existing install: opens a COPY of real user data (sounds/ +
//! config.json written by the Python app) and checks the Rust version reads it as is.
//!
//!     set JB_REAL_COPY=<folder with a copy of sounds\ and config.json>
//!     cargo test --release --test compat_real -- --nocapture
//!
//! Skipped when JB_REAL_COPY isn't set. Never point it at the real folder: closing the app saves
//! config.json (and the first save makes config.backup.json). Test mode: no devices, no
//! clipboard, the window stays hidden.

use jondisboard::config::ConfigFile;
use jondisboard::library;
use jondisboard::paths::Paths;
use jondisboard::ui::window;
use serde_json::Value;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn check(name: &str, ok: bool) {
    println!("{} {name}", if ok { "PASS" } else { "FAIL" });
    assert!(ok, "{name}");
}

#[test]
fn reads_a_python_install() {
    let Some(dir) = std::env::var_os("JB_REAL_COPY").map(PathBuf::from) else {
        println!("JB_REAL_COPY not set: skipped");
        return;
    };
    assert!(!dir.join("JondisBoard.exe").exists() || dir.to_string_lossy().contains("Temp"), "use a copy, not the real install");
    std::env::set_var("JONDISBOARD_TEST", "1");
    let before: Value = serde_json::from_slice(&std::fs::read(dir.join("config.json")).unwrap()).unwrap();
    let scan = library::scan(&dir.join("sounds")).unwrap();
    let t0 = Instant::now();
    let h = window::create(Paths::new(Some(dir.clone())), t0, None).expect("window");
    window::pump(300);
    let (n, groups, order, theme, colors, first_frame) = window::with_app(h, |a, _| {
        (
            a.clips.len(),
            a.groups.clone(),
            a.clips.iter().map(|c| c.name.clone()).collect::<Vec<_>>(),
            a.theme().name.to_string(),
            a.cfg.group_colors.clone(),
            a.timings.iter().find(|(k, _)| *k == "first frame").map(|(_, v)| *v),
        )
    })
    .unwrap();
    println!("  {n} clips, groups {groups:?}, theme {theme}, first frame {first_frame:?} ms");
    check("every clip is on the board", n == scan.clips.len());
    check("groups = folders (not _deleted)", groups == scan.groups);
    check("the theme is kept", before["theme"].as_str().map(|t| t == theme).unwrap_or(true));
    let saved: Vec<String> = before["order"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
    let kept: Vec<&String> = saved.iter().filter(|s| order.contains(s)).collect();
    check("the saved order is kept", order.iter().take(kept.len()).collect::<Vec<_>>() == kept);
    if let Some(obj) = before["group_colors"].as_object() {
        check("group colours are kept", obj.iter().all(|(g, c)| colors.get(g).map(|x| Some(x.as_str()) == c.as_str()).unwrap_or(false)));
    }
    // every clip decodes (in the background); durations match the files
    let t = Instant::now();
    let idle = window::with_app(h, |a, _| a.player.wait_idle(Duration::from_secs(120))).unwrap();
    let (failed, bytes) = window::with_app(h, |a, _| (a.clips.iter().filter(|c| c.failed.is_some()).map(|c| c.name.clone()).collect::<Vec<_>>(), a.player.cached_bytes())).unwrap();
    println!("  decoded everything in {:.0} ms, {:.0} MB in memory", t.elapsed().as_secs_f64() * 1000.0, bytes as f64 / 1048576.0);
    check("the background decoder finishes", idle);
    check("every clip decodes", failed.is_empty());
    window::pump(100);
    let durs = window::with_app(h, |a, _| a.clips.iter().all(|c| c.duration.map(|d| d > 0.0).unwrap_or(false))).unwrap();
    check("every clip has a duration", durs);
    if let Some(snap) = std::env::var_os("JB_SNAP_DIR") {
        let p = PathBuf::from(snap).join("compat_real.png");
        window::snapshot(h, &p).unwrap();
        println!("  snapshot {}", p.display());
    }
    window::close(h);
    window::pump(100);
    // the config after a save: every key still there, with the same values (geometry may move)
    let after: Value = serde_json::from_slice(&std::fs::read(dir.join("config.json")).unwrap()).unwrap();
    let (b, a) = (before.as_object().unwrap(), after.as_object().unwrap());
    let lost: Vec<&String> = b.keys().filter(|k| !a.contains_key(*k)).collect();
    check("no config key is lost", lost.is_empty());
    let changed: Vec<&String> = b.keys().filter(|k| *k != "geometry" && a.get(*k) != b.get(*k)).collect();
    println!("  changed keys: {changed:?}");
    check("no setting changes just by opening and closing", changed.is_empty());
    check("a backup of the old config was made", dir.join("config.backup.json").exists());
    check("the Python app can still read it", ConfigFile::new(dir.join("config.json")).load().1.is_none());
}
