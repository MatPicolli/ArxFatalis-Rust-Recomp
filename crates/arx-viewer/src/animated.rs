//! CPU skinning: re-poses a model's meshes every frame from a skeleton and an animation.

use crate::convert::to_bevy;
use arx_formats::{skeleton::Skeleton, tea::Tea};
use bevy::{camera::visibility::NoFrustumCulling, prelude::*};
use std::sync::Arc;

/// One spawned mesh and, for each of its vertices, the index of the source model vertex.
pub struct MeshSrc {
    pub handle: Handle<Mesh>,
    pub src: Vec<u32>,
}

#[derive(Component)]
pub struct Animated {
    pub skeleton: Arc<Skeleton>,
    /// `None` shows the bind pose.
    pub anim: Option<Arc<Tea>>,
    pub meshes: Vec<MeshSrc>,
    pub elapsed_us: i64,
    /// Looping animations wrap; one-shot ones hold their last frame.
    pub looping: bool,
    /// Apply the animation's translation of the whole object (gates, trapdoors); characters walk by other means.
    pub root_motion: bool,
}

/// Mark a mesh entity so it is not culled by its (stale) bind-pose bounds.
pub fn no_cull() -> NoFrustumCulling {
    NoFrustumCulling
}

pub fn animate(time: Res<Time>, mut query: Query<&mut Animated>, mut meshes: ResMut<Assets<Mesh>>) {
    let dt = (time.delta_secs_f64() * 1e6) as i64;
    for mut a in &mut query {
        a.elapsed_us += dt;
        let Some(anim) = a.anim.clone() else { continue };
        let t = if a.looping { anim.looped_time(a.elapsed_us) } else { a.elapsed_us.clamp(0, anim.duration_us) };
        let pose = if a.root_motion { a.skeleton.pose_object(&anim, t) } else { a.skeleton.pose(&anim, t) };
        let world: Vec<[f32; 3]> = pose.iter().map(|p| to_bevy(p.to_array())).collect();
        for m in &a.meshes {
            let Some(mut mesh) = meshes.get_mut(&m.handle) else { continue };
            let positions: Vec<[f32; 3]> = m.src.iter().map(|&i| world[i as usize]).collect();
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
            if mesh.attribute(Mesh::ATTRIBUTE_NORMAL).is_some() {
                mesh.compute_flat_normals();
            }
        }
    }
}
