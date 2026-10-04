//! Flames and smoke on the level's torches and fires, as the engine makes them (`TreatBackgroundActions` spawns,
//! `ARX_PARTICLES_Update` moves and fades): small additive sprites that rise, shrink and sometimes turn to smoke.
//! All particles of a texture are drawn as one mesh of camera-facing quads rebuilt every frame.

use crate::convert::{TextureCache, load_texture, to_bevy};
use crate::lighting::{EXTRAS_COLORLEGACY, EXTRAS_FIREPLACE, EXTRAS_SPAWNFIRE, EXTRAS_SPAWNSMOKE, LevelLighting};
use crate::Arx;
use bevy::{asset::RenderAssetUsages, camera::visibility::NoFrustumCulling, mesh::{Indices, PrimitiveTopology}, prelude::*};

/// Fires farther than this from the camera make no particles.
const RANGE: f32 = 3500.0;
const MAX_PARTICLES: usize = 3000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sprite {
    Fire,
    Smoke,
}

struct Particle {
    /// Where it started and how it moves, in Arx coordinates (units per 100 ms, as the engine counts).
    origin: Vec3,
    velocity: Vec3,
    size: f32,
    size_delta: f32,
    duration_ms: f32,
    elapsed_ms: f32,
    gravity: bool,
    fire_to_smoke: bool,
    sprite: Sprite,
    rgb: Vec3,
    /// Degrees per millisecond.
    rotation: f32,
}

#[derive(Resource, Default)]
pub struct Particles {
    list: Vec<Particle>,
    /// Flames owed to each torch (fractions carry over to the next frame).
    owed: Vec<f32>,
    meshes: Option<[Handle<Mesh>; 2]>,
    rng: u32,
    clock_ms: f32,
}

impl Particles {
    fn random(&mut self) -> f32 {
        if self.rng == 0 {
            self.rng = 0x2468_ACE1;
        }
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1u32 << 24) as f32
    }

    fn random_vec(&mut self) -> Vec3 {
        Vec3::new(self.random(), self.random(), self.random())
    }
}

/// The two sprite sheets, each with its own mesh.
pub fn setup(mut commands: Commands, arx: Res<Arx>, mut particles: ResMut<Particles>, mut cache: ResMut<TextureCache>, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>, mut images: ResMut<Assets<Image>>) {
    let mut make = |texture: &str| {
        let tex = load_texture(&arx.0, texture, &mut cache, &mut images).map(|i| i.handle);
        let material = materials.add(StandardMaterial { base_color_texture: tex, unlit: true, alpha_mode: AlphaMode::Add, cull_mode: None, double_sided: true, ..default() });
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, Vec::<[f32; 3]>::new());
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, Vec::<[f32; 2]>::new());
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, Vec::<[f32; 4]>::new());
        mesh.insert_indices(Indices::U32(Vec::new()));
        let handle = meshes.add(mesh);
        commands.spawn((Mesh3d(handle.clone()), MeshMaterial3d(material), NoFrustumCulling, Transform::default()));
        handle
    };
    particles.meshes = Some([make("graph/particles/fire2"), make("graph/particles/smoke")]);
}

