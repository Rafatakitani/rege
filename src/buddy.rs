// Buddy portado de github.com/ramarivera/claude-buddy (MIT, (c) ramarivera)
//!
//! Tamagotchi de terminal: hatch(seed) deterministico -> especie/raridade/stats
//! fixos para o mesmo seed. Algoritmo de geracao (rarity roll, stat floor/peak/
//! dump) segue `server/engine.ts` do repo original; o hash usa `DefaultHasher`
//! (a wyhash original e especifica de runtime JS/Bun e nao precisa ser
//! bit-exata — so precisamos de determinismo estavel dentro do Regente).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

const SALT: &str = "friend-2026-401";

// ─── Rarity ─────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rarity {
    Common,
    Uncommon,
    Rare,
    Epic,
    Legendary,
}

const RARITY_WEIGHTS: [(Rarity, f64); 5] = [
    (Rarity::Common, 60.0),
    (Rarity::Uncommon, 25.0),
    (Rarity::Rare, 10.0),
    (Rarity::Epic, 4.0),
    (Rarity::Legendary, 1.0),
];

impl Rarity {
    pub fn label(self) -> &'static str {
        match self {
            Rarity::Common => "Common",
            Rarity::Uncommon => "Uncommon",
            Rarity::Rare => "Rare",
            Rarity::Epic => "Epic",
            Rarity::Legendary => "Legendary",
        }
    }

    pub fn stars(self) -> &'static str {
        match self {
            Rarity::Common => "\u{2605}",
            Rarity::Uncommon => "\u{2605}\u{2605}",
            Rarity::Rare => "\u{2605}\u{2605}\u{2605}",
            Rarity::Epic => "\u{2605}\u{2605}\u{2605}\u{2605}",
            Rarity::Legendary => "\u{2605}\u{2605}\u{2605}\u{2605}\u{2605}",
        }
    }

    /// Stat floor for this rarity (`RARITY_FLOOR` in the original).
    fn floor(self) -> i32 {
        match self {
            Rarity::Common => 5,
            Rarity::Uncommon => 15,
            Rarity::Rare => 25,
            Rarity::Epic => 35,
            Rarity::Legendary => 50,
        }
    }
}

// ─── Species ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Species {
    Duck,
    Goose,
    Blob,
    Cat,
    Dragon,
    Octopus,
    Owl,
    Penguin,
    Turtle,
    Snail,
    Ghost,
    Axolotl,
    Capybara,
    Cactus,
    Robot,
    Rabbit,
    Mushroom,
    Chonk,
    Wyvern,
}

const ALL_SPECIES: [Species; 19] = [
    Species::Duck,
    Species::Goose,
    Species::Blob,
    Species::Cat,
    Species::Dragon,
    Species::Octopus,
    Species::Owl,
    Species::Penguin,
    Species::Turtle,
    Species::Snail,
    Species::Ghost,
    Species::Axolotl,
    Species::Capybara,
    Species::Cactus,
    Species::Robot,
    Species::Rabbit,
    Species::Mushroom,
    Species::Chonk,
    Species::Wyvern,
];

