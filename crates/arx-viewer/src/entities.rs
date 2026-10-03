//! Places a level's entities (items, NPCs, fixtures from the `.dlf`) as static models, lit
//! per-vertex the way the original engine does: an ambient term plus Lambert contributions from
//! the nearest static lights.

use crate::animated::{Animated, MeshSrc, no_cull};
use crate::anims;
use crate::convert::{TextureCache, load_texture, to_bevy};
use crate::level::{Kind, trans_kind};
use arx_formats::{PakSet, dlf::Dlf, ftl::Ftl, llf::Light, poly, skeleton::Skeleton, tea::Tea};
use bevy::{asset::RenderAssetUsages, mesh::PrimitiveTopology, prelude::*, render::render_resource::Face};
use std::{collections::HashMap, sync::Arc};

/// `EERIE_LIGHT::extras` bits that matter for lighting.
const EXTRAS_SEMIDYNAMIC: u32 = 1;
const EXTRAS_STARTEXTINGUISHED: u32 = 4;

/// Original engine constants (`scene/Light.cpp`).
const GLOBAL_LIGHT_FACTOR: f32 = 0.85;
const DEFAULT_AMBIENT_255: f32 = 0.09 * 255.0;
const NPC_ITEMS_AMBIENT_255: f32 = 35.0;
const MAX_LLIGHTS: usize = 10;

pub struct StaticLight {
    pos: Vec3,
    rgb255: Vec3,
    fall_start: f32,
    fall_end: f32,
    intensity: f32,
}

impl StaticLight {
    /// Lights that illuminate objects: ignited and not semi-dynamic (those become dynamic lights).
    pub fn from_level(lights: &[Light], scene_pos: Vec3) -> Vec<StaticLight> {
        lights
            .iter()
            .filter(|l| l.extras & (EXTRAS_SEMIDYNAMIC | EXTRAS_STARTEXTINGUISHED) == 0)
            .filter(|l| l.fall_end > l.fall_start)
            .map(|l| StaticLight {
                pos: Vec3::from(to_bevy([l.pos[0] + scene_pos.x, l.pos[1] + scene_pos.y, l.pos[2] + scene_pos.z])),
                rgb255: Vec3::from(l.rgb) * 255.0,
                fall_start: l.fall_start,
                fall_end: l.fall_end,
                intensity: l.intensity,
            })
            .collect()
    }
}

/// The `MAX_LLIGHTS` lights closest to `pos` (by distance beyond their falloff end).
fn select_lights(all: &[StaticLight], pos: Vec3) -> Vec<&StaticLight> {
    let mut v: Vec<(f32, &StaticLight)> = all
        .iter()
        .filter_map(|l| {
            let d = l.pos.distance(pos);
            (d <= l.fall_end + 560.0).then(|| ((d - l.fall_end).max(0.0), l))
        })
        .collect();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v.into_iter().take(MAX_LLIGHTS).map(|(_, l)| l).collect()
}

/// Lit colour in 0..255 per channel (not yet clamped).
fn light_vertex(lights: &[&StaticLight], ambient: f32, pos: Vec3, normal: Vec3) -> Vec3 {
    let mut c = Vec3::splat(ambient);
    for l in lights {
        let to_light = l.pos - pos;
        let dist = to_light.length();
        let cos = normal.dot(to_light / dist.max(1e-4));
        if cos <= 0.0 {
            continue;
        }
        let k = if dist <= l.fall_start {
            cos * l.intensity * GLOBAL_LIGHT_FACTOR
        } else {
            let p = (l.fall_end - dist) / (l.fall_end - l.fall_start);
            if p <= 0.0 { 0.0 } else { cos * p * l.intensity * GLOBAL_LIGHT_FACTOR }
        };
        c += l.rgb255 * k;
    }
    c
}

