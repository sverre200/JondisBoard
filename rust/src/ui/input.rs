//! Mouse and keyboard. Hit-testing uses the rects the last frame registered (`App::hits`),
//! topmost last. Tiles play on mouse RELEASE (a press that turns into a drag never fires a sound
//! into the game), and a click that only closes a menu doesn't play either.

use super::anim::SLIDE;
use super::app::*;
use super::gfx::{Gfx, Rect};
use super::shell;
use super::textfield::KeyResult;
use super::view::{Hit, MENU_W};
use crate::config::{ALL, NO_GROUP};
use crate::library;
use crate::theme::{self, GROUP_COLORS};
use std::time::Instant;

const DRAG_THRESHOLD: f32 = 6.0;

impl App {
    /// The topmost hit under (x, y). With a dialog open only the dialog counts.
    pub fn hit_at(&self, x: f32, y: f32) -> Option<Hit> {
        let mut found = None;
        for (r, h) in self.hits.iter().rev() {
            if r.contains(x, y) {
                found = Some(h.clone());
                break;
            }
        }
        found
    }

    fn set_hover(&mut self, h: Option<Hit>) {
        if h != self.hover {
            self.hover = h;
            self.invalidate();
        }
    }

    // ---------------------------------------------------------------- mouse

    pub fn mouse_move(&mut self, g: &mut Gfx, x: f32, y: f32) {
        self.mouse = (x, y);
        self.mouse_in = true;
        if let Some(i) = self.slider_drag {
            self.slider_to(i, x);
            return;
        }
        if let Some((grab_y, start_scroll)) = self.scrollbar_drag {
            let l = &self.layout;
            let span = (l.track.h - l.thumb.h).max(1.0);
            let s = (start_scroll + (y - grab_y) / span * l.scroll_max).clamp(0.0, l.scroll_max);
            let v = l.view.clone();
            self.scroll_target.insert(v.clone(), s);
            self.anim.set(super::anim::key("scroll", super::view::hash_str(&v)), s);
            self.invalidate();
            return;
        }
        if self.search.dragging {
            let sr = self.layout.search;
            self.search.drag_to(g, x - (sr.x + 34.0));
            self.invalidate();
            return;
        }
        if let Some(d) = &mut self.dialog {
            if let Some(f) = d.fields.get_mut(d.focus) {
                if f.dragging {
                    if let Some(r) = self.layout.field_rects.get(d.focus).copied() {
                        f.drag_to(g, x - (r.x + 12.0));
                        self.invalidate();
                    }
                    return;
                }
            }
        }
        // a press on a tile that moves far enough becomes a drag
        if let Some(Hit::Tile(id)) = self.down.clone() {
            if self.drag.is_none() {
                let (x0, y0) = self.down_pos;
                if (x - x0).abs().max((y - y0).abs()) >= DRAG_THRESHOLD {
                    self.begin_drag(id, x, y);
                }
            }
        }
        if self.drag.is_some() {
            self.drag_move(x, y);
            return;
        }
        let h = self.hit_at(x, y);
        // the pointer takes over from the arrow keys
        if h != self.hover {
            if let Some(Hit::Menu(level, _)) = &h {
                if let Some(m) = self.menus.get_mut(*level) {
                    m.hover = None;
                }
            }
        }
        // submenus open on hover; hovering another row of the parent closes them
        if let Some(Hit::Menu(level, i)) = &h {
            let (level, i) = (*level, *i);
            let is_sub = matches!(self.menus.get(level).and_then(|m| m.items.get(i)), Some(MenuItem::Submenu { .. }));
            let open_child = self.menus.get(level + 1).and_then(|m| m.parent_row);
            if is_sub && open_child != Some(i) {
                self.open_submenu(level, i);
            } else if !is_sub && self.menus.len() > level + 1 {
                self.truncate_menus(level + 1);
            }
        }
        self.set_hover(h);
    }

    pub fn mouse_leave(&mut self) {
        self.mouse_in = false;
        if self.drag.is_none() && self.slider_drag.is_none() {
            self.set_hover(None);
        }
    }

    fn slider_to(&mut self, i: u8, x: f32) {
        let Some((_, r)) = self.layout.sliders.iter().find(|(k, _)| *k == i).copied() else { return };
        let top = if i == 2 { 1.0 } else { 1.5 };
        let frac = ((x - r.x - 9.0) / (r.w - 18.0)).clamp(0.0, 1.0) as f64;
        self.set_volume(i, frac * top);
    }

