//! What non-player characters do: follow a path over the level's anchors, walk with the speed of their walking
//! animation, turn toward where they are going, pause on patrols, flee, look for someone, notice the player (and hear
//! them), and tell their scripts about it (`reachedtarget`, `lostTarget`, `detectplayer`, `hear`, ...).
//!
//! Ported from the engine's `ManageNPCMovement` and `ARX_NPC_LaunchPathfind`. Positions are in Arx coordinates
//! (+Y down) as everywhere in the script world; the collision world is y-up, so movement converts at the boundary.

mod combat;

pub use combat::{CombatSound, Impact, STRIKE_DIRECTIONS, WeaponKind, player_weapon_kind};

use crate::anchors::{AnchorGraph, Body, Rng, heuristic_for};
use crate::to_yup;
use arx_formats::tea::Tea;
use arx_physics::{CollisionWorld, CylinderId, PLAYER_RADIUS};
use arx_script::{EntityId, EntityKind, NpcRequest, PlayAnim, ScriptWorld, StdHost, TargetSpec};
use glam::{Vec2, Vec3};
use std::collections::HashMap;
use std::sync::Arc;

/// `behavior` flags (the engine's `BEHAVIOUR_*`).
pub mod behavior {
    pub const NONE: u32 = 1 << 0;
    pub const FRIENDLY: u32 = 1 << 1;
    pub const MOVE_TO: u32 = 1 << 2;
    pub const WANDER_AROUND: u32 = 1 << 3;
    pub const FLEE: u32 = 1 << 4;
    pub const HIDE: u32 = 1 << 5;
    pub const LOOK_FOR: u32 = 1 << 6;
    pub const SNEAK: u32 = 1 << 7;
    pub const FIGHT: u32 = 1 << 8;
    pub const DISTANT: u32 = 1 << 9;
    pub const MAGIC: u32 = 1 << 10;
    pub const GUARD: u32 = 1 << 11;
    pub const GO_HOME: u32 = 1 << 12;
    pub const LOOK_AROUND: u32 = 1 << 13;
    pub const STARE_AT: u32 = 1 << 14;

    /// Behaviours that make a character walk somewhere.
    pub const TRAVELS: u32 = MOVE_TO | GO_HOME | WANDER_AROUND | FLEE | HIDE | LOOK_FOR;
}

/// Beyond this a moving character walks instead of runs, and a fighter walks forward instead of running.
pub const RUN_WALK_RADIUS: f32 = 450.0;
/// How close a fighter has to be to start a blow.
pub const STRIKE_DISTANCE: f32 = 220.0;
/// Degrees a character can turn per millisecond.
const TURN_PER_MS: f32 = 0.33;
/// Characters farther than this from the player are not simulated.
pub const ACTIVE_RANGE: f32 = 5000.0;
/// How far a character sees.
const SIGHT_RANGE: f32 = 2000.0;
/// Half the angle of the cone a character sees in, degrees.
const SIGHT_HALF_ANGLE: f32 = 110.0;
/// Contact range at which the player is noticed regardless of facing or light.
const NOTICE_CONTACT: f32 = 15.0;
/// How far a footstep carries (divided by the sneaking factor).
pub const HEAR_STEP_DISTANCE: f32 = 600.0;
pub const HEAR_ITEM_DISTANCE: f32 = 800.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveMode {
    None,
    Walk,
    Run,
    Sneak,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// `TARGET_NONE`
    None,
    Entity(EntityId),
    /// Its patrol path.
    Path,
}

/// The root translation of an animation over time: what moves a walking character.
#[derive(Debug, Clone, Default)]
pub struct MoveTrack {
    pub duration_us: i64,
    keys: Vec<(i64, Vec3)>,
    /// Times (microseconds) at which a foot comes down (the animation's `step_sound` keyframes).
    steps: Vec<i64>,
}

impl MoveTrack {
    pub fn from_tea(t: &Tea) -> Self {
        MoveTrack {
            duration_us: t.duration_us,
            keys: t.frames.iter().map(|f| (f.time_us, f.translate)).collect(),
            steps: t.frames.iter().filter(|f| f.step_sound).map(|f| f.time_us).collect(),
        }
    }

    /// The same with foot-fall times (for tests).
    pub fn with_steps(mut self, steps: Vec<i64>) -> Self {
        self.steps = steps;
        self
    }

    /// How many foot-falls lie in `(from, to]`, wrapping at the end of the animation when `looping`.
    fn steps_between(&self, from: i64, to: i64) -> u32 {
        let count = |a: i64, b: i64| self.steps.iter().filter(|&&s| s > a && s <= b).count() as u32;
        if to >= self.duration_us.max(1) && to > from {
            count(from, self.duration_us) + count(-1, to - self.duration_us.max(1))
        } else {
            count(from, to)
        }
    }

    /// Build from `(time in microseconds, translation)` keys (for tests).
    pub fn from_keys(duration_us: i64, keys: Vec<(i64, Vec3)>) -> Self {
        MoveTrack { duration_us, keys, steps: Vec::new() }
    }

    /// Where the animation has moved the object `time_us` in (Arx coordinates, object space).
    pub fn at(&self, time_us: i64) -> Vec3 {
        if self.keys.len() < 2 {
            return Vec3::ZERO;
        }
        let t = time_us.clamp(0, self.keys.last().map_or(0, |k| k.0));
        for w in self.keys.windows(2) {
            let ((t0, a), (t1, b)) = (w[0], w[1]);
            if t >= t0 && t <= t1 {
                return if t1 == t0 { b } else { a.lerp(b, (t - t0) as f32 / (t1 - t0) as f32) };
            }
        }
        self.keys.last().map_or(Vec3::ZERO, |k| k.1)
    }

    pub fn total(&self) -> Vec3 {
        self.keys.last().map_or(Vec3::ZERO, |k| k.1)
    }
}

/// Finds an animation's root translation by its file path.
pub type TrackLoader = Box<dyn Fn(&str) -> Option<MoveTrack> + Send + Sync>;

/// What a character carries out in combat, from `setnpcstat`.
#[derive(Debug, Clone)]
pub struct NpcStats {
    pub armor_class: f32,
    pub absorb: f32,
    pub damages: f32,
    pub tohit: f32,
    pub reach: f32,
    pub critical: f32,
    pub aim_time_ms: f32,
    pub backstab_skill: f32,
    pub backstab: bool,
    pub resist_fire: f32,
    pub resist_poison: f32,
    pub resist_magic: f32,
    pub mana: f32,
    pub xp_value: f32,
}

impl Default for NpcStats {
    fn default() -> Self {
        NpcStats {
            armor_class: 0.0,
            absorb: 0.0,
            damages: 20.0,
            tohit: 50.0,
            reach: 20.0,
            critical: 5.0,
            aim_time_ms: 0.0,
            backstab_skill: 0.0,
            backstab: false,
            resist_fire: 0.0,
            resist_poison: 0.0,
            resist_magic: 0.0,
            mana: 10.0,
            xp_value: 0.0,
        }
    }
}

/// How a character's current animation is playing (the engine's layer 0).
#[derive(Debug, Clone, Default)]
struct Layer {
    slot: Option<String>,
    track: Option<Arc<MoveTrack>>,
    time_us: i64,
    looping: bool,
    ended: bool,
    /// Played by a script (`playanim`): the character's own logic waits for it to end.
    force: bool,
    /// Foot-falls the last [`Layer::advance`] passed.
    steps: u32,
}

impl Layer {
    fn is(&self, slot: &str) -> bool {
        self.slot.as_deref() == Some(slot)
    }

    fn is_walking(&self) -> bool {
        matches!(self.slot.as_deref(), Some("walk" | "run" | "walk_sneak" | "fight_walk_forward"))
    }

    /// Advance by `dt_us`; the translation the animation made in that time (object space, Arx coordinates).
    fn advance(&mut self, dt_us: i64) -> Vec3 {
        self.steps = 0;
        let Some(track) = self.track.clone() else {
            if !self.looping {
                self.ended = true;
            }
            return Vec3::ZERO;
        };
        if self.ended && !self.looping {
            return Vec3::ZERO;
        }
        let duration = track.duration_us.max(1);
        let prev = self.time_us;
        let next = prev + dt_us;
        self.steps = track.steps_between(prev, next);
        if next >= duration {
            if self.looping {
                self.time_us = next % duration;
                (track.total() - track.at(prev)) + (track.at(self.time_us) - track.at(0))
            } else {
                self.time_us = duration;
                self.ended = true;
                track.at(duration) - track.at(prev)
            }
        } else {
            self.time_us = next;
            track.at(next) - track.at(prev)
        }
    }
}

/// A route over the anchors.
#[derive(Debug, Clone)]
struct Route {
    list: Vec<usize>,
    pos: usize,
    /// The engine's `listnb`: the number of anchors, `-1` none, `-2` the search failed.
    nb: i32,
    true_target: Target,
    /// Re-search while the target moves (`settarget -a`), once only (`-s`), or never (`-n`).
    always: bool,
    once: bool,
    no_update: bool,
}

impl Route {
    fn none() -> Self {
        Route { list: Vec::new(), pos: 0, nb: -1, true_target: Target::None, always: false, once: false, no_update: false }
    }

    fn clear(&mut self) {
        self.list.clear();
        self.pos = 0;
        self.nb = -1;
    }
}

#[derive(Debug, Clone)]
struct Saved {
    behavior: u32,
    param: f32,
    target: Target,
    mode: MoveMode,
}

#[derive(Debug, Clone)]
pub struct Npc {
    pub id: EntityId,
    /// Feet, Arx coordinates.
    pub pos: Vec3,
    pub init_pos: Vec3,
    /// Stored NPC yaw in degrees (what `entity_rotation(.., true)` takes).
    pub yaw: f32,
    pub radius: f32,
    pub height: f32,
    cylinder: Option<CylinderId>,
    pub behavior: u32,
    pub behavior_param: f32,
    pub move_mode: MoveMode,
    stack: Vec<Saved>,
    /// Who or what the character is heading for or looking at (`targetinfo`).
    pub target: Target,
    target_pos: Vec3,
    route: Route,
    pub reached: bool,
    reached_at_ms: f64,
    move_problem: i32,
    layer: Layer,
    seen_serial: u32,
    pub life: f32,
    pub max_life: f32,
    pub stats: NpcStats,
    /// Has noticed the player.
    pub detect: bool,
    /// Animation speed factor (`setspeed`).
    pub speed: f32,
    pub dead: bool,
    on_ground: bool,
    attack: combat::Attack,
    /// When it last cried out (`ouch`), and the damage taken since.
    ouch_at_ms: f64,
    dmg_sum: f32,
    /// A push from a blow it took, still to be walked off (Arx units).
    shove: Vec3,
}

