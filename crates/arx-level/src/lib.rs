//! Glue between the file formats, the script interpreter and the collision world: everything needed
//! to bring a level's entities to life that does not depend on a renderer.

pub mod anchors;
pub mod inventory;
pub mod npc;
pub mod player_combat;

use arx_formats::{PakSet, dlf::Dlf, ftl::Ftl};
use arx_physics::{CollisionWorld, CylinderId, ObstacleId};
use arx_script::{EntityId, EntityKind, Script, ScriptWorld, StdHost};
use glam::{Quat, Vec3};
use std::collections::HashMap;
use std::sync::Arc;

/// Arx (+Y down, +Z forward) to y-up, -Z forward: a rotation by 180 degrees about X.
pub fn to_yup(p: [f32; 3]) -> Vec3 {
    Vec3::new(p[0], -p[1], -p[2])
}

/// Rotation of an entity in y-up space, from its `(pitch, yaw, roll)` angles in degrees.
///
/// The engine does not use the stored yaw directly: before drawing it remaps it to `270 - yaw` for
/// objects and `180 - yaw` for NPCs (`UpdateInter` / `RenderInter`), then builds a quaternion in Arx
/// axes, which differ by kind:
/// - objects: `Rz(-roll) * Rx(pitch) * Ry(yaw')`;
/// - NPCs: the engine's `QuatFromAngles(pitch, yaw', roll)`, which maps pitch to the Z axis and roll
///   to the X axis (NPCs normally only have a yaw).
///
/// The result is converted from Arx axes to y-up by conjugating with the 180 degree flip about X.
pub fn entity_rotation(angle: [f32; 3], is_npc: bool) -> Quat {
    let [pitch, yaw, roll] = angle;
    if is_npc {
        let (a, b, c) = ((pitch * 0.5).to_radians(), ((180.0 - yaw) * 0.5).to_radians(), (roll * 0.5).to_radians());
        let (sin_pitch, cos_pitch) = a.sin_cos();
        let (sin_yaw, cos_yaw) = b.sin_cos();
        let (sin_roll, cos_roll) = c.sin_cos();
        // `QuatFromAngles` in the original source (its local names are historical).
        let x = sin_roll * cos_yaw * cos_pitch - cos_roll * sin_yaw * sin_pitch;
        let y = cos_roll * sin_yaw * cos_pitch + sin_roll * cos_yaw * sin_pitch;
        let z = cos_roll * cos_yaw * sin_pitch - sin_roll * sin_yaw * cos_pitch;
        let w = cos_roll * cos_yaw * cos_pitch + sin_roll * sin_yaw * sin_pitch;
        // Arx axes -> y-up: negate the Y and Z components of the rotation axis.
        return Quat::from_xyzw(x, -y, -z, w).normalize();
    }
    let yaw = 270.0 - yaw;
    // Rz(-roll) * Rx(pitch) * Ry(yaw) in Arx axes becomes Rz(roll) * Rx(pitch) * Ry(-yaw) in y-up.
    Quat::from_rotation_z(roll.to_radians()) * Quat::from_rotation_x(pitch.to_radians()) * Quat::from_rotation_y(-yaw.to_radians())
}

/// The script world of a level after its start-up sequence has run.
pub struct Scripts {
    pub world: ScriptWorld,
    pub host: StdHost,
    /// Script entity of each `.dlf` entity, in file order.
    pub ids: Vec<EntityId>,
    pub player: EntityId,
}