    pub fn mouse_down(&mut self, g: &mut Gfx, x: f32, y: f32, shift: bool, double: bool) {
        self.mouse = (x, y);
        let h = self.hit_at(x, y);
        self.down_pos = (x, y);
        self.down_menus_open = !self.menus.is_empty();
        // modal dialog: only its own parts react
        if self.dialog.is_some() {
            match &h {
                Some(Hit::Field(i)) => {
                    let i = *i;
                    if let Some(r) = self.layout.field_rects.get(i).copied() {
                        let d = self.dialog.as_mut().unwrap();
                        d.focus = i;
                        if double {
                            d.fields[i].select_word(g, x - (r.x + 12.0));
                        } else {
                            d.fields[i].click(g, x - (r.x + 12.0), shift);
                        }
                        self.caret_since = Instant::now();
                    }
                }
                Some(Hit::Menu(..)) | Some(Hit::MenuBg(_)) | Some(Hit::Swatch(..)) => {}
                // the Group control closes its own open list (instead of re-opening it)
                Some(Hit::DialogGroup) if self.menus.first().map(|m| m.owner == MenuOwner::ImportGroup).unwrap_or(false) => {
                    self.close_menus();
                    self.down = None;
                    return;
                }
                _ => {
                    if !self.menus.is_empty() {
                        self.close_menus();
                    }
                }
            }
            self.down = h;
            self.invalidate();
            return;
        }
        let in_menu = matches!(h, Some(Hit::Menu(..)) | Some(Hit::MenuBg(_)) | Some(Hit::Swatch(..)));
        if !self.menus.is_empty() && !in_menu {
            let owner = self.menus[0].owner.clone();
            self.close_menus();
            // clicking the button that opened it just closes it
            let same = matches!((&owner, &h), (MenuOwner::Theme, Some(Hit::Theme)) | (MenuOwner::More, Some(Hit::More)))
                || matches!((&owner, &h), (MenuOwner::Select(a), Some(Hit::Select(b))) if a == b)
                || matches!((&owner, &h), (MenuOwner::Tab(a), Some(Hit::TabChevron(b))) if a == b)
                || matches!((&owner, &h), (MenuOwner::Tab(a), Some(Hit::Tab(b))) if a == b)
                || matches!((&owner, &h), (MenuOwner::ImportGroup, Some(Hit::DialogGroup)));
            if same {
                self.down = None;
                return;
            }
        }
        // clicking outside Settings closes it (its own button toggles it)
        let in_panel = matches!(h, Some(Hit::Panel) | Some(Hit::Select(_)) | Some(Hit::RestartAudio) | Some(Hit::GetCable) | Some(Hit::Opt(_)) | Some(Hit::Settings));
        if self.settings_open && !in_panel && !in_menu {
            self.open_settings(false);
        }
        // the search field takes the keyboard when clicked, and gives it up otherwise
        if h == Some(Hit::Search) {
            let sr = self.layout.search;
            if double {
                self.search.select_word(g, x - (sr.x + 34.0));
            } else {
                self.search.click(g, x - (sr.x + 34.0), shift);
            }
            self.search_focused = true;
            self.caret_since = Instant::now();
            self.update_typing();
        } else if self.search_focused && !in_menu {
            self.search_focused = false;
            self.update_typing();
        }
        match &h {
            Some(Hit::Slider(i)) => {
                self.slider_drag = Some(*i);
                self.slider_to(*i, x);
            }
            Some(Hit::Scrollbar) => {
                let l = &self.layout;
                if !l.thumb.contains(x, y) {
                    // jump so the thumb centres on the pointer
                    let span = (l.track.h - l.thumb.h).max(1.0);
                    let s = ((y - l.track.y - l.thumb.h / 2.0) / span * l.scroll_max).clamp(0.0, l.scroll_max);
                    let v = l.view.clone();
                    self.scroll_target.insert(v, s);
                }
                let v = self.layout.view.clone();
                self.scrollbar_drag = Some((y, self.scroll_target.get(&v).copied().unwrap_or(0.0)));
            }
            _ => {}
        }
        self.down = h;
        self.capture_req = Some(true);
        self.invalidate();
    }

