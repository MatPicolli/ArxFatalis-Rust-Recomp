//! Cutscenes as the level scripts stage them: a camera entity the scene is seen through (often gliding along one of
//! the level's paths, looking at somebody), black bars, fades of the whole picture, the player's controls taken away
//! and given back, and the hero moved or turned by `teleport` and `playerlookat`. The logic is in `arx_level::stage`.

use crate::convert::to_bevy;
use crate::npcs::Npcs;
use crate::scripting::Scripting;
use crate::Fly;
use arx_level::stage::{StageEffect, StageWorld, fade_level};
use bevy::prelude::*;

/// The vertical field of view of the hero's eyes (the engine's focal 350 would be 69 degrees; the viewer uses 75).
pub const PLAYER_FOV: f32 = 75.0;
/// Height of each black bar at full size, in the original's 480-line screen.
const BAR_HEIGHT: f32 = 100.0;

#[derive(Resource, Default)]
pub struct Stage(pub StageWorld, /** `--no-cutscenes`: scripts still run, but the view, the controls and the hero stay the player's. */ pub bool);

#[derive(Component)]
pub struct Bar {
    top: bool,
}

#[derive(Component)]
pub struct FadeOverlay;

/// The bars and the fade are plain interface rectangles over the scene.
pub fn spawn_ui(mut commands: Commands) {
    for top in [true, false] {
        commands.spawn((
            Bar { top },
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                right: Val::Px(0.0),
                top: if top { Val::Px(0.0) } else { Val::Auto },
                bottom: if top { Val::Auto } else { Val::Px(0.0) },
                height: Val::Px(0.0),
                ..default()
            },
            BackgroundColor(Color::BLACK),
            GlobalZIndex(40),
        ));
    }
    commands.spawn((
        FadeOverlay,
        Node { position_type: PositionType::Absolute, left: Val::Px(0.0), right: Val::Px(0.0), top: Val::Px(0.0), bottom: Val::Px(0.0), ..default() },
        BackgroundColor(Color::NONE),
        GlobalZIndex(30),
    ));
}

/// Move what follows a path, and carry out the jumps and turns scripts asked for.
pub fn update(time: Res<Time>, mut stage: ResMut<Stage>, mut npcs: ResMut<Npcs>, mut s: ResMut<Scripting>, mut fly: ResMut<Fly>, mut last: Local<Option<arx_script::Stage>>) {
    let s = &mut *s;
    if s.world.entities.is_empty() {
        return;
    }
    let dt_ms = time.delta_secs().min(0.1) * 1000.0;
    let log = std::env::var_os("ARX_LOG_STAGE").is_some();
    let before = s.host.stage.clone();
    let mut effects = stage.0.update(&mut s.world, &mut s.host, npcs.0.as_mut(), dt_ms);
    if stage.1 {
        effects.clear();
        let st = &mut s.host.stage;
        (st.controls, st.cinemascope, st.interface_hidden, st.camera, st.fade) = (true, false, false, None, None);
    }
    if log {
        for e in &effects {
            eprintln!("[{:.1}s] stage: {e:?}", s.world.now_ms / 1000.0);
        }
    }
    for effect in effects {
        match effect {
            StageEffect::TeleportPlayer { pos, yaw } => {
                let feet = Vec3::from(to_bevy(pos.to_array()));
                fly.player = arx_physics::Player::new(feet);
                fly.pos = fly.player.eye();
                if let Some(yaw) = yaw {
                    fly.yaw = std::f32::consts::PI - yaw.to_radians();
                }
                let player = s.player;
                s.world.entity_mut(player).pos = pos.to_array();
            }
            StageEffect::LookAt(target) => {
                let dir = Vec3::from(to_bevy(target.to_array())) - fly.pos;
                if dir.length() > 1.0 {
                    fly.yaw = f32::atan2(-dir.x, -dir.z);
                    fly.pitch = (dir.y / dir.length()).asin().clamp(-1.3, 1.0);
                }
            }
            StageEffect::ChangeLevel { level, target, .. } => {
                s.host.push_message(format!("(the way to level {level} at {target} is not open yet)"));
            }
        }
    }
    if log && *last != Some(s.host.stage.clone()) {
        let st = &s.host.stage;
        eprintln!(
            "[{:.1}s] stage: controls {} bars {} hud hidden {} camera {:?} fade {:?}",
            s.world.now_ms / 1000.0,
            st.controls,
            st.cinemascope,
            st.interface_hidden,
            st.camera.map(|c| s.world.entity(c).id_string.clone()),
            st.fade.map(|f| (f.out, f.duration_ms))
        );
        *last = Some(s.host.stage.clone());
    }
    let _ = before;
}

/// Black bars sliding in and out, and the fade of the whole picture.
pub fn overlay(
    time: Res<Time>,
    window: Single<&Window>,
    s: Res<Scripting>,
    mut bars: Query<(&Bar, &mut Node)>,
    mut fade: Single<&mut BackgroundColor, With<FadeOverlay>>,
    mut shown: Local<f32>,
) {
    // The bars take a second to come in or leave (the engine moves them 100 lines at 10 ms a line).
    let step = time.delta_secs().min(0.1);
    let want = if s.host.stage.cinemascope { 1.0 } else { 0.0 };
    *shown += (want - *shown).clamp(-step, step);
    let height = BAR_HEIGHT * window.height() / 480.0 * *shown;
    for (bar, mut node) in &mut bars {
        let _ = bar.top;
        node.height = Val::Px(height);
    }
    fade.0 = match &s.host.stage.fade {
        Some(f) => {
            let a = fade_level(f, s.world.now_ms);
            Color::srgba(f.color[0], f.color[1], f.color[2], a)
        }
        None => Color::NONE,
    };
}

/// See the scene through the scripts' camera while one is active; otherwise leave the hero's eyes alone.
pub fn camera(time: Res<Time>, mut stage: ResMut<Stage>, s: Res<Scripting>, mut cam: Single<(&mut Transform, &mut Projection), With<Camera3d>>) {
    let dt_ms = time.delta_secs().min(0.1) * 1000.0;
    let view = stage.0.camera_view(&s.world, &s.host, dt_ms);
    let (tf, projection) = &mut *cam;
    let fov = match view {
        Some(v) => {
            let (pos, target) = (Vec3::from(to_bevy(v.pos.to_array())), Vec3::from(to_bevy(v.target.to_array())));
            if pos.distance(target) > 0.01 {
                **tf = Transform::from_translation(pos).looking_at(target, Vec3::Y);
            }
            v.fov
        }
        None => PLAYER_FOV.to_radians(),
    };
    if let Projection::Perspective(p) = &mut **projection
        && (p.fov - fov).abs() > 1e-4
    {
        p.fov = fov;
    }
}

/// During a cutscene a key press is offered to the scripts (`key_pressed`), which is how the intro is skipped.
pub fn skip(keys: Res<ButtonInput<KeyCode>>, shot: Option<Res<crate::Shot>>, mut s: ResMut<Scripting>) {
    if shot.is_some() || !s.host.stage.cinemascope || !keys.any_just_pressed([KeyCode::Escape, KeyCode::Space, KeyCode::Enter]) {
        return;
    }
    let s = &mut *s;
    for id in 0..s.world.entities.len() as u32 {
        s.world.send_event(&mut s.host, None, id, "key_pressed", Vec::new());
    }
}
