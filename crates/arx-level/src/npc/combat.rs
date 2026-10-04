//! Blows and what they do: the hero's and the characters' attacks, the damage formula, hits and deaths, ported from
//! the engine's `ARX_EQUIPMENT_ComputeDamages`, `damageNpc` and `ARX_DAMAGES_ForceDeath`, and the characters' own
//! attack sequence (`ARX_NPC_Manage_Anims`).

use super::*;
use arx_script::object_type;

/// What the hero (or a character) holds, which decides the animations of a blow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeaponKind {
    Bare,
    Dagger,
    OneHanded,
    TwoHanded,
    Bow,
}

impl WeaponKind {
    pub fn from_flags(flags: u32) -> Self {
        if flags & object_type::DAGGER != 0 {
            WeaponKind::Dagger
        } else if flags & object_type::ONE_HANDED != 0 {
            WeaponKind::OneHanded
        } else if flags & object_type::TWO_HANDED != 0 {
            WeaponKind::TwoHanded
        } else if flags & object_type::BOW != 0 {
            WeaponKind::Bow
        } else {
            WeaponKind::Bare
        }
    }

    /// The prefix of its animation slots (`bare_wait`, `1h_strike_left_start`, ...).
    pub fn prefix(self) -> &'static str {
        match self {
            WeaponKind::Bare => "bare",
            WeaponKind::Dagger => "dagger",
            WeaponKind::OneHanded => "1h",
            WeaponKind::TwoHanded => "2h",
            WeaponKind::Bow => "missile",
        }
    }

    /// What `strike` and `hit` events call it.
    pub fn event_name(self) -> &'static str {
        match self {
            WeaponKind::Bare => "bare",
            WeaponKind::Dagger => "dagger",
            WeaponKind::OneHanded => "1h",
            WeaponKind::TwoHanded => "2h",
            WeaponKind::Bow => "arrow",
        }
    }
}

/// The directions a blow can come from, in the order of the engine's animation slots.
pub const STRIKE_DIRECTIONS: [&str; 4] = ["left", "right", "top", "bottom"];

/// The weapon the hero holds.
pub fn player_weapon_kind(host: &StdHost) -> WeaponKind {
    host.player_weapon().and_then(|w| host.state(w)).map_or(WeaponKind::Bare, |s| WeaponKind::from_flags(s.type_flags))
}

/// A noise a blow made, for the application to play: what hit what (`snd_armor.ini`, `snd_weapon.ini`).
#[derive(Debug, Clone, PartialEq)]
pub struct CombatSound {
    pub hitter: String,
    pub surface: String,
    /// Where (Arx coordinates).
    pub pos: Vec3,
    pub volume: f32,
}

/// What a blow did to one of the things it struck.
#[derive(Debug, Clone, PartialEq)]
pub struct Impact {
    pub target: EntityId,
    /// Life taken away (0 when it missed, was absorbed, or hit a thing that has no life).
    pub damage: f32,
    pub missed: bool,
    pub killed: bool,
    /// Where it landed (Arx coordinates).
    pub pos: Vec3,
}

/// How a character fights: the stages of one blow.
#[derive(Debug, Clone, Default)]
pub(super) struct Attack {
    /// Slot being played (`1h_strike_left_start`, ...) and which of the four directions it is.
    pub stage: Option<(AttackStage, usize)>,
    pub started_ms: f64,
    /// The blow has already landed.
    pub landed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AttackStage {
    Start,
    Cycle,
    Strike,
}

impl NpcWorld {
    /// Noises made since the last call.
    pub fn take_sounds(&mut self) -> Vec<CombatSound> {
        std::mem::take(&mut self.sounds)
    }

    fn weapon_kind_of(&self, host: &StdHost, id: EntityId) -> WeaponKind {
        host.npc_weapons.get(&id).and_then(|&w| host.state(w)).map_or(WeaponKind::Bare, |s| WeaponKind::from_flags(s.type_flags))
    }

    /// What a character's or the hero's blows are made of (`bare`, `sword`, `claw`, ...).
    pub fn weapon_material(&self, world: &ScriptWorld, host: &StdHost, id: EntityId) -> String {
        let weapon = if Some(id) == world.player { host.player_weapon() } else { host.npc_weapons.get(&id).copied() };
        if let Some(w) = weapon.and_then(|w| host.state(w))
            && !w.weapon_material.is_empty()
        {
            return w.weapon_material.to_ascii_lowercase();
        }
        if let Some(st) = host.state(id)
            && !st.weapon_material.is_empty()
            && world.entity(id).kind == EntityKind::Npc
        {
            return st.weapon_material.to_ascii_lowercase();
        }
        "bare".to_owned()
    }

