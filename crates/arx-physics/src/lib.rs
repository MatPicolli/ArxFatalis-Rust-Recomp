//! Level collision and a first-person player body.
//!
//! All coordinates here are **y-up** (the Bevy convention); [`CollisionWorld::from_fts`] converts
//! from Arx's y-down data. The world is a bag of triangles in a 2D spatial hash. The player is a
//! vertical cylinder that slides along steep triangles, steps over low ones, stands on walkable
//! ones and bumps its head on ceilings.
//!
//! The player's movement follows the original engine (`PlayerMovementIterate`): keys push the body with a
//! force whose strength comes from the speed of the hero animation that would be playing, the body's
//! horizontal velocity is damped every step, jumps rise a fixed 130 units in 200 ms and then fall slowly
//! (that is `Player::classic`; by default the jump and the crouch are tightened, see there),
//! and a long fall hurts. See [`MoveInput`], [`Player`] and `arx player-speeds`.

pub mod items;

use arx_formats::{fts::Fts, poly};
use glam::{Vec2, Vec3};
use std::collections::HashMap;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// Dimensions of the original player cylinder.
pub const PLAYER_RADIUS: f32 = 52.0;
pub const PLAYER_HEIGHT: f32 = 170.0;
/// Height of the cylinder once the crouch animation has finished.
pub const CROUCH_HEIGHT: f32 = 120.0;
/// Eyes are slightly below the top of the cylinder.
pub const EYE_HEIGHT: f32 = 160.0;
/// Eye height while crouched (the original attaches the camera to the head of the animated model).
pub const CROUCH_EYE_HEIGHT: f32 = 110.0;
/// Highest ledge the player walks onto without jumping (`PLAYER_CYLINDER_STEP` in the engine).
pub const STEP_HEIGHT: f32 = 40.0;

const CELL: f32 = 100.0;
/// Triangles at least this flat (in either orientation) can support the player, as in the original
/// engine, which stands the player on the nearest polygon below with no slope limit.
const SUPPORT_NORMAL_Y: f32 = 0.1;
/// Triangles flatter than this are floors/ceilings; steeper ones block horizontal movement.
const WALKABLE_NORMAL_Y: f32 = 0.55;
/// Surfaces at least this flat, in either orientation, stop the head.
const CEILING_NORMAL_Y: f32 = 0.7;

/// Horizontal velocity loses `0.009` of itself per millisecond (`dampen = 1 - 0.009 * dt` in the engine).
const DAMPING_PER_MS: f32 = 0.009;
/// Gravity in units/s^2 (`WORLD_GRAVITY` 0.1 / `TARGET_DT` 33.3 ms, per ms). After a jump or once a fall has
/// reached [`FALL_TRIGGER_SPEED`] the engine switches to the much weaker `JUMP_GRAVITY`.
const WORLD_GRAVITY: f32 = 3000.0;
const FALL_GRAVITY: f32 = 600.0;
const FALL_TRIGGER_SPEED: f32 = 450.0;
/// A jump moves the body up by this much over this long, with no gravity (`jump_up_height`, `jump_up_time`).
const JUMP_RISE: f32 = 130.0;
const JUMP_RISE_MS: f32 = 200.0;
/// A jump key press that cannot be carried out yet stays valid this long.
const JUMP_REQUEST_MS: f32 = 350.0;
/// Falls shorter than this do no damage; beyond it `(height - 400) / 15` is lost.
pub const SAFE_FALL_HEIGHT: f32 = 400.0;
/// Length of the crouch-in / crouch-out animations (`human_normal_crouch_in/out`).
const CROUCH_ANIM_MS: f32 = 708.3;
/// The tightened movement (see [`Player::classic`]): how long ducking and standing up take, ...
const QUICK_CROUCH_MS: f32 = 140.0;
/// ... the one gravity that pulls on a jump and on a fall alike (units/s^2), ...
const QUICK_GRAVITY: f32 = 1700.0;
/// ... how high a jump goes (the take-off speed follows from it), and the fastest a fall gets.
const QUICK_JUMP_HEIGHT: f32 = 95.0;
const QUICK_FALL_SPEED_MAX: f32 = 2600.0;
/// A new body step is never longer than this, so thin walls cannot be skipped.
const MAX_SUBSTEP_SECS: f32 = 1.0 / 60.0;

/// Impulse strength per millisecond, from the root motion of the animation the engine plays: the speed of the
/// animation (root translation / duration) times 0.0125. Measured by `arx player-speeds`. The horizontal
/// velocity settles at `scale / 0.009` per millisecond: 266.7 units/s running, 188.3 sneaking, 100 crouched.
const SCALE_RUN: f32 = 0.002399;
const SCALE_WALK: f32 = 0.001694;
const SCALE_WALK_STRAFE: f32 = 0.001696;
const SCALE_CROUCH_WALK: f32 = 0.000900;
const SCALE_CROUCH_STRAFE: f32 = 0.000779;
const SCALE_CROUCH_TRANSITION: f32 = 0.000500;
/// While jumping or falling the engine uses fixed values instead of an animation's.
const SCALE_AIR_FORWARD: f32 = 0.0079;
const SCALE_AIR_BACKWARD: f32 = 0.0008;
const SCALE_AIR_STRAFE: f32 = 0.0026;

#[derive(Debug, Clone, Copy)]
struct Tri {
    a: Vec3,
    b: Vec3,
    c: Vec3,
    /// Unit normal, oriented to the polygon's front side.
    n: Vec3,
    min_y: f32,
    max_y: f32,
    /// Index into the world's material names (0 = unknown): what the surface is made of.
    mat: u8,
}

/// Where a ray hit a surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    /// Distance along the ray.
    pub t: f32,
    /// Unit normal facing the ray.
    pub normal: Vec3,
}

/// Moeller-Trumbore, two-sided: distance and the normal turned toward the ray origin.
fn ray_triangle(origin: Vec3, dir: Vec3, t: &Tri) -> Option<(f32, Vec3)> {
    let (e1, e2) = (t.b - t.a, t.c - t.a);
    let p = dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-7 {
        return None;
    }
    let inv = 1.0 / det;
    let to = origin - t.a;
    let u = to.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = to.cross(e1);
    let v = dir.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let dist = e2.dot(q) * inv;
    (dist > 1e-4).then(|| (dist, if t.n.dot(dir) > 0.0 { -t.n } else { t.n }))
}

/// A solid object that is not part of the level geometry (a door, a portcullis, a chest), made of
/// its own triangles. It can be switched on and off, e.g. while a door is open.
struct Obstacle {
    tris: Vec<Tri>,
    min: Vec3,
    max: Vec3,
}

/// Index of an obstacle added with [`CollisionWorld::add_obstacle`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObstacleId(pub usize);

/// A standing character that blocks the player: a vertical cylinder, as the engine models NPCs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cylinder {
    /// Centre of the bottom disc.
    pub base: Vec3,
    pub radius: f32,
    pub height: f32,
    pub enabled: bool,
}

/// Index of a cylinder added with [`CollisionWorld::add_cylinder`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CylinderId(pub usize);

#[derive(Default)]
pub struct CollisionWorld {
    tris: Vec<Tri>,
    grid: HashMap<(i32, i32), Vec<u32>>,
    obstacles: Vec<Obstacle>,
    /// One flag per obstacle; atomic so doors can be toggled while the world is shared.
    obstacle_enabled: Vec<AtomicBool>,
    /// Characters; their position and state change while the world is shared, hence the lock.
    cylinders: RwLock<Vec<Cylinder>>,
    /// Centre of the largest flat, upward-facing solid triangle: a safe place to put the player.
    fallback_spawn: Option<(f32, Vec3)>,
    /// Surface materials (`stone`, `wood`, ...), indexed by `Tri::mat`; entry 0 is `unknown`.
    material_names: Vec<String>,
    /// Water surfaces (not solid), for knowing when the player wades.
    water: Vec<Tri>,
    water_grid: HashMap<(i32, i32), Vec<u32>>,
}

/// Arx (+Y down, +Z forward) to y-up, -Z forward.
fn to_yup(p: [f32; 3]) -> Vec3 {
    Vec3::new(p[0], -p[1], -p[2])
}

fn cell(v: f32) -> i32 {
    (v / CELL).floor() as i32
}

