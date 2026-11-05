//! Turn Instagram reels / YouTube videos into clips, with the bundled `yt-dlp.exe` and
//! `ffmpeg.exe` (standalone helper programs next to JondisBoard.exe; no Python).
//!
//! 1. yt-dlp downloads the best audio and the thumbnail, and prints the video's info.
//! 2. ffmpeg cuts [start, end], converts to 48 kHz stereo WAV with the tuned resampler, and
//!    brings it to -16 LUFS as ONE fixed gain change (two-pass linear loudnorm), so the clip's
//!    dynamics stay as they were.
//! 3. The thumbnail becomes the clip's picture.
//! 4. Name: the Instagram caption without hashtags, or the YouTube title.

use crate::audio::decode::quiet_command;
use crate::library::{safe_name, unique_path};
use crate::paths::RESAMPLE;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Stdio;

pub const LOUDNESS: &str = "I=-16:TP=-1.5:LRA=11";

/// The first Instagram / YouTube link in some text.
pub fn find_link(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find("http") {
        let start = from + i;
        let rest = &lower[start..];
        let after_scheme = rest.strip_prefix("https://").or_else(|| rest.strip_prefix("http://"));
        if let Some(a) = after_scheme {
            let host_part = a.strip_prefix("www.").or_else(|| a.strip_prefix("m.")).or_else(|| a.strip_prefix("music.")).unwrap_or(a);
            for host in ["instagram.com/", "youtube.com/", "youtu.be/"] {
                if let Some(path) = host_part.strip_prefix(host) {
                    if path.chars().next().map(|c| !c.is_whitespace()).unwrap_or(false) {
                        let len = rest.find(char::is_whitespace).unwrap_or(rest.len());
                        return Some(text[start..start + len].to_string());
                    }
                }
            }
        }
        from = start + 4;
    }
    None
}

/// "83", "1:23", "1:23.5", "0:01:23" -> seconds. Empty -> None.
pub fn parse_time(s: &str) -> Result<Option<f64>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() > 3 {
        return Err(format!("Bad time: {s:?}"));
    }
    let mut secs = 0.0;
    for p in parts {
        let v: f64 = p.trim().parse().map_err(|_| format!("Bad time: {s:?}"))?;
        if !v.is_finite() || v < 0.0 {
            return Err(format!("Bad time: {s:?}"));
        }
        secs = secs * 60.0 + v;
    }
    Ok(Some(secs))
}

/// yt-dlp / ffmpeg errors are long; keep the part that says what went wrong.
pub fn short_error(text: &str) -> String {
    let first = text.trim().lines().find(|l| !l.trim().is_empty()).unwrap_or("Something went wrong").trim();
    let mut t = first;
    t = t.strip_prefix("ERROR:").map(str::trim_start).unwrap_or(t);
    if t.starts_with('[') {
        if let Some(i) = t.find(']') {
            t = t[i + 1..].trim_start();
        }
    }
    // "abc123: Video unavailable" -> "Video unavailable" (an id before a colon)
    if let Some(i) = t.find(": ") {
        if t[..i].chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') && i > 0 {
            t = &t[i + 2..];
        }
    }
    let lower = t.to_lowercase();
    if lower.contains("login") || lower.contains("cookies") {
        return "Instagram wants a login (see the README)".into();
    }
    if t.chars().count() > 90 {
        format!("{}…", t.chars().take(90).collect::<String>())
    } else if t.is_empty() {
        "Something went wrong".into()
    } else {
        t.to_string()
    }
}

/// "Instagram reel" / "YouTube video", for the pending tile.
pub fn source_of(url: &str) -> &'static str {
    if url.to_lowercase().contains("instagram") {
        "Instagram reel"
    } else {
        "YouTube video"
    }
}

