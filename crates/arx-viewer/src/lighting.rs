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
/// Torches within this distance of the camera flicker; farther ones keep a steady light (too far to see it).
const FLICKER_RANGE: f32 = 2200.0;
/// Level meshes whose colours may be rewritten per tick. Every rewritten mesh is uploaded again, which is what costs,
/// so the nearest torch flickers every tick and the others take turns within this budget.
const CHUNKS_PER_TICK: usize = 48;

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
    /// The torches that reach it: for each, the vertices it lights and how strongly (everything about a torch's light
    /// but its colour is fixed, since neither it nor the level moves).
    lit_by: Vec<(usize, Vec<(u32, f32)>)>,
}

impl LitChunk {
    pub fn new(mesh: Handle<Mesh>, positions: Vec<Vec3>, normals: Vec<Vec3>, baked: Vec<[f32; 3]>, alphas: Vec<f32>) -> Self {
        let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for p in &positions {
            min = min.min(*p);
            max = max.max(*p);
        }
        LitChunk { mesh, positions, normals, baked, alphas, min, max, lit_by: Vec::new() }
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
    /// The level meshes it reaches.
    pub chunks: Vec<usize>,
    /// What `lit` was when its meshes were last coloured.
    was_lit: bool,
}

/// How much of a torch's colour reaches a vertex (`ApplyTileLights`: Lambert, linear falloff, halved).
pub fn torch_factor(t: &Torch, pos: Vec3, normal: Vec3) -> f32 {
    let to = t.pos - pos;
    let dist = to.length();
    if dist >= t.fall_end || dist < 1e-3 {
        return 0.0;
    }
    let cos = normal.dot(to / dist);
    if cos <= 0.0 {
        return 0.0;
    }
    let k = if dist <= t.fall_start { 1.0 } else { (t.fall_end - dist) / (t.fall_end - t.fall_start).max(1e-3) };
    cos * k * t.intensity * GLOBAL_LIGHT_FACTOR * 0.5
}

/// The light one torch adds to a vertex, 0..255 per channel.
pub fn torch_light(t: &Torch, pos: Vec3, normal: Vec3) -> Vec3 {
    t.now * torch_factor(t, pos, normal)
}

/// A torch's colour between flickers, 0..255: the middle of what the flicker takes away.
fn steady(t: &Torch) -> Vec3 {
    (t.rgb - t.rgb * t.flicker * 0.25).max(Vec3::ZERO) * 255.0
}

#[derive(Resource, Default)]
pub struct LevelLighting {
    pub chunks: Vec<LitChunk>,
    pub torches: Vec<Torch>,
    lut: Vec<f32>,
    rng: u32,
    since_flicker: f32,
    /// Meshes whose colours no longer match their torches.
    dirty: Vec<bool>,
    /// Whose turn it is among the torches that share the budget.
    turn: usize,
}

impl LevelLighting {
    pub fn new(mut chunks: Vec<LitChunk>, lights: &[Light], scene_pos: Vec3) -> Self {
        let mut torches: Vec<Torch> = lights
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
                now: Vec3::ZERO,
                chunks: Vec::new(),
                was_lit: false,
            })
            .collect();
        for t in &mut torches {
            t.now = steady(t);
        }
        // Which vertices each torch lights, once.
        for (c, chunk) in chunks.iter_mut().enumerate() {
            for (i, t) in torches.iter_mut().enumerate() {
                if chunk.distance_to(t.pos) >= t.fall_end {
                    continue;
                }
                let reached: Vec<(u32, f32)> = (0..chunk.positions.len())
                    .filter(|&v| chunk.normals[v] != Vec3::ZERO)
                    .map(|v| (v as u32, torch_factor(t, chunk.positions[v], chunk.normals[v])))
                    .filter(|&(_, k)| k > 0.0)
                    .collect();
                if !reached.is_empty() {
                    chunk.lit_by.push((i, reached));
                    t.chunks.push(c);
                }
            }
        }
        let lut = (0..=255u32)
            .map(|c| {
                let c = c as f32 / 255.0;
                if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
            })
            .collect();
        let dirty = vec![false; chunks.len()];
        LevelLighting { chunks, torches, lut, rng: 0x1357_9BDF, since_flicker: 1.0, dirty, turn: 0 }
    }

    fn random(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1u32 << 24) as f32
    }
}

/// Flicker the torches near the camera and colour again the level meshes they reach.
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
    let eye = cam.translation;
    // A torch lit or put out (and every torch the first time) changes its meshes whatever the budget.
    for t in &mut l.torches {
        if t.lit != t.was_lit {
            t.was_lit = t.lit;
            for &c in &t.chunks {
                l.dirty[c] = true;
            }
        }
    }
    // The nearest torch flickers every tick; the others near the camera take turns with what is left of the budget.
    let mut near: Vec<(f32, usize)> =
        l.torches.iter().enumerate().filter(|(_, t)| t.lit && !t.chunks.is_empty()).map(|(i, t)| (t.pos.distance(eye), i)).filter(|&(d, _)| d < FLICKER_RANGE).collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0));
    let others = near.len().saturating_sub(1);
    let order: Vec<usize> = near.first().map(|n| n.1).into_iter().chain((0..others).map(|k| near[1 + (l.turn + k) % others].1)).collect();
    let mut budget = CHUNKS_PER_TICK as isize;
    let mut taken = 0;
    for (n, i) in order.into_iter().enumerate() {
        if budget <= 0 {
            break;
        }
        let r = Vec3::new(l.random(), l.random(), l.random());
        let t = &mut l.torches[i];
        t.now = ((t.rgb - t.rgb * t.flicker * r * 0.5).max(Vec3::ZERO)) * 255.0;
        budget -= t.chunks.len() as isize;
        for &c in &t.chunks {
            l.dirty[c] = true;
        }
        if n > 0 {
            taken += 1;
        }
    }
    if others > 0 {
        l.turn = (l.turn + taken) % others;
    }
    let LevelLighting { chunks, torches, lut, dirty, .. } = l;
    for (chunk, dirty) in chunks.iter().zip(dirty.iter_mut()) {
        if !std::mem::take(dirty) {
            continue;
        }
        let Some(mut mesh) = meshes.get_mut(&chunk.mesh) else { continue };
        let mut colors: Vec<Vec3> = chunk.baked.iter().map(|&c| Vec3::from(c)).collect();
        for (torch, reached) in &chunk.lit_by {
            let t = &torches[*torch];
            if !t.lit {
                continue;
            }
            for &(v, k) in reached {
                colors[v as usize] += t.now * k;
            }
        }
        let to_linear = |v: f32| lut[v.clamp(0.0, 255.0) as usize];
        let colors: Vec<[f32; 4]> = colors.iter().zip(&chunk.alphas).map(|(c, &a)| [to_linear(c.x), to_linear(c.y), to_linear(c.z), a]).collect();
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
        crate::perf::TOUCHED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
            chunks: Vec::new(),
            was_lit: false,
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
