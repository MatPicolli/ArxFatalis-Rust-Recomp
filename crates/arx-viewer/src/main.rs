//! Asset explorer for Arx Fatalis: browse the game's models (`.ftl`) and textures in Bevy.
//!
//! Level mode (`level N`): WASD = move, Q/E = down/up, Shift = fast, right mouse drag = look,
//! scroll = change speed.
//!
//! Model/texture controls: Left/Right = previous/next, PageUp/PageDown = jump 25, Home = first,
//! left mouse drag = orbit, scroll = zoom.

mod anims;
mod animated;
mod audio;
mod convert;
mod cutscene;
mod entities;
mod drag;
mod hud;
mod npcs;
mod particles;
mod player_body;
mod hud_book;
mod hud_ui;
mod level;
mod lighting;
mod menu;
mod scripting;
mod shadows;
mod speech;
mod steps;

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
    /// Level mode: start with the camera in front of this entity (e.g. `light_door_0074`), looking at
    /// it; `id:back` views it from behind, `id:side` from the side
    #[arg(long)]
    focus: Option<String>,
    /// Level mode: no sound
    #[arg(long)]
    mute: bool,
    /// Level mode: language of text and voices (`english`, `deutsch`, `francais`, ...)
    #[arg(long, default_value = "english")]
    language: String,
    /// Level mode: after start-up, have this entity (`id:key`, e.g. `goblin_base_0051:goblin_forbidden`) speak
    /// a localised line, for testing text and voices
    #[arg(long)]
    say: Option<String>,
    /// Level mode: put these entities (comma-separated ids such as `key_base_0005`) in the inventory shortly
    /// after start-up, for headless testing
    #[arg(long, value_delimiter = ',')]
    pickup: Vec<String>,
    /// Level mode: open this chest (an entity id such as `chest_metal_0051`) shortly after start-up
    #[arg(long)]
    open_chest: Option<String>,
    /// Level mode: put these entities straight into the inventory (even ones that are inside chests or NPCs),
    /// then drop them in front of the player after a moment, for headless testing
    #[arg(long, value_delimiter = ',')]
    give_and_drop: Vec<String>,
    /// Level mode: open the player's book at start-up (`stats`, the default, or `quests`)
    #[arg(long, num_args = 0..=1, default_missing_value = "stats")]
    show_book: Option<String>,
    /// Level mode: start with this much experience (levels up the hero), for headless testing
    #[arg(long, default_value_t = 0)]
    xp: i64,
    /// Level mode: walk forward by itself from the start (headless testing of movement and footsteps)
    #[arg(long)]
    walk_forward: bool,
    /// Level mode: give the player this entity, drag it out over the world along the view direction (`id` or
    /// `id:pitch-degrees`, default looking slightly down) and let go of it, for headless testing of dragging/throwing
    #[arg(long)]
    throw_test: Option<String>,
    /// Level mode: start with the inventory panel open
    #[arg(long)]
    show_inventory: bool,
    /// Level mode: give the player these items and use them (equip them), for headless testing of equipment
    #[arg(long, value_delimiter = ',')]
    equip: Vec<String>,
    /// Level mode: let cutscenes run without taking the view or the controls (their scripts still run), for testing
    #[arg(long)]
    no_cutscenes: bool,
    /// Level mode: start with the weapon drawn, for headless testing of combat
    #[arg(long)]
    draw_weapon: bool,
    /// Level mode: start with these attributes (`strength,mind,dexterity,constitution`), for headless testing of
    /// equipment that asks for them
    #[arg(long, value_delimiter = ',')]
    attrs: Vec<i32>,
    /// Level mode: swing the weapon by itself (wind up, let go, repeat), for headless testing of combat
    #[arg(long)]
    attack_test: bool,
    /// Level mode: start with this much gold in the purse, for headless testing
    #[arg(long, default_value_t = 0)]
    gold: u64,
    /// Level mode: size of the interface as a fraction of the largest that fits (the original's default is 0.5,
    /// which is its 640x480 pixel size on most screens; 1.0, the default here, is the biggest that fits)
    #[arg(long)]
    hud_scale: Option<f32>,
    /// Level mode: start with this much life (the maximum is 12 for a new hero), to see healing
    #[arg(long)]
    life: Option<f32>,
    /// Level mode: do not show dialogue subtitles
    #[arg(long)]
    no_subtitles: bool,
    /// Level mode: do not place entities (items, NPCs, fixtures)
    #[arg(long)]
    no_entities: bool,
    /// Level mode: hide NPCs (they are shown in bind pose until animations are implemented)
    #[arg(long)]
    no_npcs: bool,
    /// Level mode: go straight into the level, without the main menu and character creation
    #[arg(long)]
    no_menu: bool,
    /// Level mode: start on this menu screen (`main`, `options`, `create`, `quit`), also in a screenshot run
    #[arg(long)]
    menu: Option<String>,
    /// Level mode: start on character creation (what "New quest" does when a game is already running)
    #[arg(long)]
    new_quest: bool,
    /// Level mode: click these menu entries by themselves, one after another (`new`, `quickgen`, `skin`, `done`,
    /// `options`, `back`, `quit`, `yes`, `no`, `resume`), for headless testing
    #[arg(long, value_delimiter = ',')]
    menu_do: Vec<String>,
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
                    animated::Animated { skeleton, anim, meshes: m.meshes, elapsed_us: 0, looping: true, root_motion: false, overlay: None, keep_pose: false, pose: None },
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
    focus: Option<String>,
    say: Option<String>,
    pickup: Vec<String>,
    life: Option<f32>,
    open_container: Option<String>,
    gold: u64,
    show_inventory: bool,
    give_and_drop: Vec<String>,
    show_book: Option<String>,
    xp: i64,
    walk_forward: bool,
    throw_test: Option<String>,
    no_cutscenes: bool,
    equip: Vec<String>,
    draw_weapon: bool,
    attack_test: bool,
    attrs: Vec<i32>,
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
    /// Crouch toggled with `C` (holding `X` crouches too).
    crouch_toggle: bool,
    /// Radians turned per pixel of mouse movement, and whether up is down (the options menu).
    mouse_speed: f32,
    invert_mouse: bool,
}

