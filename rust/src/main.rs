//! JondisBoard.exe
//!
//!   JondisBoard.exe                      the app (data next to the exe)
//!   JondisBoard.exe --data-dir <folder>  use another folder's sounds/ and config.json
//!   JondisBoard.exe --bench <out.json>   scripted performance run (throwaway libraries only)
//!   JondisBoard.exe --audio-check [...]  audio diagnostics (see diag.rs)
//!   JondisBoard.exe --import-file reels.txt [--group G]   bulk import (url | name | start | end)
#![windows_subsystem = "windows"]

use jondisboard::{log, paths::Paths, ui};
use std::path::PathBuf;
use std::time::Instant;
use windows::Win32::System::Ole::OleInitialize;
use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;

use ui::taskbar::APP_ID;

fn main() {
    let started = Instant::now();
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let paths = Paths::new(arg("--data-dir").map(PathBuf::from));
    log::init(paths.log());
    std::panic::set_hook(Box::new(|info| {
        log::error(&format!("crash: {info}"));
    }));
    if args.iter().any(|a| a == "--import-file") {
        std::process::exit(jondisboard::diag::bulk_import(&paths, &args));
    }
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
    // started by an update: wait for the old copy to close (it gave its process id), then remove
    // what it left behind. Every start also clears leftovers of an earlier update.
    if let Some(pid) = arg("--updated").and_then(|p| p.parse::<u32>().ok()) {
        wait_for_exit(pid);
        log::info(&format!("JondisBoard {} started after an update", jondisboard::update::VERSION));
    }
    if let Some(work) = jondisboard::update::work_dir() {
        jondisboard::update::cleanup(&paths.exe_dir, &work);
    }
    let _ = std::fs::create_dir_all(paths.sounds());
    let bench = arg("--bench").map(|p| ui::bench::Bench::new(PathBuf::from(p)));
    if let Err(e) = ui::window::run(paths, started, bench) {
        log::error(&format!("the window failed: {e}"));
    }
}

/// Wait (at most 10 s) for the process that started us to end.
fn wait_for_exit(pid: u32) {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
    // SAFETY: a handle to another process, only waited on, then closed.
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            let _ = WaitForSingleObject(h, 10_000);
            let _ = CloseHandle(h);
        }
    }
}