fn srgb_to_linear(c: f32) -> f32 {
    let c = (c / 255.0).clamp(0.0, 1.0);
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Rotation of an entity: Arx applies `Rz(-roll) * Rx(pitch) * Ry(yaw)` in its own axes; the
/// Arx->Bevy axis flip (diag(1,-1,-1)) turns that into `Rz(roll) * Rx(pitch) * Ry(-yaw)`.
fn entity_rotation(angle: [f32; 3]) -> Quat {
    let [pitch, yaw, roll] = angle.map(f32::to_radians);
    Quat::from_rotation_z(roll) * Quat::from_rotation_x(pitch) * Quat::from_rotation_y(-yaw)
}

/// Editor-only helper objects that are invisible in the game.
fn is_hidden(class: &str) -> bool {
    class.contains("/system/") || class.contains("marker")
}

#[derive(Resource, Default)]
pub struct EntityCache {
    models: HashMap<String, Option<Arc<Ftl>>>,
    materials: HashMap<(String, Kind, bool), Option<Handle<StandardMaterial>>>,
    skeletons: HashMap<String, Arc<Skeleton>>,
    anims: HashMap<String, Option<Arc<Tea>>>,
}

#[derive(Default, Debug)]
pub struct EntityStats {
    pub spawned: usize,
    pub hidden: usize,
    pub no_model: usize,
    pub meshes: usize,
    pub animated: usize,
}

#[derive(Default)]
struct Builder {
    src: Vec<u32>,
    positions: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    colors: Vec<[f32; 4]>,
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_entities(
    commands: &mut Commands,
    pak: &PakSet,
    dlf: &Dlf,
    scene_pos: Vec3,
    lights: &[StaticLight],
    include_npcs: bool,
    ecache: &mut EntityCache,
    tcache: &mut TextureCache,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) -> EntityStats {
    let mut stats = EntityStats::default();
    for e in &dlf.entities {
        if is_hidden(&e.class) || (!include_npcs && e.class.contains("/npc/")) {
            stats.hidden += 1;
            continue;
        }
        let model_path = format!("game/{}.ftl", e.class);
        let model = ecache
            .models
            .entry(model_path.clone())
            .or_insert_with(|| {
                let bytes = pak.read(&model_path).ok()?;
                Ftl::parse(&bytes).map_err(|err| eprintln!("{model_path}: {err}")).ok().map(Arc::new)
            })
            .clone();
        let Some(ftl) = model else {
            stats.no_model += 1;
            continue;
        };

        let world_arx = [e.pos[0] + scene_pos.x, e.pos[1] + scene_pos.y, e.pos[2] + scene_pos.z];
        let translation = Vec3::from(to_bevy(world_arx));
        let rotation = entity_rotation(e.angle);
        let ambient = if e.class.contains("/npc/") || e.class.contains("/items/") {
            NPC_ITEMS_AMBIENT_255
        } else {
            DEFAULT_AMBIENT_255
        };
        let near = select_lights(lights, translation);

        let mut groups: HashMap<(Option<u16>, Kind, bool), Builder> = HashMap::new();
        for f in &ftl.faces {
            let material = if f.facetype == 0 { None } else { f.material };
            let ftype = f.facetype as u32;
            let kind = if ftype & poly::TRANS != 0 {
                match trans_kind(f.transval) {
                    Some(k) => k,
                    None => continue,
                }
            } else {
                Kind::Opaque
            };
            let alpha = if kind == Kind::Blend { f.transval } else { 1.0 };
            let b = groups.entry((material, kind, ftype & poly::DOUBLESIDED != 0)).or_default();
            let local = f.vid.map(|i| Vec3::from(to_bevy(ftl.vertices[i as usize].pos)));
            let flat = (local[1] - local[0]).cross(local[2] - local[0]).normalize_or_zero();
            for k in 0..3 {
                let v = &ftl.vertices[f.vid[k] as usize];
                let stored = Vec3::from(to_bevy(v.norm));
                let n_local = if stored.length_squared() > 0.25 { stored.normalize() } else { flat };
                let world_pos = translation + rotation * local[k];
                let lit = light_vertex(&near, ambient, world_pos, rotation * n_local);
                b.src.push(f.vid[k] as u32);
                b.positions.push(local[k].to_array());
                b.uvs.push([f.u[k], f.v[k]]);
                b.colors.push([
                    srgb_to_linear(lit.x),
                    srgb_to_linear(lit.y),
                    srgb_to_linear(lit.z),
                    alpha,
                ]);
            }
        }

        // NPCs idle with the animation their script registers as WAIT.
        let anim = (e.class.contains("/npc/"))
            .then(|| anims::script_anim(pak, &e.class, e.instance, "wait"))
            .flatten()
            .and_then(|path| {
                ecache.anims.entry(path.clone()).or_insert_with(|| anims::load_anim(pak, &path)).clone()
            });
        let skeleton = anim.as_ref().and_then(|a| {
            let sk = ecache.skeletons.entry(model_path.clone()).or_insert_with(|| Arc::new(Skeleton::from_ftl(&ftl)));
            (sk.bones.len() == a.group_count).then(|| sk.clone())
        });
        let animated = skeleton.is_some();
        let mut mesh_srcs = Vec::new();
        let mut parent_children = Vec::new();
        for ((material, kind, double), b) in groups {
            let tex_name = material.and_then(|m| ftl.textures.get(m as usize)).filter(|t| !t.is_empty());
            let key = (tex_name.cloned().unwrap_or_default(), kind, double);
            let mat = ecache
                .materials
                .entry(key)
                .or_insert_with(|| {
                    let info = tex_name.and_then(|t| load_texture(pak, t, tcache, images));
                    let keyed = info.as_ref().is_some_and(|i| i.keyed);
                    let alpha_mode = match kind {
                        Kind::Opaque if keyed => AlphaMode::Mask(0.5),
                        Kind::Opaque => AlphaMode::Opaque,
                        Kind::Blend => AlphaMode::Blend,
                        Kind::Add => AlphaMode::Add,
                        Kind::Multiply => AlphaMode::Multiply,
                    };
                    Some(materials.add(StandardMaterial {
                        base_color_texture: info.map(|i| i.handle),
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
            stats.meshes += 1;
            let handle = meshes.add(mesh);
            if animated {
                mesh_srcs.push(MeshSrc { handle: handle.clone(), src: b.src });
            }
            parent_children.push((Mesh3d(handle), MeshMaterial3d(mat)));
        }
        let mut parent = commands.spawn((Transform { translation, rotation, scale: Vec3::ONE }, Visibility::default()));
        parent.with_children(|p| {
            for bundle in parent_children {
                if animated {
                    p.spawn((bundle, no_cull()));
                } else {
                    p.spawn(bundle);
                }
            }
        });
        if let (Some(skeleton), Some(anim)) = (skeleton, anim) {
            // Desynchronise identical NPCs.
            let offset = (e.instance as i64).wrapping_mul(7_919_000).rem_euclid(anim.duration_us);
            parent.insert(Animated { skeleton, anim: Some(anim), meshes: mesh_srcs, elapsed_us: offset });
            stats.animated += 1;
        }
        stats.spawned += 1;
    }
    stats
}
