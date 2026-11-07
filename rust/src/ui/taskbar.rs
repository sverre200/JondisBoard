//! The taskbar pin follows the theme.
//!
//! A pinned taskbar button doesn't use the window's icon (`WM_SETICON`): it shows the icon of the
//! pinned shortcut (`...\User Pinned\TaskBar\*.lnk`), which Windows took from the exe, so it stayed
//! Toxic Green. So, for the current theme:
//! - the logo is written as an .ico file (one file per theme, so Explorer's icon cache never shows
//!   a stale one) under %LOCALAPPDATA%\JondisBoard\icons;
//! - the window carries the AppUserModel relaunch properties (command, name, icon), so pinning it
//!   from the taskbar records that icon;
//! - an existing pin of this exe is pointed at it, and Explorer is told to refresh.
//!
//! Nothing here runs in tests (`test_mode`): they must never touch the user's shell.

use super::logo;
use crate::theme::{self, Theme};
use std::io::Write;
use std::path::{Path, PathBuf};
use windows::core::{Interface, GUID, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{HWND, PROPERTYKEY};
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PROPVARIANT};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemAlloc, IPersistFile, CLSCTX_INPROC_SERVER, STGM_READWRITE};
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::Shell::{IShellLinkW, SHChangeNotify, ShellLink, SHCNE_ASSOCCHANGED, SHCNE_UPDATEITEM, SHCNF_IDLIST, SHCNF_PATHW};

/// The app's taskbar identity (set for the process in main, and on the window here).
pub const APP_ID: &str = "JondisBoard.Soundboard";

/// System.AppUserModel.* (propkey.h): ID 5, RelaunchCommand 2, RelaunchIconResource 3,
/// RelaunchDisplayNameResource 4.
const AUMID_FMT: GUID = GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3);
fn key(pid: u32) -> PROPERTYKEY {
    PROPERTYKEY { fmtid: AUMID_FMT, pid }
}

/// The sizes Windows asks for (taskbar at 100-250 % scaling, Explorer, Alt+Tab).
const SIZES: [u32; 8] = [16, 20, 24, 32, 40, 48, 64, 256];

/// Where a theme's icon file lives.
pub fn icon_path(t: &Theme) -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    let slug: String = t.name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    Some(PathBuf::from(base).join("JondisBoard").join("icons").join(format!("JondisBoard-{slug}.ico")))
}