/// The clip name from yt-dlp's info: the Instagram caption without hashtags, else the title.
pub fn name_from_info(info: &Value) -> String {
    let s = |k: &str| info.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let caption = if s("extractor_key").to_lowercase().starts_with("instagram") {
        let d = s("description");
        let no_tags: String = d.split_whitespace_keep_lines();
        no_tags.lines().next().unwrap_or("").trim().to_string()
    } else {
        s("title")
    };
    let cut: String = caption.chars().take(50).collect();
    let cut = cut.trim().to_string();
    if cut.is_empty() {
        let who = [s("channel"), s("uploader_id")].into_iter().find(|x| !x.is_empty()).unwrap_or_else(|| "clip".into());
        format!("{who}_{}", s("id"))
    } else {
        cut
    }
}

trait StripTags {
    fn split_whitespace_keep_lines(&self) -> String;
}

impl StripTags for String {
    /// Remove #hashtags (a '#' and the non-space run after it), keeping line breaks.
    fn split_whitespace_keep_lines(&self) -> String {
        let mut out = String::with_capacity(self.len());
        let mut skipping = false;
        for c in self.chars() {
            if skipping {
                if c.is_whitespace() {
                    skipping = false;
                    out.push(c);
                }
                continue;
            }
            if c == '#' {
                skipping = true;
                continue;
            }
            out.push(c);
        }
        out
    }
}

pub struct Tools {
    pub yt_dlp: Option<PathBuf>,
    pub ffmpeg: Option<PathBuf>,
}

pub struct Request {
    pub url: String,
    pub name: Option<String>,
    pub start: Option<f64>,
    pub end: Option<f64>,
    pub normalize: bool,
    pub cookies_from_browser: Option<String>,
    pub folder: PathBuf,
}

pub struct Imported {
    pub audio: PathBuf,
    /// the downloaded thumbnail, if any (the caller turns it into the clip's picture)
    pub thumbnail: Option<PathBuf>,
    /// temp folder to remove once the thumbnail is used
    pub temp: PathBuf,
}

fn tail(bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    // yt-dlp's real error is usually the last ERROR line
    lines.iter().rev().find(|l| l.contains("ERROR")).or(lines.last()).map(|l| l.to_string()).unwrap_or_default()
}

/// The ffmpeg input arguments for the cut.
fn cut_args(src: &Path, start: Option<f64>, end: Option<f64>) -> Vec<String> {
    let mut a = Vec::new();
    if let Some(s) = start {
        a.push("-ss".into());
        a.push(format!("{s:.3}"));
    }
    a.push("-i".into());
    a.push(src.to_string_lossy().into_owned());
    if let Some(e) = end {
        a.push("-t".into());
        a.push(format!("{:.3}", e - start.unwrap_or(0.0)));
    }
    a
}

