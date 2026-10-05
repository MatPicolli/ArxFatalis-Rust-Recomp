//! Experimental: real lights and shadows. The original lights everything per vertex and has no shadows but the dark
//! blobs under things; with this option on, the torches and fires near the camera become point lights of the
//! renderer, which light each pixel and cast shadows (the nearest ones do), on top of the level's baked light, which
//! stays as the ambient light of the scene. It is not how the game looked, and it costs frame time: it is an option
//! (Options > "Dynamic lights and shadows"), off by default.
//!
//! How it is done: every world material is switched from unlit to lit (a material of the renderer lit by an ambient
//! light as strong as the baked vertex colours were), the per-vertex torch light is taken off the level's meshes,
//! and a small pool of point lights is moved to the lit torches nearest the camera each frame, flickering with them.

use crate::book_hero::BookCamera;
use crate::lighting::LevelLighting;
use bevy::prelude::*;

/// Point lights in use at once, and how many of them (the nearest) cast shadows.
const LIGHTS: usize = 8;
const SHADOWED: usize = 2;
/// Torches farther than this from the camera get no light of their own.
const RANGE: f32 = 3200.0;
/// The ambient light under which a lit material shows its colours as an unlit one does: measured, by comparing
/// screenshots of the same view lit both ways (1000 came out a third as bright, 8000 a little too bright).
const AMBIENT: f32 = 6800.0;
/// Lumens for one unit of the engine's light intensity at one world unit of distance: chosen, again by comparing
/// screenshots, so that a torch-lit room is about as bright as the engine's per-vertex light makes it.
const LUMENS_PER_UNIT: f32 = 400_000.0;

#[derive(Resource, Default)]
pub struct DynamicLight {
    pub enabled: bool,
    applied: Option<bool>,
    materials_seen: usize,
    pool: Vec<Entity>,
}

#[derive(Component)]
pub struct TorchLight;

/// World materials are the plain white ones (interface, particles, blob shadows and spell effects are left alone).
fn is_world_material(m: &StandardMaterial) -> bool {
    m.base_color == Color::WHITE && !matches!(m.alpha_mode, AlphaMode::Add | AlphaMode::Multiply)
}

pub fn update(
    mut commands: Commands,
    cam: Single<&Transform, (With<Camera3d>, Without<BookCamera>, Without<TorchLight>)>,
    mut dynamic: ResMut<DynamicLight>,
    mut lighting: ResMut<LevelLighting>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut ambient: ResMut<GlobalAmbientLight>,
    mut lights: Query<(&mut PointLight, &mut Transform, &mut Visibility), With<TorchLight>>,
) {
    let d = &mut *dynamic;
    let on = d.enabled;
    if d.pool.is_empty() {
        if !on {
            return;
        }
        d.pool = (0..LIGHTS).map(|_| commands.spawn((TorchLight, PointLight::default(), Transform::default(), Visibility::Hidden)).id()).collect();
    }
    // Materials: lit or unlit as the option says, also those made since the last look.
    if d.applied != Some(on) || d.materials_seen != materials.len() {
        let wrong: Vec<AssetId<StandardMaterial>> = materials.iter().filter(|(_, m)| is_world_material(m) && m.unlit == on).map(|(id, _)| id).collect();
        for id in wrong {
            if let Some(mut m) = materials.get_mut(id) {
                m.unlit = !on;
                // Stone and skin, not polished metal.
                m.perceptual_roughness = 1.0;
                m.reflectance = 0.0;
                m.metallic = 0.0;
            }
        }
        d.materials_seen = materials.len();
        d.applied = Some(on);
        // The torches light either the vertices or the pixels, not both.
        lighting.vertex_torches = !on;
        *ambient = if on { GlobalAmbientLight { color: Color::WHITE, brightness: AMBIENT, ..default() } } else { GlobalAmbientLight::default() };
    }
    if !on {
        for (_, _, mut vis) in &mut lights {
            *vis = Visibility::Hidden;
        }
        return;
    }
    // The lit torches nearest the camera, nearest first.
    let eye = cam.translation;
    let mut near: Vec<(f32, usize)> = lighting.torches.iter().enumerate().filter(|(_, t)| t.lit).map(|(i, t)| (t.pos.distance(eye), i)).filter(|&(dist, _)| dist < RANGE).collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (slot, entity) in d.pool.iter().enumerate() {
        let Ok((mut light, mut tf, mut vis)) = lights.get_mut(*entity) else { continue };
        let Some(&(_, index)) = near.get(slot) else {
            *vis = Visibility::Hidden;
            continue;
        };
        let t = &lighting.torches[index];
        // Its colour this instant (it flickers), and its strength from the engine's intensity and reach.
        let now = t.now / 255.0;
        let reach = t.fall_start.max(120.0);
        *light = PointLight {
            color: Color::srgb(now.x.min(1.0), now.y.min(1.0), now.z.min(1.0)),
            intensity: LUMENS_PER_UNIT * t.intensity * reach * reach,
            range: t.fall_end * 1.5,
            radius: 4.0,
            shadow_maps_enabled: slot < SHADOWED,
            // The renderer's defaults are for a world in metres; this one is in centimetres.
            shadow_depth_bias: 3.0,
            shadow_normal_bias: 4.0,
            shadow_map_near_z: 6.0,
            ..default()
        };
        tf.translation = t.pos;
        *vis = Visibility::Inherited;
    }
}
