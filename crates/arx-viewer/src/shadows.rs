//! The shadows of the original: soft dark blobs on the floor under everything that stands in the scene
//! (`ARXDRAW_DrawInterShadows`). A character gets one under each of its bones, sized by the model's own shadow size for
//! that bone, so the blobs merge into a rough outline of the body; a simple object gets small ones under some of its
//! vertices. They fade as the thing rises off the floor. All blobs are one mesh rebuilt every frame.

use crate::animated::Animated;
use crate::convert::to_bevy;
use crate::Fly;
use arx_formats::ftl::Ftl;
use bevy::{
    asset::RenderAssetUsages,
    camera::visibility::NoFrustumCulling,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};

/// Corners of a blob's outline.
const SEGMENTS: usize = 10;
/// Things farther than this from the camera cast no blob.
const RANGE: f32 = 1800.0;

/// What casts the blobs of one entity.
#[derive(Component)]
pub struct Shadow {
    /// Characters and other jointed models: one blob per bone, at the bone's origin vertex.
    jointed: bool,
    /// Vertex index, its place in the model (Bevy axes), and the blob's scale.
    spots: Vec<(usize, Vec3, f32)>,
}

impl Shadow {
    /// From the model: its groups' origins and shadow sizes, or every ninth vertex if it has no joints.
    pub fn from_model(ftl: &Ftl, scale: f32) -> Shadow {
        let at = |i: usize| Vec3::from(to_bevy(ftl.vertices[i].pos));
        if ftl.groups.len() > 1 {
            let spots = ftl.groups.iter().filter(|g| g.blob_shadow_size > 0.0).map(|g| (g.origin as usize, at(g.origin as usize), g.blob_shadow_size)).collect();
            Shadow { jointed: true, spots }
        } else {
            let spots = (0..ftl.vertices.len()).step_by(9).map(|i| (i, at(i), scale)).collect();
            Shadow { jointed: false, spots }
        }
    }
}

#[derive(Resource, Default)]
pub struct Shadows {
    mesh: Option<Handle<Mesh>>,
}

/// One mesh for all blobs.
pub fn setup(mut commands: Commands, mut shadows: ResMut<Shadows>, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>) {
    // No texture: each blob is a small fan, dark in the middle and clear at the rim.
    let material = materials.add(StandardMaterial { base_color: Color::BLACK, unlit: true, alpha_mode: AlphaMode::Blend, cull_mode: None, double_sided: true, ..default() });
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    // Never empty (the renderer cannot hold a mesh without vertices): one triangle with no size until there are blobs.
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0f32; 3]; 3]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[0.0f32; 4]; 3]);
    mesh.insert_indices(Indices::U32(vec![0, 1, 2]));
    let handle = meshes.add(mesh);
    commands.spawn((Mesh3d(handle.clone()), MeshMaterial3d(material), NoFrustumCulling, Transform::default()));
    shadows.mesh = Some(handle);
}

/// Lay a blob on the floor under every spot of every visible thing near the camera.
pub fn update(
    cam: Single<&Transform, With<Camera3d>>,
    fly: Res<Fly>,
    shadows: Res<Shadows>,
    casters: Query<(&Shadow, &Transform, &InheritedVisibility, Option<&Animated>)>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let (Some(handle), Some(world)) = (shadows.mesh.as_ref(), fly.world.as_ref()) else { return };
    let eye = cam.translation;
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut colors: Vec<[f32; 4]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    for (shadow, tf, visible, animated) in &casters {
        if !visible.get() || tf.translation.distance(eye) > RANGE {
            continue;
        }
        let pose = animated.and_then(|a| a.pose.as_ref());
        for &(vertex, rest, scale) in &shadow.spots {
            let local = pose.and_then(|p| p.vertices.get(vertex)).map_or(rest, |v| Vec3::from(to_bevy(v.to_array())));
            let at = tf.transform_point(local);
            let Some(floor) = world.floor_height(at.x, at.z, at.y + 20.0) else { continue };
            let lift = (at.y - floor).abs();
            let strength = if shadow.jointed { (0.8 - lift * 0.002) * scale } else { (0.5 - lift * 0.002) * scale };
            if strength <= 0.0 {
                continue;
            }
            let half = if shadow.jointed { 44.0 } else { 16.0 } * scale * 0.5;
            let y = floor + 1.5;
            // The engine darkens the picture as it is shown (`dst x (1 - strength)`); Bevy blends in linear light,
            // where the same darkening needs a stronger alpha.
            let alpha = 1.0 - (1.0 - strength.min(1.0)).powf(2.2);
            // A fan: the middle, an inner ring at full strength and an outer ring that fades to nothing.
            let base = positions.len() as u32;
            positions.push([at.x, y, at.z]);
            colors.push([0.0, 0.0, 0.0, alpha]);
            for (radius, a) in [(half * 0.7, alpha), (half * 1.25, 0.0)] {
                for k in 0..SEGMENTS {
                    let ang = k as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
                    positions.push([at.x + ang.cos() * radius, y, at.z + ang.sin() * radius]);
                    colors.push([0.0, 0.0, 0.0, a]);
                }
            }
            let n = SEGMENTS as u32;
            for k in 0..n {
                let (i0, i1) = (base + 1 + k, base + 1 + (k + 1) % n);
                let (o0, o1) = (i0 + n, i1 + n);
                indices.extend([base, i0, i1, i0, o0, o1, i0, o1, i1]);
            }
        }
    }
    if positions.is_empty() {
        (positions, colors, indices) = (vec![[0.0; 3]; 3], vec![[0.0; 4]; 3], vec![0, 1, 2]);
    }
    if let Some(mut mesh) = meshes.get_mut(handle) {
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
        mesh.insert_indices(Indices::U32(indices));
    }
}
