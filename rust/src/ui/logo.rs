//! The JondisBoard logo: a matte rounded tile with a waveform in the theme's accent colour
//! (`logo.py`). The same geometry is drawn with Direct2D in the header and rasterised here (4x4
//! supersampling per pixel, each size on its own pixel grid) for the window and taskbar icons.

use crate::theme::{logo_tile, Rgb, Theme};
use windows::Win32::Graphics::Gdi::{CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS};
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, DestroyIcon, HICON, ICONINFO};

/// Rounded rectangles (x, y, w, h, radius) in a size x size square.
pub fn shapes(size: f32, tile: bool) -> (Option<[f32; 5]>, Vec<[f32; 5]>) {
    let tile_rect = if tile {
        let inset = (size * 0.04).round().max(0.0);
        Some([inset, inset, size - 2.0 * inset, size - 2.0 * inset, size * 0.225])
    } else {
        None
    };
    let (heights, bar, gap, span): (&[f32], f32, f32, f32) = if size <= 24.0 {
        (&[0.55, 1.0, 0.55], (size * 0.125).round().max(2.0), 2.0, size * 0.68)
    } else {
        (&[0.34, 0.62, 0.86, 0.62, 0.34], (size * 0.1).round().max(2.0), (size * 0.05).round().max(1.0), size * 0.56)
    };
    let n = heights.len() as f32;
    let total = n * bar + (n - 1.0) * gap;
    let mut x = ((size - total) / 2.0).floor();
    let mut bars = Vec::new();
    for h in heights {
        let hh = (span * h).round().max(bar);
        let y = (size - hh) / 2.0;
        bars.push([x, y, bar, hh, bar / 2.0]);
        x += bar + gap;
    }
    (tile_rect, bars)
}

fn inside(r: &[f32; 5], px: f32, py: f32) -> bool {
    let [x, y, w, h, rad] = *r;
    if px < x || py < y || px > x + w || py > y + h {
        return false;
    }
    let rad = rad.min(w / 2.0).min(h / 2.0);
    let cx = px.clamp(x + rad, x + w - rad);
    let cy = py.clamp(y + rad, y + h - rad);
    let (dx, dy) = (px - cx, py - cy);
    dx * dx + dy * dy <= rad * rad
}

/// The logo as premultiplied BGRA pixels (top-down).
pub fn raster(size: u32, tile: Rgb, mark: Rgb) -> Vec<u8> {
    let (tile_rect, bars) = shapes(size as f32, true);
    let mut out = vec![0u8; (size * size * 4) as usize];
    const K: u32 = 4;
    for py in 0..size {
        for px in 0..size {
            let (mut t, mut m) = (0u32, 0u32);
            for sy in 0..K {
                for sx in 0..K {
                    let fx = px as f32 + (sx as f32 + 0.5) / K as f32;
                    let fy = py as f32 + (sy as f32 + 0.5) / K as f32;
                    if bars.iter().any(|b| inside(b, fx, fy)) {
                        m += 1;
                    } else if tile_rect.as_ref().map(|r| inside(r, fx, fy)).unwrap_or(false) {
                        t += 1;
                    }
                }
            }
            let total = (K * K) as f32;
            let (ta, ma) = (t as f32 / total, m as f32 / total);
            let a = ta + ma;
            let ch = |tc: u8, mc: u8| (tc as f32 * ta + mc as f32 * ma).round().min(255.0) as u8;
            let i = ((py * size + px) * 4) as usize;
            out[i] = ch(tile.b(), mark.b());
            out[i + 1] = ch(tile.g(), mark.g());
            out[i + 2] = ch(tile.r(), mark.r());
            out[i + 3] = (a * 255.0).round().min(255.0) as u8;
        }
    }
    out
}

/// An HICON of the logo at `size` px for a theme. The caller destroys it (see `Icon`).
pub fn icon(theme: &Theme, size: u32) -> Option<Icon> {
    let px = raster(size, logo_tile(theme), theme.accent);
    // SAFETY: a top-down 32-bit DIB section of size x size, filled from `px`, plus a monochrome
    // mask; both bitmaps are deleted after CreateIconIndirect copied them.
    unsafe {
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size as i32,
                biHeight: -(size as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let color = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u8, px.len());
        let mask = CreateBitmap(size as i32, size as i32, 1, 1, None);
        let info = ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let h = CreateIconIndirect(&info).ok();
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        h.map(Icon)
    }
}

/// An owned HICON.
pub struct Icon(pub HICON);

impl Drop for Icon {
    fn drop(&mut self) {
        // SAFETY: we created this icon and nothing else owns it.
        unsafe {
            let _ = DestroyIcon(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centre_is_the_mark_and_corner_is_clear() {
        let px = raster(32, Rgb(0x101010), Rgb(0x45e33f));
        let at = |x: u32, y: u32| {
            let i = ((y * 32 + x) * 4) as usize;
            (px[i + 2], px[i + 1], px[i], px[i + 3])
        };
        assert_eq!(at(16, 16), (0x45, 0xe3, 0x3f, 255)); // middle bar
        assert_eq!(at(0, 0).3, 0); // rounded-off corner
        assert_eq!(at(16, 2), (0x10, 0x10, 0x10, 255)); // the tile above the bars
    }

    #[test]
    fn small_sizes_get_three_bars() {
        assert_eq!(shapes(16.0, true).1.len(), 3);
        assert_eq!(shapes(48.0, true).1.len(), 5);
    }
}
