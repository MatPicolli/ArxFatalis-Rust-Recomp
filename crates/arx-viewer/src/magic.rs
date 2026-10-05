//! Casting: hold `Ctrl`, draw a rune with the left button held, let the button go (the rune is named aloud), draw
//! the next, and let `Ctrl` go to cast what the runes make. The rules, the recognition and the spells themselves are
//! in `arx_level::magic`; this is the hand that draws, the trail on the screen, the sounds, and what the spells do
//! to the world one can see (torches lit and put out, missiles).

use crate::convert::to_bevy;
use crate::hud::Ui;
use crate::hud_ui::{UiAssets, UiFont};
use crate::lighting::LevelLighting;
use crate::npcs::Npcs;
use crate::scripting::Scripting;
use crate::{Arx, Fly, LevelArgs, Shot};
use arx_level::magic::{ActiveSpells, CastError, Effect, Rune, recognise};
use arx_level::npc::Env;
use bevy::{input::mouse::AccumulatedMouseMotion, prelude::*};

/// How fast a missile flies, how far it gets, and how close it has to pass to hit (units, seconds).
const MISSILE_SPEED: f32 = 1100.0;
const MISSILE_LIFE: f32 = 3.0;
const MISSILE_RADIUS: f32 = 18.0;

#[derive(Resource, Default)]
pub struct Magic {
    pub spells: ActiveSpells,
    /// The stroke being drawn (screen pixels), and where the pen is.
    stroke: Vec<Vec2>,
    pen: Vec2,
    /// The last stroke, fading (with whether it was a rune).
    faded: Option<(Vec<Vec2>, bool, f32)>,
    /// The runes drawn so far in this casting.
    pub runes: Vec<Rune>,
    was_casting: bool,
    /// What to tell the player, and for how long still.
    note: Option<(String, f32)>,
    missile_mesh: Option<(Handle<Mesh>, Handle<StandardMaterial>)>,
    frames: u32,
}

#[derive(Component)]
pub struct MagicNode;

#[derive(Component)]
pub struct Missile {
    velocity: Vec3,
    damage: f32,
    left: f32,
}

impl Magic {
    fn say(&mut self, text: impl Into<String>) {
        self.note = Some((text.into(), 2.5));
    }
}

fn title(rune: Rune) -> String {
    let n = rune.name();
    format!("{}{}", n[..1].to_ascii_uppercase(), &n[1..])
}

/// Cast the runes drawn and carry out what the spell does to the world.
#[allow(clippy::too_many_arguments)]
fn cast(
    commands: &mut Commands,
    magic: &mut Magic,
    ui: &mut Ui,
    s: &mut Scripting,
    fly: &Fly,
    look: Vec3,
    lighting: &mut LevelLighting,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    let runes = std::mem::take(&mut magic.runes);
    let player = s.player;
    match magic.spells.cast(&mut s.world, &mut s.host, player, &runes) {
        Err(CastError::Nothing) => {}
        Err(CastError::RuneNotKnown(r)) => {
            ui.sfx.push("magic_fizzle");
            magic.say(format!("You do not have the rune {}", title(r)));
        }
        Err(CastError::NoSuchSpell) => {
            ui.sfx.push("magic_fizzle");
            magic.say(format!("{}: no such spell", runes.iter().map(|r| title(*r)).collect::<Vec<_>>().join(" ")));
        }
        Err(CastError::NotEnoughMana { needs, .. }) => {
            ui.sfx.push("magic_fizzle");
            magic.say(format!("Not enough mana ({needs:.0} needed)"));
        }
        Ok(done) => {
            ui.sfx.push(done.sound);
            let name = done.spell.name.replace('_', " ");
            if std::env::var_os("ARX_LOG_MAGIC").is_some() {
                eprintln!("cast {} at level {:.1}: {:?}; mana left {:.1}", done.spell.name, done.level, done.effect, s.host.player.mana.current);
            }
            let at = fly.player.feet + Vec3::Y * 100.0;
            match done.effect {
                Effect::Ignite { radius } | Effect::Douse { radius } => {
                    let light = matches!(done.effect, Effect::Ignite { .. });
                    let mut changed = 0;
                    for t in &mut lighting.torches {
                        if t.pos.distance(at) <= radius && t.lit != light {
                            t.lit = light;
                            changed += 1;
                        }
                    }
                    magic.say(format!("{name}: {changed} {}", if light { "lit" } else { "put out" }));
                }
                Effect::Missiles { count, damage } => {
                    let (mesh, material) = magic
                        .missile_mesh
                        .get_or_insert_with(|| {
                            (
                                meshes.add(Sphere::new(7.0).mesh().ico(2).expect("a small sphere")),
                                materials.add(StandardMaterial { base_color: Color::linear_rgb(2.0, 2.6, 6.0), unlit: true, ..default() }),
                            )
                        })
                        .clone();
                    let side = look.cross(Vec3::Y).normalize_or_zero();
                    for i in 0..count {
                        // Side by side, a little apart, and each a little later than the last.
                        let offset = (i as f32 - (count - 1) as f32 / 2.0) * 22.0;
                        let from = fly.pos + look * (40.0 - i as f32 * 25.0) + side * offset - Vec3::Y * 20.0;
                        commands.spawn((Missile { velocity: look * MISSILE_SPEED, damage, left: MISSILE_LIFE }, Mesh3d(mesh.clone()), MeshMaterial3d(material.clone()), Transform::from_translation(from)));
                    }
                    magic.say(name);
                }
                Effect::OnCaster => magic.say(name),
                Effect::ToldOnly => magic.say(format!("{name} (this spell does nothing yet)")),
            }
        }
    }
}

