//! Asset explorer for Arx Fatalis: browse the game's models (`.ftl`) and textures in Bevy.
//!
//! Level mode (`level N`): WASD = move, Q/E = down/up, Shift = fast, right mouse drag = look,
//! scroll = change speed.
//!
//! Model/texture controls: Left/Right = previous/next, PageUp/PageDown = jump 25, Home = first,
//! left mouse drag = orbit, scroll = zoom.

mod anims;
mod animated;
mod convert;
mod entities;
mod level;
mod scripting;

use arx_formats::PakSet;
use bevy::{
    window::{CursorGrabMode, CursorOptions},
    app::AppExit,
    input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll},
    prelude::*,
    render::view::screenshot::{Screenshot, save_to_disk},
};
use clap::{Parser, ValueEnum};
use convert::TextureCache;
use std::path::PathBuf;

const DEFAULT_GAME_DIR: &str = r"D:\Steam\steamapps\common\Arx Fatalis";

#[derive(Clone, Copy, ValueEnum, PartialEq, Eq)]
enum Mode {
    Models,
    Textures,
    /// Fly through a level; the filter argument is the level number (default 1)
    Level,
}

#[derive(Clone, Copy, ValueEnum, PartialEq, Eq)]
enum StartPos {
    Fts,
    Dlf,
}

#[derive(Parser)]
#[command(about = "Arx Fatalis asset explorer")]
struct Args {
    #[arg(value_enum, default_value = "models")]
    mode: Mode,
    /// Only browse files whose path contains this text
    filter: Option<String>,
    #[arg(long, env = "ARX_DIR", default_value = DEFAULT_GAME_DIR)]
    game_dir: PathBuf,
    /// Start at this index into the (filtered, sorted) list
    #[arg(long, default_value_t = 0)]
    index: usize,
    /// Models mode: start with this animation (substring of its path, e.g. `goblin_normal_wait`)
    #[arg(long)]
    anim: Option<String>,
    /// Level mode: camera position in Arx coordinates, `x,y,z` (default: player start)
    #[arg(long, value_delimiter = ',', allow_negative_numbers = true)]
    cam: Option<Vec<f32>>,
    /// Level mode: camera `yaw,pitch` in degrees (yaw 0 looks along Arx +Z)
    #[arg(long, value_delimiter = ',', allow_negative_numbers = true)]
    look: Option<Vec<f32>>,
    /// Level mode: where the camera starts: the scene's saved player position (`fts`) or the
    /// level file's editor camera (`dlf`). Scripts decide the real start in the original game.
    #[arg(long, value_enum, default_value = "fts")]
    start: StartPos,
    /// Level mode: start in free-flight mode instead of walking (toggle with F)
    #[arg(long)]
    fly: bool,
    /// Level mode: send `action` to these entities (comma-separated ids such as `light_door_0007`)
    /// shortly after start-up, as if the player had used them; for headless testing
    #[arg(long, value_delimiter = ',')]
    use_entity: Vec<String>,
    /// Level mode: do not place entities (items, NPCs, fixtures)
    #[arg(long)]
    no_entities: bool,
    /// Level mode: hide NPCs (they are shown in bind pose until animations are implemented)
    #[arg(long)]
    no_npcs: bool,
    /// Save a screenshot to this file after a few frames, then exit
    #[arg(long)]
    shot: Option<PathBuf>,
}

#[derive(Resource)]
struct Arx(std::sync::Arc<PakSet>);

#[derive(Resource)]
struct Browse {
    mode: Mode,
    items: Vec<String>,
    index: usize,
    dirty: bool,
}

/// Animation choice in model mode: indices into `list`; `list.len()` means bind pose.
#[derive(Resource, Default)]
struct AnimSel {
    list: Vec<String>,
    index: usize,
    preferred: Option<String>,
}

#[derive(Resource)]
struct Orbit {
    target: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
}

#[derive(Resource)]
struct Shot {
    path: PathBuf,
    frames: u32,
}

#[derive(Component)]
struct Shown;

#[derive(Component)]
struct Hud;

