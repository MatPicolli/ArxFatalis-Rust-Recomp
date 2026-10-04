//! What cutscenes are made of: entities (cameras mostly) that follow the level's paths, the camera the scene is seen
//! through and what it looks at, and the jumps scripts ask for (`teleport`, `playerlookat`). Ported from the
//! engine's `UpdateCameras`, `ARX_PATHS_Interpolate` and the camera script commands. The fades, the black bars and
//! whether the player has control are plain state in `StdHost::stage`.

use crate::npc::NpcWorld;
use arx_formats::dlf::Dlf;
use arx_script::{EntityId, EntityKind, ScriptWorld, StageRequest, StdHost, TargetSpec};
use glam::Vec3;
use std::collections::HashMap;

/// The path's next point is reached along a curve through the point after it.
const PATHWAY_BEZIER: i32 = 1;

#[derive(Debug, Clone)]
pub struct Waypoint {
    /// Relative to the path's origin (Arx coordinates).
    pub pos: Vec3,
    pub bezier: bool,
    /// Time taken to get here from the previous point, milliseconds.
    pub time_ms: f32,
}

#[derive(Debug, Clone)]
pub struct LevelPath {
    pub name: String,
    pub pos: Vec3,
    pub points: Vec<Waypoint>,
}

impl LevelPath {
    /// Where the path is `time_ms` after its start, and which waypoint was passed last (`-2` once it has ended).
    /// The first point is where it starts (its own time does not count).
    pub fn at(&self, time_ms: f32) -> (Vec3, i64) {
        let p = &self.points;
        if p.is_empty() {
            return (self.pos, -1);
        }
        if time_ms <= 0.0 {
            return (self.pos + p[0].pos, 0);
        }
        let mut tim = time_ms;
        let mut target = 1;
        while target < p.len() {
            let bezier = p[target - 1].bezier && target + 1 < p.len();
            if bezier {
                target += 1;
            }
            let t = p[target].time_ms;
            if tim >= t {
                tim -= t;
                target += 1;
                continue;
            }
            let rel = tim / t;
            let pos = if bezier {
                let (p0, p1, p2) = (p[target - 2].pos, p[target - 1].pos, p[target].pos);
                self.pos + p0 * (1.0 - rel) + p1 * (rel - rel * rel) + p2 * (rel * rel)
            } else {
                self.pos + p[target - 1].pos.lerp(p[target].pos, rel)
            };
            return (pos, target as i64 - 1);
        }
        (self.pos + p[p.len() - 1].pos, -2)
    }
}

#[derive(Debug, Clone)]
struct PathUse {
    path: usize,
    time_ms: f32,
    /// 1 forward, -1 backward, 0 paused.
    direction: i8,
    last_waypoint: i64,
}

/// Something the application has to carry out.
#[derive(Debug, Clone, PartialEq)]
pub enum StageEffect {
    /// Put the hero's feet here (Arx coordinates), facing `yaw` (the engine's player yaw in degrees) if given.
    TeleportPlayer { pos: Vec3, yaw: Option<f32> },
    /// Turn the hero's head toward this point.
    LookAt(Vec3),
    /// Go to another level, arriving at the entity called `target`.
    ChangeLevel { level: u32, target: String, yaw: Option<f32> },
}

/// What the active camera sees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraView {
    pub pos: Vec3,
    pub target: Vec3,
    /// Vertical field of view in radians (the engine's focal length over a 480-unit image plane).
    pub fov: f32,
}

#[derive(Default)]
pub struct StageWorld {
    pub paths: Vec<LevelPath>,
    uses: HashMap<EntityId, PathUse>,
    /// Where each camera looked last frame, for smoothing.
    last_target: HashMap<EntityId, Vec3>,
    /// Where entities were when the level began (`teleport -i`).
    init_pos: HashMap<EntityId, [f32; 3]>,
    /// The stored yaw of each entity (degrees), for cameras with nothing to look at.
    yaw: HashMap<EntityId, f32>,
}

impl StageWorld {
    /// The paths of a level (the scene's paths without a height; those with one are zones).
    pub fn from_dlf(dlf: &Dlf, scene_pos: Vec3, ids: &[EntityId]) -> Self {
        let paths = dlf
            .paths
            .iter()
            .filter(|p| p.height == 0)
            .map(|p| LevelPath {
                name: p.name.to_ascii_lowercase(),
                pos: Vec3::from(p.pos) + scene_pos,
                points: p.pathways.iter().map(|w| Waypoint { pos: Vec3::from(w.pos), bezier: w.flag == PATHWAY_BEZIER, time_ms: w.time_ms as f32 }).collect(),
            })
            .collect();
        let yaw = dlf.entities.iter().zip(ids).map(|(e, &id)| (id, e.angle[1])).collect();
        StageWorld { paths, yaw, ..Default::default() }
    }