impl Npc {
    pub fn new(id: EntityId, pos: Vec3, yaw: f32) -> Self {
        Npc {
            id,
            pos,
            init_pos: pos,
            yaw,
            radius: 30.0,
            height: 170.0,
            cylinder: None,
            behavior: behavior::NONE,
            behavior_param: 0.0,
            move_mode: MoveMode::Walk,
            stack: Vec::new(),
            target: Target::None,
            target_pos: pos,
            route: Route::none(),
            reached: false,
            reached_at_ms: 0.0,
            move_problem: 0,
            layer: Layer::default(),
            seen_serial: 0,
            life: 20.0,
            max_life: 20.0,
            stats: NpcStats::default(),
            detect: false,
            speed: 1.0,
            dead: false,
            on_ground: true,
            attack: combat::Attack::default(),
            ouch_at_ms: -10_000.0,
            dmg_sum: 0.0,
            shove: Vec3::ZERO,
        }
    }

    /// The animation slot playing now.
    pub fn animation(&self) -> Option<&str> {
        self.layer.slot.as_deref()
    }

    /// There is floor under the character.
    pub fn on_ground(&self) -> bool {
        self.on_ground
    }

    pub fn is_traveling(&self) -> bool {
        self.route.nb > 0
    }

    fn body(&self) -> Body {
        Body { radius: self.radius, height: self.height }
    }
}

/// What the characters need to know about the world around them.
pub struct Env<'a> {
    pub collision: &'a CollisionWorld,
    /// The player's feet (Arx coordinates).
    pub player_pos: Vec3,
    pub player_alive: bool,
    /// How well the player hides (`15 + stealth / 10`): characters only notice a player lit more than this.
    pub player_stealth: f32,
    /// How brightly the player is lit (0..255).
    pub player_light: f32,
    /// The player carries a lit torch.
    pub player_torch: bool,
}

struct Ctx<'a> {
    world: &'a mut ScriptWorld,
    host: &'a mut StdHost,
    env: &'a Env<'a>,
    dt_ms: f32,
}

impl Ctx<'_> {
    fn send(&mut self, sender: Option<EntityId>, target: EntityId, event: &str, params: Vec<String>) {
        self.world.send_event(self.host, sender, target, event, params);
    }

    fn has_slot(&self, id: EntityId, slot: &str) -> bool {
        self.host.state(id).is_some_and(|s| s.anims.contains_key(slot))
    }
}

pub struct NpcWorld {
    graph: Arc<AnchorGraph>,
    npcs: Vec<Npc>,
    by_id: HashMap<EntityId, usize>,
    tracks: HashMap<String, Option<Arc<MoveTrack>>>,
    loader: Option<TrackLoader>,
    rng: Rng,
    now_ms: f64,
    /// Characters farther than this from the player stand still.
    pub active_range: f32,
    /// Print the commands given to characters whose id contains this text (diagnostics).
    pub log: Option<String>,
    /// Which solid entity each collision obstacle belongs to, so a character bumping a door can tell it.
    obstacle_owner: HashMap<usize, EntityId>,
    /// When a door last reported a collision (milliseconds), so it is told at most twice a second.
    door_bumps: HashMap<EntityId, f64>,
    sounds: Vec<CombatSound>,
    footsteps: Vec<Footstep>,
}

/// A character's foot came down.
#[derive(Debug, Clone, PartialEq)]
pub struct Footstep {
    pub id: EntityId,
    /// Where (Arx coordinates).
    pub pos: Vec3,
}

/// Yaw (Arx degrees, as stored for NPCs) of a character facing along `(dx, dz)` in Arx coordinates.
///
/// Walking animations move a model along its object-space -Z, and the engine turns that by `180 - yaw` about the
/// vertical axis, so a character with yaw 0 walks toward +Z, yaw 90 toward -X, yaw 180 toward -Z and yaw 270 toward +X.
pub fn yaw_toward(dx: f32, dz: f32) -> f32 {
    (-dx).atan2(dz).to_degrees().rem_euclid(360.0)
}

/// The unit vector (x, z) a character with this yaw faces, in Arx coordinates.
pub fn facing(yaw: f32) -> Vec2 {
    let r = yaw.to_radians();
    Vec2::new(-r.sin(), r.cos())
}

/// The engine's `VRotateY`.
fn rotate_y(v: Vec3, degrees: f32) -> Vec3 {
    let (s, c) = degrees.to_radians().sin_cos();
    Vec3::new(v.x * c + v.z * s, v.y, v.z * c - v.x * s)
}

fn angle_diff(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(360.0);
    if d > 180.0 { 360.0 - d } else { d }
}

fn distance_xz(a: Vec3, b: Vec3) -> f32 {
    Vec2::new(a.x - b.x, a.z - b.z).length()
}

impl NpcWorld {
    pub fn new(graph: Arc<AnchorGraph>) -> Self {
        NpcWorld { graph, npcs: Vec::new(), by_id: HashMap::new(), tracks: HashMap::new(), loader: None, rng: Rng(0x9E37_79B9), now_ms: 0.0, active_range: ACTIVE_RANGE, log: None, obstacle_owner: HashMap::new(), door_bumps: HashMap::new(), sounds: Vec::new(), footsteps: Vec::new() }
    }

    /// Tell the world which collision obstacle belongs to which entity.
    pub fn set_obstacle_owners(&mut self, owners: impl IntoIterator<Item = (arx_physics::ObstacleId, EntityId)>) {
        self.obstacle_owner = owners.into_iter().map(|(o, e)| (o.0, e)).collect();
    }

    /// Footsteps taken since the last call.
    pub fn take_footsteps(&mut self) -> Vec<Footstep> {
        std::mem::take(&mut self.footsteps)
    }

    pub fn set_track_loader(&mut self, f: TrackLoader) {
        self.loader = Some(f);
    }

    /// Register a character. `cylinder` is its collision cylinder (id, radius, height).
    pub fn add(&mut self, id: EntityId, pos: Vec3, yaw: f32, cylinder: Option<(CylinderId, f32, f32)>) {
        let mut npc = Npc::new(id, pos, yaw);
        if let Some((c, r, h)) = cylinder {
            npc.cylinder = Some(c);
            npc.radius = r;
            npc.height = h;
        }
        self.by_id.insert(id, self.npcs.len());
        self.npcs.push(npc);
    }

    pub fn npc(&self, id: EntityId) -> Option<&Npc> {
        self.by_id.get(&id).map(|&i| &self.npcs[i])
    }

    pub fn npc_mut(&mut self, id: EntityId) -> Option<&mut Npc> {
        self.by_id.get(&id).copied().map(|i| &mut self.npcs[i])
    }

    pub fn iter(&self) -> impl Iterator<Item = &Npc> {
        self.npcs.iter()
    }

    /// The anchors of a character's current route and which one it is heading for (diagnostics).
    pub fn route_of(&self, id: EntityId) -> Option<(Vec<Vec3>, usize, Vec3)> {
        let n = self.npc(id)?;
        Some((n.route.list.iter().map(|&a| self.graph.anchors[a].pos).collect(), n.route.pos, n.target_pos))
    }

    pub fn graph(&self) -> &AnchorGraph {
        &self.graph
    }

    fn track(&mut self, path: &str) -> Option<Arc<MoveTrack>> {
        if !self.tracks.contains_key(path) {
            let t = self.loader.as_ref().and_then(|f| f(path)).map(Arc::new);
            self.tracks.insert(path.to_owned(), t);
        }
        self.tracks[path].clone()
    }

    // ------------------------------------------------------------------------------------------ requests

    fn apply(&mut self, cx: &mut Ctx, r: NpcRequest) {
        if let Some(filter) = &self.log {
            let who = match &r {
                NpcRequest::Behavior { entity, .. } | NpcRequest::SetTarget { entity, .. } | NpcRequest::MoveMode { entity, .. } | NpcRequest::Pathfind { entity, .. } => Some(*entity),
                NpcRequest::ForceDeath { target, .. } => Some(*target),
                _ => None,
            };
            if let Some(e) = who.filter(|&e| cx.world.entity(e).id_string.contains(filter.as_str())) {
                eprintln!("[{:.1}s] {}: {r:?}", self.now_ms / 1000.0, cx.world.entity(e).id_string);
            }
        }
        match r {
            NpcRequest::Behavior { entity, flags, command, param } => {
                let Some(&i) = self.by_id.get(&entity) else { return };
                self.behavior_command(i, cx, &flags, &command, param);
            }
            NpcRequest::SetTarget { entity, flags, target } => {
                if let Some(&i) = self.by_id.get(&entity) {
                    self.set_target(i, cx, &flags, target);
                }
            }
            NpcRequest::MoveMode { entity, mode } => {
                let Some(&i) = self.by_id.get(&entity) else { return };
                let m = match mode.as_str() {
                    "walk" => MoveMode::Walk,
                    "run" => MoveMode::Run,
                    "sneak" => MoveMode::Sneak,
                    "none" => MoveMode::None,
                    _ => return,
                };
                self.change_move_mode(i, cx, m);
            }
            NpcRequest::Stat { entity, name, value } => {
                if let Some(n) = self.npc_mut(entity) {
                    n.set_stat(&name, value);
                }
            }
            NpcRequest::Detect { .. } => {}
            NpcRequest::Speed { entity, value } => {
                if let Some(n) = self.npc_mut(entity) {
                    n.speed = value;
                }
            }
            NpcRequest::XpValue { entity, value } => {
                if let Some(n) = self.npc_mut(entity) {
                    n.stats.xp_value = value;
                }
            }
            NpcRequest::Life { entity, value } => {
                if let Some(n) = self.npc_mut(entity) {
                    n.life = value;
                    n.max_life = value;
                }
            }
            NpcRequest::Revive { entity, init } => {
                if let Some(&i) = self.by_id.get(&entity) {
                    self.revive(i, cx, init);
                }
            }
            NpcRequest::ForceDeath { target, killer } => {
                if let Some(&i) = self.by_id.get(&target) {
                    self.kill_npc(i, cx, Some(killer));
                }
            }
            NpcRequest::Pathfind { entity, target } => {
                if let Some(&i) = self.by_id.get(&entity) {
                    self.launch_pathfind(i, cx, target.map_or(Target::None, Target::Entity));
                }
            }
        }
    }