    fn rand(&mut self) -> f32 {
        self.rng.next_f32()
    }

    /// Port of `ARX_EQUIPMENT_ComputeDamages`: a blow from `source` lands on `target` with `ratio` (how fully it was
    /// aimed, 0..1). Rolls to hit against armour class, subtracts what the armour absorbs, and applies the damage.
    fn compute_damages(&mut self, cx: &mut Ctx, source: EntityId, target: EntityId, ratio: f32, at: Vec3) -> Impact {
        let mut impact = Impact { target, damage: 0.0, missed: false, killed: false, pos: at };
        cx.send(Some(source), target, "aggression", Vec::new());
        let player = cx.world.player;
        let wmat = self.weapon_material(cx.world, cx.host, source);
        let target_kind = cx.world.entity(target).kind;
        let weapon_cat = if Some(source) == player {
            player_weapon_kind(cx.host).event_name()
        } else {
            self.weapon_kind_of(cx.host, source).event_name()
        };

        if target_kind != EntityKind::Npc && Some(target) != player {
            // Props: they get `hit` with the damage and weapon, and decide for themselves.
            if target_kind == EntityKind::Fix {
                let dmg = if Some(source) == player { cx.host.player.misc().damages } else { self.npc(source).map_or(1.0, |n| n.stats.damages) };
                cx.send(Some(source), target, "hit", vec![format!("{dmg}"), weapon_cat.to_owned()]);
            }
            return impact;
        }

        // Attack strength, damage, critical hits and backstabs.
        let (attack, mut damages, mut backstab, mut critical);
        backstab = 1.0;
        critical = false;
        if Some(source) == player {
            let misc = cx.host.player.misc();
            attack = misc.damages;
            if self.rand() * 100.0 <= misc.critical_hit {
                critical = true;
            }
            damages = attack * ratio;
            if self.npc(target).is_some_and(|n| n.stats.backstab) && self.rand() * 100.0 <= cx.host.player.full_skills().get(arx_script::Skill::Stealth) * 0.5 {
                backstab = 1.5;
            }
        } else {
            let Some(src) = self.npc(source) else { return impact };
            let (tohit, dmg, crit, back) = (src.stats.tohit, src.stats.damages, src.stats.critical, src.stats.backstab_skill);
            attack = tohit;
            damages = dmg * ratio * (0.5 + 0.5 * self.rand());
            if self.rand() * 100.0 <= crit {
                critical = true;
            }
            if self.rand() * 100.0 <= back {
                backstab = 1.5;
            }
        }

        // The target's armour class and what it absorbs.
        let (ac, absorb) = if Some(target) == player {
            (cx.host.player.misc().armor_class, cx.host.player.full_skills().get(arx_script::Skill::Defense) * 0.5)
        } else {
            let n = self.npc(target).expect("an NPC target");
            (n.stats.armor_class.max(0.0), n.stats.absorb)
        };
        let mut amat = cx.host.state(target).map(|s| s.armor_material.to_ascii_lowercase()).filter(|m| !m.is_empty()).unwrap_or_else(|| "flesh".to_owned());
        if Some(target) == player
            && let Some(armor) = cx.host.player.equipped_in(arx_script::EquipSlot::Armor).and_then(|a| cx.host.state(a))
            && !armor.armor_material.is_empty()
        {
            amat = armor.armor_material.to_ascii_lowercase();
        }

        damages *= backstab;
        damages -= damages * absorb * 0.01;
        let volume = (damages * 0.05).min(1.0) * 0.1 + 0.9;
        self.sounds.push(CombatSound { hitter: amat, surface: wmat.clone(), pos: at, volume });

        // Does it hit?
        let chance = 100.0 - (ac - attack);
        if self.rand() * 100.0 > chance {
            impact.missed = true;
            return impact;
        }
        self.sounds.push(CombatSound { hitter: "flesh".to_owned(), surface: wmat, pos: at, volume });
        if damages <= 0.0 {
            return impact;
        }
        if critical {
            damages *= 1.5;
        }
        impact.damage = damages;
        if Some(target) == player {
            self.damage_player(cx, damages, Some(source));
            impact.killed = cx.host.player.is_dead();
        } else {
            // A shove away from the attacker.
            let from = Self::entity_pos(cx, source);
            if let Some(i) = self.by_id.get(&target).copied() {
                let away = Vec2::new(self.npcs[i].pos.x - from.x, self.npcs[i].pos.z - from.z).normalize_or_zero();
                self.npcs[i].shove += Vec3::new(away.x, 0.0, away.y) * damages * 2.0;
            }
            impact.killed = self.damage_npc(cx, target, damages, Some(source), weapon_cat) > 0.0 && self.npc(target).is_some_and(|n| n.life <= 0.0);
        }
        impact
    }