fn main() {
    let args = Args::parse();
    let pak = PakSet::open_game_dir(&args.game_dir).unwrap_or_else(|e| {
        eprintln!("cannot open game data in {}: {e}", args.game_dir.display());
        std::process::exit(1);
    });
    if args.mode == Mode::Level {
        run_level(args, pak);
        return;
    }
    let ext: &[&str] = match args.mode {
        Mode::Models => &[".ftl"],
        Mode::Textures => &[".bmp", ".jpg", ".tga"],
        Mode::Level => unreachable!(),
    };
    let filter = args.filter.as_deref().map(str::to_ascii_lowercase);
    let items: Vec<String> = pak
        .list("")
        .into_iter()
        .filter(|p| ext.iter().any(|e| p.ends_with(e)))
        .filter(|p| filter.as_ref().is_none_or(|f| p.contains(f.as_str())))
        .map(str::to_owned)
        .collect();
    if items.is_empty() {
        eprintln!("no matching files");
        std::process::exit(1);
    }

    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window { title: "Arx Fatalis asset explorer".into(), ..default() }),
        ..default()
    }))
    .insert_resource(ClearColor(Color::srgb(0.08, 0.08, 0.1)))
    .insert_resource(Arx(std::sync::Arc::new(pak)))
    .insert_resource(Browse { mode: args.mode, index: args.index.min(items.len() - 1), items, dirty: true })
    .insert_resource(Orbit { target: Vec3::ZERO, yaw: if args.mode == Mode::Textures { 0.0 } else { 0.6 }, pitch: if args.mode == Mode::Textures { 0.0 } else { 0.35 }, distance: 300.0 })
    .insert_resource(TextureCache::default())
    .insert_resource(AnimSel { preferred: args.anim.clone().map(|a| a.to_ascii_lowercase()), ..default() })
    .add_systems(Startup, setup)
    .add_systems(
        Update,
        (navigate, load_current, select_anim, animated::animate, orbit_camera, update_hud).chain(),
    );
    if let Some(path) = args.shot {
        app.insert_resource(Shot { path, frames: 0 }).add_systems(Update, take_shot);
    }
    app.run();
}

