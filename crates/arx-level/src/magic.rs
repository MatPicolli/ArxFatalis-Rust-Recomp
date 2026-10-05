//! Magic: the twenty runes, how a rune drawn in the air is recognised, which runes make which spell, what a spell
//! costs, and what the first spells do.
//!
//! **Recognition is not the original's.** The original turns the stroke into a list of the eight compass directions
//! and wants that list to equal the rune's exactly, which fails on a wobble or a rounded corner. Here the stroke is
//! compared with every rune's ideal shape as a whole: both are resampled to the same number of points evenly spaced
//! along the path, centred and scaled to the same size, and the rune whose points and whose direction of travel lie
//! closest wins, if it is close enough and clearly closer than the runner-up. The ideal shapes are built from the
//! original's direction lists, so every rune is still drawn the way the game's rune stones show it.
//!
//! The spell table, mana costs, levels and the numbers of the effects are the engine's (`SpellRecognition.cpp`,
//! `Spells.cpp`, `spells/SpellsLvl*.cpp`).

use arx_script::{EntityId, PlayerState, ScriptWorld, Skill, StdHost};
use glam::Vec2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rune {
    Aam,
    Cetrius,
    Comunicatum,
    Cosum,
    Folgora,
    Fridd,
    Kaom,
    Mega,
    Morte,
    Movis,
    Nhi,
    Rhaa,
    Spacium,
    Stregum,
    Taar,
    Tempus,
    Tera,
    Vista,
    Vitae,
    Yok,
}

use Rune::*;

impl Rune {
    pub const ALL: [Rune; 20] = [Aam, Cetrius, Comunicatum, Cosum, Folgora, Fridd, Kaom, Mega, Morte, Movis, Nhi, Rhaa, Spacium, Stregum, Taar, Tempus, Tera, Vista, Vitae, Yok];

    /// Its name in scripts (`rune -a aam`) and in the rune stones' file names.
    pub fn name(self) -> &'static str {
        match self {
            Aam => "aam",
            Cetrius => "cetrius",
            Comunicatum => "comunicatum",
            Cosum => "cosum",
            Folgora => "folgora",
            Fridd => "fridd",
            Kaom => "kaom",
            Mega => "mega",
            Morte => "morte",
            Movis => "movis",
            Nhi => "nhi",
            Rhaa => "rhaa",
            Spacium => "spacium",
            Stregum => "stregum",
            Taar => "taar",
            Tempus => "tempus",
            Tera => "tera",
            Vista => "vista",
            Vitae => "vitae",
            Yok => "yok",
        }
    }

    pub fn from_name(name: &str) -> Option<Rune> {
        let name = name.trim().to_ascii_lowercase();
        // (Some scripts and sound files spell it "citrius".)
        let name = if name == "citrius" { "cetrius" } else { name.as_str() };
        Rune::ALL.into_iter().find(|r| r.name() == name)
    }

    /// The voice that names the rune as it is drawn (`sfx/<this>.wav`).
    pub fn sound(self) -> &'static str {
        match self {
            Aam => "magic_aam",
            Cetrius => "magic_citrius",
            Comunicatum => "magic_comunicatum",
            Cosum => "magic_cosum",
            Folgora => "magic_folgora",
            Fridd => "magic_fridd",
            Kaom => "magic_kaom",
            Mega => "magic_mega",
            Morte => "magic_morte",
            Movis => "magic_movis",
            Nhi => "magic_nhi",
            Rhaa => "magic_rhaa",
            Spacium => "magic_spacium",
            Stregum => "magic_stregum",
            Taar => "magic_taar",
            Tempus => "magic_tempus",
            Tera => "magic_tera",
            Vista => "magic_vista",
            Vitae => "magic_vitae",
            Yok => "magic_yok",
        }
    }

    /// How it is drawn: the strokes as keypad directions (8 up, 2 down, 4 left, 6 right, 9/3/1/7 the diagonals), one
    /// string per accepted way of drawing it.
    pub fn strokes(self) -> &'static [&'static str] {
        match self {
            Aam => &["6"],
            Cetrius => &["386"],
            Comunicatum => &["62426"],
            Cosum => &["6248"],
            Folgora => &["93"],
            Fridd => &["862"],
            Kaom => &["41236", "1236"],
            Mega => &["8"],
            Morte => &["62"],
            Movis => &["616"],
            Nhi => &["4"],
            Rhaa => &["2"],
            Spacium => &["4268"],
            Stregum => &["838"],
            Taar => &["626"],
            Tempus => &["862686"],
            Tera => &["926"],
            Vista => &["31"],
            Vitae => &["68"],
            Yok => &["268"],
        }
    }
}

