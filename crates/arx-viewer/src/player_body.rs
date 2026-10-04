//! The hero's body in first person, and the fighting done with it. As in the original, the hero is the full human
//! model standing where the player stands, with the faces around the head and shoulders left out; looking down shows
//! legs and, when a weapon is drawn, the arms swinging it. The legs play the walking animation and the arms a
//! second animation on top (`1h_wait`, `1h_strike_left_cycle`, ...), the equipped weapon is attached to the hand,
//! and the blows that connect hurt what they touch.

use crate::animated::{Animated, Overlay};
use crate::convert::to_bevy;
use crate::entities::{EntityCache, EntityStats, LevelLights, SpawnOpts, SpawnedEntities, spawn_entity};
use crate::hud::Ui;
use crate::npcs::Npcs;
use crate::scripting::{Pickables, Scripting};
use crate::{Arx, Fly};
use arx_formats::ftl::Ftl;
use arx_level::npc::Env;
use arx_level::player_combat::{PlayerCombat, Stage};
use arx_script::{EntityId, SpeechEvent, SpeechFlags, SpeechRequest};
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use std::sync::Arc;

/// The model the hero is drawn with.
const BODY_CLASS: &str = "graph/obj3d/interactive/npc/human_base/human_base";

/// The weapon in the hand: its model, where it is held, and where its blade is.
struct WeaponVisual {
    item: EntityId,
    entity: Entity,
    /// The vertex of the weapon that goes in the hand (`primary_attach`), in the model's own coordinates (Arx).
    attach: Vec3,
    /// The places the blade can hurt: `hit_<radius>` vertices.
    hits: Vec<(Vec3, f32)>,
}

#[derive(Resource, Default)]
pub struct PlayerBody {
    pub entity: Option<Entity>,
    weapon: Option<WeaponVisual>,
    shield: Option<WeaponVisual>,
    base_slot: Option<String>,
    overlay_slot: Option<String>,
    /// Characters the current swing has hit.
    hit: Vec<EntityId>,
    /// Vertices of the body that carry things: the hand, the other hand.
    primary_attach: Option<usize>,
    left_attach: Option<usize>,
    shield_attach: Option<usize>,
}

#[derive(Resource, Default)]
pub struct Combat {
    pub state: PlayerCombat,
    /// A blow can land this frame (set by [`drive`], used by [`attach`]).
    blow: bool,
}

/// What building a model needs.
#[derive(SystemParam)]
pub struct Caches<'w> {
    pickables: ResMut<'w, Pickables>,
    spawned: ResMut<'w, SpawnedEntities>,
    ecache: ResMut<'w, EntityCache>,
    tcache: ResMut<'w, crate::convert::TextureCache>,
    meshes: ResMut<'w, Assets<Mesh>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
    images: ResMut<'w, Assets<Image>>,
}

/// Put the hero's body in the scene.
#[allow(clippy::too_many_arguments)]
pub fn spawn(
    commands: &mut Commands,
    pak: &arx_formats::PakSet,
    lights: &[crate::entities::StaticLight],
    scripting: &mut Scripting,
    pickables: &mut Vec<crate::scripting::Pickable>,
    ecache: &mut EntityCache,
    tcache: &mut crate::convert::TextureCache,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    feet: Vec3,
    body: &mut PlayerBody,
) {
    let player = scripting.player;
    // The hero's own script loads the animations; without it there is nothing to play.
    if !scripting.host.has_slot(player, "wait") {
        return;
    }
    let arx = [feet.x, -feet.y, -feet.z];
    let mut stats = EntityStats::default();
    let opts = SpawnOpts { hide_selection: Some("1st"), not_pickable: true, lenient_skeleton: true };
    let e = spawn_entity(commands, pak, lights, scripting, pickables, ecache, tcache, meshes, materials, images, BODY_CLASS, arx, [0.0; 3], 1, player, true, &mut stats, &opts);
    body.entity = e;
    eprintln!("player body: {e:?} stats {stats:?}");
    // Which vertices carry the weapon.
    if let Some(ftl) = pak.read("game/graph/obj3d/interactive/npc/human_base/human_base.ftl").ok().and_then(|b| Ftl::parse(&b).ok()) {
        let find = |name: &str| ftl.actions.iter().find(|a| a.name.eq_ignore_ascii_case(name)).map(|a| a.vertex as usize);
        body.primary_attach = find("primary_attach");
        body.left_attach = find("left_attach");
        body.shield_attach = find("shield_attach");
    }
}

