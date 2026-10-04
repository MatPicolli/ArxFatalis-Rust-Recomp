//! The player's game state that scripts read and change: life, mana, hunger and the inventory.

use crate::world::EntityId;
use std::collections::HashMap;

/// One bag of the player's inventory is this many slots wide and high.
pub const BAG_WIDTH: u8 = 16;
pub const BAG_HEIGHT: u8 = 3;
pub const MAX_BAGS: usize = 3;

/// Where an item sits in the inventory grid, and how much room it takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub bag: u8,
    pub x: u8,
    pub y: u8,
    pub w: u8,
    pub h: u8,
}

impl Slot {
    fn overlaps(&self, other: &Slot) -> bool {
        self.bag == other.bag && self.x < other.x + other.w && other.x < self.x + self.w && self.y < other.y + other.h && other.y < self.y + self.h
    }
}

/// A value that stays between 0 and a maximum (life, mana).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pool {
    pub current: f32,
    pub max: f32,
}

impl Pool {
    pub fn full(max: f32) -> Self {
        Pool { current: max, max }
    }

    /// Add (or, if negative, remove) points, staying within `0..=max`.
    pub fn add(&mut self, points: f32) {
        self.current = (self.current + points).clamp(0.0, self.max);
    }

    /// Fraction of the maximum that is left.
    pub fn fraction(&self) -> f32 {
        if self.max <= 0.0 { 0.0 } else { self.current / self.max }
    }
}

/// The four attributes of the character sheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attribute {
    Strength,
    Mind,
    Dexterity,
    Constitution,
}

impl Attribute {
    pub const ALL: [Attribute; 4] = [Attribute::Strength, Attribute::Mind, Attribute::Dexterity, Attribute::Constitution];
}

/// The nine skills, in the character sheet's order (rows of three).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skill {
    Stealth,
    Mecanism,
    Intuition,
    EtheralLink,
    ObjectKnowledge,
    Casting,
    CloseCombat,
    Projectile,
    Defense,
}

impl Skill {
    pub const ALL: [Skill; 9] = [
        Skill::Stealth,
        Skill::Mecanism,
        Skill::Intuition,
        Skill::EtheralLink,
        Skill::ObjectKnowledge,
        Skill::Casting,
        Skill::CloseCombat,
        Skill::Projectile,
        Skill::Defense,
    ];

    /// Name used by scripts: `^player_skill_<name>`.
    pub fn script_name(self) -> &'static str {
        match self {
            Skill::Stealth => "stealth",
            Skill::Mecanism => "mecanism",
            Skill::Intuition => "intuition",
            Skill::EtheralLink => "etheral_link",
            Skill::ObjectKnowledge => "object_knowledge",
            Skill::Casting => "casting",
            Skill::CloseCombat => "close_combat",
            Skill::Projectile => "projectile",
            Skill::Defense => "defense",
        }
    }
}

/// Skill values, indexed by [`Skill`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Skills(pub [f32; 9]);

impl Skills {
    pub fn get(&self, s: Skill) -> f32 {
        self.0[s as usize]
    }

    pub fn set(&mut self, s: Skill, v: f32) {
        self.0[s as usize] = v;
    }
}

/// Values derived from attributes and skills (the right-hand numbers of the sheet's left page).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Misc {
    pub armor_class: f32,
    pub resist_magic: f32,
    pub resist_poison: f32,
    pub critical_hit: f32,
    pub damages: f32,
}