impl Species {
    pub fn name(self) -> &'static str {
        match self {
            Species::Duck => "duck",
            Species::Goose => "goose",
            Species::Blob => "blob",
            Species::Cat => "cat",
            Species::Dragon => "dragon",
            Species::Octopus => "octopus",
            Species::Owl => "owl",
            Species::Penguin => "penguin",
            Species::Turtle => "turtle",
            Species::Snail => "snail",
            Species::Ghost => "ghost",
            Species::Axolotl => "axolotl",
            Species::Capybara => "capybara",
            Species::Cactus => "cactus",
            Species::Robot => "robot",
            Species::Rabbit => "rabbit",
            Species::Mushroom => "mushroom",
            Species::Chonk => "chonk",
            Species::Wyvern => "wyvern",
        }
    }

    /// 3 idle animation frames, verbatim from `SPECIES_ART` in art.ts.
    /// `{E}` is the eye placeholder, substituted at hatch time.
    fn frames(self) -> &'static [&'static [&'static str]] {
        match self {
            Species::Duck => &[
                &["            ", "    __      ", "  <({E} )___  ", "   (  ._>   ", "    `--'    "],
                &["            ", "    __      ", "  <({E} )___  ", "   (  ._>   ", "    `--'~   "],
                &["            ", "    __      ", "  <({E} )___  ", "   (  .__>  ", "    `--'    "],
            ],
            Species::Goose => &[
                &["            ", "     ({E}>    ", "     ||     ", "   _(__)_   ", "    ^^^^    "],
                &["            ", "    ({E}>     ", "     ||     ", "   _(__)_   ", "    ^^^^    "],
                &["            ", "     ({E}>>   ", "     ||     ", "   _(__)_   ", "    ^^^^    "],
            ],
            Species::Blob => &[
                &["            ", "   .----.   ", "  ( {E}  {E} )  ", "  (      )  ", "   `----'   "],
                &["            ", "  .------.  ", " (  {E}  {E}  ) ", " (        ) ", "  `------'  "],
                &["            ", "    .--.    ", "   ({E}  {E})   ", "   (    )   ", "    `--'    "],
            ],
            Species::Cat => &[
                &["            ", "   /\\_/\\    ", "  ( {E}   {E})  ", "  (  \u{3c9}  )   ", "  (\")_(\")   "],
                &["            ", "   /\\_/\\    ", "  ( {E}   {E})  ", "  (  \u{3c9}  )   ", "  (\")_(\")~  "],
                &["            ", "   /\\-/\\    ", "  ( {E}   {E})  ", "  (  \u{3c9}  )   ", "  (\")_(\")   "],
            ],
            Species::Dragon => &[
                &["            ", "  /^\\  /^\\  ", " <  {E}  {E}  > ", " (   ~~   ) ", "  `-vvvv-'  "],
                &["            ", "  /^\\  /^\\  ", " <  {E}  {E}  > ", " (        ) ", "  `-vvvv-'  "],
                &["   ~    ~   ", "  /^\\  /^\\  ", " <  {E}  {E}  > ", " (   ~~   ) ", "  `-vvvv-'  "],
            ],
            Species::Octopus => &[
                &["            ", "   .----.   ", "  ( {E}  {E} )  ", "  (______)  ", "  /\\/\\/\\/\\  "],
                &["            ", "   .----.   ", "  ( {E}  {E} )  ", "  (______)  ", "  \\/\\/\\/\\/  "],
                &["     o      ", "   .----.   ", "  ( {E}  {E} )  ", "  (______)  ", "  /\\/\\/\\/\\  "],
            ],
            Species::Owl => &[
                &["            ", "   /\\  /\\   ", "  (({E})({E}))  ", "  (  ><  )  ", "   `----'   "],
                &["            ", "   /\\  /\\   ", "  (({E})({E}))  ", "  (  ><  )  ", "   .----.   "],
                &["            ", "   /\\  /\\   ", "  (({E})(-))  ", "  (  ><  )  ", "   `----'   "],
            ],
            Species::Penguin => &[
                &["            ", "  .---.     ", "  ({E}>{E})     ", " /(   )\\    ", "  `---'     "],
                &["            ", "  .---.     ", "  ({E}>{E})     ", " |(   )|    ", "  `---'     "],
                &["  .---.     ", "  ({E}>{E})     ", " /(   )\\    ", "  `---'     ", "   ~ ~      "],
            ],
            Species::Turtle => &[
                &["            ", "   _,--._   ", "  ( {E}  {E} )  ", " /[______]\\ ", "  ``    ``  "],
                &["            ", "   _,--._   ", "  ( {E}  {E} )  ", " /[______]\\ ", "   ``  ``   "],
                &["            ", "   _,--._   ", "  ( {E}  {E} )  ", " /[======]\\ ", "  ``    ``  "],
            ],
            Species::Snail => &[
                &["            ", " {E}    .--.  ", "  \\  ( @ )  ", "   \\_`--'   ", "  ~~~~~~~   "],
                &["            ", "  {E}   .--.  ", "  |  ( @ )  ", "   \\_`--'   ", "  ~~~~~~~   "],
                &["            ", " {E}    .--.  ", "  \\  ( @  ) ", "   \\_`--'   ", "   ~~~~~~   "],
            ],
            Species::Ghost => &[
                &["            ", "   .----.   ", "  / {E}  {E} \\  ", "  |      |  ", "  ~`~``~`~  "],
                &["            ", "   .----.   ", "  / {E}  {E} \\  ", "  |      |  ", "  `~`~~`~`  "],
                &["    ~  ~    ", "   .----.   ", "  / {E}  {E} \\  ", "  |      |  ", "  ~~`~~`~~  "],
            ],
            Species::Axolotl => &[
                &["            ", "}~(______)~{", "}~({E} .. {E})~{", "  ( .--. )  ", "  (_/  \\_)  "],
                &["            ", "~}(______){~", "~}({E} .. {E}){~", "  ( .--. )  ", "  (_/  \\_)  "],
                &["            ", "}~(______)~{", "}~({E} .. {E})~{", "  (  --  )  ", "  ~_/  \\_~  "],
            ],
            Species::Capybara => &[
                &["            ", "  n______n  ", " ( {E}    {E} ) ", " (   oo   ) ", "  `------'  "],
                &["            ", "  n______n  ", " ( {E}    {E} ) ", " (   Oo   ) ", "  `------'  "],
                &["    ~  ~    ", "  u______n  ", " ( {E}    {E} ) ", " (   oo   ) ", "  `------'  "],
            ],
            Species::Cactus => &[
                &["            ", " n  ____  n ", " | |{E}  {E}| | ", " |_|    |_| ", "   |    |   "],
                &["            ", "    ____    ", " n |{E}  {E}| n ", " |_|    |_| ", "   |    |   "],
                &[" n        n ", " |  ____  | ", " | |{E}  {E}| | ", " |_|    |_| ", "   |    |   "],
            ],
            Species::Robot => &[
                &["            ", "   .[||].   ", "  [ {E}  {E} ]  ", "  [ ==== ]  ", "  `------'  "],
                &["            ", "   .[||].   ", "  [ {E}  {E} ]  ", "  [ -==- ]  ", "  `------'  "],
                &["     *      ", "   .[||].   ", "  [ {E}  {E} ]  ", "  [ ==== ]  ", "  `------'  "],
            ],
            Species::Rabbit => &[
                &["            ", "   (\\__/)   ", "  ( {E}  {E} )  ", " =(  ..  )= ", "  (\")__(\")"],
                &["            ", "   (|__/)   ", "  ( {E}  {E} )  ", " =(  ..  )= ", "  (\")__(\")"],
                &["            ", "   (\\__/)   ", "  ( {E}  {E} )  ", " =( .  . )= ", "  (\")__(\")"],
            ],
            Species::Mushroom => &[
                &["            ", " .-o-OO-o-. ", "(__________)", "   |{E}  {E}|   ", "   |____|   "],
                &["            ", " .-O-oo-O-. ", "(__________)", "   |{E}  {E}|   ", "   |____|   "],
                &["   . o  .   ", " .-o-OO-o-. ", "(__________)", "   |{E}  {E}|   ", "   |____|   "],
            ],
            Species::Chonk => &[
                &["            ", "  /\\    /\\  ", " ( {E}    {E} ) ", " (   ..   ) ", "  `------'  "],
                &["            ", "  /\\    /|  ", " ( {E}    {E} ) ", " (   ..   ) ", "  `------'  "],
                &["            ", "  /\\    /\\  ", " ( {E}    {E} ) ", " (   ..   ) ", "  `------'~ "],
            ],
            Species::Wyvern => &[
                &["}       {", "|\\^```^/|", "\\ {E}' '{E} /", " \\ } { /", " \u{2248}(\u{b0} \u{b0})\u{2248}", "   '-'"],
                &["}       {", "|\\^```^/|", "\\ {E}' '{E} /", " \\ } { /", " \u{2248}(\u{b0} \u{b0})\u{2248}", "  \x1b[38;2;255;120;0m//|\\\\\x1b[0m"],
                &["}       {", "|\\^```^/|", "\\ {E}' '{E} /", " \\ } { /", " \u{2248}(\u{b0} \u{b0})\u{2248}", "   'v'"],
            ],
        }
    }
}