/// Closest point to `p` on the 2D triangle `(a, b, c)` (which may be degenerate), and whether `p`
/// is inside it.
fn closest_on_tri_2d(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> (Vec2, bool) {
    let seg = |p: Vec2, a: Vec2, b: Vec2| {
        let ab = b - a;
        let l2 = ab.length_squared();
        let t = if l2 < 1e-9 { 0.0 } else { ((p - a).dot(ab) / l2).clamp(0.0, 1.0) };
        a + ab * t
    };
    let sign = |p1: Vec2, p2: Vec2, p3: Vec2| (p1.x - p3.x) * (p2.y - p3.y) - (p2.x - p3.x) * (p1.y - p3.y);
    let (d1, d2, d3) = (sign(p, a, b), sign(p, b, c), sign(p, c, a));
    let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    if !(has_neg && has_pos) && sign(a, b, c).abs() > 1e-6 {
        return (p, true);
    }
    let candidates = [seg(p, a, b), seg(p, b, c), seg(p, c, a)];
    let best = candidates.into_iter().min_by(|x, y| p.distance_squared(*x).total_cmp(&p.distance_squared(*y)));
    (best.unwrap_or(a), false)
}

impl CollisionWorld {
    /// Build collision geometry from a level. Like the original engine, water, translucent and
    /// `NOCOL` polygons are not solid.
    pub fn from_fts(fts: &Fts) -> Self {
        let mut w = CollisionWorld::default();
        w.material_names.push("unknown".to_owned());
        for p in &fts.polys {
            let v: Vec<Vec3> = p.verts[..p.vertex_count()].iter().map(|v| to_yup(v.pos)).collect();
            if p.flags & poly::WATER != 0 {
                w.push_water(v[0], v[1], v[2]);
                if v.len() == 4 {
                    w.push_water(v[3], v[2], v[1]);
                }
            }
            if p.flags & (poly::WATER | poly::TRANS | poly::NOCOL) != 0 {
                continue;
            }
            // A floor with no texture counts as earth, one whose texture is not recognised as unknown.
            let material = match fts.textures.get(&p.tex).filter(|n| !n.is_empty()) {
                Some(name) => arx_formats::soundmap::floor_material(name),
                None => "earth",
            };
            let mat = w.material_id(material);
            w.push(v[0], v[1], v[2], to_yup(p.norm), mat);
            if v.len() == 4 {
                w.push(v[3], v[2], v[1], to_yup(p.norm2), mat);
            }
        }
        w
    }

    fn material_id(&mut self, name: &str) -> u8 {
        if let Some(i) = self.material_names.iter().position(|n| n == name) {
            return i as u8;
        }
        self.material_names.push(name.to_owned());
        (self.material_names.len() - 1) as u8
    }

    fn push_water(&mut self, a: Vec3, b: Vec3, c: Vec3) {
        let geo = (b - a).cross(c - a);
        if geo.length_squared() < 1e-6 {
            return;
        }
        let (min, max) = (a.min(b).min(c), a.max(b).max(c));
        let id = self.water.len() as u32;
        self.water.push(Tri { a, b, c, n: geo.normalize(), min_y: min.y, max_y: max.y, mat: 0 });
        for cx in cell(min.x)..=cell(max.x) {
            for cz in cell(min.z)..=cell(max.z) {
                self.water_grid.entry((cx, cz)).or_default().push(id);
            }
        }
    }

    /// Height of the water surface over `(x, z)`, if there is water there.
    pub fn water_level_at(&self, x: f32, z: f32) -> Option<f32> {
        let p = Vec2::new(x, z);
        let mut best: Option<f32> = None;
        for t in self.water_grid.get(&(cell(x), cell(z)))?.iter().map(|&i| &self.water[i as usize]) {
            let (_, inside) = closest_on_tri_2d(p, Vec2::new(t.a.x, t.a.z), Vec2::new(t.b.x, t.b.z), Vec2::new(t.c.x, t.c.z));
            if inside {
                let y = if t.n.y.abs() > 0.1 { t.a.y - (t.n.x * (x - t.a.x) + t.n.z * (z - t.a.z)) / t.n.y } else { t.max_y };
                if best.is_none_or(|b| y > b) {
                    best = Some(y);
                }
            }
        }
        best
    }

    /// What the highest surface under `(x, z)` that is not above `max_y` is made of (`stone`, `wood`, ...).
    pub fn floor_material(&self, x: f32, z: f32, max_y: f32) -> Option<&str> {
        let (_, mat) = self.floor_surface(x, z, max_y)?;
        Some(self.material_names.get(mat as usize).map_or("unknown", String::as_str))
    }

    /// Build a world from explicit y-up triangles (mainly for tests and tools).
    pub fn from_triangles(tris: impl IntoIterator<Item = [Vec3; 3]>) -> Self {
        let mut w = CollisionWorld::default();
        w.material_names.push("unknown".to_owned());
        for [a, b, c] in tris {
            w.push(a, b, c, Vec3::ZERO, 0);
        }
        w
    }

    /// Like [`CollisionWorld::from_triangles`], with a material name for each triangle (tests and tools).
    pub fn from_material_triangles<'a>(tris: impl IntoIterator<Item = ([Vec3; 3], &'a str)>) -> Self {
        let mut w = CollisionWorld::default();
        w.material_names.push("unknown".to_owned());
        for ([a, b, c], material) in tris {
            let mat = w.material_id(material);
            w.push(a, b, c, Vec3::ZERO, mat);
        }
        w
    }

    fn push(&mut self, a: Vec3, b: Vec3, c: Vec3, stored_normal: Vec3, mat: u8) {
        let geo = (b - a).cross(c - a);
        if geo.length_squared() < 1e-6 {
            return; // degenerate
        }
        let mut n = geo.normalize();
        if stored_normal.length_squared() > 0.25 && n.dot(stored_normal) < 0.0 {
            n = -n;
        }
        let id = self.tris.len() as u32;
        let (min, max) = (a.min(b).min(c), a.max(b).max(c));
        if n.y > 0.95 {
            let area = geo.length() * 0.5;
            if self.fallback_spawn.is_none_or(|(best, _)| area > best) {
                self.fallback_spawn = Some((area, (a + b + c) / 3.0));
            }
        }
        self.tris.push(Tri { a, b, c, n, min_y: min.y, max_y: max.y, mat });
        for cx in cell(min.x)..=cell(max.x) {
            for cz in cell(min.z)..=cell(max.z) {
                self.grid.entry((cx, cz)).or_default().push(id);
            }
        }
    }

    /// `preferred` (feet position) if there is ground under it, otherwise a safe fallback location.
    pub fn spawn_point(&self, preferred: Vec3) -> Vec3 {
        match self.floor_height(preferred.x, preferred.z, preferred.y + STEP_HEIGHT) {
            Some(f) if f >= preferred.y - 1000.0 => preferred,
            _ => self.fallback_spawn.map_or(preferred, |(_, p)| p),
        }
    }

    /// Add a solid object from y-up world-space triangles. Returns `None` if it has no usable
    /// triangles. Obstacles start enabled.
    pub fn add_obstacle(&mut self, tris: impl IntoIterator<Item = [Vec3; 3]>) -> Option<ObstacleId> {
        let mut list = Vec::new();
        let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for [a, b, c] in tris {
            let geo = (b - a).cross(c - a);
            if geo.length_squared() < 1e-6 {
                continue;
            }
            let n = geo.normalize();
            let (lo, hi) = (a.min(b).min(c), a.max(b).max(c));
            min = min.min(lo);
            max = max.max(hi);
            list.push(Tri { a, b, c, n, min_y: lo.y, max_y: hi.y, mat: 0 });
        }
        if list.is_empty() {
            return None;
        }
        self.obstacles.push(Obstacle { tris: list, min, max });
        self.obstacle_enabled.push(AtomicBool::new(true));
        Some(ObstacleId(self.obstacles.len() - 1))
    }

    pub fn set_obstacle_enabled(&self, id: ObstacleId, enabled: bool) {
        self.obstacle_enabled[id.0].store(enabled, Ordering::Relaxed);
    }

    pub fn obstacle_enabled(&self, id: ObstacleId) -> bool {
        self.obstacle_enabled[id.0].load(Ordering::Relaxed)
    }

    /// Add a character cylinder (feet position `base`). Returns its id.
    pub fn add_cylinder(&mut self, base: Vec3, radius: f32, height: f32) -> CylinderId {
        let list = self.cylinders.get_mut().expect("cylinder lock");
        list.push(Cylinder { base, radius, height, enabled: true });
        CylinderId(list.len() - 1)
    }

    /// Move a character and/or switch it on or off (a dead or hidden character does not block).
    pub fn set_cylinder(&self, id: CylinderId, base: Vec3, enabled: bool) {
        if let Some(c) = self.cylinders.write().expect("cylinder lock").get_mut(id.0) {
            c.base = base;
            c.enabled = enabled;
        }
    }

    pub fn cylinder(&self, id: CylinderId) -> Option<Cylinder> {
        self.cylinders.read().expect("cylinder lock").get(id.0).copied()
    }

    pub fn obstacle_count(&self) -> usize {
        self.obstacles.len()
    }

    pub fn triangle_count(&self) -> usize {
        self.tris.len()
    }

    /// Triangles overlapping the XZ rectangle, without duplicates.
    fn query(&self, min: Vec2, max: Vec2) -> Vec<&Tri> {
        let mut ids: Vec<u32> = Vec::new();
        for cx in cell(min.x)..=cell(max.x) {
            for cz in cell(min.y)..=cell(max.y) {
                if let Some(v) = self.grid.get(&(cx, cz)) {
                    ids.extend_from_slice(v);
                }
            }
        }
        ids.sort_unstable();
        ids.dedup();
        ids.into_iter().map(|i| &self.tris[i as usize]).collect()
    }

    /// Height of the highest walkable surface under `(x, z)` that is not above `max_y`.
    pub fn floor_height(&self, x: f32, z: f32, max_y: f32) -> Option<f32> {
        self.floor_surface(x, z, max_y).map(|(y, _)| y)
    }

    /// The highest walkable surface under `(x, z)` that is not above `max_y`: its height and material.
    fn floor_surface(&self, x: f32, z: f32, max_y: f32) -> Option<(f32, u8)> {
        let p = Vec2::new(x, z);
        let mut best: Option<(f32, u8)> = None;
        let mut consider = |t: &Tri| {
            if t.n.y.abs() < SUPPORT_NORMAL_Y || t.min_y > max_y + 1.0 {
                return;
            }
            let (_, inside) = closest_on_tri_2d(p, Vec2::new(t.a.x, t.a.z), Vec2::new(t.b.x, t.b.z), Vec2::new(t.c.x, t.c.z));
            if !inside {
                return;
            }
            // Plane: n . (q - a) = 0 solved for y.
            let y = t.a.y - (t.n.x * (x - t.a.x) + t.n.z * (z - t.a.z)) / t.n.y;
            if y <= max_y && best.is_none_or(|b| y > b.0) {
                best = Some((y, t.mat));
            }
        };
        for t in self.query(p, p) {
            consider(t);
        }
        // Solid entities are ground too: a trapdoor plugging a hole in the level, or a low crate.
        for (o, enabled) in self.obstacles.iter().zip(&self.obstacle_enabled) {
            if o.min.x > x || o.max.x < x || o.min.z > z || o.max.z < z || o.min.y > max_y + 1.0 || !enabled.load(Ordering::Relaxed) {
                continue;
            }
            for t in &o.tris {
                consider(t);
            }
        }
        best
    }

    /// The first surface a ray meets within `max_t` (level geometry and solid entities, whichever way they face).
    pub fn raycast(&self, origin: Vec3, dir: Vec3, max_t: f32) -> Option<RayHit> {
        let dir = dir.normalize_or_zero();
        if dir == Vec3::ZERO {
            return None;
        }
        // Candidate triangles: every grid cell along the ray's shadow on the ground.
        let steps = ((max_t / (CELL * 0.5)).ceil() as usize).max(1);
        let mut best: Option<RayHit> = None;
        let mut test = |t: &Tri| {
            if let Some((dist, n)) = ray_triangle(origin, dir, t)
                && dist <= max_t
                && best.is_none_or(|b| dist < b.t)
            {
                best = Some(RayHit { t: dist, normal: n });
            }
        };
        let mut seen: Vec<u32> = Vec::new();
        for i in 0..=steps {
            let p = origin + dir * (max_t * i as f32 / steps as f32);
            if let Some(ids) = self.grid.get(&(cell(p.x), cell(p.z))) {
                for &id in ids {
                    if !seen.contains(&id) {
                        seen.push(id);
                        test(&self.tris[id as usize]);
                    }
                }
            }
        }
        let (lo, hi) = (origin.min(origin + dir * max_t), origin.max(origin + dir * max_t));
        for (o, enabled) in self.obstacles.iter().zip(&self.obstacle_enabled) {
            if o.max.cmpge(lo).all() && o.min.cmple(hi).all() && enabled.load(Ordering::Relaxed) {
                for t in &o.tris {
                    test(t);
                }
            }
        }
        best
    }

    /// Highest floor under the cylinder footprint (centre plus four points on its rim).
    fn footprint_floor(&self, pos: Vec3, max_y: f32) -> Option<f32> {
        self.footprint_floor_r(pos, max_y, PLAYER_RADIUS)
    }

    fn footprint_floor_r(&self, pos: Vec3, max_y: f32, radius: f32) -> Option<f32> {
        let r = radius * 0.7;
        [(0.0, 0.0), (r, 0.0), (-r, 0.0), (0.0, r), (0.0, -r)]
            .iter()
            .filter_map(|(dx, dz)| self.floor_height(pos.x + dx, pos.z + dz, max_y))
            .max_by(f32::total_cmp)
    }

    /// Lowest flat surface (floor of the storey above, ceiling, beam, table top) over the footprint whose height
    /// is in `(from_y, to_y]`: what a head reaching up to `to_y` would hit.
    fn ceiling_above(&self, pos: Vec3, from_y: f32, to_y: f32) -> Option<f32> {
        let r = PLAYER_RADIUS * 0.5;
        let mut best: Option<f32> = None;
        for (dx, dz) in [(0.0, 0.0), (r, 0.0), (-r, 0.0), (0.0, r), (0.0, -r)] {
            let (x, z) = (pos.x + dx, pos.z + dz);
            let p = Vec2::new(x, z);
            let mut consider = |t: &Tri| {
                if t.n.y.abs() < CEILING_NORMAL_Y || t.max_y <= from_y || t.min_y > to_y {
                    return;
                }
                let (_, inside) = closest_on_tri_2d(p, Vec2::new(t.a.x, t.a.z), Vec2::new(t.b.x, t.b.z), Vec2::new(t.c.x, t.c.z));
                if !inside {
                    return;
                }
                let y = t.a.y - (t.n.x * (x - t.a.x) + t.n.z * (z - t.a.z)) / t.n.y;
                if y > from_y && y <= to_y && best.is_none_or(|b| y < b) {
                    best = Some(y);
                }
            };
            for t in self.query(p, p) {
                consider(t);
            }
            for (o, enabled) in self.obstacles.iter().zip(&self.obstacle_enabled) {
                if o.min.x > x || o.max.x < x || o.min.z > z || o.max.z < z || o.max.y <= from_y || o.min.y > to_y || !enabled.load(Ordering::Relaxed) {
                    continue;
                }
                for t in &o.tris {
                    consider(t);
                }
            }
        }
        best
    }

    /// Free height above `base_y` at `pos` before a ceiling (infinite if there is none within `reach`).
    fn headroom(&self, pos: Vec3, base_y: f32, reach: f32) -> f32 {
        self.ceiling_above(pos, base_y + STEP_HEIGHT, base_y + reach).map_or(f32::INFINITY, |c| c - base_y)
    }

    /// Push the cylinder at `pos` out of nearby steep triangles (and enabled obstacles) in the
    /// horizontal plane. `prev` is where the player was before this move: it decides which side of a
    /// wall the player belongs on, so a wall can never push them through itself. Obstacles whose top
    /// is below `pos.y + step` are low enough to step over (and are left to the floor pass).
    fn push_out_of_walls(&self, pos: &mut Vec3, prev: Vec3, step: f32, height: f32) {
        self.push_out_of_walls_ex(pos, prev, step, height, PLAYER_RADIUS, None, &[]);
    }

    /// [`CollisionWorld::push_out_of_walls`] for a body of any radius. `skip` is a character's own cylinder (it
    /// must not push itself) and `extra` further circles (centre in x/z, radius) to keep out of, such as the player.
    #[allow(clippy::too_many_arguments)]
    fn push_out_of_walls_ex(&self, pos: &mut Vec3, prev: Vec3, step: f32, height: f32, r: f32, skip: Option<CylinderId>, extra: &[(Vec2, f32)]) {
        for _ in 0..8 {
            let (lo, hi) = (pos.y + step, pos.y + height);
            let center = Vec2::new(pos.x, pos.z);
            let mut push: Option<Vec2> = None;

            let mut consider = |t: &Tri, solid_both_ways: bool| {
                if t.max_y < lo || t.min_y > hi {
                    return;
                }
                // Steep triangles block. Level geometry has oriented normals (floors face up, ceilings
                // down); entity triangles are solid however they are oriented.
                let steep = if solid_both_ways { t.n.y.abs() < 0.9 } else { t.n.y < WALKABLE_NORMAL_Y && t.n.y > -0.95 };
                if !steep {
                    return;
                }
                let a = Vec2::new(t.a.x, t.a.z);
                let (q, inside) = closest_on_tri_2d(center, a, Vec2::new(t.b.x, t.b.z), Vec2::new(t.c.x, t.c.z));
                let n_h = Vec2::new(t.n.x, t.n.z).normalize_or_zero();
                let (dir, dist) = if inside {
                    // The centre is within the wall's footprint (wide for slanted walls): leave on
                    // the side the player came from, whatever the stored normal says.
                    let side = n_h.dot(Vec2::new(prev.x, prev.z) - a);
                    (if side >= 0.0 { n_h } else { -n_h }, 0.0)
                } else {
                    let d = center - q;
                    let l = d.length();
                    (if l > 1e-5 { d / l } else { n_h }, l)
                };
                if dist < r && dir != Vec2::ZERO {
                    let amount = r - dist + 0.01;
                    if push.is_none_or(|p| amount > p.length()) {
                        push = Some(dir * amount);
                    }
                }
            };

            for t in self.query(center - Vec2::splat(r), center + Vec2::splat(r)) {
                consider(t, false);
            }
            for (o, enabled) in self.obstacles.iter().zip(&self.obstacle_enabled) {
                if !enabled.load(Ordering::Relaxed)
                    || o.max.x < center.x - r
                    || o.min.x > center.x + r
                    || o.max.z < center.y - r
                    || o.min.z > center.y + r
                    || o.max.y < lo
                    || o.min.y > hi
                {
                    continue;
                }
                for t in &o.tris {
                    consider(t, true);
                }
            }

            // Characters: circles around their cylinders, whichever side the player is on.
            let mut circle = |centre: Vec2, radius: f32| {
                let d = center - centre;
                let (len, reach) = (d.length(), r + radius);
                if len < reach {
                    let away = if len > 1e-4 { d / len } else { (Vec2::new(prev.x, prev.z) - centre).normalize_or(Vec2::X) };
                    let amount = reach - len + 0.01;
                    if push.is_none_or(|p| amount > p.length()) {
                        push = Some(away * amount);
                    }
                }
            };
            for (i, c) in self.cylinders.read().expect("cylinder lock").iter().enumerate() {
                if skip.is_some_and(|s| s.0 == i) || !c.enabled || c.base.y + c.height < lo || c.base.y > hi {
                    continue;
                }
                circle(Vec2::new(c.base.x, c.base.z), c.radius);
            }
            for &(centre, radius) in extra {
                circle(centre, radius);
            }

            match push {
                Some(p) => {
                    pos.x += p.x;
                    pos.z += p.y;
                }
                None => break,
            }
        }
    }
}

