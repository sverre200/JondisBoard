//! Where the app's files live.
//!
//! User data (sounds/, config.json, soundboard.log) sits next to JondisBoard.exe, like the Python
//! build. For development and tests, `--data-dir <folder>` or the `JONDISBOARD_HOME` environment
//! variable point it somewhere else (never at the real data while developing).

use std::path::{Path, PathBuf};

/// ffmpeg's resampler set up for transparent sample-rate changes (aliasing ~ -120 dB). The usual
/// ffmpeg builds have no soxr. Used for every conversion to 48 kHz.
pub const RESAMPLE: &str = "aresample=48000:filter_size=64:phase_shift=10:cutoff=0.97:filter_type=kaiser:kaiser_beta=12";

#[derive(Clone, Debug)]
pub struct Paths {
    /// the folder with sounds/, config.json and the log
    pub app_dir: PathBuf,
    /// the folder with the exe (and its helper binaries)
    pub exe_dir: PathBuf,
}

impl Paths {
    pub fn new(data_dir: Option<PathBuf>) -> Paths {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        let app_dir = data_dir
            .or_else(|| std::env::var_os("JONDISBOARD_HOME").map(PathBuf::from))
            .unwrap_or_else(|| exe_dir.clone());
        Paths { app_dir, exe_dir }
    }

    pub fn sounds(&self) -> PathBuf {
        self.app_dir.join("sounds")
    }
    pub fn trash(&self) -> PathBuf {
        self.sounds().join("_deleted")
    }
    pub fn config(&self) -> PathBuf {
        self.app_dir.join("config.json")
    }
    pub fn log(&self) -> PathBuf {
        self.app_dir.join("soundboard.log")
    }

    /// A helper exe: next to JondisBoard.exe, then next to the data, then on PATH.
    pub fn tool(&self, name: &str) -> Option<PathBuf> {
        for dir in [&self.exe_dir, &self.app_dir] {
            let p = dir.join(name);
            if p.is_file() {
                return Some(p);
            }
        }
        which(name)
    }

    pub fn ffmpeg(&self) -> Option<PathBuf> {
        self.tool("ffmpeg.exe")
    }
    pub fn yt_dlp(&self) -> Option<PathBuf> {
        self.tool("yt-dlp.exe")
    }
}

/// Search PATH for an exe.
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(name)).find(|p| p.is_file())
}