    fn behavior_command(&mut self, i: usize, cx: &mut Ctx, flags: &str, command: &str, param: f32) {
        use arx_script::has_flag;
        let mut b = 0;
        for (letter, bit) in [('l', behavior::LOOK_AROUND), ('s', behavior::SNEAK), ('d', behavior::DISTANT), ('m', behavior::MAGIC), ('f', behavior::FIGHT), ('a', behavior::STARE_AT)] {
            if has_flag(flags, letter) {
                b |= bit;
            }
        }
        if flags.is_empty() {
            match command {
                "stack" => return self.stack_behavior(i),
                "unstack" => return self.unstack_behavior(i, cx),
                "unstackall" => {
                    self.npcs[i].behavior = behavior::NONE;
                    self.npcs[i].stack.clear();
                    return;
                }
                _ => {}
            }
        }
        let mut param_used = 0.0;
        let n = &mut self.npcs[i];
        match command {
            "go_home" => b |= behavior::GO_HOME,
            "friendly" => {
                n.move_mode = MoveMode::None;
                b |= behavior::FRIENDLY;
            }
            "move_to" => {
                n.move_mode = MoveMode::Walk;
                b |= behavior::MOVE_TO;
            }
            "flee" => {
                param_used = param;
                n.move_mode = MoveMode::Run;
                b |= behavior::FLEE;
            }
            "look_for" => {
                param_used = param;
                n.move_mode = MoveMode::Walk;
                b |= behavior::LOOK_FOR;
            }
            "hide" => {
                param_used = param;
                n.move_mode = MoveMode::Walk;
                b |= behavior::HIDE;
            }
            "wander_around" => {
                param_used = param;
                n.move_mode = MoveMode::Walk;
                b |= behavior::WANDER_AROUND;
            }
            "guard" => {
                b |= behavior::GUARD;
                n.target = Target::None;
                n.move_mode = MoveMode::None;
            }
            _ => {}
        }
        self.change_behavior(i, cx, b, param_used);
    }

    /// `ARX_NPC_Behaviour_Change`
    fn change_behavior(&mut self, i: usize, cx: &mut Ctx, b: u32, param: f32) {
        let id = self.npcs[i].id;
        if b == 0 || b & behavior::NONE != 0 {
            // Back to idling: stop whatever was walking.
            self.npcs[i].layer.looping = false;
            if cx.has_slot(id, "wait") && !self.npcs[i].layer.force {
                self.change_anim(i, cx, "wait", false);
            }
        }
        if b & behavior::FRIENDLY != 0 && cx.has_slot(id, "wait") {
            self.change_anim(i, cx, "wait", false);
        }
        let n = &mut self.npcs[i];
        n.behavior = b;
        n.behavior_param = param;
    }

    fn stack_behavior(&mut self, i: usize) {
        let n = &mut self.npcs[i];
        if n.stack.len() >= 5 {
            return;
        }
        let target = if n.route.nb > 0 { n.route.true_target } else { n.target };
        n.stack.push(Saved { behavior: n.behavior, param: n.behavior_param, target, mode: n.move_mode });
    }

    fn unstack_behavior(&mut self, i: usize, cx: &mut Ctx) {
        let Some(s) = self.npcs[i].stack.pop() else { return };
        let n = &mut self.npcs[i];
        n.behavior = s.behavior;
        n.behavior_param = s.param;
        n.target = s.target;
        n.move_mode = s.mode;
        self.launch_pathfind(i, cx, s.target);
    }

    fn change_move_mode(&mut self, i: usize, cx: &mut Ctx, mode: MoveMode) {
        let id = self.npcs[i].id;
        let l = &self.npcs[i].layer;
        let (walk, run, sneak) = (l.is("walk"), l.is("run"), l.is("walk_sneak"));
        match mode {
            MoveMode::Run if (walk || sneak) && cx.has_slot(id, "run") => self.change_anim(i, cx, "run", true),
            MoveMode::Walk if (run || sneak) && cx.has_slot(id, "walk") => self.change_anim(i, cx, "walk", true),
            MoveMode::None if (walk || run || sneak) && cx.has_slot(id, "wait") => self.change_anim(i, cx, "wait", false),
            MoveMode::Sneak if (walk || run) && cx.has_slot(id, "walk_sneak") => self.change_anim(i, cx, "walk_sneak", true),
            _ => {}
        }
        self.npcs[i].move_mode = mode;
    }

    fn set_target(&mut self, i: usize, cx: &mut Ctx, flags: &str, target: TargetSpec) {
        use arx_script::has_flag;
        let n = &mut self.npcs[i];
        n.route.always = has_flag(flags, 'a');
        n.route.once = has_flag(flags, 's');
        n.route.no_update = has_flag(flags, 'n');
        let mut old = None;
        if n.reached {
            old = Some(n.target);
        }
        if n.behavior & (behavior::FLEE | behavior::WANDER_AROUND) != 0 {
            old = None;
        }
        let new = match target {
            TargetSpec::None => Target::None,
            TargetSpec::Path => Target::Path,
            TargetSpec::Entity(e) => Target::Entity(e),
        };
        n.target = new;
        self.update_target_pos(i, cx);
        if old != Some(new) {
            self.npcs[i].reached = false;
            self.launch_pathfind(i, cx, new);
        }
    }

    fn revive(&mut self, i: usize, cx: &mut Ctx, init: bool) {
        let id = self.npcs[i].id;
        let n = &mut self.npcs[i];
        n.life = n.max_life;
        n.dead = false;
        n.layer = Layer::default();
        n.behavior = behavior::NONE;
        n.route = Route::none();
        n.reached = false;
        if init {
            n.pos = n.init_pos;
        }
        if let Some(c) = n.cylinder {
            cx.env.collision.set_cylinder(c, to_yup(n.pos.to_array()), true);
        }
        cx.host.modify(id, |s| {
            s.collision = true;
            s.playing = None;
        });
    }

}

impl Npc {
    fn set_stat(&mut self, name: &str, value: f32) -> bool {
        let v = value.max(0.0);
        match name {
            "armor_class" => self.stats.armor_class = v,
            "backstab_skill" | "backstabskill" => self.stats.backstab_skill = v,
            "backstab" => self.stats.backstab = value != 0.0,
            "reach" => self.stats.reach = v,
            "critical" => self.stats.critical = v,
            "absorb" => self.stats.absorb = v,
            "damages" => self.stats.damages = v,
            "tohit" => self.stats.tohit = v,
            "aimtime" => self.stats.aim_time_ms = v,
            "life" => {
                let l = if value < 0.0 { 1e-7 } else { value };
                self.life = l;
                self.max_life = l;
            }
            "mana" => self.stats.mana = v,
            "resistfire" => self.stats.resist_fire = value.clamp(0.0, 100.0),
            "resistpoison" => self.stats.resist_poison = value.clamp(0.0, 100.0),
            "resistmagic" => self.stats.resist_magic = value.clamp(0.0, 100.0),
            _ => return false,
        }
        true
    }
}

impl NpcWorld {
    // -------------------------------------------------------------------------------------- animations

    /// Start `slot` from its beginning (the engine's `changeAnimation`).
    fn change_anim(&mut self, i: usize, cx: &mut Ctx, slot: &str, looping: bool) {
        let id = self.npcs[i].id;
        let Some(path) = cx.host.state(id).and_then(|s| s.anims.get(slot)).cloned() else { return };
        let track = self.track(&path);
        let n = &mut self.npcs[i];
        n.layer = Layer { slot: Some(slot.to_owned()), track, time_us: 0, looping, ended: false, force: false, steps: 0 };
        cx.host.modify(id, |s| {
            s.playing = Some(PlayAnim { slot: slot.to_owned(), looping });
            s.anim_serial += 1;
        });
        n.seen_serial = cx.host.state(id).map_or(0, |s| s.anim_serial);
    }

    /// Start `slot` unless it is already playing (`setAnimation`).
    fn set_anim(&mut self, i: usize, cx: &mut Ctx, slot: &str, looping: bool) {
        if !self.npcs[i].layer.is(slot) {
            self.change_anim(i, cx, slot, looping);
        }
    }

    /// A script started an animation on the character (`playanim`): take it over and wait for it.
    fn adopt_script_anim(&mut self, i: usize, cx: &mut Ctx) {
        let id = self.npcs[i].id;
        let Some(st) = cx.host.state(id) else { return };
        let serial = st.anim_serial;
        if serial == self.npcs[i].seen_serial {
            return;
        }
        let playing = st.playing.clone();
        let path = playing.as_ref().and_then(|p| st.anims.get(&p.slot)).cloned();
        let track = path.and_then(|p| self.track(&p));
        let n = &mut self.npcs[i];
        n.seen_serial = serial;
        n.layer = match playing {
            Some(p) => Layer { slot: Some(p.slot), track, time_us: 0, looping: p.looping, ended: false, force: true, steps: 0 },
            None => Layer::default(),
        };
    }

    // --------------------------------------------------------------------------------------- targeting

    fn entity_pos(cx: &Ctx, id: EntityId) -> Vec3 {
        Vec3::from(cx.world.entity(id).pos)
    }

    /// `GetTargetPos`: where the character is heading right now.
    fn update_target_pos(&mut self, i: usize, cx: &Ctx) {
        let n = &mut self.npcs[i];
        if n.behavior & behavior::NONE != 0 {
            n.target_pos = n.pos;
            return;
        }
        let on_path = n.route.nb != -1 && !n.route.list.is_empty() && n.behavior & behavior::FRIENDLY == 0;
        if n.behavior & behavior::GO_HOME != 0 {
            n.target_pos = if n.route.pos < n.route.nb.max(0) as usize && !n.route.list.is_empty() {
                self.graph.anchors[n.route.list[n.route.pos]].pos
            } else {
                n.init_pos
            };
            return;
        }
        if on_path {
            if n.route.pos < n.route.nb.max(0) as usize {
                n.target_pos = self.graph.anchors[n.route.list[n.route.pos]].pos;
            } else if let Target::Entity(e) = n.route.true_target {
                n.target_pos = Self::entity_pos(cx, e);
            }
            return;
        }
        n.target_pos = match n.target {
            Target::Entity(e) => Self::entity_pos(cx, e),
            _ => n.pos,
        };
    }