    pub fn mouse_up(&mut self, x: f32, y: f32) {
        self.capture_req = Some(false);
        self.search.dragging = false;
        if let Some(d) = &mut self.dialog {
            for f in &mut d.fields {
                f.dragging = false;
            }
        }
        let down = self.down.take();
        if self.slider_drag.take().is_some() {
            self.save_config();
            return;
        }
        if self.scrollbar_drag.take().is_some() {
            return;
        }
        if self.drag.is_some() {
            self.drop_drag(x, y);
            return;
        }
        let up = self.hit_at(x, y);
        let Some(h) = down else { return };
        if Some(&h) != up.as_ref() {
            return;
        }
        if let Hit::Tile(id) = h {
            // plays on release; a click that closed a menu doesn't play
            if !self.down_menus_open {
                self.press(id);
            }
            return;
        }
        self.click(h);
    }

    pub fn right_click(&mut self, x: f32, y: f32) {
        if self.dialog.is_some() || self.drag.is_some() {
            return;
        }
        let h = self.hit_at(x, y);
        match h {
            Some(Hit::Tile(id)) | Some(Hit::TileMore(id)) => self.clip_menu(id, x, y),
            Some(Hit::Tab(k)) | Some(Hit::TabChevron(k)) if k != ALL && k != NO_GROUP => self.group_menu(&k, Some((x, y))),
            _ => {
                if !self.menus.is_empty() {
                    self.close_menus();
                }
            }
        }
    }

    pub fn wheel(&mut self, delta: f32) {
        if self.dialog.is_some() {
            return;
        }
        if !self.menus.is_empty() {
            self.close_menus();
        }
        let v = self.layout.view.clone();
        let cur = self.scroll_target.get(&v).copied().unwrap_or(0.0);
        let s = (cur - delta / 120.0 * 92.0 * 1.5).clamp(0.0, self.layout.scroll_max);
        self.scroll_target.insert(v, s);
        self.invalidate();
        if let Some((x, y)) = self.drag.as_ref().map(|d| d.pos) {
            self.drag_move(x, y);
        }
    }

    // ---------------------------------------------------------------- clicks

    fn click(&mut self, h: Hit) {
        match h {
            Hit::QuickImport | Hit::EmptyQuick => self.quick_import(),
            Hit::ImportLink | Hit::EmptyImport => self.import_dialog(),
            Hit::AddFiles => self.add_files(),
            Hit::More => self.more_menu(),
            Hit::Theme => self.theme_menu(),
            Hit::Settings => {
                let o = !self.settings_open;
                self.open_settings(o);
            }
            Hit::Tab(k) => {
                if k == self.view_key() && k != ALL && k != NO_GROUP {
                    self.group_menu(&k, None);
                } else {
                    self.show_group(&k);
                }
            }
            Hit::TabChevron(k) => self.group_menu(&k, None),
            Hit::NewGroup => self.ask_text("New group", "", "Create", "Group name, e.g. Memes", TextAction::NewGroup(None)),
            Hit::TileMore(id) => {
                let (x, y) = self.mouse;
                self.clip_menu(id, x, y)
            }
            Hit::Job(id) => {
                if let Some(e) = self.jobs.iter().find(|j| j.id == id).and_then(|j| j.full_error.clone()) {
                    self.toast(&format!("Couldn't import: {e}"), Level::Bad);
                }
            }
            Hit::JobRetry(id) => self.retry_job(id),
            Hit::JobDismiss(id) => self.dismiss_job(id),
            Hit::SendVoice => {
                let on = !self.cfg.mic_passthrough;
                self.set_passthrough(on);
            }
            Hit::StopAll => self.stop_all(),
            Hit::StopHotkey => self.record_hotkey(HotkeyTarget::StopAll),
            Hit::Select(slot) => self.device_menu(slot),
            Hit::RestartAudio => self.start_audio(),
            Hit::GetCable => shell::open("https://vb-audio.com/Cable/"),
            Hit::Opt(solo) => {
                let v = if solo { !self.cfg.solo } else { !self.cfg.toggle_stop };
                self.set_option(solo, v);
            }
            Hit::Menu(level, i) => self.menu_click(level, i),
            Hit::Swatch(level, k) => {
                let group = self.menus.get(level).and_then(|m| {
                    m.items.iter().find_map(|it| match it {
                        MenuItem::Swatches { group, .. } => Some(group.clone()),
                        _ => None,
                    })
                });
                self.close_menus();
                if let Some(g) = group {
                    self.set_group_color(&g, GROUP_COLORS[k]);
                }
            }
            Hit::DialogOk => self.dialog_ok(),
            Hit::DialogCancel => self.close_dialog(),
            Hit::DialogToggle => {
                if let Some(Dialog { kind: DialogKind::Import { normalize, .. }, .. }) = &mut self.dialog {
                    *normalize = !*normalize;
                }
                self.invalidate();
            }
            Hit::DialogGroup => self.import_group_menu(),
            Hit::Toast => self.close_toast(),
            _ => {}
        }
    }