/// The hand: `Ctrl` held makes the mouse a pen.
#[allow(clippy::too_many_arguments)]
pub fn input(
    mut commands: Commands,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    window: Single<&Window>,
    cam: Single<&Transform, (With<Camera3d>, Without<crate::book_hero::BookCamera>)>,
    shot: Option<Res<Shot>>,
    args: Res<LevelArgs>,
    fly: Res<Fly>,
    mut magic: ResMut<Magic>,
    mut ui: ResMut<Ui>,
    mut s: ResMut<Scripting>,
    mut lighting: ResMut<LevelLighting>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let magic = &mut *magic;
    let s = &mut *s;
    magic.frames += 1;
    let look = cam.forward().as_vec3();
    // Headless testing: `--runes all|aam,yok` teaches runes, `--cast aam,yok` casts once the level has settled.
    if magic.frames == 30 {
        for name in &args.runes {
            if name == "all" {
                s.host.player.runes.extend(Rune::ALL.iter().map(|r| r.name().to_owned()));
            } else if let Some(r) = Rune::from_name(name) {
                s.host.player.runes.insert(r.name().to_owned());
            }
        }
    }
    if magic.frames == 90 && !args.cast.is_empty() {
        magic.runes = args.cast.iter().filter_map(|n| Rune::from_name(n)).collect();
        cast(&mut commands, magic, &mut ui, s, &fly, look, &mut lighting, &mut meshes, &mut materials);
    }

    let dt = time.delta_secs().min(0.1);
    if let Some((_, left)) = &mut magic.note {
        *left -= dt;
        if *left <= 0.0 {
            magic.note = None;
        }
    }
    if let Some((_, _, left)) = &mut magic.faded {
        *left -= dt;
        if *left <= 0.0 {
            magic.faded = None;
        }
    }

    let busy = ui.open || ui.book.is_some() || ui.reading.is_some() || s.host.open_container.is_some();
    let casting = shot.is_none() && keys.pressed(KeyCode::ControlLeft) && s.host.stage.controls && fly.walk && !s.host.player.is_dead() && !busy;
    ui.casting = casting;
    if casting && !magic.was_casting {
        magic.runes.clear();
        magic.stroke.clear();
    }
    if casting {
        let middle = Vec2::new(window.width(), window.height()) / 2.0;
        if buttons.just_pressed(MouseButton::Left) {
            // With a cursor the pen is the cursor; with the mouse turning the view it starts in the middle.
            magic.pen = window.cursor_position().filter(|_| ui.cursor_mode).unwrap_or(middle);
            magic.stroke = vec![magic.pen];
        }
        if buttons.pressed(MouseButton::Left) && !magic.stroke.is_empty() {
            magic.pen = match window.cursor_position().filter(|_| ui.cursor_mode) {
                Some(at) => at,
                None => (magic.pen + motion.delta).clamp(Vec2::ZERO, Vec2::new(window.width(), window.height())),
            };
            if magic.stroke.last().is_none_or(|last| last.distance(magic.pen) > 2.0) {
                magic.stroke.push(magic.pen);
            }
        }
        if buttons.just_released(MouseButton::Left) && !magic.stroke.is_empty() {
            let stroke = std::mem::take(&mut magic.stroke);
            let found = recognise(&stroke);
            if std::env::var_os("ARX_LOG_MAGIC").is_some() {
                eprintln!("stroke of {} points: {found:?}", stroke.len());
            }
            match found {
                Some(r) if s.host.player.runes.contains(r.rune.name()) => {
                    magic.runes.push(r.rune);
                    ui.sfx.push(r.rune.sound());
                    magic.say(magic.runes.iter().map(|r| title(*r)).collect::<Vec<_>>().join("  "));
                }
                Some(r) => {
                    ui.sfx.push("magic_fizzle");
                    magic.say(format!("{}: you do not have this rune", title(r.rune)));
                }
                None if stroke.len() > 3 => {
                    ui.sfx.push("magic_fizzle");
                    magic.say("That is no rune");
                }
                None => {}
            }
            magic.faded = Some((stroke, found.is_some(), 0.6));
        }
    } else if magic.was_casting {
        magic.stroke.clear();
        cast(&mut commands, magic, &mut ui, s, &fly, look, &mut lighting, &mut meshes, &mut materials);
    }
    magic.was_casting = casting;
}

