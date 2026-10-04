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

    /// Like [`Skeleton::pose`] for something that is not a character: the animation's root translation is applied
    /// too (the original ignores it for NPCs, whose movement it drives instead).
    pub fn pose_object(&self, anim: &Tea, time_us: i64) -> Vec<Vec3> {
        let (frame, t) = anim.locate(time_us);
        let root = if anim.frames.len() > 1 { anim.root_translation(frame, t) } else { Vec3::ZERO };
        self.pose_with_root(anim, frame, t, root)
    }

    pub fn pose_at(&self, anim: &Tea, frame: usize, t: f32) -> Vec<Vec3> {
        self.pose_with_root(anim, frame, t, Vec3::ZERO)
    }

    fn pose_with_root(&self, anim: &Tea, frame: usize, t: f32, root: Vec3) -> Vec<Vec3> {
        let (rot, trans, scale) = self.locals(&[(anim, frame, t)]);
        self.concatenate(&rot, &trans, &scale, root).vertices
    }

    /// Local transform of every bone from the animation layers given from the topmost down, each as (animation, frame,
    /// blend). A bone that a higher layer animates takes nothing from the lower ones (the engine's `Cedric_AnimateObject`),
    /// which is how an arm's attack plays over the legs' walk.
    fn locals(&self, layers: &[(&Tea, usize, f32)]) -> (Vec<Quat>, Vec<Vec3>, Vec<Vec3>) {
        let n = self.bones.len();
        let mut rot = vec![Quat::IDENTITY; n];
        let mut trans: Vec<Vec3> = self.bones.iter().map(|b| b.rest_offset).collect();
        let mut scale = vec![Vec3::ZERO; n];
        let mut done = vec![false; n];
        for &(anim, frame, t) in layers {
            if anim.frames.len() <= 1 {
                continue;
            }
            for j in 0..n.min(anim.group_count) {
                if done[j] {
                    continue;
                }
                if !anim.void_groups.get(j).copied().unwrap_or(false) {
                    done[j] = true;
                }
                let (s, e) = (anim.group(frame, j), anim.group(frame + 1, j));
                rot[j] = slerp(s.rotate, e.rotate, t);
                trans[j] = s.translate.lerp(e.translate, t) + self.bones[j].rest_offset;
                scale[j] = s.zoom.lerp(e.zoom, t);
            }
        }
        (rot, trans, scale)
    }

    /// Pose from several layers at once (topmost first), each an animation and the time into it. `root` is added to the
    /// root bone. Also gives each bone's final rotation and position, for attaching things to it.
    pub fn pose_layers(&self, layers: &[(&Tea, i64)], root: Vec3) -> Pose {
        self.pose_layers_bent(layers, root, &[])
    }

    /// [`Skeleton::pose_layers`] with extra rotations put on some bones before their animation's own (the engine's
    /// `ex_rotate`): how a body bends at the belt, chest, neck and head to look up or down. Each is a bone and a
    /// rotation in the model's own axes (Arx).
    pub fn pose_layers_bent(&self, layers: &[(&Tea, i64)], root: Vec3, bend: &[(usize, Quat)]) -> Pose {
        let located: Vec<(&Tea, usize, f32)> = layers
            .iter()
            .map(|&(anim, time)| {
                let (frame, t) = anim.locate(time);
                (anim, frame, t)
            })
            .collect();
        let (mut rot, trans, scale) = self.locals(&located);
        for &(bone, extra) in bend {
            if let Some(r) = rot.get_mut(bone) {
                *r = extra * *r;
            }
        }
        self.concatenate(&rot, &trans, &scale, root)
    }

    fn concatenate(&self, init_rot: &[Quat], init_trans: &[Vec3], init_scale: &[Vec3], root: Vec3) -> Pose {
        let n = self.bones.len();
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
                    trans[j] = init_trans[j] + root;
                    scale[j] = init_scale[j] + Vec3::ONE;
                }
            }
        }

        let matrices: Vec<Mat3> = (0..n).map(|j| Mat3::from_quat(quat[j]) * Mat3::from_diagonal(scale[j])).collect();
        let vertices = self
            .vertex_bone
            .iter()
            .zip(&self.vertex_local)
            .map(|(&b, &local)| matrices[b] * local + trans[b])
            .collect();
        Pose { vertices, bone_quat: quat, bone_trans: trans }
    }
}

