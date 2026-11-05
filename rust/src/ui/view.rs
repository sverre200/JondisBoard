//! Drawing and layout. The whole window is drawn in one pass (one Direct2D frame), so nothing
//! can tear or paint in piece by piece; while drawing, every clickable thing registers its rect
//! (`hits`), which is what mouse input is tested against. Colours come from the current theme on
//! every frame, so a theme switch is one repaint.

use super::anim::{self, ease_out, key, MOTION_FLASH, MOTION_HOVER, MOTION_OPEN, MOTION_SLIDE, MOTION_UNHOVER, SLIDE};
use super::app::*;
use super::gfx::{Align, Canvas, Color, Gfx, Rect, Weight};
use super::icons::{self, Icon};
use crate::config::{ALL, NO_GROUP};
use crate::theme::{self, Rgb, Theme, DANGER, DANGER_HOVER, GROUP_COLORS, WHITE};
use std::time::Instant;

pub const TILE_W: f32 = 250.0;
pub const TILE_H: f32 = 80.0;
pub const ART: f32 = 52.0;
pub const GAP: f32 = 6.0;
pub const MENU_W: f32 = 220.0;
pub const SETTINGS_W: f32 = 660.0;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Hit {
    QuickImport,
    ImportLink,
    AddFiles,
    More,
    Theme,
    Settings,
    Search,
    Tab(String),
    TabChevron(String),
    NewGroup,
    Tile(u32),
    TileMore(u32),
    Job(u64),
    JobRetry(u64),
    JobDismiss(u64),
    EmptyQuick,
    EmptyImport,
    Scrollbar,
    Board,
    Slider(u8),
    SendVoice,
    StopAll,
    StopHotkey,
    Panel,
    Select(DeviceSlot),
    RestartAudio,
    GetCable,
    Opt(bool),
    Menu(usize, usize),
    MenuBg(usize),
    Swatch(usize, usize),
    Scrim,
    DialogCard,
    DialogOk,
    DialogCancel,
    Field(usize),
    DialogToggle,
    DialogGroup,
    Toast,
}

fn hkey(h: &Hit) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut s = std::collections::hash_map::DefaultHasher::new();
    h.hash(&mut s);
    s.finish()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

/// Geometry from the last frame, for input handling.
#[derive(Default, Clone)]
pub struct Layout {
    pub w: f32,
    pub h: f32,
    pub board: Rect,
    pub content_h: f32,
    pub cols: usize,
    pub colw: f32,
    pub rowh: f32,
    pub area_x: f32,
    /// tile rects in content coordinates (not scrolled), in visible order
    pub tiles: Vec<(u32, Rect)>,
    pub jobs: usize,
    pub tabs: Vec<(String, Rect)>,
    pub buttons: Vec<(Hit, Rect)>,
    pub footer_y: f32,
    pub search: Rect,
    pub scroll_to: Option<u32>,
    pub view: String,
    pub scroll_max: f32,
    pub thumb: Rect,
    pub track: Rect,
    pub sliders: Vec<(u8, Rect)>,
    pub panel: Rect,
    pub select_rects: Vec<(DeviceSlot, Rect)>,
    pub field_rects: Vec<Rect>,
    pub dialog_group: Rect,
    pub board_scroll_speed: f32,
}

impl Layout {
    pub fn button(&self, h: &Hit) -> Option<Rect> {
        self.buttons.iter().find(|(b, _)| b == h).map(|(_, r)| *r)
    }
}

pub fn fmt_duration(secs: f64) -> String {
    if secs < 10.0 {
        format!("{secs:.1}s")
    } else if secs < 60.0 {
        format!("{secs:.0}s")
    } else {
        let s = secs.round() as u64;
        format!("{}:{:02}", s / 60, s % 60)
    }
}

impl App {
    fn hit(&mut self, r: Rect, h: Hit) {
        self.hits.push((r, h));
    }

    pub fn is_hover(&self, h: &Hit) -> bool {
        self.hover.as_ref() == Some(h)
    }

    fn is_down(&self, h: &Hit) -> bool {
        self.down.as_ref() == Some(h) && self.is_hover(h)
    }

    pub fn button_width(&self, g: &mut Gfx, label: &str, icon: bool, kind: Kind) -> f32 {
        let wt = if kind == Kind::Primary { Weight::Bold } else { Weight::Normal };
        let tw = if label.is_empty() { 0.0 } else { g.measure(label, 13.0, wt) };
        tw + if icon && !label.is_empty() { 22.0 } else if icon { 14.0 } else { 0.0 } + if label.is_empty() { 22.0 } else { 32.0 }
    }

    /// The app's one button: sized to its label (unless `w` > 0), optional icon, pressed look.
    #[allow(clippy::too_many_arguments)]
    fn button(&mut self, c: &mut Canvas, g: &mut Gfx, x: f32, y: f32, w: f32, h: f32, label: &str, icon: Option<Icon>, kind: Kind, hit: Hit, toggled: bool) -> Rect {
        let t = self.theme();
        let w = if w > 0.0 { w } else { self.button_width(g, label, icon.is_some(), kind) };
        let r = Rect::new(x, y, w, h);
        let hover = self.is_hover(&hit);
        let down = self.is_down(&hit);
        let (base, hov, text, border) = match kind {
            Kind::Primary => (t.accent, t.accent_hover, t.accent_text, None),
            Kind::Secondary if toggled => (t.accent_soft, t.accent_soft_hover, t.text, Some(t.accent_edge)),
            Kind::Secondary => (t.card, t.card_hover, t.text, Some(t.border)),
            Kind::Ghost => (t.bg, t.card, t.muted, None),
            Kind::Danger => (DANGER, DANGER_HOVER, WHITE, None),
        };
        let pressed = match kind {
            Kind::Primary | Kind::Danger => hov.mix(theme::BLACK, 0.16),
            _ => hov.mix(t.text, 0.07),
        };
        let target = if down { pressed } else if hover { hov } else { base };
        let fill = self.anim.color(key("btn", hkey(&hit)), target, if hover { MOTION_HOVER } else { MOTION_UNHOVER });
        let ghost_idle = kind == Kind::Ghost && fill == t.bg;
        if let Some(b) = border {
            let bc = self.anim.color(key("btnb", hkey(&hit)), b, MOTION_OPEN);
            c.card(r, 10.0, fill, bc);
        } else if !ghost_idle {
            c.fill_rrect(r, 10.0, fill);
        }
        let wt = if kind == Kind::Primary { Weight::Bold } else { Weight::Normal };
        let tw = if label.is_empty() { 0.0 } else { g.measure(label, 13.0, wt) };
        let content = tw + if icon.is_some() { if label.is_empty() { 14.0 } else { 22.0 } } else { 0.0 };
        let mut cx = r.x + (r.w - content) / 2.0;
        if let Some(ic) = icon {
            icons::draw(c, g, ic, cx, r.cy() - 7.0, 14.0, text);
            cx += if label.is_empty() { 14.0 } else { 22.0 };
        }
        if !label.is_empty() {
            c.text(g, cx, r.cy() - 9.0, label, 13.0, wt, 0.0, 1, Align::Left, text);
        }
        self.hit(r, hit.clone());
        self.layout.buttons.push((hit, r));
        r
    }

    fn toggle(&mut self, c: &mut Canvas, x: f32, y: f32, on: bool, id: u64, hover: bool) {
        let t = self.theme();
        let k = self.anim.value(key("toggle", id), if on { 1.0 } else { 0.0 }, MOTION_OPEN);
        let off = if hover { t.border.mix(t.text, 0.1) } else { t.border };
        let onc = if hover { t.accent.mix(WHITE, 0.12) } else { t.accent };
        c.fill_rrect(Rect::new(x, y, 40.0, 22.0), 11.0, off.mix(onc, k));
        let knob = t.text.mix(t.bg, 0.15).mix(WHITE, k);
        let d = 16.0;
        c.fill_circle(x + 3.0 + d / 2.0 + (40.0 - d - 6.0) * k, y + 11.0, d / 2.0, knob);
    }

    // ---------------------------------------------------------------- frame

    /// Time-based things, run at the start of every frame (while frames come continuously the
    /// window's timers don't get a turn, so these can't rely on them alone).
    fn frame_tick(&mut self) {
        if self.toast.as_ref().map(|t| t.born.elapsed().as_millis() >= 4200).unwrap_or(false) {
            self.close_toast();
        }
        if !self.playing.is_empty() || self.player.is_playing_any() {
            self.poll_progress();
        }
        if self.drag.is_some() && self.layout.board_scroll_speed != 0.0 {
            self.drag_tick();
        }
    }