    fn menu_click(&mut self, level: usize, i: usize) {
        let Some(item) = self.menus.get(level).and_then(|m| m.items.get(i)).cloned() else { return };
        match item {
            MenuItem::Item { action, enabled, checked, .. } => {
                if !enabled || checked {
                    return; // the current choice reads normally but does nothing
                }
                self.close_menus();
                self.run(action);
            }
            MenuItem::Submenu { .. } => self.open_submenu(level, i),
            _ => {}
        }
    }

    // ---------------------------------------------------------------- menus

    fn open_menu(&mut self, items: Vec<MenuItem>, x: f32, y: f32, w: f32, owner: MenuOwner, align_right: bool) {
        self.close_menus();
        if owner != MenuOwner::Select(DeviceSlot::Mic) && owner != MenuOwner::Select(DeviceSlot::Cable) && owner != MenuOwner::Select(DeviceSlot::Monitor) && owner != MenuOwner::ImportGroup {
            self.open_settings(false);
        }
        let h = App::menu_height(&items);
        let mut x = if align_right { x - w } else { x };
        let (ww, wh) = (self.layout.w, self.layout.h);
        if x + w > ww - 4.0 {
            x = ww - w - 4.0;
        }
        x = x.max(4.0);
        let y = y.min(wh - h - 4.0).max(4.0);
        self.menus.push(Menu { items, x, y, w, born: Instant::now(), owner, parent_row: None, dx: 0.0, dy: -SLIDE, rows: vec![], rect: Rect::new(x, y, w, h), hover: None });
        self.invalidate();
    }

    fn truncate_menus(&mut self, n: usize) {
        let now = Instant::now();
        while self.menus.len() > n {
            let m = self.menus.pop().unwrap();
            self.closing_menus.push(ClosingMenu { menu: m, since: now, alpha: 1.0 });
        }
        self.invalidate();
    }

    fn open_submenu(&mut self, level: usize, i: usize) {
        let Some(parent) = self.menus.get(level) else { return };
        let Some(MenuItem::Submenu { items, .. }) = parent.items.get(i).cloned() else { return };
        let row = parent.rows.get(i).copied().unwrap_or(Rect::new(parent.x, parent.y, parent.w, 32.0));
        let (px, pw) = (parent.x, parent.w);
        self.truncate_menus(level + 1);
        let w = MENU_W;
        let h = App::menu_height(&items);
        let (mut x, mut dx) = (px + pw + 4.0, -SLIDE);
        if x + w > self.layout.w - 4.0 {
            x = px - 4.0 - w;
            dx = SLIDE;
        }
        let y = (row.y - 5.0).min(self.layout.h - h - 4.0).max(4.0);
        self.menus.push(Menu { items, x, y, w, born: Instant::now(), owner: MenuOwner::Context, parent_row: Some(i), dx, dy: 0.0, rows: vec![], rect: Rect::new(x, y, w, h), hover: None });
        self.invalidate();
    }

    pub fn clip_menu(&mut self, id: u32, x: f32, y: f32) {
        let Some(c) = self.clip(id).cloned() else { return };
        let mut items = vec![MenuItem::Header(c.stem.clone())];
        items.push(MenuItem::Item { text: "Play".into(), action: Action::Play(id), danger: false, dot: None, enabled: true, checked: false, instant: true });
        match self.hotkey_of(id).map(|b| b.pretty()) {
            Some(k) => {
                items.push(MenuItem::item(format!("Change hotkey  ({k})..."), Action::SetHotkey(id)));
                items.push(MenuItem::item("Clear hotkey", Action::ClearHotkey(id)));
            }
            None => items.push(MenuItem::item("Set hotkey...", Action::SetHotkey(id))),
        }
        items.push(MenuItem::Separator);
        let t = self.theme();
        let mut groups: Vec<MenuItem> = self
            .groups
            .iter()
            .map(|g| MenuItem::Item {
                text: g.clone(),
                action: Action::MoveTo(id, g.clone()),
                danger: false,
                dot: self.group_color(g),
                enabled: true,
                checked: *g == c.group,
                instant: false,
            })
            .collect();
        if !c.group.is_empty() {
            groups.push(MenuItem::Item { text: "No group".into(), action: Action::MoveTo(id, String::new()), danger: false, dot: Some(t.muted), enabled: true, checked: false, instant: false });
        }
        if !self.groups.is_empty() {
            groups.push(MenuItem::Separator);
        }
        groups.push(MenuItem::item("New group...", Action::NewGroupWith(id)));
        let mut pics = vec![
            MenuItem::item("Choose picture...", Action::ChoosePicture(id)),
            MenuItem::Item { text: "Paste picture".into(), action: Action::PastePicture(id), danger: false, dot: None, enabled: shell::clipboard_has_image(), checked: false, instant: false },
        ];
        if c.picture.is_some() {
            pics.push(MenuItem::Separator);
            pics.push(MenuItem::danger("Remove picture", Action::RemovePicture(id)));
        }
        items.push(MenuItem::Submenu { text: "Move to group".into(), items: groups });
        items.push(MenuItem::Submenu { text: "Picture".into(), items: pics });
        items.push(MenuItem::Separator);
        items.push(MenuItem::item("Rename...", Action::Rename(id)));
        items.push(MenuItem::danger("Delete", Action::Delete(id)));
        self.open_menu(items, x, y, MENU_W, MenuOwner::Context, false);
    }