// ─── Stats ──────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum StatName {
    Debugging,
    Patience,
    Chaos,
    Wisdom,
    Snark,
}

const STAT_ORDER: [StatName; 5] =
    [StatName::Debugging, StatName::Patience, StatName::Chaos, StatName::Wisdom, StatName::Snark];

impl StatName {
    fn label(self) -> &'static str {
        match self {
            StatName::Debugging => "DEBUGGING",
            StatName::Patience => "PATIENCE",
            StatName::Chaos => "CHAOS",
            StatName::Wisdom => "WISDOM",
            StatName::Snark => "SNARK",
        }
    }
}

const EYES: [char; 6] = ['\u{b7}', '\u{2726}', '\u{d7}', '\u{25c9}', '@', '\u{b0}'];

// Reactions verbatim (subset) from `server/reactions.ts`'s `pet` reaction pool.
const PET_REACTIONS: [&str; 6] = [
    "*purrs contentedly*",
    "*happy noises*",
    "*nuzzles your cursor*",
    "*wiggles*",
    "again! again!",
    "*closes eyes peacefully*",
];

// Frame index sequence per tick, matching art.ts's `STATUS_FRAME_SEQUENCE`
// (index 3 is the pre-baked blink frame).
const FRAME_SEQUENCE: [usize; 15] = [0, 0, 0, 0, 1, 0, 0, 0, 3, 0, 0, 2, 0, 0, 0];