impl Scripts {
    /// Load every entity's scripts, then run the level start-up sequence: `load`, `init` and
    /// `initend` for each entity, then `game_ready` for all. `scene_pos` is the scene origin offset
    /// (Arx coordinates) that entity positions are relative to.
    pub fn build(pak: &Arc<PakSet>, dlf: &Dlf, scene_pos: Vec3) -> Scripts {
        let mut world = ScriptWorld::new();
        let mut host = StdHost::new();
        {
            // Animation length in ms, straight from the header (frame count at 24 fps).
            let pak = pak.clone();
            host.set_anim_duration(Box::new(move |path| {
                let bytes = pak.read(path).ok()?;
                let frames = i32::from_le_bytes(bytes.get(280..284)?.try_into().ok()?);
                Some(frames as f64 * 1000.0 / 24.0)
            }));
        }
        {
            let pak = pak.clone();
            host.set_icon_size(Box::new(move |class| item_icon_size(&pak, class)));
        }
        {
            // Item scripts, for `inventory add`.
            let pak = pak.clone();
            host.set_script_loader(Box::new(move |class| pak.read(&format!("{class}.asl")).ok().map(|b| Arc::new(Script::new(&b)))));
        }
        let load = |path: &str| pak.read(path).ok().map(|b| Arc::new(Script::new(&b)));
        // The hero has a script too: the animations its body plays, what it says when hurt, and so on.
        let player_script = load("graph/obj3d/interactive/player/player.asl");
        let player = world.add_entity(EntityKind::Player, "graph/obj3d/interactive/player/player", 1, player_script, None);
        let mut ids = Vec::with_capacity(dlf.entities.len());
        for e in &dlf.entities {
            let (dir, name) = e.class.rsplit_once('/').unwrap_or(("", &e.class));
            let class_script = load(&format!("{}.asl", e.class));
            let over_script = load(&format!("{dir}/{name}_{:04}/{name}.asl", e.instance));
            let id = world.add_entity(EntityKind::from_class(&e.class), &e.class, e.instance, class_script, over_script);
            world.entity_mut(id).pos = [e.pos[0] + scene_pos.x, e.pos[1] + scene_pos.y, e.pos[2] + scene_pos.z];
            ids.push(id);
        }

        host.publish_player(&mut world);
        let everyone: Vec<EntityId> = std::iter::once(player).chain(ids.iter().copied()).collect();
        for &id in &everyone {
            world.send_event(&mut host, None, id, "load", Vec::new());
        }
        for &id in &everyone {
            world.send_init(&mut host, id);
        }
        for &id in &everyone {
            world.send_event(&mut host, None, id, "game_ready", Vec::new());
        }
        world.update(&mut host, 0.0);
        Scripts { world, host, ids, player }
    }
}

/// Size in pixels of an item's inventory icon (`<class>[icon].bmp`), read from the bitmap header.
pub fn item_icon_size(pak: &PakSet, class: &str) -> Option<(u32, u32)> {
    let bytes = pak.read(&format!("{class}[icon].bmp")).ok()?;
    let w = i32::from_le_bytes(bytes.get(18..22)?.try_into().ok()?);
    let h = i32::from_le_bytes(bytes.get(22..26)?.try_into().ok()?);
    Some((w.unsigned_abs(), h.unsigned_abs()))
}

/// Radius and height of the cylinder that stands for a character in collisions, from its model's vertices
/// (Arx coordinates, y down; `origin` is the model's origin vertex, normally at the feet). Ported from the
/// engine: the radius is 1.2 times the farthest foot-level vertex, shrunk for very short models, capped at 40,
/// and the collision test clamps the scaled values to 25..60 and 45..165.
pub fn character_cylinder(vertices: &[[f32; 3]], origin: usize, scale: f32) -> Option<(f32, f32)> {
    let o = *vertices.get(origin)?;
    let (mut reach, mut height) = (0.0f32, 0.0f32);
    for (i, v) in vertices.iter().enumerate() {
        if i != origin && (o[1] - v[1]).abs() < 20.0 {
            reach = reach.max(Vec3::from(o).distance(Vec3::from(*v)));
        }
        height = height.max(o[1] - v[1]);
    }
    if reach == 0.0 || height == 0.0 {
        return None;
    }
    let mut radius = reach * 1.2;
    if height < 40.0 {
        radius *= 0.5 + height / 40.0 * 0.5;
    }
    let height = height.clamp(40.0, 165.0);
    let radius = radius.min(40.0);
    Some(((radius * scale).clamp(25.0, 60.0), (height * scale).clamp(45.0, 165.0)))
}

/// Which collision obstacle belongs to which entity.
#[derive(Default)]
pub struct EntityObstacles {
    pub by_entity: HashMap<EntityId, ObstacleId>,
    /// Characters (NPCs) block the player with a cylinder.
    pub characters: HashMap<EntityId, CylinderId>,
}

