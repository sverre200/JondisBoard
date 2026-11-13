//! Updates from GitHub Releases (the same scheme as JondisClip's).
//!
//! The check is one anonymous HTTPS GET for the latest release's version; it sends nothing about
//! this PC, its sounds or its settings. Installing is always the user's click. Windows' own
//! `curl.exe` and `tar.exe` (in System32 since Windows 10 1803) do the network and unzip work, so
//! the app gains no HTTP or zip code. Only the program files are ever replaced: `sounds\`,
//! `config.json` and the log are never touched.

use std::fs;
use std::io;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// `owner/name` of the GitHub repository that publishes the releases.
pub const REPO: &str = "sverre200/JondisBoard";
/// Release assets must come from here; anything else in the API answer is refused.
const DOWNLOAD_PREFIX: &str = "https://github.com/sverre200/JondisBoard/releases/download/";
const ASSET_SUFFIX: &str = "-win64.zip";
/// What a release must contain before it's allowed to replace anything.
const REQUIRED: &str = "JondisBoard.exe";
/// The only kinds of file an update may put in the install folder.
const PROGRAM_EXTS: [&str; 2] = ["exe", "txt"];
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// This build's version (Cargo.toml).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// Without the leading "v", e.g. "2.1.0".
    pub version: String,
    pub zip_url: String,
    /// The zip's size in bytes, when GitHub says (for download progress).
    pub size: Option<u64>,
}

/// "v2.1.0" or "2.1.0" as (major, minor, patch).
pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let s = s.trim();
    let mut parts = s.strip_prefix('v').unwrap_or(s).split('.');
    let v = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
    parts.next().is_none().then_some(v)
}

/// The release in a GitHub `releases/latest` answer, if it's newer than `current`.
pub fn newer_release(json: &str, current: &str) -> Option<Release> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let latest = parse_version(v.get("tag_name")?.as_str()?)?;
    if latest <= parse_version(current)? {
        return None;
    }
    let (zip_url, size) = v.get("assets")?.as_array()?.iter().find_map(|a| {
        let url = a.get("browser_download_url")?.as_str()?;
        let name = url.strip_prefix(DOWNLOAD_PREFIX)?;
        // only our own plain file names: nothing curl could read as an option or a path
        let plain = name.chars().all(|c| c.is_ascii_alphanumeric() || "/.-_".contains(c));
        (plain && !name.contains("..") && name.ends_with(ASSET_SUFFIX)).then(|| {
            let size = a.get("size").and_then(serde_json::Value::as_u64);
            (url.to_string(), size.filter(|s| *s > 0))
        })
    })?;
    Some(Release { version: format!("{}.{}.{}", latest.0, latest.1, latest.2), zip_url, size })
}

/// File the download is written to under the work folder.
const ZIP_NAME: &str = "update.zip";

/// Where downloads are unpacked: %LOCALAPPDATA%\JondisBoard\update.
pub fn work_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("JondisBoard").join("update"))
}

/// How far the download of `release` into `work_dir` has come, 0 to 99 (`None` when the size
/// isn't known). Reads only the file's size.
pub fn download_progress(release: &Release, work_dir: &Path) -> Option<u8> {
    let total = release.size?;
    let have = fs::metadata(work_dir.join(ZIP_NAME)).map_or(0, |m| m.len());
    Some((have.saturating_mul(100) / total).min(99) as u8)
}

fn system_tool(name: &str) -> PathBuf {
    // the full path, so a `curl.exe` dropped next to the app is never picked up
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    PathBuf::from(root).join("System32").join(name)
}

fn run(tool: &str, args: &[&std::ffi::OsStr]) -> Result<Vec<u8>, String> {
    let out = Command::new(system_tool(tool)).args(args).creation_flags(CREATE_NO_WINDOW).output().map_err(|e| format!("{tool}: {e}"))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(format!("{tool} failed ({}): {}", out.status, err.trim()))
    }
}