// ─── Hash + mulberry32 PRNG ─────────────────────────────────────────────────
// Mirrors `mulberry32` in engine.ts bit-for-bit; only the seed hash differs
// (DefaultHasher instead of wyhash — the original picks wyhash so JS/Bun
// runtimes agree with each other, which doesn't apply here).

fn hash_seed(s: &str) -> u32 {
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish() as u32
}

fn mulberry32(seed: u32) -> impl FnMut() -> f64 {
    let mut a = seed;
    move || {
        a = a.wrapping_add(0x6D2B79F5);
        let mut t = (a ^ (a >> 15)).wrapping_mul(1 | a);
        t = (t.wrapping_add((t ^ (t >> 7)).wrapping_mul(61 | t))) ^ t;
        ((t ^ (t >> 14)) as f64) / 4294967296.0
    }
}

fn pick<T: Copy>(rng: &mut impl FnMut() -> f64, arr: &[T]) -> T {
    arr[(rng() * arr.len() as f64) as usize]
}

fn roll_rarity(rng: &mut impl FnMut() -> f64) -> Rarity {
    let total: f64 = RARITY_WEIGHTS.iter().map(|(_, w)| w).sum();
    let mut roll = rng() * total;
    for (r, w) in RARITY_WEIGHTS {
        roll -= w;
        if roll < 0.0 {
            return r;
        }
    }
    Rarity::Common
}

// ─── Buddy ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Buddy {
    pub species: Species,
    pub rarity: Rarity,
    pub debugging: u8,
    pub patience: u8,
    pub chaos: u8,
    pub wisdom: u8,
    pub snark: u8,
    /// 4 pre-resolved frames (eye baked in): idle 0, idle 1, idle 2, blink.
    pub frames: Vec<Vec<String>>,
    pets: u32,
}

impl Buddy {
    /// Deterministic hatch: same seed always yields the same species, rarity
    /// and stats. Mirrors `generateBones` in engine.ts.
    pub fn hatch(seed: &str) -> Buddy {
        let mut rng = mulberry32(hash_seed(&format!("{seed}{SALT}")));

        let rarity = roll_rarity(&mut rng);
        let species = pick(&mut rng, &ALL_SPECIES);
        let eye = pick(&mut rng, &EYES);

        let peak = pick(&mut rng, &STAT_ORDER);
        let mut dump = pick(&mut rng, &STAT_ORDER);
        while dump == peak {
            dump = pick(&mut rng, &STAT_ORDER);
        }

        let floor = rarity.floor();
        let mut vals = [0i32; 5];
        for (i, name) in STAT_ORDER.iter().enumerate() {
            vals[i] = if *name == peak {
                (floor + 50 + (rng() * 30.0) as i32).min(100)
            } else if *name == dump {
                (floor - 10 + (rng() * 15.0) as i32).max(1)
            } else {
                floor + (rng() * 40.0) as i32
            };
        }

        let raw = species.frames();
        let bake = |frame: &[&str], eye: char| -> Vec<String> {
            frame.iter().map(|line| line.replace("{E}", &eye.to_string())).collect()
        };
        let frames = vec![
            bake(raw[0], eye),
            bake(raw[1], eye),
            bake(raw[2], eye),
            bake(raw[0], '-'),
        ];

        Buddy {
            species,
            rarity,
            debugging: vals[0] as u8,
            patience: vals[1] as u8,
            chaos: vals[2] as u8,
            wisdom: vals[3] as u8,
            snark: vals[4] as u8,
            frames,
            pets: 0,
        }
    }

    /// Pet the buddy: nudges patience up, chaos down. Returns a short reaction.
    pub fn pet(&mut self) -> String {
        self.patience = self.patience.saturating_add(3).min(100);
        self.chaos = self.chaos.saturating_sub(2).max(1);
        let reaction = PET_REACTIONS[(self.pets as usize) % PET_REACTIONS.len()];
        self.pets += 1;
        reaction.to_string()
    }