    /// at: where to open it (right-click); None: under its tab.
    pub fn group_menu(&mut self, group: &str, at: Option<(f32, f32)>) {
        let (x, y) = match at {
            Some(p) => p,
            None => match self.layout.tabs.iter().find(|(k, _)| k == group) {
                Some((_, r)) => (r.x, r.bottom() + 6.0),
                None => self.mouse,
            },
        };
        let items = vec![
            MenuItem::Header(group.to_string()),
            MenuItem::item("Rename group...", Action::RenameGroup(group.to_string())),
            MenuItem::Separator,
            MenuItem::Label("Colour".into()),
            MenuItem::Swatches { group: group.to_string(), current: self.group_color(group) },
            MenuItem::Separator,
            MenuItem::danger("Delete group", Action::DeleteGroup(group.to_string())),
        ];
        self.open_menu(items, x, y, MENU_W, MenuOwner::Tab(group.to_string()), false);
    }

    fn more_menu(&mut self) {
        let b = self.layout.button(&Hit::More).unwrap_or_default();
        let stop = self.cfg.stop_hotkey.as_ref().map(|b| b.pretty());
        let items = vec![
            MenuItem::item("Open sounds folder", Action::OpenFolder),
            MenuItem::item("Reload sounds", Action::Reload),
            MenuItem::item("Sort sounds A–Z", Action::SortAz),
            MenuItem::Separator,
            MenuItem::item(stop.map(|s| format!("Stop all hotkey  ({s})...")).unwrap_or_else(|| "Set Stop all hotkey...".into()), Action::StopHotkey),
        ];
        self.open_menu(items, b.x, b.bottom() + 8.0, MENU_W, MenuOwner::More, false);
    }

    fn theme_menu(&mut self) {
        let b = self.layout.button(&Hit::Theme).unwrap_or_default();
        let current = self.theme().name;
        let mut items = vec![MenuItem::Header("Theme".into())];
        for t in theme::all() {
            items.push(MenuItem::Item {
                text: t.name.into(),
                action: Action::Theme(t.name.into()),
                danger: false,
                dot: Some(t.accent),
                enabled: true,
                checked: t.name == current,
                instant: true,
            });
        }
        self.open_menu(items, b.right(), b.bottom() + 8.0, MENU_W, MenuOwner::Theme, true);
    }

    fn device_menu(&mut self, slot: DeviceSlot) {
        let Some((_, r)) = self.layout.select_rects.iter().find(|(s, _)| *s == slot).copied() else { return };
        let list = if slot == DeviceSlot::Mic { &self.devices.inputs } else { &self.devices.outputs };
        let current = self.device_name(slot);
        let mut items: Vec<MenuItem> = list
            .iter()
            .filter(|n| slot != DeviceSlot::Mic || !n.to_lowercase().contains("cable output") || true)
            .map(|n| MenuItem::Item {
                text: n.clone(),
                action: Action::Device(slot, n.clone()),
                danger: false,
                dot: None,
                enabled: true,
                checked: Some(n) == current.as_ref(),
                instant: false,
            })
            .collect();
        if items.is_empty() {
            items.push(MenuItem::Item { text: "No devices found".into(), action: Action::Reload, danger: false, dot: None, enabled: false, checked: false, instant: false });
        }
        self.open_menu(items, r.x, r.bottom() + 6.0, r.w.max(MENU_W), MenuOwner::Select(slot), false);
    }

