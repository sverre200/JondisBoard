//! Clock-timed tweens (the Python app's `animate()` / `fade_to()`): every hover, fade and slide
//! eases toward its target with ease-out cubic, and picks up from wherever it was if the target
//! changes half-way. While anything is still moving the window asks for another frame; when
//! everything has settled, nothing runs at all (no redraw loop when idle).

use crate::theme::Rgb;
use std::collections::HashMap;
use std::time::Instant;

/// Motion tokens (ms), the same as the Python app.
pub const MOTION_OPEN: f32 = 170.0;
pub const MOTION_CLOSE: f32 = 110.0;
pub const MOTION_HOVER: f32 = 90.0;
pub const MOTION_UNHOVER: f32 = 170.0;
pub const MOTION_FLASH: f32 = 360.0;
pub const MOTION_SLIDE: f32 = 220.0;
/// px a popup glides while it fades in
pub const SLIDE: f32 = 7.0;

pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

#[derive(Clone, Copy)]
enum Val {
    F(f32, f32),
    C(Rgb, Rgb),
}

struct Tween {
    from: Val,
    start: Instant,
    ms: f32,
    used: Instant,
}

pub struct Anim {
    now: Instant,
    tweens: HashMap<u64, Tween>,
    active: bool,
    slow: bool,
    frames: u64,
}

/// A key for a tween: FNV-1a over a tag and an id.
pub fn key(tag: &str, id: u64) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in tag.bytes().chain(id.to_le_bytes()) {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

impl Default for Anim {
    fn default() -> Self {
        Self::new()
    }
}

impl Anim {
    pub fn new() -> Anim {
        Anim { now: Instant::now(), tweens: HashMap::new(), active: false, slow: false, frames: 0 }
    }

    /// Call at the start of each frame.
    pub fn begin_frame(&mut self) {
        self.now = Instant::now();
        self.active = false;
        self.slow = false;
        self.frames += 1;
        if self.frames % 600 == 0 {
            let now = self.now;
            self.tweens.retain(|_, t| now.duration_since(t.used).as_secs_f32() < 5.0);
        }
    }

    pub fn now(&self) -> Instant {
        self.now
    }

    /// Is anything still moving (so another frame is needed)?
    pub fn active(&self) -> bool {
        self.active
    }

    /// Keep frames coming at the display's rate (short animations: fades, slides, flashes).
    pub fn keep_alive(&mut self) {
        self.active = true;
    }

    /// Keep frames coming at ~30 fps (long-running indicators: a playing sound's progress bar,
    /// an import's spinner), which is plenty for them and half the work.
    pub fn keep_slow(&mut self) {
        self.slow = true;
    }

    pub fn slow(&self) -> bool {
        self.slow
    }

    fn progress(&mut self, t: &Tween) -> f32 {
        if t.ms <= 0.0 {
            return 1.0;
        }
        let p = self.now.duration_since(t.start).as_secs_f32() * 1000.0 / t.ms;
        if p < 1.0 {
            self.active = true;
        }
        ease_out(p)
    }

    /// A number easing toward `target` over `ms`. The first call just sets it.
    pub fn value(&mut self, key: u64, target: f32, ms: f32) -> f32 {
        let now = self.now;
        let t = self.tweens.entry(key).or_insert(Tween { from: Val::F(target, target), start: now, ms: 0.0, used: now });
        t.used = now;
        let Val::F(from, to) = t.from else {
            t.from = Val::F(target, target);
            return target;
        };
        let t_snapshot = Tween { from: t.from, start: t.start, ms: t.ms, used: now };
        let e = self.progress(&t_snapshot);
        let cur = from + (to - from) * e;
        if (to - target).abs() > f32::EPSILON {
            let t = self.tweens.get_mut(&key).unwrap();
            t.from = Val::F(cur, target);
            t.start = now;
            t.ms = ms;
            if ms > 0.0 {
                self.active = true;
            }
            return if ms > 0.0 { cur } else { target };
        }
        cur
    }

    /// A colour easing toward `target` over `ms`. The first call just sets it.
    pub fn color(&mut self, key: u64, target: Rgb, ms: f32) -> Rgb {
        let now = self.now;
        let t = self.tweens.entry(key).or_insert(Tween { from: Val::C(target, target), start: now, ms: 0.0, used: now });
        t.used = now;
        let Val::C(from, to) = t.from else {
            t.from = Val::C(target, target);
            return target;
        };
        let snap = Tween { from: t.from, start: t.start, ms: t.ms, used: now };
        let e = self.progress(&snap);
        let cur = from.mix(to, e);
        if to != target {
            let t = self.tweens.get_mut(&key).unwrap();
            t.from = Val::C(cur, target);
            t.start = now;
            t.ms = ms;
            if ms > 0.0 {
                self.active = true;
                return cur;
            }
            return target;
        }
        cur
    }

    /// Jump a colour to `now_color` and let it ease back to wherever it's asked to go next
    /// (the tile flash).
    pub fn flash_color(&mut self, key: u64, now_color: Rgb) {
        let now = self.now;
        self.tweens.insert(key, Tween { from: Val::C(now_color, now_color), start: now, ms: 0.0, used: now });
    }

    /// Jump a number (no easing) — e.g. a slide's start position.
    pub fn set(&mut self, key: u64, v: f32) {
        let now = self.now;
        self.tweens.insert(key, Tween { from: Val::F(v, v), start: now, ms: 0.0, used: now });
    }

    pub fn forget(&mut self, key: u64) {
        self.tweens.remove(&key);
    }

    /// The target a number tween is heading to, if it exists.
    pub fn target(&self, key: u64) -> Option<f32> {
        match self.tweens.get(&key)?.from {
            Val::F(_, to) => Some(to),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_call_sets_then_eases_and_settles() {
        let mut a = Anim::new();
        a.begin_frame();
        assert_eq!(a.value(1, 10.0, 100.0), 10.0);
        assert!(!a.active());
        let v = a.value(1, 20.0, 100.0);
        assert_eq!(v, 10.0);
        assert!(a.active());
        std::thread::sleep(std::time::Duration::from_millis(120));
        a.begin_frame();
        assert_eq!(a.value(1, 20.0, 100.0), 20.0);
        assert!(!a.active());
    }

    #[test]
    fn colour_turns_around_mid_way() {
        let mut a = Anim::new();
        a.begin_frame();
        a.color(2, Rgb(0x000000), 100.0);
        a.color(2, Rgb(0xffffff), 100.0);
        std::thread::sleep(std::time::Duration::from_millis(40));
        a.begin_frame();
        let mid = a.color(2, Rgb(0xffffff), 100.0);
        assert!(mid.r() > 0 && mid.r() < 255);
        let back = a.color(2, Rgb(0x000000), 100.0);
        assert_eq!(back, mid); // starts back from where it was
    }

    #[test]
    fn ease_out_cubic() {
        assert_eq!(ease_out(0.0), 0.0);
        assert_eq!(ease_out(1.0), 1.0);
        assert!(ease_out(0.5) > 0.8);
    }
}