    /// The hero loses life; their script hears `ouch` (and `die` at the end).
    fn damage_player(&mut self, cx: &mut Ctx, dmg: f32, source: Option<EntityId>) {
        let Some(player) = cx.world.player else { return };
        if cx.host.player.is_dead() {
            return;
        }
        cx.host.player.life.add(-dmg);
        cx.send(source, player, "ouch", vec![format!("{dmg:.0}")]);
        if cx.host.player.is_dead() {
            cx.send(source, player, "die", Vec::new());
            cx.host.push_message("You are dead - press R".to_owned());
        }
    }

    /// Port of `damageNpc`: the character's script may refuse a `hit`; otherwise it loses life, and at zero it dies and
    /// the hero gets its experience. Returns the life taken.
    fn damage_npc(&mut self, cx: &mut Ctx, target: EntityId, dmg: f32, source: Option<EntityId>, weapon_cat: &str) -> f32 {
        let Some(i) = self.by_id.get(&target).copied() else { return 0.0 };
        if self.npcs[i].life <= 0.0 || self.npcs[i].dead {
            return 0.0;
        }
        // `ouch` at most twice a second.
        self.npcs[i].dmg_sum += dmg;
        if self.now_ms - self.npcs[i].ouch_at_ms > 500.0 {
            self.npcs[i].ouch_at_ms = self.now_ms;
            let sum = std::mem::take(&mut self.npcs[i].dmg_sum);
            cx.send(source, target, "ouch", vec![format!("{sum:.0}")]);
        }
        if dmg < 0.0 {
            return 0.0;
        }
        let result = cx.world.send_event(cx.host, source, target, "hit", vec![format!("{dmg}"), weapon_cat.to_owned()]);
        if result != arx_script::ScriptResult::Accept {
            return 0.0;
        }
        let done = dmg.min(self.npcs[i].life);
        self.npcs[i].life -= dmg;
        if self.npcs[i].life <= 0.0 {
            self.npcs[i].life = 0.0;
            let xp = self.npcs[i].stats.xp_value;
            self.kill_npc(i, cx, source);
            if source == cx.world.player {
                let gained = cx.host.player.add_xp(xp as i64);
                if let Some(p) = cx.world.player {
                    for _ in 0..gained {
                        cx.send(None, p, "level_up", Vec::new());
                    }
                }
            }
        }
        done
    }

    /// Port of `ARX_DAMAGES_ForceDeath`: stop its timers and behaviour, tell its script (`die`: it usually plays the
    /// death), let anyone who was after it know, and stop it blocking the way.
    pub(super) fn kill_npc(&mut self, i: usize, cx: &mut Ctx, killer: Option<EntityId>) {
        let id = self.npcs[i].id;
        if self.npcs[i].dead {
            return;
        }
        {
            let n = &mut self.npcs[i];
            n.behavior = behavior::NONE;
            n.stack.clear();
            n.route.clear();
            n.attack = Attack::default();
            n.life = 0.0;
        }
        cx.world.clear_timers_for(id);
        cx.send(killer, id, "die", Vec::new());
        if cx.host.state(id).is_none_or(|s| s.destroyed) {
            self.npcs[i].dead = true;
            return;
        }
        self.npcs[i].dead = true;
        // The script's own `forceanim die` takes over; otherwise the death plays by itself.
        self.adopt_script_anim(i, cx);
        if !self.npcs[i].layer.is("die") && cx.has_slot(id, "die") && !self.npcs[i].layer.force {
            self.change_anim(i, cx, "die", false);
            self.npcs[i].layer.force = true;
        }
        if let Some(c) = self.npcs[i].cylinder {
            cx.env.collision.set_cylinder(c, to_yup(self.npcs[i].pos.to_array()), false);
        }
        // Whoever was after it forgets.
        let followers: Vec<usize> =
            (0..self.npcs.len()).filter(|&k| k != i && (self.npcs[k].target == Target::Entity(id) || self.npcs[k].route.true_target == Target::Entity(id))).collect();
        for k in followers {
            let fid = self.npcs[k].id;
            let n = &mut self.npcs[k];
            n.target = Target::None;
            n.route.true_target = Target::None;
            n.reached = false;
            let name = cx.world.entity(id).id_string.clone();
            cx.send(Some(id), fid, "target_death", vec![name]);
        }
    }