/// Which animation the legs play, from what the player is doing.
fn legs_slot(fly: &Fly, keys: &ButtonInput<KeyCode>, fighting: bool, args_forward: bool) -> &'static str {
    let (w, s, a, d) = (keys.pressed(KeyCode::KeyW) || args_forward, keys.pressed(KeyCode::KeyS), keys.pressed(KeyCode::KeyA), keys.pressed(KeyCode::KeyD));
    let sneak = keys.pressed(KeyCode::ShiftLeft);
    let p = &fly.player;
    if !p.on_ground {
        return "jump_cycle";
    }
    let moving = w || s || a || d;
    if p.is_crouching() {
        return match (w, s, a, d) {
            (true, ..) => "crouch_walk",
            (_, true, ..) => "crouch_walk_backward",
            (_, _, true, _) => "crouch_strafe_left",
            (_, _, _, true) => "crouch_strafe_right",
            _ => "crouch_wait",
        };
    }
    if fighting {
        return match (w, s, a, d) {
            (true, ..) => "fight_walk_forward",
            (_, true, ..) => "fight_walk_backward",
            (_, _, true, _) => "fight_strafe_left",
            (_, _, _, true) => "fight_strafe_right",
            _ => "fight_wait",
        };
    }
    if !moving {
        return "wait";
    }
    match (w, s, a, d, sneak) {
        (true, .., true) => "walk",
        (true, ..) => "run",
        (_, true, .., true) => "walk_backward",
        (_, true, ..) => "run_backward",
        (_, _, true, _, true) => "strafe_left",
        (_, _, true, ..) => "strafe_run_left",
        (_, _, _, true, true) => "strafe_right",
        _ => "strafe_run_right",
    }
}