    pub fn paint(&mut self, c: &mut Canvas, g: &mut Gfx) {
        let t0 = Instant::now();
        self.frame_tick();
        self.anim.begin_frame();
        if c.gen != self.themes.generation() {
            // a new theme: no colour tweens from the old one (theme changes are instant)
            self.anim = anim::Anim::new();
            self.anim.begin_frame();
            c.gen = self.themes.generation();
        }
        let t = self.theme();
        let (w, h) = c.size();
        self.hits.clear();
        let scroll_to = self.layout.scroll_to.take();
        let board_scroll_speed = self.layout.board_scroll_speed;
        self.layout = Layout { w, h, scroll_to, board_scroll_speed, ..Default::default() };
        c.begin(t.bg);
        self.upload_pictures(c);
        let footer_h = 87.0;
        let footer_y = (h - footer_h).max(200.0);
        self.layout.footer_y = footer_y;
        self.header(c, g, w);
        self.toolbar(c, g, w);
        let tabs_bottom = self.group_tabs(c, g, w);
        self.board(c, g, Rect::new(16.0, tabs_bottom, w - 26.0, footer_y - tabs_bottom));
        self.footer(c, g, w, footer_y);
        self.settings_panel(c, g);
        self.draw_menus(c, g);
        self.drag_ghost(c, g);
        self.toasts(c, g);
        self.dialogs(c, g);
        if !c.end() {
            // device lost: the window makes a new render target on the next paint
            c.gen = u64::MAX;
        }
        // keep frames coming while anything moves (else: no redraw at all)
        let caret_blink = self.search_focused || self.dialog.as_ref().map(|d| !d.fields.is_empty()).unwrap_or(false);
        if caret_blink {
            super::window::set_timer(self.hwnd, super::window::TIMER_CARET, 530);
        }
        self.dirty = self.anim.active();
        self.slow_anim = self.anim.slow();
        if self.record_frames {
            self.frame_times.push(t0.elapsed().as_secs_f32() * 1000.0);
        }
        let _ = t;
    }

    fn upload_pictures(&mut self, c: &mut Canvas) {
        if self.pic_ready.is_empty() {
            return;
        }
        let ready = std::mem::take(&mut self.pic_ready);
        for (k, (px, pixels)) in ready {
            if pixels.len() == (px * px * 4) as usize {
                c.bitmap(k, px, px, &pixels);
            }
        }
    }

    // ---------------------------------------------------------------- header

    fn header(&mut self, c: &mut Canvas, g: &mut Gfx, w: f32) {
        let t = self.theme();
        // logo (the same geometry as the icon)
        let (lx, ly, ls) = (24.0, 23.0, 36.0);
        let (tile, bars) = super::logo::shapes(ls, true);
        if let Some([x, y, tw, th, r]) = tile {
            c.fill_rrect(Rect::new(lx + x, ly + y, tw, th), r, theme::logo_tile(&t));
        }
        for [x, y, bw, bh, r] in bars {
            c.fill_rrect(Rect::new(lx + x, ly + y, bw, bh), r, t.accent);
        }
        c.text(g, 72.0, 17.0, "JondisBoard", 22.0, Weight::Bold, 0.0, 1, Align::Left, t.text);
        let n = self.clips.len();
        c.text(g, 72.0, 46.0, &format!("{n} sound{}", if n == 1 { "" } else { "s" }), 12.0, Weight::Normal, 0.0, 1, Align::Left, t.muted);
        // right side, right to left: Settings, Theme, status pill
        let sw = self.button_width(g, "Settings", true, Kind::Secondary);
        let sx = w - 24.0 - sw;
        let open = self.settings_open;
        self.button(c, g, sx, 23.0, sw, 36.0, "Settings", Some(Icon::Sliders), Kind::Secondary, Hit::Settings, open);
        let tw = g.measure("Theme", 13.0, Weight::Normal) + 22.0 + 32.0;
        let tx = sx - 8.0 - tw;
        let tr = self.button(c, g, tx, 23.0, tw, 36.0, "", None, Kind::Secondary, Hit::Theme, false);
        let label_w = g.measure("Theme", 13.0, Weight::Normal);
        let cx = tr.x + (tr.w - label_w - 22.0) / 2.0;
        c.fill_circle(cx + 6.0, tr.cy(), 6.0, t.accent);
        c.text(g, cx + 22.0, tr.cy() - 9.0, "Theme", 13.0, Weight::Normal, 0.0, 1, Align::Left, t.text);
        // status pill
        let (text, level) = self.status.clone();
        let color = if level == Level::Info { t.muted } else { level.color(&t) };
        let pw = g.measure(&text, 12.0, Weight::Bold) + 8.0 + 8.0 + 30.0;
        let pr = Rect::new(tx - 10.0 - pw, 26.0, pw, 30.0);
        c.fill_rrect(pr, 15.0, t.bg.mix(color, 0.12));
        c.fill_circle(pr.x + 17.0, pr.cy(), 4.0, color);
        c.text(g, pr.x + 29.0, pr.cy() - 8.0, &text, 12.0, Weight::Bold, 0.0, 1, Align::Left, color);
    }

    fn toolbar(&mut self, c: &mut Canvas, g: &mut Gfx, w: f32) {
        let t = self.theme();
        let y = 78.0;
        let mut x = 24.0;
        let r = self.button(c, g, x, y, 0.0, 36.0, "Quick import", Some(Icon::Bolt), Kind::Primary, Hit::QuickImport, false);
        x = r.right() + 8.0;
        let r = self.button(c, g, x, y, 0.0, 36.0, "Import link…", Some(Icon::Link), Kind::Secondary, Hit::ImportLink, false);
        x = r.right() + 8.0;
        let r = self.button(c, g, x, y, 0.0, 36.0, "Add files", Some(Icon::Note), Kind::Secondary, Hit::AddFiles, false);
        x = r.right() + 8.0;
        self.button(c, g, x, y, 36.0, 36.0, "", Some(Icon::More), Kind::Ghost, Hit::More, false);
        // search
        let sr = Rect::new(w - 24.0 - 240.0, y + 1.0, 240.0, 34.0);
        let focused = self.search_focused;
        let ring = self.anim.color(key("search", 0), if focused { t.accent_edge } else { t.border }, if focused { MOTION_HOVER } else { MOTION_UNHOVER });
        c.card(sr, 10.0, t.card, ring);
        icons::draw(c, g, Icon::Search, sr.x + 11.0, sr.cy() - 7.0, 14.0, t.muted);
        let tr = Rect::new(sr.x + 34.0, sr.y + 2.0, sr.w - 44.0, sr.h - 4.0);
        let caret = self.caret_visible();
        let sel = t.accent.mix(t.card, 0.55);
        self.search.draw(c, g, tr, focused, caret, t.text, t.muted, sel);
        self.layout.search = sr;
        self.hit(sr, Hit::Search);
    }

    pub fn caret_visible(&self) -> bool {
        (self.caret_since.elapsed().as_millis() / 530) % 2 == 0
    }

    fn tab_width(&self, g: &mut Gfx, key: &str, label: &str, n: usize) -> f32 {
        let custom = key != ALL && key != NO_GROUP;
        let name = g.measure(label, 13.0, Weight::Normal);
        let count = g.measure(&n.to_string(), 12.0, Weight::Normal);
        (if key == ALL { 15.0 } else { 13.0 + 8.0 + 8.0 }) + name + 7.0 + count + if custom { 6.0 + 18.0 + 7.0 } else { 15.0 }
    }

