//! "Claude Buddy"-like tamagotchi companion. Deterministic hatch from a seed
//! string (same seed -> same species/rarity/stats), petted via `/buddy pet`.

use std::collections::BTreeMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rarity {
    Common,
    Uncommon,
    Rare,
    Epic,
    Legendary,
}

impl Rarity {
    pub fn label(self) -> &'static str {
        match self {
            Rarity::Common => "Comum",
            Rarity::Uncommon => "Incomum",
            Rarity::Rare => "Raro",
            Rarity::Epic => "Epico",
            Rarity::Legendary => "Lendario",
        }
    }

    /// Weighted pick from a 0-99 roll (common most likely, legendary rarest).
    fn from_roll(roll: u8) -> Rarity {
        let roll = roll % 100;
        match roll {
            0..=49 => Rarity::Common,
            50..=74 => Rarity::Uncommon,
            75..=89 => Rarity::Rare,
            90..=97 => Rarity::Epic,
            _ => Rarity::Legendary,
        }
    }
}

struct Species {
    key: &'static str,
    name_pt: &'static str,
    art: &'static [&'static str],
}

const SPECIES: &[Species] = &[
    Species { key: "duck", name_pt: "pato", art: &["  __", "<(o )___", " ( ._> /", "  `---'"] },
    Species { key: "goose", name_pt: "ganso", art: &[" ,_?", " )_)-<", " \" \""] },
    Species { key: "cat", name_pt: "gato", art: &[" /\\_/\\", "( o.o )", " > ^ <"] },
    Species { key: "rabbit", name_pt: "coelho", art: &[" (\\(\\", " (-.-)", "o_(\")(\")"] },
    Species { key: "owl", name_pt: "coruja", art: &["  ,___,", "  (o,o)", "  (\")_(\")"] },
    Species { key: "penguin", name_pt: "pinguim", art: &["  (o_o)", " /|_|\\", "  ' '"] },
    Species { key: "turtle", name_pt: "tartaruga", art: &["  _____", " /,---.\\", " \\|o o|/", "  `---`"] },
    Species { key: "snail", name_pt: "caracol", art: &["  _@/", " /o \\", "(____)"] },
    Species { key: "dragon", name_pt: "dragao", art: &["  /\\  /\\", " ((oo))~~", "  \\  /"] },
    Species { key: "octopus", name_pt: "polvo", art: &[" .-\"\"-.", "/ o  o \\", "\\_/\\/\\_/", " ' ' ' '"] },
    Species { key: "axolotl", name_pt: "axolote", art: &[" ^  ^", "(o..o)", " \\__/~~"] },
    Species { key: "ghost", name_pt: "fantasma", art: &[" .-.", "(o o)", " |=| ", " ~~~"] },
    Species { key: "robot", name_pt: "robo", art: &["[o_o]", "/|_|\\", " d b"] },
    Species { key: "blob", name_pt: "geleia", art: &[" .-\"-.", "( o o )", " \\___/ "] },
    Species { key: "cactus", name_pt: "cacto", art: &[" \\|||/", "(o   o)", " |||||"] },
    Species { key: "mushroom", name_pt: "cogumelo", art: &["  ___", " /...\\", "  | |"] },
    Species { key: "chonk", name_pt: "gordinho", art: &[" /\\_/\\", "( o.o )~", "(\")_(\") big"] },
    Species { key: "capybara", name_pt: "capivara", art: &["______", "(o.o  )", "(_____)"] },
];

#[derive(Clone, Debug)]
pub struct Buddy {
    pub species: &'static str,
    pub name_pt: &'static str,
    pub rarity: Rarity,
    pub stats: BTreeMap<String, u8>,
}

fn hash_seed(seed: &str, salt: u64) -> u64 {
    let mut hasher = DefaultHasher::new();
    seed.hash(&mut hasher);
    salt.hash(&mut hasher);
    hasher.finish()
}

