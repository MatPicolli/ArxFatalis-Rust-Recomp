//! CPU skinning: re-poses a model's meshes every frame from a skeleton and an animation.

use crate::convert::to_bevy;
use arx_formats::{skeleton::{Pose, Skeleton}, tea::Tea};
use bevy::{camera::visibility::NoFrustumCulling, prelude::*};
use std::sync::Arc;

/// One spawned mesh and, for each of its vertices, the index of the source model vertex.
pub struct MeshSrc {
    pub handle: Handle<Mesh>,
    pub src: Vec<u32>,
}

/// A second animation played over the first: where it moves a bone, it wins (an arm's attack over the legs' walk).
pub struct Overlay {
    pub anim: Arc<Tea>,
    pub elapsed_us: i64,
    pub looping: bool,
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
    pub overlay: Option<Overlay>,
    /// Keep each frame's [`Pose`] (bone rotations and positions) in `pose`, to attach things to the body.
    pub keep_pose: bool,
    pub pose: Option<Pose>,
}

/// Mark a mesh entity so it is not culled by its (stale) bind-pose bounds.
pub fn no_cull() -> NoFrustumCulling {
    NoFrustumCulling
}

pub fn animate(time: Res<Time>, mut query: Query<&mut Animated>, mut meshes: ResMut<Assets<Mesh>>) {
    let dt = (time.delta_secs_f64() * 1e6) as i64;
    for mut a in &mut query {
        a.elapsed_us += dt;
        if let Some(o) = &mut a.overlay {
            o.elapsed_us += dt;
        }
        let Some(anim) = a.anim.clone() else { continue };
        let t = if a.looping { anim.looped_time(a.elapsed_us) } else { a.elapsed_us.clamp(0, anim.duration_us) };
        let world: Vec<[f32; 3]> = if a.overlay.is_some() || a.keep_pose {
            let top = a.overlay.as_ref().map(|o| {
                let t = if o.looping { o.anim.looped_time(o.elapsed_us) } else { o.elapsed_us.clamp(0, o.anim.duration_us) };
                (o.anim.clone(), t)
            });
            let mut layers: Vec<(&Tea, i64)> = Vec::new();
            if let Some((o, ot)) = &top {
                layers.push((o, *ot));
            }
            layers.push((&anim, t));
            let pose = a.skeleton.pose_layers(&layers, Vec3::ZERO.to_array().into());
            let out = pose.vertices.iter().map(|p| to_bevy(p.to_array())).collect();
            if a.keep_pose {
                a.pose = Some(pose);
            }
            out
        } else {
            let pose = if a.root_motion { a.skeleton.pose_object(&anim, t) } else { a.skeleton.pose(&anim, t) };
            pose.iter().map(|p| to_bevy(p.to_array())).collect()
        };
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