/// Place the body, choose its animations, run the fight (draw, wind up, strike) and show or hide the weapon.
#[allow(clippy::too_many_arguments)]
pub fn drive(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    shot: Option<Res<crate::Shot>>,
    args: Res<crate::LevelArgs>,
    fly: Res<Fly>,
    ui: Res<Ui>,
    arx: Res<Arx>,
    mut body: ResMut<PlayerBody>,
    mut combat: ResMut<Combat>,
    mut script: ResMut<Scripting>,
    mut q: Query<(&mut Transform, &mut Visibility, &mut Animated)>,
) {
    let Some(entity) = body.entity else { return };
    let live = shot.is_none();
    let dt_ms = time.delta_secs().min(0.1) * 1000.0;
    let s = &mut *script;
    let player = s.player;
    let dead = s.host.player.is_dead();

    // Combat controls: Tab draws or puts away the weapon; the left button winds up and strikes.
    let attack_held = live && mouse.pressed(MouseButton::Left) && !ui.cursor_mode && !dead && fly.walk;
    let mut updates = Vec::new();
    if live && keys.just_pressed(KeyCode::Tab) && !dead && fly.walk && !ui.open && ui.book.is_none() {
        updates.push(combat.state.toggle(&mut s.host, player));
    }
    if args.draw_weapon && combat.state.stage == Stage::Sheathed && fly.walk && (args.equip.is_empty() || s.host.player_weapon().is_some()) {
        updates.push(combat.state.toggle(&mut s.host, player));
    }
    if live {
        combat.state.steer(motion.delta);
    }
    // A hero with a drawn weapon who dies, or leaves walking mode, puts it away at once.
    if (dead || !fly.walk) && combat.state.stage != Stage::Sheathed {
        combat.state = PlayerCombat::default();
        s.host.player.fighting = false;
    }
    // `--attack-test` lets a scripted run swing by itself: wind up for a while, then let go.
    let scripted = args.attack_test && combat.state.stage != Stage::Sheathed && time.elapsed_secs() % 4.0 < 3.0;
    let main = combat.state.update(&mut s.host, player, dt_ms, attack_held || scripted);
    combat.blow = main.blow;
    updates.push(main);

    let Ok((mut tf, mut vis, mut anim)) = q.get_mut(entity) else { return };
    anim.keep_pose = true;
    // Where the hero stands, facing where they look.
    tf.translation = fly.player.feet;
    tf.rotation = Quat::from_rotation_y(fly.yaw + std::f32::consts::PI);
    *vis = if fly.walk && !dead { Visibility::Inherited } else { Visibility::Hidden };

    // Legs.
    let pressed: &ButtonInput<KeyCode> = if live { &keys } else { &ButtonInput::default() };
    let slot = legs_slot(&fly, pressed, combat.state.is_fighting(), args.walk_forward);
    if body.base_slot.as_deref() != Some(slot)
        && let Some(path) = s.host.state(player).and_then(|st| st.anims.get(slot)).cloned()
        && let Some(tea) = script_anim(s, &arx, &path)
    {
        anim.anim = Some(tea);
        anim.looping = true;
        anim.elapsed_us = 0;
        body.base_slot = Some(slot.to_owned());
    }

    // Arms.
    for up in updates {
        if let Some((slot, looping)) = up.animation {
            body.overlay_slot = slot.clone();
            anim.overlay = slot
                .and_then(|slot| s.host.state(player).and_then(|st| st.anims.get(&slot)).cloned())
                .and_then(|path| script_anim(s, &arx, &path))
                .map(|tea| Overlay { anim: tea, elapsed_us: 0, looping });
        }
        if up.struck {
            body.hit.clear();
            s.world.send_event(&mut s.host, None, player, "strike", vec![kind(&combat.state).to_owned()]);
            if up.grunt {
                let key = player_strike_speech(&s.host);
                s.host.push_speech(SpeechEvent::Say(SpeechRequest {
                    speaker: player,
                    script_entity: player,
                    key,
                    flags: SpeechFlags { no_text: true, ..default() },
                    on_end: None,
                }));
            }
        }
    }
}

fn kind(c: &PlayerCombat) -> &'static str {
    c.weapon().event_name()
}

/// What the hero shouts when a well-aimed blow is let go: the weapon's own `strikespeech`, or the usual.
fn player_strike_speech(host: &arx_script::StdHost) -> String {
    host.player_weapon()
        .and_then(|w| host.state(w))
        .map(|s| s.strike_speech.trim_matches(|c| c == '[' || c == ']').to_ascii_lowercase())
        .filter(|k| !k.is_empty())
        .unwrap_or_else(|| "player_strike".to_owned())
}

fn script_anim(s: &mut Scripting, arx: &Arx, path: &str) -> Option<Arc<arx_formats::tea::Tea>> {
    s.anim(&arx.0, path)
}

/// Build the model of something held (a weapon in the hand, a shield on the arm) and find where it is gripped.
#[allow(clippy::too_many_arguments)]
fn make_visual(
    commands: &mut Commands,
    arx: &Arx,
    lights: &LevelLights,
    s: &mut Scripting,
    caches: &mut Caches,
    item: EntityId,
    grip: &str,
) -> Option<WeaponVisual> {
    let (class, instance) = {
        let e = s.world.entity(item);
        (e.class.clone(), e.instance)
    };
    let mut stats = EntityStats::default();
    let opts = SpawnOpts { hide_selection: None, not_pickable: true, lenient_skeleton: false };
    let made = spawn_entity(
        commands, &arx.0, &lights.0, s, &mut caches.pickables.0, &mut caches.ecache, &mut caches.tcache, &mut caches.meshes, &mut caches.materials, &mut caches.images,
        &class, [0.0; 3], [0.0; 3], instance, item, true, &mut stats, &opts,
    );
    let Some(e) = made else {
        eprintln!("could not build the model of {class}: {stats:?}");
        return None;
    };
    commands.entity(e).insert(WeaponTag);
    caches.spawned.0.insert(item);
    let model = s.host.state(item).and_then(|st| st.mesh.clone()).unwrap_or_else(|| class.clone());
    let ftl = arx.0.read(&format!("game/{model}.ftl")).ok().and_then(|b| Ftl::parse(&b).ok());
    let (attach, hits) = ftl.map_or((Vec3::ZERO, Vec::new()), |f| {
        let v = |i: u32| Vec3::from(f.vertices[i as usize].pos);
        let attach = f.actions.iter().find(|a| a.name.eq_ignore_ascii_case(grip)).map_or_else(|| v(f.origin), |a| v(a.vertex));
        let hits = f
            .actions
            .iter()
            .filter_map(|a| a.name.to_ascii_lowercase().strip_prefix("hit_").and_then(|n| n.parse::<f32>().ok()).map(|r| (v(a.vertex), r)))
            .collect();
        (attach, hits)
    });
    Some(WeaponVisual { item, entity: e, attach, hits })
}

