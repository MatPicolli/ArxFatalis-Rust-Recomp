//! The hero as the book shows them (`StatsPage::RenderBookPlayerCharacter`): the whole model standing on the left page
//! of the character sheet with what they wear and wield, and, while a new hero is being made, their face from close
//! up. It is a second copy of the hero's model that only a second camera sees; that camera draws into a picture the
//! interface puts on the page.

use crate::animated::Animated;
use crate::entities::{EntityStats, LevelLights, ModelSerial, SpawnOpts, StaticLight, resolve_model, spawn_entity};
use crate::hud::Ui;
use crate::hud_book::{BOOK_SIZE, BookPage};
use crate::player_body::{BODY_CLASS, Caches, WeaponTag, WeaponVisual, held_transform, make_visual};
use crate::scripting::{ScriptRef, Scripting};
use crate::shadows::Shadow;
use crate::Arx;
use arx_script::EquipSlot;
use bevy::{
    camera::{ClearColorConfig, ImageRenderTarget, RenderTarget, Viewport, visibility::RenderLayers},
    prelude::*,
    render::render_resource::TextureFormat,
};

/// Picture pixels per unit of the book (the book is 513 x 313 units).
const DETAIL: f32 = 2.0;
/// The part of the page the hero is drawn in, in book units: with the book open in the game ...
const BOOK_AREA: (Vec2, Vec2) = (Vec2::new(21.0, 5.0), Vec2::new(182.0, 269.0));
/// ... and while a new hero is made.
const CREATION_AREA: (Vec2, Vec2) = (Vec2::new(44.0, 5.0), Vec2::new(168.0, 231.0));
/// The layer only the book's camera sees.
const LAYER: usize = 1;

/// Marks the book's camera (every other system wants the camera the world is seen through).
#[derive(Component)]
pub struct BookCamera;

/// Marks what only the book's camera sees.
#[derive(Component)]
pub struct BookLayer;

#[derive(Resource, Default)]
pub struct BookHero {
    /// What the camera draws into.
    pub image: Handle<Image>,
    /// The part of the picture in use, in its pixels, and where it goes on the page (book units): `None` until the
    /// hero has been drawn.
    pub shown: Option<(Rect, Rect)>,
    body: Option<Entity>,
    serial: u32,
    pose: String,
    hand: Option<usize>,
    shield_arm: Option<usize>,
    weapon: Option<WeaponVisual>,
    shield: Option<WeaponVisual>,
    lights: Option<LevelLights>,
}

pub fn setup(mut commands: Commands, mut images: ResMut<Assets<Image>>, mut hero: ResMut<BookHero>) {
    let size = (BOOK_AREA.1 * DETAIL).as_uvec2();
    let image = images.add(Image::new_target_texture(size.x, size.y, TextureFormat::Rgba8UnormSrgb, None));
    hero.image = image.clone();
    commands.spawn((
        BookCamera,
        Camera3d::default(),
        // Before the world's camera, onto nothing (the page shows through where the hero is not).
        Camera { order: -1, is_active: false, clear_color: ClearColorConfig::Custom(Color::NONE), ..default() },
        RenderTarget::Image(ImageRenderTarget { handle: image, scale_factor: 1.0 }),
        Projection::Perspective(PerspectiveProjection { near: 5.0, far: 2200.0, ..default() }),
        Transform::IDENTITY,
        RenderLayers::layer(LAYER),
    ));
    // The engine's two lights for this picture: a strong white one behind the viewer and a dim orange one.
    hero.lights = Some(LevelLights(vec![
        StaticLight::new(Vec3::new(-50.0, 50.0, 200.0), Vec3::splat(0.6 * 255.0), 0.0, 3460.0, 3.8),
        StaticLight::new(Vec3::new(50.0, -50.0, -200.0), Vec3::new(0.15, 0.06, 0.003) * 255.0, 2020.0, 2080.0, 8.8),
    ]));
}

