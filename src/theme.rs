//! Named color themes (RGB truecolor) + prompt strings. Porta
//! `legacy/lib/regente/theme.rb`. Selected via config `ui.theme`.

pub const DEFAULT: &str = "hacker";

pub struct Palette {
    pub name: &'static str,
    pub accent: (u8, u8, u8),
    pub accent2: (u8, u8, u8),
    pub dim: (u8, u8, u8),
    pub text: (u8, u8, u8),
    pub ok: (u8, u8, u8),
    pub warn: (u8, u8, u8),
    pub fail: (u8, u8, u8),
    pub prompt: &'static str,
}

const PALETTES: &[Palette] = &[
    Palette {
        name: "hacker",
        accent: (0, 255, 65),
        accent2: (0, 170, 60),
        dim: (70, 120, 80),
        text: (150, 255, 150),
        ok: (0, 255, 65),
        warn: (255, 200, 0),
        fail: (255, 60, 60),
        prompt: "root@regente:~# ",
    },
    Palette {
        name: "luxury",
        accent: (212, 175, 55),
        accent2: (160, 130, 60),
        dim: (120, 105, 70),
        text: (232, 220, 192),
        ok: (200, 170, 80),
        warn: (220, 160, 60),
        fail: (200, 70, 70),
        prompt: "❖ ",
    },
    Palette {
        name: "cyberpunk",
        accent: (255, 42, 109),
        accent2: (5, 217, 232),
        dim: (120, 60, 90),
        text: (255, 220, 240),
        ok: (57, 255, 20),
        warn: (249, 240, 2),
        fail: (255, 42, 109),
        prompt: "▶ ",
    },
    Palette {
        name: "synthwave",
        accent: (255, 110, 199),
        accent2: (114, 239, 221),
        dim: (110, 90, 140),
        text: (240, 220, 255),
        ok: (114, 239, 221),
        warn: (255, 215, 120),
        fail: (255, 90, 140),
        prompt: "➤ ",
    },
    Palette {
        name: "dracula",
        accent: (189, 147, 249),
        accent2: (255, 121, 198),
        dim: (98, 114, 164),
        text: (248, 248, 242),
        ok: (80, 250, 123),
        warn: (241, 250, 140),
        fail: (255, 85, 85),
        prompt: "λ ",
    },
    Palette {
        name: "forest",
        accent: (88, 204, 120),
        accent2: (40, 140, 90),
        dim: (80, 110, 85),
        text: (220, 240, 220),
        ok: (88, 204, 120),
        warn: (220, 190, 80),
        fail: (230, 90, 80),
        prompt: "❯ ",
    },
    Palette {
        name: "ember",
        accent: (255, 122, 60),
        accent2: (255, 180, 80),
        dim: (130, 90, 70),
        text: (255, 225, 200),
        ok: (120, 200, 120),
        warn: (255, 180, 80),
        fail: (255, 80, 70),
        prompt: "❯ ",
    },
];

pub enum Role {
    Accent,
    Accent2,
    Dim,
    Text,
    Ok,
    Warn,
    Fail,
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
        Role::Ok => p.ok,
        Role::Warn => p.warn,
        Role::Fail => p.fail,
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

    #[test]
    fn seven_named_themes() {
        assert_eq!(names().len(), 7);
    }
}