    /// The distance to the final target of the current route.
    fn true_target_distance(&self, i: usize, cx: &Ctx) -> f32 {
        let n = &self.npcs[i];
        if n.behavior & behavior::GO_HOME != 0 {
            return n.pos.distance(n.init_pos);
        }
        match n.route.true_target {
            Target::Entity(e) => n.pos.distance(Self::entity_pos(cx, e)),
            _ => 99_999_999.0,
        }
    }

    /// How near is near enough (`ComputeTolerance`).
    fn tolerance(&self, i: usize, cx: &Ctx, target: Target) -> f32 {
        let n = &self.npcs[i];
        let mut t = 30.0;
        if let Target::Entity(e) = target {
            let target_radius = if Some(e) == cx.world.player {
                PLAYER_RADIUS
            } else {
                self.npc(e).map_or(25.0, |o| o.radius)
            };
            t = (target_radius + n.radius + 5.0).max(0.0);
            let kind = cx.world.entity(e).kind;
            if kind == EntityKind::Fix {
                t += 100.0;
            }
            if matches!(kind, EntityKind::Npc | EntityKind::Item | EntityKind::Camera) {
                t += 20.0;
            }
            if n.target == Target::Entity(cx.world.player.unwrap_or(u32::MAX)) {
                t += 10.0;
            }
            if n.behavior & behavior::FIGHT != 0 {
                t += n.stats.reach * 0.7;
            }
            if n.behavior & (behavior::MAGIC | behavior::DISTANT) != 0 {
                t += 300.0;
            }
            if kind == EntityKind::Marker {
                t = 21.0 + n.move_problem as f32 * 0.1;
            }
        }
        t + n.move_problem as f32 * 0.1
    }

    // ------------------------------------------------------------------------------------- pathfinding

    /// `ARX_NPC_LaunchPathfind`: plan a route to `target` (the searches run at once here, where the engine queues
    /// them for a thread). Returns whether a route is under way.
    fn launch_pathfind(&mut self, i: usize, cx: &mut Ctx, target: Target) -> bool {
        let id = self.npcs[i].id;
        let n = &mut self.npcs[i];
        let old_target = n.target;
        if n.behavior & behavior::FRIENDLY != 0 {
            n.target = target;
            return false;
        }
        if n.route.nb > 0 {
            n.route.clear();
            n.route.true_target = Target::None;
        }
        if n.behavior & behavior::WANDER_AROUND != 0 {
            let pos2 = n.pos + Vec3::new(1000.0, 0.0, 1000.0);
            return self.launch_end(i, cx, target, pos2);
        }
        if matches!(target, Target::None | Target::Path) || n.behavior & behavior::GO_HOME != 0 {
            n.route.true_target = Target::Entity(id);
            let pos2 = if n.behavior & behavior::GO_HOME != 0 { n.init_pos } else { n.pos };
            return self.launch_end(i, cx, Target::Entity(id), pos2);
        }
        let Target::Entity(t) = target else { return false };
        if t == id {
            return false;
        }
        if old_target != target {
            n.reached = false;
        }
        let pos2 = Self::entity_pos(cx, t);
        n.route.true_target = target;
        // Close, level and in the open: no need for a route, walk straight there.
        if n.pos.distance(pos2) < 520.0
            && (n.pos.y - pos2.y).abs() < 50.0
            && n.behavior & behavior::MOVE_TO != 0
            && n.behavior & (behavior::SNEAK | behavior::FLEE) == 0
            && self.clear_line(cx.env.collision, self.npcs[i].pos, pos2)
        {
            return false;
        }
        self.launch_end(i, cx, target, pos2)
    }

    /// Is there nothing solid between the two points at waist height, and floor to walk on all the way?
    fn clear_line(&self, collision: &CollisionWorld, a: Vec3, b: Vec3) -> bool {
        let (a, b) = (to_yup(a.to_array()), to_yup(b.to_array()));
        let lift = Vec3::Y * 60.0;
        let d = (b + lift) - (a + lift);
        let len = d.length();
        if len < 1.0 {
            return true;
        }
        if collision.raycast(a + lift, d / len, len).is_some_and(|h| h.normal.y.abs() < 0.9) {
            return false;
        }
        let steps = (len / 50.0).ceil() as usize;
        (1..=steps).all(|k| {
            let p = a + d * (k as f32 / steps as f32);
            collision.floor_height(p.x, p.z, a.y + 60.0).is_some_and(|f| (f - a.y).abs() < 60.0)
        })
    }

    fn launch_end(&mut self, i: usize, cx: &mut Ctx, target: Target, pos2: Vec3) -> bool {
        let id = self.npcs[i].id;
        self.npcs[i].target = target;
        let n = &self.npcs[i];
        let b = n.behavior;
        let body = n.body();
        let wander = b & behavior::WANDER_AROUND != 0;
        let flee = b & (behavior::FLEE | behavior::HIDE) != 0;
        let from = if wander || b & behavior::FLEE != 0 {
            self.graph.nearest(n.pos, body, None)
        } else {
            // The engine looks 50 units ahead (with a copy-paste slip that uses x twice).
            let d = n.yaw.to_radians();
            let v = Vec3::new(-d.sin(), 0.0, d.cos());
            self.graph.nearest(n.pos + Vec3::new(v.x * 50.0, 0.0, v.x * 50.0), body, None)
        };
        let to = if b & behavior::FLEE != 0 {
            self.graph.nearest(pos2, body, from)
        } else if wander {
            from
        } else {
            self.graph.nearest(pos2, body, None)
        };
        if let (Some(from), Some(to)) = (from, to) {
            if from == to && !wander {
                return true;
            }
            let a = &self.graph.anchors;
            if wander || n.pos.distance(a[from].pos) < 200.0 {
                if !wander && !flee {
                    if a[from].pos.distance(a[to].pos) < 200.0 {
                        return false;
                    }
                    if pos2.distance(a[to].pos) > 200.0 {
                        return self.pathfind_failed(i, cx);
                    }
                }
                if wander {
                    self.npcs[i].route.true_target = Target::None;
                }
                let route = self.search(i, from, to, pos2);
                let n = &mut self.npcs[i];
                match route {
                    Some(list) if !list.is_empty() => {
                        n.route.nb = list.len() as i32;
                        n.route.list = list;
                        n.route.pos = 0;
                        cx.send(None, id, "pathfinder_success", Vec::new());
                        return true;
                    }
                    _ => {
                        n.route.nb = 0;
                        n.route.list.clear();
                    }
                }
                n.route.pos = 0;
                return true;
            }
        }
        self.pathfind_failed(i, cx)
    }

    fn pathfind_failed(&mut self, i: usize, cx: &mut Ctx) -> bool {
        let id = self.npcs[i].id;
        let n = &mut self.npcs[i];
        n.route.list.clear();
        n.route.pos = 0;
        n.route.nb = -2;
        if !n.route.always {
            cx.send(None, id, "pathfinder_failure", Vec::new());
        }
        false
    }

    /// The search the character's behaviour calls for.
    fn search(&mut self, i: usize, from: usize, to: usize, pos2: Vec3) -> Option<Vec<usize>> {
        let n = &self.npcs[i];
        let (b, param, body, pos) = (n.behavior, n.behavior_param, n.body(), n.pos);
        let a = &self.graph.anchors;
        if b & (behavior::MOVE_TO | behavior::GO_HOME) != 0 {
            let h = heuristic_for(a[from].pos.distance(a[to].pos));
            self.graph.path(from, to, body, h)
        } else if b & behavior::WANDER_AROUND != 0 {
            self.graph.wander(from, param, body, &mut self.rng)
        } else if b & (behavior::FLEE | behavior::HIDE) != 0 {
            let danger = pos2;
            let safe = param + pos.distance(danger);
            self.graph.flee(from, danger, safe, body)
        } else if b & behavior::LOOK_FOR != 0 {
            self.graph.look_for(from, pos2, param, body, &mut self.rng)
        } else {
            None
        }
    }

    // ------------------------------------------------------------------------------------- the update

    /// Carry out the scripts' character commands, then move every character that is near the player by `dt_ms`.
    pub fn update(&mut self, world: &mut ScriptWorld, host: &mut StdHost, env: &Env, dt_ms: f32) {
        self.now_ms += f64::from(dt_ms);
        let mut cx = Ctx { world, host, env, dt_ms };
        for r in cx.host.take_npc_requests() {
            self.apply(&mut cx, r);
        }
        for i in 0..self.npcs.len() {
            let id = self.npcs[i].id;
            let Some(st) = cx.host.state(id) else { continue };
            if st.hidden || st.destroyed || st.in_inventory {
                continue;
            }
            // Only the characters near the player live.
            if distance_xz(self.npcs[i].pos, env.player_pos) > self.active_range {
                continue;
            }
            self.step(i, &mut cx);
            let n = &self.npcs[i];
            cx.world.entity_mut(id).pos = n.pos.to_array();
            if let Some(c) = n.cylinder
                && !n.dead
            {
                env.collision.set_cylinder(c, to_yup(n.pos.to_array()), cx.host.state(id).is_none_or(|s| s.collision));
            }
        }
    }

