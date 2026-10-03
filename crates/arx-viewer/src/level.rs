//! Builds Bevy meshes for a level's static geometry (`fast.fts`) with its baked vertex lighting.

use crate::convert::{TextureCache, load_texture, to_bevy};
use arx_formats::{PakSet, fts::{Fts, PolyVertex}, poly};
use bevy::{asset::RenderAssetUsages, mesh::PrimitiveTopology, prelude::*, render::render_resource::Face};
use std::collections::HashMap;

/// Tiles per chunk edge. Chunks are the unit of frustum culling.
const CHUNK_TILES: u16 = 8;

pub struct LevelInfo {
    /// Player start in Bevy coordinates (the `.dlf` editor position, which is where a new game starts).
    pub player_pos: Vec3,
    /// Scene origin offset in Arx coordinates; entity and light positions are relative to it.
    pub scene_pos: Vec3,
    pub poly_count: usize,
    pub mesh_count: usize,
    pub collision: std::sync::Arc<arx_physics::CollisionWorld>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    Opaque,
    Blend,
    Add,
    Multiply,
}

/// Blend mode of a translucent polygon from its `transval`; `None` = subtractive (unsupported).
pub fn trans_kind(transval: f32) -> Option<Kind> {
    match transval {
        t if t >= 2.0 => Some(Kind::Multiply),
        t if t >= 1.0 => Some(Kind::Add),
        t if t > 0.0 => Some(Kind::Blend),
        _ => None,
    }
}

#[derive(Default)]
struct Builder {
    positions: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    colors: Vec<[f32; 4]>,
}

fn srgb_to_linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Triangle with winding chosen so that its geometric normal agrees with the polygon's stored
/// normal (the original engine culls by comparing that normal against the view direction).
fn push_tri(b: &mut Builder, v: [&PolyVertex; 3], col: [[f32; 4]; 3], norm: Vec3) {
    let p = v.map(|v| Vec3::from(to_bevy(v.pos)));
    let geo = (p[1] - p[0]).cross(p[2] - p[0]);
    let order = if norm != Vec3::ZERO && geo.dot(norm) < 0.0 { [0, 2, 1] } else { [0, 1, 2] };
    for i in order {
        b.positions.push(p[i].to_array());
        b.uvs.push(v[i].uv);
        b.colors.push(col[i]);
    }
}

pub fn spawn_level(
    commands: &mut Commands,
    pak: &PakSet,
    level: u32,
    cache: &mut TextureCache,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) -> Result<LevelInfo, String> {
    let path = format!("game/graph/levels/level{level}/fast.fts");
    let fts = pak
        .read(&path)
        .map_err(|e| e.to_string())
        .and_then(|b| Fts::parse(&b).map_err(|e| format!("{path}: {e}")))?;
    let llf = pak.load_llf(level);
    if llf.is_none() {
        eprintln!("level {level}: no baked lighting, using flat grey");
    }

    let lut: Vec<f32> = (0..=255).map(srgb_to_linear).collect();
    let mut chunks: HashMap<(i32, Kind, bool, u16, u16), Builder> = HashMap::new();
    let mut ci = 0usize; // index of the next baked vertex colour; advances for every polygon

    for p in &fts.polys {
        let n = p.vertex_count();
        let base = ci;
        ci += n;

        if p.tex == 0 || p.flags & (poly::IGNORE | poly::NODRAW | poly::HIDE) != 0 {
            continue;
        }
        let kind = if p.flags & poly::TRANS != 0 {
            match trans_kind(p.transval) {
                Some(k) => k,
                None => continue, // subtractive blending has no Bevy equivalent yet
            }
        } else {
            Kind::Opaque
        };
        let alpha = if kind == Kind::Blend { p.transval } else { 1.0 };

        let mut col = [[0.7, 0.7, 0.7, alpha]; 4];
        for (k, c) in col.iter_mut().enumerate().take(n) {
            if p.flags & poly::GLOW != 0 {
                *c = [1.0, 1.0, 1.0, alpha];
            } else if let Some(rgb) = llf.as_ref().and_then(|l| l.colors.get(base + k)) {
                *c = [lut[rgb[0] as usize], lut[rgb[1] as usize], lut[rgb[2] as usize], alpha];
            }
        }

        let double = p.flags & poly::DOUBLESIDED != 0;
        let key = (p.tex, kind, double, p.tile.0 / CHUNK_TILES, p.tile.1 / CHUNK_TILES);
        let b = chunks.entry(key).or_default();
        let v = &p.verts;
        push_tri(b, [&v[0], &v[1], &v[2]], [col[0], col[1], col[2]], Vec3::from(to_bevy(p.norm)));
        if n == 4 {
            push_tri(b, [&v[3], &v[2], &v[1]], [col[3], col[2], col[1]], Vec3::from(to_bevy(p.norm2)));
        }
    }

    let mut mat_cache: HashMap<(i32, Kind, bool), Option<Handle<StandardMaterial>>> = HashMap::new();
    let mut mesh_count = 0;
    for ((tex, kind, double, _, _), b) in chunks {
        let mat = mat_cache
            .entry((tex, kind, double))
            .or_insert_with(|| {
                let name = fts.textures.get(&tex)?;
                let info = load_texture(pak, name, cache, images)?;
                let alpha_mode = match kind {
                    Kind::Opaque if info.keyed => AlphaMode::Mask(0.5),
                    Kind::Opaque => AlphaMode::Opaque,
                    Kind::Blend => AlphaMode::Blend,
                    Kind::Add => AlphaMode::Add,
                    Kind::Multiply => AlphaMode::Multiply,
                };
                Some(materials.add(StandardMaterial {
                    base_color_texture: Some(info.handle),
                    unlit: true,
                    alpha_mode,
                    cull_mode: if double { None } else { Some(Face::Back) },
                    double_sided: double,
                    ..default()
                }))
            })
            .clone();
        let Some(mat) = mat else { continue };

        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, b.positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, b.uvs);
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, b.colors);
        commands.spawn((Mesh3d(meshes.add(mesh)), MeshMaterial3d(mat)));
        mesh_count += 1;
    }

    Ok(LevelInfo {
        player_pos: Vec3::from(to_bevy(fts.player_pos)),
        scene_pos: Vec3::from(fts.scene_pos),
        poly_count: fts.polys.len(),
        mesh_count,
        collision: std::sync::Arc::new(arx_physics::CollisionWorld::from_fts(&fts)),
    })
}
