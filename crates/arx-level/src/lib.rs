//! Glue between the file formats, the script interpreter and the collision world: everything needed
//! to bring a level's entities to life that does not depend on a renderer.

pub mod inventory;

use arx_formats::{PakSet, dlf::Dlf, ftl::Ftl};
use arx_physics::{CollisionWorld, ObstacleId};
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
        let player = world.add_entity(EntityKind::Player, "graph/obj3d/interactive/npc/player/player", 1, None, None);

        let load = |path: &str| pak.read(path).ok().map(|b| Arc::new(Script::new(&b)));
        let mut ids = Vec::with_capacity(dlf.entities.len());
        for e in &dlf.entities {
            let (dir, name) = e.class.rsplit_once('/').unwrap_or(("", &e.class));
            let class_script = load(&format!("{}.asl", e.class));
            let over_script = load(&format!("{dir}/{name}_{:04}/{name}.asl", e.instance));
            let id = world.add_entity(EntityKind::from_class(&e.class), &e.class, e.instance, class_script, over_script);
            world.entity_mut(id).pos = [e.pos[0] + scene_pos.x, e.pos[1] + scene_pos.y, e.pos[2] + scene_pos.z];
            ids.push(id);
        }

        for &id in &ids {
            world.send_event(&mut host, None, id, "load", Vec::new());
        }
        for &id in &ids {
            world.send_init(&mut host, id);
        }
        for &id in &ids {
            world.send_event(&mut host, None, id, "game_ready", Vec::new());
        }
        world.update(&mut host, 0.0);
        Scripts { world, host, ids, player }
    }
}

/// Which collision obstacle belongs to which entity.
#[derive(Default)]
pub struct EntityObstacles {
    pub by_entity: HashMap<EntityId, ObstacleId>,
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
            if world.entity(id).kind != EntityKind::Fix {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