    fn step(&mut self, i: usize, cx: &mut Ctx) {
        let id = self.npcs[i].id;
        self.adopt_script_anim(i, cx);

        // Dead: only the death animation plays, and the body stays where it fell.
        if self.npcs[i].life <= 0.0 && !self.npcs[i].dead {
            self.kill_npc(i, cx, None);
        }
        let scale = cx.host.state(id).map_or(1.0, |s| s.scale);
        let dt_us = (f64::from(cx.dt_ms) * 1000.0 * f64::from(self.npcs[i].speed.max(0.0))) as i64;
        let raw = self.npcs[i].layer.advance(dt_us);
        if self.npcs[i].layer.steps > 0 && !self.npcs[i].dead && self.npcs[i].on_ground {
            let pos = self.npcs[i].pos;
            for _ in 0..self.npcs[i].layer.steps {
                self.footsteps.push(Footstep { id, pos });
            }
        }
        if self.npcs[i].dead {
            if self.npcs[i].layer.ended {
                self.npcs[i].layer.force = false;
            }
            return;
        }
        let yaw = self.npcs[i].yaw;
        let mv = rotate_y(raw * scale, 180.0 - yaw);

        self.perceive(i, cx);

        // A blow it took pushes it back, a little each frame.
        let shove = self.npcs[i].shove;
        if shove.length() > 0.5 {
            let part = shove * ((cx.dt_ms / 6.0) / shove.length()).min(1.0);
            self.move_body(i, cx, part);
            self.npcs[i].shove -= part;
        } else {
            self.npcs[i].shove = Vec3::ZERO;
        }

        // Without physics (a hanging corpse) it stays put, waiting and watching.
        if cx.host.state(id).is_some_and(|s| s.physical_off) {
            if self.npcs[i].layer.slot.is_none() || self.npcs[i].layer.ended {
                self.change_anim(i, cx, "wait", false);
            }
            self.update_target_pos(i, cx);
            if !self.npcs[i].layer.force {
                self.face_target(i, cx);
            }
            return;
        }

        // A script's own animation plays out first; the character only drifts with it.
        let l = &self.npcs[i].layer;
        if l.force && !(l.ended && !l.looping) {
            // The engine just adds the animation's own movement: no collision and no gravity.
            self.npcs[i].pos += mv;
            return;
        }
        if l.force {
            self.npcs[i].layer.force = false;
        }
        self.movement(i, cx, mv);
    }

    /// Move the body by `mv` (Arx coordinates, horizontal part) against the level and the other bodies.
    fn move_body(&mut self, i: usize, cx: &mut Ctx, mv: Vec3) -> bool {
        let n = &self.npcs[i];
        let feet = to_yup(n.pos.to_array());
        let disp = Vec2::new(mv.x, -mv.z);
        let player = [(Vec2::new(to_yup(cx.env.player_pos.to_array()).x, to_yup(cx.env.player_pos.to_array()).z), PLAYER_RADIUS)];
        let extra: &[(Vec2, f32)] = if cx.env.player_alive { &player } else { &[] };
        let r = cx.env.collision.move_character(feet, disp, n.radius, n.height, cx.dt_ms / 1000.0, n.cylinder, extra);
        let n = &mut self.npcs[i];
        n.pos = Vec3::new(r.pos.x, -r.pos.y, -r.pos.z);
        n.on_ground = r.on_ground;
        let id = n.id;
        // Bumping a door tells both it and the character (once every half second): the door's script opens it for a
        // character that has the key, and the character's script remembers which door it is dealing with.
        for o in &r.touched {
            let Some(&door) = self.obstacle_owner.get(&o.0) else { continue };
            if !cx.world.entity(door).groups.contains("door") {
                continue;
            }
            if self.now_ms - self.door_bumps.get(&door).copied().unwrap_or(f64::MIN / 2.0) > 500.0 {
                self.door_bumps.insert(door, self.now_ms);
                cx.send(Some(id), door, "collide_door", Vec::new());
                cx.send(Some(door), id, "collide_door", Vec::new());
            }
        }
        r.blocked
    }

    fn movement(&mut self, i: usize, cx: &mut Ctx, mv: Vec3) {
        let id = self.npcs[i].id;
        let b = self.npcs[i].behavior;

        // Idle: stand, and keep the wait animation going.
        if b & behavior::NONE != 0 {
            // The engine returns here before it applies gravity: placed characters stay where the level put them.
            if self.npcs[i].layer.slot.is_none() || self.npcs[i].layer.ended {
                self.change_anim(i, cx, "wait", false);
            }
            return;
        }

        self.update_target_pos(i, cx);

        // A fighter close to its target prepares and lands blows instead of walking.
        if b & behavior::FIGHT != 0 && self.attack(i, cx) {
            self.face_target(i, cx);
            self.move_body(i, cx, Vec3::ZERO);
            return;
        }

        // A finished one-shot animation gives way to waiting.
        if self.npcs[i].layer.ended && !self.npcs[i].layer.looping && !self.npcs[i].layer.is("wait") {
            let friendly = b & behavior::FRIENDLY != 0;
            let slot = if b & behavior::FIGHT != 0 && cx.has_slot(id, "fight_wait") { "fight_wait" } else { "wait" };
            let _ = friendly;
            self.change_anim(i, cx, slot, slot == "fight_wait");
        }
        // A wandering character pauses where the route repeats an anchor.
        {
            let r = &self.npcs[i].route;
            if r.nb > 0 && b & behavior::WANDER_AROUND != 0 && r.pos + 2 < r.nb as usize && r.list[r.pos] == r.list[r.pos + 1] {
                if !self.npcs[i].layer.is("wait") && cx.has_slot(id, "wait") {
                    self.change_anim(i, cx, "wait", false);
                } else if self.npcs[i].layer.ended || !cx.has_slot(id, "wait") {
                    self.advance_route(i, cx);
                }
                self.move_body(i, cx, Vec3::ZERO);
                return;
            }
        }

        // The wait animation restarts when it ends.
        if self.npcs[i].layer.is("wait") && self.npcs[i].layer.ended {
            self.change_anim(i, cx, "wait", false);
        }

        let n = &self.npcs[i];
        let dist = distance_xz(n.pos, n.target_pos);
        let mut dis = if n.route.nb > 0 { self.true_target_distance(i, cx) } else { dist };
        if b & behavior::FLEE != 0 {
            dis = 9_999_999.0;
        }

        // Plan again when the plan has run out.
        if n.route.nb <= 0 {
            if b & behavior::WANDER_AROUND != 0 {
                let t = n.target;
                self.launch_pathfind(i, cx, t);
            } else if dis > STRIKE_DISTANCE && b & behavior::MOVE_TO != 0 && b & (behavior::FIGHT | behavior::MAGIC | behavior::SNEAK) == 0 && n.route.nb != -2 {
                let t = n.target;
                self.launch_pathfind(i, cx, t);
            } else if n.route.nb == -2 && !n.route.no_update && self.npcs[i].layer.is("wait") && self.npcs[i].layer.ended {
                // A failed search is tried again after a pause.
                self.npcs[i].route.nb = -1;
                let t = self.npcs[i].target;
                self.launch_pathfind(i, cx, t);
            }
        }

        // What to play.
        let n = &self.npcs[i];
        let has_route = n.route.nb > 0;
        let travelling_target = n.target != Target::None || b & (behavior::WANDER_AROUND | behavior::FLEE | behavior::GO_HOME) != 0;
        if !has_route && b & behavior::FLEE != 0 {
            self.set_anim(i, cx, "wait", false);
        } else if b & behavior::FRIENDLY != 0 {
            // Stands and talks.
            if self.npcs[i].layer.is_walking() {
                self.change_anim(i, cx, "wait", false);
            }
        } else if !n.reached {
            if travelling_target && has_route && !n.layer.is_walking() {
                let slot = self.walk_slot(i, cx, dis);
                if cx.has_slot(id, slot) {
                    self.change_anim(i, cx, slot, true);
                }
            }
        } else if b & (behavior::FIGHT | behavior::MAGIC | behavior::DISTANT) != 0 && cx.has_slot(id, "fight_wait") {
            if !matches!(self.npcs[i].layer.slot.as_deref(), Some("fight_strafe_left" | "fight_strafe_right" | "fight_walk_backward" | "fight_walk_forward")) {
                self.set_anim(i, cx, "fight_wait", true);
            }
        } else if self.npcs[i].layer.is_walking() {
            self.npcs[i].layer.looping = false;
            if self.now_ms - self.npcs[i].reached_at_ms > 500.0 {
                self.change_anim(i, cx, "wait", false);
            }
        }

        // Face the way it is going (only while standing or walking).
        let can_turn = matches!(self.npcs[i].layer.slot.as_deref(), None | Some("wait" | "fight_wait" | "walk" | "run" | "walk_sneak" | "fight_walk_forward" | "fight_strafe_left" | "fight_strafe_right" | "fight_walk_backward"));
        if can_turn {
            self.face_target(i, cx);
        }

        // Tolerances, then the move itself.
        let (tolerance, tolerance2) = {
            let n = &self.npcs[i];
            if n.route.nb > 0 && n.route.pos < n.route.nb as usize {
                (30.0 + n.move_problem as f32 * 0.1, self.tolerance(i, cx, n.route.true_target))
            } else {
                let t = self.tolerance(i, cx, n.target);
                (t, t)
            }
        };
        let walking = self.npcs[i].layer.is_walking() || self.npcs[i].layer.slot.as_deref().is_some_and(|s| s.starts_with("fight_strafe") || s == "fight_walk_backward");
        let blocked = self.move_body(i, cx, if walking { mv } else { Vec3::ZERO });
        {
            let n = &mut self.npcs[i];
            if blocked && walking {
                n.move_problem += 3;
            } else {
                n.move_problem = 0;
            }
        }
        let n = &self.npcs[i];
        let dist = distance_xz(n.pos, n.target_pos);
        let mut dis = if n.route.nb > 0 { self.true_target_distance(i, cx) } else { dist };
        if b & behavior::FLEE != 0 {
            dis = 9_999_999.0;
        }

        // Stuck: plan again.
        if self.npcs[i].move_problem > 11 {
            if dist > tolerance {
                let n = &self.npcs[i];
                let t = if n.route.nb > 0 { n.route.true_target } else { n.target };
                self.launch_pathfind(i, cx, t);
            }
            self.npcs[i].move_problem = 0;
        }

        // Has the target wandered off from where the route leads?
        {
            let n = &self.npcs[i];
            if !n.route.once
                && !n.route.no_update
                && n.route.nb > 0
                && n.route.pos < n.route.nb as usize
                && b & behavior::MOVE_TO != 0
                && b & behavior::FLEE == 0
                && let Target::Entity(t) = n.route.true_target
                && let Some(near) = self.graph.nearest(Self::entity_pos(cx, t), n.body(), None)
                && let Some(&last) = n.route.list.last()
                && near != last
                && self.graph.anchors[near].pos.distance(self.graph.anchors[last].pos) > 200.0
            {
                self.launch_pathfind(i, cx, Target::Entity(t));
            }
        }

        let n = &self.npcs[i];
        if dist > tolerance && dis > tolerance2 {
            if n.reached {
                let who = match n.target {
                    Target::Entity(e) => Some(e),
                    _ => None,
                };
                self.npcs[i].reached = false;
                cx.send(who, id, "losttarget", Vec::new());
            }
            // Start moving if standing: the walking animation drives the movement.
            let n = &self.npcs[i];
            let standing = matches!(n.layer.slot.as_deref(), None | Some("wait" | "fight_wait")) && !n.layer.force;
            if standing && (n.route.nb > 0 || b & behavior::FLEE == 0) && n.target != Target::None {
                let slot = self.walk_slot(i, cx, dis);
                if cx.has_slot(id, slot) {
                    self.set_anim(i, cx, slot, true);
                }
            }
        } else {
            if dis <= tolerance2 {
                self.npcs[i].route.clear();
            }
            if self.npcs[i].route.nb > 0 {
                self.advance_route(i, cx);
            } else if !self.npcs[i].reached {
                let sender = match self.npcs[i].target {
                    Target::Entity(e) => Some(e),
                    _ => None,
                };
                self.npcs[i].reached = true;
                self.npcs[i].reached_at_ms = self.now_ms;
                if sender != Some(id) {
                    cx.send(sender, id, "reachedtarget", Vec::new());
                }
            }
        }
    }

