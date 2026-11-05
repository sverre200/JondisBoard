//! JondisBoard: a Windows soundboard that plays clips into Discord and games through VB-CABLE.
//!
//! The crate is a library plus a thin binary so the logic (config, library, audio, importer,
//! hotkey matching) can be unit-tested without opening windows or devices.
pub mod audio;
pub mod config;
pub mod hotkeys;
pub mod importer;
pub mod library;
pub mod log;
pub mod paths;
pub mod theme;