fn run_level(args: Args, pak: PakSet) {
    let level = args.filter.as_deref().and_then(|s| s.parse().ok()).unwrap_or(1);
    let locale = pak.load_locale(&args.language).unwrap_or_else(|| {
        eprintln!("no localisation for language {:?}; text will show its keys", args.language);
        Default::default()
    });
    // The menu comes first, as in the game, unless this is a scripted run. Screenshot runs neither read nor write the
    // saved options, so that they always look the same.
    let persistent = args.shot.is_none();
    let start_screen = if args.new_quest {
        Some(menu::Screen::Create)
    } else if let Some(name) = &args.menu {
        Some(menu::Screen::from_name(name).unwrap_or(menu::Screen::Main))
    } else if args.shot.is_some() || args.no_menu {
        None
    } else {
        Some(menu::Screen::Main)
    };
    let mut options = if persistent { menu::load_options() } else { menu::Options::default() };
    // What the command line asks for wins over what was saved.
    if let Some(scale) = args.hud_scale {
        options.set(menu::Opt::HudScale, ((scale - 0.5) / 0.05).round().clamp(0.0, 10.0) as u8);
    }
    if args.no_subtitles {
        options.set(menu::Opt::Subtitles, 0);
    }
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window { title: format!("Arx Fatalis - level {level}"), ..default() }),
                ..default()
            })
            // Arx units are about centimetres; spatial audio works in metres.
            .set(bevy::audio::AudioPlugin { default_spatial_scale: bevy::audio::SpatialScale::new(0.01), ..default() }),
    )
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
        focus: args.focus.clone(),
        say: args.say.clone(),
        pickup: args.pickup.clone(),
        life: args.life,
        open_container: args.open_chest.clone(),
        gold: args.gold,
        show_inventory: args.show_inventory,
        give_and_drop: args.give_and_drop.clone(),
        show_book: args.show_book.clone(),
        xp: args.xp,
        walk_forward: args.walk_forward,
        throw_test: args.throw_test.clone(),
        no_cutscenes: args.no_cutscenes,
        equip: args.equip.clone(),
        draw_weapon: args.draw_weapon,
        attack_test: args.attack_test,
        attrs: args.attrs.clone(),
    })
    .insert_resource(entities::EntityCache::default())
    .insert_resource(entities::SpawnedEntities::default())
    .insert_resource(entities::LevelLights::default())
    .insert_resource(Fly {
        pos: Vec3::ZERO,
        yaw: 0.0,
        pitch: 0.0,
        speed: 600.0,
        walk: !args.fly,
        player: arx_physics::Player::new(Vec3::ZERO),
        world: None,
        crouch_toggle: false,
        mouse_speed: 0.003,
        invert_mouse: false,
    })
    .add_systems(Startup, setup_level)
    .insert_resource(scripting::Scripting::default())
    .insert_resource(scripting::Pickables::default())
    .insert_resource(scripting::Obstacles::default())
    .insert_resource(npcs::Npcs::default())
    .insert_resource(npcs::LevelZones::default())
    .insert_resource(cutscene::Stage(Default::default(), args.no_cutscenes, cutscene::PLAYER_FOV))
    .insert_resource(menu::Menu::new(start_screen, options, persistent, args.menu_do.iter().filter_map(|n| menu::Action::from_name(n)).collect()))
    .insert_resource(lighting::LevelLighting::default())
    .insert_resource(particles::Particles::default())
    .insert_resource(shadows::Shadows::default())
    .insert_resource(player_body::PlayerBody::default())
    .insert_resource(player_body::Combat::default())
    .insert_resource(audio::Sounds::new(args.mute))
    .insert_resource(speech::Speech::new(locale, args.language.clone(), !args.no_subtitles, args.mute))
    .add_systems(Startup, (hud_ui::load_font, speech::spawn_ui).chain())
    .add_systems(Startup, (steps::load, cutscene::spawn_ui, particles::setup, shadows::setup))
    .insert_resource(steps::StepSounds::default())
    .insert_resource(drag::ItemBodies::default())
    .insert_resource(hud_ui::UiFont::default())
    .insert_resource(hud::Ui { hud_scale: args.hud_scale.unwrap_or(1.0), ..default() })
    .insert_resource(hud_ui::UiAssets::default())
    .add_systems(
        Update,
        (menu::update, (hud_ui::mouse, hud_ui::draw).chain().run_if(menu::creating), ((
            scripting::tick,
            npcs::zones,
            npcs::update,
            cutscene::update,
            cutscene::skip,
            speech::debug_say,
            speech::update,
            entities::spawn_dropped,
            scripting::apply_state,
            npcs::apply,
            npcs::log,
            scripting::auto_use,
            scripting::sync_obstacles,
            audio::play_sounds,
        )
            .chain(),
        (
            fly_camera,
            steps::footsteps,
            steps::ui_sounds,
            steps::combat_sounds,
            steps::npc_footsteps,
            hud::debug_pickup,
            player_body::debug_equip,
            hud::input,
            scripting::interact,
            hud_ui::mouse,
            drag::debug_throw,
            drag::step_bodies,
        )
            .chain(),
        (
            lighting::update,
            particles::update,
            player_body::drive,
            hud_ui::draw,
            animated::animate,
            shadows::update,
            player_body::attach,
            cutscene::camera,
            cutscene::overlay,
            level_hud,
        )
            .chain())
            .chain()
            .run_if(menu::closed))
            .chain(),
    );
    if let Some(path) = args.shot {
        app.insert_resource(Shot { path, frames: 0 }).add_systems(Update, take_shot);
    }
    app.run();
}

