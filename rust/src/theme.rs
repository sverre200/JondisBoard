//! Colour themes and the runtime theme manager.
//!
//! All themes share one cool, dark foundation (the surfaces, borders and text of JondisClip's
//! family look) and differ only in their accent, so they read as accent variations of the same
//! app. The UI never caches a colour: it asks the current [`Theme`] each time it paints, so
//! switching theme is just swapping this value and repainting once (no rebuild, nothing lost).

/// An sRGB colour, 0xRRGGBB.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgb(pub u32);

impl Rgb {
    pub const fn r(self) -> u8 {
        (self.0 >> 16) as u8
    }
    pub const fn g(self) -> u8 {
        (self.0 >> 8) as u8
    }
    pub const fn b(self) -> u8 {
        self.0 as u8
    }

    /// Parse "#rrggbb".
    pub fn parse(s: &str) -> Option<Rgb> {
        let s = s.strip_prefix('#')?;
        if s.len() != 6 {
            return None;
        }
        u32::from_str_radix(s, 16).ok().map(Rgb)
    }

    pub fn hex(self) -> String {
        format!("#{:06x}", self.0)
    }

    /// Blend self toward `other` by t (0..1), rounding like the Python app's `mix()`.
    pub fn mix(self, other: Rgb, t: f32) -> Rgb {
        let ch = |a: u8, b: u8| -> u32 {
            let v = a as f32 + (b as f32 - a as f32) * t;
            // Python's round() is banker's rounding; the difference is invisible, use half-up.
            v.round().clamp(0.0, 255.0) as u32
        };
        Rgb((ch(self.r(), other.r()) << 16) | (ch(self.g(), other.g()) << 8) | ch(self.b(), other.b()))
    }

    /// COLORREF (0x00BBGGRR) for GDI / DWM.
    pub const fn colorref(self) -> u32 {
        ((self.b() as u32) << 16) | ((self.g() as u32) << 8) | self.r() as u32
    }
}

pub const WHITE: Rgb = Rgb(0xffffff);
pub const BLACK: Rgb = Rgb(0x000000);
pub const GOOD: Rgb = Rgb(0x3ddc97);
pub const WARN: Rgb = Rgb(0xffb547);
pub const DANGER: Rgb = Rgb(0xf04566);
pub const DANGER_HOVER: Rgb = Rgb(0xd63655);

/// The 8 group colours, in the order new groups get them.
pub const GROUP_COLORS: [Rgb; 8] = [
    Rgb(0x7c5cff),
    Rgb(0x3b82f6),
    Rgb(0x14b8a6),
    Rgb(0x22c55e),
    Rgb(0xeab308),
    Rgb(0xf97316),
    Rgb(0xef4444),
    Rgb(0xec4899),
];

/// One theme: the 11 base tokens plus the derived ones.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    pub bg: Rgb,
    pub surface: Rgb,
    pub card: Rgb,
    pub card_hover: Rgb,
    pub card_playing: Rgb,
    pub border: Rgb,
    pub text: Rgb,
    /// secondary text (metadata, labels)
    pub muted: Rgb,
    /// tertiary text: section titles, counts, placeholders
    pub faint: Rgb,
    /// a stronger border: hover, the selected row
    pub border_strong: Rgb,
    pub accent: Rgb,
    pub accent_hover: Rgb,
    pub accent_text: Rgb,
    // derived (see `derive`)
    /// menus, popovers, toasts
    pub elevated: Rgb,
    /// their 1 px outline
    pub edge: Rgb,
    /// a quiet button that's switched on
    pub accent_soft: Rgb,
    pub accent_soft_hover: Rgb,
    pub accent_edge: Rgb,
}

#[allow(clippy::too_many_arguments)]
const fn base(
    name: &'static str,
    bg: u32,
    surface: u32,
    card: u32,
    card_hover: u32,
    card_playing: u32,
    border: u32,
    text: u32,
    muted: u32,
    accent: u32,
    accent_hover: u32,
    accent_text: u32,
) -> Theme {
    Theme {
        name,
        bg: Rgb(bg),
        surface: Rgb(surface),
        card: Rgb(card),
        card_hover: Rgb(card_hover),
        card_playing: Rgb(card_playing),
        border: Rgb(border),
        text: Rgb(text),
        muted: Rgb(muted),
        faint: Rgb(0),
        border_strong: Rgb(0),
        accent: Rgb(accent),
        accent_hover: Rgb(accent_hover),
        accent_text: Rgb(accent_text),
        elevated: Rgb(0),
        edge: Rgb(0),
        accent_soft: Rgb(0),
        accent_soft_hover: Rgb(0),
        accent_edge: Rgb(0),
    }
}

