//! `config.json`: read leniently, written back with every key the Python app knows (and any
//! unknown keys kept as they were), so both versions can share one file.
//!
//! Reading never fails: a missing or broken file gives the defaults, a wrong type for one key
//! gives that key's default. The first time this version saves over a config written by someone
//! else, it copies the old file to `config.backup.json` first.

use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The special group views (same strings as the Python app).
pub const ALL: &str = "*";
pub const NO_GROUP: &str = "";

/// A hotkey: scan code + numpad flag + modifiers (`hotkeys.py` format).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub scan: u32,
    pub keypad: bool,
    /// any of "ctrl", "shift", "alt", in that order
    pub mods: Vec<String>,
    pub label: String,
}

impl Binding {
    /// What makes two bindings the same key (the label doesn't count).
    pub fn key(&self) -> (u32, bool, Vec<String>) {
        let mut m = self.mods.clone();
        m.sort();
        (self.scan, self.keypad, m)
    }

    pub fn same(&self, other: &Binding) -> bool {
        self.key() == other.key()
    }

    /// "ctrl+f5" (hotkeys.binding_label)
    pub fn label_raw(&self) -> String {
        let mut parts: Vec<String> = self.mods.clone();
        parts.push(if self.label.is_empty() { "?".into() } else { self.label.clone() });
        parts.join("+")
    }

    /// "Ctrl+F5" (app.pretty_key: Python's str.title())
    pub fn pretty(&self) -> String {
        title_case(&self.label_raw())
    }

    fn from_value(v: &Value) -> Option<Binding> {
        let o = v.as_object()?;
        let scan = o.get("scan")?.as_u64()? as u32;
        let keypad = o.get("keypad").and_then(Value::as_bool).unwrap_or(false);
        let mods = o
            .get("mods")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|m| m.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        let label = o.get("label").and_then(Value::as_str).unwrap_or("?").to_owned();
        Some(Binding { scan, keypad, mods, label })
    }

    fn to_value(&self) -> Value {
        json!({"scan": self.scan, "keypad": self.keypad, "mods": self.mods, "label": self.label})
    }
}

/// Python's str.title(): upper-case the first letter of every run of letters.
pub fn title_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_letter = false;
    for c in s.chars() {
        if c.is_alphabetic() {
            if prev_letter {
                out.extend(c.to_lowercase());
            } else {
                out.extend(c.to_uppercase());
            }
            prev_letter = true;
        } else {
            out.push(c);
            prev_letter = false;
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub mic: Option<String>,
    pub cable: Option<String>,
    pub monitor: Option<String>,
    pub mic_volume: f64,
    pub clip_volume: f64,
    pub monitor_volume: f64,
    pub mic_passthrough: bool,
    /// clip path inside sounds/ ("Group/clip.wav") -> binding
    pub hotkeys: BTreeMap<String, Binding>,
    pub stop_hotkey: Option<Binding>,
    /// the tab shown last ("*" = All, "" = No group)
    pub group: String,
    pub group_colors: BTreeMap<String, String>,
    /// clip order you dragged them into (new clips go last)
    pub order: Vec<String>,
    pub solo: bool,
    pub toggle_stop: bool,
    pub cookies_from_browser: Option<String>,
    pub theme: String,
    /// "WxH+X+Y" (width/height in DIPs, position in screen pixels, like customtkinter)
    pub geometry: Option<String>,
    /// keys this version doesn't know, kept as they were
    extra: Map<String, Value>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            mic: None,
            cable: None,
            monitor: None,
            mic_volume: 1.0,
            clip_volume: 0.8,
            monitor_volume: 0.6,
            mic_passthrough: true,
            hotkeys: BTreeMap::new(),
            stop_hotkey: None,
            group: ALL.into(),
            group_colors: BTreeMap::new(),
            order: Vec::new(),
            solo: false,
            toggle_stop: false,
            cookies_from_browser: None,
            theme: crate::theme::DEFAULT_THEME.into(),
            geometry: None,
            extra: Map::new(),
        }
    }
}

const KNOWN: &[&str] = &[
    "mic", "cable", "monitor", "mic_volume", "clip_volume", "monitor_volume", "mic_passthrough",
    "hotkeys", "stop_hotkey", "group", "group_colors", "order", "solo", "toggle_stop",
    "cookies_from_browser", "theme", "geometry",
];

fn opt_str(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned)
}

