//! Light that changes: the level's torches and fires. The engine bakes most lights into the level's vertex colours, but
//! the ones marked "semi-dynamic" (every torch) are left out of the bake and added each frame, flickering, to the
//! vertices near them (`TreatBackgroundDynlights`, `ApplyTileLights`). Without them the dungeons are far too dark.
//! Each chunk of the level keeps what is needed to do that sum again: positions, normals and the baked colours.

use crate::convert::to_bevy;
use arx_formats::llf::Light;
use bevy::prelude::*;

/// `EERIE_LIGHT::extras` bits.
pub const EXTRAS_SEMIDYNAMIC: u32 = 0x1;
pub const EXTRAS_STARTEXTINGUISHED: u32 = 0x4;
pub const EXTRAS_SPAWNFIRE: u32 = 0x8;
pub const EXTRAS_SPAWNSMOKE: u32 = 0x10;
pub const EXTRAS_OFF: u32 = 0x20;
pub const EXTRAS_COLORLEGACY: u32 = 0x40;
pub const EXTRAS_FIREPLACE: u32 = 0x200;

/// The engine's `GLOBAL_LIGHT_FACTOR`.
const GLOBAL_LIGHT_FACTOR: f32 = 0.85;
/// Chunks farther than this from the camera are left with their baked light (too far to see a flicker).
const LIGHTING_RANGE: f32 = 4500.0;

/// One mesh of the level and how to light it again.
pub struct LitChunk {
    mesh: Handle<Mesh>,
    positions: Vec<Vec3>,
    /// Zero for vertices that take no light (glowing polygons).
    normals: Vec<Vec3>,
    /// Baked colour, 0..255 per channel.
    baked: Vec<[f32; 3]>,
    alphas: Vec<f32>,
    min: Vec3,
    max: Vec3,
    /// Its colours currently differ from the bake.
    changed: bool,
}

impl LitChunk {
    pub fn new(mesh: Handle<Mesh>, positions: Vec<Vec3>, normals: Vec<Vec3>, baked: Vec<[f32; 3]>, alphas: Vec<f32>) -> Self {
        let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for p in &positions {
            min = min.min(*p);
            max = max.max(*p);
        }
        LitChunk { mesh, positions, normals, baked, alphas, min, max, changed: false }
    }

    fn distance_to(&self, p: Vec3) -> f32 {
        (p - p.clamp(self.min, self.max)).length()
    }
}

/// A torch or fire: a light the level adds at run time.
#[derive(Debug, Clone)]
pub struct Torch {
    /// Bevy coordinates.
    pub pos: Vec3,
    pub rgb: Vec3,
    pub flicker: Vec3,
    pub fall_start: f32,
    pub fall_end: f32,
    pub intensity: f32,
    pub extras: u32,
    pub ex_radius: f32,
    pub ex_frequency: f32,
    pub ex_size: f32,
    pub ex_speed: f32,
    pub lit: bool,
    /// The colour this frame, 0..255 (flickering).
    pub now: Vec3,
}

/// The light one torch adds to a vertex, 0..255 per channel (`ApplyTileLights`: Lambert, linear falloff, halved).
pub fn torch_light(t: &Torch, pos: Vec3, normal: Vec3) -> Vec3 {
    let to = t.pos - pos;
    let dist = to.length();
    if dist >= t.fall_end || dist < 1e-3 {
        return Vec3::ZERO;
    }
    let cos = normal.dot(to / dist);
    if cos <= 0.0 {
        return Vec3::ZERO;
    }
    let k = if dist <= t.fall_start { 1.0 } else { (t.fall_end - dist) / (t.fall_end - t.fall_start).max(1e-3) };
    t.now * (cos * k * t.intensity * GLOBAL_LIGHT_FACTOR * 0.5)
}

#[derive(Resource, Default)]
pub struct LevelLighting {
    pub chunks: Vec<LitChunk>,
    pub torches: Vec<Torch>,
    lut: Vec<f32>,
    rng: u32,
    since_flicker: f32,
}

impl LevelLighting {
    pub fn new(chunks: Vec<LitChunk>, lights: &[Light], scene_pos: Vec3) -> Self {
        let torches = lights
            .iter()
            .filter(|l| l.extras & EXTRAS_SEMIDYNAMIC != 0 && l.fall_end > l.fall_start)
            .map(|l| Torch {
                pos: Vec3::from(to_bevy([l.pos[0] + scene_pos.x, l.pos[1] + scene_pos.y, l.pos[2] + scene_pos.z])),
                rgb: Vec3::from(l.rgb),
                flicker: Vec3::from(l.ex_flicker),
                fall_start: l.fall_start,
                fall_end: l.fall_end,
                intensity: l.intensity,
                extras: l.extras,
                ex_radius: l.ex_radius,
                ex_frequency: l.ex_frequency,
                ex_size: l.ex_size,
                ex_speed: l.ex_speed,
                lit: l.extras & (EXTRAS_STARTEXTINGUISHED | EXTRAS_OFF) == 0,
                now: Vec3::from(l.rgb) * 255.0,
            })
            .collect();
        let lut = (0..=255u32)
            .map(|c| {
                let c = c as f32 / 255.0;
                if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
            })
            .collect();
        LevelLighting { chunks, torches, lut, rng: 0x1357_9BDF, since_flicker: 1.0 }
    }