    fn import_group_menu(&mut self) {
        let r = self.layout.dialog_group;
        let (current, typing_new) = match &self.dialog {
            Some(Dialog { kind: DialogKind::Import { group, new_group, busy: false, .. }, .. }) => (group.clone(), *new_group),
            _ => return,
        };
        let mut items = vec![MenuItem::Item {
            text: "No group".into(),
            action: Action::ImportGroup(String::new()),
            danger: false,
            dot: None,
            enabled: true,
            checked: !typing_new && current.is_empty(),
            instant: false,
        }];
        for g in &self.groups {
            let checked = !typing_new && *g == current;
            items.push(MenuItem::Item { text: g.clone(), action: Action::ImportGroup(g.clone()), danger: false, dot: self.group_color(g), enabled: true, checked, instant: false });
        }
        items.push(MenuItem::Separator);
        items.push(MenuItem::Item { text: "New group…".into(), action: Action::ImportNewGroup, danger: false, dot: None, enabled: true, checked: false, instant: false });
        self.open_menu(items, r.x, r.bottom() + 6.0, r.w.max(180.0), MenuOwner::ImportGroup, false);
    }

    // ---------------------------------------------------------------- drag & drop

    fn begin_drag(&mut self, id: u32, x: f32, y: f32) {
        self.close_menus();
        let visible = self.visible();
        let origin = visible.iter().position(|v| *v == id).unwrap_or(0);
        self.drag = Some(Drag { id, grab: self.down_pos, pos: (x, y), born: Instant::now(), slot: Some(origin), tab: None, origin });
        self.set_hover(None);
        self.drag_move(x, y);
    }

    fn drag_move(&mut self, x: f32, y: f32) {
        let Some(id) = self.drag.as_ref().map(|d| d.id) else { return };
        let current = self.clip(id).map(|c| c.group.clone()).unwrap_or_default();
        let tab = self
            .layout
            .tabs
            .iter()
            .find(|(k, r)| {
                k != ALL && !(k == NO_GROUP && current.is_empty()) && *k != current && x >= r.x - 4.0 && x <= r.right() + 4.0 && y >= r.y - 6.0 && y <= r.bottom() + 6.0
            })
            .map(|(k, _)| k.clone());
        let b = self.layout.board;
        let n = self.visible().len();
        let slot = if tab.is_some() || n == 0 || !(x >= b.x && x <= b.right() && y >= b.y - 20.0 && y <= b.bottom() + 20.0) {
            None
        } else {
            let scroll = self.scroll.get(&self.layout.view).copied().unwrap_or(0.0);
            let l = &self.layout;
            let py = (y - b.y + scroll).max(0.0);
            let row = (py / l.rowh).floor() as usize;
            let col = (((x - l.area_x) / l.colw).floor().max(0.0) as usize).min(l.cols - 1);
            let k = (row * l.cols + col).saturating_sub(l.jobs);
            Some(k.min(n - 1))
        };
        if let Some(d) = &mut self.drag {
            d.pos = (x, y);
            if tab != d.tab || (slot.is_some() && slot != d.slot) {
                self.slide_until = Instant::now() + std::time::Duration::from_millis(300);
            }
            d.tab = tab;
            if slot.is_some() {
                d.slot = slot;
            }
        }
        // near the board's top or bottom edge: scroll (faster the closer)
        let edge = 48.0;
        let speed = if y < b.y + edge { -(b.y + edge - y) } else if y > b.bottom() - edge { y - (b.bottom() - edge) } else { 0.0 };
        if speed != 0.0 && x >= b.x && x <= b.right() {
            super::window::set_timer(self.hwnd, super::window::TIMER_DRAG, 16);
            self.layout.board_scroll_speed = speed.clamp(-48.0, 48.0);
        } else {
            self.layout.board_scroll_speed = 0.0;
        }
        self.invalidate();
    }

    /// The drag auto-scroll timer.
    pub fn drag_tick(&mut self) -> bool {
        let speed = self.layout.board_scroll_speed;
        if self.drag.is_none() || speed == 0.0 {
            return false;
        }
        let v = self.layout.view.clone();
        let cur = self.scroll_target.get(&v).copied().unwrap_or(0.0);
        let s = (cur + speed / 2.0).clamp(0.0, self.layout.scroll_max);
        self.scroll_target.insert(v.clone(), s);
        self.anim.set(super::anim::key("scroll", super::view::hash_str(&v)), s);
        if let Some((x, y)) = self.drag.as_ref().map(|d| d.pos) {
            self.drag_move(x, y);
        }
        true
    }