    fn remember_start(&mut self, world: &ScriptWorld) {
        if self.init_pos.is_empty() {
            self.init_pos = world.entities.iter().map(|e| (e.id, e.pos)).collect();
        }
    }

    fn place(world: &mut ScriptWorld, host: &mut StdHost, npcs: &mut Option<&mut NpcWorld>, id: EntityId, pos: Vec3) {
        world.entity_mut(id).pos = pos.to_array();
        match world.entity(id).kind {
            EntityKind::Npc => {
                if let Some(n) = npcs.as_mut().and_then(|n| n.npc_mut(id)) {
                    n.pos = pos;
                }
            }
            EntityKind::Camera | EntityKind::Marker | EntityKind::Player => {}
            _ => host.modify(id, |s| s.moved_to = Some(pos.to_array())),
        }
    }

    /// Carry out what scripts asked for and move everything that follows a path by `dt_ms`.
    pub fn update(&mut self, world: &mut ScriptWorld, host: &mut StdHost, mut npcs: Option<&mut NpcWorld>, dt_ms: f32) -> Vec<StageEffect> {
        self.remember_start(world);
        let mut effects = Vec::new();
        for r in host.take_stage_requests() {
            match r {
                StageRequest::SetPath { entity, name, .. } => match name {
                    None => {
                        self.uses.remove(&entity);
                    }
                    Some(name) => match self.paths.iter().position(|p| p.name == name) {
                        Some(path) => {
                            self.uses.insert(entity, PathUse { path, time_ms: 0.0, direction: 1, last_waypoint: -1 });
                        }
                        None => {
                            let who = world.entity(entity).id_string.clone();
                            *world.stats.warnings.entry(format!("{who}: unknown path: {name}")).or_default() += 1;
                        }
                    },
                },
                StageRequest::UsePath { entity, mode } => {
                    if let Some(u) = self.uses.get_mut(&entity) {
                        u.direction = match mode {
                            'b' => -1,
                            'p' => 0,
                            _ => 1,
                        };
                    }
                }
                StageRequest::Teleport { entity, to } => {
                    let pos = match to {
                        Some(t) => Some(Vec3::from(world.entity(t).pos)),
                        None => self.init_pos.get(&entity).map(|p| Vec3::from(*p)),
                    };
                    if let Some(pos) = pos {
                        if Some(entity) == world.player {
                            effects.push(StageEffect::TeleportPlayer { pos, yaw: None });
                        } else {
                            Self::place(world, host, &mut npcs, entity, pos);
                        }
                    }
                }
                StageRequest::TeleportPlayer { to, yaw } => {
                    effects.push(StageEffect::TeleportPlayer { pos: Vec3::from(world.entity(to).pos), yaw });
                }
                StageRequest::ChangeLevel { level, target, yaw } => effects.push(StageEffect::ChangeLevel { level, target, yaw }),
                StageRequest::LookAt { entity } => {
                    // A character is looked at in the face, anything else where it is.
                    let up = if world.entity(entity).kind == EntityKind::Npc { Vec3::new(0.0, -150.0, 0.0) } else { Vec3::ZERO };
                    effects.push(StageEffect::LookAt(Vec3::from(world.entity(entity).pos) + up));
                }
                StageRequest::Cinematic { entity, name } => {
                    // The 2D cinematics are not played yet: they end at once, so what follows them goes on.
                    world.queue_event(None, entity, "cine_end", vec![name]);
                }
            }
        }

        // Along the paths.
        let ids: Vec<EntityId> = self.uses.keys().copied().collect();
        for id in ids {
            let (pos, wp, last, count) = {
                let u = self.uses.get_mut(&id).expect("listed");
                let path = &self.paths[u.path];
                let finished = u.last_waypoint == -2;
                match u.direction {
                    1 if !finished => u.time_ms += dt_ms,
                    -1 => u.time_ms = (u.time_ms - dt_ms).max(1.0),
                    _ => {}
                }
                let (pos, wp) = path.at(u.time_ms);
                let last = u.last_waypoint;
                u.last_waypoint = wp;
                (pos, wp, last, path.points.len())
            };
            Self::place(world, host, &mut npcs, id, pos);
            if wp != last {
                if wp == -2 {
                    let n = (count.max(1) - 1).to_string();
                    world.send_event(host, None, id, "waypoint", vec![n.clone()]);
                    world.send_event(host, None, id, &format!("waypoint{n}"), Vec::new());
                    world.send_event(host, None, id, "pathend", Vec::new());
                } else {
                    let mut ii = last + 1;
                    if ii < 0 || ii > wp {
                        ii = 0;
                    }
                    world.send_event(host, None, id, "waypoint", vec![ii.to_string()]);
                    world.send_event(host, None, id, &format!("waypoint{ii}"), Vec::new());
                    if ii as usize == count {
                        world.send_event(host, None, id, "pathend", Vec::new());
                    }
                }
            }
        }
        effects
    }