// ------------------------------------------------------------------------------------------------ recognition

/// Points a stroke is reduced to.
const SAMPLES: usize = 48;
/// A stroke shorter than this many pixels is a click, not a rune.
const MIN_LENGTH: f32 = 30.0;
/// The most a stroke may differ from a rune's shape and still be it, and by how much it must beat the next rune.
const ACCEPT: f32 = 0.24;
const MARGIN: f32 = 1.12;

fn direction(digit: u8) -> Vec2 {
    // Screen axes: x to the right, y down.
    let d = match digit {
        b'8' => Vec2::new(0.0, -1.0),
        b'2' => Vec2::new(0.0, 1.0),
        b'4' => Vec2::new(-1.0, 0.0),
        b'6' => Vec2::new(1.0, 0.0),
        b'9' => Vec2::new(1.0, -1.0),
        b'3' => Vec2::new(1.0, 1.0),
        b'7' => Vec2::new(-1.0, -1.0),
        b'1' => Vec2::new(-1.0, 1.0),
        _ => Vec2::ZERO,
    };
    d.normalize_or_zero()
}

/// The corners of a rune drawn exactly: one unit along each direction in turn.
pub fn ideal_path(strokes: &str) -> Vec<Vec2> {
    let mut at = Vec2::ZERO;
    let mut out = vec![at];
    for d in strokes.bytes() {
        at += direction(d);
        out.push(at);
    }
    out
}

/// `SAMPLES` points evenly spaced along the path, centred on their middle and scaled so the longer side of their
/// bounding box is 1. `None` if the path is shorter than `min_length`.
fn shape(points: &[Vec2], min_length: f32) -> Option<Vec<Vec2>> {
    let length: f32 = points.windows(2).map(|w| w[0].distance(w[1])).sum();
    if points.len() < 2 || length < min_length || length <= 0.0 {
        return None;
    }
    let step = length / (SAMPLES - 1) as f32;
    let mut out = Vec::with_capacity(SAMPLES);
    out.push(points[0]);
    let (mut carried, mut prev) = (0.0, points[0]);
    for &p in &points[1..] {
        let mut seg = prev.distance(p);
        while carried + seg >= step && out.len() < SAMPLES {
            let t = (step - carried) / seg;
            prev = prev.lerp(p, t);
            out.push(prev);
            seg = prev.distance(p);
            carried = 0.0;
        }
        carried += seg;
        prev = p;
    }
    while out.len() < SAMPLES {
        out.push(*points.last().expect("at least two points"));
    }
    let centre = out.iter().copied().sum::<Vec2>() / SAMPLES as f32;
    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for p in &out {
        lo = lo.min(*p);
        hi = hi.max(*p);
    }
    let size = (hi - lo).max_element().max(1e-6);
    Some(out.into_iter().map(|p| (p - centre) / size).collect())
}

/// The length of a shape's path (in units of its longer side).
fn path_length(s: &[Vec2]) -> f32 {
    s.windows(2).map(|w| w[0].distance(w[1])).sum()
}

/// How far apart two shapes are: the mean distance between matching points, plus how much the direction of travel
/// differs along the way (which is what tells a stroke to the right from one to the left, and a corner from a curve),
/// plus how much longer or shorter one path is than the other (a zigzag across the screen is not a straight line).
fn difference(a: &[Vec2], b: &[Vec2]) -> f32 {
    let n = a.len() as f32;
    let apart: f32 = a.iter().zip(b).map(|(p, q)| p.distance(*q)).sum::<f32>() / n;
    let heading = |s: &[Vec2], i: usize| (s[(i + 2).min(s.len() - 1)] - s[i.saturating_sub(2)]).normalize_or_zero();
    let turned: f32 = (0..a.len()).map(|i| 1.0 - heading(a, i).dot(heading(b, i))).sum::<f32>() / n;
    let longer = (path_length(a) / path_length(b).max(1e-6)).ln().abs();
    apart + 0.22 * turned + 0.3 * longer
}