impl EntityObstacles {
    /// Add an obstacle for every fixed entity (doors, portcullises, chests, statues, ...) that has a
    /// model, using its mesh as scripts left it (mesh variant, scale). Items and NPCs do not block
    /// the player, like in the original engine.
    pub fn build(
        collision: &mut CollisionWorld,
        pak: &PakSet,
        dlf: &Dlf,
        scene_pos: Vec3,
        world: &ScriptWorld,
        host: &StdHost,
        ids: &[EntityId],
    ) -> EntityObstacles {
        let mut out = EntityObstacles::default();
        let mut models: HashMap<String, Option<Arc<Ftl>>> = HashMap::new();
        for (index, e) in dlf.entities.iter().enumerate() {
            let id = ids[index];
            let kind = world.entity(id).kind;
            if kind != EntityKind::Fix && kind != EntityKind::Npc {
                continue;
            }
            let st = host.state(id).cloned().unwrap_or_default();
            let class = st.mesh.as_deref().unwrap_or(&e.class);
            let path = format!("game/{class}.ftl");
            let model = models
                .entry(path.clone())
                .or_insert_with(|| pak.read(&path).ok().and_then(|b| Ftl::parse(&b).ok()).map(Arc::new))
                .clone();
            let Some(ftl) = model else { continue };
            let origin = to_yup([e.pos[0] + scene_pos.x, e.pos[1] + scene_pos.y, e.pos[2] + scene_pos.z]);
            if kind == EntityKind::Npc {
                let points: Vec<[f32; 3]> = ftl.vertices.iter().map(|v| v.pos).collect();
                if let Some((radius, height)) = character_cylinder(&points, ftl.origin as usize, st.scale) {
                    // `physical radius` / `physical height` in the script replace the model's own size.
                    let radius = st.radius.map_or(radius, |r| (r * st.scale).clamp(25.0, 60.0));
                    let height = st.height.map_or(height, |h| (h * st.scale).clamp(45.0, 165.0));
                    out.characters.insert(id, collision.add_cylinder(origin, radius, height));
                }
                continue;
            }
            let rot = entity_rotation(e.angle, false);
            let world_pos = |i: u16| origin + rot * (to_yup(ftl.vertices[i as usize].pos) * st.scale);
            let tris = ftl.faces.iter().map(|f| [world_pos(f.vid[0]), world_pos(f.vid[1]), world_pos(f.vid[2])]);
            if let Some(oid) = collision.add_obstacle(tris) {
                out.by_entity.insert(id, oid);
            }
        }
        out.sync(collision, host);
        out
    }

    /// Apply the scripts' current collision flags: an entity blocks while its collision is on and it
    /// is neither hidden nor destroyed (an open door has collision off).
    pub fn sync(&self, collision: &CollisionWorld, host: &StdHost) {
        for (&entity, &obstacle) in &self.by_entity {
            let solid = host.state(entity).is_none_or(|s| s.collision && !s.hidden && !s.destroyed);
            if collision.obstacle_enabled(obstacle) != solid {
                collision.set_obstacle_enabled(obstacle, solid);
            }
        }
        for (&entity, &cylinder) in &self.characters {
            let solid = host.state(entity).is_none_or(|s| s.collision && !s.hidden && !s.destroyed);
            if let Some(c) = collision.cylinder(cylinder)
                && c.enabled != solid
            {
                collision.set_cylinder(cylinder, c.base, solid);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn character_cylinders_follow_the_engines_rules() {
        // A human-ish figure: origin at the feet, a wide stance (feet 40 from the origin) and 170 tall.
        let pts = [[0.0, 0.0, 0.0], [40.0, 0.0, 0.0], [-30.0, 5.0, 10.0], [0.0, -170.0, 0.0], [20.0, -90.0, 0.0]];
        let (radius, height) = character_cylinder(&pts, 0, 1.0).unwrap();
        assert!((radius - 40.0).abs() < 1e-4, "1.2 x 40 = 48, capped at 40: {radius}");
        assert!((height - 165.0).abs() < 1e-4, "170 is clamped to 165: {height}");
        // Scaled down, the radius clamps to at least 25.
        let (small_r, small_h) = character_cylinder(&pts, 0, 0.5).unwrap();
        assert_eq!((small_r, small_h), (25.0, 82.5));
        // A flat model has no cylinder.
        assert!(character_cylinder(&[[0.0; 3], [10.0, 0.0, 0.0]], 0, 1.0).is_none());
        assert!(character_cylinder(&[], 0, 1.0).is_none());
    }

    #[test]
    fn engine_yaw_remapping() {
        // An object with stored yaw 270 is drawn unrotated; yaw 0 is turned by 270 degrees in Arx axes.
        let upright = entity_rotation([0.0, 270.0, 0.0], false);
        assert!(upright.angle_between(Quat::IDENTITY) < 1e-5);
        // Stored yaw 90 -> engine yaw 180: a half turn about the vertical axis.
        let half = entity_rotation([0.0, 90.0, 0.0], false) * Vec3::X;
        assert!((half - Vec3::NEG_X).length() < 1e-5, "{half:?}");
        // NPCs: stored yaw 180 is drawn unrotated, stored 90 turns them a quarter.
        assert!(entity_rotation([0.0, 180.0, 0.0], true).angle_between(Quat::IDENTITY) < 1e-5);
        let quarter = entity_rotation([0.0, 90.0, 0.0], true) * Vec3::X;
        assert!(quarter.y.abs() < 1e-5 && (quarter.x.abs() < 1e-5), "{quarter:?}");
        assert_eq!(to_yup([1.0, 2.0, 3.0]), Vec3::new(1.0, -2.0, -3.0));
    }
}
