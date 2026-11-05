//! Drawing: Direct2D for shapes and pictures, DirectWrite for text (Segoe UI, like the Python
//! app). Everything is drawn in DIPs (device-independent pixels); the render target's DPI does
//! the scaling, so the same layout code is sharp at 100 %, 150 % or 200 %.
//!
//! All `unsafe` here is calls into those COM interfaces with valid, live arguments. Text layouts
//! are cached (creating one per frame is the expensive part of text), as are brushes and pictures.

use crate::theme::Rgb;
use std::collections::HashMap;
use windows::core::w;
use windows::Win32::Foundation::{D2DERR_RECREATE_TARGET, HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows_numerics::{Matrix3x2, Vector2};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
    pub fn inset(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, (self.w - 2.0 * dx).max(0.0), (self.h - 2.0 * dy).max(0.0))
    }
    pub fn offset(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w, self.h)
    }
    pub fn intersects(&self, o: &Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }
    pub fn cx(&self) -> f32 {
        self.x + self.w / 2.0
    }
    pub fn cy(&self) -> f32 {
        self.y + self.h / 2.0
    }
    fn d2d(&self) -> D2D_RECT_F {
        D2D_RECT_F { left: self.x, top: self.y, right: self.x + self.w, bottom: self.y + self.h }
    }
}

/// A colour with alpha.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub rgb: Rgb,
    pub a: f32,
}

impl From<Rgb> for Color {
    fn from(rgb: Rgb) -> Color {
        Color { rgb, a: 1.0 }
    }
}

impl Rgb {
    pub fn alpha(self, a: f32) -> Color {
        Color { rgb: self, a }
    }
}