/// Spawn flames at the lit fires near the camera, move every particle on, and rebuild the two meshes.
pub fn update(time: Res<Time>, cam: Single<&Transform, With<Camera3d>>, lighting: Res<LevelLighting>, mut particles: ResMut<Particles>, mut meshes: ResMut<Assets<Mesh>>) {
    let dt_ms = time.delta_secs().min(0.1) * 1000.0;
    let p = &mut *particles;
    p.clock_ms += dt_ms;
    let eye = cam.translation;
    if p.owed.len() != lighting.torches.len() {
        p.owed = vec![0.0; lighting.torches.len()];
    }

    // New flames.
    for (i, t) in lighting.torches.iter().enumerate() {
        if !t.lit || t.extras & (EXTRAS_SPAWNFIRE | EXTRAS_SPAWNSMOKE) == 0 {
            continue;
        }
        let dist = t.pos.distance(eye);
        if dist > RANGE {
            continue;
        }
        let amount = if dist < 600.0 { 0.183 } else if dist < 1200.0 { 0.1525 } else { 0.122 };
        p.owed[i] += dt_ms * amount;
        let count = p.owed[i].floor();
        p.owed[i] -= count;
        let fire = t.extras & EXTRAS_SPAWNFIRE != 0;
        let pos = Vec3::new(t.pos.x, -t.pos.y, -t.pos.z); // back to Arx coordinates, where the engine's numbers apply
        let rgb = if t.extras & EXTRAS_COLORLEGACY != 0 { t.rgb } else { Vec3::ONE };
        for _ in 0..count as usize {
            if p.list.len() >= MAX_PARTICLES {
                break;
            }
            if p.random() < t.ex_frequency {
                let a = p.random() * std::f32::consts::PI;
                let s = Vec3::new(a.sin(), a.sin(), a.cos()) * p.random_vec();
                let velocity = (Vec3::splat(2.0) - Vec3::new(4.0, 22.0, 4.0) * p.random_vec()) * t.ex_speed;
                let duration_ms = 500.0 + p.random() * 1000.0 * t.ex_speed;
                let rotation = 0.1 - p.random() * 0.2 * t.ex_speed;
                p.list.push(Particle {
                    origin: pos + s * t.ex_radius,
                    velocity,
                    size: 7.0 * t.ex_size,
                    size_delta: -8.0,
                    duration_ms,
                    elapsed_ms: 0.0,
                    gravity: false,
                    fire_to_smoke: fire && t.extras & EXTRAS_SPAWNSMOKE != 0,
                    sprite: if fire { Sprite::Fire } else { Sprite::Smoke },
                    rgb,
                    rotation,
                });
            }
            if fire && p.random() < t.ex_frequency * 0.05 {
                // A spark that leaps out and falls.
                let a = p.random() * std::f32::consts::TAU - std::f32::consts::PI;
                let s = Vec3::new(a.sin(), a.sin(), a.cos()) * p.random_vec();
                let origin = pos + s * t.ex_radius;
                let out = (origin - pos).normalize_or_zero();
                let d = if t.extras & EXTRAS_FIREPLACE != 0 { 6.0 } else { 4.0 };
                let up = -18.0 + p.random() * 8.0;
                let duration_ms = 1200.0 + p.random() * 500.0 * t.ex_speed;
                let rotation = 0.1 - p.random() * 0.2 * t.ex_speed;
                p.list.push(Particle {
                    origin,
                    velocity: Vec3::new(out.x * d, up, out.z * d) * t.ex_speed,
                    size: 4.0 * t.ex_size * 0.3,
                    size_delta: -3.0,
                    duration_ms,
                    elapsed_ms: 0.0,
                    gravity: true,
                    fire_to_smoke: false,
                    sprite: Sprite::Fire,
                    rgb,
                    rotation,
                });
            }
        }
    }

    // Age them; a flame that burns out may linger as smoke.
    let mut i = 0;
    while i < p.list.len() {
        p.list[i].elapsed_ms += dt_ms;
        if p.list[i].elapsed_ms >= p.list[i].duration_ms {
            if p.list[i].fire_to_smoke && p.random() > 0.7 {
                let q = &mut p.list[i];
                q.origin += q.velocity * (q.duration_ms / 100.0);
                q.duration_ms *= 1.375;
                q.fire_to_smoke = false;
                q.sprite = Sprite::Smoke;
                q.size_delta = (q.size_delta * 2.4).abs();
                q.rgb = Vec3::splat(0.45);
                q.velocity *= 0.5;
                q.size /= 3.0;
                q.elapsed_ms = 0.0;
            } else {
                p.list.swap_remove(i);
                continue;
            }
        }
        i += 1;
    }

    // Draw: quads facing the camera, fading and shrinking with age.
    let Some(handles) = p.meshes.clone() else { return };
    let (right, up) = (cam.right().as_vec3(), cam.up().as_vec3());
    for (which, handle) in [Sprite::Fire, Sprite::Smoke].into_iter().zip(handles) {
        let mut positions: Vec<[f32; 3]> = Vec::new();
        let mut uvs: Vec<[f32; 2]> = Vec::new();
        let mut colors: Vec<[f32; 4]> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        for q in p.list.iter().filter(|q| q.sprite == which) {
            let val = q.elapsed_ms / 100.0;
            let mut at = q.origin + q.velocity * val;
            if q.gravity {
                at.y += 1.47 * val * val;
            }
            let fd = q.elapsed_ms / q.duration_ms;
            let fade = 1.0 - fd;
            let size = (q.size + q.size_delta * fd).max(0.0);
            if fade <= 0.0 || size <= 0.0 {
                continue;
            }
            let centre = Vec3::from(to_bevy(at.to_array()));
            let angle = ((p.clock_ms + q.elapsed_ms) * q.rotation).to_radians();
            let (sin, cos) = angle.sin_cos();
            let (ax, ay) = ((right * cos + up * sin) * size, (up * cos - right * sin) * size);
            let base = positions.len() as u32;
            for (corner, uv) in [(-ax - ay, [0.0, 1.0]), (ax - ay, [1.0, 1.0]), (ax + ay, [1.0, 0.0]), (-ax + ay, [0.0, 0.0])] {
                positions.push((centre + corner).to_array());
                uvs.push(uv);
                colors.push([q.rgb.x * fade, q.rgb.y * fade, q.rgb.z * fade, 1.0]);
            }
            indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        if let Some(mut mesh) = meshes.get_mut(&handle) {
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
            mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
            mesh.insert_indices(Indices::U32(indices));
        }
    }
}
