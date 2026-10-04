//! Runs the level's entity scripts and reflects their effects in the scene: visibility, scale,
//! animations, door collision, and what the player is looking at.

use crate::animated::Animated;
use crate::anims;
use crate::convert::to_bevy;
use crate::speech::Speech;
use crate::{Arx, Fly};
use arx_formats::{dlf::Dlf, tea::Tea};
use arx_level::{EntityObstacles, Scripts, inventory};
use arx_physics::CollisionWorld;
use arx_script::{EntityId, EntityKind, ScriptWorld, StdHost};
use bevy::prelude::*;
use std::{collections::HashMap, sync::Arc};

/// How far the player can reach to use something.
const REACH: f32 = 350.0;

#[derive(Component)]
pub struct ScriptRef(pub EntityId);

/// The angles the level gave an entity (Arx pitch, yaw, roll in degrees), which `rotate` adds to.
#[derive(Component)]
pub struct BaseAngle {
    pub angle: [f32; 3],
    pub npc: bool,
}

/// A clickable bounding sphere for an entity (Bevy coordinates).
pub struct Pickable {
    pub id: EntityId,
    /// Centre of the sphere relative to the entity's position, so a dropped item can be picked up again.
    pub offset: Vec3,
    pub radius: f32,
}

#[derive(Resource, Default)]
pub struct Pickables(pub Vec<Pickable>);

/// Entity obstacles in the shared collision world, kept in step with the scripts.
#[derive(Resource, Default)]
pub struct Obstacles {
    pub entities: EntityObstacles,
    pub world: Option<Arc<CollisionWorld>>,
}

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
    /// Load every entity's scripts and run the level start-up sequence (see [`Scripts::build`]).
    pub fn build(pak: &Arc<arx_formats::PakSet>, dlf: &Dlf, scene_pos: Vec3) -> Scripting {
        let Scripts { world, host, ids, player } = Scripts::build(pak, dlf, scene_pos);
        Scripting { world, host, ids, player, ..default() }
    }

    fn load_anim(&mut self, pak: &arx_formats::PakSet, path: &str) -> Option<Arc<Tea>> {
        self.anim_cache.entry(path.to_owned()).or_insert_with(|| anims::load_anim(pak, path)).clone()
    }

    /// Cached animation lookup for the entity spawner.
    pub fn anim(&mut self, pak: &arx_formats::PakSet, path: &str) -> Option<Arc<Tea>> {
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
    s.host.publish_player(&mut s.world);
    s.world.update(&mut s.host, time.delta_secs_f64().min(0.1) * 1000.0);
}

/// Make doors and other entities solid or passable as their scripts say (`collision on/off`,
/// hidden, destroyed).
pub fn sync_obstacles(s: Res<Scripting>, o: Res<Obstacles>) {
    if let Some(world) = &o.world {
        o.entities.sync(world, &s.host);
    }
}