/// A rune's shape is tried with its strokes at a few different proportions, since nobody draws every stroke the same
/// length.
fn templates() -> &'static [(Rune, Vec<Vec2>)] {
    static T: std::sync::OnceLock<Vec<(Rune, Vec<Vec2>)>> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let mut all = Vec::new();
        for rune in Rune::ALL {
            for strokes in rune.strokes() {
                let dirs: Vec<Vec2> = strokes.bytes().map(direction).collect();
                // Every stroke the same length, then each stroke in turn longer and shorter than the rest.
                let mut weights: Vec<Vec<f32>> = vec![vec![1.0; dirs.len()]];
                if dirs.len() > 1 {
                    for i in 0..dirs.len() {
                        for w in [0.6, 1.6] {
                            let mut v = vec![1.0; dirs.len()];
                            v[i] = w;
                            weights.push(v);
                        }
                    }
                }
                for w in weights {
                    let mut at = Vec2::ZERO;
                    let mut path = vec![at];
                    for (d, len) in dirs.iter().zip(&w) {
                        at += *d * *len;
                        path.push(at);
                    }
                    if let Some(s) = shape(&path, 0.0) {
                        all.push((rune, s));
                    }
                }
            }
        }
        all
    })
}

/// What a stroke was taken for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Recognised {
    pub rune: Rune,
    /// How far the stroke is from the rune's shape (0 is exact; `ACCEPT` is the limit).
    pub difference: f32,
}

/// The rune a stroke (screen points, in the order drawn) is, if it is clearly one.
pub fn recognise(stroke: &[Vec2]) -> Option<Recognised> {
    let drawn = shape(stroke, MIN_LENGTH)?;
    // The best match of each rune.
    let mut best: Vec<(Rune, f32)> = Vec::new();
    for (rune, template) in templates() {
        let d = difference(&drawn, template);
        match best.iter_mut().find(|(r, _)| r == rune) {
            Some((_, old)) => *old = old.min(d),
            None => best.push((*rune, d)),
        }
    }
    best.sort_by(|a, b| a.1.total_cmp(&b.1));
    let (rune, d) = best[0];
    let next = best.get(1).map_or(f32::MAX, |b| b.1);
    (d <= ACCEPT && next >= d * MARGIN).then_some(Recognised { rune, difference: d })
}

// ------------------------------------------------------------------------------------------------ spells

/// A spell: its name in scripts (the `spellcast` event carries it) and the runes that cast it, in order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spell {
    pub name: &'static str,
    pub runes: &'static [Rune],
}

