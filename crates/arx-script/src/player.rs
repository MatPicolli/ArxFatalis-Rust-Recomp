//! The player's game state that scripts read and change: life, mana, hunger and the inventory.

use crate::world::EntityId;

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
}

#[cfg(test)]
mod tests {
    use super::*;

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
