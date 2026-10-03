//! Runs the level's entity scripts and reflects their effects in the scene.

use crate::animated::Animated;
use crate::anims;
use crate::{Arx, Fly};
use arx_formats::{PakSet, dlf::Dlf, tea::Tea};
use arx_script::{EntityId, EntityKind, Script, ScriptWorld, StdHost};
use bevy::prelude::*;
use std::{collections::HashMap, sync::Arc};

/// How far the player can reach to use something.
const REACH: f32 = 350.0;

#[derive(Component)]
pub struct ScriptRef(pub EntityId);

/// A clickable bounding sphere for an entity (Bevy coordinates).
pub struct Pickable {
    pub id: EntityId,
    pub center: Vec3,
    pub radius: f32,
}

#[derive(Resource, Default)]
pub struct Pickables(pub Vec<Pickable>);

#[derive(Resource, Default)]
pub struct Scripting {
    pub world: ScriptWorld,
    pub host: StdHost,
    /// Script entity of each `.dlf` entity, in file order.
    pub ids: Vec<EntityId>,
    pub player: EntityId,
    /// What the player is currently looking at.
    pub target: Option<EntityId>,
    /// (revision, animation serial) last applied to the scene, per entity.
    applied: HashMap<EntityId, (u32, u32)>,
    anim_cache: HashMap<String, Option<Arc<Tea>>>,
}

impl Scripting {
    /// Load every entity's scripts, then run the level start-up sequence: `load`, `init` and
    /// `initend` for each entity, then `game_ready` for all.
    pub fn build(pak: &Arc<PakSet>, dlf: &Dlf, scene_pos: Vec3) -> Scripting {
        let mut world = ScriptWorld::new();
        let mut host = StdHost::new();
        {
            // Animation length in ms, straight from the header (frame count at 24 fps).
            let pak = pak.clone();
            host.set_anim_duration(Box::new(move |path| {
                let bytes = pak.read(path).ok()?;
                let frames = i32::from_le_bytes(bytes.get(280..284)?.try_into().ok()?);
                Some(frames as f64 * 1000.0 / 24.0)
            }));
        }
        let player = world.add_entity(EntityKind::Player, "graph/obj3d/interactive/npc/player/player", 1, None, None);

        let load = |path: &str| pak.read(path).ok().map(|b| Arc::new(Script::new(&b)));
        let mut ids = Vec::with_capacity(dlf.entities.len());
        for e in &dlf.entities {
            let (dir, name) = e.class.rsplit_once('/').unwrap_or(("", &e.class));
            let class_script = load(&format!("{}.asl", e.class));
            let over_script = load(&format!("{dir}/{name}_{:04}/{name}.asl", e.instance));
            let id = world.add_entity(EntityKind::from_class(&e.class), &e.class, e.instance, class_script, over_script);
            world.entity_mut(id).pos = [e.pos[0] + scene_pos.x, e.pos[1] + scene_pos.y, e.pos[2] + scene_pos.z];
            ids.push(id);
        }

        for &id in &ids {
            world.send_event(&mut host, None, id, "load", Vec::new());
        }
        for &id in &ids {
            world.send_init(&mut host, id);
        }
        for &id in &ids {
            world.send_event(&mut host, None, id, "game_ready", Vec::new());
        }
        world.update(&mut host, 0.0);
        Scripting { world, host, ids, player, ..default() }
    }

    fn load_anim(&mut self, pak: &PakSet, path: &str) -> Option<Arc<Tea>> {
        self.anim_cache.entry(path.to_owned()).or_insert_with(|| anims::load_anim(pak, path)).clone()
    }

    /// Cached animation lookup for the entity spawner.
    pub fn anim(&mut self, pak: &PakSet, path: &str) -> Option<Arc<Tea>> {
        self.load_anim(pak, path)
    }
}

/// Advance script time and deliver queued events.
pub fn tick(time: Res<Time>, fly: Res<Fly>, mut s: ResMut<Scripting>) {
    let s = &mut *s;
    if s.world.entities.is_empty() {
        return;
    }
    let feet = if fly.walk { fly.player.feet } else { fly.pos - Vec3::Y * arx_physics::EYE_HEIGHT };
    let player = s.player;
    s.world.entity_mut(player).pos = [feet.x, -feet.y, -feet.z];
    s.world.update(&mut s.host, time.delta_secs_f64().min(0.1) * 1000.0);
}