    /// The group tabs (they wrap onto more rows when there are many). Returns the bottom.
    fn group_tabs(&mut self, c: &mut Canvas, g: &mut Gfx, w: f32) -> f32 {
        let t = self.theme();
        let tabs = self.tabs();
        let current = self.view_key();
        let drop_tab = self.drag.as_ref().and_then(|d| d.tab.clone());
        let (mut x, mut y) = (24.0, 124.0);
        let max_x = w - 24.0;
        for (k, label, n) in tabs {
            let tw = self.tab_width(g, &k, &label, n);
            if x + tw > max_x && x > 24.0 {
                x = 24.0;
                y += 40.0;
            }
            let r = Rect::new(x, y + 2.0, tw, 32.0);
            let custom = k != ALL && k != NO_GROUP;
            let color = self.group_color(&k).unwrap_or(if k == ALL { t.accent } else { t.muted });
            let hit = Hit::Tab(k.clone());
            let hover = self.is_hover(&hit) || self.is_hover(&Hit::TabChevron(k.clone()));
            let on = current == k;
            let dropping = drop_tab.as_deref() == Some(k.as_str());
            let (fg, border, count) = if dropping {
                (t.card.mix(color, 0.42), color, t.text)
            } else if on {
                (t.card.mix(color, 0.24), t.border.mix(color, 0.75), t.muted.mix(t.text, 0.55))
            } else if hover {
                (t.card, t.border.mix(t.text, 0.12), t.muted)
            } else {
                (t.bg, t.border, t.muted)
            };
            let ms = if on || dropping { MOTION_OPEN } else if hover { MOTION_HOVER } else { MOTION_UNHOVER };
            let fg = self.anim.color(key("tabf", hkey(&hit)), fg, ms);
            let border = self.anim.color(key("tabb", hkey(&hit)), border, ms);
            c.card(r, 16.0, fg, border);
            let mut cx = r.x;
            if k != ALL {
                c.fill_circle(cx + 13.0 + 4.0, r.cy(), 4.0, color);
                cx += 13.0 + 8.0 + 8.0;
            } else {
                cx += 15.0;
            }
            let (nw, _) = c.text(g, cx, r.cy() - 9.0, &label, 13.0, Weight::Normal, 0.0, 1, Align::Left, t.text);
            cx += nw + 7.0;
            c.text(g, cx, r.cy() - 8.5, &n.to_string(), 12.0, Weight::Normal, 0.0, 1, Align::Left, count);
            self.hit(r, hit);
            if custom {
                let cr = Rect::new(r.right() - 7.0 - 18.0, r.cy() - 11.0, 18.0, 22.0);
                let ch = Hit::TabChevron(k.clone());
                if self.is_hover(&ch) {
                    c.fill_rrect(cr, 6.0, fg.mix(t.text, 0.1));
                }
                icons::draw(c, g, Icon::ChevronDown, cr.x + 4.0, cr.y + 6.0, 10.0, t.muted);
                self.hit(cr, ch);
            }
            self.layout.tabs.push((k, r));
            x += tw + 8.0;
        }
        // + New group
        let nw = g.measure("New group", 13.0, Weight::Normal) + 22.0 + 32.0;
        if x + nw > max_x && x > 24.0 {
            x = 24.0;
            y += 40.0;
        }
        let r = self.button(c, g, x, y + 2.0, nw, 32.0, "", None, Kind::Ghost, Hit::NewGroup, false);
        let lw = g.measure("New group", 13.0, Weight::Normal);
        let cx = r.x + (r.w - lw - 22.0) / 2.0;
        icons::draw(c, g, Icon::Plus, cx + 2.0, r.cy() - 5.0, 10.0, t.muted);
        c.text(g, cx + 22.0, r.cy() - 9.0, "New group", 13.0, Weight::Normal, 0.0, 1, Align::Left, t.muted);
        y + 36.0 + 6.0
    }

    // ---------------------------------------------------------------- board

    /// The board's items in order: imports still running (on All / No group), then the clips
    /// shown; while dragging, the dragged clip sits in the slot under the pointer.
    pub fn board_items(&self) -> (usize, Vec<u32>) {
        let view = self.view_key();
        let jobs = if (view == ALL || view == NO_GROUP) && self.query.is_empty() { self.jobs.len() } else { 0 };
        let mut ids = self.visible();
        if let Some(d) = &self.drag {
            if let Some(i) = ids.iter().position(|x| *x == d.id) {
                ids.remove(i);
                let at = if d.tab.is_some() { d.origin } else { d.slot.unwrap_or(d.origin) };
                ids.insert(at.min(ids.len()), d.id);
            }
        }
        (jobs, ids)
    }

    pub fn grid(&self, board: Rect) -> (usize, f32, f32, f32) {
        let cols = (((board.w - 24.0) / TILE_W).floor() as usize).max(1);
        let area_w = board.w - 14.0;
        (cols, area_w / cols as f32, TILE_H + 2.0 * GAP, board.x)
    }

    pub fn cell(&self, i: usize, cols: usize, colw: f32, rowh: f32, x0: f32) -> Rect {
        let (r, col) = (i / cols, i % cols);
        let xa = (col as f32 * colw).round();
        let xb = ((col + 1) as f32 * colw).round();
        Rect::new(x0 + xa + GAP, r as f32 * rowh + GAP, xb - xa - 2.0 * GAP, TILE_H)
    }

    fn board(&mut self, c: &mut Canvas, g: &mut Gfx, board: Rect) {
        let t = self.theme();
        let view = self.view_key();
        self.layout.board = board;
        self.layout.view = view.clone();
        self.hit(board, Hit::Board);
        let (jobs, ids) = self.board_items();
        let (cols, colw, rowh, x0) = self.grid(board);
        self.layout.cols = cols;
        self.layout.colw = colw;
        self.layout.rowh = rowh;
        self.layout.area_x = x0;
        self.layout.jobs = jobs;
        let total = jobs + ids.len();
        let content_h = if total == 0 { 0.0 } else { ((total + cols - 1) / cols) as f32 * rowh + GAP };
        self.layout.content_h = content_h;
        let max = (content_h - board.h).max(0.0);
        self.layout.scroll_max = max;
        // scroll a clip into view if asked (a new import)
        if let Some(id) = self.layout.scroll_to {
            if let Some(i) = ids.iter().position(|x| *x == id) {
                let r = self.cell(jobs + i, cols, colw, rowh, x0);
                let cur = self.scroll_target.get(&view).copied().unwrap_or(0.0);
                if r.y < cur || r.bottom() > cur + board.h {
                    self.scroll_target.insert(view.clone(), (r.y - 12.0).clamp(0.0, max));
                }
            }
        }
        let target = self.scroll_target.get(&view).copied().unwrap_or(0.0).clamp(0.0, max);
        self.scroll_target.insert(view.clone(), target);
        let scroll = self.anim.value(key("scroll", hash_str(&view)), target, 140.0).clamp(0.0, max);
        self.scroll.insert(view.clone(), scroll);
        c.push_clip(board);
        let sliding = Instant::now() < self.slide_until || self.drag.is_some();
        // tile rects (content coords), sliding to their cells
        let mut rects = Vec::with_capacity(ids.len());
        for (i, id) in ids.iter().enumerate() {
            let cell = self.cell(jobs + i, cols, colw, rowh, x0);
            let ms = if sliding { MOTION_SLIDE } else { 0.0 };
            let x = self.anim.value(key("tx", *id as u64), cell.x, ms);
            let y = self.anim.value(key("ty", *id as u64), cell.y, ms);
            let wv = self.anim.value(key("tw", *id as u64), cell.w, ms);
            rects.push((*id, cell, Rect::new(x, y, wv, TILE_H)));
        }
        self.layout.tiles = rects.iter().map(|(id, cell, _)| (*id, *cell)).collect();
        // pending imports first
        for i in 0..jobs {
            let r = self.cell(i, cols, colw, rowh, x0).offset(0.0, board.y - scroll);
            if r.bottom() >= board.y && r.y <= board.bottom() {
                self.pending_tile(c, g, r, i);
            }
        }
        // tiles that change rows glide over the others
        let mut order: Vec<usize> = (0..rects.len()).collect();
        order.sort_by_key(|&i| (rects[i].1.y != rects[i].2.y) as u8);
        let dragging = self.drag.as_ref().map(|d| d.id);
        for i in order {
            let (id, _, r) = rects[i];
            let r = r.offset(0.0, board.y - scroll);
            if r.bottom() < board.y || r.y > board.bottom() {
                continue;
            }
            if Some(id) == dragging {
                // the spot it would land in: a faint copy
                c.card(r, 14.0, t.card.mix(t.bg, 0.6), t.border.mix(t.accent, 0.25));
                continue;
            }
            self.tile(c, g, id, r, &view);
        }
        if total == 0 {
            self.empty_state(c, g, board, &view);
        }
        c.pop_clip();
        // scrollbar
        if max > 0.0 {
            let track = Rect::new(board.right() - 10.0, board.y + 4.0, 8.0, board.h - 8.0);
            let th = (board.h / content_h * track.h).max(32.0);
            let ty = track.y + (track.h - th) * (scroll / max);
            let thumb = Rect::new(track.x, ty, 8.0, th);
            let hover = self.is_hover(&Hit::Scrollbar) || self.scrollbar_drag.is_some();
            let col = self.anim.color(key("sb", 0), if hover { t.card_hover } else { t.card }, MOTION_HOVER);
            c.fill_rrect(thumb, 4.0, col);
            self.layout.track = track;
            self.layout.thumb = thumb;
            self.hit(Rect::new(track.x - 4.0, track.y, 14.0, track.h), Hit::Scrollbar);
        }
    }

    fn empty_state(&mut self, c: &mut Canvas, g: &mut Gfx, board: Rect, view: &str) {
        let t = self.theme();
        if view == ALL && self.clips.is_empty() && self.query.is_empty() {
            let cx = board.cx();
            let y = board.y + 64.0;
            self.placeholder_art(c, g, Rect::new(cx - ART / 2.0, y, ART, ART), t.accent, None, true);
            c.text(g, board.x, y + ART + 14.0, "No sounds yet", 18.0, Weight::Bold, board.w, 1, Align::Center, t.text);
            c.text(g, board.x, y + ART + 44.0, "Copy an Instagram or YouTube link, then press Quick import (or Ctrl+V).", 13.0, Weight::Normal, board.w, 1, Align::Center, t.muted);
            let w1 = self.button_width(g, "Quick import", true, Kind::Primary);
            let w2 = self.button_width(g, "Import link…", true, Kind::Secondary);
            let x = cx - (w1 + 8.0 + w2) / 2.0;
            let by = y + ART + 80.0;
            self.button(c, g, x, by, w1, 36.0, "Quick import", Some(Icon::Bolt), Kind::Primary, Hit::EmptyQuick, false);
            self.button(c, g, x + w1 + 8.0, by, w2, 36.0, "Import link…", Some(Icon::Link), Kind::Secondary, Hit::EmptyImport, false);
        } else {
            let msg = if !self.query.is_empty() { "No sounds match your search." } else { "This group is empty. Right-click a sound > Move to group, or import straight into it." };
            c.text(g, board.x, board.y + 60.0, msg, 13.0, Weight::Normal, board.w, 2, Align::Center, t.muted);
        }
    }