    /// Animation frame for a given tick, following the original 15-tick cycle
    /// (mostly idle, occasional blink/variant frames).
    pub fn frame(&self, tick: usize) -> &Vec<String> {
        let idx = FRAME_SEQUENCE[tick % FRAME_SEQUENCE.len()];
        &self.frames[idx]
    }

    fn stat_bar(&self, name: StatName) -> String {
        let val = match name {
            StatName::Debugging => self.debugging,
            StatName::Patience => self.patience,
            StatName::Chaos => self.chaos,
            StatName::Wisdom => self.wisdom,
            StatName::Snark => self.snark,
        };
        let filled = ((val as usize) * 10 / 100).min(10);
        let bar = format!("{}{}", "\u{2588}".repeat(filled), "\u{2591}".repeat(10 - filled));
        format!("{:<3} {} {:>3}", &name.label()[..3], bar, val)
    }

    /// Art frame (for the given tick) + species/rarity header + 5 compact stat
    /// bars, ready to embed in a chat message or floating widget.
    pub fn render_lines(&self, tick: usize) -> Vec<String> {
        let mut lines: Vec<String> =
            self.frame(tick).iter().filter(|l| !l.trim().is_empty()).cloned().collect();
        lines.push(format!("{} \u{b7} {} {}", self.species.name(), self.rarity.label(), self.rarity.stars()));
        for name in STAT_ORDER {
            lines.push(self.stat_bar(name));
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hatch_is_deterministic() {
        let a = Buddy::hatch("alex");
        let b = Buddy::hatch("alex");
        assert_eq!(a.species, b.species);
        assert_eq!(a.rarity, b.rarity);
        assert_eq!(a.debugging, b.debugging);
        assert_eq!(a.patience, b.patience);
        assert_eq!(a.chaos, b.chaos);
        assert_eq!(a.wisdom, b.wisdom);
        assert_eq!(a.snark, b.snark);
        assert_eq!(a.frames, b.frames);
    }

    #[test]
    fn hatch_varies_by_seed() {
        let a = Buddy::hatch("alex");
        let b = Buddy::hatch("someone-else-entirely");
        assert!(
            a.species != b.species
                || a.rarity != b.rarity
                || (a.debugging, a.patience, a.chaos, a.wisdom, a.snark)
                    != (b.debugging, b.patience, b.chaos, b.wisdom, b.snark)
        );
    }

    #[test]
    fn stats_stay_in_bounds() {
        for seed in ["a", "b", "c", "regente", "zzz", "1234"] {
            let buddy = Buddy::hatch(seed);
            for v in [buddy.debugging, buddy.patience, buddy.chaos, buddy.wisdom, buddy.snark] {
                assert!(v >= 1);
                assert!(v <= 100);
            }
        }
    }

    #[test]
    fn pet_changes_reaction_and_stats() {
        let mut buddy = Buddy::hatch("regente");
        let before = buddy.patience;
        let reaction = buddy.pet();
        assert!(!reaction.is_empty());
        assert!(buddy.patience >= before);
    }

    #[test]
    fn pet_cycles_through_reactions() {
        let mut buddy = Buddy::hatch("regente");
        let mut seen = std::collections::HashSet::new();
        for _ in 0..PET_REACTIONS.len() {
            seen.insert(buddy.pet());
        }
        assert_eq!(seen.len(), PET_REACTIONS.len());
    }

    #[test]
    fn frame_follows_status_sequence() {
        let buddy = Buddy::hatch("regente");
        assert_eq!(buddy.frame(0), &buddy.frames[0]);
        assert_eq!(buddy.frame(4), &buddy.frames[1]);
        assert_eq!(buddy.frame(8), &buddy.frames[3]);
        assert_eq!(buddy.frame(11), &buddy.frames[2]);
    }

    #[test]
    fn render_lines_has_art_header_and_five_stats() {
        let buddy = Buddy::hatch("regente");
        let lines = buddy.render_lines(0);
        assert!(lines.iter().any(|l| l.contains(buddy.rarity.label())));
        assert!(lines.iter().any(|l| l.contains("DEB")));
        assert!(lines.iter().any(|l| l.contains("PAT")));
        assert!(lines.iter().any(|l| l.contains("CHA")));
        assert!(lines.iter().any(|l| l.contains("WIS")));
        assert!(lines.iter().any(|l| l.contains("SNA")));
    }

    #[test]
    fn all_species_have_three_frames_of_five_or_more_lines() {
        for species in ALL_SPECIES {
            let frames = species.frames();
            assert_eq!(frames.len(), 3);
            for f in frames {
                assert!(f.len() >= 5);
            }
        }
    }
}
