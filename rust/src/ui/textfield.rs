//! A single-line text field drawn like the rest of the UI (no native child window): caret,
//! selection, mouse placement and drag-select, word jumps, clipboard, and a placeholder.

use super::gfx::{Align, Canvas, Gfx, Rect, Weight};
use crate::theme::Rgb;

#[derive(Clone, Debug, Default)]
pub struct TextField {
    pub text: String,
    /// byte offsets (always on char boundaries)
    pub caret: usize,
    pub anchor: usize,
    pub placeholder: String,
    pub scroll: f32,
    pub size: f32,
    /// true while the mouse is drag-selecting
    pub dragging: bool,
}

pub enum KeyResult {
    /// the text changed
    Changed,
    /// caret/selection moved only
    Moved,
    /// not a key the field uses
    Ignored,
}

fn utf16_len(s: &str) -> u32 {
    s.encode_utf16().count() as u32
}

fn byte_at_utf16(s: &str, u: u32) -> usize {
    let mut n = 0u32;
    for (i, c) in s.char_indices() {
        if n >= u {
            return i;
        }
        n += c.len_utf16() as u32;
    }
    s.len()
}

impl TextField {
    pub fn new(placeholder: &str) -> TextField {
        TextField { placeholder: placeholder.into(), size: 13.0, ..Default::default() }
    }

    pub fn with_text(placeholder: &str, text: &str) -> TextField {
        let mut f = TextField::new(placeholder);
        f.set(text);
        f.select_all();
        f
    }

    pub fn set(&mut self, text: &str) {
        self.text = text.to_string();
        self.caret = self.text.len();
        self.anchor = self.caret;
        self.scroll = 0.0;
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.text.len();
    }

    fn sel(&self) -> (usize, usize) {
        (self.caret.min(self.anchor), self.caret.max(self.anchor))
    }

    pub fn selected(&self) -> &str {
        let (a, b) = self.sel();
        &self.text[a..b]
    }

    fn prev(&self, i: usize) -> usize {
        self.text[..i].char_indices().next_back().map(|(j, _)| j).unwrap_or(0)
    }

    fn next(&self, i: usize) -> usize {
        self.text[i..].chars().next().map(|c| i + c.len_utf8()).unwrap_or(i)
    }

    fn word_left(&self, i: usize) -> usize {
        let mut j = i;
        while j > 0 && self.text[..j].ends_with(char::is_whitespace) {
            j = self.prev(j);
        }
        while j > 0 && !self.text[..j].ends_with(char::is_whitespace) {
            j = self.prev(j);
        }
        j
    }

    fn word_right(&self, i: usize) -> usize {
        let mut j = i;
        while j < self.text.len() && !self.text[j..].starts_with(char::is_whitespace) {
            j = self.next(j);
        }
        while j < self.text.len() && self.text[j..].starts_with(char::is_whitespace) {
            j = self.next(j);
        }
        j
    }

    /// Replace the selection with `s` (typing / pasting). Newlines become spaces.
    pub fn insert(&mut self, s: &str) {
        let clean: String = s.chars().map(|c| if c == '\r' || c == '\n' || c == '\t' { ' ' } else { c }).filter(|c| !c.is_control()).collect();
        let (a, b) = self.sel();
        self.text.replace_range(a..b, &clean);
        self.caret = a + clean.len();
        self.anchor = self.caret;
    }

    fn delete_sel(&mut self) -> bool {
        let (a, b) = self.sel();
        if a == b {
            return false;
        }
        self.text.replace_range(a..b, "");
        self.caret = a;
        self.anchor = a;
        true
    }

    /// A typed character (WM_CHAR).
    pub fn char(&mut self, c: char) -> KeyResult {
        if c.is_control() {
            return KeyResult::Ignored;
        }
        let mut b = [0u8; 4];
        self.insert(c.encode_utf8(&mut b));
        KeyResult::Changed
    }

    /// A key press (WM_KEYDOWN). Clipboard keys go through `clip_get` / `clip_set`.
    pub fn key(&mut self, vk: u32, ctrl: bool, shift: bool, clip_get: &dyn Fn() -> Option<String>, clip_set: &dyn Fn(&str)) -> KeyResult {
        let moved = |f: &mut TextField, to: usize| {
            f.caret = to;
            if !shift {
                f.anchor = to;
            }
            KeyResult::Moved
        };
        match vk {
            0x25 => {
                // left
                let (a, b) = self.sel();
                if !shift && a != b {
                    return moved(self, a);
                }
                let to = if ctrl { self.word_left(self.caret) } else { self.prev(self.caret) };
                moved(self, to)
            }
            0x27 => {
                let (a, b) = self.sel();
                if !shift && a != b {
                    return moved(self, b);
                }
                let to = if ctrl { self.word_right(self.caret) } else { self.next(self.caret) };
                moved(self, to)
            }
            0x24 => moved(self, 0),
            0x23 => moved(self, self.text.len()),
            0x08 => {
                if !self.delete_sel() {
                    let to = if ctrl { self.word_left(self.caret) } else { self.prev(self.caret) };
                    self.text.replace_range(to..self.caret, "");
                    self.caret = to;
                    self.anchor = to;
                }
                KeyResult::Changed
            }
            0x2E => {
                if !self.delete_sel() {
                    let to = if ctrl { self.word_right(self.caret) } else { self.next(self.caret) };
                    self.text.replace_range(self.caret..to, "");
                }
                KeyResult::Changed
            }
            0x41 if ctrl => {
                self.select_all();
                KeyResult::Moved
            }
            0x43 if ctrl => {
                if !self.selected().is_empty() {
                    clip_set(self.selected());
                }
                KeyResult::Moved
            }
            0x58 if ctrl => {
                if !self.selected().is_empty() {
                    clip_set(self.selected());
                    self.delete_sel();
                }
                KeyResult::Changed
            }
            0x56 if ctrl => {
                if let Some(t) = clip_get() {
                    self.insert(&t);
                }
                KeyResult::Changed
            }
            _ => KeyResult::Ignored,
        }
    }