    fn drop_drag(&mut self, x: f32, y: f32) {
        self.drag_move(x, y);
        let Some(d) = self.drag.take() else { return };
        self.layout.board_scroll_speed = 0.0;
        let ghost = (d.pos.0 + 16.0, d.pos.1 + 16.0);
        if let Some(tab) = d.tab {
            // into a group: drift towards the tab while fading, then move the files
            if let Some((_, r)) = self.layout.tabs.iter().find(|(k, _)| *k == tab) {
                let to = (ghost.0 + (r.cx() - ghost.0) / 4.0, ghost.1 + (r.y - ghost.1) / 4.0);
                self.landing = Some(Landing { id: d.id, from: ghost, to, since: Instant::now() });
            }
            let group = if tab == NO_GROUP { String::new() } else { tab };
            self.move_clip(d.id, &group, true);
            return;
        }
        let visible = self.visible();
        let without: Vec<u32> = visible.iter().copied().filter(|v| *v != d.id).collect();
        let slot = d.slot.unwrap_or(d.origin).min(without.len());
        let moved = slot != d.origin;
        if moved {
            let name = self.clip(d.id).map(|c| c.name.clone()).unwrap_or_default();
            let order = if slot < without.len() {
                let anchor = self.clip(without[slot]).map(|c| c.name.clone()).unwrap_or_default();
                library::move_in_order(&self.cfg.order, &name, &anchor, false)
            } else {
                let anchor = self.clip(*without.last().unwrap()).map(|c| c.name.clone()).unwrap_or_default();
                library::move_in_order(&self.cfg.order, &name, &anchor, true)
            };
            self.set_order(order);
            self.flash_tile(d.id, 0.16);
        }
        // the lifted copy drifts toward the tile's spot while it fades out
        let target = self.layout.tiles.iter().find(|(i, _)| *i == d.id).map(|(_, r)| *r);
        let scroll = self.scroll.get(&self.layout.view).copied().unwrap_or(0.0);
        let to = match target {
            Some(r) => {
                let (cx, cy) = (r.cx(), r.cy() + self.layout.board.y - scroll);
                (ghost.0 + (cx - ghost.0 - 60.0) / 3.0, ghost.1 + (cy - ghost.1 - 25.0) / 3.0)
            }
            None => ghost,
        };
        self.landing = Some(Landing { id: d.id, from: ghost, to, since: Instant::now() });
        self.slide_until = Instant::now() + std::time::Duration::from_millis(300);
        self.invalidate();
    }

    pub fn cancel_drag(&mut self) {
        if let Some(d) = self.drag.take() {
            self.landing = Some(Landing { id: d.id, from: (d.pos.0 + 16.0, d.pos.1 + 16.0), to: (d.pos.0 + 16.0, d.pos.1 + 16.0), since: Instant::now() });
            self.slide_until = Instant::now() + std::time::Duration::from_millis(300);
            self.down = None;
            self.invalidate();
        }
    }

    // ---------------------------------------------------------------- keyboard

    /// A key press. Returns true if it was used.
    /// Arrow keys walk the open menu (the deepest one), Enter picks, Right opens a submenu,
    /// Left closes it. Returns whether the key was used. (Not Space: its WM_CHAR would still
    /// type into a focused field.)
    fn menu_key(&mut self, vk: u32) -> bool {
        const UP: u32 = 0x26;
        const DOWN: u32 = 0x28;
        const LEFT: u32 = 0x25;
        const RIGHT: u32 = 0x27;
        const ENTER: u32 = 0x0D;
        let level = self.menus.len() - 1;
        let pickable = |it: &MenuItem| matches!(it, MenuItem::Item { enabled: true, .. } | MenuItem::Submenu { .. });
        let m = &self.menus[level];
        let rows: Vec<usize> = m.items.iter().enumerate().filter(|(_, it)| pickable(it)).map(|(i, _)| i).collect();
        if rows.is_empty() {
            return false;
        }
        // start from the row under the pointer, if any
        let cur = m.hover.or(match &self.hover {
            Some(Hit::Menu(l, i)) if *l == level => Some(*i),
            _ => None,
        });
        match vk {
            UP | DOWN => {
                let pos = cur.and_then(|c| rows.iter().position(|r| *r == c));
                let next = match (pos, vk == DOWN) {
                    (None, true) => rows[0],
                    (None, false) => rows[rows.len() - 1],
                    (Some(p), true) => rows[(p + 1) % rows.len()],
                    (Some(p), false) => rows[(p + rows.len() - 1) % rows.len()],
                };
                self.menus[level].hover = Some(next);
                self.hover = None;
            }
            ENTER | RIGHT => {
                let Some(i) = cur else { return vk != RIGHT };
                let is_sub = matches!(self.menus[level].items.get(i), Some(MenuItem::Submenu { .. }));
                if vk == RIGHT && !is_sub {
                    return false;
                }
                if is_sub {
                    self.open_submenu(level, i);
                    if let Some(first) = self.menus.last().and_then(|m| m.items.iter().position(pickable)) {
                        self.menus.last_mut().unwrap().hover = Some(first);
                    }
                } else {
                    self.menu_click(level, i);
                }
            }
            LEFT if level > 0 => self.truncate_menus(level),
            _ => return false,
        }
        self.invalidate();
        true
    }