/// Experience needed to reach `level` (`GetXPforLevel`).
pub fn xp_for_level(level: i32) -> i64 {
    const TABLE: [i64; 15] = [0, 2000, 4000, 6000, 10000, 16000, 26000, 42000, 68000, 110000, 178000, 300000, 450000, 600000, 750000];
    usize::try_from(level).ok().and_then(|l| TABLE.get(l).copied()).unwrap_or(i64::from(level) * 60000)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attributes {
    pub strength: i32,
    pub mind: i32,
    pub dexterity: i32,
    pub constitution: i32,
}

#[derive(Debug, Clone)]
pub struct PlayerState {
    pub level: i32,
    pub attributes: Attributes,
    pub life: Pool,
    pub mana: Pool,
    /// 100 = fed, 0 = starving.
    pub hunger: f32,
    /// Stack entities in the inventory, in the order they were picked up. A stack's size is the `count` of its
    /// entity (see `EntityState`); destroyed entries are dropped by [`PlayerState::prune`].
    pub inventory: Vec<EntityId>,
    /// Skill points spent by the player (the skills the attributes give come on top).
    pub skills: Skills,
    /// The skills when the points were last handed out: they cannot be refunded below this.
    pub skills_floor: Skills,
    pub xp: i64,
    /// Points to hand out to attributes / skills (a new hero has 16 and 18, as in character creation).
    pub attribute_points: u32,
    pub skill_points: u32,
    /// Quest log entries (localisation keys), oldest first.
    pub quests: Vec<String>,
    /// Where each carried stack sits in the grid.
    pub slots: HashMap<EntityId, Slot>,
    /// Number of bags (each `BAG_WIDTH` x `BAG_HEIGHT`); `addbag` adds one.
    pub bags: usize,
    pub gold: u64,
}

impl Default for PlayerState {
    /// A new hero before the character creation screen: every attribute at 6, level 0.
    fn default() -> Self {
        let attributes = Attributes { strength: 6, mind: 6, dexterity: 6, constitution: 6 };
        let level = 0;
        PlayerState {
            level,
            attributes,
            life: Pool::full(Self::max_life(&attributes, level)),
            mana: Pool::full(Self::max_mana(&attributes, level)),
            hunger: 100.0,
            inventory: Vec::new(),
            skills: Skills::default(),
            skills_floor: Skills::default(),
            xp: 0,
            attribute_points: 16,
            skill_points: 18,
            quests: Vec::new(),
            slots: HashMap::new(),
            bags: 1,
            gold: 0,
        }
    }
}

impl PlayerState {
    pub fn max_life(a: &Attributes, level: i32) -> f32 {
        (a.constitution * (level + 2)) as f32
    }

    pub fn max_mana(a: &Attributes, level: i32) -> f32 {
        (a.mind * (level + 1)) as f32
    }

    pub fn attribute(&self, a: Attribute) -> i32 {
        match a {
            Attribute::Strength => self.attributes.strength,
            Attribute::Mind => self.attributes.mind,
            Attribute::Dexterity => self.attributes.dexterity,
            Attribute::Constitution => self.attributes.constitution,
        }
    }

    fn attribute_mut(&mut self, a: Attribute) -> &mut i32 {
        match a {
            Attribute::Strength => &mut self.attributes.strength,
            Attribute::Mind => &mut self.attributes.mind,
            Attribute::Dexterity => &mut self.attributes.dexterity,
            Attribute::Constitution => &mut self.attributes.constitution,
        }
    }

    /// Skills including what the attributes give (`getAttributeSkillModifiers`).
    pub fn full_skills(&self) -> Skills {
        let a = &self.attributes;
        let (st, mi, de, co) = (a.strength as f32, a.mind as f32, a.dexterity as f32, a.constitution as f32);
        let from_attributes = [
            de * 2.0,
            de + mi,
            mi * 2.0,
            mi * 2.0,
            mi * 1.5 + de * 0.5 + st * 0.5,
            mi * 2.0,
            de + st * 2.0,
            de * 2.0 + st,
            co * 3.0,
        ];
        let mut full = self.skills;
        for (v, extra) in full.0.iter_mut().zip(from_attributes) {
            *v += extra;
        }
        full
    }

    /// Armour class, resistances, critical hit and damage (`getMiscStats`; no equipment yet).
    pub fn misc(&self) -> Misc {
        let a = &self.attributes;
        let (st, mi, de, co) = (a.strength as f32, a.mind as f32, a.dexterity as f32, a.constitution as f32);
        let sk = self.full_skills();
        Misc {
            armor_class: (sk.get(Skill::Defense) * 0.1 - 1.0).max(1.0).floor(),
            resist_magic: (mi * 2.0 * (1.0 + sk.get(Skill::Casting) * 0.005)).floor(),
            resist_poison: (co * 2.0 + sk.get(Skill::Defense) * 0.25).floor(),
            critical_hit: de * 2.0 + sk.get(Skill::CloseCombat) * 0.2 - 18.0,
            damages: ((st * 0.5 - 5.0).max(1.0) + sk.get(Skill::CloseCombat) * 0.1).max(1.0),
        }
    }

    /// Bring the life and mana maximums in line with the attributes and level. While the hero is still being
    /// created (level 0) the pools are kept full.
    pub fn recompute(&mut self) {
        self.life.max = Self::max_life(&self.attributes, self.level);
        self.mana.max = Self::max_mana(&self.attributes, self.level);
        if self.level == 0 {
            self.life.current = self.life.max;
            self.mana.current = self.mana.max;
        }
        self.life.current = self.life.current.min(self.life.max);
        self.mana.current = self.mana.current.min(self.mana.max);
    }

    /// Spend a point on an attribute; false if there is none to spend.
    pub fn spend_attribute(&mut self, a: Attribute) -> bool {
        if self.attribute_points == 0 {
            return false;
        }
        self.attribute_points -= 1;
        *self.attribute_mut(a) += 1;
        self.recompute();
        true
    }

    /// Take a point back (only while creating the hero, level 0, and never below the starting 6).
    pub fn refund_attribute(&mut self, a: Attribute) -> bool {
        if self.level != 0 || self.attribute(a) <= 6 {
            return false;
        }
        *self.attribute_mut(a) -= 1;
        self.attribute_points += 1;
        self.recompute();
        true
    }

    pub fn spend_skill(&mut self, s: Skill) -> bool {
        if self.skill_points == 0 {
            return false;
        }
        self.skill_points -= 1;
        let v = self.skills.get(s) + 1.0;
        self.skills.set(s, v);
        true
    }

    pub fn refund_skill(&mut self, s: Skill) -> bool {
        if self.level != 0 || self.skills.get(s) <= self.skills_floor.get(s) {
            return false;
        }
        let v = self.skills.get(s) - 1.0;
        self.skills.set(s, v);
        self.skill_points += 1;
        true
    }

    /// Add experience; every level reached gives 15 skill points and 1 attribute point and refills life and mana.
    /// Returns how many levels were gained (`ARX_PLAYER_Modify_XP`).
    pub fn add_xp(&mut self, points: i64) -> u32 {
        self.xp += points;
        let mut gained = 0;
        for next in (self.level + 1)..11 {
            if self.xp >= xp_for_level(next) {
                self.level += 1;
                self.skill_points += 15;
                self.attribute_points += 1;
                self.recompute();
                self.life.current = self.life.max;
                self.mana.current = self.mana.max;
                self.skills_floor = self.skills;
                gained += 1;
            }
        }
        gained
    }

    /// Values scripts read as `^player_*` variables.
    pub fn script_vars(&self) -> Vec<(String, f32)> {
        let mut v = vec![
            ("^player_life".to_owned(), self.life.current),
            ("^player_maxlife".to_owned(), self.life.max),
            ("^player_maxmana".to_owned(), self.mana.max),
            ("^player_gold".to_owned(), self.gold as f32),
            ("^player_hunger".to_owned(), self.hunger),
            ("^player_poison".to_owned(), 0.0),
            ("^player_attribute_strength".to_owned(), self.attributes.strength as f32),
            ("^player_attribute_dexterity".to_owned(), self.attributes.dexterity as f32),
            ("^player_attribute_constitution".to_owned(), self.attributes.constitution as f32),
            ("^player_attribute_mind".to_owned(), self.attributes.mind as f32),
        ];
        let full = self.full_skills();
        for s in Skill::ALL {
            v.push((format!("^player_skill_{}", s.script_name()), full.get(s)));
        }
        v
    }

    /// Eat something worth `food` points (the engine counts each one as four points of hunger).
    pub fn eat(&mut self, food: f32) {
        self.hunger = (self.hunger + food * 4.0).min(100.0);
    }

    pub fn is_dead(&self) -> bool {
        self.life.current <= 0.0
    }

    /// Is the `w` x `h` area at (`x`, `y`) of `bag` inside the bag and free of other items (`ignore` is
    /// treated as absent, to move an item onto itself)?
    pub fn area_free(&self, bag: u8, x: u8, y: u8, w: u8, h: u8, ignore: Option<EntityId>) -> bool {
        if bag as usize >= self.bags || x as u16 + w as u16 > BAG_WIDTH as u16 || y as u16 + h as u16 > BAG_HEIGHT as u16 {
            return false;
        }
        let area = Slot { bag, x, y, w, h };
        !self.slots.iter().any(|(&id, s)| Some(id) != ignore && s.overlaps(&area))
    }

    /// First free place for an item of `w` x `h` slots, in the engine's order: bag by bag, column by column,
    /// top to bottom within a column.
    pub fn find_free(&self, w: u8, h: u8) -> Option<Slot> {
        if w > BAG_WIDTH || h > BAG_HEIGHT {
            return None;
        }
        for bag in 0..self.bags as u8 {
            for x in 0..=(BAG_WIDTH - w) {
                for y in 0..=(BAG_HEIGHT - h) {
                    if self.area_free(bag, x, y, w, h, None) {
                        return Some(Slot { bag, x, y, w, h });
                    }
                }
            }
        }
        None
    }

    /// The carried stack that covers a slot.
    pub fn item_at(&self, bag: u8, x: u8, y: u8) -> Option<EntityId> {
        let cell = Slot { bag, x, y, w: 1, h: 1 };
        self.slots.iter().find(|(_, s)| s.overlaps(&cell)).map(|(&id, _)| id)
    }

    /// Move a carried item to another place; false (and nothing changes) if it does not fit there.
    pub fn move_item(&mut self, id: EntityId, bag: u8, x: u8, y: u8) -> bool {
        let Some(old) = self.slots.get(&id).copied() else { return false };
        if !self.area_free(bag, x, y, old.w, old.h, Some(id)) {
            return false;
        }
        self.slots.insert(id, Slot { bag, x, y, ..old });
        true
    }

    /// Forget an item (dropped, used up, given away).
    pub fn remove_item(&mut self, id: EntityId) {
        self.inventory.retain(|&i| i != id);
        self.slots.remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skills_and_derived_stats_follow_the_engines_formulas() {
        let mut p = PlayerState::default();
        // All attributes 6: skills come only from the attributes.
        let sk = p.full_skills();
        assert_eq!(sk.get(Skill::Stealth), 12.0);
        assert_eq!(sk.get(Skill::Mecanism), 12.0);
        assert_eq!(sk.get(Skill::ObjectKnowledge), 15.0);
        assert_eq!(sk.get(Skill::CloseCombat), 18.0);
        assert_eq!(sk.get(Skill::Defense), 18.0);
        let m = p.misc();
        assert_eq!((m.armor_class, m.resist_magic, m.resist_poison), (1.0, 12.0, 16.0));
        assert!((m.damages - 2.8).abs() < 1e-4, "strength 6 gives the minimum 1, plus 18 x 0.1 from close combat: {}", m.damages);
        // Spending points: constitution raises life; hero creation keeps the pools full.
        assert!(p.spend_attribute(Attribute::Constitution));
        assert_eq!((p.attribute_points, p.life.max, p.life.current), (15, 14.0, 14.0));
        assert!(p.spend_skill(Skill::Stealth));
        assert_eq!((p.skill_points, p.full_skills().get(Skill::Stealth)), (17, 13.0));
        // Refunds: only while level 0, never below the start.
        assert!(p.refund_attribute(Attribute::Constitution));
        assert!(!p.refund_attribute(Attribute::Constitution), "never below 6");
        assert!(p.refund_skill(Skill::Stealth));
        assert!(!p.refund_skill(Skill::Stealth));
        assert_eq!((p.attribute_points, p.skill_points), (16, 18));
        // Out of points.
        p.attribute_points = 0;
        assert!(!p.spend_attribute(Attribute::Mind));
    }

    #[test]
    fn experience_levels_the_hero_up() {
        let mut p = PlayerState::default();
        p.attribute_points = 0;
        p.skill_points = 0;
        assert_eq!(p.add_xp(1999), 0);
        p.life.current = 3.0;
        assert_eq!(p.add_xp(1), 1, "2000 xp is level 1");
        assert_eq!((p.level, p.attribute_points, p.skill_points), (1, 1, 15));
        assert_eq!((p.life.max, p.life.current, p.mana.max), (18.0, 18.0, 12.0), "level 1: 6 x 3 life, 6 x 2 mana, refilled");
        // A big gain can jump several levels.
        assert_eq!(p.add_xp(8000), 3, "10000 xp: levels 2, 3 and 4");
        assert_eq!(p.level, 4);
        assert_eq!(xp_for_level(15), 900_000, "beyond the table: 60000 per level");
        assert_eq!(xp_for_level(3), 6000);
        // Refunds are over once the game has begun.
        assert!(!p.refund_attribute(Attribute::Mind));
    }

    #[test]
    fn scripts_can_read_the_players_stats() {
        let p = PlayerState::default();
        let vars: std::collections::HashMap<_, _> = p.script_vars().into_iter().collect();
        assert_eq!(vars["^player_skill_mecanism"], 12.0);
        assert_eq!(vars["^player_attribute_mind"], 6.0);
        assert_eq!(vars["^player_maxlife"], 12.0);
        assert_eq!(vars["^player_gold"], 0.0);
    }

    #[test]
    fn items_are_placed_column_by_column_and_never_overlap() {
        let mut p = PlayerState::default();
        let a = p.find_free(1, 1).unwrap();
        assert_eq!((a.bag, a.x, a.y), (0, 0, 0));
        p.slots.insert(1, a);
        // Next free place is below it (the inner loop runs down a column of 3), then the next column.
        let b = p.find_free(1, 1).unwrap();
        assert_eq!((b.x, b.y), (0, 1));
        p.slots.insert(2, b);
        // A 2x2 item cannot use column 0 (only one row is left there), so it starts at column 1.
        let big = p.find_free(2, 2).unwrap();
        assert_eq!((big.x, big.y), (1, 0));
        p.slots.insert(3, big);
        assert_eq!(p.item_at(0, 2, 1), Some(3));
        assert_eq!(p.item_at(0, 5, 0), None);
        assert!(!p.area_free(0, 1, 0, 1, 1, None));
        assert!(p.area_free(0, 1, 0, 1, 1, Some(3)));
        // Moving.
        assert!(p.move_item(3, 0, 4, 0));
        assert!(!p.move_item(3, 0, 0, 0), "occupied");
        assert!(!p.move_item(3, 0, 15, 0), "would stick out of the bag");
        assert!(!p.move_item(3, 1, 0, 0), "there is only one bag");
        p.remove_item(3);
        assert!(p.slots.get(&3).is_none());
        // Fill the bag completely.
        let mut p = PlayerState::default();
        for i in 0..(BAG_WIDTH as u32 * BAG_HEIGHT as u32) {
            let s = p.find_free(1, 1).expect("room");
            p.slots.insert(i, s);
        }
        assert_eq!(p.find_free(1, 1), None);
        p.bags = 2;
        assert_eq!(p.find_free(1, 1).map(|s| s.bag), Some(1));
        assert_eq!(p.find_free(4, 1), Some(Slot { bag: 1, x: 0, y: 0, w: 4, h: 1 }));
    }

    #[test]
    fn a_fresh_hero_has_the_engines_starting_pools() {
        let p = PlayerState::default();
        assert_eq!((p.life.max, p.life.current), (12.0, 12.0));
        assert_eq!((p.mana.max, p.mana.current), (6.0, 6.0));
    }

    #[test]
    fn pools_are_clamped_and_food_stops_at_full() {
        let mut p = PlayerState::default();
        p.life.add(-5.0);
        p.life.add(100.0);
        assert_eq!(p.life.current, 12.0);
        p.life.add(-100.0);
        assert!(p.is_dead());
        p.hunger = 50.0;
        p.eat(14.0);
        assert_eq!(p.hunger, 100.0);
        p.hunger = 10.0;
        p.eat(4.0);
        assert_eq!(p.hunger, 26.0);
    }
}