/// Keep the hero of the book as the hero is, and draw them while the character sheet is open.
#[allow(clippy::too_many_arguments)]
pub fn update(
    mut commands: Commands,
    arx: Res<Arx>,
    ui: Res<Ui>,
    mut hero: ResMut<BookHero>,
    mut script: ResMut<Scripting>,
    mut caches: Caches,
    mut cam: Single<(&mut Camera, &mut Projection), With<BookCamera>>,
    mut bodies: Query<(&mut Transform, &mut Animated), Without<WeaponTag>>,
    mut held: Query<(&mut Transform, &mut Visibility), With<WeaponTag>>,
    roots: Query<&Children, With<BookLayer>>,
    layered: Query<(), With<RenderLayers>>,
) {
    let open = ui.book == Some(BookPage::Stats);
    cam.0.is_active = open;
    if !open {
        return;
    }
    let hero = &mut *hero;
    let s = &mut *script;
    let player = s.player;
    let Some(st) = s.host.state(player).cloned() else { return };
    let Some(lights) = hero.lights.take() else { return };
    let creating = ui.creating;

    // Where the engine stands the hero in front of its camera (which looks along Arx +Z from the origin), and how
    // they are turned; a new hero shows each face from a slightly different side.
    let (pos, yaw) = if creating {
        ([8.0, 162.0, 75.0], [-25.0, -10.0, 20.0, 35.0][usize::from(s.host.player.skin.min(3))])
    } else {
        ([20.0, 96.0, 260.0], -20.0)
    };
    let transform = Transform {
        translation: Vec3::new(pos[0], -pos[1], -pos[2]),
        rotation: arx_level::entity_rotation([0.0, 180.0 - yaw, 0.0], true),
        scale: Vec3::ONE,
    };

    // The model: built again whenever the hero's own changes (armour, the face).
    if hero.body.is_none() || hero.serial != st.model_serial {
        if let Some(old) = hero.body.take() {
            commands.entity(old).despawn();
        }
        hero.serial = st.model_serial;
        hero.pose.clear();
        let mut stats = EntityStats::default();
        let opts = SpawnOpts { hide_selection: None, not_pickable: true };
        let made = spawn_entity(
            &mut commands, &arx.0, &lights.0, s, &mut caches.pickables.0, &mut caches.ecache, &mut caches.tcache, &mut caches.meshes, &mut caches.materials, &mut caches.images,
            BODY_CLASS, pos, [0.0, 180.0 - yaw, 0.0], 1, player, true, &mut stats, &opts,
        );
        if let Some(e) = made {
            // It is a picture, not somebody in the level: scripts, shadows and rebuilding do not concern it.
            commands.entity(e).remove::<(ScriptRef, Shadow, ModelSerial)>().insert((BookLayer, Visibility::Inherited));
            hero.body = Some(e);
        }
        let model = resolve_model(&mut caches.ecache, &arx.0, BODY_CLASS, &st);
        let find = |name: &str| model.as_ref().and_then(|(ftl, _)| ftl.actions.iter().find(|a| a.name.eq_ignore_ascii_case(name)).map(|a| a.vertex as usize));
        hero.hand = find("primary_attach");
        hero.shield_arm = find("shield_attach");
    }

    // What is in the hands.
    let (weapon_item, shield_item) = (s.host.player_weapon(), s.host.player.equipped_in(EquipSlot::Shield));
    for (slot, item, grip) in [(&mut hero.weapon, weapon_item, "primary_attach"), (&mut hero.shield, shield_item, "shield_attach")] {
        if slot.as_ref().map(|w| w.item) != item {
            if let Some(old) = slot.take() {
                commands.entity(old.entity).despawn();
            }
            *slot = item.and_then(|item| make_visual(&mut commands, &arx, &lights, s, &mut caches, item, grip));
            if let Some(w) = slot {
                commands.entity(w.entity).remove::<(ScriptRef, Shadow, ModelSerial)>().insert(BookLayer);
            }
        }
    }
    hero.lights = Some(lights);

    // Whatever was just built is for the book's camera only.
    for children in &roots {
        for child in children.iter() {
            if layered.get(child).is_err() {
                commands.entity(child).insert(RenderLayers::layer(LAYER));
            }
        }
    }

    // The camera: the engine's focal length of 520 on a 480-line screen, spread over the height of the book, aimed
    // at the middle of the hero's part of the page. The picture is that part of the page and nothing else.
    let (min, size) = if creating { CREATION_AREA } else { BOOK_AREA };
    let focal = BOOK_SIZE.y * 520.0 / 480.0;
    let pixels = (size * DETAIL).as_uvec2();
    cam.0.viewport = Some(Viewport { physical_position: UVec2::ZERO, physical_size: pixels, ..default() });
    if let Projection::Perspective(p) = &mut *cam.1 {
        p.fov = 2.0 * (size.y * 0.5 / focal).atan();
    }
    // A new hero is cut off where the engine cuts (its scissor), clear of the numbers at the sides of the page.
    let visible = if creating { Rect::new(75.0, 5.0, 212.0, 219.0) } else { Rect::from_corners(min, min + size) };
    hero.shown = Some((Rect::from_corners((visible.min - min) * DETAIL, (visible.max - min) * DETAIL), visible));

    // The pose: standing as the book shows them, both hands on a two-handed weapon.
    let two_handed = weapon_item.and_then(|w| s.host.state(w)).is_some_and(|st| st.type_flags & arx_script::object_type::TWO_HANDED != 0);
    let pose = if two_handed && !creating { "graph/obj3d/anims/npc/human_wait_book_2handed.tea" } else { "graph/obj3d/anims/npc/human_wait_book.tea" };
    let Some(body) = hero.body else { return };
    let Ok((mut tf, mut anim)) = bodies.get_mut(body) else { return };
    *tf = transform;
    anim.keep_pose = true;
    if hero.pose != pose
        && let Some(tea) = s.anim(&arx.0, pose)
    {
        anim.anim = Some(tea);
        anim.looping = true;
        anim.elapsed_us = 0;
        anim.overlay = None;
        hero.pose = pose.to_owned();
    }
    let (tf, anim) = (*tf, &*anim);
    for (w, vertex) in [(&hero.weapon, hero.hand), (&hero.shield, hero.shield_arm)] {
        if let (Some(w), Some(v)) = (w, vertex)
            && let Ok((mut at, mut vis)) = held.get_mut(w.entity)
        {
            match held_transform(w, v, anim, &tf) {
                // A face is all a new hero shows.
                Some(t) if !creating => {
                    *at = t;
                    *vis = Visibility::Inherited;
                }
                _ => *vis = Visibility::Hidden,
            }
        }
    }
}
