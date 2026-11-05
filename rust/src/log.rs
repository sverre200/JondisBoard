//! A tiny append-only log (soundboard.log next to the app), like the Python app's logging setup.
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

pub fn init(path: PathBuf) {
    let _ = LOG_PATH.set(path);
}

fn stamp() -> String {
    // local time via the Win32 API would need more code; UTC with a Z is unambiguous
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (d, s) = (secs / 86400, secs % 86400);
    // civil-from-days (Howard Hinnant)
    let z = d as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{day:02} {:02}:{:02}:{:02}Z", s / 3600, s / 60 % 60, s % 60)
}

pub fn write(level: &str, msg: &str) {
    let Some(p) = LOG_PATH.get() else { return };
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(p) {
        let _ = writeln!(f, "{} {level} {msg}", stamp());
    }
}

pub fn info(msg: &str) {
    write("INFO", msg)
}
pub fn warn(msg: &str) {
    write("WARNING", msg)
}
pub fn error(msg: &str) {
    write("ERROR", msg)
}
