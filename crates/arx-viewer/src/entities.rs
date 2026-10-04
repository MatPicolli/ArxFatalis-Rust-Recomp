//! Places a level's entities (items, NPCs, fixtures from the `.dlf`) as static models, lit
//! per-vertex the way the original engine does: an ambient term plus Lambert contributions from
//! the nearest static lights.

use crate::animated::{Animated, MeshSrc, no_cull};
use crate::scripting::{BaseAngle, Pickable, ScriptRef, Scripting};
use crate::convert::{TextureCache, load_texture, to_bevy};
use arx_level::entity_rotation;
use crate::level::{Kind, trans_kind};
use arx_formats::{PakSet, dlf::Dlf, ftl::Ftl, llf::Light, poly, skeleton::Skeleton};
use bevy::{asset::RenderAssetUsages, mesh::PrimitiveTopology, prelude::*, render::render_resource::Face};
use std::{collections::HashMap, sync::Arc};

/// `EERIE_LIGHT::extras` bits that matter for lighting.

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
    /// Lights that illuminate objects: every one that is lit, the torches too (the engine adds those at run time).
    pub fn from_level(lights: &[Light], scene_pos: Vec3) -> Vec<StaticLight> {
        lights
            .iter()
            .filter(|l| l.extras & (EXTRAS_STARTEXTINGUISHED | crate::lighting::EXTRAS_OFF) == 0)
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

/// How brightly the level's lights light a spot, 0..255 (the strongest colour channel): what hides the hero in the
/// dark from characters that look for them.
pub fn light_at(lights: &[StaticLight], pos: Vec3) -> f32 {
    let near = select_lights(lights, pos);
    // Light falls on a spot from every side: sum the lights with no surface to face (cos = 1).
    let mut c = Vec3::splat(DEFAULT_AMBIENT_255);
    for l in &near {
        let d = l.pos.distance(pos);
        let k = if d <= l.fall_start { l.intensity * GLOBAL_LIGHT_FACTOR } else { ((l.fall_end - d) / (l.fall_end - l.fall_start)).max(0.0) * l.intensity * GLOBAL_LIGHT_FACTOR };
        c += l.rgb255 * k;
    }
    c.max_element().min(255.0)
}

fn srgb_to_linear(c: f32) -> f32 {
    let c = (c / 255.0).clamp(0.0, 1.0);
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
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

/// The entities that have a model in the scene (the rest are, for now, in inventories or chests).
#[derive(Resource, Default)]
pub struct SpawnedEntities(pub std::collections::HashSet<arx_script::EntityId>);

/// The level's lights, for lighting what is spawned later (dropped items).
#[derive(Resource, Default)]
pub struct LevelLights(pub Vec<StaticLight>);

/// How [`spawn_entity`] should treat the entity.
#[derive(Default)]
pub struct SpawnOpts {
    /// Leave out the faces that touch this vertex selection (the hero's head and shoulders, which the camera is in).
    pub hide_selection: Option<&'static str>,
    /// Do not make it clickable.
    pub not_pickable: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_entities(
    commands: &mut Commands,
    pak: &PakSet,
    dlf: &Dlf,
    scene_pos: Vec3,
    lights: &[StaticLight],
    include_npcs: bool,
    scripting: &mut Scripting,
    pickables: &mut Vec<Pickable>,
    spawned: &mut SpawnedEntities,
    ecache: &mut EntityCache,
    tcache: &mut TextureCache,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) -> EntityStats {
    let mut stats = EntityStats::default();
    for (index, e) in dlf.entities.iter().enumerate() {
        let script_id = scripting.ids[index];
        let world_arx = [e.pos[0] + scene_pos.x, e.pos[1] + scene_pos.y, e.pos[2] + scene_pos.z];
        let (class, angle, instance) = (e.class.as_str(), e.angle, e.instance);
        if spawn_entity(
            commands, pak, lights, scripting, pickables, ecache, tcache, meshes, materials, images,
            class, world_arx, angle, instance, script_id, include_npcs, &mut stats, &SpawnOpts::default(),
        )
        .is_some()
        {
            spawned.0.insert(script_id);
        }
    }
    stats
}

/// Give a model to items the player put back into the world that have none yet (things that came out of chests),
/// so they can be seen and picked up again. Items that were in the level already just move (see `apply_state`).
#[allow(clippy::too_many_arguments)]
pub fn spawn_dropped(
    mut commands: Commands,
    arx: Res<crate::Arx>,
    lights: Res<LevelLights>,
    mut scripting: ResMut<Scripting>,
    mut pickables: ResMut<crate::scripting::Pickables>,
    mut spawned: ResMut<SpawnedEntities>,
    mut ecache: ResMut<EntityCache>,
    mut tcache: ResMut<TextureCache>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    for id in scripting.host.take_dropped() {
        if spawned.0.contains(&id) {
            continue;
        }
        let Some(at) = scripting.host.state(id).and_then(|st| st.moved_to) else { continue };
        let (class, instance) = {
            let e = scripting.world.entity(id);
            (e.class.clone(), e.instance)
        };
        let mut stats = EntityStats::default();
        if spawn_entity(
            &mut commands, &arx.0, &lights.0, &mut scripting, &mut pickables.0, &mut ecache, &mut tcache,
            &mut meshes, &mut materials, &mut images, &class, at, [0.0; 3], instance, id, true, &mut stats,
            &SpawnOpts::default(),
        )
        .is_some()
        {
            spawned.0.insert(id);
        }
    }
}

/// Put one entity's model in the scene (its mesh, lit per vertex, animated if its scripts loaded animations) and make
/// it pickable. `false` if it is hidden, destroyed or has no model.
#[allow(clippy::too_many_arguments)]
pub fn spawn_entity(
    commands: &mut Commands,
    pak: &PakSet,
    lights: &[StaticLight],
    scripting: &mut Scripting,
    pickables: &mut Vec<Pickable>,
    ecache: &mut EntityCache,
    tcache: &mut TextureCache,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    class: &str,
    world_arx: [f32; 3],
    angle: [f32; 3],
    instance: i32,
    script_id: arx_script::EntityId,
    include_npcs: bool,
    stats: &mut EntityStats,
    opts: &SpawnOpts,
) -> Option<Entity> {
    let fullbright = std::env::var_os("ARX_FULLBRIGHT").is_some();
    // What the entity's scripts did to it during start-up.
    let st = scripting.host.state(script_id).cloned().unwrap_or_default();
    if is_hidden(class) || st.destroyed || (!include_npcs && class.contains("/npc/")) {
        stats.hidden += 1;
        return None;
    }
    let model_class = st.mesh.as_deref().unwrap_or(class);
    let model_path = format!("game/{model_class}.ftl");
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
        return None;
    };
    // Vertices of the selection to leave out (the faces that touch any of them are not drawn).
    let hidden_vertices: std::collections::HashSet<u32> = opts
        .hide_selection
        .and_then(|name| ftl.selections.iter().find(|s| s.name.eq_ignore_ascii_case(name)))
        .map(|s| s.vertices.iter().copied().collect())
        .unwrap_or_default();

    let scale = st.scale;
    let translation = Vec3::from(to_bevy(world_arx));
    let rotation = entity_rotation(angle, class.contains("/npc/"));
    let ambient = if class.contains("/npc/") || class.contains("/items/") {
        NPC_ITEMS_AMBIENT_255
    } else {
        DEFAULT_AMBIENT_255
    };
    let near = select_lights(lights, translation);

    let mut groups: HashMap<(Option<u16>, Kind, bool), Builder> = HashMap::new();
    for f in &ftl.faces {
        if f.vid.iter().any(|v| hidden_vertices.contains(&u32::from(*v))) {
            continue;
        }
        // The stumps of limbs that can be cut off (`npc_gore`) stay hidden until they are (`ARX_INTERACTIVE_HideGore`).
        if f.material.and_then(|m| ftl.textures.get(m as usize)).is_some_and(|t| t.to_ascii_lowercase().contains("gore")) {
            continue;
        }
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
            let world_pos = translation + rotation * (local[k] * scale);
            let lit = if fullbright { Vec3::splat(255.0) } else { light_vertex(&near, ambient, world_pos, rotation * n_local) };
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

    // The animation to start with: whatever the scripts are playing, else NPCs idle (WAIT).
    let is_npc = class.contains("/npc/");
    let playing_path = st.playing.as_ref().and_then(|p| st.anims.get(&p.slot)).cloned();
    let anim_path = playing_path.or_else(|| is_npc.then(|| st.anims.get("wait").cloned()).flatten());
    let looping = st.playing.as_ref().is_none_or(|p| p.looping);
    let anim = anim_path.and_then(|p| scripting.anim(pak, &p));
    // Entities with any loaded animation get a skeleton so later `playanim` can move them. Like the engine, an
    // animation that drives more or fewer groups than the model has bones still plays on the ones they share (a
    // lever's two animations need not agree, and checking only one of them used to leave some levers frozen).
    let skeleton = (anim.is_some() || !st.anims.is_empty())
        .then(|| ecache.skeletons.entry(model_path.clone()).or_insert_with(|| Arc::new(Skeleton::from_ftl(&ftl))).clone());
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
    let visibility = if st.hidden { Visibility::Hidden } else { Visibility::Inherited };
    let mut parent = commands.spawn((
        Transform { translation, rotation, scale: Vec3::splat(scale) },
        visibility,
        ScriptRef(script_id),
        BaseAngle { angle: angle, npc: class.contains("/npc/") },
    ));
    parent.with_children(|p| {
        for bundle in parent_children {
            if animated {
                p.spawn((bundle, no_cull()));
            } else {
                p.spawn(bundle);
            }
        }
    });
    if let Some(skeleton) = skeleton {
        // Desynchronise identical NPCs.
        let offset = anim.as_ref().map_or(0, |a| (instance as i64).wrapping_mul(7_919_000).rem_euclid(a.duration_us));
        parent.insert(Animated {
            skeleton,
            anim,
            meshes: mesh_srcs,
            elapsed_us: offset,
            looping,
            root_motion: !class.contains("/npc/"),
            overlay: None,
            keep_pose: false,
            pose: None,
        });
        stats.animated += 1;
    }

    // Bounding sphere for picking.
    if !opts.not_pickable {
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for v in &ftl.vertices {
            let p = Vec3::from(to_bevy(v.pos));
            lo = lo.min(p);
            hi = hi.max(p);
        }
        pickables.push(Pickable {
            id: script_id,
            offset: rotation * ((lo + hi) * 0.5 * scale),
            radius: ((hi - lo).length() * 0.5 * scale).max(20.0),
        });
    }
    stats.spawned += 1;
    Some(parent.id())
}
