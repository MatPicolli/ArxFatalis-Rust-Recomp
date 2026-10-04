//! The hero's body in first person, and the fighting done with it. As in the original, the hero is the full human
//! model standing where the player stands, with the faces around the head and shoulders left out; looking down shows
//! legs and, when a weapon is drawn, the arms swinging it. The legs play the walking animation and the arms a
//! second animation on top (`1h_wait`, `1h_strike_left_cycle`, ...), the equipped weapon is attached to the hand,
//! and the blows that connect hurt what they touch.

use crate::animated::{Animated, Overlay};
use crate::convert::to_bevy;
use crate::entities::{EntityCache, EntityStats, LevelLights, SpawnOpts, SpawnedEntities, resolve_model, spawn_entity};
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
pub const BODY_CLASS: &str = "graph/obj3d/interactive/npc/human_base/human_base";

/// The weapon in the hand: its model, where it is held, and where its blade is.
pub struct WeaponVisual {
    pub item: EntityId,
    pub entity: Entity,
    /// The vertex of the weapon that goes in the hand (`primary_attach`), in the model's own coordinates (Arx).
    attach: Vec3,
    /// The places the blade can hurt: `hit_<radius>` vertices.
    hits: Vec<(Vec3, f32)>,
}

#[derive(Resource, Default)]
pub struct PlayerBody {
    pub entity: Option<Entity>,
    /// The same body with its head and chest, shown when the scene is seen from outside (a cutscene camera).
    outside: Option<Entity>,
    weapon: Option<WeaponVisual>,
    shield: Option<WeaponVisual>,
    base_slot: Option<String>,
    outside_slot: Option<String>,
    overlay_slot: Option<String>,
    /// Characters the current swing has hit.
    hit: Vec<EntityId>,
    /// Vertices of the body that carry things: the hand, the other hand.
    primary_attach: Option<usize>,
    left_attach: Option<usize>,
    shield_attach: Option<usize>,
    /// The vertex the eyes are at (`view_attach`): the camera sits there, as in the original.
    view_attach: Option<usize>,
    /// The model serial the bodies were built at (armour and the chosen face change the model).
    serial: u32,
    /// The bones the body bends at to look up and down: head, neck, chest, belt.
    bend_bones: [Option<usize>; 4],
    /// How high above the feet the eyes are shown (it follows the body, but eases through a crouch).
    eye_rise: Option<f32>,
    /// What scripts last played on the hero, to notice when they play something else.
    script_key: Option<(u32, Option<String>)>,
    /// A pose scripts gave the hero (lying in the cell, being dragged, sitting at the council): the slot, whether it
    /// loops, how long it still has if it does not, and how the animation itself moves the body (Arx units per
    /// millisecond, in the model's own axes).
    scripted: Option<(String, bool, f32, Vec3)>,
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
    pub pickables: ResMut<'w, Pickables>,
    pub spawned: ResMut<'w, SpawnedEntities>,
    pub ecache: ResMut<'w, EntityCache>,
    pub tcache: ResMut<'w, crate::convert::TextureCache>,
    pub meshes: ResMut<'w, Assets<Mesh>>,
    pub materials: ResMut<'w, Assets<StandardMaterial>>,
    pub images: ResMut<'w, Assets<Image>>,
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
    let opts = SpawnOpts { hide_selection: Some("1st"), not_pickable: true };
    let e = spawn_entity(commands, pak, lights, scripting, pickables, ecache, tcache, meshes, materials, images, BODY_CLASS, arx, [0.0; 3], 1, player, true, &mut stats, &opts);
    body.entity = e;
    let whole = SpawnOpts { hide_selection: None, not_pickable: true };
    body.outside = spawn_entity(commands, pak, lights, scripting, pickables, ecache, tcache, meshes, materials, images, BODY_CLASS, arx, [0.0; 3], 1, player, true, &mut stats, &whole);
    // Which vertices carry the weapon, on the model as it is now (armour changes it).
    let st = scripting.host.state(player).cloned().unwrap_or_default();
    body.serial = st.model_serial;
    if let Some((ftl, _)) = resolve_model(ecache, pak, BODY_CLASS, &st) {
        let find = |name: &str| ftl.actions.iter().find(|a| a.name.eq_ignore_ascii_case(name)).map(|a| a.vertex as usize);
        body.primary_attach = find("primary_attach");
        body.left_attach = find("left_attach");
        body.shield_attach = find("shield_attach");
        body.view_attach = find("view_attach");
        // A bone is a group of the model, in the same order.
        let bone = |name: &str| ftl.groups.iter().position(|g| g.name.eq_ignore_ascii_case(name));
        body.bend_bones = [bone("head"), bone("neck"), bone("chest"), bone("belt")];
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
        // In first person the engine idles with the short wait (`player_wait_1st`), made to be seen from the eyes.
        return "wait_short";
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
    mut fly: ResMut<Fly>,
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
    let attack_held = live && mouse.pressed(MouseButton::Left) && !ui.cursor_mode && !dead && fly.walk && s.host.stage.controls;
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
    // A hero with a drawn weapon who dies, leaves walking mode or loses the controls to a cutscene puts it away at
    // once and stands up (`ARX_PLAYER_PutPlayerInNormalStance`); otherwise the body seen from outside would hold a
    // fighting stance with nothing moving its arms.
    let mut put_away = false;
    if (dead || !fly.walk || !s.host.stage.controls) && combat.state.stage != Stage::Sheathed {
        combat.state = PlayerCombat::default();
        s.host.player.fighting = false;
        updates.clear();
        put_away = true;
    }
    if !s.host.stage.controls {
        fly.crouch_toggle = false;
    }
    // `--attack-test` lets a scripted run swing by itself: wind up for a while, then let go.
    let scripted = args.attack_test && combat.state.stage != Stage::Sheathed && time.elapsed_secs() % 4.0 < 3.0;
    let main = combat.state.update(&mut s.host, player, dt_ms, attack_held || scripted);
    combat.blow = main.blow;
    updates.push(main);

    // Seen from outside (a cutscene camera), the whole body is shown instead of the one without head and chest.
    let from_outside = s.host.stage.camera.is_some();
    // A pose scripts gave the hero (`playanim -p`): it replaces the body's own animation until scripts play `wait` or
    // `none`, or, if it does not loop, until it ends.
    let key = s.host.state(player).map(|st| (st.anim_serial, st.playing.as_ref().map(|p| p.slot.clone())));
    if key != body.script_key {
        let playing = s.host.state(player).and_then(|st| st.playing.clone()).filter(|p| !matches!(p.slot.as_str(), "wait" | "none"));
        // (The first look is the level's start, not a pose.)
        body.scripted = if body.script_key.is_some() || playing.as_ref().is_some_and(|p| p.slot.starts_with("action")) {
            playing.and_then(|p| {
                let path = s.host.state(player).and_then(|st| st.anims.get(&p.slot)).cloned()?;
                let tea = script_anim(s, &arx, &path)?;
                let length_ms = (tea.duration_us as f32 / 1000.0).max(1.0);
                let drift = tea.frames.last().map_or(Vec3::ZERO, |f| Vec3::from(f.translate)) / length_ms;
                Some((p.slot, p.looping, length_ms, drift))
            })
        } else {
            None
        };
        body.script_key = key;
    }
    if let Some((_, looping, left_ms, drift)) = &mut body.scripted {
        // The pose carries the hero along (dragged across the floor), as the engine adds an animation's own movement
        // to whoever plays it: no collision, no gravity.
        let step = *drift * dt_ms;
        if step != Vec3::ZERO {
            let turned = Quat::from_rotation_y(fly.yaw + std::f32::consts::PI) * Vec3::new(step.x, -step.y, -step.z);
            fly.player.feet += turned;
        }
        *left_ms -= dt_ms;
        if !*looping && *left_ms <= 0.0 {
            body.scripted = None;
        }
    }
    fly.posed = body.scripted.as_ref().is_some_and(|s| s.3 != Vec3::ZERO);
    if std::env::var_os("ARX_LOG_BODY").is_some() && body.scripted.is_some() {
        eprintln!("[{:.1}s] hero posed {:?} at {:.0},{:.0},{:.0} (Arx) yaw {:.0}", s.world.now_ms / 1000.0, body.scripted.as_ref().map(|p| &p.0), fly.player.feet.x, -fly.player.feet.y, -fly.player.feet.z, fly.yaw.to_degrees());
    }
    // The animation of the whole body: what it is called here, its file, and whether it repeats.
    let base_path: Option<(String, String, bool)> = {
        let pressed: &ButtonInput<KeyCode> = if live && s.host.stage.controls { &keys } else { &ButtonInput::default() };
        let (slot, looping) = match &body.scripted {
            Some((slot, looping, ..)) => (slot.clone(), *looping),
            None => (legs_slot(&fly, pressed, combat.state.is_fighting(), args.walk_forward && s.host.stage.controls).to_owned(), true),
        };
        s.host.state(player).and_then(|st| st.anims.get(&slot)).cloned().map(|p| (format!("{slot}|{p}"), p, looping))
    };
    if let Some(outside) = body.outside
        && let Ok((mut tf, mut vis, mut anim)) = q.get_mut(outside)
    {
        tf.translation = fly.player.feet;
        tf.rotation = Quat::from_rotation_y(fly.yaw + std::f32::consts::PI);
        *vis = if fly.walk && from_outside { Visibility::Inherited } else { Visibility::Hidden };
        if let Some((slot, path, looping)) = &base_path
            && body.outside_slot.as_deref() != Some(slot)
            && let Some(tea) = script_anim(s, &arx, path)
        {
            anim.anim = Some(tea);
            anim.looping = *looping;
            anim.elapsed_us = 0;
            body.outside_slot = Some(slot.clone());
        }
    }
    let Ok((mut tf, mut vis, mut anim)) = q.get_mut(entity) else { return };
    anim.keep_pose = true;
    if put_away {
        anim.overlay = None;
        body.overlay_slot = None;
    }
    // The body bends to where the hero looks (the engine's extra rotations): a tenth of the pitch at the head and
    // at the neck and four tenths at the chest and at the belt with a weapon drawn, so the arms and the blade swing
    // where the eyes point; a quarter at each otherwise.
    anim.bend.clear();
    if body.scripted.is_none() && !dead {
        let pitch = -fly.pitch;
        let share = if combat.state.is_fighting() { [0.1, 0.1, 0.4, 0.4] } else { [0.25; 4] };
        for (bone, part) in body.bend_bones.iter().zip(share) {
            if let Some(bone) = bone {
                anim.bend.push((*bone, Quat::from_rotation_x(pitch * part)));
            }
        }
    }
    // Where the hero stands, facing where they look.
    tf.translation = fly.player.feet;
    tf.rotation = Quat::from_rotation_y(fly.yaw + std::f32::consts::PI);
    *vis = if fly.walk && !dead && !from_outside { Visibility::Inherited } else { Visibility::Hidden };

    // Legs.
    if let Some((slot, path, looping)) = &base_path
        && body.base_slot.as_deref() != Some(slot)
        && let Some(tea) = script_anim(s, &arx, path)
    {
        anim.anim = Some(tea);
        anim.looping = *looping;
        anim.elapsed_us = 0;
        body.base_slot = Some(slot.clone());
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
pub fn make_visual(
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
    let opts = SpawnOpts { hide_selection: None, not_pickable: true };
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
pub fn held_transform(w: &WeaponVisual, vertex: usize, anim: &Animated, body_tf: &Transform) -> Option<Transform> {
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
    time: Res<Time>,
    lights: Res<LevelLights>,
    mut body: ResMut<PlayerBody>,
    mut combat: ResMut<Combat>,
    mut script: ResMut<Scripting>,
    mut npcs: ResMut<Npcs>,
    mut caches: Caches,
    bodies: Query<(&Transform, &Animated), (Without<WeaponTag>, Without<Camera3d>)>,
    mut held: Query<(&mut Transform, &mut Visibility), (With<WeaponTag>, Without<Camera3d>)>,
    mut camera: Single<&mut Transform, (With<Camera3d>, Without<crate::book_hero::BookCamera>)>,
) {
    let Some(body_entity) = body.entity else { return };
    let s = &mut *script;
    // Armour put on or taken off, or another face: the model is another one, so the bodies are built again.
    if s.host.state(s.player).is_some_and(|st| st.model_serial != body.serial) {
        for e in [body.entity.take(), body.outside.take()].into_iter().flatten() {
            commands.entity(e).despawn();
        }
        (body.base_slot, body.outside_slot, body.overlay_slot) = (None, None, None);
        spawn(
            &mut commands, &arx.0, &lights.0, s, &mut caches.pickables.0, &mut caches.ecache, &mut caches.tcache, &mut caches.meshes, &mut caches.materials, &mut caches.images,
            fly.player.feet, &mut body,
        );
        return;
    }
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

    // (A cutscene camera, if one is active, takes over after this.)
    // The eyes: the camera sits at the body's `view_attach` vertex (so it bobs with the animation and is in front of
    // the chest, not inside it), never more than 46 units off the body's axis.
    if let Some(v) = body.view_attach
        && fly.walk
        && !s.host.player.is_dead()
    {
        let mut eye = body_tf.transform_point(flip(pose.vertices[v]));
        let off = Vec2::new(eye.x - body_tf.translation.x, eye.z - body_tf.translation.z);
        if off.length() > 46.0 {
            let o = off * (46.0 / off.length());
            eye.x = body_tf.translation.x + o.x;
            eye.z = body_tf.translation.z + o.y;
        }
        // The pose changes at once when the hero ducks or stands up; the eyes ease into it instead of snapping.
        let rise = eye.y - body_tf.translation.y;
        let shown = match body.eye_rise {
            Some(now) if (rise - now).abs() > 3.0 && (rise - now).abs() < 90.0 => now + (rise - now) * (time.delta_secs() * 14.0).min(1.0),
            _ => rise,
        };
        body.eye_rise = Some(shown);
        eye.y = body_tf.translation.y + shown;
        camera.translation = eye;
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