/// The outcome of [`CollisionWorld::move_character`].
#[derive(Debug, Clone, PartialEq)]
pub struct CharacterMove {
    /// Where the feet ended up (y-up).
    pub pos: Vec3,
    /// A wall or another body stopped most of the requested movement.
    pub blocked: bool,
    /// There is floor under the feet (within the step height).
    pub on_ground: bool,
    /// The solid entities (doors, ...) the body is touching.
    pub touched: Vec<ObstacleId>,
}

/// Fall speed of a character that has walked off an edge, units per second (the engine adds 1.5 per ms).
const CHARACTER_FALL_SPEED: f32 = 1500.0;

impl CollisionWorld {
    /// Move a walking character (an NPC) by the horizontal displacement `disp`: walls and other bodies push it out,
    /// it climbs steps of up to [`STEP_HEIGHT`], follows the floor down, and falls when there is none.
    #[allow(clippy::too_many_arguments)]
    pub fn move_character(&self, feet: Vec3, disp: Vec2, radius: f32, height: f32, dt: f32, skip: Option<CylinderId>, extra: &[(Vec2, f32)]) -> CharacterMove {
        let wanted = feet + Vec3::new(disp.x, 0.0, disp.y);
        let mut pos = wanted;
        self.push_out_of_walls_ex(&mut pos, feet, STEP_HEIGHT, height, radius, skip, extra);
        let done = Vec2::new(pos.x - feet.x, pos.z - feet.z);
        let blocked = disp.length() > 0.05 && done.dot(disp) < 0.5 * disp.length_squared();
        let floor = self.footprint_floor_r(pos, pos.y + STEP_HEIGHT, radius);
        let on_ground = match floor {
            Some(f) if f >= pos.y - STEP_HEIGHT => {
                pos.y = f;
                true
            }
            Some(f) => {
                pos.y = (pos.y - CHARACTER_FALL_SPEED * dt).max(f);
                pos.y <= f
            }
            None => false,
        };
        let touched = self.obstacles_touching(pos, radius, height, 15.0);
        CharacterMove { pos, blocked, on_ground, touched }
    }

    /// The enabled solid entities within `margin` of a body standing at `pos`.
    pub fn obstacles_touching(&self, pos: Vec3, radius: f32, height: f32, margin: f32) -> Vec<ObstacleId> {
        let reach = radius + margin;
        let centre = Vec2::new(pos.x, pos.z);
        let mut out = Vec::new();
        for (i, (o, enabled)) in self.obstacles.iter().zip(&self.obstacle_enabled).enumerate() {
            if !enabled.load(Ordering::Relaxed)
                || o.max.x < pos.x - reach
                || o.min.x > pos.x + reach
                || o.max.z < pos.z - reach
                || o.min.z > pos.z + reach
                || o.max.y < pos.y + STEP_HEIGHT
                || o.min.y > pos.y + height
            {
                continue;
            }
            let near = o.tris.iter().any(|t| {
                if t.max_y < pos.y + STEP_HEIGHT || t.min_y > pos.y + height {
                    return false;
                }
                let (q, inside) = closest_on_tri_2d(centre, Vec2::new(t.a.x, t.a.z), Vec2::new(t.b.x, t.b.z), Vec2::new(t.c.x, t.c.z));
                inside || q.distance(centre) < reach
            });
            if near {
                out.push(ObstacleId(i));
            }
        }
        out
    }
}

/// What a movement key press means in the original: the direction of the push and which animation (and so
/// which speed) goes with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MoveKind {
    #[default]
    None,
    Forward,
    Backward,
    Strafe,
}

/// One frame of player input.
#[derive(Debug, Clone, Copy, Default)]
pub struct MoveInput {
    /// Horizontal direction the keys push toward in world space (unit length), or zero.
    pub dir: Vec2,
    pub kind: MoveKind,
    /// Sneak (Shift in the original): the slower walking animations.
    pub stealth: bool,
    pub crouch: bool,
    pub jump: bool,
}

impl MoveInput {
    /// Push toward `dir` as if walking forward (used by tools and tests).
    pub fn toward(dir: Vec2) -> Self {
        let dir = dir.normalize_or_zero();
        MoveInput { dir, kind: if dir == Vec2::ZERO { MoveKind::None } else { MoveKind::Forward }, ..Default::default() }
    }