/// Spells that last run on; missiles fly and hit.
#[allow(clippy::too_many_arguments)]
pub fn update(
    mut commands: Commands,
    time: Res<Time>,
    mut fly: ResMut<Fly>,
    mut magic: ResMut<Magic>,
    mut ui: ResMut<Ui>,
    mut s: ResMut<Scripting>,
    mut npcs: ResMut<Npcs>,
    mut missiles: Query<(Entity, &mut Missile, &mut Transform)>,
) {
    let dt = time.delta_secs().min(0.1);
    let s = &mut *s;
    for ended in magic.spells.update(&mut s.host, dt * 1000.0) {
        match ended {
            "speed" => ui.sfx.push("magic_spell_speedend"),
            "armor" => ui.sfx.push("magic_spell_armor_end"),
            _ => {}
        }
    }
    fly.player.speed = magic.spells.speed_factor();

    let Some(world) = fly.world.clone() else { return };
    for (entity, mut m, mut tf) in &mut missiles {
        let step = m.velocity * dt;
        let (from, dir, len) = (tf.translation, m.velocity.normalize_or_zero(), step.length());
        m.left -= dt;
        // A character in the way?
        let mut struck = None;
        if let Some(all) = npcs.0.as_ref() {
            for n in all.iter() {
                if n.dead || n.life <= 0.0 || s.host.state(n.id).is_none_or(|st| st.hidden || st.destroyed) {
                    continue;
                }
                // The character's cylinder as a few spheres up its height.
                let feet = Vec3::from(to_bevy(n.pos.to_array()));
                let hit = (0..4).any(|k| {
                    let centre = feet + Vec3::Y * (n.height * (0.15 + 0.25 * k as f32));
                    let along = (centre - from).dot(dir).clamp(0.0, len);
                    (from + dir * along).distance(centre) < n.radius + MISSILE_RADIUS
                });
                if hit {
                    struck = Some(n.id);
                    break;
                }
            }
        }
        let wall = world.raycast(from, dir, len + MISSILE_RADIUS).is_some();
        if let Some(target) = struck {
            let env = Env {
                collision: &world,
                player_pos: Vec3::from(s.world.entity(s.player).pos),
                player_alive: !s.host.player.is_dead(),
                player_stealth: 15.0,
                player_light: 255.0,
                player_torch: false,
            };
            let player = s.player;
            if let Some(all) = npcs.0.as_mut() {
                let done = all.hurt(&mut s.world, &mut s.host, &env, target, m.damage, Some(player));
                if std::env::var_os("ARX_LOG_MAGIC").is_some() {
                    eprintln!("missile hits {}: {done:.1} damage", s.world.entity(target).id_string);
                }
            }
        }
        if struck.is_some() || wall || m.left <= 0.0 {
            if struck.is_some() || wall {
                ui.sfx.push("magic_spell_missilehit");
            }
            commands.entity(entity).despawn();
        } else {
            tf.translation += step;
        }
    }
}

