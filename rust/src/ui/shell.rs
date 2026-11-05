//! Small Windows shell helpers: the clipboard, file pickers, opening folders/links, and the
//! Recycle Bin. Each `unsafe` block is a plain Win32/COM call with checked handles.

use std::path::{Path, PathBuf};
use windows::core::{Interface, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::{CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

struct Clipboard;

impl Clipboard {
    fn open(hwnd: HWND) -> Option<Clipboard> {
        // another app can hold the clipboard for a moment: try a few times
        for _ in 0..5 {
            // SAFETY: plain call; closed in Drop.
            if unsafe { OpenClipboard(Some(hwnd)) }.is_ok() {
                return Some(Clipboard);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        None
    }
}

impl Drop for Clipboard {
    fn drop(&mut self) {
        // SAFETY: balances the successful OpenClipboard.
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

fn with_global<T>(h: HANDLE, f: impl FnOnce(*const u8, usize) -> T) -> Option<T> {
    let g = HGLOBAL(h.0);
    // SAFETY: h is clipboard data owned by the clipboard; locked for the duration of f only.
    unsafe {
        let p = GlobalLock(g) as *const u8;
        if p.is_null() {
            return None;
        }
        let r = f(p, GlobalSize(g));
        let _ = GlobalUnlock(g);
        Some(r)
    }
}

/// Tests never touch the real clipboard: with JONDISBOARD_TEST=1 it's a process-local string.
fn test_clipboard() -> Option<&'static std::sync::Mutex<String>> {
    static CLIP: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();
    if std::env::var_os("JONDISBOARD_TEST").is_some() {
        Some(CLIP.get_or_init(|| std::sync::Mutex::new(String::new())))
    } else {
        None
    }
}

/// Tests: put text on the stand-in clipboard.
pub fn set_test_clipboard(text: &str) {
    if let Some(c) = test_clipboard() {
        *c.lock().unwrap() = text.to_string();
    }
}

pub fn clipboard_text(hwnd: HWND) -> Option<String> {
    if let Some(c) = test_clipboard() {
        let s = c.lock().unwrap().clone();
        return if s.is_empty() { None } else { Some(s) };
    }
    let _c = Clipboard::open(hwnd)?;
    // SAFETY: the clipboard is open; the handle stays valid until it's closed.
    let h = unsafe { GetClipboardData(CF_UNICODETEXT.0 as u32) }.ok()?;
    with_global(h, |p, size| {
        // SAFETY: CF_UNICODETEXT is UTF-16, `size` bytes long, NUL-terminated within it.
        let s = unsafe { std::slice::from_raw_parts(p as *const u16, size / 2) };
        let end = s.iter().position(|c| *c == 0).unwrap_or(s.len());
        String::from_utf16_lossy(&s[..end])
    })
}

pub fn set_clipboard_text(hwnd: HWND, text: &str) {
    if let Some(c) = test_clipboard() {
        *c.lock().unwrap() = text.to_string();
        return;
    }
    let Some(_c) = Clipboard::open(hwnd) else { return };
    let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    // SAFETY: a moveable global block of the right size, filled while locked; on success the
    // clipboard owns it.
    unsafe {
        let _ = EmptyClipboard();
        let Ok(g) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) else { return };
        let p = GlobalLock(g) as *mut u16;
        if p.is_null() {
            return;
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
        let _ = GlobalUnlock(g);
        let _ = SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(g.0)));
    }
}

/// A picture on the clipboard.
pub enum ClipImage {
    /// a copied image: top-down BGRA (straight alpha), w x h
    Pixels(u32, u32, Vec<u8>),
    /// a copied picture file (Explorer)
    File(PathBuf),
}

pub fn clipboard_has_image() -> bool {
    if test_clipboard().is_some() {
        return false;
    }
    // SAFETY: plain queries.
    unsafe {
        IsClipboardFormatAvailable(CF_DIB.0 as u32).is_ok()
            || IsClipboardFormatAvailable(CF_DIBV5.0 as u32).is_ok()
            || IsClipboardFormatAvailable(CF_HDROP.0 as u32).is_ok()
    }
}

/// Decode a DIB (BITMAPINFOHEADER / V5 + pixels) into top-down BGRA.
pub fn dib_to_bgra(d: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    if d.len() < 40 {
        return None;
    }
    let rd32 = |o: usize| u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]);
    let hsize = rd32(0) as usize;
    let w = rd32(4) as i32;
    let h = rd32(8) as i32;
    let bits = u16::from_le_bytes([d[14], d[15]]);
    let compression = rd32(16);
    let used = rd32(32) as usize;
    if w <= 0 || h == 0 || !(bits == 24 || bits == 32) {
        return None;
    }
    let (w, top_down, h) = (w as u32, h < 0, h.unsigned_abs());
    // BI_BITFIELDS with a plain header: 3 masks follow the header
    let masks = if compression == 3 && hsize == 40 { 12 } else { 0 };
    let palette = if bits <= 8 { (if used == 0 { 1 << bits } else { used }) * 4 } else { used * 4 };
    let off = hsize + masks + palette;
    let stride = ((w * bits as u32 + 31) / 32 * 4) as usize;
    if d.len() < off + stride * h as usize {
        return None;
    }
    let mut out = vec![0u8; (w * h * 4) as usize];
    let mut any_alpha = false;
    for y in 0..h as usize {
        let src_y = if top_down { y } else { h as usize - 1 - y };
        let row = &d[off + src_y * stride..];
        for x in 0..w as usize {
            let o = (y * w as usize + x) * 4;
            let (b, g, r, a) = if bits == 32 {
                (row[x * 4], row[x * 4 + 1], row[x * 4 + 2], row[x * 4 + 3])
            } else {
                (row[x * 3], row[x * 3 + 1], row[x * 3 + 2], 255)
            };
            any_alpha |= a != 0;
            out[o..o + 4].copy_from_slice(&[b, g, r, a]);
        }
    }
    if bits == 32 && !any_alpha {
        // many apps put 0 in the unused byte: treat it as opaque
        for px in out.chunks_exact_mut(4) {
            px[3] = 255;
        }
    }
    Some((w, h, out))
}

pub fn clipboard_image(hwnd: HWND) -> Option<ClipImage> {
    if test_clipboard().is_some() {
        return None;
    }
    let _c = Clipboard::open(hwnd)?;
    // SAFETY: the clipboard is open; the data handles stay valid until it's closed.
    unsafe {
        for fmt in [CF_DIBV5, CF_DIB] {
            if let Ok(h) = GetClipboardData(fmt.0 as u32) {
                if let Some(Some((w, hh, px))) = with_global(h, |p, size| dib_to_bgra(std::slice::from_raw_parts(p, size))) {
                    return Some(ClipImage::Pixels(w, hh, px));
                }
            }
        }
        if let Ok(h) = GetClipboardData(CF_HDROP.0 as u32) {
            let drop = HDROP(h.0);
            let n = DragQueryFileW(drop, u32::MAX, None);
            for i in 0..n {
                let len = DragQueryFileW(drop, i, None) as usize;
                let mut buf = vec![0u16; len + 1];
                DragQueryFileW(drop, i, Some(&mut buf));
                let p = PathBuf::from(String::from_utf16_lossy(&buf[..len]));
                if crate::library::PICTURE_EXTS.contains(&p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase().as_str()) {
                    return Some(ClipImage::File(p));
                }
            }
        }
    }
    None
}

/// The Windows file picker. `filters`: (label, "*.wav;*.mp3").
pub fn open_files(hwnd: HWND, title: &str, filters: &[(&str, &str)], multi: bool) -> Vec<PathBuf> {
    // SAFETY: COM file dialog on the UI (STA) thread; every string outlives the call that uses
    // it and returned paths are freed with CoTaskMemFree.
    unsafe {
        let Ok(d) = CoCreateInstance::<_, IFileOpenDialog>(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) else { return vec![] };
        let names: Vec<HSTRING> = filters.iter().map(|f| HSTRING::from(f.0)).collect();
        let specs_s: Vec<HSTRING> = filters.iter().map(|f| HSTRING::from(f.1)).collect();
        let specs: Vec<COMDLG_FILTERSPEC> = names
            .iter()
            .zip(&specs_s)
            .map(|(n, s)| COMDLG_FILTERSPEC { pszName: PCWSTR(n.as_ptr()), pszSpec: PCWSTR(s.as_ptr()) })
            .collect();
        let _ = d.SetFileTypes(&specs);
        let _ = d.SetTitle(&HSTRING::from(title));
        let mut opts = d.GetOptions().unwrap_or_default();
        opts |= FOS_FORCEFILESYSTEM | FOS_FILEMUSTEXIST;
        if multi {
            opts |= FOS_ALLOWMULTISELECT;
        }
        let _ = d.SetOptions(opts);
        if d.Show(Some(hwnd)).is_err() {
            return vec![];
        }
        let Ok(items) = d.GetResults() else { return vec![] };
        let n = items.GetCount().unwrap_or(0);
        let mut out = Vec::new();
        for i in 0..n {
            if let Ok(item) = items.GetItemAt(i) {
                if let Ok(p) = item.GetDisplayName(SIGDN_FILESYSPATH) {
                    out.push(PathBuf::from(p.to_string().unwrap_or_default()));
                    CoTaskMemFree(Some(p.0 as *const _));
                }
            }
        }
        out
    }
}

/// Open a folder in Explorer, or a link in the browser.
pub fn open(target: &str) {
    // SAFETY: plain ShellExecute with valid strings.
    unsafe {
        ShellExecuteW(None, &HSTRING::from("open"), &HSTRING::from(target), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

/// Move a file to the Recycle Bin (never a permanent delete).
pub fn recycle(path: &Path) -> bool {
    let wide: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().chain([0, 0]).collect();
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(wide.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI).0 as u16,
        ..Default::default()
    };
    // SAFETY: pFrom is a double-NUL-terminated list that outlives the call.
    unsafe { SHFileOperationW(&mut op) == 0 && !op.fAnyOperationsAborted.as_bool() }
}

#[allow(dead_code)]
fn _iface_check(d: &IFileOpenDialog) -> Option<IFileDialog> {
    d.cast().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dib_24_bottom_up() {
        // 2x2, 24-bit, bottom-up: rows padded to 8 bytes
        let mut d = vec![0u8; 40];
        d[0] = 40;
        d[4] = 2;
        d[8] = 2;
        d[12] = 1;
        d[14] = 24;
        // bottom row: blue, green; top row: red, white
        d.extend_from_slice(&[255, 0, 0, 0, 255, 0, 0, 0]);
        d.extend_from_slice(&[0, 0, 255, 255, 255, 255, 0, 0]);
        let (w, h, px) = dib_to_bgra(&d).unwrap();
        assert_eq!((w, h), (2, 2));
        assert_eq!(&px[0..4], &[0, 0, 255, 255]); // top-left red
        assert_eq!(&px[8..12], &[255, 0, 0, 255]); // bottom-left blue
    }
}
