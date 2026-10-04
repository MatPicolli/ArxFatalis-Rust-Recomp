//! The level's zones: named areas (a polygon seen from above, with a height) that scripts watch. Whoever walks into
//! one is told (`enterzone`), and so is the entity that controls it (`setcontrolledzone` -> `controlledzone_enter` with
//! who came in): that is how a level notices the player reaching a place. Ported from the engine's `ai/Paths.cpp`.

use arx_formats::dlf::Dlf;
use arx_script::{EntityId, EntityKind, ScriptWorld, StdHost, Value};
use glam::Vec3;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Zone {
    /// Lowercased.
    pub name: String,
    /// Origin (Arx coordinates); the outline is relative to it.
    pub pos: Vec3,
    pub outline: Vec<Vec3>,
    min: Vec3,
    max: Vec3,
    /// Sound that plays while the player is inside.
    pub ambiance: String,
    pub ambiance_volume: f32,
}

impl Zone {
    pub fn new(name: &str, pos: Vec3, outline: Vec<Vec3>, height: i32) -> Self {
        let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for p in &outline {
            min = min.min(pos + *p);
            max = max.max(pos + *p);
        }
        // A positive height bounds the zone above its origin (Arx y is down); otherwise it is as tall as the world.
        if height > 0 {
            min.y = pos.y - height as f32;
            max.y = pos.y;
        } else {
            min.y = -99_999_999.0;
            max.y = 99_999_999.0;
        }
        Zone { name: name.to_ascii_lowercase(), pos, outline, min, max, ambiance: String::new(), ambiance_volume: 100.0 }
    }

    /// Is the point (Arx coordinates) inside? An even-odd test of the outline seen from above, within the height.
    pub fn contains(&self, p: Vec3) -> bool {
        if p.x < self.min.x || p.x > self.max.x || p.z < self.min.z || p.z > self.max.z || p.y < self.min.y || p.y > self.max.y {
            return false;
        }
        let (x, z) = (p.x - self.pos.x, p.z - self.pos.z);
        let mut inside = false;
        let n = self.outline.len();
        for i in 0..n {
            let (a, b) = (self.outline[i], self.outline[(i + n - 1) % n]);
            if ((a.z <= z && z < b.z) || (b.z <= z && z < a.z)) && x < (b.x - a.x) * (z - a.z) / (b.z - a.z) + a.x {
                inside = !inside;
            }
        }
        inside
    }
}

#[derive(Default)]
pub struct Zones {
    pub zones: Vec<Zone>,
    /// The zone each watched entity was in last.
    inside: HashMap<EntityId, usize>,
}

impl Zones {
    /// The zones of a level: the scene's paths that have a height. Positions are relative to the scene origin.
    pub fn from_dlf(dlf: &Dlf, scene_pos: Vec3) -> Self {
        let zones = dlf
            .paths
            .iter()
            .filter(|p| p.height != 0)
            .map(|p| {
                let mut z = Zone::new(&p.name, Vec3::from(p.pos) + scene_pos, p.pathways.iter().map(|w| Vec3::from(w.pos)).collect(), p.height);
                z.ambiance = p.ambiance.to_ascii_lowercase();
                z.ambiance_volume = if p.amb_max_vol <= 1.0 { 100.0 } else { p.amb_max_vol };
                z
            })
            .collect();
        Zones { zones, inside: HashMap::new() }
    }

    /// The first zone containing a point.
    pub fn at(&self, p: Vec3) -> Option<usize> {
        self.zones.iter().position(|z| z.contains(p))
    }

    /// The zone the player is in (for its ambiance).
    pub fn of(&self, entity: EntityId) -> Option<&Zone> {
        self.inside.get(&entity).map(|&i| &self.zones[i])
    }