fn setup(mut commands: Commands) {
    commands.spawn((Camera3d::default(), Transform::default()));
    commands.spawn((
        DirectionalLight { illuminance: 9000.0, shadow_maps_enabled: false, ..default() },
        Transform::from_xyz(0.4, 1.0, 0.7).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.insert_resource(GlobalAmbientLight { color: Color::WHITE, brightness: 450.0, ..default() });
    commands.spawn((
        Hud,
        Text::new(""),
        TextFont { font_size: FontSize::Px(16.0), ..default() },
        TextColor(Color::WHITE),
        Node { position_type: PositionType::Absolute, left: Val::Px(10.0), top: Val::Px(8.0), ..default() },
    ));
}

fn navigate(keys: Res<ButtonInput<KeyCode>>, mut browse: ResMut<Browse>) {
    let n = browse.items.len();
    let mut idx = browse.index;
    if keys.just_pressed(KeyCode::ArrowRight) {
        idx = (idx + 1) % n;
    }
    if keys.just_pressed(KeyCode::ArrowLeft) {
        idx = (idx + n - 1) % n;
    }
    if keys.just_pressed(KeyCode::PageDown) {
        idx = (idx + 25) % n;
    }
    if keys.just_pressed(KeyCode::PageUp) {
        idx = (idx + n - 25 % n) % n;
    }
    if keys.just_pressed(KeyCode::Home) {
        idx = 0;
    }
    if idx != browse.index {
        browse.index = idx;
        browse.dirty = true;
    }
}

fn load_current(
    mut commands: Commands,
    mut browse: ResMut<Browse>,
    arx: Res<Arx>,
    mut cache: ResMut<TextureCache>,
    mut orbit: ResMut<Orbit>,
    mut sel: ResMut<AnimSel>,
    shown: Query<Entity, With<Shown>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    if !browse.dirty {
        return;
    }
    browse.dirty = false;
    for e in &shown {
        commands.entity(e).despawn();
    }
    let path = browse.items[browse.index].clone();
    let bounds = match browse.mode {
        Mode::Models => {
            let spawned = convert::spawn_model(
                &mut commands, &arx.0, &path, &mut cache, &mut meshes, &mut materials, &mut images,
            );
            sel.list.clear();
            let mut anim = None;
            let mut bounds = None;
            if let Some(m) = spawned {
                let skeleton = std::sync::Arc::new(arx_formats::skeleton::Skeleton::from_ftl(&m.ftl));
                sel.list = anims::compatible_anims(&arx.0, &path, skeleton.bones.len());
                sel.index = sel.list.len();
                if let Some(p) = sel.preferred.take() {
                    // Prefer an exact file-name match, else the first path containing the text.
                    let exact = sel.list.iter().position(|a| a.rsplit('/').next().is_some_and(|f| f == format!("{p}.tea")));
                    if let Some(i) = exact.or_else(|| sel.list.iter().position(|a| a.contains(&p))) {
                        sel.index = i;
                    }
                }
                if let Some(a) = sel.list.get(sel.index) {
                    anim = anims::load_anim(&arx.0, a);
                }
                bounds = Some(m.bounds);
                commands.spawn((
                    Shown,
                    animated::Animated { skeleton, anim, meshes: m.meshes, elapsed_us: 0, looping: true },
                ));
            }
            bounds
        }
        Mode::Textures => convert::spawn_texture(
            &mut commands, &arx.0, &path, &mut cache, &mut meshes, &mut materials, &mut images,
        ),
        Mode::Level => unreachable!(),
    };
    if let Some((min, max)) = bounds {
        orbit.target = (min + max) / 2.0;
        orbit.distance = ((max - min).length() * 1.2).max(1.0);
    }
}

/// `.` / `,` cycle through the animations that fit the current model; the last slot is the bind pose.
fn select_anim(
    keys: Res<ButtonInput<KeyCode>>,
    arx: Res<Arx>,
    mut sel: ResMut<AnimSel>,
    mut query: Query<&mut animated::Animated>,
) {
    let slots = sel.list.len() + 1;
    let mut idx = sel.index;
    if keys.just_pressed(KeyCode::Period) {
        idx = (idx + 1) % slots;
    }
    if keys.just_pressed(KeyCode::Comma) {
        idx = (idx + slots - 1) % slots;
    }
    if idx == sel.index {
        return;
    }
    sel.index = idx;
    let anim = sel.list.get(idx).and_then(|p| anims::load_anim(&arx.0, p));
    for mut a in &mut query {
        a.anim = anim.clone();
        a.elapsed_us = 0;
    }
}

fn orbit_camera(
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    mut orbit: ResMut<Orbit>,
    mut cam: Single<&mut Transform, With<Camera3d>>,
) {
    if mouse.pressed(MouseButton::Left) {
        orbit.yaw -= motion.delta.x * 0.005;
        orbit.pitch = (orbit.pitch + motion.delta.y * 0.005).clamp(-1.5, 1.5);
    }
    if scroll.delta.y != 0.0 {
        orbit.distance = (orbit.distance * (1.0 - scroll.delta.y * 0.1)).max(0.1);
    }
    let rot = Quat::from_euler(EulerRot::YXZ, orbit.yaw, -orbit.pitch, 0.0);
    cam.translation = orbit.target + rot * Vec3::new(0.0, 0.0, orbit.distance);
    cam.look_at(orbit.target, Vec3::Y);
}

fn update_hud(browse: Res<Browse>, sel: Res<AnimSel>, mut hud: Single<&mut Text, With<Hud>>) {
    let anim_line = if browse.mode == Mode::Models {
        match sel.list.get(sel.index) {
            Some(a) => format!("\nanim [{}/{}] {a}   ,/. cycle", sel.index + 1, sel.list.len()),
            None if sel.list.is_empty() => "\nno compatible animations".to_owned(),
            None => format!("\nbind pose ({} animations fit; ,/. cycle)", sel.list.len()),
        }
    } else {
        String::new()
    };
    hud.0 = format!(
        "[{}/{}] {}{anim_line}\nLeft/Right: prev/next   PgUp/PgDn: +-25   Home: first   drag: orbit   scroll: zoom",
        browse.index + 1,
        browse.items.len(),
        browse.items[browse.index]
    );
}

/// Frame to capture on; scenes with many meshes need a while before everything is uploaded.
/// Override with `ARX_SHOT_FRAME`.
fn shot_frame() -> u32 {
    std::env::var("ARX_SHOT_FRAME").ok().and_then(|v| v.parse().ok()).unwrap_or(180)
}

fn take_shot(mut commands: Commands, mut shot: ResMut<Shot>, mut exit: MessageWriter<AppExit>) {
    shot.frames += 1;
    let at = shot_frame();
    if shot.frames == at {
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(shot.path.clone()));
    }
    if shot.frames == at + 60 {
        exit.write(AppExit::Success);
    }
}

// ---------------------------------------------------------------------------------------------
// Level mode
// ---------------------------------------------------------------------------------------------

#[derive(Resource)]
struct LevelArgs {
    level: u32,
    cam: Option<Vec3>,
    look: Option<(f32, f32)>,
    entities: bool,
    npcs: bool,
    start: StartPos,
    use_entity: Vec<String>,
}

#[derive(Resource)]
struct Fly {
    /// Camera (eye) position.
    pos: Vec3,
    yaw: f32,
    pitch: f32,
    speed: f32,
    /// Walking with collision and gravity instead of free flight.
    walk: bool,
    player: arx_physics::Player,
    world: Option<std::sync::Arc<arx_physics::CollisionWorld>>,
}

fn run_level(args: Args, pak: PakSet) {
    let level = args.filter.as_deref().and_then(|s| s.parse().ok()).unwrap_or(1);
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window { title: format!("Arx Fatalis - level {level}"), ..default() }),
        ..default()
    }))
    .insert_resource(ClearColor(Color::BLACK))
    .insert_resource(Arx(std::sync::Arc::new(pak)))
    .insert_resource(TextureCache::default())
    .insert_resource(LevelArgs {
        level,
        // Arx coordinates -> Bevy
        cam: args.cam.filter(|c| c.len() == 3).map(|c| Vec3::new(c[0], -c[1], -c[2])),
        look: args.look.filter(|l| l.len() == 2).map(|l| (l[0], l[1])),
        entities: !args.no_entities,
        npcs: !args.no_npcs,
        start: args.start,
        use_entity: args.use_entity.clone(),
    })
    .insert_resource(entities::EntityCache::default())
    .insert_resource(Fly {
        pos: Vec3::ZERO,
        yaw: 0.0,
        pitch: 0.0,
        speed: 600.0,
        walk: !args.fly,
        player: arx_physics::Player::new(Vec3::ZERO),
        world: None,
    })
    .add_systems(Startup, setup_level)
    .insert_resource(scripting::Scripting::default())
    .insert_resource(scripting::Pickables::default())
    .add_systems(
        Update,
        (
            scripting::tick,
            scripting::apply_state,
            scripting::auto_use,
            fly_camera,
            scripting::interact,
            animated::animate,
            level_hud,
        )
            .chain(),
    );
    if let Some(path) = args.shot {
        app.insert_resource(Shot { path, frames: 0 }).add_systems(Update, take_shot);
    }
    app.run();
}