/// The shared foundation: window, side/header/footer surfaces, cards, card hover, border, text,
/// secondary text. `card_playing` is worked out per accent in `derive`.
const N_BG: u32 = 0x0b1018;
const N_SURFACE: u32 = 0x0d131d;
const N_CARD: u32 = 0x151f2d;
const N_CARD_HOVER: u32 = 0x1c293b;
const N_BORDER: u32 = 0x223147;
const N_TEXT: u32 = 0xf0f4fa;
const N_MUTED: u32 = 0x91a4bd;

const fn accent(name: &'static str, accent: u32, hover: u32, on_accent: u32) -> Theme {
    base(name, N_BG, N_SURFACE, N_CARD, N_CARD_HOVER, 0, N_BORDER, N_TEXT, N_MUTED, accent, hover, on_accent)
}

const BASES: [Theme; 6] = [
    // the default: JondisBoard's green (it matches the icon)
    accent("Toxic Green", 0x45e33f, 0x5be656, 0x04120a),
    accent("Midnight Purple", 0x7c5cff, 0x8c70ff, 0xffffff),
    accent("Ocean Blue", 0x3b8cff, 0x539aff, 0xffffff),
    accent("Sunset Orange", 0xff8a2b, 0xff9a45, 0x1a0d04),
    accent("Crimson", 0xff3d5e, 0xff5873, 0xffffff),
    accent("Graphite", 0xe6e6ea, 0xf4f4f7, 0x111113),
];

pub const DEFAULT_THEME: &str = "Toxic Green";

fn derive(mut t: Theme) -> Theme {
    t.faint = Rgb(0x627994);
    t.border_strong = Rgb(0x2f4260);
    t.card_playing = t.card.mix(t.accent, 0.1);
    t.elevated = Rgb(0x162131);
    t.edge = Rgb(0x2b3c55);
    t.accent_soft = t.card.mix(t.accent, 0.16);
    t.accent_soft_hover = t.card.mix(t.accent, 0.26);
    t.accent_edge = t.border.mix(t.accent, 0.6);
    t
}

/// All themes, in menu order.
pub fn all() -> Vec<Theme> {
    BASES.iter().map(|t| derive(*t)).collect()
}

/// The theme called `name`, or the default one.
pub fn by_name(name: &str) -> Theme {
    BASES
        .iter()
        .find(|t| t.name == name)
        .map(|t| derive(*t))
        .unwrap_or_else(|| derive(BASES[0]))
}

/// The logo's tile colour for a theme (`logo.tile_color` in the Python app).
pub fn logo_tile(t: &Theme) -> Rgb {
    t.bg.mix(t.card, 0.7)
}

/// The runtime theme manager: holds the current theme and a generation counter that anything
/// caching theme-dependent resources (brushes, icons, pictures tinted with group colours) checks.
pub struct ThemeManager {
    current: Theme,
    generation: u64,
}

impl ThemeManager {
    pub fn new(name: &str) -> Self {
        ThemeManager { current: by_name(name), generation: 1 }
    }
    pub fn get(&self) -> &Theme {
        &self.current
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    /// Switch theme. Returns false (and does nothing) if it's already the current one.
    pub fn set(&mut self, name: &str) -> bool {
        let t = by_name(name);
        if t.name == self.current.name {
            return false;
        }
        self.current = t;
        self.generation += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix_matches_python() {
        // reference values from the Python app's mix()
        let a = Rgb::parse("#142018").unwrap();
        let b = Rgb::parse("#45e33f").unwrap();
        assert_eq!(a.mix(b, 0.0), a);
        assert_eq!(a.mix(b, 1.0), b);
        let m = a.mix(b, 0.16);
        assert_eq!(m.hex(), "#1c3f1e");
        assert_eq!(Rgb(0x0e1610).mix(Rgb(0x142018), 0.6).hex(), "#121c15");
        assert_eq!(Rgb(0x22362a).mix(Rgb(0xe6f5e9), 0.12).hex(), "#3a4d41");
    }

    #[test]
    fn six_themes_with_unique_names_and_default() {
        let all = all();
        assert_eq!(all.len(), 6);
        assert_eq!(by_name("nope").name, DEFAULT_THEME);
        assert_eq!(by_name("Crimson").accent, Rgb(0xff3d5e));
        for t in &all {
            assert_ne!(t.elevated, Rgb(0));
            // one foundation: only the accent differs
            assert_eq!((t.bg, t.card, t.text, t.muted), (all[0].bg, all[0].card, all[0].text, all[0].muted));
        }
    }

    #[test]
    fn manager_switches_and_counts_generations() {
        let mut m = ThemeManager::new("Graphite");
        let g = m.generation();
        assert!(!m.set("Graphite"));
        assert!(m.set("Ocean Blue"));
        assert_eq!(m.get().name, "Ocean Blue");
        assert_eq!(m.generation(), g + 1);
    }

    #[test]
    fn colorref_is_bgr() {
        assert_eq!(Rgb(0x112233).colorref(), 0x332211);
    }
}