    /// Is this entity following a path?
    pub fn on_path(&self, id: EntityId) -> bool {
        self.uses.contains_key(&id)
    }

    /// What the scripts' active camera sees (none while the scene is seen through the hero's eyes). The camera looks at
    /// its target plus its offset, following it with the smoothing it was given.
    pub fn camera_view(&mut self, world: &ScriptWorld, host: &StdHost, dt_ms: f32) -> Option<CameraView> {
        let cam = host.stage.camera?;
        let st = host.state(cam).cloned().unwrap_or_default();
        let pos = Vec3::from(world.entity(cam).pos);
        let mut target = match st.target {
            TargetSpec::Entity(t) if t != cam => Vec3::from(world.entity(t).pos) + Vec3::from(st.cam_translate),
            _ => {
                // Nothing to look at: along its own heading.
                let yaw = self.yaw.get(&cam).copied().unwrap_or(0.0).to_radians();
                pos + Vec3::new(yaw.cos(), 0.0, yaw.sin()) * 20.0
            }
        };
        if let Some(&last) = self.last_target.get(&cam)
            && st.cam_smoothing != 0.0
        {
            let vv = (8000.0 - st.cam_smoothing.min(8000.0)) / 4000.0;
            let f1 = (dt_ms / 1000.0 * vv).min(1.0);
            target = target * (1.0 - f1) + last * f1;
        }
        self.last_target.insert(cam, target);
        let focal = if st.cam_focal < 100.0 { 350.0 } else { st.cam_focal };
        Some(CameraView { pos, target, fov: 2.0 * (240.0 / focal).atan() })
    }
}