    /// The hero's weapon, or fist, strikes: `spheres` are the places the blow reaches (centre in Arx coordinates and
    /// radius), `ratio` how fully it was aimed. Every living character a sphere touches is hit, once (`already` lists
    /// those hit by this swing).
    pub fn player_strike(&mut self, world: &mut ScriptWorld, host: &mut StdHost, env: &Env, spheres: &[(Vec3, f32)], ratio: f32, already: &mut Vec<EntityId>) -> Vec<Impact> {
        let Some(player) = world.player else { return Vec::new() };
        let mut cx = Ctx { world, host, env, dt_ms: 0.0 };
        let mut out = Vec::new();
        for i in 0..self.npcs.len() {
            let n = &self.npcs[i];
            if n.dead || n.life <= 0.0 || already.contains(&n.id) || cx.host.state(n.id).is_none_or(|s| s.hidden || s.destroyed || s.in_inventory) {
                continue;
            }
            let (id, pos, radius, height) = (n.id, n.pos, n.radius, n.height);
            // Sphere against the character's cylinder (feet at `pos`, up is -Y).
            let touch = spheres.iter().find(|(c, r)| {
                let d = Vec2::new(c.x - pos.x, c.z - pos.z).length();
                d <= radius + r && c.y <= pos.y + r && c.y >= pos.y - height - r
            });
            let Some(&(centre, _)) = touch else { continue };
            already.push(id);
            let at = Vec3::new(pos.x, centre.y.clamp(pos.y - height, pos.y), pos.z);
            out.push(self.compute_damages(&mut cx, player, id, ratio, at));
        }
        out
    }

    /// Hurt a character directly (a trap, a spell, a script): `damage` life, from `source`.
    pub fn hurt(&mut self, world: &mut ScriptWorld, host: &mut StdHost, env: &Env, target: EntityId, damage: f32, source: Option<EntityId>) -> f32 {
        let mut cx = Ctx { world, host, env, dt_ms: 0.0 };
        self.damage_npc(&mut cx, target, damage, source, "")
    }

    /// Kill a character at once (`forcedeath`).
    pub fn kill(&mut self, world: &mut ScriptWorld, host: &mut StdHost, env: &Env, target: EntityId, killer: Option<EntityId>) {
        let Some(i) = self.by_id.get(&target).copied() else { return };
        let mut cx = Ctx { world, host, env, dt_ms: 0.0 };
        self.kill_npc(i, &mut cx, killer);
    }

    // ---------------------------------------------------------------------------- the characters' blows