    /// The square on the left of a tile without a picture: its group's colour with a note.
    fn placeholder_art(&mut self, c: &mut Canvas, g: &mut Gfx, r: Rect, tint: Rgb, frame: Option<usize>, note: bool) {
        let t = self.theme();
        c.fill_rrect(r, r.w * 0.19, t.card.mix(tint, 0.28));
        if frame.is_none() && note {
            let s = r.w * 0.42;
            icons::draw(c, g, Icon::Note, r.cx() - s / 2.0, r.cy() - s / 2.0, s, tint.mix(WHITE, 0.3));
        }
    }

    fn equalizer(&self, c: &mut Canvas, r: Rect, frame: usize) {
        c.fill_rrect(r, r.w * 0.19, theme::BLACK.alpha(120.0 / 255.0));
        let n = r.w;
        let (bw, gap) = (n * 0.11, n * 0.07);
        let x0 = r.x + (n - 3.0 * bw - 2.0 * gap) / 2.0;
        let bottom = r.y + n * 0.7;
        let heights = [[0.45, 0.8, 0.6], [0.75, 0.5, 0.9], [0.6, 0.95, 0.4]][frame % 3];
        for (i, hh) in heights.iter().enumerate() {
            let x = x0 + i as f32 * (bw + gap);
            let bh = hh * n * 0.4;
            c.fill_rrect(Rect::new(x, bottom - bh, bw, bh), bw / 2.0, WHITE);
        }
    }

