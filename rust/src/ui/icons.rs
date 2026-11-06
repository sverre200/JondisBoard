//! The app's one family of line icons (same shapes as the Python app's `icon_pil`), drawn as
//! vectors so they're crisp at any DPI and share one stroke weight.

use super::gfx::{Canvas, Color, Gfx};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Search,
    Sliders,
    Folder,
    Refresh,
    Note,
    Stop,
    Check,
    ChevronDown,
    ChevronUp,
    ChevronRight,
    ChevronLeft,
    /// four squares: All sounds
    Grid,
    Bolt,
    Link,
    X,
    More,
    Plus,
}

/// Draw `icon` in the square (x, y, size).
pub fn draw(c: &Canvas, g: &Gfx, icon: Icon, x: f32, y: f32, size: f32, color: impl Into<Color>) {
    let color = color.into();
    let n = size;
    let w = (n * 0.11).max(1.2);
    let p = |px: f32, py: f32| (x + px * n, y + py * n);
    let line = |pts: &[(f32, f32)]| {
        let v: Vec<(f32, f32)> = pts.iter().map(|q| p(q.0, q.1)).collect();
        c.path(g, &v, false, false, w, color);
    };
    match icon {
        Icon::Search => {
            let (cx, cy) = p(0.39, 0.39);
            c.stroke_circle(cx, cy, 0.29 * n - w / 2.0, w, color);
            line(&[(0.64, 0.64), (0.88, 0.88)]);
        }
        Icon::Sliders => {
            for (yy, k) in [(0.22, 0.66), (0.5, 0.32), (0.78, 0.58)] {
                line(&[(0.1, yy), (0.9, yy)]);
                let (cx, cy) = p(k, yy);
                c.fill_circle(cx, cy, 0.12 * n, color);
            }
        }
        Icon::Folder => line(&[(0.08, 0.8), (0.08, 0.2), (0.38, 0.2), (0.5, 0.33), (0.92, 0.33), (0.92, 0.8), (0.08, 0.8)]),
        Icon::Refresh => {
            let (cx, cy) = p(0.5, 0.5);
            // PIL arcs start at 3 o'clock and go clockwise: 40..320 degrees
            c.arc(g, cx, cy, 0.37 * n - w / 2.0, 40.0, 320.0, w, color);
            let tri = [p(0.93, 0.43), p(0.65, 0.34), p(0.88, 0.15)];
            c.path(g, &tri, true, true, w, color);
        }
        Icon::Note => {
            line(&[(0.64, 0.7), (0.64, 0.12), (0.88, 0.24)]);
            let (cx, cy) = p(0.42, 0.73);
            c.fill_circle(cx, cy, 0.2 * n, color);
        }
        Icon::Stop => {
            let (x0, y0) = p(0.18, 0.18);
            c.fill_rrect(super::gfx::Rect::new(x0, y0, 0.64 * n, 0.64 * n), 0.14 * n, color);
        }
        Icon::Check => line(&[(0.16, 0.54), (0.4, 0.76), (0.86, 0.26)]),
        Icon::ChevronDown => line(&[(0.18, 0.36), (0.5, 0.66), (0.82, 0.36)]),
        Icon::ChevronUp => line(&[(0.18, 0.66), (0.5, 0.36), (0.82, 0.66)]),
        Icon::ChevronRight => line(&[(0.36, 0.18), (0.66, 0.5), (0.36, 0.82)]),
        Icon::ChevronLeft => line(&[(0.64, 0.18), (0.34, 0.5), (0.64, 0.82)]),
        Icon::Grid => {
            for (a, b) in [(0.1, 0.1), (0.56, 0.1), (0.1, 0.56), (0.56, 0.56)] {
                let (x0, y0) = p(a, b);
                c.stroke_rrect(super::gfx::Rect::new(x0, y0, 0.34 * n, 0.34 * n), 0.08 * n, w, color);
            }
        }
        Icon::Bolt => {
            let pts = [p(0.6, 0.04), p(0.18, 0.57), p(0.46, 0.57), p(0.4, 0.96), p(0.82, 0.42), p(0.54, 0.42)];
            c.path(g, &pts, true, true, w, color);
        }
        Icon::Link => {
            for a in [0.04, 0.42] {
                let (x0, y0) = p(a, 0.3);
                c.stroke_rrect(super::gfx::Rect::new(x0, y0, 0.54 * n, 0.4 * n), 0.2 * n, w, color);
            }
        }
        Icon::X => {
            line(&[(0.24, 0.24), (0.76, 0.76)]);
            line(&[(0.76, 0.24), (0.24, 0.76)]);
        }
        Icon::More => {
            for xx in [0.18, 0.5, 0.82] {
                let (cx, cy) = p(xx, 0.5);
                c.fill_circle(cx, cy, 0.1 * n, color);
            }
        }
        Icon::Plus => {
            let t = 0.2 * n;
            let (x0, y0) = p(0.0, 0.0);
            c.fill_rrect(super::gfx::Rect::new(x0 + (n - t) / 2.0, y0, t, n), t / 2.0, color);
            c.fill_rrect(super::gfx::Rect::new(x0, y0 + (n - t) / 2.0, n, t), t / 2.0, color);
        }
    }
}