    fn random(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1u32 << 24) as f32
    }

    /// The torches' light at a point, 0..255 per channel, for something facing `normal` (characters and objects take
    /// the whole of it, not the half the level's own vertices get).
    pub fn torches_at(&self, pos: Vec3, normal: Vec3) -> Vec3 {
        self.torches.iter().filter(|t| t.lit).map(|t| torch_light(t, pos, normal) * 2.0).sum()
    }
}

/// Flicker the torches and light the level's chunks near the camera again.
pub fn update(time: Res<Time>, cam: Single<&Transform, With<Camera3d>>, mut lighting: ResMut<LevelLighting>, mut meshes: ResMut<Assets<Mesh>>) {
    let l = &mut *lighting;
    if l.torches.is_empty() || l.chunks.is_empty() {
        return;
    }
    // The engine picks a new flicker every frame; thirty times a second is as lively and costs less.
    l.since_flicker += time.delta_secs();
    if l.since_flicker < 1.0 / 30.0 {
        return;
    }
    l.since_flicker = 0.0;
    for i in 0..l.torches.len() {
        let r = Vec3::new(l.random(), l.random(), l.random());
        let t = &mut l.torches[i];
        t.now = ((t.rgb - t.rgb * t.flicker * r * 0.5).max(Vec3::ZERO)) * 255.0;
    }
    let eye = cam.translation;
    let LevelLighting { chunks, torches, lut, .. } = l;
    for chunk in chunks.iter_mut() {
        let near: Vec<&Torch> = if chunk.distance_to(eye) > LIGHTING_RANGE {
            Vec::new()
        } else {
            torches.iter().filter(|t| t.lit && chunk.distance_to(t.pos) < t.fall_end).collect()
        };
        if near.is_empty() && !chunk.changed {
            continue;
        }
        let Some(mut mesh) = meshes.get_mut(&chunk.mesh) else { continue };
        let colors: Vec<[f32; 4]> = (0..chunk.positions.len())
            .map(|i| {
                let mut c = Vec3::from(chunk.baked[i]);
                if chunk.normals[i] != Vec3::ZERO {
                    for t in &near {
                        c += torch_light(t, chunk.positions[i], chunk.normals[i]);
                    }
                }
                let to_linear = |v: f32| lut[v.clamp(0.0, 255.0) as usize];
                [to_linear(c.x), to_linear(c.y), to_linear(c.z), chunk.alphas[i]]
            })
            .collect();
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
        chunk.changed = !near.is_empty();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn torch() -> Torch {
        Torch {
            pos: Vec3::new(0.0, 100.0, 0.0),
            rgb: Vec3::ONE,
            flicker: Vec3::ZERO,
            fall_start: 100.0,
            fall_end: 300.0,
            intensity: 2.0,
            extras: EXTRAS_SEMIDYNAMIC,
            ex_radius: 0.0,
            ex_frequency: 0.0,
            ex_size: 0.0,
            ex_speed: 0.0,
            lit: true,
            now: Vec3::splat(255.0),
        }
    }

    #[test]
    fn a_torch_lights_what_faces_it_and_fades_out_with_distance() {
        let t = torch();
        // Straight below, facing up, inside the full-strength radius: 255 x intensity x 0.85 x one half.
        let full = torch_light(&t, Vec3::ZERO, Vec3::Y);
        assert!((full.x - 255.0 * 2.0 * 0.85 * 0.5).abs() < 0.01, "{full:?}");
        // Facing away: nothing.
        assert_eq!(torch_light(&t, Vec3::ZERO, Vec3::NEG_Y), Vec3::ZERO);
        // Half-way through the falloff: half of it.
        let half = torch_light(&t, Vec3::new(0.0, -100.0, 0.0), Vec3::Y);
        assert!((half.x - full.x * 0.5).abs() < 0.01, "{half:?}");
        // Beyond its reach: nothing.
        assert_eq!(torch_light(&t, Vec3::new(0.0, -300.0, 0.0), Vec3::Y), Vec3::ZERO);
        // At a slant the cosine counts.
        let slant = torch_light(&t, Vec3::new(60.0, 20.0, 0.0), Vec3::Y);
        assert!((slant.x - full.x * 0.8).abs() < 0.01, "3-4-5 triangle: cos 0.8, {slant:?}");
    }
}