/// Make the held model follow what is equipped in a slot (building or removing it).
#[allow(clippy::too_many_arguments)]
fn follow_equipment(
    commands: &mut Commands,
    arx: &Arx,
    lights: &LevelLights,
    s: &mut Scripting,
    caches: &mut Caches,
    slot: &mut Option<WeaponVisual>,
    equipped: Option<EntityId>,
    grip: &str,
) {
    if slot.as_ref().map(|w| w.item) == equipped {
        return;
    }
    if let Some(old) = slot.take() {
        commands.entity(old.entity).despawn();
    }
    if let Some(item) = equipped {
        *slot = make_visual(commands, arx, lights, s, caches, item, grip);
    }
}

/// Where a held model goes so that its grip point sits at `vertex` of the posed body: the bone's rotation turns it,
/// its own grip vertex is the pivot.
fn held_transform(w: &WeaponVisual, vertex: usize, anim: &Animated, body_tf: &Transform) -> Option<Transform> {
    let pose = anim.pose.as_ref()?;
    let flip = |v: Vec3| Vec3::from(to_bevy(v.to_array()));
    let bone = anim.skeleton.vertex_bone[vertex];
    // Bone rotation in Bevy axes: conjugate by the Arx -> Bevy flip.
    let q = pose.bone_quat[bone];
    let q_b = Quat::from_xyzw(q.x, -q.y, -q.z, q.w);
    let local = Transform { translation: flip(pose.vertices[vertex]) - q_b * flip(w.attach), rotation: q_b, scale: Vec3::ONE };
    Some(body_tf.mul_transform(local))
}

