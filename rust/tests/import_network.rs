//! The real link import, end to end (opt-in: JB_NETWORK_IMPORT=1, needs the internet and
//! rust\vendor\yt-dlp.exe + ffmpeg.exe): a trimmed YouTube import into a throwaway group
//! folder, then a link to a video that doesn't exist. Nothing touches the real library.

use jondisboard::audio::wav;
use jondisboard::importer::{self, Request, Tools};
use std::path::PathBuf;

#[test]
fn imports_a_link() {
    if std::env::var_os("JB_NETWORK_IMPORT").is_none() {
        println!("JB_NETWORK_IMPORT not set: skipped");
        return;
    }
    let vendor = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("vendor");
    let tools = Tools { yt_dlp: Some(vendor.join("yt-dlp.exe")), ffmpeg: Some(vendor.join("ffmpeg.exe")) };
    let dir = std::env::temp_dir().join(format!("jb_net_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let folder = dir.join("sounds").join("New group");
    let req = |url: &str| Request {
        url: url.into(),
        name: None,
        start: Some(2.0),
        end: Some(5.0),
        normalize: true,
        cookies_from_browser: None,
        folder: folder.clone(),
    };
    let steps = std::cell::RefCell::new(Vec::<String>::new());
    // "Me at the zoo": the first YouTube video, 19 s
    let imp = importer::import_clip(&req("https://www.youtube.com/watch?v=jNQXAC9IVRw"), &tools, &|s| steps.borrow_mut().push(s.to_string())).expect("import");
    println!("steps {:?}\naudio {}\nthumbnail {:?}", steps.borrow(), imp.audio.display(), imp.thumbnail);
    assert_eq!(imp.audio.parent(), Some(folder.as_path()), "lands in the chosen group's folder");
    assert!(imp.audio.file_stem().unwrap().to_string_lossy().to_lowercase().contains("zoo"), "named from the video's title");
    let info = wav::read_info(&imp.audio).unwrap();
    let secs = info.frames() as f64 / info.rate as f64;
    println!("{} Hz, {} ch, {secs:.2} s", info.rate, info.channels);
    assert_eq!((info.rate, info.channels), (48000, 2));
    assert!((secs - 3.0).abs() < 0.1, "trimmed to start..end");
    assert!(std::fs::read_dir(&folder).unwrap().flatten().all(|e| !e.path().to_string_lossy().ends_with(".part")));
    let _ = std::fs::remove_dir_all(&imp.temp);

    let bad = importer::import_clip(&req("https://www.youtube.com/watch?v=xxxxxxxxxxx"), &tools, &|_| {});
    let e = bad.err().expect("a missing video fails");
    println!("missing video: {}", importer::short_error(&e));
    assert!(!importer::short_error(&e).is_empty());
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 1, "a failed import leaves nothing behind");
    let _ = std::fs::remove_dir_all(&dir);
}