pub const SPELLS: &[Spell] = &[
    Spell { name: "curse", runes: &[Rhaa, Stregum, Vitae] },
    Spell { name: "freeze_time", runes: &[Rhaa, Tempus] },
    Spell { name: "lower_armor", runes: &[Rhaa, Kaom] },
    Spell { name: "slowdown", runes: &[Rhaa, Movis] },
    Spell { name: "harm", runes: &[Rhaa, Vitae] },
    Spell { name: "confuse", runes: &[Rhaa, Vista] },
    Spell { name: "mass_paralyse", runes: &[Mega, Nhi, Movis] },
    Spell { name: "armor", runes: &[Mega, Kaom] },
    Spell { name: "magic_sight", runes: &[Mega, Vista] },
    Spell { name: "heal", runes: &[Mega, Vitae] },
    Spell { name: "speed", runes: &[Mega, Movis] },
    Spell { name: "bless", runes: &[Mega, Stregum, Vitae] },
    Spell { name: "enchant_weapon", runes: &[Mega, Stregum, Cosum] },
    Spell { name: "mass_incinerate", runes: &[Mega, Aam, Mega, Yok] },
    Spell { name: "activate_portal", runes: &[Mega, Spacium] },
    Spell { name: "levitate", runes: &[Mega, Spacium, Movis] },
    Spell { name: "paralyse", runes: &[Nhi, Movis] },
    Spell { name: "cure_poison", runes: &[Nhi, Cetrius] },
    Spell { name: "douse", runes: &[Nhi, Yok] },
    Spell { name: "dispell_illusion", runes: &[Nhi, Stregum, Vista] },
    Spell { name: "negate_magic", runes: &[Nhi, Stregum, Spacium] },
    Spell { name: "dispell_field", runes: &[Nhi, Spacium] },
    Spell { name: "disarm_trap", runes: &[Nhi, Morte, Cosum] },
    Spell { name: "invisibility", runes: &[Nhi, Vista] },
    Spell { name: "flying_eye", runes: &[Vista, Movis] },
    Spell { name: "repel_undead", runes: &[Morte, Kaom] },
    Spell { name: "detect_trap", runes: &[Morte, Cosum, Vista] },
    Spell { name: "control", runes: &[Movis, Comunicatum] },
    Spell { name: "mana_drain", runes: &[Stregum, Movis] },
    Spell { name: "incinerate", runes: &[Aam, Mega, Yok] },
    Spell { name: "explosion", runes: &[Aam, Mega, Morte] },
    Spell { name: "create_field", runes: &[Aam, Kaom, Spacium] },
    Spell { name: "raise_dead", runes: &[Aam, Morte, Vitae] },
    Spell { name: "rune_of_guarding", runes: &[Aam, Morte, Cosum] },
    Spell { name: "summon_creature", runes: &[Aam, Vitae, Tera] },
    Spell { name: "create_food", runes: &[Aam, Vitae, Cosum] },
    Spell { name: "lightning_strike", runes: &[Aam, Folgora, Taar] },
    Spell { name: "mass_lightning_strike", runes: &[Aam, Folgora, Spacium] },
    Spell { name: "ignit", runes: &[Aam, Yok] },
    Spell { name: "fire_field", runes: &[Aam, Yok, Spacium] },
    Spell { name: "fireball", runes: &[Aam, Yok, Taar] },
    Spell { name: "ice_field", runes: &[Aam, Fridd, Spacium] },
    Spell { name: "ice_projectile", runes: &[Aam, Fridd, Taar] },
    Spell { name: "poison_projectile", runes: &[Aam, Cetrius, Taar] },
    Spell { name: "magic_missile", runes: &[Aam, Taar] },
    Spell { name: "fire_protection", runes: &[Yok, Kaom] },
    Spell { name: "cold_protection", runes: &[Fridd, Kaom] },
    Spell { name: "life_drain", runes: &[Vitae, Movis] },
    Spell { name: "telekinesis", runes: &[Spacium, Comunicatum] },
];

pub fn spell_for(runes: &[Rune]) -> Option<&'static Spell> {
    SPELLS.iter().find(|s| s.runes == runes)
}

/// Mana a spell costs a caster of `level` (`ARX_SPELLS_GetManaCost`).
pub fn mana_cost(spell: &str, level: f32) -> f32 {
    match spell {
        "telekinesis" | "curse" => 0.001,
        "armor" | "lower_armor" | "speed" | "bless" => 0.01,
        "detect_trap" => 0.03,
        "magic_sight" => 0.3,
        "harm" | "mana_drain" => 0.4,
        "ignit" | "douse" | "fire_protection" | "cold_protection" | "levitate" => 1.0,
        "create_field" | "slowdown" => 1.2,
        "activate_portal" | "negate_magic" => 2.0,
        "invisibility" | "life_drain" => 3.0,
        "heal" | "flying_eye" => 4.0,
        "create_food" => 5.0,
        "dispell_illusion" | "dispell_field" => 7.0,
        "rune_of_guarding" => 9.0,
        "cure_poison" => 10.0,
        "raise_dead" => 12.0,
        "disarm_trap" | "fire_field" | "ice_field" => 15.0,
        "repel_undead" => 18.0,
        "enchant_weapon" => 35.0,
        "incinerate" | "control" => 40.0,
        "explosion" => 45.0,
        "freeze_time" => 60.0,
        "mass_incinerate" => 160.0,
        "confuse" => level * 0.1,
        "magic_missile" => level,
        "ice_projectile" => level * 1.5,
        "poison_projectile" => level * 2.0,
        "fireball" | "paralyse" | "mass_paralyse" => level * 3.0,
        "lightning_strike" => level * 6.0,
        "mass_lightning_strike" => level * 8.0,
        "summon_creature" => if level < 9.0 { 20.0 } else { 80.0 },
        _ => 0.0,
    }
}