    pub fn picture_key(path: &std::path::Path, stamp: u64, px: u32) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut s = std::collections::hash_map::DefaultHasher::new();
        path.hash(&mut s);
        stamp.hash(&mut s);
        px.hash(&mut s);
        s.finish()
    }

    fn tile_art(&mut self, c: &mut Canvas, g: &mut Gfx, clip: &Clip, r: Rect, playing: bool, dim: bool) {
        let t = self.theme();
        let tint = self.tint(clip);
        let frame = if playing { Some((self.started.elapsed().as_millis() / 150) as usize % 3) } else { None };
        let mut drew = false;
        if let Some((path, stamp)) = &clip.picture {
            let px = (ART * c.scale).ceil() as u32;
            let k = Self::picture_key(path, *stamp, px);
            if c.has_bitmap(k) {
                c.draw_bitmap_rounded(k, r, r.w * 0.19, 1.0);
                drew = true;
            } else if !self.pic_failed.contains(&k) {
                if self.pic_pending.insert(k) {
                    if let Some(tx) = &self.pic_tx {
                        let _ = tx.send(super::images::Request { key: k, path: path.clone(), px });
                    }
                }
                // loading: a plain square (no note, so nothing flips when the picture arrives)
                c.fill_rrect(r, r.w * 0.19, t.card.mix(tint, 0.28));
                drew = true;
            }
        }
        if !drew {
            self.placeholder_art(c, g, r, tint, frame, true);
        }
        if let Some(f) = frame {
            self.equalizer(c, r, f);
        }
        if dim {
            c.fill_rrect(r, r.w * 0.19, t.bg.alpha(0xa0 as f32 / 255.0));
        }
    }

    fn tile(&mut self, c: &mut Canvas, g: &mut Gfx, id: u32, r: Rect, view: &str) {
        let Some(clip) = self.clip(id).cloned() else { return };
        let t = self.theme();
        let hit = Hit::Tile(id);
        let more = Hit::TileMore(id);
        let hover = (self.is_hover(&hit) || self.is_hover(&more)) && self.drag.is_none();
        let playing = self.playing.get(&id).copied();
        let accent = self.tint(&clip);
        let (fg, border) = match playing {
            Some(_) => (t.card.mix(accent, if hover { 0.14 } else { 0.1 }), accent),
            None if hover => (t.card_hover, t.border.mix(t.text, 0.12)),
            None => (t.card, t.border),
        };
        let ms = if hover || playing.is_some() { MOTION_HOVER } else { MOTION_UNHOVER };
        let mut fill = self.anim.color(key("tf", id as u64), fg, ms);
        let mut edge = self.anim.color(key("tb", id as u64), border, ms);
        if let Some((since, strength)) = self.flash.get(&id).copied() {
            let e = since.elapsed().as_secs_f32() * 1000.0 / MOTION_FLASH;
            if e >= 1.0 {
                self.flash.remove(&id);
            } else {
                let k = ease_out(e);
                fill = fg.mix(accent, strength).mix(fill, k);
                edge = accent.mix(edge, k);
                self.anim.keep_alive();
            }
        }
        c.card(r, 14.0, fill, edge);
        let art = Rect::new(r.x + 12.0, r.y + 12.0, ART, ART);
        self.tile_art(c, g, &clip, art, playing.is_some(), false);
        // title: up to 2 lines, "…" if it doesn't fit
        let title_w = r.w - (12.0 + ART + 12.0 + 30.0);
        c.text(g, r.x + 76.0, r.y + 9.0, &clip.stem, 13.0, Weight::Bold, title_w.max(20.0), 2, Align::Left, t.text);
        // info row: duration · group (in All) · hotkey chip
        let base = r.y + 12.0 + ART;
        let meta = match (&clip.failed, clip.duration) {
            (Some(_), _) => "can't load".to_string(),
            (None, Some(d)) => fmt_duration(d),
            (None, None) => "…".to_string(),
        };
        let meta_color = if clip.failed.is_some() { t.muted.mix(DANGER, 0.6) } else { t.muted };
        let (mw, _) = c.text(g, r.x + 76.0, base - 16.0, &meta, 12.0, Weight::Normal, 0.0, 1, Align::Left, meta_color);
        let chip = self.hotkey_of(id).map(|b| b.pretty());
        let chip_w = chip.as_ref().map(|s| g.measure(s, 11.0, Weight::Bold) + 14.0).unwrap_or(0.0);
        if view == ALL && !clip.group.is_empty() {
            let gx = r.x + 76.0 + mw + 10.0;
            let gc = self.group_color(&clip.group).unwrap_or(t.muted);
            c.fill_circle(gx + 3.5, base - 7.0, 3.5, gc);
            let avail = r.right() - 12.0 - chip_w - 8.0 - (gx + 11.0);
            if avail > 16.0 {
                c.text(g, gx + 11.0, base - 16.0, &clip.group, 12.0, Weight::Normal, avail, 1, Align::Left, t.muted);
            }
        }
        if let Some(s) = chip {
            let cr = Rect::new(r.right() - 12.0 - chip_w, base + 2.0 - 20.0, chip_w, 20.0);
            c.fill_rrect(cr, 6.0, t.card.mix(t.text, 0.09));
            c.text(g, cr.x, cr.y + 2.5, &s, 11.0, Weight::Bold, cr.w, 1, Align::Center, t.text);
        }
        // progress bar while playing
        if let Some(p) = playing {
            let track = Rect::new(r.x + 12.0, r.bottom() - 10.0, r.w - 24.0, 3.0);
            c.fill_rect(track, fill.mix(t.text, 0.08));
            c.fill_rect(Rect::new(track.x, track.y, track.w * p, track.h), accent);
            self.anim.keep_slow();
        }
        // ⋯ on hover (its own menu button)
        if hover {
            let mr = Rect::new(r.right() - 8.0 - 26.0, r.y + 8.0, 26.0, 24.0);
            if self.is_hover(&more) {
                c.fill_rrect(mr, 7.0, fill.mix(t.text, 0.1));
            }
            icons::draw(c, g, Icon::More, mr.x + 5.0, mr.y + 4.0, 16.0, t.muted);
            self.hit(r, hit);
            self.hit(mr, more);
        } else {
            self.hit(r, hit);
        }
    }

    fn pending_tile(&mut self, c: &mut Canvas, g: &mut Gfx, r: Rect, i: usize) {
        let t = self.theme();
        let job = &self.jobs[i];
        let (id, failed) = (job.id, job.error.is_some());
        let title = if failed { "Import failed".to_string() } else { "Importing…".to_string() };
        let meta = job.error.clone().unwrap_or_else(|| format!("{}  ·  {}", job.source, job.status));
        let started = job.started;
        c.card(r, 14.0, t.card, if failed { t.border.mix(DANGER, 0.5) } else { t.border });
        let art = Rect::new(r.x + 12.0, r.y + 12.0, ART, ART);
        if failed {
            c.fill_rrect(art, 10.0, t.card.mix(DANGER, 0.2));
            icons::draw(c, g, Icon::X, art.cx() - 8.0, art.cy() - 8.0, 16.0, DANGER);
        } else {
            // the picture square breathes while it works
            let ph = (started.elapsed().as_secs_f32() / 0.76 * std::f32::consts::PI).sin() * 0.5 + 0.5;
            c.fill_rrect(art, 10.0, t.card_hover.mix(t.card_hover.mix(t.accent, 0.16), ph));
            icons::draw(c, g, Icon::Link, art.cx() - 9.0, art.cy() - 9.0, 18.0, t.muted);
            // indeterminate bar
            let track = Rect::new(r.x + 12.0, r.bottom() - 10.0, r.w - 24.0, 3.0);
            c.fill_rrect(track, 1.5, t.card_hover);
            let p = (started.elapsed().as_secs_f32() * 0.6) % 1.4 - 0.2;
            let seg = 0.3;
            let a = (p).clamp(0.0, 1.0);
            let b = (p + seg).clamp(0.0, 1.0);
            if b > a {
                c.fill_rrect(Rect::new(track.x + track.w * a, track.y, track.w * (b - a), 3.0), 1.5, t.accent);
            }
            self.anim.keep_slow();
        }
        c.text(g, r.x + 76.0, r.y + 9.0, &title, 13.0, Weight::Bold, r.w - 76.0 - if failed { 70.0 } else { 12.0 }, 1, Align::Left, t.text);
        c.text(g, r.x + 76.0, r.y + 12.0 + ART - 16.0, &meta, 12.0, Weight::Normal, r.w - 76.0 - 12.0, 1, Align::Left, if failed { t.muted.mix(DANGER, 0.6) } else { t.muted });
        self.hit(r, Hit::Job(id));
        if failed {
            let x = r.right() - 8.0 - 28.0 * 2.0 - 2.0;
            self.button(c, g, x, r.y + 8.0, 28.0, 26.0, "", Some(Icon::Refresh), Kind::Ghost, Hit::JobRetry(id), false);
            self.button(c, g, x + 30.0, r.y + 8.0, 28.0, 26.0, "", Some(Icon::X), Kind::Ghost, Hit::JobDismiss(id), false);
        }
    }

    // ---------------------------------------------------------------- footer

    fn footer(&mut self, c: &mut Canvas, g: &mut Gfx, w: f32, y: f32) {
        let t = self.theme();
        c.fill_rect(Rect::new(0.0, y, w, self.layout.h - y), t.surface);
        c.fill_rect(Rect::new(0.0, y, w, 1.0), t.border);
        let top = y + 1.0 + 14.0;
        // right: Stop all with its hotkey under it
        let sx = w - 24.0 - 124.0;
        let r = self.button(c, g, sx, top, 124.0, 36.0, "", None, Kind::Danger, Hit::StopAll, false);
        let lw = g.measure("Stop all", 13.0, Weight::Normal);
        let cx = r.x + (r.w - lw - 20.0) / 2.0;
        icons::draw(c, g, Icon::Stop, cx, r.cy() - 6.0, 12.0, WHITE);
        c.text(g, cx + 20.0, r.cy() - 9.0, "Stop all", 13.0, Weight::Normal, 0.0, 1, Align::Left, WHITE);
        let key_text = self.cfg.stop_hotkey.as_ref().map(|b| format!("Hotkey: {}", b.pretty())).unwrap_or_else(|| "Set hotkey".into());
        let kr = Rect::new(sx, top + 40.0, 124.0, 18.0);
        if self.is_hover(&Hit::StopHotkey) {
            c.fill_rrect(kr, 6.0, t.card);
        }
        c.text(g, kr.x, kr.y + 1.0, &key_text, 11.0, Weight::Normal, kr.w, 1, Align::Center, t.muted);
        self.hit(kr, Hit::StopHotkey);
        // Send my voice
        let words = "Send my voice";
        let ww = g.measure(words, 13.0, Weight::Normal);
        let tx = sx - 20.0 - (40.0 + 10.0 + ww);
        let ty = top + 18.0 - 11.0;
        let hover = self.is_hover(&Hit::SendVoice);
        self.toggle(c, tx, ty, self.cfg.mic_passthrough, 1, hover);
        c.text(g, tx + 50.0, ty + 2.0, words, 13.0, Weight::Normal, 0.0, 1, Align::Left, t.text);
        self.hit(Rect::new(tx, ty - 6.0, 50.0 + ww, 34.0), Hit::SendVoice);
        // sliders share the rest
        let left = 24.0;
        let avail = tx - 24.0 - left;
        let sw = ((avail - 2.0 * 24.0) / 3.0).max(60.0);
        let vols = [("Mic", self.cfg.mic_volume, 1.5), ("Clips", self.cfg.clip_volume, 1.5), ("In your ears", self.cfg.monitor_volume, 1.0)];
        for (i, (label, v, top_v)) in vols.iter().enumerate() {
            let x = left + i as f32 * (sw + 24.0);
            c.text(g, x, top, label, 12.0, Weight::Normal, 0.0, 1, Align::Left, t.muted);
            let pct = format!("{}%", (v * 100.0).round());
            c.text(g, x, top, &pct, 12.0, Weight::Bold, sw, 1, Align::Right, t.text);
            let sr = Rect::new(x, top + 22.0, sw, 18.0);
            let frac = (*v / top_v).clamp(0.0, 1.0) as f32;
            let track = Rect::new(sr.x, sr.cy() - 3.0, sr.w, 6.0);
            c.fill_rrect(track, 3.0, t.surface.mix(t.border, 0.9));
            c.fill_rrect(Rect::new(track.x, track.y, (track.w * frac).max(6.0), 6.0), 3.0, t.accent);
            let hov = self.is_hover(&Hit::Slider(i as u8)) || self.slider_drag == Some(i as u8);
            let knob = self.anim.color(key("knob", i as u64), if hov { WHITE } else { t.text }, MOTION_HOVER);
            c.fill_circle(track.x + 9.0 + (track.w - 18.0) * frac, sr.cy(), 9.0, knob);
            self.hit(sr.inset(0.0, -6.0), Hit::Slider(i as u8));
            self.layout.sliders.push((i as u8, sr));
        }
    }

    // ---------------------------------------------------------------- settings

    fn settings_panel(&mut self, c: &mut Canvas, g: &mut Gfx) {
        let open = self.settings_open;
        let e = (self.settings_changed.elapsed().as_secs_f32() * 1000.0 / if open { MOTION_OPEN } else { anim::MOTION_CLOSE }).min(1.0);
        if e < 1.0 {
            self.anim.keep_alive();
        }
        let alpha = if open { ease_out(e) } else { 1.0 - ease_out(e) };
        if alpha <= 0.0 {
            return;
        }
        let t = self.theme();
        let Some(btn) = self.layout.button(&Hit::Settings) else { return };
        let w = SETTINGS_W.min(self.layout.w - 48.0);
        let (dy, x) = (if open { -SLIDE * (1.0 - ease_out(e)) } else { -SLIDE / 2.0 * ease_out(e) }, btn.right() - w);
        let y = btn.bottom() + 8.0 + dy;
        let pad = 20.0;
        let label_w = ["Your microphone", "Send to (virtual cable)", "Hear clips on"].iter().map(|l| g.measure(l, 13.0, Weight::Normal)).fold(0.0, f32::max);
        let restart_w = self.button_width(g, "Restart audio", true, Kind::Secondary);
        let help = !self.devices.cable_installed && !self.audio_starting;
        let mut h = 10.0 + 22.0 + 3.0 * 42.0 + 12.0;
        if help {
            h += 72.0;
        }
        h += 12.0 + 24.0 + 2.0 * 52.0 + 14.0;
        let r = Rect::new(x, y, w, h);
        self.layout.panel = r;
        let pushed = c.push_opacity(alpha);
        c.shadow(r, 10.0, 1.0);
        c.card(r, 10.0, t.elevated, t.edge);
        if open {
            self.hit(r, Hit::Panel);
        }
        let mut cy = y + 14.0;
        c.text(g, x + pad, cy, "Audio", 14.0, Weight::Bold, 0.0, 1, Align::Left, t.text);
        cy += 30.0;
        let sel_x = x + pad + label_w + 14.0;
        let sel_w = w - (sel_x - x) - restart_w - 2.0 * pad;
        let rows = [("Your microphone", DeviceSlot::Mic), ("Send to (virtual cable)", DeviceSlot::Cable), ("Hear clips on", DeviceSlot::Monitor)];
        let rows_top = cy;
        for (label, slot) in rows {
            c.text(g, x + pad, cy + 7.0, label, 13.0, Weight::Normal, 0.0, 1, Align::Left, t.muted);
            let sr = Rect::new(sel_x, cy, sel_w, 34.0);
            let value = self.device_name(slot).unwrap_or_else(|| "Choose...".into());
            let hit = Hit::Select(slot);
            let is_open = self.menus.first().map(|m| m.owner == MenuOwner::Select(slot)).unwrap_or(false);
            let hover = self.is_hover(&hit) && open;
            let fg = self.anim.color(key("self", slot as u64), if hover || is_open { t.card_hover } else { t.card }, if hover || is_open { MOTION_HOVER } else { MOTION_UNHOVER });
            c.card(sr, 10.0, fg, if is_open { t.accent_edge } else { t.border });
            c.text(g, sr.x + 12.0, sr.cy() - 9.0, &value, 13.0, Weight::Normal, sr.w - 40.0, 1, Align::Left, t.text);
            icons::draw(c, g, if is_open { Icon::ChevronUp } else { Icon::ChevronDown }, sr.right() - 24.0, sr.cy() - 6.0, 12.0, if is_open { t.text } else { t.muted });
            if open {
                self.hit(sr, hit);
            }
            self.layout.select_rects.push((slot, sr));
            cy += 42.0;
        }
        let rb_y = rows_top + (3.0 * 42.0 - 8.0) / 2.0 - 18.0;
        let hits_before = self.hits.len();
        self.button(c, g, x + w - pad - restart_w, rb_y, restart_w, 36.0, "Restart audio", Some(Icon::Refresh), Kind::Secondary, Hit::RestartAudio, false);
        if !open {
            self.hits.truncate(hits_before);
        }
        cy += 4.0;
        if help {
            let hr = Rect::new(x + 12.0, cy, w - 24.0, 62.0);
            c.card(hr, 10.0, t.card.mix(DANGER, 0.12), t.border.mix(DANGER, 0.45));
            c.text(
                g,
                hr.x + 14.0,
                hr.y + 10.0,
                "VB-CABLE isn't installed. It's the free virtual mic that carries the sounds into games. Install it, restart your PC, then open the soundboard again.",
                12.0,
                Weight::Normal,
                hr.w - 28.0 - 132.0,
                3,
                Align::Left,
                t.text,
            );
            let hb = self.hits.len();
            self.button(c, g, hr.right() - 12.0 - 120.0, hr.cy() - 18.0, 120.0, 36.0, "Get VB-CABLE", None, Kind::Primary, Hit::GetCable, false);
            if !open {
                self.hits.truncate(hb);
            }
            cy += 72.0;
        }
        c.fill_rect(Rect::new(x + pad, cy, w - 2.0 * pad, 1.0), t.elevated.mix(t.border, 0.8));
        cy += 12.0;
        c.text(g, x + pad, cy, "Playback", 14.0, Weight::Bold, 0.0, 1, Align::Left, t.text);
        cy += 26.0;
        let opts = [
            (true, "One sound at a time", "Starting a sound stops the others.", self.cfg.solo),
            (false, "Press a playing sound to stop it", "A second click or hotkey stops it, instead of starting it over.", self.cfg.toggle_stop),
        ];
        for (solo, title, about, on) in opts {
            c.text(g, x + pad, cy, title, 13.0, Weight::Normal, 0.0, 1, Align::Left, t.text);
            c.text(g, x + pad, cy + 19.0, about, 12.0, Weight::Normal, w - 2.0 * pad - 60.0, 1, Align::Left, t.muted);
            let hit = Hit::Opt(solo);
            let hover = self.is_hover(&hit) && open;
            self.toggle(c, x + w - pad - 40.0, cy + 6.0, on, 10 + solo as u64, hover);
            if open {
                self.hit(Rect::new(x + pad, cy - 4.0, w - 2.0 * pad, 44.0), hit);
            }
            cy += 52.0;
        }
        c.pop_opacity(pushed);
    }

    // ---------------------------------------------------------------- menus

    pub fn menu_height(items: &[MenuItem]) -> f32 {
        10.0 + items
            .iter()
            .map(|i| match i {
                MenuItem::Header(_) => 30.0,
                MenuItem::Item { .. } | MenuItem::Submenu { .. } => 34.0,
                MenuItem::Separator => 11.0,
                MenuItem::Label(_) => 22.0,
                MenuItem::Swatches { .. } => 36.0,
            })
            .sum::<f32>()
    }

    fn draw_menus(&mut self, c: &mut Canvas, g: &mut Gfx) {
        // closing ones fade out first (underneath)
        let now = Instant::now();
        self.closing_menus.retain(|m| now.duration_since(m.since).as_secs_f32() * 1000.0 < anim::MOTION_CLOSE);
        for i in 0..self.closing_menus.len() {
            let e = ease_out(now.duration_since(self.closing_menus[i].since).as_secs_f32() * 1000.0 / anim::MOTION_CLOSE);
            let m = std::mem::replace(&mut self.closing_menus[i].menu, empty_menu());
            self.menu(c, g, &m, usize::MAX, 1.0 - e, (0.0, -SLIDE / 2.0 * e));
            self.closing_menus[i].menu = m;
            self.anim.keep_alive();
        }
        for level in 0..self.menus.len() {
            let m = std::mem::replace(&mut self.menus[level], empty_menu());
            let e = ease_out(now.duration_since(m.born).as_secs_f32() * 1000.0 / MOTION_OPEN);
            if e < 1.0 {
                self.anim.keep_alive();
            }
            let rows = self.menu(c, g, &m, level, e, (m.dx * (1.0 - e), m.dy * (1.0 - e)));
            self.menus[level] = m;
            self.menus[level].rows = rows;
        }
    }

    /// Draw one menu; returns its row rects.
    fn menu(&mut self, c: &mut Canvas, g: &mut Gfx, m: &Menu, level: usize, alpha: f32, off: (f32, f32)) -> Vec<Rect> {
        let t = self.theme();
        let h = Self::menu_height(&m.items);
        let r = Rect::new(m.x + off.0, m.y + off.1, m.w, h);
        let pushed = c.push_opacity(alpha);
        c.shadow(r, 8.0, 1.0);
        c.card(r, 8.0, t.elevated, t.edge);
        let live = level != usize::MAX;
        if live {
            self.hit(r, Hit::MenuBg(level));
        }
        let mut rows = Vec::new();
        let mut y = r.y + 5.0;
        let child_row = level.checked_add(1).and_then(|l| self.menus.get(l)).and_then(|c| c.parent_row);
        for (i, item) in m.items.iter().enumerate() {
            match item {
                MenuItem::Header(text) => {
                    c.text(g, r.x + 17.0, y + 7.0, text, 12.0, Weight::Bold, r.w - 34.0, 1, Align::Left, t.muted);
                    rows.push(Rect::new(r.x, y, r.w, 30.0));
                    y += 30.0;
                }
                MenuItem::Separator => {
                    c.fill_rect(Rect::new(r.x + 15.0, y + 5.0, r.w - 30.0, 1.0), t.elevated.mix(t.border, 0.8));
                    rows.push(Rect::new(r.x, y, r.w, 11.0));
                    y += 11.0;
                }
                MenuItem::Label(text) => {
                    c.text(g, r.x + 17.0, y + 2.0, text, 12.0, Weight::Normal, 0.0, 1, Align::Left, t.muted);
                    rows.push(Rect::new(r.x, y, r.w, 22.0));
                    y += 22.0;
                }
                MenuItem::Swatches { current, .. } => {
                    let mut x = r.x + 15.0;
                    for (k, col) in GROUP_COLORS.iter().enumerate() {
                        let sr = Rect::new(x, y + 4.0, 22.0, 22.0);
                        let hit = Hit::Swatch(level, k);
                        let hov = live && self.is_hover(&hit);
                        c.fill_circle(sr.cx(), sr.cy(), 11.0, if hov { col.mix(WHITE, 0.2) } else { *col });
                        if Some(*col) == *current {
                            c.stroke_circle(sr.cx(), sr.cy(), 9.5, 3.0, t.text);
                        }
                        if live {
                            self.hit(sr, hit);
                        }
                        x += 22.0 + 4.0;
                    }
                    rows.push(Rect::new(r.x, y, r.w, 36.0));
                    y += 36.0;
                }
                MenuItem::Item { text, danger, dot, enabled, checked, .. } => {
                    let row = Rect::new(r.x + 5.0, y + 1.0, r.w - 10.0, 32.0);
                    let hit = Hit::Menu(level, i);
                    let lit = live && *enabled && !*checked && (self.is_hover(&hit) || child_row == Some(i));
                    let col = self.anim.color(key("menurow", (level as u64) << 32 | i as u64 | (hash_str(text) << 40)), if lit { t.card_hover } else { t.elevated }, if lit { MOTION_HOVER } else { MOTION_UNHOVER });
                    if col != t.elevated {
                        c.fill_rrect(row, 7.0, col);
                    }
                    let mut tx = row.x + 12.0;
                    if let Some(d) = dot {
                        c.fill_circle(tx + 5.0, row.cy(), 5.0, *d);
                        tx += 10.0 + 10.0;
                    }
                    let color = if !*enabled { t.muted } else if *danger { DANGER } else { t.text };
                    c.text(g, tx, row.cy() - 9.0, text, 13.0, Weight::Normal, row.right() - tx - if *checked { 30.0 } else { 12.0 }, 1, Align::Left, color);
                    if *checked {
                        icons::draw(c, g, Icon::Check, row.right() - 10.0 - 14.0, row.cy() - 7.0, 14.0, if *enabled { t.accent } else { t.muted });
                    }
                    if live {
                        self.hit(row, hit);
                    }
                    rows.push(row);
                    y += 34.0;
                }
                MenuItem::Submenu { text, .. } => {
                    let row = Rect::new(r.x + 5.0, y + 1.0, r.w - 10.0, 32.0);
                    let hit = Hit::Menu(level, i);
                    let lit = live && (self.is_hover(&hit) || child_row == Some(i));
                    let col = self.anim.color(key("menurow", (level as u64) << 32 | i as u64 | (hash_str(text) << 40)), if lit { t.card_hover } else { t.elevated }, if lit { MOTION_HOVER } else { MOTION_UNHOVER });
                    if col != t.elevated {
                        c.fill_rrect(row, 7.0, col);
                    }
                    c.text(g, row.x + 12.0, row.cy() - 9.0, text, 13.0, Weight::Normal, row.w - 40.0, 1, Align::Left, t.text);
                    icons::draw(c, g, Icon::ChevronRight, row.right() - 10.0 - 12.0, row.cy() - 6.0, 12.0, t.muted);
                    if live {
                        self.hit(row, hit);
                    }
                    rows.push(row);
                    y += 34.0;
                }
            }
        }
        c.pop_opacity(pushed);
        rows
    }

    // ---------------------------------------------------------------- drag

    fn ghost_chip(&mut self, c: &mut Canvas, g: &mut Gfx, id: u32, x: f32, y: f32, alpha: f32) {
        let Some(clip) = self.clip(id).cloned() else { return };
        let t = self.theme();
        let tw = g.measure(&clip.stem, 13.0, Weight::Bold).min(200.0);
        let r = Rect::new(x, y, 8.0 + 34.0 + 10.0 + tw + 14.0, 50.0);
        let pushed = c.push_opacity(alpha);
        c.shadow(r, 8.0, 1.0);
        c.card(r, 8.0, t.elevated, t.edge);
        let art = Rect::new(r.x + 8.0, r.y + 8.0, 34.0, 34.0);
        self.tile_art(c, g, &clip, art, false, false);
        c.text(g, art.right() + 10.0, r.cy() - 9.0, &clip.stem, 13.0, Weight::Bold, 200.0, 1, Align::Left, t.text);
        c.pop_opacity(pushed);
    }

    fn drag_ghost(&mut self, c: &mut Canvas, g: &mut Gfx) {
        if let Some(d) = &self.drag {
            let (id, pos, born) = (d.id, d.pos, d.born);
            let a = ease_out(born.elapsed().as_secs_f32() * 1000.0 / MOTION_HOVER);
            if a < 1.0 {
                self.anim.keep_alive();
            }
            self.ghost_chip(c, g, id, pos.0 + 16.0, pos.1 + 16.0, a);
        }
        if let Some(l) = &self.landing {
            let e = ease_out(l.since.elapsed().as_secs_f32() * 1000.0 / MOTION_OPEN);
            let (id, from, to) = (l.id, l.from, l.to);
            if e >= 1.0 {
                self.landing = None;
            } else {
                self.anim.keep_alive();
                let x = from.0 + (to.0 - from.0) * e;
                let y = from.1 + (to.1 - from.1) * e;
                self.ghost_chip(c, g, id, x, y, 1.0 - e);
            }
        }
    }

    // ---------------------------------------------------------------- toasts

    fn toasts(&mut self, c: &mut Canvas, g: &mut Gfx) {
        let now = Instant::now();
        self.old_toasts.retain(|t| t.closing.map(|s| now.duration_since(s).as_secs_f32() * 1000.0 < MOTION_UNHOVER).unwrap_or(false));
        let mut list: Vec<(String, Level, f32, f32, bool)> = Vec::new();
        for t in &self.old_toasts {
            let e = ease_out(now.duration_since(t.closing.unwrap()).as_secs_f32() * 1000.0 / MOTION_UNHOVER);
            list.push((t.text.clone(), t.level, 1.0 - e, SLIDE / 2.0 * e, false));
        }
        if let Some(t) = &self.toast {
            let e = ease_out(now.duration_since(t.born).as_secs_f32() * 1000.0 / MOTION_OPEN);
            list.push((t.text.clone(), t.level, e, (SLIDE + 3.0) * (1.0 - e), true));
        }
        let th = self.theme();
        for (text, level, alpha, dy, live) in list {
            if alpha < 1.0 {
                self.anim.keep_alive();
            }
            let tw = g.layout(&text, 13.0, Weight::Normal, 560.0, 4, Align::Left).map(|l| (l.width, l.height)).unwrap_or((100.0, 18.0));
            let r = Rect::new((self.layout.w - (tw.0 + 50.0)) / 2.0, self.layout.footer_y - 14.0 - (tw.1 + 20.0) + dy, tw.0 + 50.0, tw.1 + 20.0);
            let pushed = c.push_opacity(alpha);
            c.shadow(r, 8.0, 1.0);
            c.card(r, 8.0, th.elevated, th.edge);
            c.fill_circle(r.x + 20.0, r.y + 10.0 + 9.0, 4.0, level.color(&th));
            c.text(g, r.x + 34.0, r.y + 10.0, &text, 13.0, Weight::Normal, 560.0, 4, Align::Left, th.text);
            c.pop_opacity(pushed);
            if live {
                self.hit(r, Hit::Toast);
            }
        }
    }

    // ---------------------------------------------------------------- dialogs

    fn field(&mut self, c: &mut Canvas, g: &mut Gfx, d: &mut Dialog, i: usize, r: Rect) {
        let t = self.theme();
        let focused = d.focus == i && d.closing.is_none();
        let border = if focused { t.accent } else { t.border };
        c.card(r, 10.0, t.card, border);
        let caret = self.caret_visible();
        d.fields[i].draw(c, g, Rect::new(r.x + 12.0, r.y + 1.0, r.w - 24.0, r.h - 2.0), focused, caret, t.text, t.muted, t.accent.mix(t.card, 0.55));
        self.layout.field_rects.push(r);
        if d.closing.is_none() {
            self.hit(r, Hit::Field(i));
        }
    }

    fn dialogs(&mut self, c: &mut Canvas, g: &mut Gfx) {
        if let Some(mut d) = self.closing_dialog.take() {
            let e = d.closing.map(|s| s.elapsed().as_secs_f32() * 1000.0 / anim::MOTION_CLOSE).unwrap_or(1.0);
            if e < 1.0 {
                self.anim.keep_alive();
                let hits = self.hits.len();
                self.dialog_card(c, g, &mut d, 1.0 - ease_out(e), 6.0 * ease_out(e));
                self.hits.truncate(hits);
                self.closing_dialog = Some(d);
            }
        }
        if let Some(mut d) = self.dialog.take() {
            let e = ease_out(d.born.elapsed().as_secs_f32() * 1000.0 / (MOTION_OPEN + 20.0));
            if e < 1.0 {
                self.anim.keep_alive();
            }
            self.dialog_card(c, g, &mut d, e, 10.0 * (1.0 - e));
            self.dialog = Some(d);
        }
    }

    fn dialog_card(&mut self, c: &mut Canvas, g: &mut Gfx, d: &mut Dialog, alpha: f32, rise: f32) {
        let t = self.theme();
        let (w, h) = (self.layout.w, self.layout.h);
        let live = d.closing.is_none();
        // scrim: the rest of the window goes quiet behind a dialog
        c.fill_rect(Rect::new(0.0, 0.0, w, h), t.bg.alpha(0.55 * alpha));
        if live {
            self.hit(Rect::new(0.0, 0.0, w, h), Hit::Scrim);
        }
        let dh = self.dialog_height(g, d);
        let dw = d.width.min(w - 32.0);
        let x = (w - dw) / 2.0;
        let y = ((h - dh) / 3.0).max(16.0) + rise;
        let r = Rect::new(x, y, dw, dh);
        d.rect = r;
        let pushed = c.push_opacity(alpha);
        c.shadow(r, 12.0, 1.4);
        c.card(r, 12.0, t.bg, t.edge);
        if live {
            self.hit(r, Hit::DialogCard);
        }
        let bh = |hits: &mut Vec<(Rect, Hit)>, n: usize| {
            if !live {
                hits.truncate(n);
            }
        };
        match &d.kind {
            DialogKind::Confirm { title, text, ok, .. } => {
                let (title, text, ok) = (title.clone(), text.clone(), ok.clone());
                c.text(g, x + 22.0, y + 20.0, &title, 16.0, Weight::Bold, dw - 44.0, 1, Align::Left, t.text);
                c.text(g, x + 22.0, y + 48.0, &text, 13.0, Weight::Normal, dw - 44.0, 4, Align::Left, t.muted);
                let n = self.hits.len();
                self.button(c, g, x + dw - 22.0 - 100.0, y + dh - 18.0 - 36.0, 100.0, 36.0, &ok, None, Kind::Danger, Hit::DialogOk, false);
                self.button(c, g, x + dw - 22.0 - 100.0 - 8.0 - 80.0, y + dh - 18.0 - 36.0, 80.0, 36.0, "Cancel", None, Kind::Ghost, Hit::DialogCancel, false);
                bh(&mut self.hits, n);
            }
            DialogKind::Text { title, ok, .. } => {
                let (title, ok) = (title.clone(), ok.clone());
                c.text(g, x + 22.0, y + 20.0, &title, 16.0, Weight::Bold, dw - 44.0, 1, Align::Left, t.text);
                self.field(c, g, d, 0, Rect::new(x + 22.0, y + 56.0, dw - 44.0, 36.0));
                let n = self.hits.len();
                self.button(c, g, x + dw - 22.0 - 90.0, y + dh - 18.0 - 36.0, 90.0, 36.0, &ok, None, Kind::Primary, Hit::DialogOk, false);
                self.button(c, g, x + dw - 22.0 - 90.0 - 8.0 - 80.0, y + dh - 18.0 - 36.0, 80.0, 36.0, "Cancel", None, Kind::Ghost, Hit::DialogCancel, false);
                bh(&mut self.hits, n);
            }
            DialogKind::Hotkey { what, .. } => {
                let what = what.clone();
                c.text(g, x + 16.0, y + 24.0, &what, 13.0, Weight::Normal, dw - 32.0, 1, Align::Center, t.muted);
                c.text(g, x, y + 48.0, "Press a key", 22.0, Weight::Bold, dw, 1, Align::Center, t.text);
                c.text(g, x, y + 84.0, "Ctrl / Shift / Alt combos work  ·  Esc to cancel", 12.0, Weight::Normal, dw, 1, Align::Center, t.muted);
                let n = self.hits.len();
                self.button(c, g, x + (dw - 80.0) / 2.0, y + dh - 18.0 - 32.0, 80.0, 32.0, "Cancel", None, Kind::Ghost, Hit::DialogCancel, false);
                bh(&mut self.hits, n);
            }
            DialogKind::Import { normalize, group, busy, started } => {
                let (normalize, group, busy, started) = (*normalize, group.clone(), *busy, *started);
                c.text(g, x + 24.0, y + 22.0, "Import a sound", 18.0, Weight::Bold, 0.0, 1, Align::Left, t.text);
                c.text(g, x + 24.0, y + 50.0, "From an Instagram reel or a YouTube video. Only the link is needed.", 12.0, Weight::Normal, dw - 48.0, 1, Align::Left, t.muted);
                let mut cy = y + 84.0;
                let cap = |c: &mut Canvas, g: &mut Gfx, x: f32, y: f32, s: &str| {
                    c.text(g, x, y, s, 11.0, Weight::Bold, 0.0, 1, Align::Left, t.muted);
                };
                cap(c, g, x + 26.0, cy, "Link");
                self.field(c, g, d, 0, Rect::new(x + 24.0, cy + 20.0, dw - 48.0, 38.0));
                cy += 20.0 + 38.0 + 14.0;
                cap(c, g, x + 26.0, cy, "Name");
                self.field(c, g, d, 1, Rect::new(x + 24.0, cy + 20.0, dw - 48.0, 38.0));
                cy += 20.0 + 38.0 + 14.0;
                let inner = dw - 48.0;
                let unit = (inner - 24.0) / 4.0;
                cap(c, g, x + 26.0, cy, "Start");
                cap(c, g, x + 24.0 + unit + 12.0 + 2.0, cy, "End");
                cap(c, g, x + 24.0 + 2.0 * unit + 24.0 + 2.0, cy, "Group");
                self.field(c, g, d, 2, Rect::new(x + 24.0, cy + 20.0, unit, 38.0));
                self.field(c, g, d, 3, Rect::new(x + 24.0 + unit + 12.0, cy + 20.0, unit, 38.0));
                let gr = Rect::new(x + 24.0 + 2.0 * unit + 24.0, cy + 20.0, 2.0 * unit, 38.0);
                let hov = live && self.is_hover(&Hit::DialogGroup);
                let is_open = self.menus.first().map(|m| m.owner == MenuOwner::ImportGroup).unwrap_or(false);
                c.card(gr, 10.0, if hov || is_open { t.card_hover } else { t.card }, if is_open { t.accent_edge } else { t.border });
                c.text(g, gr.x + 12.0, gr.cy() - 9.0, if group.is_empty() { "No group" } else { &group }, 13.0, Weight::Normal, gr.w - 40.0, 1, Align::Left, t.text);
                icons::draw(c, g, if is_open { Icon::ChevronUp } else { Icon::ChevronDown }, gr.right() - 24.0, gr.cy() - 6.0, 12.0, t.muted);
                self.layout.dialog_group = gr;
                if live {
                    self.hit(gr, Hit::DialogGroup);
                }
                cy += 20.0 + 38.0 + 16.0;
                let th = live && self.is_hover(&Hit::DialogToggle);
                self.toggle(c, x + 24.0, cy, normalize, 77, th);
                c.text(g, x + 24.0 + 50.0, cy + 2.0, "Even out loudness", 13.0, Weight::Normal, 0.0, 1, Align::Left, t.text);
                if live {
                    let ww = g.measure("Even out loudness", 13.0, Weight::Normal);
                    self.hit(Rect::new(x + 24.0, cy - 4.0, 50.0 + ww, 30.0), Hit::DialogToggle);
                }
                cy += 22.0;
                if let Some((msg, col)) = &d.message {
                    let (msg, col) = (msg.clone(), *col);
                    cy += 12.0;
                    let (_, mh) = c.text(g, x + 24.0, cy, &msg, 12.0, Weight::Normal, dw - 48.0, 8, Align::Left, col);
                    cy += mh;
                }
                if busy {
                    cy += 12.0;
                    let track = Rect::new(x + 24.0, cy, dw - 48.0, 3.0);
                    c.fill_rrect(track, 1.5, t.card);
                    let p = started.map(|s| s.elapsed().as_secs_f32() * 0.6 % 1.4 - 0.2).unwrap_or(0.0);
                    let (a, b) = (p.clamp(0.0, 1.0), (p + 0.3).clamp(0.0, 1.0));
                    if b > a {
                        c.fill_rrect(Rect::new(track.x + track.w * a, cy, track.w * (b - a), 3.0), 1.5, t.accent);
                    }
                    self.anim.keep_slow();
                    cy += 3.0;
                }
                cy += 18.0;
                c.fill_rect(Rect::new(x, cy, dw, 1.0), t.border);
                let n = self.hits.len();
                self.button(c, g, x + dw - 24.0 - 120.0, cy + 16.0, 120.0, 38.0, "Import", None, Kind::Primary, Hit::DialogOk, false);
                self.button(c, g, x + dw - 24.0 - 120.0 - 8.0 - 88.0, cy + 16.0, 88.0, 38.0, "Cancel", None, Kind::Ghost, Hit::DialogCancel, false);
                bh(&mut self.hits, n);
            }
        }
        c.pop_opacity(pushed);
    }

    fn dialog_height(&self, g: &mut Gfx, d: &Dialog) -> f32 {
        match &d.kind {
            DialogKind::Confirm { .. } | DialogKind::Text { .. } => 170.0,
            DialogKind::Hotkey { .. } => 190.0,
            DialogKind::Import { busy, .. } => {
                let mut h = 84.0 + 3.0 * (20.0 + 38.0 + 14.0) + 2.0 + 22.0;
                if let Some((m, _)) = &d.message {
                    h += 12.0 + g.layout(m, 12.0, Weight::Normal, d.width - 48.0, 8, Align::Left).map(|l| l.height).unwrap_or(18.0);
                }
                if *busy {
                    h += 15.0;
                }
                h + 18.0 + 1.0 + 16.0 + 38.0 + 16.0
            }
        }
    }
}

fn empty_menu() -> Menu {
    Menu { items: vec![], x: 0.0, y: 0.0, w: 0.0, born: Instant::now(), owner: MenuOwner::Context, parent_row: None, dx: 0.0, dy: 0.0, rows: vec![], rect: Rect::default(), hover: None }
}

pub fn hash_str(s: &str) -> u64 {
    key(s, 0)
}

#[allow(dead_code)]
fn _color(_: Color, _: &Theme) {}
