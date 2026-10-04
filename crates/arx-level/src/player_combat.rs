//! The hero's way of fighting: draw the weapon, wind up a blow while the button is held, strike when it is let go
//! (harder the longer it was held), put the weapon away. Ported from the engine's `ManageCombatModeAnimations`: the
//! stages follow the hero's own animations (`1h_ready_part_1`, `1h_strike_left_start`, ...), which also say when
//! the blow can land.

use crate::npc::{STRIKE_DIRECTIONS, WeaponKind, player_weapon_kind};
use arx_script::{EntityId, StdHost};

/// A blow takes about this long when the slot's animation cannot be measured.
const DEFAULT_MS: f32 = 400.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Weapon away.
    Sheathed,
    /// Drawing it (the second part for weapons that have two).
    Readying(u8),
    /// Ready, waiting for the button.
    Ready,
    /// Button pressed: the arm goes back.
    WindUp,
    /// Arm back, button still held: the blow gathers force.
    Aim,
    /// Button let go: the blow comes down.
    Strike,
    /// Putting it away.
    Unreadying(u8),
}

/// What changed in a frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Update {
    /// The animation slot (and whether it loops) the arm layer should switch to; `Some((None, ..))` clears it.
    pub animation: Option<(Option<String>, bool)>,
    /// The blow was let go this frame (the hero's script gets `strike`).
    pub struck: bool,
    /// The aim at that moment was good enough to grunt (`strikespeech`).
    pub grunt: bool,
    /// The blow can land now: check what the weapon touches and call [`PlayerCombat::landed`] if it hits something.
    pub blow: bool,
}

#[derive(Debug, Clone)]
pub struct PlayerCombat {
    pub stage: Stage,
    slot: Option<String>,
    time_ms: f32,
    /// How long the blow has been gathering (`m_aimTime`), 0 when not aiming.
    pub aim_ms: f32,
    /// How fully the last blow was aimed, 0.1..1 (`m_strikeAimRatio`).
    pub strike_ratio: f32,
    /// Which of the four directions the next blow comes from (index into [`STRIKE_DIRECTIONS`]).
    pub direction: usize,
    /// The weapon in use, fixed when it was drawn.
    weapon: WeaponKind,
    /// The blow has hit something (`m_weaponBlocked`): it cannot hit again.
    blocked: bool,
}

impl Default for PlayerCombat {
    fn default() -> Self {
        PlayerCombat { stage: Stage::Sheathed, slot: None, time_ms: 0.0, aim_ms: 0.0, strike_ratio: 0.1, direction: 0, weapon: WeaponKind::Bare, blocked: false }
    }
}

impl PlayerCombat {
    pub fn is_fighting(&self) -> bool {
        !matches!(self.stage, Stage::Sheathed | Stage::Unreadying(_))
    }

    pub fn is_aiming(&self) -> bool {
        matches!(self.stage, Stage::Aim)
    }

    /// The animation slot the arm layer plays now.
    pub fn slot(&self) -> Option<&str> {
        self.slot.as_deref()
    }

    pub fn weapon(&self) -> WeaponKind {
        self.weapon
    }

    /// How full the aim gauge is, 0.2 (idle) to 1.
    pub fn gauge(&self, host: &StdHost) -> f32 {
        if self.is_aiming() { (self.aim_ms / host.player.aim_time_ms()).clamp(0.2, 1.0) } else { 0.2 }
    }

    fn duration(&self, host: &StdHost, player: EntityId, slot: &str) -> f32 {
        host.slot_duration_ms(player, slot).map_or(DEFAULT_MS, |d| d as f32)
    }

    fn set(&mut self, slot: Option<String>, looping: bool) -> Option<(Option<String>, bool)> {
        self.slot = slot.clone();
        self.time_ms = 0.0;
        Some((slot, looping))
    }

    /// The slot of a stage of the draw or the sheathing: `<w>_ready_part_<n>` (bare hands have one `bare_ready`).
    fn ready_slot(&self, host: &StdHost, player: EntityId, part: u8, unready: bool) -> Option<String> {
        let w = self.weapon.prefix();
        let phase = if unready { "unready" } else { "ready" };
        let candidates = if self.weapon == WeaponKind::Bare { vec![format!("{w}_{phase}")] } else { vec![format!("{w}_{phase}_part_{part}")] };
        candidates.into_iter().find(|s| host.has_slot(player, s))
    }

    /// Draw the weapon, or put it away. (A bow is held ready but cannot yet be shot.)
    pub fn toggle(&mut self, host: &mut StdHost, player: EntityId) -> Update {
        let mut up = Update::default();
        match self.stage {
            Stage::Sheathed => {
                self.weapon = player_weapon_kind(host);
                host.player.fighting = true;
                self.aim_ms = 0.0;
                self.blocked = false;
                match self.ready_slot(host, player, 1, false) {
                    Some(slot) => {
                        self.stage = Stage::Readying(1);
                        up.animation = self.set(Some(slot), false);
                    }
                    None => {
                        self.stage = Stage::Ready;
                        up.animation = self.set(Some(format!("{}_wait", self.weapon.prefix())), true);
                    }
                }
            }
            Stage::Ready | Stage::Readying(_) => {
                host.player.fighting = false;
                self.aim_ms = 0.0;
                match self.ready_slot(host, player, 1, true) {
                    Some(slot) => {
                        self.stage = Stage::Unreadying(1);
                        up.animation = self.set(Some(slot), false);
                    }
                    None => {
                        self.stage = Stage::Sheathed;
                        up.animation = self.set(None, false);
                    }
                }
            }
            // Mid-blow: finish it first.
            _ => {}
        }
        up
    }