/// A skeleton posed: where every vertex is (Arx coordinates, object space), and each bone's rotation and origin.
#[derive(Debug, Clone)]
pub struct Pose {
    pub vertices: Vec<Vec3>,
    pub bone_quat: Vec<Quat>,
    pub bone_trans: Vec<Vec3>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gate_rises_by_its_root_translation_while_a_character_does_not() {
        use crate::tea::{GroupAnim, KeyFrame};
        let key = |t: i64, y: f32| KeyFrame { num_frame: 0, time_us: t, translate: Vec3::new(0.0, y, 0.0), rotate: Quat::IDENTITY, step_sound: false };
        let g = GroupAnim { rotate: Quat::IDENTITY, translate: Vec3::ZERO, zoom: Vec3::ZERO };
        let anim = Tea {
            name: String::new(),
            frames: vec![key(0, 0.0), key(1_000_000, -200.0)],
            group_count: 1,
            groups: vec![g; 2],
            void_groups: vec![true],
            duration_us: 1_000_000,
        };
        let sk = Skeleton {
            bones: vec![Bone { parent: None, origin: Vec3::ZERO, rest_offset: Vec3::ZERO }],
            vertex_bone: vec![0, 0],
            vertex_local: vec![Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, -10.0, 0.0)],
        };
        let half = sk.pose_object(&anim, 500_000);
        assert!((half[0] - Vec3::new(1.0, -100.0, 0.0)).length() < 1e-3, "{:?}", half[0]);
        let end = sk.pose_object(&anim, 1_000_000);
        assert!((end[1] - Vec3::new(0.0, -210.0, 0.0)).length() < 1e-3, "{:?}", end[1]);
        assert!((sk.pose(&anim, 1_000_000)[0] - Vec3::new(1.0, 0.0, 0.0)).length() < 1e-4, "characters stay in place");
    }

    #[test]
    fn a_higher_layer_wins_on_the_bones_it_animates_and_leaves_the_rest_to_the_lower_one() {
        use crate::tea::{GroupAnim, KeyFrame};
        let key = |t: i64| KeyFrame { num_frame: 0, time_us: t, translate: Vec3::ZERO, rotate: Quat::IDENTITY, step_sound: false };
        let turn = |angle: f32| GroupAnim { rotate: Quat::from_rotation_z(angle), translate: Vec3::ZERO, zoom: Vec3::ZERO };
        let still = GroupAnim { rotate: Quat::IDENTITY, translate: Vec3::ZERO, zoom: Vec3::ZERO };
        // Two bones: a root at the origin and a child one unit along x. Vertex 0 is on the root, vertex 1 on the child.
        let sk = Skeleton {
            bones: vec![
                Bone { parent: None, origin: Vec3::ZERO, rest_offset: Vec3::ZERO },
                Bone { parent: Some(0), origin: Vec3::X, rest_offset: Vec3::X },
            ],
            vertex_bone: vec![0, 1],
            vertex_local: vec![Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, 1.0, 0.0)],
        };
        let quarter = std::f32::consts::FRAC_PI_2;
        // The lower layer turns both bones a quarter turn; the upper one turns only the child, the other way.
        let legs = Tea { name: String::new(), frames: vec![key(0), key(1000)], group_count: 2, groups: vec![turn(quarter), turn(quarter), turn(quarter), turn(quarter)], void_groups: vec![false, false], duration_us: 1000 };
        let arm = Tea { name: String::new(), frames: vec![key(0), key(1000)], group_count: 2, groups: vec![still, turn(-quarter), still, turn(-quarter)], void_groups: vec![true, false], duration_us: 1000 };
        let both = sk.pose_layers(&[(&arm, 0), (&legs, 0)], Vec3::ZERO);
        // Vertex 0 follows the legs (a quarter turn about z carries (0,1,0) to (-1,0,0)).
        assert!((both.vertices[0] - Vec3::new(-1.0, 0.0, 0.0)).length() < 1e-4, "{:?}", both.vertices[0]);
        // The child's own turn comes from the arm: it is not the legs' quarter turn on top of the root's.
        let only_legs = sk.pose_layers(&[(&legs, 0)], Vec3::ZERO);
        assert!((both.vertices[1] - only_legs.vertices[1]).length() > 0.5, "the arm changed the child: {:?} vs {:?}", both.vertices[1], only_legs.vertices[1]);
        // Bone rotations are reported for attaching things.
        assert!(both.bone_quat[0].angle_between(Quat::from_rotation_z(quarter)) < 1e-3, "{:?}", both.bone_quat);
    }

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
