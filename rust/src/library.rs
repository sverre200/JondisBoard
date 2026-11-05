//! The sound library: the file system is the data model.
//!
//! ```text
//! sounds/
//! ├─ Chinese Rap.wav          clip in "No group"
//! ├─ Chinese Rap.png          its picture (optional)
//! ├─ THE LION/                a group
//! │  └─ To the gossiper.wav
//! └─ _deleted/                deleted clips (folders starting with "_" are hidden)
//! ```
//!
//! A clip's identity is its path inside sounds/ ("THE LION/To the gossiper.wav"). Everything
//! here is plain file work, with no UI, so it's unit-tested on throwaway folders.

use crate::config::{Config, ALL, NO_GROUP};
use crate::theme::{Rgb, GROUP_COLORS};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const AUDIO_EXTS: &[&str] = &["wav", "mp3", "ogg", "flac", "m4a", "mp4", "aac", "opus", "webm"];
pub const PICTURE_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "bmp"];

pub fn is_audio(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// "Memes/clip.wav" -> "Memes", "clip.wav" -> "".
pub fn group_of(name: &str) -> &str {
    match name.rfind('/') {
        Some(i) => &name[..i],
        None => NO_GROUP,
    }
}

/// "Memes/clip.wav" -> "clip".
pub fn stem_of(name: &str) -> &str {
    let file = name.rsplit('/').next().unwrap_or(name);
    match file.rfind('.') {
        Some(i) if i > 0 => &file[..i],
        _ => file,
    }
}

/// A clip's picture: a PNG next to it with the same name.
pub fn picture_path(audio: &Path) -> PathBuf {
    audio.with_extension("png")
}

/// A name that's safe as a file or folder name (importer.safe_name).
pub fn safe_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') && (*c as u32) >= 0x20)
        .collect();
    let trimmed = cleaned.trim_matches(|c| c == ' ' || c == '.');
    let cut: String = trimmed.chars().take(80).collect();
    let cut = cut.trim_end_matches([' ', '.']).to_string();
    if cut.is_empty() {
        "clip".into()
    } else {
        cut
    }
}

/// folder/name.ext, or folder/name (2).ext ... if that's taken.
pub fn unique_path(folder: &Path, name: &str, ext: &str) -> PathBuf {
    let ext = if ext.is_empty() || ext.starts_with('.') { ext.to_string() } else { format!(".{ext}") };
    let mut p = folder.join(format!("{name}{ext}"));
    let mut n = 2;
    while p.exists() {
        p = folder.join(format!("{name} ({n}){ext}"));
        n += 1;
    }
    p
}

/// One sound file.
#[derive(Clone, Debug)]
pub struct ClipFile {
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
    pub mtime: Option<SystemTime>,
}

impl ClipFile {
    pub fn group(&self) -> &str {
        group_of(&self.name)
    }
    pub fn stem(&self) -> &str {
        stem_of(&self.name)
    }
    pub fn picture(&self) -> PathBuf {
        picture_path(&self.path)
    }
}

/// What's in sounds/: groups (case-insensitive order) and clips (by name, case-insensitive).
#[derive(Clone, Debug, Default)]
pub struct Scan {
    pub groups: Vec<String>,
    pub clips: Vec<ClipFile>,
}

