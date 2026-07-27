//! Named color themes (RGB truecolor) + prompt strings. Porta
//! `legacy/lib/rege/theme.rb`. Selected via config `ui.theme`.

pub const DEFAULT: &str = "hacker";

/// A palette is built around four levels of loudness, not around one hue:
/// `strong` (headings, bold) > `text` (prose) > `dim` (chrome, tool output),
/// with `accent` for the theme's own marks and `accent2` for code. `accent2`
/// deliberately sits on a different hue from `accent` — a palette painted in a
/// single hue reads as one flat wash, which is what the gold `luxury` looked
/// like before.
pub struct Palette {
    pub name: &'static str,
    pub accent: (u8, u8, u8),
    pub accent2: (u8, u8, u8),
    pub dim: (u8, u8, u8),
    pub text: (u8, u8, u8),
    pub strong: (u8, u8, u8),
    pub ok: (u8, u8, u8),
    pub warn: (u8, u8, u8),
    pub fail: (u8, u8, u8),
    pub prompt: &'static str,
}

/// Cursor-only accent: kept fully saturated even on the desaturated `hacker`
/// palette, so the input caret still reads as "alive" against sober text.
pub const NEON: (u8, u8, u8) = (0, 255, 102);

const PALETTES: &[Palette] = &[
    Palette {
        name: "hacker",
        accent: (92, 232, 138),
        accent2: (99, 216, 255),
        dim: (124, 138, 128),
        text: (214, 226, 216),
        strong: (255, 255, 255),
        ok: (92, 232, 138),
        warn: (255, 200, 0),
        fail: (255, 90, 90),
        prompt: "\u{276f} ",
    },
    Palette {
        name: "luxury",
        accent: (212, 175, 55),
        accent2: (104, 196, 146),
        dim: (146, 136, 120),
        text: (236, 228, 212),
        strong: (255, 250, 235),
        ok: (104, 196, 146),
        warn: (230, 170, 70),
        fail: (216, 88, 84),
        prompt: "\u{276f} ",
    },
    Palette {
        name: "cyberpunk",
        accent: (255, 42, 109),
        accent2: (5, 217, 232),
        dim: (158, 124, 146),
        text: (245, 225, 240),
        strong: (255, 255, 255),
        ok: (57, 255, 20),
        warn: (249, 240, 2),
        fail: (255, 42, 109),
        prompt: "\u{276f} ",
    },
    Palette {
        name: "synthwave",
        accent: (255, 110, 199),
        accent2: (114, 239, 221),
        dim: (154, 138, 178),
        text: (238, 224, 255),
        strong: (255, 255, 255),
        ok: (114, 239, 221),
        warn: (255, 215, 120),
        fail: (255, 90, 140),
        prompt: "\u{276f} ",
    },
    Palette {
        name: "dracula",
        accent: (189, 147, 249),
        accent2: (98, 224, 250),
        dim: (128, 142, 190),
        text: (248, 248, 242),
        strong: (255, 255, 255),
        ok: (80, 250, 123),
        warn: (241, 250, 140),
        fail: (255, 85, 85),
        prompt: "\u{276f} ",
    },
    Palette {
        name: "forest",
        accent: (88, 204, 120),
        accent2: (226, 183, 92),
        dim: (132, 150, 136),
        text: (223, 238, 223),
        strong: (247, 255, 247),
        ok: (88, 204, 120),
        warn: (226, 183, 92),
        fail: (230, 90, 80),
        prompt: "\u{276f} ",
    },
    Palette {
        name: "ember",
        accent: (255, 122, 60),
        accent2: (124, 196, 226),
        dim: (156, 130, 116),
        text: (250, 228, 210),
        strong: (255, 246, 238),
        ok: (140, 210, 140),
        warn: (255, 190, 90),
        fail: (255, 80, 70),
        prompt: "\u{276f} ",
    },
];

#[derive(Clone, Copy)]
pub enum Role {
    Accent,
    Accent2,
    Dim,
    Text,
    Strong,
    Ok,
    Warn,
    Fail,
    Neon,
}

pub fn names() -> Vec<&'static str> {
    PALETTES.iter().map(|p| p.name).collect()
}

pub fn exists(name: &str) -> bool {
    PALETTES.iter().any(|p| p.name == name)
}

fn palette(name: &str) -> &'static Palette {
    PALETTES.iter().find(|p| p.name == name).unwrap_or(&PALETTES[0])
}

pub fn color(name: &str, role: Role) -> (u8, u8, u8) {
    let p = palette(name);
    match role {
        Role::Accent => p.accent,
        Role::Accent2 => p.accent2,
        Role::Dim => p.dim,
        Role::Text => p.text,
        Role::Strong => p.strong,
        Role::Ok => p.ok,
        Role::Warn => p.warn,
        Role::Fail => p.fail,
        Role::Neon => NEON,
    }
}

pub fn prompt(name: &str) -> &'static str {
    palette(name).prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_hacker() {
        assert_eq!(DEFAULT, "hacker");
        assert!(exists(DEFAULT));
    }

    #[test]
    fn all_names_resolve_a_palette() {
        for n in names() {
            assert!(exists(n));
            let _ = color(n, Role::Accent);
            assert!(!prompt(n).is_empty());
        }
    }

    #[test]
    fn unknown_theme_falls_back_to_default() {
        assert_eq!(color("nope", Role::Accent), color(DEFAULT, Role::Accent));
        assert_eq!(prompt("nope"), prompt(DEFAULT));
    }

    /// Rough perceptual weight of a color on a dark terminal. Not gamma-correct
    /// — it only has to separate "reads as chrome" from "vanishes".
    fn lum(c: (u8, u8, u8)) -> f32 {
        (0.2126 * c.0 as f32 + 0.7152 * c.1 as f32 + 0.0722 * c.2 as f32) / 255.0
    }

    fn dist(a: (u8, u8, u8), b: (u8, u8, u8)) -> f32 {
        let d = |x: u8, y: u8| (x as f32 - y as f32).powi(2);
        (d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)).sqrt()
    }

    #[test]
    fn every_theme_keeps_its_levels_apart() {
        // `luxury` used to paint accent, accent2, dim and text all in gold: on
        // screen the whole conversation was one flat wash. Each palette owes
        // four distinguishable levels and two distinct accent hues.
        for n in names() {
            let (accent, accent2) = (color(n, Role::Accent), color(n, Role::Accent2));
            let (dim, text, strong) = (color(n, Role::Dim), color(n, Role::Text), color(n, Role::Strong));
            assert!(dist(accent, accent2) >= 100.0, "{n}: code and the theme marks share one color");
            assert!(dist(text, dim) >= 90.0, "{n}: prose and chrome share one color");
            assert!(dist(text, accent2) >= 60.0, "{n}: code does not stand out from prose");
            assert!(lum(dim) >= 0.40, "{n}: dim too dark to read on the terminal background");
            assert!(lum(strong) > lum(text), "{n}: bold has to go up, not down");
        }
    }

    #[test]
    fn seven_named_themes() {
        assert_eq!(names().len(), 7);
    }
}
