//! JondisBoard.exe
//!
//!   JondisBoard.exe                      the app (data next to the exe)
//!   JondisBoard.exe --data-dir <folder>  use another folder's sounds/ and config.json
//!   JondisBoard.exe --bench <out.json>   scripted performance run (throwaway libraries only)
//!   JondisBoard.exe --audio-check [...]  audio diagnostics (see diag.rs)
#![windows_subsystem = "windows"]

use jondisboard::{log, paths::Paths, ui};
use std::path::PathBuf;
use std::time::Instant;
use windows::Win32::System::Ole::OleInitialize;
use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;

const APP_ID: &str = "JondisBoard.Soundboard";

fn main() {
    let started = Instant::now();
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let paths = Paths::new(arg("--data-dir").map(PathBuf::from));
    log::init(paths.log());
    std::panic::set_hook(Box::new(|info| {
        log::error(&format!("crash: {info}"));
    }));
    if args.iter().any(|a| a == "--audio-check") {
        std::process::exit(jondisboard::diag::main(&paths, &args));
    }
    // our own taskbar identity before any window exists: started from a shortcut, Windows would
    // otherwise show the shortcut's (exe's) icon on the taskbar instead of the themed one
    // SAFETY: plain calls at startup on the UI thread.
    unsafe {
        let _ = SetCurrentProcessExplicitAppUserModelID(&windows::core::HSTRING::from(APP_ID));
        let _ = OleInitialize(None);
    }
    let _ = std::fs::create_dir_all(paths.sounds());
    let bench = arg("--bench").map(|p| ui::bench::Bench::new(PathBuf::from(p)));
    if let Err(e) = ui::window::run(paths, started, bench) {
        log::error(&format!("the window failed: {e}"));
    }
}