/// Asks GitHub for the latest release. `Ok(None)` means up to date.
pub fn check(current: &str) -> Result<Option<Release>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let args: [&std::ffi::OsStr; 12] = [
        "--silent".as_ref(),
        "--show-error".as_ref(),
        "--fail".as_ref(),
        "--location".as_ref(),
        "--proto".as_ref(),
        "=https".as_ref(),
        "--max-time".as_ref(),
        "20".as_ref(),
        "--user-agent".as_ref(),
        "JondisBoard".as_ref(),
        "--header".as_ref(),
        "Accept: application/vnd.github+json".as_ref(),
    ];
    let mut all: Vec<&std::ffi::OsStr> = args.to_vec();
    all.push(url.as_ref());
    let out = run("curl.exe", &all)?;
    Ok(newer_release(&String::from_utf8_lossy(&out), current))
}

/// Downloads and unpacks `release` under `work_dir` (wiped first). Returns the folder holding
/// the new files. Nothing in the install folder is touched.
pub fn download(release: &Release, work_dir: &Path) -> Result<PathBuf, String> {
    let _ = fs::remove_dir_all(work_dir);
    fs::create_dir_all(work_dir).map_err(|e| format!("{}: {e}", work_dir.display()))?;
    let zip = work_dir.join(ZIP_NAME);
    run(
        "curl.exe",
        &[
            "--silent".as_ref(),
            "--show-error".as_ref(),
            "--fail".as_ref(),
            "--location".as_ref(),
            "--proto".as_ref(),
            "=https".as_ref(),
            "--max-time".as_ref(),
            "900".as_ref(),
            "--user-agent".as_ref(),
            "JondisBoard".as_ref(),
            "--output".as_ref(),
            zip.as_os_str(),
            release.zip_url.as_ref(),
        ],
    )?;
    run("tar.exe", &["-xf".as_ref(), zip.as_os_str(), "-C".as_ref(), work_dir.as_os_str()])?;
    let _ = fs::remove_file(&zip);
    // the zip holds one top folder ("JondisBoard"); accept a flat zip too
    [work_dir.join("JondisBoard"), work_dir.to_path_buf()]
        .into_iter()
        .find(|d| d.join(REQUIRED).is_file())
        .ok_or_else(|| "the downloaded update is missing files".to_string())
}

/// The program files in `staged`: top-level .exe and .txt only. Whatever else a zip holds (a
/// `sounds` folder, a config) never reaches the install folder.
fn program_files(staged: &Path) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(staged)? {
        let entry = entry?;
        let name = PathBuf::from(entry.file_name());
        let ok = name.extension().and_then(|e| e.to_str()).map(|e| PROGRAM_EXTS.iter().any(|p| e.eq_ignore_ascii_case(p))).unwrap_or(false);
        if entry.file_type()?.is_file() && ok {
            out.push(name);
        }
    }
    Ok(out)
}

fn old_name(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".old");
    PathBuf::from(s)
}

/// Replaces the program files in `install_dir` with those in `staged`. A running exe can't be
/// overwritten on Windows but can be renamed, so each file is moved aside to `<name>.old` first;
/// [`cleanup`] removes those on the next start. If anything fails, every file is put back.
pub fn apply(staged: &Path, install_dir: &Path) -> Result<(), String> {
    let files = program_files(staged).map_err(|e| e.to_string())?;
    if !files.iter().any(|f| f.as_os_str().eq_ignore_ascii_case(REQUIRED)) {
        return Err("the downloaded update is missing files".into());
    }
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut written: Vec<PathBuf> = Vec::new();
    let result = (|| -> io::Result<()> {
        for rel in &files {
            let dest = install_dir.join(rel);
            if dest.exists() {
                let old = old_name(&dest);
                if old.exists() {
                    fs::remove_file(&old)?;
                }
                fs::rename(&dest, &old)?;
                moved.push((old, dest.clone()));
            }
            written.push(dest.clone());
            fs::copy(staged.join(rel), &dest)?;
        }
        Ok(())
    })();
    if let Err(e) = result {
        for dest in &written {
            let _ = fs::remove_file(dest);
        }
        for (old, dest) in &moved {
            let _ = fs::rename(old, dest);
        }
        return Err(format!("could not replace the app files: {e}"));
    }
    Ok(())
}