fn rel_name(sounds: &Path, p: &Path) -> String {
    p.strip_prefix(sounds).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

fn file_info(sounds: &Path, p: PathBuf) -> ClipFile {
    let meta = fs::metadata(&p).ok();
    ClipFile {
        name: rel_name(sounds, &p),
        size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
        mtime: meta.and_then(|m| m.modified().ok()),
        path: p,
    }
}

/// Read sounds/ (one level of group folders; folders starting with "_" are the app's own).
pub fn scan(sounds: &Path) -> io::Result<Scan> {
    fs::create_dir_all(sounds)?;
    let mut out = Scan::default();
    for entry in fs::read_dir(sounds)?.flatten() {
        let p = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        let fname = entry.file_name().to_string_lossy().into_owned();
        if ft.is_dir() {
            if fname.starts_with('_') {
                continue;
            }
            out.groups.push(fname);
            if let Ok(rd) = fs::read_dir(&p) {
                for e in rd.flatten() {
                    let q = e.path();
                    if e.file_type().map(|t| t.is_file()).unwrap_or(false) && is_audio(&q) {
                        out.clips.push(file_info(sounds, q));
                    }
                }
            }
        } else if ft.is_file() && is_audio(&p) {
            out.clips.push(file_info(sounds, p));
        }
    }
    out.groups.sort_by_key(|g| g.to_lowercase());
    out.clips.sort_by(|a, b| a.stem().to_lowercase().cmp(&b.stem().to_lowercase()).then(a.name.cmp(&b.name)));
    Ok(out)
}

/// The board's order: the saved one (renamed/moved clips keep their place via `moved`, new name ->
/// old name), then any clips not in it, in scan order.
pub fn merge_order(saved: &[String], present: &[String], moved: &HashMap<String, String>) -> Vec<String> {
    let renamed: HashMap<&str, &str> = moved.iter().map(|(new, old)| (old.as_str(), new.as_str())).collect();
    let have: HashSet<&str> = present.iter().map(String::as_str).collect();
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(present.len());
    for n in saved {
        let n = renamed.get(n.as_str()).copied().unwrap_or(n.as_str());
        if have.contains(n) && seen.insert(n.to_string()) {
            out.push(n.to_string());
        }
    }
    for n in present {
        if seen.insert(n.clone()) {
            out.push(n.clone());
        }
    }
    out
}

/// `order` with `name` moved to just before `anchor` (or after it).
pub fn move_in_order(order: &[String], name: &str, anchor: &str, after: bool) -> Vec<String> {
    let mut out: Vec<String> = order.iter().filter(|n| *n != name).cloned().collect();
    let i = match out.iter().position(|n| n == anchor) {
        Some(i) => i + after as usize,
        None => out.len(),
    };
    out.insert(i, name.to_string());
    out
}

/// Give every group a colour (unused ones first) and forget colours of groups that are gone.
pub fn assign_group_colors(cfg: &mut Config, groups: &[String]) {
    cfg.group_colors.retain(|g, _| groups.contains(g));
    for g in groups {
        if !cfg.group_colors.contains_key(g) {
            let used: Vec<&String> = cfg.group_colors.values().collect();
            let free = GROUP_COLORS.iter().find(|c| !used.iter().any(|u| Rgb::parse(u) == Some(**c)));
            cfg.group_colors.insert(g.clone(), free.unwrap_or(&GROUP_COLORS[0]).hex());
        }
    }
}

/// Fix up the config after a scan: order, hotkeys of clips that are gone, the open group.
pub fn reconcile(cfg: &mut Config, scan: &Scan, moved: &HashMap<String, String>) {
    let present: Vec<String> = scan.clips.iter().map(|c| c.name.clone()).collect();
    cfg.order = merge_order(&cfg.order, &present, moved);
    let have: HashSet<&String> = present.iter().collect();
    cfg.hotkeys.retain(|k, _| have.contains(k));
    if cfg.group != ALL && cfg.group != NO_GROUP && !scan.groups.contains(&cfg.group) {
        cfg.group = ALL.into();
    }
    assign_group_colors(cfg, &scan.groups);
}

/// File operations on the library. Each returns the new clip name where one changes.
pub struct Library {
    pub sounds: PathBuf,
}

impl Library {
    pub fn new(sounds: PathBuf) -> Library {
        Library { sounds }
    }

    pub fn path_of(&self, name: &str) -> PathBuf {
        self.sounds.join(name.replace('/', "\\"))
    }

    pub fn trash(&self) -> PathBuf {
        self.sounds.join("_deleted")
    }

    pub fn folder_of_group(&self, group: &str) -> PathBuf {
        if group.is_empty() || group == ALL {
            self.sounds.clone()
        } else {
            self.sounds.join(group)
        }
    }

    /// Move a clip (and its picture) into `folder`, optionally renamed. Returns the new name.
    pub fn move_clip(&self, name: &str, folder: &Path, new_stem: Option<&str>) -> io::Result<String> {
        let src = self.path_of(name);
        fs::create_dir_all(folder)?;
        let stem = new_stem.map(str::to_owned).unwrap_or_else(|| stem_of(name).to_string());
        let ext = src.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        if src.parent() == Some(folder) && new_stem.is_none() {
            return Ok(name.to_string());
        }
        let dst = unique_path(folder, &stem, &ext);
        fs::rename(&src, &dst)?;
        let pic = picture_path(&src);
        if pic.exists() {
            let _ = fs::rename(&pic, picture_path(&dst));
        }
        Ok(rel_name(&self.sounds, &dst))
    }

    /// Move a clip and its picture to sounds/_deleted (you can get it back from there).
    pub fn delete_clip(&self, name: &str) -> io::Result<PathBuf> {
        let src = self.path_of(name);
        let trash = self.trash();
        fs::create_dir_all(&trash)?;
        let ext = src.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let dst = unique_path(&trash, stem_of(name), &ext);
        fs::rename(&src, &dst)?;
        let pic = picture_path(&src);
        if pic.exists() {
            let _ = fs::rename(&pic, picture_path(&dst));
        }
        Ok(dst)
    }

    pub fn create_group(&self, name: &str) -> io::Result<String> {
        let g = safe_name(name);
        fs::create_dir_all(self.sounds.join(&g))?;
        Ok(g)
    }

    /// Rename a group folder and carry its hotkeys, order, colour and the open tab along.
    pub fn rename_group(&self, cfg: &mut Config, group: &str, new_name: &str) -> io::Result<String> {
        let new = safe_name(new_name);
        if new == group {
            return Ok(new);
        }
        let dst = self.sounds.join(&new);
        // a case-only rename is the same folder on Windows
        if dst.exists() && !new.eq_ignore_ascii_case(group) {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("There's already a group called {new}")));
        }
        fs::rename(self.sounds.join(group), &dst)?;
        let prefix = format!("{group}/");
        let fix = |k: &String| -> String {
            match k.strip_prefix(&prefix) {
                Some(rest) => format!("{new}/{rest}"),
                None => k.clone(),
            }
        };
        cfg.hotkeys = cfg.hotkeys.iter().map(|(k, v)| (fix(k), v.clone())).collect();
        cfg.order = cfg.order.iter().map(fix).collect();
        if cfg.group == group {
            cfg.group = new.clone();
        }
        if let Some(c) = cfg.group_colors.remove(group) {
            cfg.group_colors.insert(new.clone(), c);
        }
        Ok(new)
    }

    /// Delete a group: its sounds move to No group (keeping their hotkeys and places), anything
    /// else in the folder goes to _deleted, then the folder is removed. Returns old -> new names.
    pub fn delete_group(&self, cfg: &mut Config, group: &str, clips: &[String]) -> io::Result<HashMap<String, String>> {
        let folder = self.sounds.join(group);
        let mut moved = HashMap::new();
        for name in clips.iter().filter(|n| group_of(n) == group) {
            let new = self.move_clip(name, &self.sounds, None)?;
            carry(cfg, name, &new);
            moved.insert(new, name.clone());
        }
        if let Ok(rd) = fs::read_dir(&folder) {
            let leftovers: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
            if !leftovers.is_empty() {
                let trash = self.trash();
                fs::create_dir_all(&trash)?;
                for p in leftovers {
                    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
                    fs::rename(&p, unique_path(&trash, &stem, &ext))?;
                }
            }
        }
        fs::remove_dir(&folder)?;
        cfg.group_colors.remove(group);
        if cfg.group == group {
            cfg.group = ALL.into();
        }
        Ok(moved)
    }

    /// Copy files into `folder` (Add files). Returns the new clip names.
    pub fn add_files(&self, files: &[PathBuf], folder: &Path) -> Vec<Result<String, String>> {
        let _ = fs::create_dir_all(folder);
        files
            .iter()
            .map(|p| {
                let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default();
                let dst = unique_path(folder, &safe_name(&stem), &ext);
                fs::copy(p, &dst)
                    .map(|_| rel_name(&self.sounds, &dst))
                    .map_err(|e| format!("{}: {e}", p.display()))
            })
            .collect()
    }
}