    /// A fighting character close to its target prepares, aims and lands blows. Called while it stands near the target.
    /// Returns whether it is busy attacking (and so should not walk).
    pub(super) fn attack(&mut self, i: usize, cx: &mut Ctx) -> bool {
        let id = self.npcs[i].id;
        let b = self.npcs[i].behavior;
        let Target::Entity(target) = self.npcs[i].target else {
            self.npcs[i].attack = Attack::default();
            return false;
        };
        if b & behavior::FIGHT == 0 || b & behavior::FLEE != 0 {
            self.npcs[i].attack = Attack::default();
            return false;
        }
        let dist = self.npcs[i].pos.distance(Self::entity_pos(cx, target));
        let kind = self.weapon_kind_of(cx.host, id);
        let prefix = kind.prefix();
        let slot_of = |dir: usize, suffix: &str| format!("{prefix}_strike_{}{suffix}", STRIKE_DIRECTIONS[dir]);
        let now = self.now_ms;
        let stage = self.npcs[i].attack.stage;
        match stage {
            None => {
                if dist < STRIKE_DISTANCE && !self.npcs[i].layer.force && matches!(self.npcs[i].layer.slot.as_deref(), None | Some("wait" | "fight_wait")) {
                    let dir = self.rng.range(0, 3) as usize;
                    let slot = slot_of(dir, "_start");
                    if cx.has_slot(id, &slot) {
                        self.change_anim(i, cx, &slot, false);
                        self.npcs[i].attack = Attack { stage: Some((AttackStage::Start, dir)), started_ms: now, landed: false };
                        return true;
                    }
                }
                false
            }
            Some((AttackStage::Start, dir)) => {
                if self.npcs[i].layer.ended {
                    let slot = slot_of(dir, "_cycle");
                    if cx.has_slot(id, &slot) {
                        self.change_anim(i, cx, &slot, true);
                    }
                    self.npcs[i].attack = Attack { stage: Some((AttackStage::Cycle, dir)), started_ms: now, landed: false };
                }
                true
            }
            Some((AttackStage::Cycle, dir)) => {
                let aim = f64::from(self.npcs[i].stats.aim_time_ms);
                let elapsed = now - self.npcs[i].attack.started_ms;
                let ready = elapsed > aim || (elapsed * 2.0 > aim && self.rand() > 0.9);
                if ready && dist < STRIKE_DISTANCE {
                    let slot = slot_of(dir, "");
                    if cx.has_slot(id, &slot) {
                        self.change_anim(i, cx, &slot, false);
                    }
                    self.npcs[i].attack = Attack { stage: Some((AttackStage::Strike, dir)), started_ms: now, landed: false };
                    cx.send(None, id, "strike", vec![kind.event_name().to_owned()]);
                } else if dist >= STRIKE_DISTANCE * 1.5 {
                    // The target walked away: give up the blow.
                    self.npcs[i].attack = Attack::default();
                    self.change_anim(i, cx, "fight_wait", true);
                }
                true
            }
            Some((AttackStage::Strike, _)) => {
                let (time, duration) = {
                    let l = &self.npcs[i].layer;
                    (l.time_us as f64, l.track.as_ref().map_or(1.0, |t| t.duration_us.max(1) as f64))
                };
                let ended = self.npcs[i].layer.ended;
                if !self.npcs[i].attack.landed && !ended && time > duration * 0.25 && time <= duration * 0.8 {
                    self.npcs[i].attack.landed = true;
                    self.land_blow(i, cx, target);
                }
                if ended || !self.npcs[i].layer.slot.as_deref().is_some_and(|s| s.contains("strike")) {
                    self.npcs[i].attack = Attack::default();
                    if cx.has_slot(id, "fight_wait") {
                        self.change_anim(i, cx, "fight_wait", true);
                    } else {
                        self.change_anim(i, cx, "wait", false);
                    }
                    return false;
                }
                true
            }
        }
    }

    /// The point a character's blow reaches (about 90 units in front of its chest) against its target.
    fn land_blow(&mut self, i: usize, cx: &mut Ctx, target: EntityId) {
        let n = &self.npcs[i];
        let f = facing(n.yaw);
        let point = Vec3::new(n.pos.x + f.x * 90.0, n.pos.y - 80.0, n.pos.z + f.y * 90.0);
        let (reach, radius, id) = (n.stats.reach, n.radius, n.id);
        let tpos = Self::entity_pos(cx, target);
        let t_radius = if Some(target) == cx.world.player { PLAYER_RADIUS } else { self.npc(target).map_or(30.0, |t| t.radius) };
        // Level with the target, and within reach of it.
        if (tpos.y - n.pos.y).abs() > 170.0 {
            return;
        }
        let d = Vec2::new(point.x - tpos.x, point.z - tpos.z).length();
        if d <= reach + radius + t_radius {
            self.compute_damages(cx, id, target, 1.0, Vec3::new(tpos.x, tpos.y - 80.0, tpos.z));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weapon_kinds_come_from_object_types() {
        assert_eq!(WeaponKind::from_flags(0), WeaponKind::Bare);
        assert_eq!(WeaponKind::from_flags(object_type::WEAPON | object_type::DAGGER), WeaponKind::Dagger);
        assert_eq!(WeaponKind::from_flags(object_type::WEAPON | object_type::ONE_HANDED), WeaponKind::OneHanded);
        assert_eq!(WeaponKind::from_flags(object_type::TWO_HANDED), WeaponKind::TwoHanded);
        assert_eq!(WeaponKind::OneHanded.prefix(), "1h");
        assert_eq!(WeaponKind::Bow.event_name(), "arrow");
    }
}