    /// Combine the movement keys the way the engine does. `yaw` is the view direction in radians, 0 looking
    /// along -Z and increasing counter-clockwise. Moving forward and sideways at once is a little weaker
    /// forward (0.8) and strafing counts 6 against 10 forward / 5 backward; only the resulting direction
    /// matters, the speed comes from the animation that goes with the dominant key.
    pub fn from_keys(yaw: f32, forward: bool, backward: bool, left: bool, right: bool) -> Self {
        let f = Vec2::new(-yaw.sin(), -yaw.cos());
        let r = Vec2::new(yaw.cos(), -yaw.sin());
        let strafing = left || right;
        let diagonal = if strafing { 0.8 } else { 1.0 };
        let mut tm = Vec2::ZERO;
        if backward {
            tm -= f * 5.0 * diagonal;
        }
        if forward {
            tm += f * 10.0 * diagonal;
        }
        if left {
            tm -= r * 6.0;
        }
        if right {
            tm += r * 6.0;
        }
        let kind = if forward {
            MoveKind::Forward
        } else if backward {
            MoveKind::Backward
        } else if left != right {
            MoveKind::Strafe
        } else {
            MoveKind::None
        };
        MoveInput { dir: tm.normalize_or_zero(), kind, ..Default::default() }
    }
}