    /// See who changed zone since the last call (the player, characters and items in the world) and tell them and the
    /// zones' controllers.
    pub fn update(&mut self, world: &mut ScriptWorld, host: &mut StdHost) {
        if self.zones.is_empty() {
            return;
        }
        let player = world.player;
        let watched: Vec<(EntityId, Vec3)> = world
            .entities
            .iter()
            .filter(|e| matches!(e.kind, EntityKind::Player | EntityKind::Npc | EntityKind::Item))
            .filter(|e| Some(e.id) == player || host.state(e.id).is_none_or(|s| !s.destroyed && !s.in_inventory && !s.equipped))
            .map(|e| (e.id, Vec3::from(e.pos)))
            .collect();
        for (id, pos) in watched {
            let now = self.at(pos);
            let before = self.inside.get(&id).copied();
            if now == before {
                continue;
            }
            let who = world.entity(id).id_string.clone();
            if std::env::var_os("ARX_LOG_ZONES").is_some() {
                let name = |z: Option<usize>| z.map_or("-", |z| self.zones[z].name.as_str());
                eprintln!("zone: {who} {} -> {} (controller {:?})", name(before), name(now), now.and_then(|z| host.controlled_zones.get(&self.zones[z].name)));
            }
            if let Some(last) = before {
                let name = self.zones[last].name.clone();
                world.send_event(host, None, id, "leavezone", vec![name.clone()]);
                if let Some(&c) = host.controlled_zones.get(&name) {
                    world.send_event(host, None, c, "controlledzone_leave", vec![who.clone(), name]);
                }
            }
            match now {
                Some(z) => {
                    self.inside.insert(id, z);
                    let name = self.zones[z].name.clone();
                    world.send_event(host, None, id, "enterzone", vec![name.clone()]);
                    if let Some(&c) = host.controlled_zones.get(&name) {
                        world.send_event(host, None, c, "controlledzone_enter", vec![who, name]);
                    }
                }
                None => {
                    self.inside.remove(&id);
                }
            }
            if Some(id) == player {
                let name = now.map_or("none".to_owned(), |z| self.zones[z].name.clone());
                world.sys.insert("^player_zone".to_owned(), Value::Text(name));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arx_script::Script;
    use std::sync::Arc;

    fn square(name: &str, at: Vec3, half: f32, height: i32) -> Zone {
        Zone::new(name, at, vec![Vec3::new(-half, 0.0, -half), Vec3::new(half, 0.0, -half), Vec3::new(half, 0.0, half), Vec3::new(-half, 0.0, half)], height)
    }

    #[test]
    fn a_zone_is_its_outline_from_above_and_its_height() {
        let z = square("Cell", Vec3::new(100.0, 50.0, 100.0), 50.0, 200);
        assert_eq!(z.name, "cell");
        assert!(z.contains(Vec3::new(120.0, 0.0, 80.0)));
        assert!(!z.contains(Vec3::new(160.0, 0.0, 80.0)), "outside the outline");
        assert!(!z.contains(Vec3::new(120.0, -200.0, 80.0)), "above it (y is down)");
        assert!(!z.contains(Vec3::new(120.0, 60.0, 80.0)), "below its floor");
        // Without a height it reaches everywhere up and down.
        assert!(square("tall", Vec3::ZERO, 50.0, -1).contains(Vec3::new(0.0, -5000.0, 0.0)));
    }

    #[test]
    fn walking_into_a_controlled_zone_tells_its_controller_who_came() {
        let latin = |src: &str| Arc::new(Script::new(&src.chars().map(|c| c as u8).collect::<Vec<u8>>()));
        let mut world = ScriptWorld::new();
        let mut host = StdHost::new();
        let player = world.add_entity(EntityKind::Player, "x/player/player", 1, None, None);
        let marker = world.add_entity(
            EntityKind::Marker,
            "x/system/marker/marker",
            1,
            Some(latin("on init {\n setcontrolledzone Kultar_Wake\n accept\n}\non controlledzone_enter {\n if (^$param1 != \"player\") accept\n set §entered 1\n unsetcontrolledzone kultar_wake\n accept\n}\non controlledzone_leave {\n set §left 1\n accept\n}")),
            None,
        );
        world.send_init(&mut host, marker);
        let mut zones = Zones { zones: vec![square("kultar_wake", Vec3::ZERO, 100.0, -1)], inside: HashMap::new() };
        world.entity_mut(player).pos = [500.0, 0.0, 0.0];
        zones.update(&mut world, &mut host);
        assert_eq!(world.entity(marker).vars.get_int("§entered"), 0);
        world.entity_mut(player).pos = [50.0, 0.0, 0.0];
        zones.update(&mut world, &mut host);
        assert_eq!(world.entity(marker).vars.get_int("§entered"), 1, "the player's id is `player`");
        assert_eq!(world.sys.get("^player_zone"), Some(&Value::Text("kultar_wake".into())));
        // The marker let go of the zone, so leaving it tells nobody.
        world.entity_mut(player).pos = [500.0, 0.0, 0.0];
        zones.update(&mut world, &mut host);
        assert_eq!(world.entity(marker).vars.get_int("§left"), 0);
        assert_eq!(world.sys.get("^player_zone"), Some(&Value::Text("none".into())));
    }
}
