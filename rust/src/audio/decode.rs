//! Loading clips into memory: 48 kHz mono/stereo WAVs are read directly (bit-exact); anything
//! else (other rates, >2 channels, MP3, Opus, M4A...) goes through ffmpeg with its tuned resampler
//! and a proper downmix (keeps a 5.1 file's centre channel, where dialogue is).

use super::wav::{self, Samples};
use crate::paths::RESAMPLE;
use std::io;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;
pub const BELOW_NORMAL_PRIORITY: u32 = 0x0000_4000;

/// A Command for a helper exe: no console window, below-normal priority (so a game never
/// stutters while it works).
pub fn quiet_command(exe: &Path) -> Command {
    let mut c = Command::new(exe);
    c.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY);
    c.stdin(Stdio::null());
    c
}

/// Duration in seconds from the file header, without decoding (WAV only).
pub fn quick_duration(path: &Path) -> Option<f64> {
    wav::read_info(path).ok().map(|i| i.duration())
}

/// Decode any supported file to 48 kHz interleaved stereo.
pub fn load(path: &Path, ffmpeg: Option<&Path>) -> Result<Samples, String> {
    if let Ok(info) = wav::read_info(path) {
        if wav::direct(&info) {
            return wav::decode(path, &info).map_err(|e| e.to_string());
        }
    }
    let ffmpeg = ffmpeg.ok_or_else(|| "ffmpeg not found (needed for this file type)".to_string())?;
    ffmpeg_decode(path, ffmpeg).map_err(|e| e.to_string())
}

pub fn ffmpeg_decode(path: &Path, ffmpeg: &Path) -> io::Result<Samples> {
    let out = quiet_command(ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-vn", "-af", RESAMPLE, "-f", "f32le", "-ac", "2", "-ar", "48000", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(io::Error::other(if msg.is_empty() { "ffmpeg failed".into() } else { msg }));
    }
    let n = out.stdout.len() / 8 * 2;
    let mut v = Vec::with_capacity(n);
    for b in out.stdout.chunks_exact(4).take(n) {
        v.push(f32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    }
    Ok(Samples::F32(v.into_boxed_slice()))
}