/// Keep the weapon model in the hand and the shield on the arm, and land blows: after the animations moved the arm,
/// find where the weapon is.
#[allow(clippy::too_many_arguments)]
pub fn attach(
    mut commands: Commands,
    arx: Res<Arx>,
    fly: Res<Fly>,
    lights: Res<LevelLights>,
    mut body: ResMut<PlayerBody>,
    mut combat: ResMut<Combat>,
    mut script: ResMut<Scripting>,
    mut npcs: ResMut<Npcs>,
    mut caches: Caches,
    bodies: Query<(&Transform, &Animated), Without<WeaponTag>>,
    mut held: Query<(&mut Transform, &mut Visibility), With<WeaponTag>>,
) {
    let Some(body_entity) = body.entity else { return };
    let s = &mut *script;
    let (weapon_item, shield_item) = (s.host.player_weapon(), s.host.player.equipped_in(arx_script::EquipSlot::Shield));
    let b = &mut *body;
    follow_equipment(&mut commands, &arx, &lights, s, &mut caches, &mut b.weapon, weapon_item, "primary_attach");
    follow_equipment(&mut commands, &arx, &lights, s, &mut caches, &mut b.shield, shield_item, "shield_attach");

    let Ok((body_tf, anim)) = bodies.get(body_entity) else { return };
    let Some(pose) = anim.pose.as_ref() else { return };
    let fighting = combat.state.stage != Stage::Sheathed;
    let flip = |v: Vec3| Vec3::from(to_bevy(v.to_array()));
    if std::env::var_os("ARX_LOG_BODY").is_some() {
        eprintln!("body at {:?}; slot {:?}/{:?}; weapon {} shield {}", body_tf.translation, body.base_slot, body.overlay_slot, body.weapon.is_some(), body.shield.is_some());
    }

    // The weapon in the hand (only while it is drawn), and the shield on the left arm.
    let mut centres: Vec<(Vec3, f32)> = Vec::new();
    if let (Some(w), Some(v)) = (&body.weapon, body.primary_attach)
        && let Ok((mut tf, mut vis)) = held.get_mut(w.entity)
        && let Some(t) = held_transform(w, v, anim, body_tf)
    {
        *tf = t;
        *vis = if fighting { Visibility::Inherited } else { Visibility::Hidden };
        centres = w.hits.iter().map(|&(v, r)| (tf.transform_point(flip(v)), r)).collect();
    }
    if let (Some(w), Some(v)) = (&body.shield, body.shield_attach)
        && let Ok((mut tf, mut vis)) = held.get_mut(w.entity)
        && let Some(t) = held_transform(w, v, anim, body_tf)
    {
        *tf = t;
        *vis = Visibility::Inherited;
    }
    if body.weapon.is_none() {
        // Bare hands: the fists themselves.
        for v in [body.primary_attach, body.left_attach].into_iter().flatten() {
            centres.push((body_tf.transform_point(flip(pose.vertices[v])), 40.0));
        }
    }

    // A blow that can land now: whatever the blade touches is hurt, once.
    if combat.blow && std::env::var_os("ARX_LOG_BODY").is_some() {
        eprintln!("blow window: {} spheres {:?}", centres.len(), centres.iter().map(|(c, r)| (c.round(), *r)).collect::<Vec<_>>());
    }
    if combat.blow && !centres.is_empty() {
        let Some(world) = fly.world.as_ref() else { return };
        let Some(npcs) = npcs.0.as_mut() else { return };
        let spheres: Vec<(Vec3, f32)> = centres.iter().map(|&(c, r)| (Vec3::new(c.x, -c.y, -c.z), r)).collect();
        let env = Env {
            collision: world,
            player_pos: Vec3::from(s.world.entity(s.player).pos),
            player_alive: !s.host.player.is_dead(),
            player_stealth: 15.0,
            player_light: 255.0,
            player_torch: false,
        };
        let ratio = combat.state.strike_ratio;
        let mut hit = std::mem::take(&mut body.hit);
        let impacts = npcs.player_strike(&mut s.world, &mut s.host, &env, &spheres, ratio, &mut hit);
        body.hit = hit;
        if !impacts.is_empty() {
            combat.state.landed();
            let kind = combat.state.weapon();
            for i in &impacts {
                eprintln!("blow ({kind:?}, aim {ratio:.2}) on {}: damage {:.1}{}{}", s.world.entity(i.target).id_string, i.damage, if i.missed { " (missed)" } else { "" }, if i.killed { " (killed)" } else { "" });
            }
        }
    }
}

/// Marks the weapon model in the hand.
#[derive(Component)]
pub struct WeaponTag;

/// Headless testing aid (`--equip id,id`): once the level has settled the hero takes these items and uses them.
pub fn debug_equip(mut frames: Local<u32>, args: Res<crate::LevelArgs>, mut script: ResMut<Scripting>) {
    if args.equip.is_empty() {
        return;
    }
    *frames += 1;
    if *frames != 12 {
        return;
    }
    let s = &mut *script;
    let player = s.player;
    if let [st, mi, de, co] = args.attrs[..] {
        s.host.player.attributes = arx_script::Attributes { strength: st, mind: mi, dexterity: de, constitution: co };
        s.host.player.recompute();
        s.host.publish_player(&mut s.world);
    }
    for name in &args.equip {
        let Some(item) = s.world.find(name, player) else {
            eprintln!("--equip: no entity named {name}");
            continue;
        };
        let carried = s.host.carry(&s.world, item);
        arx_level::inventory::use_item(&mut s.world, &mut s.host, player, item);
        s.world.update(&mut s.host, 0.0);
        eprintln!("equip {name}: carried {carried:?}; wielding {:?}", s.host.player_weapon().map(|w| s.world.entity(w).id_string.clone()));
    }
}