/// Deterministic hatch: same seed always yields the same species, rarity and stats.
pub fn hatch(seed: &str) -> Buddy {
    let base = hash_seed(seed, 0);
    let species_idx = (base % SPECIES.len() as u64) as usize;
    let species = &SPECIES[species_idx];

    let rarity_roll = ((base >> 8) % 100) as u8;
    let rarity = Rarity::from_roll(rarity_roll);

    let mut stats = BTreeMap::new();
    for (i, key) in ["curiosidade", "energia", "fome", "humor"].iter().enumerate() {
        let h = hash_seed(seed, i as u64 + 1);
        let value = 30 + (h % 66) as u8; // 30-95
        stats.insert(key.to_string(), value);
    }

    Buddy { species: species.key, name_pt: species.name_pt, rarity, stats }
}

impl Buddy {
    /// Pet the buddy: humor and energia rise (capped 100), fome drops. Returns a short PT reaction.
    pub fn pet(&mut self) -> String {
        bump(&mut self.stats, "humor", 15);
        bump(&mut self.stats, "energia", 5);
        drop_stat(&mut self.stats, "fome", 10);

        match self.species {
            "cat" => "ronrona".into(),
            "duck" | "goose" => "grasna feliz".into(),
            "rabbit" => "pula de alegria".into(),
            "owl" => "pisca devagar".into(),
            "penguin" => "bate as asinhas".into(),
            "turtle" | "snail" => "esconde a cabeca, contente".into(),
            "dragon" => "solta uma fumacinha".into(),
            "octopus" => "muda de cor".into(),
            "axolotl" => "sorri (sempre sorri)".into(),
            "ghost" => "flutua mais rapido".into(),
            "robot" => "bip bip feliz".into(),
            "blob" => "treme todo".into(),
            "cactus" => "balanca os espinhos".into(),
            "mushroom" => "solta esporinhos".into(),
            "chonk" => "ronca satisfeito".into(),
            "capybara" => "fica ainda mais tranquilo".into(),
            _ => "reage com carinho".into(),
        }
    }

    /// ASCII art + species/rarity line + stat bars, ready to push into chat.
    pub fn render(&self) -> Vec<String> {
        let species = SPECIES.iter().find(|s| s.key == self.species).expect("known species");
        let mut lines: Vec<String> = species.art.iter().map(|l| l.to_string()).collect();
        lines.push(format!("{} · {}", species.name_pt, self.rarity.label()));
        for (key, value) in &self.stats {
            lines.push(format!("{:<12} {} {}", key, bar(*value), value));
        }
        lines
    }
}

fn bump(stats: &mut BTreeMap<String, u8>, key: &str, amount: u8) {
    if let Some(v) = stats.get_mut(key) {
        *v = v.saturating_add(amount).min(100);
    }
}

fn drop_stat(stats: &mut BTreeMap<String, u8>, key: &str, amount: u8) {
    if let Some(v) = stats.get_mut(key) {
        *v = v.saturating_sub(amount);
    }
}

fn bar(value: u8) -> String {
    let filled = (value as usize * 10 / 100).min(10);
    format!("{}{}", "█".repeat(filled), "░".repeat(10 - filled))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hatch_is_deterministic() {
        let a = hatch("alex");
        let b = hatch("alex");
        assert_eq!(a.species, b.species);
        assert_eq!(a.rarity as u8, b.rarity as u8);
        assert_eq!(a.stats, b.stats);
    }

    #[test]
    fn hatch_varies_by_seed() {
        let a = hatch("alex");
        let b = hatch("someone-else-entirely");
        // not a strict guarantee for arbitrary seeds, but true for these two
        assert!(a.species != b.species || a.stats != b.stats);
    }

    #[test]
    fn pet_changes_humor() {
        let mut buddy = hatch("regente");
        let before = *buddy.stats.get("humor").unwrap();
        let reaction = buddy.pet();
        let after = *buddy.stats.get("humor").unwrap();
        assert!(after >= before);
        assert!(!reaction.is_empty());
    }

    #[test]
    fn render_has_art_and_stats() {
        let buddy = hatch("regente");
        let lines = buddy.render();
        assert!(lines.len() > 4);
        assert!(lines.iter().any(|l| l.contains(buddy.rarity.label())));
    }
}