    /// Advance by `dt_ms` with the attack button held or not.
    pub fn update(&mut self, host: &mut StdHost, player: EntityId, dt_ms: f32, attack_held: bool) -> Update {
        let mut up = Update::default();
        self.time_ms += dt_ms;
        if self.aim_ms > 0.0 {
            self.aim_ms += dt_ms;
        }
        let w = self.weapon.prefix();
        match self.stage {
            Stage::Sheathed => {}
            Stage::Readying(part) => {
                let slot = self.slot.clone().unwrap_or_default();
                if self.time_ms >= self.duration(host, player, &slot) {
                    match self.ready_slot(host, player, part + 1, false).filter(|_| part == 1 && self.weapon != WeaponKind::Bare) {
                        Some(next) => {
                            self.stage = Stage::Readying(part + 1);
                            up.animation = self.set(Some(next), false);
                        }
                        None => {
                            self.stage = Stage::Ready;
                            up.animation = self.set(Some(format!("{w}_wait")), true);
                        }
                    }
                }
            }
            Stage::Ready => {
                self.aim_ms = 0.0;
                if attack_held && self.weapon != WeaponKind::Bow {
                    self.blocked = false;
                    let slot = format!("{w}_strike_{}_start", STRIKE_DIRECTIONS[self.direction]);
                    if host.has_slot(player, &slot) {
                        self.stage = Stage::WindUp;
                        up.animation = self.set(Some(slot), false);
                    }
                }
            }
            Stage::WindUp => {
                let slot = self.slot.clone().unwrap_or_default();
                if self.time_ms >= self.duration(host, player, &slot) {
                    self.stage = Stage::Aim;
                    self.aim_ms = 1.0;
                    up.animation = self.set(Some(format!("{w}_strike_{}_cycle", STRIKE_DIRECTIONS[self.direction])), true);
                }
            }
            Stage::Aim => {
                if !attack_held {
                    self.strike_ratio = (self.aim_ms / host.player.aim_time_ms()).clamp(0.1, 1.0);
                    up.grunt = self.strike_ratio > 0.8;
                    up.struck = true;
                    self.aim_ms = 0.0;
                    self.blocked = false;
                    self.stage = Stage::Strike;
                    up.animation = self.set(Some(format!("{w}_strike_{}", STRIKE_DIRECTIONS[self.direction])), false);
                }
            }
            Stage::Strike => {
                let slot = self.slot.clone().unwrap_or_default();
                let duration = self.duration(host, player, &slot);
                // Weapons connect between 30 % and 70 % of the swing, fists between 20 % and 80 %.
                let (from, to) = if self.weapon == WeaponKind::Bare { (0.2, 0.8) } else { (0.3, 0.7) };
                let t = self.time_ms / duration.max(1.0);
                if t > from && t < to && !self.blocked {
                    up.blow = true;
                }
                if self.time_ms >= duration {
                    self.stage = Stage::Ready;
                    self.aim_ms = 0.0;
                    up.animation = self.set(Some(format!("{w}_wait")), true);
                }
            }
            Stage::Unreadying(part) => {
                let slot = self.slot.clone().unwrap_or_default();
                if self.time_ms >= self.duration(host, player, &slot) {
                    match self.ready_slot(host, player, part + 1, true).filter(|_| part == 1 && self.weapon != WeaponKind::Bare) {
                        Some(next) => {
                            self.stage = Stage::Unreadying(part + 1);
                            up.animation = self.set(Some(next), false);
                        }
                        None => {
                            self.stage = Stage::Sheathed;
                            up.animation = self.set(None, false);
                        }
                    }
                }
            }
        }
        up
    }

    /// The blow hit something: it cannot hit again.
    pub fn landed(&mut self) {
        self.blocked = true;
    }