/// Copy script-driven state (visibility, scale, animation) onto the spawned entities.
pub fn apply_state(
    mut s: ResMut<Scripting>,
    arx: Res<Arx>,
    mut q: Query<(&ScriptRef, &mut Visibility, &mut Transform, Option<&mut Animated>)>,
) {
    let s = &mut *s;
    for (r, mut vis, mut tf, animated) in &mut q {
        let Some(st) = s.host.state(r.0) else { continue };
        let key = (st.revision, st.anim_serial);
        if s.applied.get(&r.0) == Some(&key) {
            continue;
        }
        let prev = s.applied.insert(r.0, key);
        let st = st.clone(); // release the borrow of `s.host` so animations can be loaded below
        *vis = if st.hidden || st.destroyed { Visibility::Hidden } else { Visibility::Inherited };
        tf.scale = Vec3::splat(st.scale);
        if let (Some(mut a), true) = (animated, prev.is_none_or(|(_, serial)| serial != st.anim_serial)) {
            if let Some(play) = &st.playing {
                if let Some(path) = st.anims.get(&play.slot).cloned() {
                    a.anim = s.load_anim(&arx.0, &path);
                    a.looping = play.looping;
                    a.elapsed_us = 0;
                }
            }
        }
    }
}

/// Find what the player is looking at, and send it `action` when E is pressed.
pub fn interact(
    keys: Res<ButtonInput<KeyCode>>,
    cam: Single<&Transform, With<Camera3d>>,
    pickables: Res<Pickables>,
    mut s: ResMut<Scripting>,
) {
    let s = &mut *s;
    let (origin, dir) = (cam.translation, cam.forward().as_vec3());
    let mut best: Option<(f32, EntityId)> = None;
    for p in &pickables.0 {
        let Some(st) = s.host.state(p.id) else { continue };
        if st.hidden || st.destroyed || !st.interactive {
            continue;
        }
        // Ray / sphere intersection.
        let to = p.center - origin;
        let along = to.dot(dir);
        if along < 0.0 || along > REACH + p.radius {
            continue;
        }
        let miss2 = to.length_squared() - along * along;
        if miss2 > p.radius * p.radius {
            continue;
        }
        let t = along - (p.radius * p.radius - miss2).sqrt();
        if t <= REACH && best.is_none_or(|(bt, _)| t < bt) {
            best = Some((t, p.id));
        }
    }
    s.target = best.map(|(_, id)| id);
    if keys.just_pressed(KeyCode::KeyE)
        && let Some(target) = s.target
    {
        let event = if s.world.entity(target).kind == EntityKind::Npc { "chat" } else { "action" };
        let player = s.player;
        s.world.send_event(&mut s.host, Some(player), target, event, Vec::new());
    }
}

/// Headless testing aid (`--use-entity id[:event]`): send an event (default `action`) from the
/// player to the named entities a moment after start-up, and report what the scripts did.
pub fn auto_use(mut frames: Local<u32>, args: Res<crate::LevelArgs>, mut s: ResMut<Scripting>) {
    *frames += 1;
    if *frames != 20 {
        return;
    }
    let s = &mut *s;
    let player = s.player;
    for spec in &args.use_entity {
        let (name, event) = spec.split_once(':').unwrap_or((spec, "action"));
        let Some(t) = s.world.find(name, player) else {
            eprintln!("no such entity: {name}");
            continue;
        };
        let result = s.world.send_event(&mut s.host, Some(player), t, event, Vec::new());
        s.world.update(&mut s.host, 0.0);
        let st = s.host.state(t);
        eprintln!(
            "sent {event} to {name}: {result:?}; playing {:?}, collision {:?}, interactive {:?}; open={}",
            st.and_then(|x| x.playing.as_ref().map(|p| p.slot.clone())),
            st.map(|x| x.collision),
            st.map(|x| x.interactive),
            s.world.entity(t).vars.get_int("\u{a7}open")
        );
    }
}