    /// Which walking animation fits the mode and the distance still to go.
    fn walk_slot(&self, i: usize, cx: &Ctx, dis: f32) -> &'static str {
        let n = &self.npcs[i];
        let fighting = n.behavior & behavior::FIGHT != 0;
        if fighting && dis <= RUN_WALK_RADIUS {
            return "fight_walk_forward";
        }
        match n.move_mode {
            MoveMode::Sneak => "walk_sneak",
            MoveMode::Walk => "walk",
            MoveMode::Run if dis > RUN_WALK_RADIUS && cx.has_slot(n.id, "run") => "run",
            MoveMode::Run => "walk",
            MoveMode::None => "wait",
        }
    }

    /// Reached the current anchor: take the next one (`ManageNPCMovement_check_target_reached`).
    fn advance_route(&mut self, i: usize, cx: &mut Ctx) {
        let id = self.npcs[i].id;
        let n = &mut self.npcs[i];
        n.route.pos += 1;
        if n.route.pos < n.route.nb.max(0) as usize {
            return;
        }
        n.route.clear();
        let fled = n.behavior & behavior::FLEE != 0;
        let no_update = n.route.no_update;
        if fled {
            cx.send(None, id, "flee_end", Vec::new());
        }
        let n = &mut self.npcs[i];
        if no_update {
            if !n.reached {
                n.reached = true;
                n.reached_at_ms = self.now_ms;
                if n.target != Target::Entity(id) {
                    n.target = Target::Entity(id);
                    cx.send(None, id, "reachedtarget", vec!["fake".to_owned()]);
                }
            }
        } else {
            n.target = n.route.true_target;
            self.update_target_pos(i, cx);
            let n = &mut self.npcs[i];
            if (n.pos.y - n.target_pos.y).abs() > 200.0 {
                n.route.nb = -2;
            }
        }
    }

    /// Turn toward the target at the engine's rate (`FaceTarget2`).
    fn face_target(&mut self, i: usize, cx: &Ctx) {
        let n = &self.npcs[i];
        if n.life <= 0.0 || n.behavior & behavior::NONE != 0 || (n.route.nb <= 0 && n.behavior & behavior::FLEE != 0) {
            return;
        }
        let t = n.target_pos;
        let (dx, dz) = (t.x - n.pos.x, t.z - n.pos.z);
        if Vec2::new(dx, dz).length() <= 5.0 {
            return;
        }
        let want = yaw_toward(dx, dz);
        let diff = (want - n.yaw + 540.0).rem_euclid(360.0) - 180.0;
        let step = (TURN_PER_MS * cx.dt_ms).min(diff.abs());
        let n = &mut self.npcs[i];
        n.yaw = (n.yaw + step * diff.signum()).rem_euclid(360.0);
    }

    // ---------------------------------------------------------------------------------------- senses

    /// Does the character see the player? Sends `detectplayer` / `undetectplayer` when that changes.
    fn perceive(&mut self, i: usize, cx: &mut Ctx) {
        let id = self.npcs[i].id;
        let env = cx.env;
        let n = &self.npcs[i];
        let mut visible = false;
        let ds = n.pos.distance(env.player_pos);
        if env.player_alive && ds < SIGHT_RANGE && !n.dead {
            if ds < n.radius + PLAYER_RADIUS + NOTICE_CONTACT && (env.player_pos.y - n.pos.y).abs() < 200.0 {
                visible = true;
            } else {
                let to = env.player_pos - n.pos;
                let bearing = yaw_toward(to.x, to.z);
                if angle_diff(bearing, n.yaw) < SIGHT_HALF_ANGLE && (env.player_light > env.player_stealth || env.player_torch || ds < 200.0) {
                    // Eye to head, through the level.
                    let a = to_yup((n.pos + Vec3::new(0.0, -120.0, 0.0)).to_array());
                    let b = to_yup((env.player_pos + Vec3::new(0.0, -90.0, 0.0)).to_array());
                    let d = b - a;
                    let len = d.length();
                    visible = len < 1.0 || env.collision.raycast(a, d / len, len).is_none_or(|h| h.t >= len - 25.0);
                }
            }
        }
        let n = &mut self.npcs[i];
        if visible && !n.detect {
            n.detect = true;
            cx.send(None, id, "detectplayer", Vec::new());
        } else if !visible && n.detect {
            n.detect = false;
            cx.send(None, id, "undetectplayer", Vec::new());
        }
    }

    /// A sound at `pos` (Arx coordinates) made by `source`: every living character within `max_distance` hears it
    /// (the event carries the distance).
    pub fn hear(&mut self, world: &mut ScriptWorld, host: &mut StdHost, source: EntityId, pos: Vec3, max_distance: f32) {
        let mut heard = Vec::new();
        for n in &self.npcs {
            if n.id == source || n.life <= 0.0 || n.dead {
                continue;
            }
            let hidden = host.state(n.id).is_some_and(|s| s.destroyed || s.in_inventory);
            let d = n.pos.distance(pos);
            if !hidden && d < max_distance {
                heard.push((n.id, d));
            }
        }
        for (id, d) in heard {
            world.send_event(host, Some(source), id, "hear", vec![format!("{}", d as i64)]);
        }
    }
}