/// Where the player is in a crouch. The collision cylinder only shrinks once the crouch-in animation has
/// finished, and while either animation plays the body moves slowly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Stance {
    Standing,
    /// Crouch-in animation playing; milliseconds left.
    GoingDown(f32),
    Crouched,
    /// Crouch-out animation playing; milliseconds left.
    GettingUp(f32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JumpPhase {
    None,
    /// Rising for 200 ms; milliseconds so far.
    Ascending(f32),
    /// Falling after a jump or off a ledge; gravity is weak.
    Descending,
}

/// First-person player body: feet position, vertical velocity, ground state.
#[derive(Debug, Clone, Copy)]
pub struct Player {
    pub feet: Vec3,
    /// Vertical velocity in units/s, positive up.
    pub vel_y: f32,
    pub on_ground: bool,
    /// Horizontal velocity in units/s. It builds up while keys push and is damped every step.
    pub vel_h: Vec2,
    /// Last position that had solid ground under it (the engine's "last valid position").
    pub last_ground: Vec3,
    /// How many times the player fell out of the world and was put back on `last_ground`.
    pub rescues: u32,
    pub stance: Stance,
    pub phase: JumpPhase,
    /// In a (weak-gravity) fall; the fall height is measured from `fall_start_y`.
    pub falling: bool,
    fall_start_y: f32,
    clock_ms: f32,
    last_landing_ms: f32,
    jump_held: bool,
    jump_request_ms: Option<f32>,
    landed_fall: Option<f32>,
    /// Distance walked since the last footstep (units).
    walked: f32,
    steps: u32,
    jumped: bool,
    /// Move exactly as the original does: a jump that rises 130 units in a fifth of a second and then floats down,
    /// three times the running speed in the air, a dead stop on landing, and a crouch that takes 0.7 s before the
    /// body is any lower. Off by default: the owner of this project asked for a jump with ordinary gravity that
    /// keeps its speed, and a crouch that answers at once.
    pub classic: bool,
}

/// Distance walked between footsteps (`STEP_DISTANCE`).
const STEP_DISTANCE: f32 = 120.0;

/// Falling this far below the last solid ground means the player left the level.
const RESCUE_DEPTH: f32 = 3000.0;

/// Damage for a fall of `height` units (zero up to [`SAFE_FALL_HEIGHT`]).
pub fn fall_damage(height: f32) -> f32 {
    ((height - SAFE_FALL_HEIGHT) / 15.0).max(0.0)
}

impl Player {
    pub fn new(feet: Vec3) -> Self {
        Player {
            feet,
            vel_y: 0.0,
            on_ground: false,
            vel_h: Vec2::ZERO,
            last_ground: feet,
            rescues: 0,
            stance: Stance::Standing,
            phase: JumpPhase::None,
            falling: false,
            fall_start_y: feet.y,
            clock_ms: 0.0,
            last_landing_ms: f32::NEG_INFINITY,
            jump_held: false,
            jump_request_ms: None,
            landed_fall: None,
            walked: 0.0,
            steps: 0,
            jumped: false,
            classic: false,
        }
    }

    /// Height of the collision cylinder right now.
    pub fn height(&self) -> f32 {
        let low = match self.stance {
            Stance::Crouched => true,
            // The original only gets lower once the crouch-in animation has ended.
            Stance::GoingDown(_) => !self.classic,
            _ => false,
        };
        if low { CROUCH_HEIGHT } else { PLAYER_HEIGHT }
    }

    /// How long ducking or standing up takes.
    fn crouch_ms(&self) -> f32 {
        if self.classic { CROUCH_ANIM_MS } else { QUICK_CROUCH_MS }
    }

    pub fn is_crouching(&self) -> bool {
        self.stance != Stance::Standing
    }

    /// Eye height above the feet; it follows the crouch animations.
    pub fn eye_height(&self) -> f32 {
        let total = self.crouch_ms();
        let lerp = |from: f32, to: f32, left: f32| from + (to - from) * (1.0 - left / total);
        match self.stance {
            Stance::Standing => EYE_HEIGHT,
            Stance::GoingDown(left) => lerp(EYE_HEIGHT, CROUCH_EYE_HEIGHT, left),
            Stance::Crouched => CROUCH_EYE_HEIGHT,
            Stance::GettingUp(left) => lerp(CROUCH_EYE_HEIGHT, EYE_HEIGHT, left),
        }
    }

    pub fn eye(&self) -> Vec3 {
        self.feet + Vec3::Y * self.eye_height()
    }

    /// Footsteps taken since the last call: one for every 120 units walked on the ground (twice as often crouched).
    pub fn take_steps(&mut self) -> u32 {
        std::mem::take(&mut self.steps)
    }

    /// Whether the player left the ground by jumping since the last call.
    pub fn take_jump(&mut self) -> bool {
        std::mem::take(&mut self.jumped)
    }

    /// The height of the last landing that hurt (falls above [`SAFE_FALL_HEIGHT`]); each is reported once.
    pub fn take_landing(&mut self) -> Option<f32> {
        self.landed_fall.take()
    }

    /// Advance by `dt` seconds.
    pub fn step(&mut self, world: &CollisionWorld, dt: f32, input: MoveInput) {
        let n = ((dt / MAX_SUBSTEP_SECS).ceil() as usize).max(1);
        let sub_ms = dt * 1000.0 / n as f32;
        if input.jump && !self.jump_held {
            self.jump_request_ms = Some(0.0);
        }
        self.jump_held = input.jump;
        let before = self.feet;
        for _ in 0..n {
            self.substep(world, sub_ms, input);
        }
        // Footsteps: only while walking on the ground, and not in the air or falling.
        if self.on_ground && self.phase == JumpPhase::None && !self.falling {
            let moved = (self.feet - before).length();
            self.walked += if self.is_crouching() { moved * 2.0 } else { moved };
            while self.walked >= STEP_DISTANCE {
                self.walked -= STEP_DISTANCE;
                self.steps += 1;
            }
        }
        if self.on_ground {
            self.last_ground = self.feet;
        } else if self.feet.y < self.last_ground.y - RESCUE_DEPTH {
            self.feet = self.last_ground;
            self.vel_y = 0.0;
            self.vel_h = Vec2::ZERO;
            self.on_ground = true;
            self.phase = JumpPhase::None;
            self.falling = false;
            self.rescues += 1;
        }
    }

    /// Strength of the push this step, from the animation the engine would be playing.
    fn push_scale(&self, input: &MoveInput) -> f32 {
        if self.phase != JumpPhase::None && !self.classic {
            // In the air the body keeps about its running speed, a little more going forward.
            return match input.kind {
                MoveKind::Backward => SCALE_RUN * 0.8,
                MoveKind::Forward => SCALE_RUN * 1.3,
                MoveKind::Strafe => SCALE_RUN,
                MoveKind::None => 0.0,
            };
        }
        if self.phase != JumpPhase::None {
            return match input.kind {
                MoveKind::Backward => SCALE_AIR_BACKWARD,
                MoveKind::Forward => SCALE_AIR_FORWARD,
                MoveKind::Strafe => SCALE_AIR_STRAFE,
                MoveKind::None => 0.0,
            };
        }
        match self.stance {
            Stance::GoingDown(_) | Stance::GettingUp(_) if self.classic => SCALE_CROUCH_TRANSITION,
            Stance::GoingDown(_) | Stance::GettingUp(_) => SCALE_CROUCH_WALK,
            Stance::Crouched => match input.kind {
                MoveKind::Strafe => SCALE_CROUCH_STRAFE,
                _ => SCALE_CROUCH_WALK,
            },
            Stance::Standing if input.stealth => match input.kind {
                MoveKind::Strafe => SCALE_WALK_STRAFE,
                _ => SCALE_WALK,
            },
            Stance::Standing => SCALE_RUN,
        }
    }

    /// Landing slows the push for a while: half strength for 300 ms, then the engine's own ramp, which
    /// (as in the original) overshoots below zero before it reaches full strength again at 600 ms.
    fn landing_recovery(&self) -> f32 {
        if !self.classic {
            return 1.0;
        }
        let since = self.clock_ms - self.last_landing_ms;
        if since >= 600.0 {
            return 1.0;
        }
        let mut mul = 0.5;
        if since >= 300.0 {
            mul += (300.0 - since) / 300.0;
        }
        mul.min(1.0)
    }

    fn substep(&mut self, world: &CollisionWorld, dt_ms: f32, input: MoveInput) {
        let dt = dt_ms / 1000.0;
        self.clock_ms += dt_ms;

        // Crouching. The body stays crouched while there is no room to stand.
        let can_stand = world.headroom(self.feet, self.feet.y, PLAYER_HEIGHT) >= PLAYER_HEIGHT;
        let want_crouch = input.crouch || !can_stand;
        let crouch_ms = self.crouch_ms();
        // (Turning round half-way carries on from where the body is, so that a tap does not make it jump.)
        let rest = |left: f32| if self.classic { crouch_ms } else { crouch_ms - left };
        self.stance = match (self.stance, want_crouch) {
            (Stance::Standing, true) => Stance::GoingDown(crouch_ms),
            (Stance::GoingDown(left), false) => Stance::GettingUp(rest(left)),
            (Stance::GoingDown(left), true) if left > dt_ms => Stance::GoingDown(left - dt_ms),
            (Stance::GoingDown(_), true) => Stance::Crouched,
            (Stance::Crouched, false) => Stance::GettingUp(crouch_ms),
            (Stance::GettingUp(left), true) => Stance::GoingDown(rest(left)),
            (Stance::GettingUp(left), false) if left > dt_ms => Stance::GettingUp(left - dt_ms),
            (Stance::GettingUp(_), false) => Stance::Standing,
            (stance, _) => stance,
        };

        // Jump: needs the ground, and room to stand if the player was crouching.
        if let Some(t) = &mut self.jump_request_ms {
            *t += dt_ms;
            if *t > JUMP_REQUEST_MS {
                self.jump_request_ms = None;
            }
        }
        if self.jump_request_ms.is_some() && self.on_ground && self.phase == JumpPhase::None {
            self.jump_request_ms = None;
            if self.stance == Stance::Standing || can_stand {
                self.stance = Stance::Standing;
                self.on_ground = false;
                self.jumped = true;
                if self.classic {
                    self.phase = JumpPhase::Ascending(0.0);
                    self.vel_y = 0.0;
                } else {
                    // Thrown up and pulled back down by the same gravity all the way.
                    self.phase = JumpPhase::Descending;
                    self.falling = true;
                    self.fall_start_y = self.feet.y;
                    self.vel_y = (2.0 * QUICK_GRAVITY * QUICK_JUMP_HEIGHT).sqrt();
                }
            }
        }

        // Horizontal: damping, then the push.
        let damp = (1.0 - DAMPING_PER_MS * dt_ms).max(0.0);
        self.vel_h *= damp;
        if self.vel_h.x.abs() < 1.0 {
            self.vel_h.x = 0.0;
        }
        if self.vel_h.y.abs() < 1.0 {
            self.vel_h.y = 0.0;
        }
        if input.dir != Vec2::ZERO {
            let push = self.push_scale(&input) * self.landing_recovery();
            self.vel_h += input.dir * push * dt_ms * 1000.0;
        }

        // Vertical: gravity, unless on the ground or rising.
        let mut rise = 0.0;
        match self.phase {
            JumpPhase::Ascending(elapsed) => {
                let now = (elapsed + dt_ms).min(JUMP_RISE_MS);
                rise = (now - elapsed) / JUMP_RISE_MS * JUMP_RISE;
                self.phase = if now >= JUMP_RISE_MS { self.start_fall(rise) } else { JumpPhase::Ascending(now) };
            }
            _ if self.on_ground => self.vel_y = 0.0,
            _ if self.classic => self.vel_y -= if self.falling { FALL_GRAVITY } else { WORLD_GRAVITY } * dt,
            _ => self.vel_y = (self.vel_y - QUICK_GRAVITY * dt).max(-QUICK_FALL_SPEED_MAX),
        }

        // A drop that gets fast enough and has nothing close below counts as a fall.
        if !self.on_ground && self.phase == JumpPhase::None && self.vel_y < -FALL_TRIGGER_SPEED {
            let gap = world.floor_height(self.feet.x, self.feet.z, self.feet.y).map_or(f32::INFINITY, |f| self.feet.y - f);
            if gap > 80.0 {
                if self.classic {
                    self.phase = self.start_fall(0.0);
                } else {
                    // The fall is measured from here; its speed is kept.
                    self.falling = true;
                    self.fall_start_y = self.feet.y;
                    self.phase = JumpPhase::Descending;
                }
            }
        }

        let was_on_ground = self.on_ground;
        self.move_body(world, self.vel_h * dt, rise, dt);
        // A fall is as high as the highest point reached.
        if self.falling && !self.classic {
            self.fall_start_y = self.fall_start_y.max(self.feet.y);
        }
        if !was_on_ground && self.on_ground {
            self.land();
        }
    }

    /// Begin a fall from `extra` above the feet (the rise still to be applied this step).
    fn start_fall(&mut self, extra: f32) -> JumpPhase {
        self.falling = true;
        self.fall_start_y = self.feet.y + extra;
        self.vel_y = 0.0;
        JumpPhase::Descending
    }

    fn land(&mut self) {
        self.phase = JumpPhase::None;
        self.last_landing_ms = self.clock_ms;
        if self.falling {
            if self.classic {
                self.vel_h = Vec2::ZERO;
            }
            let height = self.fall_start_y - self.feet.y;
            if height > SAFE_FALL_HEIGHT {
                self.landed_fall = Some(height);
            }
            self.falling = false;
        }
    }

    /// Move the body by the horizontal displacement `disp` and the vertical `rise`, resolving walls, ceilings
    /// and floors.
    fn move_body(&mut self, world: &CollisionWorld, disp: Vec2, rise: f32, dt: f32) {
        let h = self.height();
        let old = self.feet;
        let mut pos = old + Vec3::new(disp.x, 0.0, disp.y);
        // Only a grounded player can step over low obstacles; a jump cannot hop over a parapet.
        world.push_out_of_walls(&mut pos, old, if self.on_ground { STEP_HEIGHT } else { 5.0 }, h);

        // Too little headroom stops the body like a wall, unless it is already that low and not getting lower.
        let on_ground = self.on_ground;
        let base = |p: Vec3| if on_ground { world.footprint_floor(p, old.y + STEP_HEIGHT).unwrap_or(old.y) } else { old.y };
        let room = |p: Vec3| world.headroom(p, base(p), h);
        let before = room(old);
        let fits = |p: Vec3| {
            let r = room(p);
            r >= h || r >= before - 0.5
        };
        if (pos.x != old.x || pos.z != old.z) && !fits(pos) {
            let slide_x = Vec3::new(pos.x, pos.y, old.z);
            let slide_z = Vec3::new(old.x, pos.y, pos.z);
            pos = if fits(slide_x) {
                slide_x
            } else if fits(slide_z) {
                slide_z
            } else {
                Vec3::new(old.x, pos.y, old.z)
            };
        }

        // Highest floor under the footprint that the player could step onto from here.
        let floor = world.footprint_floor(pos, pos.y + STEP_HEIGHT);

        if rise > 0.0 {
            pos.y += rise;
            self.on_ground = false;
            if let Some(c) = world.ceiling_above(pos, old.y + STEP_HEIGHT, pos.y + h)
                && c < pos.y + h
            {
                // Bumped the head: the rise ends and the fall begins from here.
                pos.y = (c - h).max(old.y);
                self.feet = pos;
                self.phase = self.start_fall(0.0);
                return;
            }
            self.feet = pos;
            return;
        }
        if self.on_ground && self.vel_y <= 0.0 {
            // Standing: follow the floor up and down (stairs) within the step height.
            if let Some(f) = floor
                && f >= pos.y - STEP_HEIGHT
            {
                pos.y = f;
                self.vel_y = 0.0;
                self.feet = pos;
                return;
            }
            self.on_ground = false;
        }

        // Going up (a jump thrown by its speed): the head may meet a ceiling, and then the way is down.
        if self.vel_y > 0.0 {
            pos.y += self.vel_y * dt;
            if let Some(c) = world.ceiling_above(pos, old.y + STEP_HEIGHT, pos.y + h)
                && c < pos.y + h
            {
                pos.y = (c - h).max(old.y);
                self.vel_y = 0.0;
            }
            self.on_ground = false;
            self.feet = pos;
            return;
        }

        // Falling: land on a floor we reach or cross during this step.
        pos.y += self.vel_y * dt;
        match floor {
            Some(f) if f >= pos.y && self.vel_y <= 0.0 => {
                pos.y = f;
                self.vel_y = 0.0;
                self.on_ground = true;
            }
            _ => self.on_ground = false,
        }
        self.feet = pos;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(a: Vec3, b: Vec3, c: Vec3, d: Vec3) -> [[Vec3; 3]; 2] {
        [[a, b, c], [a, c, d]]
    }

    /// A flat floor at y = 0, `half` units to each side of the origin.
    fn floor(half: f32) -> Vec<[Vec3; 3]> {
        quad(Vec3::new(-half, 0.0, -half), Vec3::new(-half, 0.0, half), Vec3::new(half, 0.0, half), Vec3::new(half, 0.0, -half)).to_vec()
    }

    fn wall_z(z: f32, height: f32) -> [[Vec3; 3]; 2] {
        quad(Vec3::new(-1000.0, 0.0, z), Vec3::new(1000.0, 0.0, z), Vec3::new(1000.0, height, z), Vec3::new(-1000.0, height, z))
    }

    /// A 2000x2000 floor at y=0 with a 300-high wall across z=500.
    fn room() -> CollisionWorld {
        let mut t = floor(1000.0);
        t.extend(wall_z(500.0, 300.0));
        CollisionWorld::from_triangles(t)
    }

    fn floor_only() -> CollisionWorld {
        CollisionWorld::from_triangles(floor(1000.0))
    }

    /// Run `secs` of 60 Hz frames with the same input.
    fn run(w: &CollisionWorld, p: &mut Player, input: MoveInput, secs: f32) {
        for _ in 0..(secs * 60.0).round() as usize {
            p.step(w, 1.0 / 60.0, input);
        }
    }

    fn settle(w: &CollisionWorld, p: &mut Player, secs: f32) {
        run(w, p, MoveInput::default(), secs);
    }

    #[test]
    fn walking_characters_follow_floors_stop_at_walls_and_notice_doors() {
        let mut w = room();
        // A door: a thin solid slab at x = 300, from z = -100 to 100.
        let door = w.add_obstacle(quad(Vec3::new(300.0, 0.0, -100.0), Vec3::new(300.0, 0.0, 100.0), Vec3::new(300.0, 200.0, 100.0), Vec3::new(300.0, 200.0, -100.0))).unwrap();
        // Walk toward the wall at z = 500: stops short of it by about the radius.
        let mut feet = Vec3::new(0.0, 0.0, 400.0);
        let mut last = None;
        for _ in 0..120 {
            let m = w.move_character(feet, Vec2::new(0.0, 2.0), 30.0, 170.0, 1.0 / 60.0, None, &[]);
            feet = m.pos;
            last = Some(m);
        }
        assert!(feet.z > 440.0 && feet.z < 475.0, "stopped at the wall: {feet:?}");
        assert!(last.as_ref().unwrap().blocked && last.unwrap().on_ground);
        // Walking into the door touches it; walking past it does not.
        let mut feet = Vec3::new(200.0, 0.0, 0.0);
        let mut touched = false;
        for _ in 0..120 {
            let m = w.move_character(feet, Vec2::new(2.0, 0.0), 30.0, 170.0, 1.0 / 60.0, None, &[]);
            feet = m.pos;
            touched |= m.touched.contains(&door);
        }
        assert!(touched && feet.x < 300.0, "stopped by the door: {feet:?}");
        assert!(w.move_character(Vec3::new(0.0, 0.0, -400.0), Vec2::ZERO, 30.0, 170.0, 1.0 / 60.0, None, &[]).touched.is_empty());
        // Another body in the way is walked around, not through: a circle at (0, 450) blocks a walk along z.
        let blockers = [(Vec2::new(0.0, 250.0), 30.0)];
        let mut feet = Vec3::new(0.0, 0.0, 0.0);
        for _ in 0..180 {
            feet = w.move_character(feet, Vec2::new(0.0, 2.0), 30.0, 170.0, 1.0 / 60.0, None, &blockers).pos;
        }
        assert!(feet.z < 200.0, "kept out of the other body: {feet:?}");
        // Standing still on the floor keeps the character on it.
        let mut feet = Vec3::new(0.0, 0.0, 0.0);
        let m = w.move_character(feet, Vec2::new(0.0, 0.0), 30.0, 170.0, 1.0 / 60.0, None, &[]);
        feet = m.pos;
        assert!(m.on_ground && feet.y.abs() < 1e-3);
    }

    fn forward() -> MoveInput {
        MoveInput::toward(Vec2::new(0.0, 1.0))
    }

    #[test]
    fn falls_and_stands_on_floor() {
        let w = room();
        let mut p = Player::new(Vec3::new(0.0, 400.0, 0.0));
        settle(&w, &mut p, 3.0);
        assert!(p.on_ground);
        assert!(p.feet.y.abs() < 0.01, "feet at {}", p.feet.y);
    }

    #[test]
    fn speeds_settle_at_the_originals_animation_driven_values() {
        let w = CollisionWorld::from_triangles(floor(8000.0));
        let speed_of = |input: MoveInput, warm_up: f32| {
            let mut p = Player::new(Vec3::new(0.0, 0.0, -7000.0));
            settle(&w, &mut p, 0.3);
            if input.crouch {
                run(&w, &mut p, MoveInput { dir: Vec2::ZERO, ..input }, 1.0);
            }
            run(&w, &mut p, input, warm_up);
            let before = p.feet.z;
            run(&w, &mut p, input, 1.0);
            p.feet.z - before
        };
        let run_speed = speed_of(forward(), 1.5);
        assert!((run_speed - 266.7).abs() < 4.0, "running: {run_speed} units/s");
        let sneak = speed_of(MoveInput { stealth: true, ..forward() }, 1.5);
        assert!((sneak - 188.3).abs() < 4.0, "sneaking: {sneak} units/s");
        let crouch = speed_of(MoveInput { crouch: true, ..forward() }, 1.5);
        assert!((crouch - 100.0).abs() < 3.0, "crouched: {crouch} units/s");
        assert!(run_speed > sneak && sneak > crouch);
    }

    #[test]
    fn movement_builds_up_and_stops_within_a_fraction_of_a_second() {
        let w = CollisionWorld::from_triangles(floor(4000.0));
        let mut p = Player::new(Vec3::new(0.0, 0.0, -3000.0));
        settle(&w, &mut p, 0.3);
        run(&w, &mut p, forward(), 0.1);
        assert!(p.vel_h.length() > 40.0 && p.vel_h.length() < 200.0, "after 0.1 s: {}", p.vel_h.length());
        run(&w, &mut p, forward(), 1.5);
        run(&w, &mut p, MoveInput::default(), 0.4);
        assert!(p.vel_h.length() < 10.0, "should have stopped: {}", p.vel_h.length());
    }

    #[test]
    fn keys_combine_like_the_engine() {
        let fwd = MoveInput::from_keys(0.0, true, false, false, false);
        assert_eq!(fwd.kind, MoveKind::Forward);
        assert!((fwd.dir - Vec2::new(0.0, -1.0)).length() < 1e-5, "yaw 0 looks along -Z: {:?}", fwd.dir);
        let turned = MoveInput::from_keys(std::f32::consts::FRAC_PI_2, true, false, false, false);
        assert!((turned.dir - Vec2::new(-1.0, 0.0)).length() < 1e-5, "{:?}", turned.dir);
        let strafe = MoveInput::from_keys(0.0, false, false, false, true);
        assert_eq!(strafe.kind, MoveKind::Strafe);
        assert!((strafe.dir - Vec2::new(1.0, 0.0)).length() < 1e-5);
        let diag = MoveInput::from_keys(0.0, true, false, false, true);
        assert_eq!(diag.kind, MoveKind::Forward, "forward decides the animation");
        assert!(diag.dir.x > 0.0 && diag.dir.y < 0.0 && (diag.dir.length() - 1.0).abs() < 1e-5);
        // 8 forward against 6 sideways: more forward than sideways.
        assert!(diag.dir.y.abs() > diag.dir.x.abs());
        assert_eq!(MoveInput::from_keys(0.0, false, false, true, true).kind, MoveKind::None, "left and right cancel");
        let back = MoveInput::from_keys(0.0, false, true, false, false);
        assert_eq!(back.kind, MoveKind::Backward);
        assert!(back.dir.y > 0.0);
        assert_eq!(MoveInput::from_keys(0.0, true, true, false, false).dir, Vec2::new(0.0, -1.0), "forward beats backward");
    }

    #[test]
    fn wall_blocks_walking_and_fast_movement() {
        let w = room();
        let mut p = Player::new(Vec3::ZERO);
        settle(&w, &mut p, 0.5);
        for _ in 0..5 {
            run(&w, &mut p, forward(), 1.0);
            assert!(p.feet.z < 500.0, "passed through the wall: z = {}", p.feet.z);
        }
        assert!(p.feet.z > 500.0 - PLAYER_RADIUS - 2.0, "should be pressed against it: z = {}", p.feet.z);
        // Even a huge frame time cannot skip it.
        let mut q = Player::new(Vec3::new(0.0, 0.0, 400.0));
        settle(&w, &mut q, 0.5);
        for _ in 0..30 {
            q.step(&w, 0.2, forward());
        }
        assert!(q.feet.z < 500.0, "z = {}", q.feet.z);
    }

    #[test]
    fn a_jump_rises_about_130_units_and_takes_under_a_second() {
        let w = CollisionWorld::from_triangles(floor(4000.0));
        let mut p = Player::new(Vec3::ZERO);
        // The original's movement is what this pins down.
        p.classic = true;
        settle(&w, &mut p, 0.5);
        let (mut apex, mut air) = (0.0f32, 0.0f32);
        let jump = MoveInput { jump: true, ..Default::default() };
        p.step(&w, 1.0 / 60.0, jump);
        for _ in 0..180 {
            p.step(&w, 1.0 / 60.0, MoveInput::default());
            apex = apex.max(p.feet.y);
            if !p.on_ground {
                air += 1.0 / 60.0;
            }
        }
        assert!((apex - 130.0).abs() < 8.0, "apex {apex}");
        // 0.2 s up, then 130 units at 600 units/s^2: about 0.66 s down.
        assert!(air > 0.75 && air < 1.0, "air time {air}");
        assert!(p.on_ground && p.phase == JumpPhase::None);
    }

    #[test]
    fn holding_jump_does_not_bounce_and_a_press_during_a_fall_is_remembered_briefly() {
        let w = CollisionWorld::from_triangles(floor(4000.0));
        let mut p = Player::new(Vec3::ZERO);
        // The original's movement is what this pins down.
        p.classic = true;
        settle(&w, &mut p, 0.5);
        let held = MoveInput { jump: true, ..Default::default() };
        let mut jumps = 0;
        let mut was_ground = true;
        for _ in 0..360 {
            p.step(&w, 1.0 / 60.0, held);
            if was_ground && !p.on_ground {
                jumps += 1;
            }
            was_ground = p.on_ground;
        }
        assert_eq!(jumps, 1, "one press, one jump");
    }

    #[test]
    fn a_running_jump_carries_forward_and_landing_stops_the_slide() {
        let w = CollisionWorld::from_triangles(floor(4000.0));
        let mut p = Player::new(Vec3::new(0.0, 0.0, -3000.0));
        // The original's movement is what this pins down.
        p.classic = true;
        settle(&w, &mut p, 0.3);
        run(&w, &mut p, forward(), 1.5);
        let start = p.feet.z;
        p.step(&w, 1.0 / 60.0, MoveInput { jump: true, ..forward() });
        let mut landed_at = None;
        for i in 0..240 {
            p.step(&w, 1.0 / 60.0, forward());
            if p.on_ground && landed_at.is_none() {
                landed_at = Some(i);
                assert_eq!(p.vel_h, Vec2::ZERO, "landing from a jump kills the horizontal velocity");
            }
        }
        assert!(landed_at.is_some());
        let _ = start;
        let mut q = Player::new(Vec3::new(0.0, 0.0, -3000.0));
        // The original's movement is what this pins down.
        q.classic = true;
        settle(&w, &mut q, 0.3);
        run(&w, &mut q, forward(), 1.5);
        let z0 = q.feet.z;
        q.step(&w, 1.0 / 60.0, MoveInput { jump: true, ..forward() });
        for _ in 0..120 {
            q.step(&w, 1.0 / 60.0, forward());
            if q.on_ground {
                break;
            }
        }
        // The air push (7.9 against 2.4 for running) makes a held-forward jump far longer than the run-up.
        let distance = q.feet.z - z0;
        assert!(distance > 300.0 && distance < 800.0, "forward distance of a running jump: {distance}");
    }

    #[test]
    fn landing_slows_the_push_with_the_engines_ramp() {
        let mut p = Player::new(Vec3::ZERO);
        // The original's movement is what this pins down.
        p.classic = true;
        p.last_landing_ms = 0.0;
        for (since, expect) in [(0.0, 0.5), (299.0, 0.5), (450.0, 0.0), (599.0, -0.497), (600.0, 1.0), (5000.0, 1.0)] {
            p.clock_ms = since;
            assert!((p.landing_recovery() - expect).abs() < 0.01, "{since} ms: {}", p.landing_recovery());
        }
    }

    #[test]
    fn jump_cannot_clear_a_tall_wall_and_lands_again() {
        let w = room();
        let mut p = Player::new(Vec3::ZERO);
        // The original's movement is what this pins down.
        p.classic = true;
        settle(&w, &mut p, 0.5);
        p.step(&w, 1.0 / 60.0, MoveInput { jump: true, ..forward() });
        let mut apex = 0.0f32;
        for _ in 0..240 {
            p.step(&w, 1.0 / 60.0, forward());
            apex = apex.max(p.feet.y);
        }
        assert!(apex > 100.0 && apex < 150.0, "jump apex {apex}");
        assert!(p.feet.z < 500.0);
        assert!(p.on_ground);
    }

    #[test]
    fn steps_up_low_ledges_but_not_high_ones() {
        let step_world = |height: f32| {
            let mut t = floor(500.0);
            t.extend(quad(Vec3::new(-500.0, height, 100.0), Vec3::new(500.0, height, 100.0), Vec3::new(500.0, height, 500.0), Vec3::new(-500.0, height, 500.0)));
            t.extend(quad(Vec3::new(-500.0, 0.0, 100.0), Vec3::new(500.0, 0.0, 100.0), Vec3::new(500.0, height, 100.0), Vec3::new(-500.0, height, 100.0)));
            CollisionWorld::from_triangles(t)
        };
        for (height, climbs) in [(30.0, true), (38.0, true), (46.0, false), (120.0, false)] {
            let w = step_world(height);
            let mut p = Player::new(Vec3::new(0.0, 0.0, -200.0));
            settle(&w, &mut p, 0.5);
            run(&w, &mut p, forward(), 2.0);
            if climbs {
                assert!(p.feet.z > 150.0 && (p.feet.y - height).abs() < 0.1, "{height}-high step: z {} y {}", p.feet.z, p.feet.y);
            } else {
                assert!(p.feet.z < 100.0 && p.feet.y.abs() < 0.1, "{height}-high ledge: z {} y {}", p.feet.z, p.feet.y);
            }
        }
    }

    #[test]
    fn falling_out_of_the_world_is_rescued() {
        let w = room();
        let mut p = Player::new(Vec3::ZERO);
        settle(&w, &mut p, 0.5);
        let mut lowest = 0.0f32;
        for _ in 0..1800 {
            p.step(&w, 1.0 / 60.0, MoveInput::toward(Vec2::new(1.0, 0.0))); // keep walking off the +x edge
            lowest = lowest.min(p.feet.y);
        }
        assert!(p.rescues >= 1, "rescues: {}", p.rescues);
        assert!(lowest > -(RESCUE_DEPTH + 800.0), "fell to {lowest}");
    }

    #[test]
    fn closed_door_obstacle_blocks_and_open_door_does_not() {
        let mut w = floor_only();
        // A door: a 200-wide, 230-high vertical panel across z = 300.
        let door = w
            .add_obstacle(quad(Vec3::new(-100.0, 0.0, 300.0), Vec3::new(100.0, 0.0, 300.0), Vec3::new(100.0, 230.0, 300.0), Vec3::new(-100.0, 230.0, 300.0)))
            .unwrap();
        let mut p = Player::new(Vec3::ZERO);
        settle(&w, &mut p, 0.5);
        run(&w, &mut p, forward(), 4.0);
        assert!(p.feet.z < 300.0, "walked through a closed door: z = {}", p.feet.z);
        assert!(p.feet.z > 300.0 - PLAYER_RADIUS - 5.0, "should be pressed against it: z = {}", p.feet.z);

        w.set_obstacle_enabled(door, false);
        assert!(!w.obstacle_enabled(door));
        run(&w, &mut p, forward(), 3.0);
        assert!(p.feet.z > 400.0, "should walk through an open door: z = {}", p.feet.z);
    }

    #[test]
    fn obstacle_does_not_block_what_is_not_in_the_way() {
        let mut w = floor_only();
        w.add_obstacle(quad(Vec3::new(500.0, 0.0, 0.0), Vec3::new(500.0, 0.0, 200.0), Vec3::new(500.0, 200.0, 200.0), Vec3::new(500.0, 200.0, 0.0)));
        let mut p = Player::new(Vec3::new(0.0, 0.0, -500.0));
        settle(&w, &mut p, 0.5);
        run(&w, &mut p, forward(), 5.0);
        assert!(p.feet.z > 300.0, "z = {}", p.feet.z);
    }

    /// A steep wall leaning away from the player. Whichever way its stored normal points, the
    /// player must stay on the side they came from.
    #[test]
    fn slanted_wall_with_either_normal_orientation_cannot_be_passed() {
        for flip in [false, true] {
            let (a, b, c, d) = (
                Vec3::new(-600.0, 0.0, 300.0),
                Vec3::new(600.0, 0.0, 300.0),
                Vec3::new(600.0, 300.0, 400.0),
                Vec3::new(-600.0, 300.0, 400.0),
            );
            let mut tris = floor(1000.0);
            let wall = if flip { [[a, c, b], [a, d, c]] } else { [[a, b, c], [a, c, d]] };
            tris.extend(wall);
            let w = CollisionWorld::from_triangles(tris);
            let mut p = Player::new(Vec3::ZERO);
            settle(&w, &mut p, 0.5);
            run(&w, &mut p, forward(), 5.0);
            assert!(p.feet.z < 300.0, "flip={flip}: passed the wall, z = {}", p.feet.z);
        }
    }

    #[test]
    fn solid_entity_can_be_stood_on_and_stops_being_ground_when_disabled() {
        // A floor with a 600-wide hole in the middle, plugged by a trapdoor obstacle at floor level.
        let mut tris = Vec::new();
        for (x0, x1) in [(-1000.0, -300.0), (300.0, 1000.0)] {
            tris.extend(quad(Vec3::new(x0, 0.0, -1000.0), Vec3::new(x0, 0.0, 1000.0), Vec3::new(x1, 0.0, 1000.0), Vec3::new(x1, 0.0, -1000.0)));
        }
        let mut w = CollisionWorld::from_triangles(tris);
        let trap = w
            .add_obstacle(quad(Vec3::new(-300.0, 0.0, -1000.0), Vec3::new(-300.0, 0.0, 1000.0), Vec3::new(300.0, 0.0, 1000.0), Vec3::new(300.0, 0.0, -1000.0)))
            .unwrap();
        let mut p = Player::new(Vec3::new(0.0, 30.0, 0.0));
        settle(&w, &mut p, 1.0);
        assert!(p.on_ground && p.feet.y.abs() < 0.01, "should stand on the trapdoor, y = {}", p.feet.y);
        w.set_obstacle_enabled(trap, false);
        let mut q = Player::new(Vec3::new(0.0, 30.0, 0.0));
        settle(&w, &mut q, 1.0);
        assert!(!q.on_ground && q.feet.y < -100.0, "an open trapdoor is a hole, y = {}", q.feet.y);
    }

    /// A floor with a ceiling slab `clearance` above it for z > 300.
    fn low_ceiling(clearance: f32) -> CollisionWorld {
        let mut t = floor(1500.0);
        t.extend(quad(Vec3::new(-1500.0, clearance, 300.0), Vec3::new(1500.0, clearance, 300.0), Vec3::new(1500.0, clearance, 1500.0), Vec3::new(-1500.0, clearance, 1500.0)));
        CollisionWorld::from_triangles(t)
    }

    #[test]
    fn crouching_lowers_the_body_and_eyes_after_the_animation() {
        let w = floor_only();
        let mut p = Player::new(Vec3::ZERO);
        // The original's movement is what this pins down.
        p.classic = true;
        settle(&w, &mut p, 0.3);
        assert_eq!((p.height(), p.eye_height()), (PLAYER_HEIGHT, EYE_HEIGHT));
        let crouch = MoveInput { crouch: true, ..Default::default() };
        run(&w, &mut p, crouch, 0.3);
        assert!(matches!(p.stance, Stance::GoingDown(_)));
        assert_eq!(p.height(), PLAYER_HEIGHT, "the cylinder shrinks only when the animation ends");
        assert!(p.eye_height() < EYE_HEIGHT && p.eye_height() > CROUCH_EYE_HEIGHT);
        run(&w, &mut p, crouch, 0.6);
        assert_eq!((p.stance, p.height(), p.eye_height()), (Stance::Crouched, CROUCH_HEIGHT, CROUCH_EYE_HEIGHT));
        run(&w, &mut p, MoveInput::default(), 1.0);
        assert_eq!((p.stance, p.eye_height()), (Stance::Standing, EYE_HEIGHT));
    }

    #[test]
    fn a_low_ceiling_needs_a_crouch_to_enter_and_keeps_you_crouched() {
        // 150 high: too low to stand, enough to crouch.
        let w = low_ceiling(150.0);
        let mut p = Player::new(Vec3::new(0.0, 0.0, 0.0));
        // The original's movement is what this pins down.
        p.classic = true;
        settle(&w, &mut p, 0.3);
        // Standing, the head hits the ceiling's edge like a wall (the original does not crouch for you here).
        run(&w, &mut p, forward(), 4.0);
        assert!(p.feet.z < 300.0, "walked into a ceiling too low to stand under: z = {}", p.feet.z);
        // Crouch first, and the way is open.
        let crouch = MoveInput { crouch: true, ..forward() };
        run(&w, &mut p, MoveInput { crouch: true, ..Default::default() }, 1.0);
        run(&w, &mut p, crouch, 12.0);
        assert!(p.feet.z > 500.0, "should get under the ceiling crouched: z = {}", p.feet.z);
        // Releasing the key does not stand up under it...
        run(&w, &mut p, MoveInput::default(), 2.0);
        assert!(p.is_crouching() && p.height() == CROUCH_HEIGHT, "{:?}", p.stance);
        // ...but walking back out does.
        run(&w, &mut p, MoveInput::toward(Vec2::new(0.0, -1.0)), 12.0);
        run(&w, &mut p, MoveInput::default(), 2.0);
        assert_eq!(p.stance, Stance::Standing, "z = {}", p.feet.z);
    }

    #[test]
    fn a_ceiling_lowering_onto_a_standing_player_forces_a_crouch() {
        let w = low_ceiling(150.0);
        let mut p = Player::new(Vec3::new(0.0, 0.0, 700.0));
        settle(&w, &mut p, 0.3);
        assert!(p.is_crouching(), "spawned under a 150-high ceiling");
        settle(&w, &mut p, 1.0);
        assert_eq!(p.stance, Stance::Crouched);
    }

    #[test]
    fn a_gap_lower_than_a_crouch_cannot_be_entered() {
        let w = low_ceiling(100.0);
        let mut p = Player::new(Vec3::new(0.0, 0.0, 0.0));
        settle(&w, &mut p, 0.3);
        run(&w, &mut p, MoveInput { crouch: true, ..Default::default() }, 1.0);
        run(&w, &mut p, MoveInput { crouch: true, ..forward() }, 12.0);
        assert!(p.feet.z < 330.0, "a 100-high gap is too low to enter: z = {}", p.feet.z);
    }

    #[test]
    fn jumping_under_a_ceiling_bumps_the_head() {
        let w = low_ceiling(250.0);
        let mut p = Player::new(Vec3::new(0.0, 0.0, 700.0));
        settle(&w, &mut p, 0.5);
        p.step(&w, 1.0 / 60.0, MoveInput { jump: true, ..Default::default() });
        let mut apex = 0.0f32;
        for _ in 0..120 {
            p.step(&w, 1.0 / 60.0, MoveInput::default());
            apex = apex.max(p.feet.y);
        }
        assert!(apex <= 250.0 - PLAYER_HEIGHT + 0.5, "head went through the ceiling: apex {apex}");
        assert!(apex > 60.0, "should still rise to the ceiling: apex {apex}");
        assert!(p.on_ground);
    }

    #[test]
    fn long_falls_hurt_short_ones_do_not() {
        let w = CollisionWorld::from_triangles(floor(4000.0));
        let mut short = Player::new(Vec3::new(0.0, 300.0, 0.0));
        settle(&w, &mut short, 3.0);
        assert!(short.on_ground);
        assert_eq!(short.take_landing(), None, "300 units is harmless");

        let mut long = Player::new(Vec3::new(0.0, 900.0, 0.0));
        long.vel_h = Vec2::new(100.0, 0.0);
        settle(&w, &mut long, 6.0);
        assert!(long.on_ground);
        assert_eq!(long.vel_h, Vec2::ZERO, "a hard landing stops the slide");
        let fell = long.take_landing().expect("a 900-unit drop must report a damaging landing");
        assert!(fell > 700.0 && fell < 900.0, "fall height {fell}");
        assert!((fall_damage(fell) - (fell - 400.0) / 15.0).abs() < 1e-4 && fall_damage(fell) > 20.0);
        assert_eq!(long.take_landing(), None, "reported once");
        assert_eq!(fall_damage(399.0), 0.0);
    }

    #[test]
    fn falls_are_floaty_once_fast_enough() {
        let w = CollisionWorld::from_triangles(floor(4000.0));
        let mut p = Player::new(Vec3::new(0.0, 2000.0, 0.0));
        // The original's movement is what this pins down.
        p.classic = true;
        let mut v_late = 0.0f32;
        for _ in 0..200 {
            p.step(&w, 1.0 / 60.0, MoveInput::default());
            if p.falling {
                v_late = p.vel_y;
            }
        }
        // Gravity drops from 3000 to 600 units/s^2 once the fall passes 450 units/s, so after more than a second
        // the speed is far below what 3000 units/s^2 would give (3000+).
        assert!(v_late > -1700.0 && v_late < -500.0, "vel_y {v_late}");
    }

    #[test]
    fn a_character_cylinder_blocks_until_it_is_switched_off_or_moved() {
        let mut w = floor_only();
        let npc = w.add_cylinder(Vec3::new(0.0, 0.0, 300.0), 30.0, 170.0);
        let mut p = Player::new(Vec3::ZERO);
        settle(&w, &mut p, 0.5);
        run(&w, &mut p, forward(), 4.0);
        assert!(p.feet.z < 300.0 - 30.0 + 1.0 && p.feet.z > 300.0 - 30.0 - PLAYER_RADIUS - 5.0, "pressed against the character: z = {}", p.feet.z);
        // It can be walked around.
        run(&w, &mut p, MoveInput::toward(Vec2::new(1.0, 0.0)), 1.5);
        run(&w, &mut p, forward(), 2.0);
        assert!(p.feet.z > 400.0, "should have got past it: z = {}", p.feet.z);
        // A dead (disabled) one does not block.
        let mut q = Player::new(Vec3::ZERO);
        settle(&w, &mut q, 0.5);
        w.set_cylinder(npc, Vec3::new(0.0, 0.0, 300.0), false);
        run(&w, &mut q, forward(), 3.0);
        assert!(q.feet.z > 350.0, "z = {}", q.feet.z);
        // A short one (below the step height) is stepped over, a character far above is not in the way.
        let w2 = {
            let mut w = floor_only();
            w.add_cylinder(Vec3::new(0.0, 200.0, 300.0), 30.0, 100.0);
            w
        };
        let mut r = Player::new(Vec3::ZERO);
        settle(&w2, &mut r, 0.5);
        run(&w2, &mut r, forward(), 3.0);
        assert!(r.feet.z > 350.0, "a cylinder floating above the head does not block: z = {}", r.feet.z);
    }

    #[test]
    fn floors_know_their_material_and_water_is_found() {
        let tris = [
            ([Vec3::new(-100.0, 0.0, -100.0), Vec3::new(-100.0, 0.0, 100.0), Vec3::new(100.0, 0.0, 100.0)], "stone"),
            ([Vec3::new(-100.0, 0.0, -100.0), Vec3::new(100.0, 0.0, 100.0), Vec3::new(100.0, 0.0, -100.0)], "stone"),
            ([Vec3::new(300.0, 0.0, -100.0), Vec3::new(300.0, 0.0, 100.0), Vec3::new(500.0, 0.0, 100.0)], "wood"),
        ];
        let w = CollisionWorld::from_material_triangles(tris);
        assert_eq!(w.floor_material(0.0, 0.0, 10.0), Some("stone"));
        assert_eq!(w.floor_material(450.0, 90.0, 10.0), Some("wood"));
        assert_eq!(w.floor_material(1000.0, 0.0, 10.0), None);
        assert_eq!(w.water_level_at(0.0, 0.0), None);
        let mut v = CollisionWorld::default();
        v.push_water(Vec3::new(-50.0, -20.0, -50.0), Vec3::new(-50.0, -20.0, 50.0), Vec3::new(50.0, -20.0, 50.0));
        assert_eq!(v.water_level_at(-30.0, 30.0), Some(-20.0));
        assert_eq!(v.water_level_at(30.0, -30.0), None, "outside the triangle");
    }

    #[test]
    fn walking_makes_a_step_every_120_units_and_crouching_doubles_it() {
        let w = CollisionWorld::from_triangles(floor(8000.0));
        let mut p = Player::new(Vec3::new(0.0, 0.0, -7000.0));
        settle(&w, &mut p, 0.3);
        p.take_steps();
        let z0 = p.feet.z;
        run(&w, &mut p, forward(), 6.0);
        let travelled = p.feet.z - z0;
        let steps = p.take_steps();
        assert_eq!(steps, (travelled / 120.0).floor() as u32, "{travelled} units, {steps} steps");
        assert!(steps >= 10);
        // Not in the air.
        p.take_steps();
        p.step(&w, 1.0 / 60.0, MoveInput { jump: true, ..forward() });
        assert!(p.take_jump());
        let before = p.feet.z;
        for _ in 0..30 {
            p.step(&w, 1.0 / 60.0, forward());
        }
        assert!(p.feet.z - before > 100.0 && p.take_steps() == 0, "no footsteps while jumping");
    }

    #[test]
    fn rays_hit_floors_walls_and_entities() {
        let mut w = room();
        // Down at the floor.
        let hit = w.raycast(Vec3::new(0.0, 100.0, 0.0), Vec3::NEG_Y, 500.0).unwrap();
        assert!((hit.t - 100.0).abs() < 1e-3 && hit.normal.y > 0.99, "{hit:?}");
        // Along the floor into the wall at z = 500, facing back at the ray.
        let hit = w.raycast(Vec3::new(0.0, 100.0, 0.0), Vec3::Z, 1000.0).unwrap();
        assert!((hit.t - 500.0).abs() < 1e-2 && hit.normal.z < -0.99, "{hit:?}");
        // Too short, or pointing away from everything.
        assert!(w.raycast(Vec3::new(0.0, 100.0, 0.0), Vec3::Z, 400.0).is_none());
        assert!(w.raycast(Vec3::new(0.0, 100.0, 0.0), Vec3::Y, 1000.0).is_none());
        // An entity in the way is hit first, and not once it is switched off.
        let door = w
            .add_obstacle(quad(Vec3::new(-100.0, 0.0, 300.0), Vec3::new(100.0, 0.0, 300.0), Vec3::new(100.0, 230.0, 300.0), Vec3::new(-100.0, 230.0, 300.0)))
            .unwrap();
        assert!((w.raycast(Vec3::new(0.0, 100.0, 0.0), Vec3::Z, 1000.0).unwrap().t - 300.0).abs() < 1e-2);
        w.set_obstacle_enabled(door, false);
        assert!((w.raycast(Vec3::new(0.0, 100.0, 0.0), Vec3::Z, 1000.0).unwrap().t - 500.0).abs() < 1e-2);
    }
    #[test]
    fn the_tightened_jump_is_thrown_and_pulled_down_by_one_gravity() {
        let w = floor_only();
        let mut p = Player::new(Vec3::new(0.0, 0.0, 0.0));
        settle(&w, &mut p, 0.5);
        assert!(p.on_ground && !p.classic);
        // Running, then a jump: up to about 95 units, back down in well under a second, and still running.
        let forward = MoveInput::from_keys(0.0, true, false, false, false);
        run(&w, &mut p, forward, 1.5);
        let speed = p.vel_h.length();
        let from = p.feet;
        p.step(&w, 1.0 / 60.0, MoveInput { jump: true, ..forward });
        assert!(p.take_jump() && !p.on_ground);
        let (mut apex, mut frames, mut airborne_speed) = (0.0f32, 0, 0.0f32);
        while !p.on_ground && frames < 600 {
            p.step(&w, 1.0 / 60.0, forward);
            apex = apex.max(p.feet.y);
            airborne_speed = airborne_speed.max(p.vel_h.length());
            frames += 1;
        }
        let secs = frames as f32 / 60.0;
        assert!((apex - QUICK_JUMP_HEIGHT).abs() < 6.0, "apex {apex}");
        assert!((0.55..0.8).contains(&secs), "in the air for {secs} s");
        assert!(airborne_speed < speed * 1.4, "no faster than a run and a third: {airborne_speed} against {speed}");
        assert!(p.vel_h.length() > speed * 0.9, "landing does not stop the run: {}", p.vel_h.length());
        let carried = (p.feet - from).length();
        assert!((170.0..300.0).contains(&carried), "carried {carried} units");
        assert_eq!(p.take_landing(), None, "a jump does not hurt");
        // A long drop still does: the height is counted from the top.
        let mut q = Player::new(Vec3::new(0.0, 700.0, 0.0));
        while !q.on_ground {
            q.step(&w, 1.0 / 60.0, MoveInput::default());
        }
        let fell = q.take_landing().expect("a 700 unit fall hurts");
        assert!((fell - 700.0).abs() < 60.0, "fell {fell}");
    }

    #[test]
    fn the_tightened_crouch_answers_at_once() {
        let w = floor_only();
        let mut p = Player::new(Vec3::ZERO);
        settle(&w, &mut p, 0.5);
        let crouch = MoveInput { crouch: true, ..Default::default() };
        p.step(&w, 1.0 / 60.0, crouch);
        // The body is low from the first frame; the eyes follow within a fraction of a second.
        assert_eq!(p.height(), CROUCH_HEIGHT);
        run(&w, &mut p, crouch, 0.05);
        assert!(p.eye_height() < EYE_HEIGHT && p.eye_height() > CROUCH_EYE_HEIGHT);
        run(&w, &mut p, crouch, 0.2);
        assert_eq!((p.stance, p.eye_height()), (Stance::Crouched, CROUCH_EYE_HEIGHT));
        // Moving while ducking is at the crouched pace straight away, not slower.
        let mut q = Player::new(Vec3::ZERO);
        settle(&w, &mut q, 0.5);
        run(&w, &mut q, MoveInput { crouch: true, ..MoveInput::from_keys(0.0, true, false, false, false) }, 1.0);
        assert!((q.vel_h.length() - 100.0).abs() < 5.0, "{}", q.vel_h.length());
        // And up again as quickly.
        run(&w, &mut p, MoveInput::default(), 0.2);
        assert_eq!((p.stance, p.height(), p.eye_height()), (Stance::Standing, PLAYER_HEIGHT, EYE_HEIGHT));
    }

}