fn d2d_color(c: Color) -> D2D1_COLOR_F {
    // sRGB values straight through (like GDI/Tk did), so the colours match the Python app
    D2D1_COLOR_F { r: c.rgb.r() as f32 / 255.0, g: c.rgb.g() as f32 / 255.0, b: c.rgb.b() as f32 / 255.0, a: c.a }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Weight {
    Normal,
    Bold,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct LayoutKey {
    text: String,
    size: u32,
    weight: Weight,
    width: u32,
    lines: u8,
    align: Align,
}

pub struct TextLayout {
    layout: IDWriteTextLayout,
    pub width: f32,
    pub height: f32,
    pub lines: u32,
    used: u64,
}

impl TextLayout {
    /// Caret x for a character index (UTF-16 units).
    pub fn caret_x(&self, pos: u32) -> f32 {
        let (mut x, mut y) = (0.0, 0.0);
        let mut m = DWRITE_HIT_TEST_METRICS::default();
        // SAFETY: valid layout and out-pointers.
        unsafe {
            let _ = self.layout.HitTestTextPosition(pos, false, &mut x, &mut y, &mut m);
        }
        x
    }
    /// Character index (UTF-16 units) nearest to x.
    pub fn hit(&self, x: f32) -> u32 {
        let mut trailing = windows::core::BOOL(0);
        let mut inside = windows::core::BOOL(0);
        let mut m = DWRITE_HIT_TEST_METRICS::default();
        // SAFETY: valid layout and out-pointers.
        unsafe {
            let _ = self.layout.HitTestPoint(x, 1.0, &mut trailing, &mut inside, &mut m);
        }
        m.textPosition + if trailing.as_bool() { m.length } else { 0 }
    }
}

pub struct Bitmap {
    pub bmp: ID2D1Bitmap,
    pub w: u32,
    pub h: u32,
}

/// The factories: one set for the app.
pub struct Gfx {
    pub d2d: ID2D1Factory,
    pub dwrite: IDWriteFactory,
    formats: HashMap<(u32, Weight), IDWriteTextFormat>,
    trimming: HashMap<(u32, Weight), IDWriteInlineObject>,
    layouts: HashMap<LayoutKey, TextLayout>,
    clock: u64,
    round: Option<ID2D1StrokeStyle>,
}

impl Gfx {
    pub fn new() -> windows::core::Result<Gfx> {
        // SAFETY: plain factory creation.
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let round = d2d
                .CreateStrokeStyle(
                    &D2D1_STROKE_STYLE_PROPERTIES {
                        startCap: D2D1_CAP_STYLE_ROUND,
                        endCap: D2D1_CAP_STYLE_ROUND,
                        dashCap: D2D1_CAP_STYLE_ROUND,
                        lineJoin: D2D1_LINE_JOIN_ROUND,
                        miterLimit: 10.0,
                        dashStyle: D2D1_DASH_STYLE_SOLID,
                        dashOffset: 0.0,
                    },
                    None,
                )
                .ok();
            Ok(Gfx { d2d, dwrite, formats: HashMap::new(), trimming: HashMap::new(), layouts: HashMap::new(), clock: 0, round })
        }
    }

    fn format(&mut self, size: f32, weight: Weight) -> Option<(IDWriteTextFormat, IDWriteInlineObject)> {
        let key = ((size * 100.0) as u32, weight);
        if !self.formats.contains_key(&key) {
            let wt = if weight == Weight::Bold { DWRITE_FONT_WEIGHT_BOLD } else { DWRITE_FONT_WEIGHT_NORMAL };
            // SAFETY: valid factory and arguments.
            unsafe {
                let f = self
                    .dwrite
                    .CreateTextFormat(w!("Segoe UI"), None, wt, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!("en-us"))
                    .ok()?;
                let sign = self.dwrite.CreateEllipsisTrimmingSign(&f).ok()?;
                self.formats.insert(key, f);
                self.trimming.insert(key, sign);
            }
        }
        Some((self.formats[&key].clone(), self.trimming[&key].clone()))
    }

    /// A cached text layout: wrapped into at most `lines` lines of `width` DIPs (0 = no limit),
    /// ending in "…" if the rest doesn't fit.
    pub fn layout(&mut self, text: &str, size: f32, weight: Weight, width: f32, lines: u8, align: Align) -> Option<&TextLayout> {
        self.clock += 1;
        let key = LayoutKey { text: text.to_string(), size: (size * 100.0) as u32, weight, width: (width * 4.0) as u32, lines, align };
        if !self.layouts.contains_key(&key) {
            if self.layouts.len() > 6000 {
                // forget the least recently used half
                let mut ages: Vec<u64> = self.layouts.values().map(|l| l.used).collect();
                ages.sort_unstable();
                let cut = ages[ages.len() / 2];
                self.layouts.retain(|_, l| l.used > cut);
            }
            let (fmt, sign) = self.format(size, weight)?;
            let wide: Vec<u16> = text.encode_utf16().collect();
            let maxw = if width > 0.0 { width } else { 100_000.0 };
            // SAFETY: valid factory, format and string slice.
            let layout = unsafe {
                let l = self.dwrite.CreateTextLayout(&wide, &fmt, maxw, 100_000.0).ok()?;
                let _ = l.SetTextAlignment(match align {
                    Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
                    Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                    Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
                });
                if lines == 1 || width <= 0.0 {
                    let _ = l.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
                } else {
                    let _ = l.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP);
                }
                if width > 0.0 {
                    let trim = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
                    let _ = l.SetTrimming(&trim, &sign);
                }
                if lines > 0 && width > 0.0 {
                    // the height of `lines` lines: measure one line of this font
                    let one = self.dwrite.CreateTextLayout(&[b'X' as u16], &fmt, 1000.0, 1000.0).ok()?;
                    let mut lm = [DWRITE_LINE_METRICS::default(); 1];
                    let mut n = 0;
                    let _ = one.GetLineMetrics(Some(&mut lm), &mut n);
                    let _ = l.SetMaxHeight(lm[0].height * lines as f32 + 0.5);
                }
                l
            };
            let mut m = DWRITE_TEXT_METRICS::default();
            // SAFETY: valid layout and out-pointer.
            unsafe {
                let _ = layout.GetMetrics(&mut m);
            }
            self.layouts.insert(
                key.clone(),
                TextLayout { layout, width: m.widthIncludingTrailingWhitespace.min(m.layoutWidth), height: m.height, lines: m.lineCount, used: 0 },
            );
        }
        let clock = self.clock;
        let l = self.layouts.get_mut(&key)?;
        l.used = clock;
        Some(l)
    }

    /// Width of a single line of text.
    pub fn measure(&mut self, text: &str, size: f32, weight: Weight) -> f32 {
        self.layout(text, size, weight, 0.0, 1, Align::Left).map(|l| l.width).unwrap_or(0.0)
    }

    pub fn round_stroke(&self) -> Option<&ID2D1StrokeStyle> {
        self.round.as_ref()
    }
}