    fn text_x(&self, g: &mut Gfx, byte: usize) -> f32 {
        let pos = utf16_len(&self.text[..byte]);
        g.layout(&self.text, self.size, Weight::Normal, 0.0, 1, Align::Left).map(|l| l.caret_x(pos)).unwrap_or(0.0)
    }

    /// Byte offset under x (relative to the field's text area).
    pub fn hit(&self, g: &mut Gfx, x: f32) -> usize {
        let u = g.layout(&self.text, self.size, Weight::Normal, 0.0, 1, Align::Left).map(|l| l.hit(x + self.scroll)).unwrap_or(0);
        byte_at_utf16(&self.text, u)
    }

    pub fn click(&mut self, g: &mut Gfx, x: f32, shift: bool) {
        self.caret = self.hit(g, x);
        if !shift {
            self.anchor = self.caret;
        }
        self.dragging = true;
    }

    pub fn drag_to(&mut self, g: &mut Gfx, x: f32) {
        if self.dragging {
            self.caret = self.hit(g, x);
        }
    }

    /// Select the word at x (double click).
    pub fn select_word(&mut self, g: &mut Gfx, x: f32) {
        let i = self.hit(g, x);
        let mut a = i;
        while a > 0 && !self.text[..a].ends_with(char::is_whitespace) {
            a = self.prev(a);
        }
        let mut b = i;
        while b < self.text.len() && !self.text[b..].starts_with(char::is_whitespace) {
            b = self.next(b);
        }
        self.anchor = a;
        self.caret = b;
    }

    /// Draw the text (or placeholder), selection and caret inside `r` (the text area).
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, c: &Canvas, g: &mut Gfx, r: Rect, focused: bool, caret_on: bool, text: Rgb, muted: Rgb, sel: Rgb) {
        let line_h = self.size * 1.33;
        let ty = r.y + (r.h - line_h) / 2.0;
        // keep the caret in view
        let cx = self.text_x(g, self.caret);
        if cx - self.scroll > r.w - 2.0 {
            self.scroll = cx - r.w + 2.0;
        } else if cx - self.scroll < 0.0 {
            self.scroll = cx;
        }
        let full = g.measure(&self.text, self.size, Weight::Normal);
        if full - self.scroll < r.w - 2.0 {
            self.scroll = (full - r.w + 2.0).max(0.0);
        }
        c.push_clip(r);
        if self.text.is_empty() {
            c.text(g, r.x, ty, &self.placeholder, self.size, Weight::Normal, r.w, 1, Align::Left, muted);
        } else {
            let (a, b) = self.sel();
            if focused && a != b {
                let (xa, xb) = (self.text_x(g, a), self.text_x(g, b));
                c.fill_rect(Rect::new(r.x + xa - self.scroll, ty, xb - xa, line_h), sel);
            }
            c.text(g, r.x - self.scroll, ty, &self.text, self.size, Weight::Normal, 0.0, 1, Align::Left, text);
        }
        if focused && caret_on {
            let x = (r.x + cx - self.scroll).round();
            c.fill_rect(Rect::new(x, ty + 1.0, 1.0, line_h - 2.0), text);
        }
        c.pop_clip();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> Option<String> {
        None
    }

    #[test]
    fn typing_editing_and_words() {
        let mut f = TextField::new("x");
        for ch in "hello wörld".chars() {
            f.char(ch);
        }
        assert_eq!(f.text, "hello wörld");
        f.key(0x08, true, false, &none, &|_| {}); // ctrl+backspace: word
        assert_eq!(f.text, "hello ");
        f.key(0x24, false, false, &none, &|_| {});
        f.key(0x27, true, true, &none, &|_| {}); // ctrl+shift+right selects "hello "
        assert_eq!(f.selected(), "hello ");
        f.char('X');
        assert_eq!(f.text, "X");
        f.insert("a\r\nb");
        assert_eq!(f.text, "Xa  b");
    }

    #[test]
    fn utf16_mapping() {
        assert_eq!(byte_at_utf16("aæ😀b", 2), 3);
        assert_eq!(byte_at_utf16("aæ😀b", 4), 7);
        assert_eq!(utf16_len("😀"), 2);
    }
}