/// Removes what an earlier update left behind: `*.old` files in the install folder and the
/// download folder. Files still in use are skipped silently.
pub fn cleanup(install_dir: &Path, work_dir: &Path) {
    let _ = fs::remove_dir_all(work_dir);
    let Ok(entries) = fs::read_dir(install_dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "old") {
            let _ = fs::remove_file(path);
        }
    }
}

/// True for a developer build run from Cargo's `target` folder, which must never replace itself
/// with a packaged release.
pub fn is_dev_install(install_dir: &Path) -> bool {
    install_dir.components().any(|c| c.as_os_str().eq_ignore_ascii_case("target"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("jb_update_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn versions_parse_and_compare() {
        assert_eq!(parse_version("v2.1.0"), Some((2, 1, 0)));
        assert_eq!(parse_version("2.10.3"), Some((2, 10, 3)));
        assert_eq!(parse_version("2.1"), None);
        assert_eq!(parse_version("2.1.0.4"), None);
        assert_eq!(parse_version("two"), None);
        assert!(parse_version("2.10.0") > parse_version("2.9.9"));
    }

    fn answer(tag: &str, url: &str) -> String {
        format!(r#"{{"tag_name": "{tag}", "assets": [{{"browser_download_url": "{url}", "size": 1000}}]}}"#)
    }

    #[test]
    fn offers_only_newer_releases_from_our_repo() {
        let good = "https://github.com/sverre200/JondisBoard/releases/download/v2.1.0/JondisBoard-2.1.0-win64.zip";
        let r = newer_release(&answer("v2.1.0", good), "2.0.0").expect("a newer release");
        assert_eq!((r.version.as_str(), r.zip_url.as_str(), r.size), ("2.1.0", good, Some(1000)));
        assert!(newer_release(&answer("v2.0.0", good), "2.0.0").is_none(), "the same version isn't an update");
        assert!(newer_release(&answer("v1.9.0", good), "2.0.0").is_none(), "nor is an older one");
        for bad in [
            "https://github.com/someone-else/JondisBoard/releases/download/v2.1.0/JondisBoard-2.1.0-win64.zip",
            "http://github.com/sverre200/JondisBoard/releases/download/v2.1.0/JondisBoard-2.1.0-win64.zip",
            "https://github.com/sverre200/JondisBoard/releases/download/v2.1.0/JondisBoard.exe",
            "https://github.com/sverre200/JondisBoard/releases/download/v2.1.0/../../x-win64.zip",
            "https://github.com/sverre200/JondisBoard/releases/download/v2.1.0/a b-win64.zip",
        ] {
            assert!(newer_release(&answer("v2.1.0", bad), "2.0.0").is_none(), "{bad}");
        }
        assert!(newer_release("not json", "2.0.0").is_none());
    }

    #[test]
    fn download_progress_follows_the_file_size() {
        let d = temp("progress");
        let r = Release { version: "2.1.0".into(), zip_url: String::new(), size: Some(200) };
        assert_eq!(download_progress(&r, &d), Some(0));
        fs::write(d.join(ZIP_NAME), vec![0u8; 50]).unwrap();
        assert_eq!(download_progress(&r, &d), Some(25));
        fs::write(d.join(ZIP_NAME), vec![0u8; 200]).unwrap();
        assert_eq!(download_progress(&r, &d), Some(99));
        assert_eq!(download_progress(&Release { size: None, ..r }, &d), None);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn apply_swaps_program_files_and_never_touches_sounds_or_settings() {
        let (staged, install) = (temp("staged"), temp("install"));
        fs::write(staged.join("JondisBoard.exe"), b"new exe").unwrap();
        fs::write(staged.join("ffmpeg.exe"), b"new ffmpeg").unwrap();
        fs::write(staged.join("READ ME FIRST.txt"), b"new readme").unwrap();
        // things a zip must never be able to overwrite
        fs::write(staged.join("config.json"), b"{\"evil\": true}").unwrap();
        fs::create_dir_all(staged.join("sounds")).unwrap();
        fs::write(staged.join("sounds").join("Mine.wav"), b"replaced").unwrap();

        fs::write(install.join("JondisBoard.exe"), b"old exe").unwrap();
        fs::write(install.join("config.json"), b"{\"mine\": true}").unwrap();
        fs::create_dir_all(install.join("sounds")).unwrap();
        fs::write(install.join("sounds").join("Mine.wav"), b"my sound").unwrap();

        apply(&staged, &install).unwrap();
        assert_eq!(fs::read(install.join("JondisBoard.exe")).unwrap(), b"new exe");
        assert_eq!(fs::read(install.join("JondisBoard.exe.old")).unwrap(), b"old exe");
        assert_eq!(fs::read(install.join("ffmpeg.exe")).unwrap(), b"new ffmpeg");
        assert_eq!(fs::read(install.join("config.json")).unwrap(), b"{\"mine\": true}");
        assert_eq!(fs::read(install.join("sounds").join("Mine.wav")).unwrap(), b"my sound");

        let work = temp("work");
        cleanup(&install, &work);
        assert!(!install.join("JondisBoard.exe.old").exists() && !work.exists());
        assert!(install.join("sounds").join("Mine.wav").exists());
        for d in [staged, install] {
            let _ = fs::remove_dir_all(d);
        }
    }

    #[test]
    fn a_zip_without_the_app_replaces_nothing() {
        let (staged, install) = (temp("staged_bad"), temp("install_bad"));
        fs::write(staged.join("ffmpeg.exe"), b"new ffmpeg").unwrap();
        fs::write(install.join("ffmpeg.exe"), b"old ffmpeg").unwrap();
        assert!(apply(&staged, &install).is_err());
        assert_eq!(fs::read(install.join("ffmpeg.exe")).unwrap(), b"old ffmpeg");
        for d in [staged, install] {
            let _ = fs::remove_dir_all(d);
        }
    }

    /// The real thing against GitHub (needs the internet and a published release newer than
    /// 2.0.0): `cargo test --release --lib real_release -- --ignored --nocapture`. It installs
    /// into a throwaway folder; nothing on this PC is replaced.
    #[test]
    #[ignore]
    fn real_release_is_found_downloaded_and_swapped_in() {
        let rel = check("2.0.0").expect("GitHub answers").expect("a release newer than 2.0.0");
        println!("offered: {} ({:?} bytes) {}", rel.version, rel.size, rel.zip_url);
        assert!(check("99.0.0").expect("GitHub answers").is_none(), "nothing is newer than 99.0.0");
        let (work, install) = (temp("real_work"), temp("real_install"));
        fs::write(install.join("JondisBoard.exe"), b"old exe").unwrap();
        fs::write(install.join("config.json"), b"{\"mine\": true}").unwrap();
        fs::create_dir_all(install.join("sounds")).unwrap();
        fs::write(install.join("sounds").join("Mine.wav"), b"my sound").unwrap();
        let staged = download(&rel, &work).expect("download and unpack");
        apply(&staged, &install).expect("swap");
        let exe = fs::metadata(install.join("JondisBoard.exe")).unwrap().len();
        println!("installed JondisBoard.exe: {exe} bytes; files: {:?}", fs::read_dir(&install).unwrap().flatten().map(|e| e.file_name()).collect::<Vec<_>>());
        assert!(exe > 500_000, "the real exe");
        assert!(install.join("ffmpeg.exe").is_file() && install.join("yt-dlp.exe").is_file());
        assert_eq!(fs::read(install.join("JondisBoard.exe.old")).unwrap(), b"old exe");
        assert_eq!(fs::read(install.join("config.json")).unwrap(), b"{\"mine\": true}");
        assert_eq!(fs::read(install.join("sounds").join("Mine.wav")).unwrap(), b"my sound");
        cleanup(&install, &work);
        assert!(!install.join("JondisBoard.exe.old").exists() && !work.exists());
        let _ = fs::remove_dir_all(&install);
    }

    #[test]
    fn dev_builds_are_recognised() {
        assert!(is_dev_install(Path::new(r"D:\Claude-Projects\JondisBoard\rust\target\release")));
        assert!(!is_dev_install(Path::new(r"D:\Claude-Projects\JondisBoard")));
        assert!(!is_dev_install(Path::new(r"C:\Users\x\Documents\JondisBoard")));
    }
}