fn num(v: Option<&Value>, default: f64) -> f64 {
    v.and_then(Value::as_f64).filter(|x| x.is_finite() && *x >= 0.0).map(|x| x.min(2.0)).unwrap_or(default)
}

fn boolean(v: Option<&Value>, default: bool) -> bool {
    v.and_then(Value::as_bool).unwrap_or(default)
}

impl Config {
    /// Build from parsed JSON, key by key; anything missing or of the wrong type gets its default.
    pub fn from_value(v: &Value) -> Config {
        let d = Config::default();
        let Some(o) = v.as_object() else { return d };
        let hotkeys = o
            .get("hotkeys")
            .and_then(Value::as_object)
            .map(|m| m.iter().filter_map(|(k, b)| Binding::from_value(b).map(|b| (k.clone(), b))).collect())
            .unwrap_or_default();
        let group_colors = o
            .get("group_colors")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter_map(|(k, c)| c.as_str().filter(|c| crate::theme::Rgb::parse(c).is_some()).map(|c| (k.clone(), c.to_owned())))
                    .collect()
            })
            .unwrap_or_default();
        let order = o
            .get("order")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        let extra = o.iter().filter(|(k, _)| !KNOWN.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect();
        Config {
            mic: opt_str(o.get("mic")),
            cable: opt_str(o.get("cable")),
            monitor: opt_str(o.get("monitor")),
            mic_volume: num(o.get("mic_volume"), d.mic_volume),
            clip_volume: num(o.get("clip_volume"), d.clip_volume),
            monitor_volume: num(o.get("monitor_volume"), d.monitor_volume),
            mic_passthrough: boolean(o.get("mic_passthrough"), d.mic_passthrough),
            hotkeys,
            stop_hotkey: o.get("stop_hotkey").and_then(Binding::from_value),
            group: o.get("group").and_then(Value::as_str).map(str::to_owned).unwrap_or(d.group),
            group_colors,
            order,
            solo: boolean(o.get("solo"), d.solo),
            toggle_stop: boolean(o.get("toggle_stop"), d.toggle_stop),
            cookies_from_browser: opt_str(o.get("cookies_from_browser")),
            theme: o.get("theme").and_then(Value::as_str).map(str::to_owned).unwrap_or(d.theme),
            geometry: opt_str(o.get("geometry")),
            extra,
        }
    }

    pub fn to_value(&self) -> Value {
        let mut o = self.extra.clone();
        let s = |x: &Option<String>| x.as_ref().map(|s| Value::String(s.clone())).unwrap_or(Value::Null);
        o.insert("mic".into(), s(&self.mic));
        o.insert("cable".into(), s(&self.cable));
        o.insert("monitor".into(), s(&self.monitor));
        o.insert("mic_volume".into(), json!(round3(self.mic_volume)));
        o.insert("clip_volume".into(), json!(round3(self.clip_volume)));
        o.insert("monitor_volume".into(), json!(round3(self.monitor_volume)));
        o.insert("mic_passthrough".into(), json!(self.mic_passthrough));
        o.insert(
            "hotkeys".into(),
            Value::Object(self.hotkeys.iter().map(|(k, b)| (k.clone(), b.to_value())).collect()),
        );
        o.insert("stop_hotkey".into(), self.stop_hotkey.as_ref().map(Binding::to_value).unwrap_or(Value::Null));
        o.insert("group".into(), json!(self.group));
        o.insert("group_colors".into(), json!(self.group_colors));
        o.insert("order".into(), json!(self.order));
        o.insert("solo".into(), json!(self.solo));
        o.insert("toggle_stop".into(), json!(self.toggle_stop));
        o.insert("cookies_from_browser".into(), s(&self.cookies_from_browser));
        o.insert("theme".into(), json!(self.theme));
        o.insert("geometry".into(), s(&self.geometry));
        Value::Object(o)
    }
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// Loads and saves one config file.
pub struct ConfigFile {
    path: PathBuf,
    backed_up: bool,
}