/// The -af chain that brings a clip to LOUDNESS as one fixed gain (two-pass linear loudnorm).
/// Falls back to one-pass if measuring fails; silence gets no gain at all.
pub fn loudness_filter(ffmpeg: &Path, cut: &[String]) -> String {
    let out = quiet_command(ffmpeg)
        .args(["-hide_banner", "-nostats"])
        .args(cut)
        .args(["-vn", "-af", &format!("loudnorm={LOUDNESS}:print_format=json"), "-f", "null", "-"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();
    let Ok(out) = out else { return format!("loudnorm={LOUDNESS},{RESAMPLE}") };
    let text = String::from_utf8_lossy(&out.stderr);
    let json = text.rfind('{').and_then(|i| text[i..].find('}').map(|j| &text[i..i + j + 1]));
    let measured: Option<Vec<f64>> = json.and_then(|j| serde_json::from_str::<Value>(j).ok()).and_then(|v| {
        ["input_i", "input_tp", "input_lra", "input_thresh", "target_offset"]
            .iter()
            .map(|k| v.get(*k).and_then(|x| x.as_str().and_then(|s| s.parse().ok()).or_else(|| x.as_f64())))
            .collect()
    });
    match measured {
        None => format!("loudnorm={LOUDNESS},{RESAMPLE}"),
        Some(m) if m[0] < -70.0 => RESAMPLE.to_string(),
        Some(m) => format!(
            "loudnorm={LOUDNESS}:measured_I={}:measured_TP={}:measured_LRA={}:measured_thresh={}:offset={}:linear=true,{RESAMPLE}",
            m[0], m[1], m[2], m[3], m[4]
        ),
    }
}

/// Cut, convert and (optionally) normalise `src` into `out` (48 kHz stereo WAV).
pub fn convert(ffmpeg: &Path, src: &Path, out: &Path, start: Option<f64>, end: Option<f64>, normalize: bool) -> Result<(), String> {
    let cut = cut_args(src, start, end);
    let af = if normalize { loudness_filter(ffmpeg, &cut) } else { RESAMPLE.to_string() };
    let r = quiet_command(ffmpeg)
        .args(["-y", "-hide_banner", "-loglevel", "error"])
        .args(&cut)
        .args(["-vn", "-af", &af, "-ac", "2", "-ar", "48000"])
        .arg(out)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("ffmpeg: {e}"))?;
    if !r.status.success() || !out.exists() {
        let msg = tail(&r.stderr);
        return Err(if msg.is_empty() { "ffmpeg failed".into() } else { msg });
    }
    Ok(())
}

/// Download and convert. `progress` gets "Downloading", "Cutting audio", "Finishing".
pub fn import_clip(req: &Request, tools: &Tools, progress: &dyn Fn(&str)) -> Result<Imported, String> {
    let yt = tools.yt_dlp.as_deref().ok_or("yt-dlp.exe not found (it should be next to JondisBoard.exe)")?;
    let ffmpeg = tools.ffmpeg.as_deref().ok_or("ffmpeg not found (put ffmpeg.exe next to the app)")?;
    if let (Some(s), Some(e)) = (req.start, req.end) {
        if e <= s {
            return Err("End time must be after start time".into());
        }
    }
    std::fs::create_dir_all(&req.folder).map_err(|e| e.to_string())?;
    let tmp = std::env::temp_dir().join(format!(
        "jondisboard_{}_{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
    ));
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let result = (|| {
        progress("Downloading");
        let mut cmd = quiet_command(yt);
        cmd.args(["-f", "bestaudio/best", "--no-playlist", "--no-progress", "--no-warnings", "--quiet", "--no-simulate", "-j"])
            .args(["--write-thumbnail", "--ffmpeg-location"])
            .arg(ffmpeg)
            .arg("-o")
            .arg(tmp.join("%(id)s.%(ext)s"));
        // YouTube needs a JavaScript runtime to unlock its audio formats
        for rt in ["deno", "node"] {
            if crate::paths::which(&format!("{rt}.exe")).is_some() {
                cmd.args(["--js-runtimes", rt]);
            }
        }
        if let Some(b) = &req.cookies_from_browser {
            cmd.args(["--cookies-from-browser", b]);
        }
        let out = cmd.arg("--").arg(&req.url).stdout(Stdio::piped()).stderr(Stdio::piped()).output().map_err(|e| format!("yt-dlp: {e}"))?;
        if !out.status.success() {
            let msg = tail(&out.stderr);
            return Err(if msg.is_empty() { "Download failed".into() } else { msg });
        }
        let info: Value = String::from_utf8_lossy(&out.stdout)
            .lines()
            .rev()
            .find_map(|l| serde_json::from_str(l).ok())
            .unwrap_or(Value::Null);
        let files: Vec<PathBuf> = std::fs::read_dir(&tmp).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
        let is_pic = |p: &Path| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| ["jpg", "jpeg", "png", "webp", "gif", "image"].contains(&e.to_ascii_lowercase().as_str()))
                .unwrap_or(false)
        };
        let src = files
            .iter()
            .filter(|p| !is_pic(p) && p.extension().map(|e| e != "json").unwrap_or(true))
            .max_by_key(|p| p.metadata().map(|m| m.len()).unwrap_or(0))
            .cloned()
            .ok_or("Download produced no file")?;
        let thumbnail = files.iter().find(|p| is_pic(p)).cloned();
        let name = req.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| name_from_info(&info));
        let out = unique_path(&req.folder, &safe_name(&name), ".wav");
        progress("Cutting audio");
        convert(ffmpeg, &src, &out, req.start, req.end, req.normalize)?;
        progress("Finishing");
        Ok(Imported { audio: out, thumbnail, temp: tmp.clone() })
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn finds_links_in_text() {
        assert_eq!(find_link("look https://www.instagram.com/reel/AbC/ lol").as_deref(), Some("https://www.instagram.com/reel/AbC/"));
        assert_eq!(find_link("https://youtu.be/xyz?t=3").as_deref(), Some("https://youtu.be/xyz?t=3"));
        assert_eq!(find_link("HTTPS://M.YouTube.com/watch?v=1").as_deref(), Some("HTTPS://M.YouTube.com/watch?v=1"));
        assert_eq!(find_link("https://music.youtube.com/watch?v=2").as_deref(), Some("https://music.youtube.com/watch?v=2"));
        assert_eq!(find_link("https://example.com/x https://youtube.com/shorts/a").as_deref(), Some("https://youtube.com/shorts/a"));
        assert_eq!(find_link("https://youtube.com/"), None);
        assert_eq!(find_link("nothing here"), None);
    }

    #[test]
    fn parses_times() {
        assert_eq!(parse_time("").unwrap(), None);
        assert_eq!(parse_time("83").unwrap(), Some(83.0));
        assert_eq!(parse_time("1:23.5").unwrap(), Some(83.5));
        assert_eq!(parse_time("0:01:23").unwrap(), Some(83.0));
        assert!(parse_time("1:2:3:4").is_err());
        assert!(parse_time("abc").is_err());
    }

    #[test]
    fn shortens_errors() {
        assert_eq!(short_error("ERROR: [youtube] abc123: Video unavailable\nmore"), "Video unavailable");
        assert_eq!(short_error("ERROR: [Instagram] x: Requested content is not available, rate-limit reached or login required"), "Instagram wants a login (see the README)");
        assert!(short_error(&"x".repeat(200)).ends_with('…'));
        assert_eq!(short_error(""), "Something went wrong");
    }

    #[test]
    fn names_from_info() {
        let ig = json!({"extractor_key": "Instagram", "description": "So funny #lol #meme\nsecond line", "id": "1"});
        assert_eq!(name_from_info(&ig), "So funny");
        let yt = json!({"extractor_key": "Youtube", "title": "A title", "id": "2"});
        assert_eq!(name_from_info(&yt), "A title");
        let empty = json!({"extractor_key": "Instagram", "description": "#only #tags", "channel": "bob", "id": "9"});
        assert_eq!(name_from_info(&empty), "bob_9");
    }

    #[test]
    fn convert_and_loudnorm_with_real_ffmpeg() {
        let Some(ffmpeg) = crate::paths::which("ffmpeg.exe") else { return };
        let dir = std::env::temp_dir().join(format!("jb_imp_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 3 s of a quiet 44.1 kHz mono tone: comes out 48 kHz stereo, louder (-16 LUFS), cut to 1 s
        let src = dir.join("src.wav");
        let tone: Vec<i16> = (0..44100 * 3).map(|i| ((i as f32 * 440.0 * 2.0 * std::f32::consts::PI / 44100.0).sin() * 1500.0) as i16).collect();
        crate::audio::wav::write_i16(&src, 44100, 1, &tone).unwrap();
        let out = dir.join("out.wav");
        convert(&ffmpeg, &src, &out, Some(0.5), Some(1.5), true).unwrap();
        let info = crate::audio::wav::read_info(&out).unwrap();
        assert_eq!((info.rate, info.channels), (48000, 2));
        assert!((info.duration() - 1.0).abs() < 0.01, "{}", info.duration());
        let s = crate::audio::wav::decode(&out, &info).unwrap();
        let peak = (0..s.frames()).map(|i| s.frame(i).0.abs()).fold(0f32, f32::max);
        assert!(peak > 1500.0 / 32768.0 * 2.0, "normalised up: {peak}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