/// How far a fade has got, 0 (clear) to 1 (all colour), at script time `now_ms`.
pub fn fade_level(fade: &arx_script::Fade, now_ms: f64) -> f32 {
    let t = if fade.duration_ms <= 0.0 { 1.0 } else { (((now_ms - fade.started_ms) as f32) / fade.duration_ms).clamp(0.0, 1.0) };
    if fade.out { t } else { 1.0 - t }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arx_script::{Script, StdHost};
    use std::sync::Arc;

    fn straight() -> LevelPath {
        LevelPath {
            name: "walk".into(),
            pos: Vec3::new(100.0, 0.0, 0.0),
            points: vec![
                Waypoint { pos: Vec3::ZERO, bezier: false, time_ms: 0.0 },
                Waypoint { pos: Vec3::new(0.0, 0.0, 100.0), bezier: false, time_ms: 1000.0 },
                Waypoint { pos: Vec3::new(100.0, 0.0, 100.0), bezier: false, time_ms: 2000.0 },
            ],
        }
    }

    #[test]
    fn a_path_is_walked_point_to_point_in_each_points_time() {
        let p = straight();
        assert_eq!(p.at(0.0), (Vec3::new(100.0, 0.0, 0.0), 0));
        assert_eq!(p.at(500.0), (Vec3::new(100.0, 0.0, 50.0), 0));
        assert_eq!(p.at(2000.0), (Vec3::new(150.0, 0.0, 100.0), 1), "half-way along the second, slower leg");
        assert_eq!(p.at(5000.0), (Vec3::new(200.0, 0.0, 100.0), -2), "past the end it stays at the last point");
    }

    #[test]
    fn a_curved_leg_bends_through_its_middle_point() {
        let mut p = straight();
        p.points[0].bezier = true;
        // One curve from the first point to the third, in the third point's time.
        let (mid, wp) = p.at(1000.0);
        assert_eq!(wp, 1);
        // p0 (1 - t) + p1 (t - t^2) + p2 t^2 at t = 0.5, with p0 = 0.
        assert!((mid - (Vec3::new(100.0, 0.0, 0.0) + Vec3::new(0.0, 0.0, 100.0) * 0.25 + Vec3::new(100.0, 0.0, 100.0) * 0.25)).length() < 1e-3, "{mid:?}");
    }

    #[test]
    fn a_camera_follows_its_path_looks_at_its_target_and_reports_waypoints() {
        let latin = |src: &str| Arc::new(Script::new(&src.chars().map(|c| c as u8).collect::<Vec<u8>>()));
        let mut world = ScriptWorld::new();
        let mut host = StdHost::new();
        let player = world.add_entity(EntityKind::Player, "x/player/player", 1, None, None);
        let cam = world.add_entity(
            EntityKind::Camera,
            "x/system/camera/camera",
            1,
            Some(latin("on init {\n settarget player\n cameratranslatetarget 0 -100 0\n camerafocal 480\n accept\n}\non go {\n setpath walk\n cameraactivate self\n cinemascope on\n setplayercontrols off\n accept\n}\non waypoint {\n set §wp ^#param1\n accept\n}\non pathend {\n set §done 1\n cameraactivate none\n setplayercontrols on\n accept\n}")),
            None,
        );
        world.send_init(&mut host, cam);
        world.entity_mut(player).pos = [0.0, 0.0, 0.0];
        let mut stage = StageWorld { paths: vec![straight()], ..Default::default() };
        assert!(stage.camera_view(&world, &host, 16.0).is_none(), "seen through the hero's eyes until a camera is activated");
        world.send_event(&mut host, None, cam, "go", vec![]);
        assert!(!host.stage.controls && host.stage.cinemascope);
        for _ in 0..30 {
            stage.update(&mut world, &mut host, None, 1000.0 / 60.0);
        }
        // Half a second in: half-way along the first leg, looking at the player's head.
        let v = stage.camera_view(&world, &host, 16.0).unwrap();
        assert!((v.pos - Vec3::new(100.0, 0.0, 50.0)).length() < 1.0, "{:?}", v.pos);
        assert_eq!(v.target, Vec3::new(0.0, -100.0, 0.0));
        assert!((v.fov - 2.0 * (0.5f32).atan()).abs() < 1e-5, "focal 480 over a 480 image plane");
        assert_eq!(world.entity(cam).vars.get_int("§wp"), 0);
        for _ in 0..200 {
            stage.update(&mut world, &mut host, None, 1000.0 / 60.0);
        }
        assert_eq!(world.entity(cam).vars.get_int("§done"), 1, "pathend was sent");
        assert!(host.stage.controls && host.stage.camera.is_none());
        assert!(stage.camera_view(&world, &host, 16.0).is_none());
    }

    #[test]
    fn teleports_move_entities_and_ask_the_application_to_move_the_hero() {
        let latin = |src: &str| Arc::new(Script::new(&src.chars().map(|c| c as u8).collect::<Vec<u8>>()));
        let mut world = ScriptWorld::new();
        let mut host = StdHost::new();
        let _player = world.add_entity(EntityKind::Player, "x/player/player", 1, None, None);
        let marker = world.add_entity(EntityKind::Marker, "x/system/marker/marker", 7, None, None);
        let crate_ = world.add_entity(
            EntityKind::Fix,
            "x/fix_inter/crate/crate",
            1,
            Some(latin("on go {\n teleport marker_0007\n teleport -pa 90 marker_0007\n playerlookat marker_0007\n worldfade out 500 0 0 0\n accept\n}\non back {\n teleport -i\n accept\n}")),
            None,
        );
        world.entity_mut(marker).pos = [50.0, 10.0, 60.0];
        world.entity_mut(crate_).pos = [1.0, 2.0, 3.0];
        let mut stage = StageWorld::default();
        stage.update(&mut world, &mut host, None, 16.0);
        world.send_event(&mut host, None, crate_, "go", vec![]);
        let fx = stage.update(&mut world, &mut host, None, 16.0);
        assert_eq!(world.entity(crate_).pos, [50.0, 10.0, 60.0]);
        assert_eq!(host.state(crate_).unwrap().moved_to, Some([50.0, 10.0, 60.0]));
        assert_eq!(fx, vec![StageEffect::TeleportPlayer { pos: Vec3::new(50.0, 10.0, 60.0), yaw: Some(90.0) }, StageEffect::LookAt(Vec3::new(50.0, 10.0, 60.0))]);
        let fade = host.stage.fade.unwrap();
        assert_eq!(fade_level(&fade, fade.started_ms + 250.0), 0.5);
        world.send_event(&mut host, None, crate_, "back", vec![]);
        stage.update(&mut world, &mut host, None, 16.0);
        assert_eq!(world.entity(crate_).pos, [1.0, 2.0, 3.0], "back where the level put it");
    }
}