/// How strong the hero's spells are, 1 to 10: a tenth of casting skill plus mind (`spellLevel`).
pub fn spell_level(player: &PlayerState) -> f32 {
    ((player.full_skills().get(Skill::Casting) + player.attributes_full().mind) * 0.1).clamp(1.0, 10.0)
}

/// What casting did, for the application to show (and, for the spells that reach into the world, to carry out).
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Light every torch and fire within `radius` of the caster.
    Ignite { radius: f32 },
    /// Put out every torch and fire within `radius`.
    Douse { radius: f32 },
    /// `count` missiles that each do `damage`, flying where the caster looks.
    Missiles { count: u32, damage: f32 },
    /// The spell went on the caster and will run its time (see [`ActiveSpells`]).
    OnCaster,
    /// Scripts were told (the `spellcast` event); whatever else the spell does is not there yet.
    ToldOnly,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CastError {
    /// No runes were drawn.
    Nothing,
    /// The hero does not have this rune.
    RuneNotKnown(Rune),
    /// These runes are no spell.
    NoSuchSpell,
    NotEnoughMana { needs: f32, has: f32 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cast {
    pub spell: &'static Spell,
    pub level: f32,
    pub effect: Effect,
    /// The sound of the spell (`sfx/<this>.wav`).
    pub sound: &'static str,
}

/// A spell that lasts.
#[derive(Debug, Clone, PartialEq)]
struct Active {
    spell: &'static str,
    level: f32,
    left_ms: f32,
}

/// The spells running on the hero.
#[derive(Debug, Default)]
pub struct ActiveSpells {
    list: Vec<Active>,
    rng: u32,
}

impl ActiveSpells {
    fn random(&mut self) -> f32 {
        if self.rng == 0 {
            self.rng = 0x1F2E_3D4C;
        }
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1u32 << 24) as f32
    }

    pub fn is_active(&self, spell: &str) -> bool {
        self.list.iter().any(|a| a.spell == spell)
    }

    /// The names of what is running, with the seconds each has left.
    pub fn running(&self) -> Vec<(&'static str, f32)> {
        self.list.iter().map(|a| (a.spell, a.left_ms / 1000.0)).collect()
    }

    /// How much faster the hero moves (1 = as usual): `speed` adds a tenth per level.
    pub fn speed_factor(&self) -> f32 {
        1.0 + self.list.iter().filter(|a| a.spell == "speed").map(|a| a.level * 0.1).fold(0.0, f32::max)
    }

    fn start(&mut self, spell: &'static str, level: f32, secs: f32) {
        // Cast again, it starts again rather than piling up.
        self.list.retain(|a| a.spell != spell);
        self.list.push(Active { spell, level, left_ms: secs * 1000.0 });
    }

    /// Advance by `dt_ms`: healing heals, mana comes back by itself, and spells run out. Returns the spells that
    /// ended.
    pub fn update(&mut self, host: &mut StdHost, dt_ms: f32) -> Vec<&'static str> {
        let p = &mut host.player;
        if !p.is_dead() {
            // Natural recovery: 0.008 mana a second for each point of mind and ethereal link.
            let link = p.full_skills().get(Skill::EtheralLink);
            let back = 0.000_000_8 * dt_ms * (p.attributes_full().mind + link) * 10.0;
            p.mana.current = (p.mana.current + back).min(p.mana.max);
        }
        for i in 0..self.list.len() {
            self.list[i].left_ms -= dt_ms;
            if self.list[i].spell == "heal" {
                let gain = (0.8 + 1.6 * self.random()) * self.list[i].level * dt_ms * 0.001;
                let p = &mut host.player;
                if !p.is_dead() {
                    p.life.current = (p.life.current + gain).min(p.life.max);
                }
            }
        }
        let ended: Vec<&'static str> = self.list.iter().filter(|a| a.left_ms <= 0.0).map(|a| a.spell).collect();
        self.list.retain(|a| a.left_ms > 0.0);
        host.player.spell_armor = self.list.iter().filter(|a| a.spell == "armor").map(|a| a.level).fold(0.0, f32::max)
            - self.list.iter().filter(|a| a.spell == "lower_armor").map(|a| a.level).fold(0.0, f32::max);
        ended
    }

    /// The hero casts the spell the runes make: checks that the hero has the runes and the mana, takes the mana,
    /// tells every script (`spellcast` with the spell's name and level), and starts what the spell does.
    pub fn cast(&mut self, world: &mut ScriptWorld, host: &mut StdHost, caster: EntityId, runes: &[Rune]) -> Result<Cast, CastError> {
        if runes.is_empty() {
            return Err(CastError::Nothing);
        }
        if let Some(missing) = runes.iter().find(|r| !host.player.runes.contains(r.name())) {
            return Err(CastError::RuneNotKnown(*missing));
        }
        let spell = spell_for(runes).ok_or(CastError::NoSuchSpell)?;
        let level = spell_level(&host.player);
        let needs = mana_cost(spell.name, level);
        if host.player.mana.current < needs {
            return Err(CastError::NotEnoughMana { needs, has: host.player.mana.current });
        }
        host.player.mana.current -= needs;
        for id in 0..world.entities.len() as EntityId {
            world.queue_event(Some(caster), id, "spellcast", vec![spell.name.to_owned(), format!("{}", level as i64)]);
        }
        let (effect, sound) = match spell.name {
            "ignit" => (Effect::Ignite { radius: 400.0 + level * 30.0 }, "magic_spell_ignite"),
            "douse" => (Effect::Douse { radius: 400.0 + level * 30.0 }, "magic_spell_douse"),
            "magic_missile" => (Effect::Missiles { count: (((level + 1.0) / 2.0) as u32).clamp(1, 5), damage: (4.0 + level * 0.2) * 0.8 }, "magic_spell_missilelaunch"),
            "heal" => {
                self.start("heal", level, 3.5);
                (Effect::OnCaster, "magic_spell_healing")
            }
            "armor" => {
                self.start("armor", level, 20.0);
                (Effect::OnCaster, "magic_spell_armor_start")
            }
            "lower_armor" => {
                self.start("lower_armor", level, 20.0);
                (Effect::OnCaster, "magic_spell_lower_armor")
            }
            "speed" => {
                self.start("speed", level, 20.0);
                (Effect::OnCaster, "magic_spell_speedstart")
            }
            _ => (Effect::ToldOnly, "magic_spell_noeffect"),
        };
        Ok(Cast { spell, level, effect, sound })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dice(u32);
    impl Dice {
        fn next(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (self.0 >> 8) as f32 / (1u32 << 24) as f32
        }
        fn between(&mut self, lo: f32, hi: f32) -> f32 {
            lo + (hi - lo) * self.next()
        }
    }

    /// A rune drawn by a hand: strokes of uneven length, corners cut, the whole thing tilted, scaled and shaky, and
    /// sampled at uneven intervals as a mouse is.
    fn by_hand(strokes: &str, dice: &mut Dice, sloppiness: f32) -> Vec<Vec2> {
        let mut at = Vec2::ZERO;
        let mut corners = vec![at];
        for d in strokes.bytes() {
            at += direction(d) * dice.between(1.0 - 0.3 * sloppiness, 1.0 + 0.4 * sloppiness);
            corners.push(at);
        }
        let size = dice.between(80.0, 320.0);
        let tilt = Vec2::from_angle(dice.between(-0.2, 0.2) * sloppiness);
        let mut out = Vec::new();
        for w in corners.windows(2) {
            let n = 4 + (dice.next() * 14.0) as usize;
            for i in 0..n {
                let t = i as f32 / n as f32;
                // Corners are cut: the point lags behind where an exact hand would be.
                let p = w[0].lerp(w[1], t);
                let shake = Vec2::new(dice.between(-1.0, 1.0), dice.between(-1.0, 1.0)) * 0.035 * sloppiness;
                out.push(Vec2::new(400.0, 300.0) + tilt.rotate(p + shake) * size);
            }
        }
        out.push(Vec2::new(400.0, 300.0) + tilt.rotate(*corners.last().unwrap()) * size);
        // Round the corners a little by averaging neighbours.
        let smooth: Vec<Vec2> = (0..out.len()).map(|i| (out[i.saturating_sub(1)] + out[i] + out[(i + 1).min(out.len() - 1)]) / 3.0).collect();
        smooth
    }

    #[test]
    fn every_rune_drawn_exactly_is_itself() {
        for rune in Rune::ALL {
            for strokes in rune.strokes() {
                let path: Vec<Vec2> = ideal_path(strokes).into_iter().map(|p| p * 150.0 + Vec2::new(300.0, 300.0)).collect();
                let r = recognise(&path).unwrap_or_else(|| panic!("{rune:?} ({strokes}) drawn exactly is not recognised"));
                assert_eq!(r.rune, rune, "{strokes}");
                assert!(r.difference < 0.02, "{rune:?}: {}", r.difference);
            }
        }
        assert_eq!(Rune::from_name("Citrius"), Some(Cetrius));
        assert_eq!(Rune::from_name("nothing"), None);
    }

    #[test]
    fn runes_drawn_by_a_shaky_hand_are_still_recognised_and_never_mistaken() {
        let mut dice = Dice(20_021_114);
        let (mut right, mut wrong, mut refused, mut total) = (0, 0, 0, 0);
        let mut confusions: Vec<(Rune, Rune)> = Vec::new();
        for _ in 0..150 {
            for rune in Rune::ALL {
                let strokes = rune.strokes()[0];
                let stroke = by_hand(strokes, &mut dice, 1.0);
                total += 1;
                match recognise(&stroke) {
                    Some(r) if r.rune == rune => right += 1,
                    Some(r) => {
                        wrong += 1;
                        confusions.push((rune, r.rune));
                    }
                    None => refused += 1,
                }
            }
        }
        let share = |n: i32| n as f32 / total as f32;
        assert!(share(right) > 0.96, "recognised {right} of {total} ({refused} refused, {wrong} mistaken: {confusions:?})");
        assert!(share(wrong) < 0.004, "{wrong} of {total} taken for another rune: {confusions:?}");
    }

    #[test]
    fn scribbles_and_clicks_are_no_rune() {
        // A click, a dot.
        assert!(recognise(&[Vec2::new(10.0, 10.0), Vec2::new(12.0, 11.0)]).is_none());
        assert!(recognise(&[]).is_none());
        // A circle, a spiral, a zigzag that is nobody's rune.
        let circle: Vec<Vec2> = (0..60).map(|i| Vec2::from_angle(i as f32 * 0.105) * 100.0 + Vec2::splat(300.0)).collect();
        assert_eq!(recognise(&circle), None);
        let spiral: Vec<Vec2> = (0..120).map(|i| Vec2::from_angle(i as f32 * 0.21) * (20.0 + i as f32 * 1.5) + Vec2::splat(300.0)).collect();
        assert_eq!(recognise(&spiral), None);
        let zigzag: Vec<Vec2> = (0..9).map(|i| Vec2::new(100.0 + i as f32 * 40.0, if i % 2 == 0 { 100.0 } else { 220.0 })).collect();
        assert_eq!(recognise(&zigzag), None);
        // The same stroke backwards is a different rune (or none): direction counts.
        let right: Vec<Vec2> = (0..20).map(|i| Vec2::new(100.0 + i as f32 * 10.0, 200.0)).collect();
        let left: Vec<Vec2> = right.iter().rev().copied().collect();
        assert_eq!((recognise(&right).map(|r| r.rune), recognise(&left).map(|r| r.rune)), (Some(Aam), Some(Nhi)));
    }

    #[test]
    fn the_spell_table_is_the_engines() {
        assert_eq!(SPELLS.len(), 49);
        assert_eq!(spell_for(&[Aam, Yok]).map(|s| s.name), Some("ignit"));
        assert_eq!(spell_for(&[Aam, Yok, Taar]).map(|s| s.name), Some("fireball"));
        assert_eq!(spell_for(&[Mega, Aam, Mega, Yok]).map(|s| s.name), Some("mass_incinerate"));
        assert_eq!(spell_for(&[Yok, Aam]), None);
        // No two spells share their runes.
        for (i, a) in SPELLS.iter().enumerate() {
            assert!(SPELLS[i + 1..].iter().all(|b| a.runes != b.runes && a.name != b.name), "{}", a.name);
        }
        assert_eq!((mana_cost("heal", 3.0), mana_cost("magic_missile", 3.0), mana_cost("fireball", 2.0), mana_cost("ignit", 9.0)), (4.0, 3.0, 6.0, 1.0));
    }

    #[test]
    fn casting_needs_the_runes_and_the_mana_and_does_what_the_spell_says() {
        let mut w = ScriptWorld::new();
        let mut h = StdHost::new();
        let player = w.add_entity(arx_script::EntityKind::Player, "graph/obj3d/interactive/player/player", 1, None, None);
        w.player = Some(player);
        let heard = w.add_entity(
            arx_script::EntityKind::Fix,
            "x/fire",
            1,
            Some(std::sync::Arc::new(arx_script::Script::new("on spellcast {\n if (^$param1 == \"ignit\") set \u{a7}lit 1\n accept\n}".chars().map(|c| c as u8).collect::<Vec<u8>>().as_slice()))),
            None,
        );
        let mut spells = ActiveSpells::default();
        // A new hero: mind 6, no casting skill beyond what mind gives -> level 1 (the least), mana 6.
        let level = spell_level(&h.player);
        assert!((1.0..=2.5).contains(&level), "{level}");
        assert_eq!(spells.cast(&mut w, &mut h, player, &[]), Err(CastError::Nothing));
        assert_eq!(spells.cast(&mut w, &mut h, player, &[Aam, Yok]), Err(CastError::RuneNotKnown(Aam)));
        for r in ["aam", "yok", "mega", "vitae", "taar", "movis", "kaom"] {
            h.player.runes.insert(r.to_owned());
        }
        assert_eq!(spells.cast(&mut w, &mut h, player, &[Yok, Aam]), Err(CastError::NoSuchSpell));
        // Ignite: one mana, a radius of 400 + 30 a level, and every script hears of it.
        let cast = spells.cast(&mut w, &mut h, player, &[Aam, Yok]).unwrap();
        assert_eq!((cast.spell.name, cast.effect.clone()), ("ignit", Effect::Ignite { radius: 400.0 + 30.0 * level }));
        assert_eq!(h.player.mana.current, 5.0);
        w.update(&mut h, 0.0);
        assert_eq!(w.entity(heard).vars.get_int("\u{a7}lit"), 1);
        // Heal: four mana, then life comes back over three and a half seconds.
        h.player.life.current = 2.0;
        assert_eq!(spells.cast(&mut w, &mut h, player, &[Mega, Vitae]).unwrap().effect, Effect::OnCaster);
        assert_eq!(h.player.mana.current, 1.0);
        assert!(spells.is_active("heal"));
        let mut ended = Vec::new();
        for _ in 0..40 {
            ended.extend(spells.update(&mut h, 100.0));
        }
        let healed = h.player.life.current - 2.0;
        assert!((0.8 * 3.5 * level * 0.9..=2.4 * 3.5 * level * 1.1).contains(&healed), "healed {healed} at level {level}");
        assert_eq!(ended, ["heal"]);
        // Not enough mana for another: nothing is taken.
        let before = h.player.mana.current;
        assert!(matches!(spells.cast(&mut w, &mut h, player, &[Mega, Vitae]), Err(CastError::NotEnoughMana { .. })));
        assert_eq!(h.player.mana.current, before);
        // Mana comes back by itself, slowly.
        spells.update(&mut h, 10_000.0);
        assert!(h.player.mana.current > before && h.player.mana.current < before + 2.0);
        // Armour and speed last twenty seconds and then stop.
        h.player.mana.current = 6.0;
        let armour = h.player.misc().armor_class;
        spells.cast(&mut w, &mut h, player, &[Mega, Kaom]).unwrap();
        spells.cast(&mut w, &mut h, player, &[Mega, Movis]).unwrap();
        spells.update(&mut h, 16.0);
        assert_eq!(h.player.misc().armor_class, armour + level.floor().max(level));
        assert!((spells.speed_factor() - (1.0 + 0.1 * level)).abs() < 1e-5);
        spells.update(&mut h, 21_000.0);
        assert_eq!((h.player.misc().armor_class, spells.speed_factor()), (armour, 1.0));
        // Missiles: more of them the stronger the caster.
        h.player.mana.current = 6.0;
        assert_eq!(spells.cast(&mut w, &mut h, player, &[Aam, Taar]).unwrap().effect, Effect::Missiles { count: 1, damage: (4.0 + level * 0.2) * 0.8 });
    }
}