/// A clip got a new name: its hotkey and place in the order go with it.
pub fn carry(cfg: &mut Config, old: &str, new: &str) {
    if old == new {
        return;
    }
    if let Some(b) = cfg.hotkeys.remove(old) {
        cfg.hotkeys.insert(new.to_string(), b);
    }
    for n in cfg.order.iter_mut() {
        if n == old {
            *n = new.to_string();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Binding;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Tmp {
            let p = std::env::temp_dir().join(format!("jb_lib_{tag}_{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
        fn touch(&self, rel: &str) {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, b"x").unwrap();
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn bind(scan: u32) -> Binding {
        Binding { scan, keypad: false, mods: vec![], label: "x".into() }
    }

    #[test]
    fn scan_finds_groups_clips_and_skips_private_folders() {
        let t = Tmp::new("scan");
        t.touch("b clip.wav");
        t.touch("A clip.MP3");
        t.touch("A clip.png");
        t.touch("notes.txt");
        t.touch("zeta/one.wav");
        t.touch("Alpha/two.ogg");
        t.touch("_deleted/gone.wav");
        let s = scan(&t.0).unwrap();
        assert_eq!(s.groups, vec!["Alpha", "zeta"]);
        let names: Vec<&str> = s.clips.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["A clip.MP3", "b clip.wav", "zeta/one.wav", "Alpha/two.ogg"]);
        assert_eq!(s.clips[2].group(), "zeta");
        assert_eq!(s.clips[0].stem(), "A clip");
    }

    #[test]
    fn order_keeps_saved_then_new_and_follows_renames() {
        let present: Vec<String> = ["a", "b", "c", "d2"].iter().map(|s| s.to_string()).collect();
        let saved: Vec<String> = ["c", "gone", "d", "a", "c"].iter().map(|s| s.to_string()).collect();
        let moved = HashMap::from([("d2".to_string(), "d".to_string())]);
        assert_eq!(merge_order(&saved, &present, &moved), vec!["c", "d2", "a", "b"]);
    }

    #[test]
    fn move_in_order_before_and_after() {
        let o: Vec<String> = ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect();
        assert_eq!(move_in_order(&o, "a", "c", false), vec!["b", "a", "c", "d"]);
        assert_eq!(move_in_order(&o, "a", "d", true), vec!["b", "c", "d", "a"]);
        assert_eq!(move_in_order(&o, "d", "a", false), vec!["d", "a", "b", "c"]);
    }

    #[test]
    fn safe_and_unique_names() {
        assert_eq!(safe_name("  a<b>:c?.  "), "abc");
        assert_eq!(safe_name("..."), "clip");
        assert_eq!(safe_name(&"x".repeat(100)).len(), 80);
        let t = Tmp::new("uniq");
        t.touch("x.wav");
        t.touch("x (2).wav");
        assert_eq!(unique_path(&t.0, "x", ".wav"), t.0.join("x (3).wav"));
        assert_eq!(unique_path(&t.0, "y", "wav"), t.0.join("y.wav"));
    }

    #[test]
    fn move_rename_delete_carry_pictures() {
        let t = Tmp::new("ops");
        t.touch("a.wav");
        t.touch("a.png");
        t.touch("G/b.wav");
        let lib = Library::new(t.0.clone());
        let new = lib.move_clip("a.wav", &t.0.join("G"), None).unwrap();
        assert_eq!(new, "G/a.wav");
        assert!(t.0.join("G/a.png").exists() && !t.0.join("a.png").exists());
        let renamed = lib.move_clip("G/a.wav", &t.0.join("G"), Some("hello")).unwrap();
        assert_eq!(renamed, "G/hello.wav");
        assert!(t.0.join("G/hello.png").exists());
        // into a folder that has the name already: numbered
        t.touch("b.wav");
        assert_eq!(lib.move_clip("G/b.wav", &t.0, None).unwrap(), "b (2).wav");
        let dst = lib.delete_clip("G/hello.wav").unwrap();
        assert_eq!(dst, t.0.join("_deleted/hello.wav"));
        assert!(t.0.join("_deleted/hello.png").exists());
    }

    #[test]
    fn rename_and_delete_group_carry_config() {
        let t = Tmp::new("groups");
        t.touch("G/a.wav");
        t.touch("G/a.png");
        t.touch("G/readme.txt");
        let lib = Library::new(t.0.clone());
        let mut cfg = Config::default();
        cfg.hotkeys.insert("G/a.wav".into(), bind(5));
        cfg.order = vec!["G/a.wav".into()];
        cfg.group = "G".into();
        cfg.group_colors.insert("G".into(), "#ef4444".into());
        let new = lib.rename_group(&mut cfg, "G", "Memes").unwrap();
        assert_eq!(new, "Memes");
        assert!(cfg.hotkeys.contains_key("Memes/a.wav"));
        assert_eq!(cfg.order, vec!["Memes/a.wav"]);
        assert_eq!(cfg.group, "Memes");
        assert_eq!(cfg.group_colors["Memes"], "#ef4444");
        let moved = lib.delete_group(&mut cfg, "Memes", &["Memes/a.wav".into()]).unwrap();
        assert_eq!(moved["a.wav"], "Memes/a.wav");
        assert!(t.0.join("a.wav").exists() && t.0.join("a.png").exists());
        assert!(t.0.join("_deleted/readme.txt").exists());
        assert!(!t.0.join("Memes").exists());
        assert!(cfg.hotkeys.contains_key("a.wav"));
        assert_eq!(cfg.group, ALL);
    }

    #[test]
    fn reconcile_drops_dead_hotkeys_and_assigns_colours() {
        let t = Tmp::new("rec");
        t.touch("x.wav");
        t.touch("One/y.wav");
        t.touch("Two/z.wav");
        let s = scan(&t.0).unwrap();
        let mut cfg = Config::default();
        cfg.hotkeys.insert("gone.wav".into(), bind(1));
        cfg.hotkeys.insert("x.wav".into(), bind(2));
        cfg.group = "Nope".into();
        cfg.group_colors.insert("One".into(), "#7c5cff".into());
        cfg.group_colors.insert("Old".into(), "#3b82f6".into());
        reconcile(&mut cfg, &s, &HashMap::new());
        assert_eq!(cfg.hotkeys.len(), 1);
        assert_eq!(cfg.group, ALL);
        assert_eq!(cfg.group_colors["One"], "#7c5cff");
        assert_eq!(cfg.group_colors["Two"], "#3b82f6"); // the first unused one
        assert!(!cfg.group_colors.contains_key("Old"));
        assert_eq!(cfg.order.len(), 3);
    }

    #[test]
    fn group_and_stem_helpers() {
        assert_eq!(group_of("Memes/clip.wav"), "Memes");
        assert_eq!(group_of("clip.wav"), "");
        assert_eq!(stem_of("Memes/clip.v2.wav"), "clip.v2");
        assert_eq!(stem_of(".hidden"), ".hidden");
    }
}