impl ConfigFile {
    pub fn new(path: PathBuf) -> Self {
        ConfigFile { path, backed_up: false }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The config, or the defaults (and the reason, for the log) if it can't be read.
    pub fn load(&self) -> (Config, Option<String>) {
        match fs::read(&self.path) {
            Ok(bytes) => {
                // tolerate a UTF-8 BOM (Windows editors add one)
                let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
                match serde_json::from_slice::<Value>(bytes) {
                    Ok(v) => (Config::from_value(&v), None),
                    Err(e) => (Config::default(), Some(format!("config.json unreadable ({e}), using defaults"))),
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => (Config::default(), None),
            Err(e) => (Config::default(), Some(format!("config.json unreadable ({e}), using defaults"))),
        }
    }

    /// Write atomically (temp file + rename). The first save of a session copies an existing file
    /// to config.backup.json if there's no backup yet, so a migration can always be undone.
    pub fn save(&mut self, cfg: &Config) -> io::Result<()> {
        if !self.backed_up {
            self.backed_up = true;
            let backup = self.path.with_file_name("config.backup.json");
            if self.path.exists() && !backup.exists() {
                let _ = fs::copy(&self.path, &backup);
            }
        }
        let text = serde_json::to_string_pretty(&cfg.to_value()).map_err(io::Error::other)?;
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, text.as_bytes())?;
        fs::rename(&tmp, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_python_config_and_keeps_unknown_keys() {
        let text = r##"{
          "mic": "Mic/Inst (Samson G-Track Pro)", "cable": "CABLE Input (VB-Audio Virtual Cable)",
          "monitor": null, "mic_volume": 0.598, "clip_volume": 0.784, "monitor_volume": 0.428,
          "mic_passthrough": false,
          "hotkeys": {"THE LION/a.wav": {"scan": 63, "keypad": false, "mods": ["ctrl"], "label": "f5"}},
          "stop_hotkey": {"scan": 79, "keypad": true, "mods": [], "label": "num 1"},
          "group": "", "group_colors": {"THE LION": "#ef4444", "bad": 5}, "order": ["a.wav", 3],
          "solo": true, "toggle_stop": true, "cookies_from_browser": null, "theme": "Crimson",
          "geometry": "1918x1000+-7+0", "future_key": [1, 2]
        }"##;
        let v: Value = serde_json::from_str(text).unwrap();
        let c = Config::from_value(&v);
        assert_eq!(c.mic.as_deref(), Some("Mic/Inst (Samson G-Track Pro)"));
        assert_eq!(c.monitor, None);
        assert_eq!(c.group, NO_GROUP);
        assert!(c.solo && c.toggle_stop && !c.mic_passthrough);
        assert_eq!(c.hotkeys["THE LION/a.wav"].pretty(), "Ctrl+F5");
        assert_eq!(c.stop_hotkey.as_ref().unwrap().pretty(), "Num 1");
        assert_eq!(c.group_colors.len(), 1);
        assert_eq!(c.order, vec!["a.wav".to_string()]);
        let back = c.to_value();
        assert_eq!(back["future_key"], json!([1, 2]));
        assert_eq!(back["geometry"], json!("1918x1000+-7+0"));
        assert_eq!(Config::from_value(&back), c);
    }

    #[test]
    fn wrong_types_fall_back_per_key() {
        let v: Value = serde_json::from_str(r#"{"mic_volume": "loud", "solo": 1, "theme": "Graphite"}"#).unwrap();
        let c = Config::from_value(&v);
        assert_eq!(c.mic_volume, 1.0);
        assert!(!c.solo);
        assert_eq!(c.theme, "Graphite");
        assert_eq!(c.group, ALL);
    }

    #[test]
    fn save_backs_up_once_and_is_atomic() {
        let dir = std::env::temp_dir().join(format!("jb_cfg_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        fs::write(&path, b"\xef\xbb\xbf{\"theme\": \"Crimson\"}").unwrap();
        let mut f = ConfigFile::new(path.clone());
        let (mut c, err) = f.load();
        assert!(err.is_none());
        assert_eq!(c.theme, "Crimson");
        c.theme = "Graphite".into();
        f.save(&c).unwrap();
        f.save(&c).unwrap();
        let backup = fs::read(dir.join("config.backup.json")).unwrap();
        assert!(String::from_utf8_lossy(&backup).contains("Crimson"));
        assert_eq!(f.load().0.theme, "Graphite");
        assert!(!dir.join("config.json.tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn broken_file_gives_defaults() {
        let dir = std::env::temp_dir().join(format!("jb_cfg2_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        fs::write(&path, "{not json").unwrap();
        let (c, err) = ConfigFile::new(path).load();
        assert!(err.is_some());
        assert_eq!(c, Config::default());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn title_case_like_python() {
        assert_eq!(title_case("ctrl+num 1"), "Ctrl+Num 1");
        assert_eq!(title_case("f12"), "F12");
    }
}