/// Set up every character of a level: its position and facing from the scene definition, its cylinder from the
/// collision world, and the walk animations' translations from the archives.
pub fn build_npcs(
    pak: &Arc<arx_formats::PakSet>,
    anchors: &[arx_formats::fts::Anchor],
    scene_pos: Vec3,
    dlf: &arx_formats::dlf::Dlf,
    ids: &[EntityId],
    world: &ScriptWorld,
    obstacles: &crate::EntityObstacles,
    collision: &CollisionWorld,
) -> NpcWorld {
    let mut npcs = NpcWorld::new(Arc::new(AnchorGraph::from_fts(anchors)));
    let pak = pak.clone();
    npcs.set_track_loader(Box::new(move |path| {
        let bytes = pak.read(path).ok()?;
        Tea::parse(&bytes).ok().map(|t| MoveTrack::from_tea(&t))
    }));
    for (index, e) in dlf.entities.iter().enumerate() {
        let id = ids[index];
        if world.entity(id).kind != EntityKind::Npc {
            continue;
        }
        let pos = Vec3::new(e.pos[0] + scene_pos.x, e.pos[1] + scene_pos.y, e.pos[2] + scene_pos.z);
        let cylinder = obstacles.characters.get(&id).and_then(|&c| collision.cylinder(c).map(|cy| (c, cy.radius, cy.height)));
        npcs.add(id, pos, e.angle[1], cylinder);
    }
    npcs.set_obstacle_owners(obstacles.by_entity.iter().map(|(&e, &o)| (o, e)));
    npcs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anchors::Node;
    use arx_script::{EntityKind, Script, ScriptWorld, StdHost};

    /// A walk animation that moves 100 units forward (object-space -Z, like the real ones) in one second.
    fn walk_track() -> MoveTrack {
        MoveTrack::from_keys(1_000_000, vec![(0, Vec3::ZERO), (1_000_000, Vec3::new(0.0, 0.0, -100.0))])
    }

    fn flat_floor() -> CollisionWorld {
        let q = |a: Vec3, b: Vec3, c: Vec3, d: Vec3| [[a, b, c], [a, c, d]];
        let h = 4000.0;
        CollisionWorld::from_triangles(q(Vec3::new(-h, 0.0, -h), Vec3::new(-h, 0.0, h), Vec3::new(h, 0.0, h), Vec3::new(h, 0.0, -h)))
    }

    /// A world with one NPC at `pos` and a marker `target`, both on the floor, plus a player far enough not to matter.
    struct Rig {
        world: ScriptWorld,
        host: StdHost,
        npcs: NpcWorld,
        collision: CollisionWorld,
        npc: EntityId,
        marker: EntityId,
        player: EntityId,
    }

    impl Rig {
        fn new(graph: Vec<Node>, script: &str) -> Rig {
            let mut world = ScriptWorld::new();
            let mut host = StdHost::new();
            let player = world.add_entity(EntityKind::Player, "x/npc/player/player", 1, None, None);
            // Scripts are Latin-1 bytes, so `§` is a single byte.
            let npc = world.add_entity(EntityKind::Npc, "x/npc/goblin/goblin", 1, Some(Arc::new(Script::new(&script.chars().map(|c| c as u8).collect::<Vec<u8>>()))), None);
            let marker = world.add_entity(EntityKind::Marker, "x/system/marker/marker", 1, None, None);
            world.entity_mut(player).pos = [-1500.0, 0.0, -1500.0];
            for slot in ["wait", "walk", "run"] {
                host.modify(npc, |s| {
                    s.anims.insert(slot.to_owned(), format!("anims/{slot}.tea"));
                });
            }
            let mut npcs = NpcWorld::new(Arc::new(AnchorGraph::new(graph)));
            npcs.set_track_loader(Box::new(|path| {
                if path.ends_with("wait.tea") {
                    Some(MoveTrack::from_keys(2_000_000, vec![(0, Vec3::ZERO), (2_000_000, Vec3::ZERO)]))
                } else if path.ends_with("run.tea") {
                    Some(MoveTrack::from_keys(1_000_000, vec![(0, Vec3::ZERO), (1_000_000, Vec3::new(0.0, 0.0, -250.0))]))
                } else {
                    Some(walk_track())
                }
            }));
            npcs.add(npc, Vec3::ZERO, 0.0, None);
            Rig { world, host, npcs, collision: flat_floor(), npc, marker, player }
        }

        fn run(&mut self, secs: f32) {
            for _ in 0..(secs * 60.0) as usize {
                let env = Env {
                    collision: &self.collision,
                    player_pos: Vec3::from(self.world.entity(self.player).pos),
                    player_alive: true,
                    player_stealth: 15.0,
                    player_light: 255.0,
                    player_torch: false,
                };
                self.npcs.update(&mut self.world, &mut self.host, &env, 1000.0 / 60.0);
                self.world.update(&mut self.host, 1000.0 / 60.0);
            }
        }

        fn pos(&self) -> Vec3 {
            self.npcs.npc(self.npc).unwrap().pos
        }
    }

    fn line(n: usize, step: f32) -> Vec<Node> {
        (0..n)
            .map(|i| {
                let mut links = Vec::new();
                if i > 0 {
                    links.push(i as i32 - 1);
                }
                if i + 1 < n {
                    links.push(i as i32 + 1);
                }
                Node { pos: Vec3::new(0.0, 0.0, i as f32 * step), radius: 60.0, height: 100.0, blocked: false, links }
            })
            .collect()
    }

    #[test]
    fn facing_and_yaw_agree_with_the_engines_axes() {
        for yaw in [0.0, 90.0, 180.0, 270.0] {
            let f = facing(yaw);
            assert!((yaw_toward(f.x, f.y) - yaw).abs() < 1e-3, "{yaw}: {f:?}");
        }
        // Yaw 0 faces +Z in Arx coordinates; yaw 180 faces -Z; yaw 90 faces -X.
        assert!((facing(0.0) - Vec2::new(0.0, 1.0)).length() < 1e-5);
        assert!((facing(180.0) - Vec2::new(0.0, -1.0)).length() < 1e-5);
        assert!((facing(90.0) - Vec2::new(-1.0, 0.0)).length() < 1e-5);
    }

    #[test]
    fn root_motion_is_rotated_by_the_engines_formula() {
        // Animations walk along object-space -Z; the engine's rotation by `180 - yaw` must carry that along the way
        // the yaw faces.
        let fwd = Vec3::new(0.0, 0.0, -1.0);
        for yaw in [0.0, 45.0, 90.0, 180.0, 270.0, 351.0] {
            let v = rotate_y(fwd, 180.0 - yaw);
            assert!((Vec2::new(v.x, v.z) - facing(yaw)).length() < 1e-4, "yaw {yaw}: {v:?}");
        }
    }

    #[test]
    fn a_move_to_character_walks_at_the_animations_speed_and_reports_arrival() {
        let mut rig = Rig::new(line(12, 100.0), "on reachedtarget {\n set §arrived 1\n accept\n}\non pathfinder_failure {\n set §failed 1\n accept\n}");
        rig.world.entity_mut(rig.marker).pos = [0.0, 0.0, 800.0];
        rig.host.push_npc_request(NpcRequest::Behavior { entity: rig.npc, flags: String::new(), command: "move_to".into(), param: 0.0 });
        rig.host.push_npc_request(NpcRequest::SetTarget { entity: rig.npc, flags: String::new(), target: TargetSpec::Entity(rig.marker) });
        rig.run(0.1);
        assert!(rig.npcs.npc(rig.npc).unwrap().is_traveling() || rig.pos().z > 0.0, "a route was planned");
        // The walk covers 100 units per second: after 3 s it is about 300 units along.
        rig.run(2.9);
        let z = rig.pos().z;
        assert!(z > 200.0 && z < 400.0, "walked {z} units in about 3 s");
        assert!(rig.pos().x.abs() < 5.0, "and stayed on the line: {:?}", rig.pos());
        assert_eq!(rig.npcs.npc(rig.npc).unwrap().animation(), Some("walk"));
        rig.run(6.0);
        let arrived = rig.world.entity(rig.npc).vars.get_int("§arrived");
        assert_eq!(arrived, 1, "reachedtarget was sent, at {:?}", rig.pos());
        assert!((rig.pos().z - 800.0).abs() < 60.0, "{:?}", rig.pos());
        assert!(rig.npcs.npc(rig.npc).unwrap().reached);
        // And it stands still afterwards.
        rig.run(2.0);
        assert_eq!(rig.npcs.npc(rig.npc).unwrap().animation(), Some("wait"));
    }

    #[test]
    fn running_mode_uses_the_run_animation_when_far() {
        let mut rig = Rig::new(line(30, 100.0), "");
        rig.world.entity_mut(rig.marker).pos = [0.0, 0.0, 2500.0];
        rig.host.push_npc_request(NpcRequest::Behavior { entity: rig.npc, flags: String::new(), command: "move_to".into(), param: 0.0 });
        rig.host.push_npc_request(NpcRequest::MoveMode { entity: rig.npc, mode: "run".into() });
        rig.host.push_npc_request(NpcRequest::SetTarget { entity: rig.npc, flags: String::new(), target: TargetSpec::Entity(rig.marker) });
        rig.run(1.0);
        assert_eq!(rig.npcs.npc(rig.npc).unwrap().animation(), Some("run"));
        rig.run(2.0);
        assert!(rig.pos().z > 500.0, "running is faster than walking: {:?}", rig.pos());
    }

    #[test]
    fn a_target_off_the_graph_reports_a_pathfinder_failure() {
        let mut rig = Rig::new(line(5, 100.0), "on pathfinder_failure {\n set §failed 1\n accept\n}");
        rig.world.entity_mut(rig.marker).pos = [3000.0, 0.0, 3000.0];
        rig.host.push_npc_request(NpcRequest::Behavior { entity: rig.npc, flags: String::new(), command: "move_to".into(), param: 0.0 });
        rig.host.push_npc_request(NpcRequest::SetTarget { entity: rig.npc, flags: String::new(), target: TargetSpec::Entity(rig.marker) });
        rig.run(0.2);
        assert_eq!(rig.world.entity(rig.npc).vars.get_int("§failed"), 1);
    }

    #[test]
    fn friendly_characters_stand_still_but_turn_to_their_target() {
        let mut rig = Rig::new(line(5, 100.0), "");
        // Marker to the +X side; the character starts facing +Z (yaw 0).
        rig.world.entity_mut(rig.marker).pos = [400.0, 0.0, 0.0];
        rig.host.push_npc_request(NpcRequest::Behavior { entity: rig.npc, flags: String::new(), command: "friendly".into(), param: 0.0 });
        rig.host.push_npc_request(NpcRequest::SetTarget { entity: rig.npc, flags: String::new(), target: TargetSpec::Entity(rig.marker) });
        rig.run(1.5);
        let n = rig.npcs.npc(rig.npc).unwrap();
        assert!(n.pos.length() < 1.0, "it did not move: {:?}", n.pos);
        assert!((n.yaw - 270.0).abs() < 2.0, "it turned to face +X: {}", n.yaw);
    }

    #[test]
    fn wandering_characters_patrol_and_pause() {
        let mut rig = Rig::new(line(20, 100.0), "");
        rig.npcs.npc_mut(rig.npc).unwrap().pos = Vec3::new(0.0, 0.0, 1000.0);
        rig.host.push_npc_request(NpcRequest::Behavior { entity: rig.npc, flags: String::new(), command: "wander_around".into(), param: 600.0 });
        rig.run(1.0);
        let mut moved = false;
        let mut paused = false;
        let mut max_dev: f32 = 0.0;
        for _ in 0..40 {
            rig.run(1.0);
            let n = rig.npcs.npc(rig.npc).unwrap();
            moved |= n.animation() == Some("walk");
            paused |= n.animation() == Some("wait");
            max_dev = max_dev.max((n.pos.z - 1000.0).abs());
        }
        assert!(moved && paused, "it walks and it pauses");
        assert!(max_dev > 50.0 && max_dev < 1200.0, "it stays around the spot: {max_dev}");
    }

    #[test]
    fn a_scripted_animation_takes_over_until_it_ends() {
        let mut rig = Rig::new(line(5, 100.0), "on init {\n playanim action1\n accept\n}");
        rig.host.modify(rig.npc, |s| {
            s.anims.insert("action1".into(), "anims/action1.tea".into());
        });
        rig.world.send_init(&mut rig.host, rig.npc);
        rig.run(0.2);
        assert_eq!(rig.npcs.npc(rig.npc).unwrap().animation(), Some("action1"));
        rig.run(3.0);
        assert_eq!(rig.npcs.npc(rig.npc).unwrap().animation(), Some("wait"), "back to waiting once it ended");
    }

    #[test]
    fn the_player_in_view_is_detected_and_losing_them_is_reported() {
        let mut rig = Rig::new(line(5, 100.0), "on detectplayer {\n set §seen 1\n accept\n}\non undetectplayer {\n set §seen 0\n accept\n}");
        // In front (yaw 0 faces +Z) and in the open.
        rig.world.entity_mut(rig.player).pos = [0.0, 0.0, 600.0];
        rig.run(0.3);
        assert_eq!(rig.world.entity(rig.npc).vars.get_int("§seen"), 1);
        // Behind it: not seen.
        rig.world.entity_mut(rig.player).pos = [0.0, 0.0, -600.0];
        rig.run(0.3);
        assert_eq!(rig.world.entity(rig.npc).vars.get_int("§seen"), 0);
        // Too far.
        rig.world.entity_mut(rig.player).pos = [0.0, 0.0, 2500.0];
        rig.run(0.3);
        assert!(!rig.npcs.npc(rig.npc).unwrap().detect);
    }

    #[test]
    fn nearby_characters_hear_noises_with_the_distance_as_parameter() {
        let mut rig = Rig::new(line(5, 100.0), "on hear {\n set §d ^#param1\n accept\n}");
        let player = rig.player;
        rig.npcs.hear(&mut rig.world, &mut rig.host, player, Vec3::new(0.0, 0.0, 300.0), HEAR_STEP_DISTANCE);
        rig.world.update(&mut rig.host, 0.0);
        assert_eq!(rig.world.entity(rig.npc).vars.get_int("§d"), 300);
        rig.world.entity_mut(rig.npc).vars.set("§d", arx_script::Value::Int(0));
        rig.npcs.hear(&mut rig.world, &mut rig.host, player, Vec3::new(0.0, 0.0, 900.0), HEAR_STEP_DISTANCE);
        rig.world.update(&mut rig.host, 0.0);
        assert_eq!(rig.world.entity(rig.npc).vars.get_int("§d"), 0, "too far to hear");
    }

    #[test]
    fn behaviors_can_be_stacked_and_restored() {
        let mut rig = Rig::new(line(5, 100.0), "");
        rig.host.push_npc_request(NpcRequest::Behavior { entity: rig.npc, flags: String::new(), command: "wander_around".into(), param: 400.0 });
        rig.run(0.1);
        rig.host.push_npc_request(NpcRequest::Behavior { entity: rig.npc, flags: String::new(), command: "stack".into(), param: 0.0 });
        rig.host.push_npc_request(NpcRequest::Behavior { entity: rig.npc, flags: String::new(), command: "friendly".into(), param: 0.0 });
        rig.run(0.1);
        assert_eq!(rig.npcs.npc(rig.npc).unwrap().behavior, behavior::FRIENDLY);
        rig.host.push_npc_request(NpcRequest::Behavior { entity: rig.npc, flags: String::new(), command: "unstack".into(), param: 0.0 });
        rig.run(0.1);
        assert_eq!(rig.npcs.npc(rig.npc).unwrap().behavior, behavior::WANDER_AROUND);
    }

    fn player_strike(rig: &mut Rig, at: Vec3, ratio: f32) -> Vec<Impact> {
        let env = Env { collision: &rig.collision, player_pos: Vec3::from(rig.world.entity(rig.player).pos), player_alive: true, player_stealth: 15.0, player_light: 255.0, player_torch: false };
        let mut hit = Vec::new();
        rig.npcs.player_strike(&mut rig.world, &mut rig.host, &env, &[(at, 20.0)], ratio, &mut hit)
    }

    #[test]
    fn the_heros_blow_hurts_a_character_and_a_killing_blow_gives_experience() {
        let mut rig = Rig::new(line(5, 100.0), "on hit {
 set §hits ^#param1
 accept
}
on die {
 set §died 1
 accept
}
on aggression {
 set §angry 1
 accept
}");
        rig.npcs.npc_mut(rig.npc).unwrap().life = 5.0;
        rig.npcs.npc_mut(rig.npc).unwrap().stats.xp_value = 2000.0;
        rig.host.modify(rig.npc, |st| {
            st.anims.insert("die".into(), "anims/die.tea".into());
        });
        let before = rig.host.player.misc().damages;
        // A swing at empty air hurts nobody.
        assert!(player_strike(&mut rig, Vec3::new(500.0, -80.0, 500.0), 1.0).is_empty());
        // A full blow on the character: it is told, takes the damage and does not die yet.
        let hit = player_strike(&mut rig, Vec3::new(0.0, -80.0, 20.0), 1.0);
        rig.world.update(&mut rig.host, 0.0);
        assert_eq!(hit.len(), 1);
        assert!(!hit[0].missed && (hit[0].damage - before).abs() < 1e-3, "{hit:?}");
        assert_eq!(rig.world.entity(rig.npc).vars.get_int("§angry"), 1, "aggression was sent");
        assert_eq!(rig.world.entity(rig.npc).vars.get_int("§hits"), before as i64, "hit carries the damage");
        let n = rig.npcs.npc(rig.npc).unwrap();
        assert!((n.life - (5.0 - before)).abs() < 1e-3 && !n.dead);
        // A half-aimed blow does half as much; a second full blow kills it, and the hero gains its experience.
        let half = player_strike(&mut rig, Vec3::new(0.0, -80.0, 20.0), 0.5);
        assert!((half[0].damage - before * 0.5).abs() < 1e-3);
        assert!(!rig.npcs.npc(rig.npc).unwrap().dead);
        let kill = player_strike(&mut rig, Vec3::new(0.0, -80.0, 20.0), 1.0);
        rig.world.update(&mut rig.host, 0.0);
        assert!(kill[0].killed);
        let n = rig.npcs.npc(rig.npc).unwrap();
        assert!(n.dead && n.life == 0.0);
        assert_eq!(rig.world.entity(rig.npc).vars.get_int("§died"), 1);
        assert_eq!(rig.host.player.xp, 2000);
        assert_eq!(rig.host.player.level, 1, "2000 experience is level 1");
        assert_eq!(n.animation(), Some("die"));
        // The dead cannot be hit again.
        assert!(player_strike(&mut rig, Vec3::new(0.0, -80.0, 20.0), 1.0).is_empty());
    }

    #[test]
    fn armour_class_decides_whether_a_blow_lands_and_a_script_can_refuse_the_hit() {
        let mut rig = Rig::new(line(5, 100.0), "");
        rig.npcs.npc_mut(rig.npc).unwrap().stats.armor_class = 500.0;
        let r = player_strike(&mut rig, Vec3::new(0.0, -80.0, 20.0), 1.0);
        assert!(r[0].missed && r[0].damage == 0.0, "nothing gets through armour class 500: {r:?}");
        assert_eq!(rig.npcs.npc(rig.npc).unwrap().life, 20.0);

        let mut rig = Rig::new(line(5, 100.0), "on hit {
 refuse
}");
        let r = player_strike(&mut rig, Vec3::new(0.0, -80.0, 20.0), 1.0);
        assert!(!r[0].missed);
        assert_eq!(rig.npcs.npc(rig.npc).unwrap().life, 20.0, "the script refused the hit");
        // Absorption removes a share of the damage.
        let mut rig = Rig::new(line(5, 100.0), "");
        rig.npcs.npc_mut(rig.npc).unwrap().stats.absorb = 50.0;
        let before = rig.host.player.misc().damages;
        let r = player_strike(&mut rig, Vec3::new(0.0, -80.0, 20.0), 1.0);
        assert!((r[0].damage - before * 0.5).abs() < 1e-3, "{r:?}");
        // It makes noises: armour on weapon, then flesh on weapon.
        let sounds = rig.npcs.take_sounds();
        assert_eq!(sounds.len(), 2);
        assert_eq!((sounds[0].hitter.as_str(), sounds[0].surface.as_str()), ("flesh", "bare"));
        assert_eq!(sounds[1].hitter, "flesh");
    }

    #[test]
    fn a_fighting_character_closes_in_aims_and_hurts_the_hero() {
        let mut rig = Rig::new(line(8, 100.0), "on strike {
 set §struck 1
 accept
}");
        for dir in ["left", "right", "top", "bottom"] {
            for (suffix, file) in [("_start", "start"), ("_cycle", "cycle"), ("", "strike")] {
                rig.host.modify(rig.npc, |s| {
                    s.anims.insert(format!("bare_strike_{dir}{suffix}"), format!("anims/{file}.tea"));
                });
            }
        }
        rig.host.modify(rig.npc, |s| {
            s.anims.insert("fight_wait".into(), "anims/wait.tea".into());
            s.anims.insert("fight_walk_forward".into(), "anims/walk.tea".into());
        });
        let timed = |path: &str| -> Option<MoveTrack> {
            let len = if path.ends_with("start.tea") { 300_000 } else if path.ends_with("strike.tea") { 600_000 } else { 500_000 };
            Some(MoveTrack::from_keys(len, vec![(0, Vec3::ZERO), (len, Vec3::ZERO)]))
        };
        rig.npcs.set_track_loader(Box::new(move |p| if p.ends_with("walk.tea") { Some(walk_track()) } else { timed(p) }));
        {
            let n = rig.npcs.npc_mut(rig.npc).unwrap();
            n.stats.damages = 4.0;
            n.stats.aim_time_ms = 400.0;
        }
        // The hero stands 150 units ahead of the character.
        rig.world.entity_mut(rig.player).pos = [0.0, 0.0, 150.0];
        rig.host.player.life = arx_script::Pool::full(1000.0);
        let player = rig.player;
        rig.host.push_npc_request(NpcRequest::Behavior { entity: rig.npc, flags: "f".into(), command: "move_to".into(), param: 0.0 });
        rig.host.push_npc_request(NpcRequest::SetTarget { entity: rig.npc, flags: String::new(), target: TargetSpec::Entity(player) });
        rig.run(6.0);
        assert_eq!(rig.world.entity(rig.npc).vars.get_int("§struck"), 1, "strike was sent before a blow");
        let life = rig.host.player.life.current;
        assert!(life < 1000.0, "the hero was hurt: {life}");
        assert!(1000.0 - life <= 4.0 * 20.0, "blows of up to 4 each, a few of them");
    }

    #[test]
    fn walking_characters_take_a_step_whenever_the_animation_puts_a_foot_down() {
        let mut rig = Rig::new(line(12, 100.0), "");
        rig.npcs.set_track_loader(Box::new(|path| {
            if path.ends_with("walk.tea") {
                Some(walk_track().with_steps(vec![250_000, 750_000]))
            } else {
                Some(MoveTrack::from_keys(2_000_000, vec![(0, Vec3::ZERO), (2_000_000, Vec3::ZERO)]))
            }
        }));
        rig.world.entity_mut(rig.marker).pos = [0.0, 0.0, 900.0];
        rig.host.push_npc_request(NpcRequest::Behavior { entity: rig.npc, flags: String::new(), command: "move_to".into(), param: 0.0 });
        rig.host.push_npc_request(NpcRequest::SetTarget { entity: rig.npc, flags: String::new(), target: TargetSpec::Entity(rig.marker) });
        let mut steps = Vec::new();
        for _ in 0..(4.0 * 60.0) as usize {
            rig.run(1.0 / 60.0);
            steps.extend(rig.npcs.take_footsteps());
        }
        // Two feet a second for about four seconds of walking.
        assert!((6..=9).contains(&steps.len()), "{} steps", steps.len());
        assert!(steps.windows(2).all(|w| w[1].pos.z >= w[0].pos.z - 1.0), "they follow the walk");
        assert!(rig.npcs.take_footsteps().is_empty());
    }

    #[test]
    fn move_tracks_interpolate_and_loop_without_a_jump() {
        let t = walk_track();
        assert_eq!(t.at(500_000), Vec3::new(0.0, 0.0, -50.0));
        let mut l = Layer { slot: Some("walk".into()), track: Some(Arc::new(t)), looping: true, ..Layer::default() };
        let mut total = 0.0;
        for _ in 0..30 {
            total -= l.advance(100_000).z;
        }
        assert!((total - 300.0).abs() < 0.01, "three loops of 100 units: {total}");
    }
}