fn setup_level(
    mut commands: Commands,
    arx: Res<Arx>,
    args: Res<LevelArgs>,
    mut fly: ResMut<Fly>,
    mut cache: ResMut<TextureCache>,
    mut ecache: ResMut<entities::EntityCache>,
    mut scripting: ResMut<scripting::Scripting>,
    mut pickables: ResMut<scripting::Pickables>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            fov: 75f32.to_radians(),
            near: 4.0,
            far: 60000.0,
            ..default()
        }),
        Transform::default(),
    ));
    commands.spawn((
        Hud,
        Text::new(""),
        TextFont { font_size: FontSize::Px(16.0), ..default() },
        TextColor(Color::WHITE),
        Node { position_type: PositionType::Absolute, left: Val::Px(10.0), top: Val::Px(8.0), ..default() },
    ));

    let t = std::time::Instant::now();
    match level::spawn_level(&mut commands, &arx.0, args.level, &mut cache, &mut meshes, &mut materials, &mut images) {
        Ok(info) => {
            eprintln!(
                "level {}: {} polygons in {} meshes, built in {:.1?}",
                args.level, info.poly_count, info.mesh_count, t.elapsed()
            );
            let dlf = arx.0.load_dlf(args.level);
            // The FTS position is at the feet (eyes ~160 higher); the DLF one is the editor camera.
            let start = match &dlf {
                Ok(d) if args.start == StartPos::Dlf => Vec3::from(convert::to_bevy([
                    d.player_pos[0] + info.scene_pos.x,
                    d.player_pos[1] + info.scene_pos.y,
                    d.player_pos[2] + info.scene_pos.z,
                ])),
                _ => info.player_pos + Vec3::Y * 160.0,
            };
            fly.pos = args.cam.unwrap_or(start);
            fly.world = Some(info.collision.clone());
            // Walking starts with the feet on the ground: an explicit camera is an eye position; the
            // default start is already a foot position (moved somewhere safe if it floats over nothing).
            let feet = match args.cam {
                Some(eye) => eye - Vec3::Y * arx_physics::EYE_HEIGHT,
                None => info.collision.spawn_point(start - Vec3::Y * arx_physics::EYE_HEIGHT * (args.start == StartPos::Dlf) as i32 as f32),
            };
            fly.player = arx_physics::Player::new(feet + Vec3::Y * 20.0);
            if args.cam.is_none() {
                fly.pos = fly.player.eye();
            }
            match dlf {
                Ok(d) if args.entities => {
                    let lights = arx.0.load_llf(args.level).map(|l| l.lights).unwrap_or_default();
                    let lights = entities::StaticLight::from_level(&lights, info.scene_pos);
                    let t = std::time::Instant::now();
                    *scripting = scripting::Scripting::build(&arx.0, &d, info.scene_pos);
                    let ws = &scripting.world.stats;
                    eprintln!(
                        "scripts: {} events, {} commands, {} warnings, {} unimplemented command kinds, run in {:.1?}",
                        ws.events_run, ws.commands_run, ws.warnings.len(), ws.unknown_commands.len(), t.elapsed()
                    );
                    let t = std::time::Instant::now();
                    let stats = entities::spawn_entities(
                        &mut commands, &arx.0, &d, info.scene_pos, &lights, args.npcs,
                        &mut scripting, &mut pickables.0,
                        &mut ecache, &mut cache, &mut meshes, &mut materials, &mut images,
                    );
                    eprintln!("entities: {stats:?} with {} static lights, built in {:.1?}", lights.len(), t.elapsed());
                }
                Ok(_) => {}
                Err(e) => eprintln!("no entities: {e}"),
            }
        }
        Err(e) => {
            eprintln!("cannot load level {}: {e}", args.level);
            std::process::exit(1);
        }
    }
    if let Some((yaw, pitch)) = args.look {
        fly.yaw = yaw.to_radians();
        fly.pitch = pitch.to_radians();
    }
}