/// The logo in a theme's colours as .ico bytes (32-bit BMP images, straight alpha, bottom-up).
pub fn ico_bytes(t: &Theme) -> Vec<u8> {
    let images: Vec<Vec<u8>> = SIZES
        .iter()
        .map(|&s| {
            let px = logo::raster(s, theme::logo_tile(t), t.accent);
            let mut img = Vec::with_capacity(40 + px.len() + (s * s / 8) as usize);
            // BITMAPINFOHEADER: height is doubled (colour + AND mask)
            img.extend_from_slice(&40u32.to_le_bytes());
            img.extend_from_slice(&(s as i32).to_le_bytes());
            img.extend_from_slice(&(2 * s as i32).to_le_bytes());
            img.extend_from_slice(&1u16.to_le_bytes());
            img.extend_from_slice(&32u16.to_le_bytes());
            img.extend_from_slice(&[0u8; 24]);
            for y in (0..s).rev() {
                for x in 0..s {
                    let i = ((y * s + x) * 4) as usize;
                    let a = px[i + 3];
                    // raster() is premultiplied; .ico wants straight alpha
                    let un = |c: u8| if a == 0 { 0 } else { ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8 };
                    img.extend_from_slice(&[un(px[i]), un(px[i + 1]), un(px[i + 2]), a]);
                }
            }
            // the AND mask (all 0: the alpha channel decides), rows padded to 4 bytes
            let row = (s.div_ceil(32) * 4) as usize;
            img.extend(std::iter::repeat_n(0u8, row * s as usize));
            img
        })
        .collect();
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(SIZES.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * SIZES.len() as u32;
    for (s, img) in SIZES.iter().zip(&images) {
        let d = if *s >= 256 { 0 } else { *s as u8 };
        out.extend_from_slice(&[d, d, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(img.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for img in images {
        out.extend_from_slice(&img);
    }
    out
}

/// Write a theme's icon file (if it isn't there yet). Returns its path.
pub fn ensure_icon(t: &Theme) -> Option<PathBuf> {
    let p = icon_path(t)?;
    if !p.exists() {
        std::fs::create_dir_all(p.parent()?).ok()?;
        let tmp = p.with_extension("ico.tmp");
        std::fs::File::create(&tmp).and_then(|mut f| f.write_all(&ico_bytes(t))).ok()?;
        std::fs::rename(&tmp, &p).ok()?;
    }
    Some(p)
}

fn lpwstr(s: &str) -> PROPVARIANT {
    let w: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: a CoTaskMem copy of the string, owned by the PROPVARIANT (PropVariantClear frees it).
    unsafe {
        let p = CoTaskMemAlloc(w.len() * 2) as *mut u16;
        std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
        let mut pv = PROPVARIANT::default();
        (*pv.Anonymous.Anonymous).vt = VT_LPWSTR;
        (*pv.Anonymous.Anonymous).Anonymous.pwszVal = PWSTR(p);
        pv
    }
}

/// The window's relaunch properties: pinning it from the taskbar then records this icon (and
/// starts this exe with this name).
pub fn set_window_relaunch(hwnd: HWND, app_id: &str, exe: &Path, ico: &Path) {
    // SAFETY: the window's own property store; every PROPVARIANT is cleared after use.
    unsafe {
        let Ok(store) = SHGetPropertyStoreForWindow::<IPropertyStore>(hwnd) else { return };
        let props = [
            (key(5), app_id.to_string()),
            (key(2), format!("\"{}\"", exe.display())),
            (key(4), "JondisBoard".to_string()),
            (key(3), format!("{},0", ico.display())),
        ];
        for (k, v) in props {
            let mut pv = lpwstr(&v);
            let _ = store.SetValue(&k, &pv);
            let _ = PropVariantClear(&mut pv);
        }
        let _ = store.Commit();
    }
}

/// Point every .lnk in `dir` that starts `exe` at `ico`. Returns how many changed.
pub fn update_pins(dir: &Path, exe: &Path, ico: &Path) -> usize {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    let exe_l = exe.to_string_lossy().to_lowercase();
    let ico_s = ico.to_string_lossy().to_string();
    let mut n = 0;
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().map(|x| !x.eq_ignore_ascii_case("lnk")).unwrap_or(true) {
            continue;
        }
        // SAFETY: plain shell link COM calls on a link file we read and write.
        unsafe {
            let Ok(link) = CoCreateInstance::<_, IShellLinkW>(&ShellLink, None, CLSCTX_INPROC_SERVER) else { continue };
            let Ok(file) = link.cast::<IPersistFile>() else { continue };
            let wp = HSTRING::from(p.as_os_str());
            if file.Load(PCWSTR(wp.as_ptr()), STGM_READWRITE).is_err() {
                continue;
            }
            let mut target = [0u16; 1024];
            if link.GetPath(&mut target, std::ptr::null_mut(), 0).is_err() {
                continue;
            }
            let end = target.iter().position(|c| *c == 0).unwrap_or(target.len());
            if String::from_utf16_lossy(&target[..end]).to_lowercase() != exe_l {
                continue;
            }
            let mut cur = [0u16; 1024];
            let mut idx = 0i32;
            let _ = link.GetIconLocation(&mut cur, &mut idx);
            let cend = cur.iter().position(|c| *c == 0).unwrap_or(cur.len());
            if String::from_utf16_lossy(&cur[..cend]).eq_ignore_ascii_case(&ico_s) && idx == 0 {
                continue; // already this theme
            }
            if link.SetIconLocation(&HSTRING::from(ico_s.as_str()), 0).is_ok() && file.Save(PCWSTR::null(), true).is_ok() {
                SHChangeNotify(SHCNE_UPDATEITEM, SHCNF_PATHW, Some(wp.as_ptr() as *const _), None);
                n += 1;
            }
        }
    }
    if n > 0 {
        // SAFETY: a broadcast with no items.
        unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
    }
    n
}

/// For the current theme: the window's relaunch icon now; the icon file and any existing pin
/// on a worker thread (writing the file takes a few ms, the pin a few more).
pub fn follow_theme(hwnd: HWND, t: &Theme) {
    if super::app::test_mode() {
        return;
    }
    let (Some(ico), Ok(exe)) = (icon_path(t), std::env::current_exe()) else { return };
    let t = *t;
    let _ = std::thread::Builder::new().name("taskbar icon".into()).spawn(move || {
        let _com = crate::audio::wasapi::ComScope::mta();
        if ensure_icon(&t).is_none() {
            crate::log::warn("couldn't write the taskbar icon");
            return;
        }
        let n: usize = pin_dirs().iter().map(|d| update_pins(d, &exe, &ico)).sum();
        if n > 0 {
            crate::log::info(&format!("taskbar pin now uses the {} icon", t.name));
        }
    });
    // the window's own properties belong to the UI thread
    if let Ok(exe) = std::env::current_exe() {
        set_window_relaunch(hwnd, APP_ID, &exe, &icon_path(&t).unwrap_or_default());
    }
}

/// Where this user's taskbar pins live: `User Pinned\TaskBar`, and the folders under
/// `User Pinned\ImplicitAppShortcuts\<hash>`, which is where Windows puts the pin of a window with
/// its own AppUserModelID and relaunch properties (that is, ours).
pub fn pin_dirs() -> Vec<PathBuf> {
    match std::env::var_os("APPDATA") {
        Some(base) => pin_dirs_in(&PathBuf::from(base).join(r"Microsoft\Internet Explorer\Quick Launch\User Pinned")),
        None => Vec::new(),
    }
}

pub fn pin_dirs_in(user_pinned: &Path) -> Vec<PathBuf> {
    let mut out = vec![user_pinned.join("TaskBar")];
    if let Ok(rd) = std::fs::read_dir(user_pinned.join("ImplicitAppShortcuts")) {
        out.extend(rd.flatten().filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false)).map(|e| e.path()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, LoadImageW, HICON, IMAGE_ICON, LR_LOADFROMFILE};

    fn make_link(link: &Path, target: &Path) {
        // SAFETY: plain shell link COM calls on a throwaway file.
        unsafe {
            let l: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
            l.SetPath(&HSTRING::from(target.as_os_str())).unwrap();
            let f: IPersistFile = l.cast().unwrap();
            f.Save(&HSTRING::from(link.as_os_str()), true).unwrap();
        }
    }

    fn icon_of(link: &Path) -> String {
        // SAFETY: as above.
        unsafe {
            let l: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
            let f: IPersistFile = l.cast().unwrap();
            f.Load(&HSTRING::from(link.as_os_str()), STGM_READWRITE).unwrap();
            let mut buf = [0u16; 1024];
            let mut i = 0;
            l.GetIconLocation(&mut buf, &mut i).unwrap();
            let end = buf.iter().position(|c| *c == 0).unwrap();
            String::from_utf16_lossy(&buf[..end])
        }
    }

    /// `JB_ICO_DIR=<folder> cargo test --release --lib dump_theme_icons -- --ignored` writes every
    /// theme's .ico there, to look at.
    #[test]
    #[ignore]
    fn dump_theme_icons() {
        let dir = PathBuf::from(std::env::var_os("JB_ICO_DIR").expect("JB_ICO_DIR"));
        std::fs::create_dir_all(&dir).unwrap();
        for t in theme::all() {
            std::fs::write(dir.join(icon_path(&t).unwrap().file_name().unwrap()), ico_bytes(&t)).unwrap();
        }
    }

    #[test]
    fn the_ico_loads_at_every_size() {
        let dir = std::env::temp_dir().join(format!("jb_ico_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.ico");
        std::fs::write(&p, ico_bytes(&theme::by_name("Crimson"))).unwrap();
        for s in [16, 32, 256] {
            // SAFETY: loading our own file; the icon is destroyed right after.
            unsafe {
                let h = LoadImageW(None, &HSTRING::from(p.as_os_str()), IMAGE_ICON, s, s, LR_LOADFROMFILE).expect("Windows reads the .ico");
                let _ = DestroyIcon(HICON(h.0));
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_pins_of_this_exe_get_the_theme_icon() {
        let _com = crate::audio::wasapi::ComScope::mta();
        let dir = std::env::temp_dir().join(format!("jb_pins_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("JondisBoard.exe");
        let other = dir.join("Other.exe");
        std::fs::write(&exe, b"").unwrap();
        std::fs::write(&other, b"").unwrap();
        make_link(&dir.join("JondisBoard.lnk"), &exe);
        make_link(&dir.join("Other.lnk"), &other);
        let ico = dir.join("JondisBoard-crimson.ico");
        std::fs::write(&ico, ico_bytes(&theme::by_name("Crimson"))).unwrap();
        assert_eq!(update_pins(&dir, &exe, &ico), 1);
        assert_eq!(icon_of(&dir.join("JondisBoard.lnk")).to_lowercase(), ico.to_string_lossy().to_lowercase());
        assert_eq!(icon_of(&dir.join("Other.lnk")), "");
        // already pointing there: nothing to do
        assert_eq!(update_pins(&dir, &exe, &ico), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pins_are_looked_for_in_taskbar_and_implicit_app_shortcuts() {
        let root = std::env::temp_dir().join(format!("jb_userpinned_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("TaskBar")).unwrap();
        std::fs::create_dir_all(root.join("ImplicitAppShortcuts").join("3f5545f95d688c2e")).unwrap();
        std::fs::create_dir_all(root.join("ImplicitAppShortcuts").join("aa11")).unwrap();
        let dirs = pin_dirs_in(&root);
        assert_eq!(dirs.len(), 3);
        assert!(dirs.contains(&root.join("TaskBar")));
        assert!(dirs.contains(&root.join("ImplicitAppShortcuts").join("3f5545f95d688c2e")));
        let _ = std::fs::remove_dir_all(&root);
    }
}