    /// Choose the direction of the next blow from how the mouse is moving: sideways blows go the way it moves,
    /// up and down ones top and bottom. Only while the weapon waits.
    pub fn steer(&mut self, mouse: glam::Vec2) {
        if self.stage != Stage::Ready || mouse.length() < 2.0 {
            return;
        }
        self.direction = if mouse.x.abs() >= mouse.y.abs() {
            if mouse.x > 0.0 { 1 } else { 0 }
        } else if mouse.y > 0.0 {
            3
        } else {
            2
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arx_script::{EntityKind, ScriptWorld, StdHost, object_type};

    fn rig() -> (ScriptWorld, StdHost, EntityId) {
        let mut w = ScriptWorld::new();
        let mut h = StdHost::new();
        let player = w.add_entity(EntityKind::Player, "x/player", 1, None, None);
        // Every slot the sequence uses, each 100 ms long.
        h.set_anim_duration(Box::new(|_| Some(100.0)));
        for slot in ["1h_ready_part_1", "1h_ready_part_2", "1h_wait", "1h_unready_part_1", "1h_unready_part_2", "bare_ready", "bare_wait", "bare_unready"] {
            h.modify(player, |s| {
                s.anims.insert(slot.into(), format!("anims/{slot}.tea"));
            });
        }
        for dir in STRIKE_DIRECTIONS {
            for slot in [format!("1h_strike_{dir}_start"), format!("1h_strike_{dir}_cycle"), format!("1h_strike_{dir}"), format!("bare_strike_{dir}_start"), format!("bare_strike_{dir}_cycle"), format!("bare_strike_{dir}")] {
                h.modify(player, |s| {
                    s.anims.insert(slot.clone(), format!("anims/{slot}.tea"));
                });
            }
        }
        (w, h, player)
    }

    fn run(c: &mut PlayerCombat, h: &mut StdHost, p: EntityId, ms: f32, held: bool) -> Vec<Update> {
        (0..(ms / 10.0) as usize).map(|_| c.update(h, p, 10.0, held)).collect()
    }

    #[test]
    fn drawing_a_weapon_goes_through_both_parts_and_ends_ready() {
        let (_w, mut h, p) = rig();
        h.modify(p, |_| {});
        let mut c = PlayerCombat::default();
        // Wield a one-handed weapon.
        let sword = 7;
        h.modify(sword, |s| s.type_flags = object_type::WEAPON | object_type::ONE_HANDED);
        h.player.equipped[arx_script::EquipSlot::Weapon as usize] = Some(sword);
        let up = c.toggle(&mut h, p);
        assert_eq!(up.animation, Some((Some("1h_ready_part_1".into()), false)));
        assert!(h.player.fighting && c.is_fighting());
        let ups = run(&mut c, &mut h, p, 150.0, false);
        assert!(ups.iter().any(|u| u.animation == Some((Some("1h_ready_part_2".into()), false))));
        run(&mut c, &mut h, p, 150.0, false);
        assert_eq!(c.stage, Stage::Ready);
        assert_eq!(c.slot(), Some("1h_wait"));
        // Putting it away goes through the unready parts and ends sheathed.
        c.toggle(&mut h, p);
        assert!(!h.player.fighting);
        run(&mut c, &mut h, p, 400.0, false);
        assert_eq!(c.stage, Stage::Sheathed);
        assert_eq!(c.slot(), None);
    }

    #[test]
    fn holding_the_button_gathers_force_and_letting_go_strikes_in_a_window() {
        let (_w, mut h, p) = rig();
        let mut c = PlayerCombat::default();
        c.toggle(&mut h, p); // bare hands
        run(&mut c, &mut h, p, 300.0, false);
        assert_eq!(c.stage, Stage::Ready);
        // Press: wind up, then aim while held.
        let ups = run(&mut c, &mut h, p, 150.0, true);
        assert!(ups.iter().any(|u| u.animation == Some((Some("bare_strike_left_start".into()), false))));
        assert_eq!(c.stage, Stage::Aim);
        // Held for a full 1.5 s: the full-strength blow.
        run(&mut c, &mut h, p, 1600.0, true);
        assert!(c.gauge(&h) > 0.99);
        let ups = run(&mut c, &mut h, p, 10.0, false);
        let struck = ups.iter().find(|u| u.struck).expect("let go: strike");
        assert!(struck.grunt);
        assert_eq!(c.strike_ratio, 1.0);
        assert_eq!(c.slot(), Some("bare_strike_left"));
        // The blow can land only in the middle of the swing, and only once.
        let ups = run(&mut c, &mut h, p, 100.0, false);
        let windows: Vec<bool> = ups.iter().map(|u| u.blow).collect();
        assert!(!windows[0] && windows.iter().any(|b| *b), "{windows:?}");
        c.landed();
        let ups = run(&mut c, &mut h, p, 100.0, false);
        assert!(ups.iter().all(|u| !u.blow), "a blow that hit cannot hit again");
        assert_eq!(c.stage, Stage::Ready, "and the swing is over");
    }

    #[test]
    fn a_quick_tap_is_a_weak_blow_and_the_mouse_picks_the_direction() {
        let (_w, mut h, p) = rig();
        let mut c = PlayerCombat::default();
        c.toggle(&mut h, p);
        run(&mut c, &mut h, p, 300.0, false);
        c.steer(glam::Vec2::new(30.0, 2.0));
        assert_eq!(c.direction, 1, "moving right");
        c.steer(glam::Vec2::new(1.0, -40.0));
        assert_eq!(c.direction, 2, "moving up: a blow from the top");
        run(&mut c, &mut h, p, 120.0, true);
        run(&mut c, &mut h, p, 20.0, true);
        let ups = run(&mut c, &mut h, p, 10.0, false);
        assert!(ups.iter().any(|u| u.struck));
        assert!(c.strike_ratio < 0.2, "barely wound up: {}", c.strike_ratio);
        assert_eq!(c.slot(), Some("bare_strike_top"));
    }
}