/// What a level's simulation keeps besides its scripts.
#[derive(bevy::ecs::system::SystemParam)]
struct LevelState<'w> {
    npcs: ResMut<'w, npcs::Npcs>,
    body: ResMut<'w, player_body::PlayerBody>,
    zones: ResMut<'w, npcs::LevelZones>,
    stage: ResMut<'w, cutscene::Stage>,
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
    mut spawned: ResMut<entities::SpawnedEntities>,
    mut obstacles: ResMut<scripting::Obstacles>,
    level: LevelState,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let LevelState { mut npcs, mut body, mut zones, mut stage } = level;
    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            fov: 75f32.to_radians(),
            near: 4.0,
            far: 60000.0,
            ..default()
        }),
        Transform::default(),
        SpatialListener::new(8.0),
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
        Ok(mut info) => {
            // Torches are not in the baked light: they are added every frame (and flicker).
            let level_lights = arx.0.load_llf(args.level).map(|l| l.lights).unwrap_or_default();
            let lighting = lighting::LevelLighting::new(std::mem::take(&mut info.chunks), &level_lights, info.scene_pos);
            eprintln!("torches: {} of {} lights", lighting.torches.len(), level_lights.len());
            if std::env::var_os("ARX_LOG_LIGHTS").is_some() {
                for t in &lighting.torches {
                    eprintln!("  torch at {:.0},{:.0},{:.0} (Arx) extras {:#x} lit {} reach {:.0}..{:.0} x{:.1} fire r{:.0} f{:.2} size {:.1} speed {:.1}", t.pos.x, -t.pos.y, -t.pos.z, t.extras, t.lit, t.fall_start, t.fall_end, t.intensity, t.ex_radius, t.ex_frequency, t.ex_size, t.ex_speed);
                }
            }
            commands.insert_resource(lighting);
            eprintln!(
                "level {}: {} polygons in {} meshes, built in {:.1?}",
                args.level, info.poly_count, info.mesh_count, t.elapsed()
            );
            let dlf = arx.0.load_dlf(args.level);
            // Where the player's feet start. The FTS position is a foot position; the DLF one is the
            // editor camera (eye height). Both are editor positions, and can lie on a ledge far from
            // the real floor: if the nearest entity (which stands on a real floor) is much higher or
            // lower, start there instead.
            let mut feet0 = match &dlf {
                Ok(d) if args.start == StartPos::Dlf => {
                    Vec3::from(arx_level::to_yup([
                        d.player_pos[0] + info.scene_pos.x,
                        d.player_pos[1] + info.scene_pos.y,
                        d.player_pos[2] + info.scene_pos.z,
                    ])) - Vec3::Y * arx_physics::EYE_HEIGHT
                }
                _ => info.player_pos,
            };
            if let Ok(d) = &dlf {
                let nearest = d
                    .entities
                    .iter()
                    .filter(|e| !e.class.contains("/items/") && !e.class.contains("/system/camera"))
                    .map(|e| Vec3::from(arx_level::to_yup([e.pos[0] + info.scene_pos.x, e.pos[1] + info.scene_pos.y, e.pos[2] + info.scene_pos.z])))
                    .min_by(|a, b| (a.x - feet0.x).hypot(a.z - feet0.z).total_cmp(&(b.x - feet0.x).hypot(b.z - feet0.z)));
                if let Some(anchor) = nearest.filter(|a| (a.y - feet0.y).abs() > 250.0) {
                    feet0 = anchor;
                }
            }
            let start = feet0 + Vec3::Y * arx_physics::EYE_HEIGHT;
            fly.pos = args.cam.unwrap_or(start);
            // Scripts decide which entities exist and what they look like; solid ones (doors,
            // portcullises, ...) join the collision world before it is shared.
            let mut collision = info.collision;
            if let (Ok(d), true) = (&dlf, args.entities) {
                let t = std::time::Instant::now();
                *scripting = scripting::Scripting::build(&arx.0, d, info.scene_pos);
                let ws = &scripting.world.stats;
                eprintln!(
                    "scripts: {} events, {} commands, {} warnings, {} unimplemented command kinds, run in {:.1?}",
                    ws.events_run, ws.commands_run, ws.warnings.len(), ws.unknown_commands.len(), t.elapsed()
                );
                obstacles.entities = arx_level::EntityObstacles::build(
                    &mut collision, &arx.0, d, info.scene_pos, &scripting.world, &scripting.host, &scripting.ids,
                );
                eprintln!("entity obstacles: {}", obstacles.entities.by_entity.len());
                npcs.0 = Some(arx_level::npc::build_npcs(
                    &arx.0, &info.anchors, info.scene_pos, d, &scripting.ids, &scripting.world, &obstacles.entities, &collision,
                ));
                if let Some(n) = npcs.0.as_mut() {
                    n.log = std::env::var("ARX_LOG_NPC").ok();
                }
                zones.0 = arx_level::zones::Zones::from_dlf(d, info.scene_pos);
                stage.0 = arx_level::stage::StageWorld::from_dlf(d, info.scene_pos, &scripting.ids);
                stage.1 = args.no_cutscenes;
                eprintln!("zones: {}", zones.0.zones.len());
            }
            let collision = std::sync::Arc::new(collision);
            obstacles.world = Some(collision.clone());
            fly.world = Some(collision.clone());
            // Walking starts with the feet on the ground: an explicit camera is an eye position; the
            // default start is already a foot position (moved somewhere safe if it floats over nothing).
            let feet = match args.cam {
                Some(eye) => eye - Vec3::Y * arx_physics::EYE_HEIGHT,
                None => collision.spawn_point(feet0),
            };
            fly.player = arx_physics::Player::new(feet + Vec3::Y * 20.0);
            if args.cam.is_none() {
                fly.pos = fly.player.eye();
            }
            if let (Some(spec), Ok(d)) = (&args.focus, &dlf) {
                let (name, side) = spec.split_once(':').unwrap_or((spec, "front"));
                // Entity ids are `<class name>_<nnnn>`.
                let found = d.entities.iter().find(|e| {
                    format!("{}_{:04}", e.class.rsplit('/').next().unwrap_or(""), e.instance) == name
                });
                match found {
                    Some(e) => {
                        let origin = Vec3::from(arx_level::to_yup([e.pos[0] + info.scene_pos.x, e.pos[1] + info.scene_pos.y, e.pos[2] + info.scene_pos.z]));
                        let rot = arx_level::entity_rotation(e.angle, e.class.contains("/npc/"));
                        let dir = match side { "back" => Vec3::Z, "side" => Vec3::X, _ => Vec3::NEG_Z };
                        // Stand 300 units away along the entity's own axis, at about half the eye height.
                        let at = origin + rot * dir * 300.0 + Vec3::Y * 110.0;
                        fly.pos = at;
                        fly.walk = false;
                        let look = (origin + Vec3::Y * 100.0) - at;
                        fly.yaw = f32::atan2(-look.x, -look.z);
                        fly.pitch = (look.y / look.xz().length().max(1.0)).atan();
                        eprintln!("focus {name}: entity at {origin:?}, angle {:?}, camera {at:?}", e.angle);
                    }
                    None => eprintln!("--focus: no entity named {name}"),
                }
            }
            match dlf {
                Ok(d) if args.entities => {
                    let lights = entities::StaticLight::from_level(&level_lights, info.scene_pos);
                    let t = std::time::Instant::now();
                    let stats = entities::spawn_entities(
                        &mut commands, &arx.0, &d, info.scene_pos, &lights, args.npcs,
                        &mut scripting, &mut pickables.0, &mut spawned,
                        &mut ecache, &mut cache, &mut meshes, &mut materials, &mut images,
                    );
                    eprintln!("entities: {stats:?} with {} static lights, built in {:.1?}", lights.len(), t.elapsed());
                    player_body::spawn(
                        &mut commands, &arx.0, &lights, &mut scripting, &mut pickables.0, &mut ecache, &mut cache,
                        &mut meshes, &mut materials, &mut images, fly.player.feet, &mut body,
                    );
                    commands.insert_resource(entities::LevelLights(lights));
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
    mut script: ResMut<scripting::Scripting>,
    mut ui: ResMut<hud::Ui>,
    args: Res<LevelArgs>,
    mut was_open: Local<bool>,
    mut regrab: Local<bool>,
) {
    // Screenshot runs are scripted: ignore the real keyboard and mouse so they are reproducible.
    // A cutscene takes the controls away as well.
    let live = shot.is_none() && script.host.stage.controls;
    let keys: &ButtonInput<KeyCode> = if live { &keys } else { &ButtonInput::default() };
    let mouse: &ButtonInput<MouseButton> = if live { &mouse } else { &ButtonInput::default() };
    let motion = if live { motion.delta } else { Vec2::ZERO };
    // Click to capture the mouse for look; Escape releases it. Right-drag also looks around. While the backpack or
    // a chest is open (or the mouse is released) the cursor works the interface, which draws the original cursor.
    let wants_cursor = ui.open || ui.book.is_some() || ui.reading.is_some() || script.host.open_container.is_some();
    if wants_cursor && !*was_open {
        *regrab = cursor.grab_mode != CursorGrabMode::None;
        cursor.grab_mode = CursorGrabMode::None;
    }
    if !wants_cursor && *was_open && *regrab {
        cursor.grab_mode = CursorGrabMode::Locked;
    }
    *was_open = wants_cursor;
    if mouse.just_pressed(MouseButton::Left) && !wants_cursor && !ui.over_hud && ui.hover_item.is_none() {
        cursor.grab_mode = CursorGrabMode::Locked;
    }
    cursor.visible = false;
    let captured = cursor.grab_mode != CursorGrabMode::None;
    ui.cursor_mode = !captured;
    if captured || mouse.pressed(MouseButton::Right) && !wants_cursor {
        fly.yaw -= motion.x * fly.mouse_speed;
        // Walking, the original lets you look 74.9 degrees down and 59 up; flying has no body to look into.
        let (down, up) = if fly.walk { (-74.9f32.to_radians(), 59f32.to_radians()) } else { (-1.55, 1.55) };
        let dy = if fly.invert_mouse { -motion.y } else { motion.y };
        fly.pitch = (fly.pitch - dy * fly.mouse_speed).clamp(down, up);
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
        // The movement keys combine like the original's: see `MoveInput::from_keys`.
        if keys.just_pressed(KeyCode::KeyC) {
            fly.crouch_toggle = !fly.crouch_toggle;
        }
        let dead = script.host.player.is_dead();
        if dead && keys.just_pressed(KeyCode::KeyR) {
            // Revive where the player last stood.
            script.host.player.life.current = script.host.player.life.max;
            let at = fly.player.last_ground;
            fly.player = arx_physics::Player::new(at);
        }
        let mut input = if dead {
            arx_physics::MoveInput::default()
        } else {
            arx_physics::MoveInput::from_keys(fly.yaw, keys.pressed(KeyCode::KeyW) || args.walk_forward, keys.pressed(KeyCode::KeyS), keys.pressed(KeyCode::KeyA), keys.pressed(KeyCode::KeyD))
        };
        input.stealth = keys.pressed(KeyCode::ShiftLeft);
        input.crouch = !dead && (keys.pressed(KeyCode::KeyX) || fly.crouch_toggle);
        input.jump = !dead && keys.pressed(KeyCode::Space);
        let world = fly.world.clone().unwrap();
        fly.player.step(&world, dt, input);
        // A long fall hurts: (height - 400) / 15 life.
        if let Some(height) = fly.player.take_landing() {
            let damage = arx_physics::fall_damage(height);
            script.host.player.life.add(-damage);
            // The player's own script cries out (`on ouch`) or dies (`on die`).
            let player = script.player;
            let sc = &mut *script;
            sc.world.send_event(&mut sc.host, None, player, "ouch", vec![format!("{damage:.0}")]);
            if sc.host.player.is_dead() {
                sc.world.send_event(&mut sc.host, None, player, "die", Vec::new());
                sc.host.push_message("You are dead - press R".to_owned());
            }
        }
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
    speech: Res<speech::Speech>,
    ui: Res<hud::Ui>,
    mut hud: Single<&mut Text, With<Hud>>,
) {
    if !ui.debug {
        hud.0.clear();
        return;
    }
    // Report the position in Arx coordinates so it can be fed back through --cam.
    let mode = if fly.walk { "walking" } else { "flying" };
    let help = if fly.walk {
        "WASD move  Shift sneak  X crouch (C toggle)  Space jump  E use/take  I inventory  click: capture mouse  Esc: menu  F: fly"
    } else {
        "WASD move  Q/E down/up  Shift fast  scroll speed  click: capture mouse  Esc: menu  F: walk"
    };
    let target = script.target.map_or(String::new(), |t| {
        let e = script.world.entity(t);
        // Names are localisation keys (`[description_door]`).
        let name = script.host.state(t).map(|s| s.name.as_str()).filter(|n| !n.is_empty()).map(|n| speech.text(n));
        format!("\n[E] {}", name.unwrap_or_else(|| e.id_string.clone()))
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
