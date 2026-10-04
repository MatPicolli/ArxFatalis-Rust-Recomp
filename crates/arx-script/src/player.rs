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