const WALK_SPEED: f32 = 300.0;
const RUN_SPEED: f32 = 600.0;

fn fly_camera(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    mut fly: ResMut<Fly>,
    mut cursor: Single<&mut CursorOptions, With<Window>>,
    mut cam: Single<&mut Transform, With<Camera3d>>,
    shot: Option<Res<Shot>>,
) {
    // Screenshot runs are scripted: ignore the real keyboard and mouse so they are reproducible.
    let live = shot.is_none();
    let keys: &ButtonInput<KeyCode> = if live { &keys } else { &ButtonInput::default() };
    let mouse: &ButtonInput<MouseButton> = if live { &mouse } else { &ButtonInput::default() };
    let motion = if live { motion.delta } else { Vec2::ZERO };
    // Click to capture the mouse for look; Escape releases it. Right-drag also looks around.
    if mouse.just_pressed(MouseButton::Left) {
        cursor.grab_mode = CursorGrabMode::Locked;
        cursor.visible = false;
    }
    if keys.just_pressed(KeyCode::Escape) {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }
    let captured = cursor.grab_mode != CursorGrabMode::None;
    if captured || mouse.pressed(MouseButton::Right) {
        fly.yaw -= motion.x * 0.003;
        fly.pitch = (fly.pitch - motion.y * 0.003).clamp(-1.55, 1.55);
    }

    if keys.just_pressed(KeyCode::KeyF) {
        fly.walk = !fly.walk;
        if fly.walk {
            let feet = fly.pos - Vec3::Y * arx_physics::EYE_HEIGHT;
            fly.player = arx_physics::Player::new(feet);
        }
    }

    let dt = time.delta_secs().min(0.1);
    let rot = Quat::from_euler(EulerRot::YXZ, fly.yaw, fly.pitch, 0.0);
    let ahead = |key: KeyCode| keys.pressed(key) as i32 as f32;

    if fly.walk && fly.world.is_some() {
        // Movement is relative to the horizontal facing direction.
        let forward = Vec2::new(-fly.yaw.sin(), -fly.yaw.cos());
        let right = Vec2::new(fly.yaw.cos(), -fly.yaw.sin());
        let wish = forward * (ahead(KeyCode::KeyW) - ahead(KeyCode::KeyS)) + right * (ahead(KeyCode::KeyD) - ahead(KeyCode::KeyA));
        let speed = if keys.pressed(KeyCode::ShiftLeft) { RUN_SPEED } else { WALK_SPEED };
        let world = fly.world.clone().unwrap();
        fly.player.step(&world, dt, wish.normalize_or_zero() * speed, keys.pressed(KeyCode::Space));
        fly.pos = fly.player.eye();
    } else {
        if live && scroll.delta.y != 0.0 {
            fly.speed = (fly.speed * (1.0 + scroll.delta.y * 0.15)).clamp(20.0, 20000.0);
        }
        let mut dir = Vec3::ZERO;
        for (key, v) in [
            (KeyCode::KeyW, Vec3::NEG_Z),
            (KeyCode::KeyS, Vec3::Z),
            (KeyCode::KeyA, Vec3::NEG_X),
            (KeyCode::KeyD, Vec3::X),
        ] {
            if keys.pressed(key) {
                dir += rot * v;
            }
        }
        dir += Vec3::Y * (ahead(KeyCode::KeyE) - ahead(KeyCode::KeyQ));
        let boost = if keys.pressed(KeyCode::ShiftLeft) { 4.0 } else { 1.0 };
        let step = dir.normalize_or_zero() * fly.speed * boost * dt;
        fly.pos += step;
    }
    cam.translation = fly.pos;
    cam.rotation = rot;
}