/// Copy script-driven state (visibility, scale, animation) onto the spawned entities.
pub fn apply_state(
    mut s: ResMut<Scripting>,
    arx: Res<Arx>,
    mut q: Query<(&ScriptRef, &mut Visibility, &mut Transform, Option<&mut Animated>, Option<&BaseAngle>)>,
) {
    let s = &mut *s;
    for (r, mut vis, mut tf, animated, base) in &mut q {
        let Some(st) = s.host.state(r.0) else { continue };
        let key = (st.revision, st.anim_serial);
        if s.applied.get(&r.0) == Some(&key) {
            continue;
        }
        let prev = s.applied.insert(r.0, key);
        let st = st.clone(); // release the borrow of `s.host` so animations can be loaded below
        *vis = if st.hidden || st.destroyed { Visibility::Hidden } else { Visibility::Inherited };
        tf.scale = Vec3::splat(st.scale);
        if let Some(p) = st.moved_to {
            tf.translation = Vec3::from(to_bevy(p));
        }
        if let (Some(b), true) = (base, st.rotation != [0.0; 3]) {
            let a = [b.angle[0] + st.rotation[0], b.angle[1] + st.rotation[1], b.angle[2] + st.rotation[2]];
            tf.rotation = arx_level::entity_rotation(a, b.npc);
        }
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

/// The nearest interactive entity a ray (from the camera, through the cursor or the middle of the screen) passes
/// through within `reach`: its distance and id.
pub fn pick_ray(pickables: &Pickables, s: &Scripting, origin: Vec3, dir: Vec3, reach: f32) -> Option<(f32, EntityId)> {
    let mut best: Option<(f32, EntityId)> = None;
    for p in &pickables.0 {
        let Some(st) = s.host.state(p.id) else { continue };
        if st.hidden || st.destroyed || !st.interactive || st.in_inventory {
            continue;
        }
        let center = Vec3::from(to_bevy(s.world.entity(p.id).pos)) + p.offset;
        let to = center - origin;
        let along = to.dot(dir);
        if along < 0.0 || along > reach + p.radius {
            continue;
        }
        let miss2 = to.length_squared() - along * along;
        if miss2 > p.radius * p.radius {
            continue;
        }
        let t = along - (p.radius * p.radius - miss2).sqrt();
        if t <= reach && best.is_none_or(|(bt, _)| t < bt) {
            best = Some((t, p.id));
        }
    }
    best
}

/// Find what the player is looking at, and act on it when E is pressed: with an item held, use the item on it;
/// otherwise pick up items, talk to people, and send `action` to everything else.
pub fn interact(
    keys: Res<ButtonInput<KeyCode>>,
    cam: Single<&Transform, With<Camera3d>>,
    pickables: Res<Pickables>,
    mut ui: ResMut<crate::hud::Ui>,
    speech: Res<Speech>,
    npcs: Res<crate::npcs::Npcs>,
    mut s: ResMut<Scripting>,
) {
    let s = &mut *s;
    let (origin, dir) = (cam.translation, cam.forward().as_vec3());
    let mut best: Option<(f32, EntityId)> = None;
    for p in &pickables.0 {
        let Some(st) = s.host.state(p.id) else { continue };
        if st.hidden || st.destroyed || !st.interactive || st.in_inventory {
            continue;
        }
        let center = Vec3::from(to_bevy(s.world.entity(p.id).pos)) + p.offset;
        // Ray / sphere intersection.
        let to = center - origin;
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
    if !keys.just_pressed(KeyCode::KeyE) || ui.reading.is_some() {
        return;
    }
    let Some(target) = s.target else { return };
    let player = s.player;
    if let Some(held) = ui.held {
        inventory::combine(&mut s.world, &mut s.host, player, held, target);
        if !s.host.player.inventory.contains(&held) {
            ui.held = None;
        }
        return;
    }
    // A dead character is searched like a chest.
    let dead = npcs.0.as_ref().and_then(|n| n.npc(target)).is_some_and(|n| n.dead);
    if dead && s.host.containers.contains_key(&target) {
        if s.host.open_container == Some(target) {
            inventory::close_container(&mut s.world, &mut s.host, player);
        } else {
            inventory::open_container(&mut s.world, &mut s.host, player, target);
        }
        return;
    }
    match s.world.entity(target).kind {
        EntityKind::Item => {
            let name = crate::hud::display_name(s, &speech, target);
            match inventory::pick_up(&mut s.world, &mut s.host, player, target) {
                inventory::PickUp::Refused("no room") => s.host.push_message("Your inventory is full".to_owned()),
                inventory::PickUp::Refused(_) => {}
                inventory::PickUp::Gold(n) => s.host.push_message(format!("{n} gold")),
                _ => s.host.push_message(format!("Picked up {name}")),
            }
        }
        EntityKind::Npc => {
            s.world.send_event(&mut s.host, Some(player), target, "chat", Vec::new());
        }
        // Chests and the like: look inside (a locked one refuses, and says so).
        _ if inventory::is_container(&s.world, &s.host, target) => {
            if s.host.open_container == Some(target) {
                inventory::close_container(&mut s.world, &mut s.host, player);
            } else {
                inventory::open_container(&mut s.world, &mut s.host, player, target);
            }
        }
        _ => {
            s.world.send_event(&mut s.host, Some(player), target, "action", Vec::new());
        }
    }
}

/// Headless testing aid (`--use-entity id[:event]`): send an event (default `action`) from the
/// player to the named entities a moment after start-up, and report what the scripts did.
pub fn auto_use(mut frames: Local<u32>, args: Res<crate::LevelArgs>, mut s: ResMut<Scripting>, o: Res<Obstacles>) {
    *frames += 1;
    if *frames == 40 {
        // One frame after the events took effect, report whether the doors are solid now.
        for spec in &args.use_entity {
            let name = spec.split_once(':').map_or(spec.as_str(), |(n, _)| n);
            if let Some(t) = s.world.find(name, s.player) {
                let solid = o.entities.by_entity.get(&t).zip(o.world.as_ref()).map(|(id, w)| w.obstacle_enabled(*id));
                eprintln!("{name}: obstacle solid after the script ran: {solid:?}");
            }
        }
        return;
    }
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
        let solid = o.entities.by_entity.get(&t).zip(o.world.as_ref()).map(|(id, w)| w.obstacle_enabled(*id));
        eprintln!(
            "sent {event} to {name}: {result:?}; playing {:?}, collision flag {:?}, interactive {:?}; open={}; obstacle currently solid: {solid:?} (applied next frame)",
            st.and_then(|x| x.playing.as_ref().map(|p| p.slot.clone())),
            st.map(|x| x.collision),
            st.map(|x| x.interactive),
            s.world.entity(t).vars.get_int("\u{a7}open")
        );
    }
}
