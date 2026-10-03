//! Glue between the file formats, the script interpreter and the collision world: everything needed
//! to bring a level's entities to life that does not depend on a renderer.

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

/// Rotation of an entity, from its `(pitch, yaw, roll)` angles in degrees. Arx applies
/// `Rz(-roll) * Rx(pitch) * Ry(yaw)` in its own axes; after the Arx to y-up flip that is
/// `Rz(roll) * Rx(pitch) * Ry(-yaw)`.
pub fn entity_rotation(angle: [f32; 3]) -> Quat {
    let [pitch, yaw, roll] = angle.map(f32::to_radians);
    Quat::from_rotation_z(roll) * Quat::from_rotation_x(pitch) * Quat::from_rotation_y(-yaw)
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
            let rot = entity_rotation(e.angle);
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
    fn rotation_conventions() {
        // Yaw 90 in Arx turns +Z (Arx forward) towards +X (Arx: x right). In y-up space forward is -Z,
        // and the rotation about the vertical axis is negated by the flip.
        let r = entity_rotation([0.0, 90.0, 0.0]);
        let v = r * Vec3::NEG_Z;
        assert!((v - Vec3::new(1.0, 0.0, 0.0)).length() < 1e-5, "{v:?}");
        assert_eq!(to_yup([1.0, 2.0, 3.0]), Vec3::new(1.0, -2.0, -3.0));
    }
}