/// A render target: the window's, or an offscreen picture (for snapshots in tests).
pub struct Canvas {
    pub rt: ID2D1RenderTarget,
    hwnd_rt: Option<ID2D1HwndRenderTarget>,
    wic: Option<windows::Win32::Graphics::Imaging::IWICBitmap>,
    brush: ID2D1SolidColorBrush,
    pub scale: f32,
    pub gen: u64,
    bitmaps: HashMap<u64, Bitmap>,
    layers: Vec<ID2D1Layer>,
    depth: usize,
}

impl Canvas {
    pub fn new(gfx: &Gfx, hwnd: HWND, dpi: u32) -> windows::core::Result<Canvas> {
        let mut rc = RECT::default();
        // SAFETY: valid window handle and out-pointer.
        unsafe {
            let _ = GetClientRect(hwnd, &mut rc);
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE },
                dpiX: dpi as f32,
                dpiY: dpi as f32,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let hprops = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd,
                pixelSize: D2D_SIZE_U { width: (rc.right - rc.left).max(1) as u32, height: (rc.bottom - rc.top).max(1) as u32 },
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };
            let hrt = gfx.d2d.CreateHwndRenderTarget(&props, &hprops)?;
            let rt: ID2D1RenderTarget = windows::core::Interface::cast(&hrt)?;
            rt.SetDpi(dpi as f32, dpi as f32);
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let brush = rt.CreateSolidColorBrush(&d2d_color(Rgb(0).into()), None)?;
            Ok(Canvas { rt, hwnd_rt: Some(hrt), wic: None, brush, scale: dpi as f32 / 96.0, gen: 0, bitmaps: HashMap::new(), layers: Vec::new(), depth: 0 })
        }
    }

    /// An offscreen canvas of w x h pixels (tests save it as a PNG with `save_png`).
    pub fn offscreen(gfx: &Gfx, w: u32, h: u32, dpi: u32) -> windows::core::Result<Canvas> {
        use windows::Win32::Graphics::Imaging::*;
        let wic = super::images::factory()?;
        // SAFETY: a WIC bitmap and a D2D render target drawing into it.
        unsafe {
            let bmp = wic.CreateBitmap(w, h, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnLoad)?;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: dpi as f32,
                dpiY: dpi as f32,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let rt = gfx.d2d.CreateWicBitmapRenderTarget(&bmp, &props)?;
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let brush = rt.CreateSolidColorBrush(&d2d_color(Rgb(0).into()), None)?;
            Ok(Canvas { rt, hwnd_rt: None, wic: Some(bmp), brush, scale: dpi as f32 / 96.0, gen: 0, bitmaps: HashMap::new(), layers: Vec::new(), depth: 0 })
        }
    }

    /// Save an offscreen canvas as a PNG.
    pub fn save_png(&self, path: &std::path::Path) -> Result<(), String> {
        let bmp = self.wic.as_ref().ok_or("not an offscreen canvas")?;
        super::images::save_png_as_is(&windows::core::Interface::cast(bmp).map_err(|e| e.message())?, path)
    }

    /// Is the window covered (e.g. by a fullscreen game) or minimised? (As of the last frame.)
    pub fn occluded(&self) -> bool {
        // SAFETY: plain query on a valid render target.
        self.hwnd_rt.as_ref().map(|h| unsafe { h.CheckWindowState() }.0 & D2D1_WINDOW_STATE_OCCLUDED.0 != 0).unwrap_or(false)
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        if let Some(h_rt) = &self.hwnd_rt {
            // SAFETY: valid render target.
            unsafe {
                let _ = h_rt.Resize(&D2D_SIZE_U { width: w.max(1), height: h.max(1) });
            }
        }
    }

    pub fn set_dpi(&mut self, dpi: u32) {
        self.scale = dpi as f32 / 96.0;
        // SAFETY: valid render target.
        unsafe { self.rt.SetDpi(dpi as f32, dpi as f32) };
    }

    /// Size in DIPs.
    pub fn size(&self) -> (f32, f32) {
        // SAFETY: valid render target.
        let s = unsafe { self.rt.GetSize() };
        (s.width, s.height)
    }

    pub fn begin(&self, bg: Rgb) {
        // SAFETY: valid render target; balanced by end().
        unsafe {
            self.rt.BeginDraw();
            self.rt.SetTransform(&Matrix3x2::identity());
            self.rt.Clear(Some(&d2d_color(bg.into())));
        }
    }

    /// Ok(false): the target must be recreated (device lost).
    pub fn end(&self) -> bool {
        // SAFETY: valid render target, after begin().
        match unsafe { self.rt.EndDraw(None, None) } {
            Ok(()) => true,
            Err(e) => e.code() != D2DERR_RECREATE_TARGET,
        }
    }

    fn brush(&self, c: Color) -> &ID2D1SolidColorBrush {
        // SAFETY: valid brush.
        unsafe { self.brush.SetColor(&d2d_color(c)) };
        &self.brush
    }

    pub fn fill_rect(&self, r: Rect, c: impl Into<Color>) {
        let b = self.brush(c.into());
        // SAFETY: valid target and brush.
        unsafe { self.rt.FillRectangle(&r.d2d(), b) };
    }

    pub fn fill_rrect(&self, r: Rect, radius: f32, c: impl Into<Color>) {
        let b = self.brush(c.into());
        let rr = D2D1_ROUNDED_RECT { rect: r.d2d(), radiusX: radius, radiusY: radius };
        // SAFETY: valid target and brush.
        unsafe { self.rt.FillRoundedRectangle(&rr, b) };
    }

    /// A rounded outline drawn inside `r` (like a 1 px Tk border).
    pub fn stroke_rrect(&self, r: Rect, radius: f32, width: f32, c: impl Into<Color>) {
        let b = self.brush(c.into());
        let h = width / 2.0;
        let rr = D2D1_ROUNDED_RECT { rect: r.inset(h, h).d2d(), radiusX: (radius - h).max(0.0), radiusY: (radius - h).max(0.0) };
        // SAFETY: valid target and brush.
        unsafe { self.rt.DrawRoundedRectangle(&rr, b, width, None) };
    }

    /// Filled card with a 1 DIP border (one fill for the border colour, one for the inside: no
    /// seams, like the Python app's card images).
    pub fn card(&self, r: Rect, radius: f32, fill: impl Into<Color>, border: impl Into<Color>) {
        let px = 1.0;
        self.fill_rrect(r, radius, border);
        self.fill_rrect(r.inset(px, px), (radius - px).max(0.0), fill);
    }

    pub fn fill_circle(&self, cx: f32, cy: f32, r: f32, c: impl Into<Color>) {
        let b = self.brush(c.into());
        let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: r, radiusY: r };
        // SAFETY: valid target and brush.
        unsafe { self.rt.FillEllipse(&e, b) };
    }

    pub fn stroke_circle(&self, cx: f32, cy: f32, r: f32, width: f32, c: impl Into<Color>) {
        let b = self.brush(c.into());
        let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: r, radiusY: r };
        // SAFETY: valid target and brush.
        unsafe { self.rt.DrawEllipse(&e, b, width, None) };
    }

    pub fn line(&self, gfx: &Gfx, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, c: impl Into<Color>) {
        let b = self.brush(c.into());
        // SAFETY: valid target, brush and (optional) stroke style.
        unsafe { self.rt.DrawLine(Vector2 { X: x0, Y: y0 }, Vector2 { X: x1, Y: y1 }, b, width, gfx.round_stroke()) };
    }

    /// A polyline or polygon (filled when `fill`), in DIPs.
    pub fn path(&self, gfx: &Gfx, pts: &[(f32, f32)], closed: bool, fill: bool, width: f32, c: impl Into<Color>) {
        if pts.len() < 2 {
            return;
        }
        // SAFETY: building and drawing a path geometry with valid points.
        unsafe {
            let Ok(geo) = gfx.d2d.CreatePathGeometry() else { return };
            let Ok(sink) = geo.Open() else { return };
            sink.BeginFigure(Vector2 { X: pts[0].0, Y: pts[0].1 }, if fill { D2D1_FIGURE_BEGIN_FILLED } else { D2D1_FIGURE_BEGIN_HOLLOW });
            let rest: Vec<Vector2> = pts[1..].iter().map(|p| Vector2 { X: p.0, Y: p.1 }).collect();
            sink.AddLines(&rest);
            sink.EndFigure(if closed { D2D1_FIGURE_END_CLOSED } else { D2D1_FIGURE_END_OPEN });
            let _ = sink.Close();
            let b = self.brush(c.into());
            if fill {
                self.rt.FillGeometry(&geo, b, None);
            } else {
                self.rt.DrawGeometry(&geo, b, width, gfx.round_stroke());
            }
        }
    }

    /// An arc from `a0` to `a1` degrees (0 = right, clockwise), for the refresh icon.
    pub fn arc(&self, gfx: &Gfx, cx: f32, cy: f32, r: f32, a0: f32, a1: f32, width: f32, c: impl Into<Color>) {
        let n = 24;
        let pts: Vec<(f32, f32)> = (0..=n)
            .map(|i| {
                let a = (a0 + (a1 - a0) * i as f32 / n as f32).to_radians();
                (cx + r * a.cos(), cy + r * a.sin())
            })
            .collect();
        self.path(gfx, &pts, false, false, width, c);
    }

    /// Draw text at (x, y) (top-left of its layout box).
    pub fn text(&self, gfx: &mut Gfx, x: f32, y: f32, text: &str, size: f32, weight: Weight, width: f32, lines: u8, align: Align, c: impl Into<Color>) -> (f32, f32) {
        let c = c.into();
        let Some(l) = gfx.layout(text, size, weight, width, lines, align) else { return (0.0, 0.0) };
        let b = self.brush(c);
        // SAFETY: valid target, layout and brush.
        unsafe { self.rt.DrawTextLayout(Vector2 { X: x, Y: y }, &l.layout, b, D2D1_DRAW_TEXT_OPTIONS_NONE) };
        (l.width, l.height)
    }

    pub fn push_clip(&self, r: Rect) {
        // SAFETY: valid target; balanced by pop_clip().
        unsafe { self.rt.PushAxisAlignedClip(&r.d2d(), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE) };
    }

    pub fn pop_clip(&self) {
        // SAFETY: after push_clip().
        unsafe { self.rt.PopAxisAlignedClip() };
    }

    /// Draw what follows at `alpha` opacity until `pop_opacity` (a fading panel). Returns false
    /// (and pushes nothing) when it's fully opaque.
    pub fn push_opacity(&mut self, alpha: f32) -> bool {
        if alpha >= 0.999 {
            return false;
        }
        if self.layers.len() <= self.depth {
            // SAFETY: valid render target.
            match unsafe { self.rt.CreateLayer(None) } {
                Ok(l) => self.layers.push(l),
                Err(_) => return false,
            }
        }
        let p = D2D1_LAYER_PARAMETERS {
            contentBounds: D2D_RECT_F { left: -1e6, top: -1e6, right: 1e6, bottom: 1e6 },
            geometricMask: std::mem::ManuallyDrop::new(None),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: Matrix3x2::identity(),
            opacity: alpha.max(0.0),
            opacityBrush: std::mem::ManuallyDrop::new(None),
            layerOptions: D2D1_LAYER_OPTIONS_NONE,
        };
        // SAFETY: valid target and layer; balanced by pop_opacity().
        unsafe { self.rt.PushLayer(&p, &self.layers[self.depth]) };
        self.depth += 1;
        true
    }

    pub fn pop_opacity(&mut self, pushed: bool) {
        if pushed {
            self.depth -= 1;
            // SAFETY: after a matching push_opacity().
            unsafe { self.rt.PopLayer() };
        }
    }

    /// A soft drop shadow under a floating panel.
    pub fn shadow(&self, r: Rect, radius: f32, strength: f32) {
        for (i, a) in [(1.0, 0.10), (3.0, 0.07), (6.0, 0.045), (10.0, 0.025)] {
            self.fill_rrect(Rect::new(r.x - i * 0.5, r.y + i * 0.6, r.w + i, r.h + i), radius + i * 0.5, crate::theme::BLACK.alpha(a * strength));
        }
    }

    pub fn translate(&self, dx: f32, dy: f32) {
        // SAFETY: valid target.
        unsafe { self.rt.SetTransform(&Matrix3x2::translation(dx, dy)) };
    }

    // ---------- pictures ----------

    /// A picture from premultiplied BGRA pixels, cached under `key`.
    pub fn bitmap(&mut self, key: u64, w: u32, h: u32, bgra: &[u8]) -> Option<&Bitmap> {
        if !self.bitmaps.contains_key(&key) {
            let props = D2D1_BITMAP_PROPERTIES {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
            };
            // SAFETY: bgra holds w*h*4 bytes (checked by the caller), pitch w*4.
            let bmp = unsafe { self.rt.CreateBitmap(D2D_SIZE_U { width: w, height: h }, Some(bgra.as_ptr() as *const _), w * 4, &props) }.ok()?;
            self.bitmaps.insert(key, Bitmap { bmp, w, h });
        }
        self.bitmaps.get(&key)
    }

    pub fn has_bitmap(&self, key: u64) -> bool {
        self.bitmaps.contains_key(&key)
    }

    pub fn forget_bitmap(&mut self, key: u64) {
        self.bitmaps.remove(&key);
    }

    pub fn bitmap_count(&self) -> usize {
        self.bitmaps.len()
    }

    /// Draw a cached picture into `r` with rounded corners.
    pub fn draw_bitmap_rounded(&self, key: u64, r: Rect, radius: f32, opacity: f32) {
        let Some(b) = self.bitmaps.get(&key) else { return };
        // SAFETY: valid target, bitmap and brush; the transform maps the bitmap onto r.
        unsafe {
            let props = D2D1_BITMAP_BRUSH_PROPERTIES {
                extendModeX: D2D1_EXTEND_MODE_CLAMP,
                extendModeY: D2D1_EXTEND_MODE_CLAMP,
                interpolationMode: D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
            };
            let Ok(brush) = self.rt.CreateBitmapBrush(&b.bmp, Some(&props), None) else { return };
            // the bitmap is 96 dpi: its size in DIPs is its pixel size
            let sx = r.w / b.w as f32;
            let sy = r.h / b.h as f32;
            brush.SetTransform(&Matrix3x2 { M11: sx, M12: 0.0, M21: 0.0, M22: sy, M31: r.x, M32: r.y });
            brush.SetOpacity(opacity);
            let rr = D2D1_ROUNDED_RECT { rect: r.d2d(), radiusX: radius, radiusY: radius };
            self.rt.FillRoundedRectangle(&rr, &brush);
        }
    }
}

/// Is this HRESULT-ish error a lost device?
pub fn is_device_lost(e: &windows::core::Error) -> bool {
    e.code() == D2DERR_RECREATE_TARGET
}