fn level_hud(
    fly: Res<Fly>,
    args: Res<LevelArgs>,
    script: Res<scripting::Scripting>,
    mut hud: Single<&mut Text, With<Hud>>,
) {
    // Report the position in Arx coordinates so it can be fed back through --cam.
    let mode = if fly.walk { "walking" } else { "flying" };
    let help = if fly.walk {
        "WASD move  Shift run  Space jump  E use  click: capture mouse  Esc: release  F: fly"
    } else {
        "WASD move  Q/E down/up  Shift fast  scroll speed  click: capture mouse  Esc: release  F: walk"
    };
    let target = script.target.map_or(String::new(), |t| {
        let e = script.world.entity(t);
        let name = script.host.state(t).map(|s| s.name.as_str()).filter(|n| !n.is_empty());
        format!("
[E] {}{}", e.id_string, name.map_or(String::new(), |n| format!("  ({n})")))
    });
    hud.0 = format!(
        "level {} ({mode})   eye {:.0},{:.0},{:.0}   yaw {:.0} pitch {:.0}   rescues {}
{help}{target}",
        args.level,
        fly.pos.x,
        -fly.pos.y,
        -fly.pos.z,
        fly.yaw.to_degrees(),
        fly.pitch.to_degrees(),
        fly.player.rescues
    );
}
