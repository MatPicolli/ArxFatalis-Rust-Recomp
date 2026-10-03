//! `.tea` — keyframe animations. Each keyframe holds a global translation/rotation plus one
//! transform per vertex group (bone) of the object it animates. Frames are timed at 24 per second.
//!
//! Layout: 292-byte header, then `nb_key_frames` x { keyframe header (288 B, or 32 B before
//! version 2015), optional move (12 B), optional orient (8 + 16 B), optional morph (16 B),
//! `nb_groups` x 52-byte group transforms, sample (i32, optionally 260 B + data), 4 B }.

use crate::reader::{Reader, Truncated};
use glam::{Quat, Vec3};
use thiserror::Error;

pub const FRAMES_PER_SECOND: i64 = 24;

#[derive(Debug, Error)]
pub enum TeaError {
    #[error("file is truncated")]
    Truncated(#[from] Truncated),
    #[error("unsupported version {0}")]
    BadVersion(u32),
    #[error("invalid {0}")]
    Invalid(&'static str),
}

#[derive(Debug, Clone)]
pub struct KeyFrame {
    pub num_frame: i32,
    /// Absolute time of this keyframe in microseconds.
    pub time_us: i64,
    /// Root motion of the whole object (Arx coordinates).
    pub translate: Vec3,
    pub rotate: Quat,
    pub step_sound: bool,
}

/// Local transform of one bone at one keyframe.
#[derive(Debug, Clone, Copy)]
pub struct GroupAnim {
    pub rotate: Quat,
    pub translate: Vec3,
    /// Scale minus one: all zero means "unscaled".
    pub zoom: Vec3,
}

#[derive(Debug, Clone)]
pub struct Tea {
    pub name: String,
    pub frames: Vec<KeyFrame>,
    pub group_count: usize,
    /// `frames.len() * group_count` entries, frame-major.
    pub groups: Vec<GroupAnim>,
    /// Groups that never move in this animation.
    pub void_groups: Vec<bool>,
    pub duration_us: i64,
}

fn quat_wxyz(r: &mut Reader) -> Result<Quat, Truncated> {
    let (w, x, y, z) = (r.f32()?, r.f32()?, r.f32()?, r.f32()?);
    Ok(Quat::from_xyzw(x, y, z, w))
}

/// Sum of keyframe times in `(f1, f2]`, exactly as the original engine computes the weights used
/// to fill keyframes that carry no key of their own.
fn time_between(frames: &[KeyFrame], f1: usize, f2: usize) -> f32 {
    frames[f1 + 1..=f2].iter().map(|f| f.time_us as f32 / 1000.0).sum()
}

impl Tea {
    pub fn parse(data: &[u8]) -> Result<Self, TeaError> {
        let mut r = Reader::new(data);
        r.skip(20)?; // identity
        let version = r.u32()?;
        if version < 2014 {
            return Err(TeaError::BadVersion(version));
        }
        let name = r.string(256)?;
        let nb_frames = r.i32()?;
        let nb_groups = usize::try_from(r.i32()?).map_err(|_| TeaError::Invalid("group count"))?;
        let nb_keys = usize::try_from(r.i32()?).map_err(|_| TeaError::Invalid("keyframe count"))?;
        if nb_keys > 100_000 || nb_groups > 4096 {
            return Err(TeaError::Invalid("animation size"));
        }

        let mut frames = Vec::with_capacity(nb_keys);
        let mut has_translate = Vec::with_capacity(nb_keys);
        let mut has_rotate = Vec::with_capacity(nb_keys);
        let mut groups = Vec::with_capacity(nb_keys * nb_groups);
        for _ in 0..nb_keys {
            let num_frame = r.i32()?;
            let flag_frame = r.i32()?;
            if version >= 2015 {
                r.skip(256)?; // info_frame
            }
            r.skip(4)?; // master_key_frame
            r.skip(4)?; // key_frame
            let key_move = r.i32()? != 0;
            let key_orient = r.i32()? != 0;
            let key_morph = r.i32()? != 0;
            r.skip(4)?; // time_frame

            let mut frame = KeyFrame {
                num_frame,
                time_us: num_frame as i64 * 1_000_000 / FRAMES_PER_SECOND,
                translate: Vec3::ZERO,
                rotate: Quat::IDENTITY,
                step_sound: flag_frame == 9,
            };
            if key_move {
                frame.translate = Vec3::from(r.vec3()?);
            }
            if key_orient {
                r.skip(8)?;
                frame.rotate = quat_wxyz(&mut r)?;
            }
            if key_morph {
                r.skip(16)?;
            }
            for _ in 0..nb_groups {
                r.skip(4 + 8)?; // key_group, angle
                let rotate = quat_wxyz(&mut r)?;
                let translate = Vec3::from(r.vec3()?);
                let zoom = Vec3::from(r.vec3()?);
                groups.push(GroupAnim { rotate, translate, zoom });
            }
            if r.i32()? != -1 {
                r.skip(256)?;
                let size = usize::try_from(r.i32()?).map_err(|_| TeaError::Invalid("sample size"))?;
                r.skip(size)?;
            }
            r.skip(4)?; // num_sfx
            frames.push(frame);
            has_translate.push(key_move);
            has_rotate.push(key_orient);
        }

        // Fill frames without their own root-motion key by interpolating between neighbours.
        for i in 0..nb_keys {
            for (is_translate, has) in [(true, &has_translate), (false, &has_rotate)] {
                if has[i] {
                    continue;
                }
                let Some(k) = (0..=i).rev().find(|&k| has[k]) else { continue };
                let Some(j) = (i..nb_keys).find(|&j| has[j]) else { continue };
                let r1 = time_between(&frames, k, i);
                let r2 = time_between(&frames, i, j);
                let tot = 1.0 / (r1 + r2);
                let (r1, r2) = (r1 * tot, r2 * tot);
                if is_translate {
                    frames[i].translate = frames[j].translate * r1 + frames[k].translate * r2;
                } else {
                    let (a, b) = (frames[j].rotate, frames[k].rotate);
                    frames[i].rotate = Quat::from_xyzw(
                        a.x * r1 + b.x * r2,
                        a.y * r1 + b.y * r2,
                        a.z * r1 + b.z * r2,
                        a.w * r1 + b.w * r2,
                    );
                }
            }
        }

        let void_groups = (0..nb_groups)
            .map(|g| {
                (0..nb_keys).all(|f| {
                    let a = &groups[f * nb_groups + g];
                    a.rotate == Quat::IDENTITY && a.translate == Vec3::ZERO && a.zoom == Vec3::ZERO
                })
            })
            .collect();

        let duration_us = (nb_frames as i64 * 1_000_000 / FRAMES_PER_SECOND).max(1000);
        Ok(Tea { name, frames, group_count: nb_groups, groups, void_groups, duration_us })
    }

    pub fn group(&self, frame: usize, group: usize) -> &GroupAnim {
        &self.groups[frame * self.group_count + group]
    }

    /// Map a time (microseconds, already wrapped into `[0, duration_us]`) to the pair of keyframes to
    /// blend and the blend factor, as `PrepareAnim` does.
    pub fn locate(&self, time_us: i64) -> (usize, f32) {
        let n = self.frames.len();
        if n < 2 {
            return (0, 0.0);
        }
        let result = (n - 2, 1.0);
        for i in 1..n {
            let (tcf, tnf) = (self.frames[i - 1].time_us, self.frames[i].time_us);
            if tcf == tnf {
                return result;
            }
            if (time_us < tnf && time_us >= tcf) || (i == n - 1 && time_us == tnf) {
                return (i - 1, (time_us - tcf) as f32 / (tnf - tcf) as f32);
            }
        }
        result
    }

    /// Time to play at, for a looping animation that has been running for `elapsed_us`.
    pub fn looped_time(&self, elapsed_us: i64) -> i64 {
        elapsed_us.rem_euclid(self.duration_us)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(t: i64) -> KeyFrame {
        KeyFrame { num_frame: 0, time_us: t, translate: Vec3::ZERO, rotate: Quat::IDENTITY, step_sound: false }
    }

    fn anim(times: &[i64]) -> Tea {
        Tea {
            name: String::new(),
            frames: times.iter().map(|&t| frame(t)).collect(),
            group_count: 0,
            groups: vec![],
            void_groups: vec![],
            duration_us: *times.last().unwrap(),
        }
    }

    #[test]
    fn locate_interpolates_between_keyframes() {
        let a = anim(&[0, 1000, 3000]);
        assert_eq!(a.locate(0), (0, 0.0));
        assert_eq!(a.locate(500), (0, 0.5));
        assert_eq!(a.locate(1000), (1, 0.0));
        assert_eq!(a.locate(2000), (1, 0.5));
        // The very last instant belongs to the last segment.
        assert_eq!(a.locate(3000), (1, 1.0));
    }

    #[test]
    fn looping_wraps_time() {
        let a = anim(&[0, 1000, 3000]);
        assert_eq!(a.looped_time(3500), 500);
        assert_eq!(a.looped_time(0), 0);
    }

    #[test]
    fn rejects_old_versions_and_garbage() {
        assert!(Tea::parse(&[0u8; 10]).is_err());
        let mut hdr = vec![0u8; 292];
        hdr[20..24].copy_from_slice(&2000u32.to_le_bytes());
        assert!(matches!(Tea::parse(&hdr), Err(TeaError::BadVersion(2000))));
    }
}