/// The trail of the pen, the runes drawn so far, and what the casting said.
pub fn draw(
    mut commands: Commands,
    arx: Res<Arx>,
    window: Single<&Window>,
    magic: Res<Magic>,
    ui: Res<Ui>,
    font: Res<UiFont>,
    mut assets: ResMut<UiAssets>,
    mut images: ResMut<Assets<Image>>,
    old: Query<Entity, With<MagicNode>>,
) {
    for e in &old {
        commands.entity(e).despawn();
    }
    let (w, h) = (window.width(), window.height());
    let scale = (h / 480.0).max(0.5);
    let mut dot = |at: Vec2, size: f32, color: Color| {
        commands.spawn((
            MagicNode,
            Node { position_type: PositionType::Absolute, left: Val::Px(at.x - size / 2.0), top: Val::Px(at.y - size / 2.0), width: Val::Px(size), height: Val::Px(size), ..default() },
            BackgroundColor(color),
            GlobalZIndex(20),
        ));
    };
    // The stroke being drawn, and the last one fading (golden if it was a rune, red if not).
    let trail = |points: &[Vec2]| -> Vec<Vec2> {
        // Evenly along the path, so that a fast stroke is as solid as a slow one.
        let mut out = Vec::new();
        for pair in points.windows(2) {
            let n = (pair[0].distance(pair[1]) / (3.0 * scale)).ceil().max(1.0) as usize;
            out.extend((0..n).map(|i| pair[0].lerp(pair[1], i as f32 / n as f32)));
        }
        let skip = (out.len() / 400).max(1);
        out.into_iter().step_by(skip).collect()
    };
    for at in trail(&magic.stroke) {
        dot(at, 5.0 * scale, Color::srgba(1.0, 0.85, 0.4, 0.95));
    }
    if let Some((points, good, left)) = &magic.faded {
        let alpha = (left / 0.6).clamp(0.0, 1.0);
        let color = if *good { Color::srgba(1.0, 0.9, 0.5, alpha) } else { Color::srgba(1.0, 0.25, 0.2, alpha) };
        for at in trail(points) {
            dot(at, 5.0 * scale, color);
        }
    }
    if ui.casting && !ui.cursor_mode {
        dot(if magic.stroke.is_empty() { Vec2::new(w, h) / 2.0 } else { magic.pen }, 7.0 * scale, Color::srgba(1.0, 1.0, 0.8, 0.9));
    }
    // The runes of this casting, as their stones, in a row at the top.
    let size = 32.0 * scale;
    let left = w / 2.0 - magic.runes.len() as f32 * size / 2.0;
    for (i, rune) in magic.runes.iter().enumerate() {
        let class = format!("graph/obj3d/interactive/items/magic/rune_aam/rune_{}", rune.name());
        if let Some(tex) = assets.icon(&arx, &mut images, &class, 1) {
            commands.spawn((
                MagicNode,
                Node { position_type: PositionType::Absolute, left: Val::Px(left + i as f32 * size), top: Val::Px(h * 0.60), width: Val::Px(size), height: Val::Px(size), ..default() },
                ImageNode { image: tex.handle, ..default() },
                GlobalZIndex(20),
            ));
        }
    }
    let line = match (&magic.note, ui.casting) {
        (Some((text, _)), _) => Some(text.clone()),
        (None, true) if magic.runes.is_empty() => Some("Draw a rune with the left button held; let go of Ctrl to cast".to_owned()),
        _ => None,
    };
    if let Some(text) = line {
        commands.spawn((
            MagicNode,
            Text::new(text),
            TextFont { font: font.0.clone().into(), font_size: FontSize::Px(15.0 * scale), ..default() },
            TextColor(Color::srgb(0.95, 0.9, 0.7)),
            TextLayout::justify(Justify::Center),
            TextShadow::default(),
            Node { position_type: PositionType::Absolute, left: Val::Px(w * 0.1), top: Val::Px(h * 0.60 + 36.0 * scale), width: Val::Px(w * 0.8), ..default() },
            GlobalZIndex(20),
        ));
    }
}
