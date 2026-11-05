//! Embeds the exe's icon (what Explorer and shortcuts show) and the application manifest
//! (per-monitor DPI awareness v2, UTF-8 code page, Windows 10/11 compatibility).
fn main() {
    println!("cargo:rerun-if-changed=res/app.rc");
    println!("cargo:rerun-if-changed=res/app.manifest");
    println!("cargo:rerun-if-changed=../assets/icon.ico");
    embed_resource::compile("res/app.rc", embed_resource::NONE)
        .manifest_required()
        .unwrap();
}
