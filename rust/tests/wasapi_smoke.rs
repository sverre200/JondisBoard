//! WASAPI smoke test (opt-in: JB_DEVICE_SMOKE=1): lists devices and opens ONLY the default
//! headphones/speakers output for 2 s, rendering silence (no clips, volume 0). It never opens a
//! CABLE device or a microphone, so nothing reaches Discord or a game and nothing is recorded.

use jondisboard::audio::engine::{Notify, Player};
use jondisboard::audio::wasapi::{self, ComScope, Kind};
use std::sync::Arc;

#[test]
fn headphones_stream_runs() {
    if std::env::var_os("JB_DEVICE_SMOKE").is_none() {
        println!("JB_DEVICE_SMOKE not set: skipped");
        return;
    }
    let _com = ComScope::mta();
    let outs = wasapi::list(Kind::Output);
    let ins = wasapi::list(Kind::Input);
    println!("outputs: {:?}", outs.iter().map(|d| &d.name).collect::<Vec<_>>());
    println!("inputs:  {:?}", ins.iter().map(|d| &d.name).collect::<Vec<_>>());
    let def = wasapi::default_device(Kind::Output).expect("a default output");
    assert!(!def.name.to_lowercase().contains("cable"), "default output is a CABLE device: not testing on it");
    println!("testing on: {}", def.name);
    let notify: Notify = Arc::new(|_| {});
    let p = Player::new(None, 1 << 20, notify);
    p.levels.monitor.set(0.0);
    let warnings = p.start(None, None, Some(&def.name)).expect("start");
    println!("warnings: {warnings:?}");
    assert_eq!(p.outputs_open(), (false, true));
    std::thread::sleep(std::time::Duration::from_secs(2));
    let st = p.stats();
    println!("stats: {st:?}");
    assert!(st.blocks > 50, "the stream thread rendered blocks");
    assert_eq!(st.clipped_samples, 0);
    p.shutdown();
}