    pub fn key_down(&mut self, vk: u32, ctrl: bool, shift: bool) -> bool {
        const ESC: u32 = 0x1B;
        const ENTER: u32 = 0x0D;
        const TAB: u32 = 0x09;
        let hwnd = self.hwnd;
        let get = move || shell::clipboard_text(hwnd);
        let set = move |s: &str| shell::set_clipboard_text(hwnd, s);
        if !self.menus.is_empty() && self.menu_key(vk) {
            return true;
        }
        if self.dialog.is_some() {
            if !self.menus.is_empty() && vk == ESC {
                self.close_menus();
                return true;
            }
            match vk {
                ESC => self.close_dialog(),
                ENTER => self.dialog_ok(),
                TAB => {
                    let d = self.dialog.as_mut().unwrap();
                    let n = d.fields.len();
                    if n > 0 {
                        d.focus = if shift { (d.focus + n - 1) % n } else { (d.focus + 1) % n };
                        d.fields[d.focus].select_all();
                    }
                }
                _ => {
                    let d = self.dialog.as_mut().unwrap();
                    let i = d.focus;
                    if let Some(f) = d.fields.get_mut(i) {
                        if let KeyResult::Ignored = f.key(vk, ctrl, shift, &get, &set) {
                            return false;
                        }
                    }
                }
            }
            self.caret_since = Instant::now();
            self.invalidate();
            return true;
        }
        if vk == ESC {
            // the topmost thing: a drag, a menu, the search text, else Settings
            if self.drag.is_some() {
                self.cancel_drag();
            } else if !self.menus.is_empty() {
                self.close_menus();
            } else if self.search_focused && !self.search.text.is_empty() {
                self.search.set("");
                self.search_changed();
            } else if self.settings_open {
                self.open_settings(false);
            } else if self.search_focused {
                self.search_focused = false;
                self.update_typing();
                self.invalidate();
            }
            return true;
        }
        if ctrl && vk == 0x46 {
            // Ctrl+F
            self.search_focused = true;
            self.search.select_all();
            self.caret_since = Instant::now();
            self.update_typing();
            self.invalidate();
            return true;
        }
        if self.search_focused {
            let r = self.search.key(vk, ctrl, shift, &get, &set);
            self.caret_since = Instant::now();
            match r {
                KeyResult::Changed => self.search_changed(),
                KeyResult::Moved => self.invalidate(),
                KeyResult::Ignored => return false,
            }
            return true;
        }
        if ctrl && vk == 0x56 {
            // Ctrl+V on the board is Quick import
            self.quick_import();
            return true;
        }
        false
    }

    pub fn char_typed(&mut self, ch: char) {
        if let Some(d) = &mut self.dialog {
            let i = d.focus;
            if let Some(f) = d.fields.get_mut(i) {
                f.char(ch);
                self.caret_since = Instant::now();
                self.invalidate();
            }
            return;
        }
        if self.search_focused {
            if let KeyResult::Changed = self.search.char(ch) {
                self.caret_since = Instant::now();
                self.search_changed();
            }
        }
    }

    /// Another program came to the front: menus and Settings close (they'd be left floating).
    pub fn deactivated(&mut self) {
        if !self.menus.is_empty() {
            self.close_menus();
        }
        if self.settings_open {
            self.open_settings(false);
        }
        self.cancel_drag();
        self.update_typing();
    }

    /// Is the pointer over text (for the I-beam cursor)?
    pub fn over_text(&self) -> bool {
        matches!(self.hover, Some(Hit::Search) | Some(Hit::Field(_)))
    }
}
