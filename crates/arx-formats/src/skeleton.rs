//! Skeletons derived from an object's vertex groups, and pose evaluation.
//!
//! Every vertex group of an [`Ftl`] is a bone. A bone's parent is the nearest earlier group that
//! contains the bone's origin vertex. Each vertex is bound rigidly to the last group listing it
//! (vertices in no group go to the root bone).

use crate::ftl::Ftl;
use crate::tea::Tea;
use glam::{Mat3, Quat, Vec3};

#[derive(Debug, Clone)]
pub struct Bone {
    pub parent: Option<usize>,
    /// Bind-pose position of the bone's origin.
    pub origin: Vec3,
    /// Offset from the parent's origin (or the absolute origin for the root).
    pub rest_offset: Vec3,
}

#[derive(Debug, Clone)]
pub struct Skeleton {
    pub bones: Vec<Bone>,
    /// Bone each vertex is bound to.
    pub vertex_bone: Vec<usize>,
    /// Vertex position relative to its bone's origin in the bind pose.
    pub vertex_local: Vec<Vec3>,
}

/// Slerp as implemented by the original engine (shortest path, lerp when nearly parallel).
fn slerp(from: Quat, mut to: Quat, ratio: f32) -> Quat {
    let mut cos = from.dot(to);
    if cos < 0.0 {
        cos = -cos;
        to = -to;
    }
    let (mut beta, mut ratio) = (1.0 - ratio, ratio);
    if 1.0 - cos > 0.001 {
        let theta = cos.acos();
        let t = 1.0 / theta.sin();
        beta = (theta * beta).sin() * t;
        ratio = (theta * ratio).sin() * t;
    }
    Quat::from_xyzw(
        beta * from.x + ratio * to.x,
        beta * from.y + ratio * to.y,
        beta * from.z + ratio * to.z,
        beta * from.w + ratio * to.w,
    )
}

impl Skeleton {
    pub fn from_ftl(ftl: &Ftl) -> Self {
        let pos = |v: u32| Vec3::from(ftl.vertices[v as usize].pos);
        let n_vert = ftl.vertices.len();

        if ftl.groups.is_empty() {
            return Skeleton {
                bones: vec![Bone { parent: None, origin: Vec3::ZERO, rest_offset: Vec3::ZERO }],
                vertex_bone: vec![0; n_vert],
                vertex_local: (0..n_vert as u32).map(pos).collect(),
            };
        }

        let mut vertex_bone = vec![usize::MAX; n_vert];
        for (gi, g) in ftl.groups.iter().enumerate().rev() {
            for &v in &g.indices {
                if vertex_bone[v as usize] == usize::MAX {
                    vertex_bone[v as usize] = gi;
                }
            }
        }
        for b in &mut vertex_bone {
            if *b == usize::MAX {
                *b = 0;
            }
        }

        let mut bones: Vec<Bone> = Vec::with_capacity(ftl.groups.len());
        for (gi, g) in ftl.groups.iter().enumerate() {
            let origin = pos(g.origin);
            let parent = (0..gi).rev().find(|&p| ftl.groups[p].indices.contains(&g.origin));
            let rest_offset = parent.map_or(origin, |p| origin - bones[p].origin);
            bones.push(Bone { parent, origin, rest_offset });
        }

        let vertex_local = (0..n_vert).map(|v| pos(v as u32) - bones[vertex_bone[v]].origin).collect();
        Skeleton { bones, vertex_bone, vertex_local }
    }

    /// Vertex positions (Arx coordinates, object space) for `anim` at `time_us`.
    pub fn pose(&self, anim: &Tea, time_us: i64) -> Vec<Vec3> {
        let (frame, t) = anim.locate(time_us);
        self.pose_at(anim, frame, t)
    }

    pub fn pose_at(&self, anim: &Tea, frame: usize, t: f32) -> Vec<Vec3> {
        let animated = anim.frames.len() > 1;

        // Local transform of each bone.
        let n = self.bones.len();
        let mut init_rot = vec![Quat::IDENTITY; n];
        let mut init_trans: Vec<Vec3> = self.bones.iter().map(|b| b.rest_offset).collect();
        let mut init_scale = vec![Vec3::ZERO; n];
        if animated {
            for j in 0..n.min(anim.group_count) {
                let (s, e) = (anim.group(frame, j), anim.group(frame + 1, j));
                init_rot[j] = slerp(s.rotate, e.rotate, t);
                init_trans[j] = s.translate.lerp(e.translate, t) + self.bones[j].rest_offset;
                init_scale[j] = s.zoom.lerp(e.zoom, t);
            }
        }

        // Concatenate down the hierarchy (parents always precede children).
        let mut quat = vec![Quat::IDENTITY; n];
        let mut trans = vec![Vec3::ZERO; n];
        let mut scale = vec![Vec3::ONE; n];
        for (j, bone) in self.bones.iter().enumerate() {
            match bone.parent {
                Some(p) => {
                    quat[j] = quat[p] * init_rot[j];
                    trans[j] = trans[p] + quat[p] * (init_trans[j] * scale[p]);
                    scale[j] = (init_scale[j] + Vec3::ONE) * scale[p];
                }
                None => {
                    quat[j] = init_rot[j];
                    trans[j] = init_trans[j];
                    scale[j] = init_scale[j] + Vec3::ONE;
                }
            }
        }

        let matrices: Vec<Mat3> = (0..n).map(|j| Mat3::from_quat(quat[j]) * Mat3::from_diagonal(scale[j])).collect();
        self.vertex_bone
            .iter()
            .zip(&self.vertex_local)
            .map(|(&b, &local)| matrices[b] * local + trans[b])
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slerp_endpoints_and_midpoint() {
        let a = Quat::IDENTITY;
        let b = Quat::from_rotation_y(1.0);
        assert!(slerp(a, b, 0.0).abs_diff_eq(a, 1e-5));
        assert!(slerp(a, b, 1.0).abs_diff_eq(b, 1e-5));
        assert!(slerp(a, b, 0.5).abs_diff_eq(Quat::from_rotation_y(0.5), 1e-4));
        // Opposite-sign quaternions describe the same rotation: take the short way round.
        assert!(slerp(a, -b, 0.5).abs_diff_eq(Quat::from_rotation_y(0.5), 1e-4));
    }
}
