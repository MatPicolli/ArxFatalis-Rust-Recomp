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
    /// An animation under the first: it moves the bones the first leaves alone (a fighter's legs keep their
    /// stance while a blow, which is arms and chest only, plays).
    pub under: Option<Overlay>,
    /// Keep each frame's [`Pose`] (bone rotations and positions) in `pose`, to attach things to the body.
    pub keep_pose: bool,
    pub pose: Option<Pose>,
    /// What the meshes show now: a pose that has not changed is not built and uploaded again.
    pub shown: Option<Shown>,
    /// Extra rotations on some bones (a bone and a rotation in the model's axes): the hero bending to look up or down.
    pub bend: Vec<(usize, Quat)>,
}

/// The animations and times a pose was built from.
#[derive(Clone, Copy, PartialEq)]
pub struct Shown {
    anim: usize,
    time: i64,
    overlay: Option<(usize, i64)>,
    under: Option<(usize, i64)>,
    /// The bend, to a thousandth.
    bend: i64,
}

/// Mark a mesh entity so it is not culled by its (stale) bind-pose bounds.
pub fn no_cull() -> NoFrustumCulling {
    NoFrustumCulling
}

/// Within this distance of the camera a pose is rebuilt every frame; farther away every third frame, and beyond
/// [`FAR`] every tenth: rebuilding means uploading the meshes again, and at a distance nobody sees the difference.
const NEAR: f32 = 1500.0;
const FAR: f32 = 3500.0;

pub fn animate(
    time: Res<Time>,
    cam: Single<&Transform, (With<Camera3d>, Without<crate::book_hero::BookCamera>)>,
    mut query: Query<(Entity, &mut Animated, Option<&GlobalTransform>)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut frame: Local<u32>,
) {
    *frame = frame.wrapping_add(1);
    let dt = (time.delta_secs_f64() * 1e6) as i64;
    for (entity, mut a, at) in &mut query {
        a.elapsed_us += dt;
        if let Some(o) = &mut a.overlay {
            o.elapsed_us += dt;
        }
        if let Some(o) = &mut a.under {
            o.elapsed_us += dt;
        }
        let Some(anim) = a.anim.clone() else { continue };
        let t = if a.looping { anim.looped_time(a.elapsed_us) } else { a.elapsed_us.clamp(0, anim.duration_us) };
        let distance = at.map_or(0.0, |g| g.translation().distance(cam.translation));
        let every = if distance < NEAR { 1 } else if distance < FAR { 3 } else { 10 };
        if (frame.wrapping_add(entity.index_u32())) % every != 0 {
            continue;
        }
        // Most things stand still most of the time (a shut door, a lever, a corpse): nothing to do for them.
        let at_time = |o: &Overlay| if o.looping { o.anim.looped_time(o.elapsed_us) } else { o.elapsed_us.clamp(0, o.anim.duration_us) };
        let overlay_at = a.overlay.as_ref().map(|o| (Arc::as_ptr(&o.anim) as usize, at_time(o)));
        let under_at = a.under.as_ref().map(|o| (Arc::as_ptr(&o.anim) as usize, at_time(o)));
        let bend = a.bend.iter().map(|(bone, q)| (*bone as i64 + 1) * ((q.x * 1000.0) as i64 * 7 + (q.y * 1000.0) as i64 * 13 + (q.z * 1000.0) as i64 * 17 + (q.w * 1000.0) as i64)).sum();
        let shown = Shown { anim: Arc::as_ptr(&anim) as usize, time: t, overlay: overlay_at, under: under_at, bend };
        if a.shown == Some(shown) {
            continue;
        }
        a.shown = Some(shown);
        let world: Vec<[f32; 3]> = if a.overlay.is_some() || a.under.is_some() || a.keep_pose {
            let top = a.overlay.as_ref().map(|o| {
                let t = if o.looping { o.anim.looped_time(o.elapsed_us) } else { o.elapsed_us.clamp(0, o.anim.duration_us) };
                (o.anim.clone(), t)
            });
            let mut layers: Vec<(&Tea, i64)> = Vec::new();
            if let Some((o, ot)) = &top {
                layers.push((o, *ot));
            }
            layers.push((&anim, t));
            let below = a.under.as_ref().map(|o| (o.anim.clone(), at_time(o)));
            if let Some((o, ot)) = &below {
                layers.push((o, *ot));
            }
            let pose = a.skeleton.pose_layers_bent(&layers, Vec3::ZERO.to_array().into(), &a.bend);
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
            crate::perf::TOUCHED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let positions: Vec<[f32; 3]> = m.src.iter().map(|&i| world[i as usize]).collect();
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
            if mesh.attribute(Mesh::ATTRIBUTE_NORMAL).is_some() {
                mesh.compute_flat_normals();
            }
        }
    }
}
