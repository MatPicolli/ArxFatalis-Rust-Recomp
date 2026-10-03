//! Level collision and a first-person player body.
//!
//! All coordinates here are **y-up** (the Bevy convention); [`CollisionWorld::from_fts`] converts
//! from Arx's y-down data. The world is a bag of triangles in a 2D spatial hash. The player is a
//! vertical cylinder that slides along steep triangles, steps over low ones and stands on walkable
//! ones.

use arx_formats::{fts::Fts, poly};
use glam::{Vec2, Vec3};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

/// Dimensions of the original player cylinder.
pub const PLAYER_RADIUS: f32 = 52.0;
pub const PLAYER_HEIGHT: f32 = 170.0;
/// Eyes are slightly below the top of the cylinder.
pub const EYE_HEIGHT: f32 = 160.0;
/// Highest ledge the player walks onto without jumping.
pub const STEP_HEIGHT: f32 = 45.0;

const CELL: f32 = 100.0;
/// Triangles at least this flat (in either orientation) can support the player, as in the original
/// engine, which stands the player on the nearest polygon below with no slope limit.
const SUPPORT_NORMAL_Y: f32 = 0.1;
/// Triangles flatter than this are floors/ceilings; steeper ones block horizontal movement.
const WALKABLE_NORMAL_Y: f32 = 0.55;
const GRAVITY: f32 = 1500.0;
/// How quickly horizontal velocity can change in the air, per second (1.0 = fully within a second).
const AIR_CONTROL: f32 = 1.2;
const JUMP_SPEED: f32 = 520.0;

#[derive(Debug, Clone, Copy)]
struct Tri {
    a: Vec3,
    b: Vec3,
    c: Vec3,
    /// Unit normal, oriented to the polygon's front side.
    n: Vec3,
    min_y: f32,
    max_y: f32,
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

#[derive(Default)]
pub struct CollisionWorld {
    tris: Vec<Tri>,
    grid: HashMap<(i32, i32), Vec<u32>>,
    obstacles: Vec<Obstacle>,
    /// One flag per obstacle; atomic so doors can be toggled while the world is shared.
    obstacle_enabled: Vec<AtomicBool>,
    /// Centre of the largest flat, upward-facing solid triangle: a safe place to put the player.
    fallback_spawn: Option<(f32, Vec3)>,
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
        for p in &fts.polys {
            if p.flags & (poly::WATER | poly::TRANS | poly::NOCOL) != 0 {
                continue;
            }
            let v: Vec<Vec3> = p.verts[..p.vertex_count()].iter().map(|v| to_yup(v.pos)).collect();
            w.push(v[0], v[1], v[2], to_yup(p.norm));
            if v.len() == 4 {
                w.push(v[3], v[2], v[1], to_yup(p.norm2));
            }
        }
        w
    }

    /// Build a world from explicit y-up triangles (mainly for tests and tools).
    pub fn from_triangles(tris: impl IntoIterator<Item = [Vec3; 3]>) -> Self {
        let mut w = CollisionWorld::default();
        for [a, b, c] in tris {
            w.push(a, b, c, Vec3::ZERO);
        }
        w
    }

    fn push(&mut self, a: Vec3, b: Vec3, c: Vec3, stored_normal: Vec3) {
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
        self.tris.push(Tri { a, b, c, n, min_y: min.y, max_y: max.y });
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
            list.push(Tri { a, b, c, n, min_y: lo.y, max_y: hi.y });
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
        let p = Vec2::new(x, z);
        let mut best: Option<f32> = None;
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
            if y <= max_y && best.is_none_or(|b| y > b) {
                best = Some(y);
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

    /// Highest floor under the cylinder footprint (centre plus four points on its rim).
    fn footprint_floor(&self, pos: Vec3, max_y: f32) -> Option<f32> {
        let r = PLAYER_RADIUS * 0.7;
        [(0.0, 0.0), (r, 0.0), (-r, 0.0), (0.0, r), (0.0, -r)]
            .iter()
            .filter_map(|(dx, dz)| self.floor_height(pos.x + dx, pos.z + dz, max_y))
            .max_by(f32::total_cmp)
    }

    /// Push the cylinder at `pos` out of nearby steep triangles (and enabled obstacles) in the
    /// horizontal plane. `prev` is where the player was before this move: it decides which side of a
    /// wall the player belongs on, so a wall can never push them through itself. Obstacles whose top
    /// is below `pos.y + step` are low enough to step over (and are left to the floor pass).
    fn push_out_of_walls(&self, pos: &mut Vec3, prev: Vec3, step: f32) {
        let r = PLAYER_RADIUS;
        for _ in 0..8 {
            let (lo, hi) = (pos.y + step, pos.y + PLAYER_HEIGHT);
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

/// First-person player body: feet position, vertical velocity, ground state.
#[derive(Debug, Clone, Copy)]
pub struct Player {
    pub feet: Vec3,
    pub vel_y: f32,
    pub on_ground: bool,
    /// Horizontal velocity: follows the wish direction on the ground, keeps its momentum in the air.
    pub vel_h: Vec2,
    /// Last position that had solid ground under it (the engine's "last valid position").
    pub last_ground: Vec3,
    /// How many times the player fell out of the world and was put back on `last_ground`.
    pub rescues: u32,
}

/// Falling this far below the last solid ground means the player left the level.
const RESCUE_DEPTH: f32 = 3000.0;

impl Player {
    pub fn new(feet: Vec3) -> Self {
        Player { feet, vel_y: 0.0, on_ground: false, vel_h: Vec2::ZERO, last_ground: feet, rescues: 0 }
    }

    pub fn eye(&self) -> Vec3 {
        self.feet + Vec3::Y * EYE_HEIGHT
    }

    /// Advance by `dt` seconds. `wish` is the desired horizontal velocity in units/second.
    pub fn step(&mut self, world: &CollisionWorld, dt: f32, wish: Vec2, jump: bool) {
        // Sub-step so fast movement cannot skip through thin walls.
        let max_move = PLAYER_RADIUS * 0.4;
        let n = ((wish.length().max(self.vel_h.length()) * dt / max_move).ceil() as usize).max(1);
        let sub = dt / n as f32;
        let mut jump = jump && self.on_ground;
        for _ in 0..n {
            self.vel_h = if self.on_ground && !jump { wish } else { self.vel_h.lerp(wish, (AIR_CONTROL * sub).min(1.0)) };
            let v = self.vel_h;
            self.step_once(world, sub, v, jump);
            jump = false;
        }
        if self.on_ground {
            self.last_ground = self.feet;
        } else if self.feet.y < self.last_ground.y - RESCUE_DEPTH {
            self.feet = self.last_ground;
            self.vel_y = 0.0;
            self.on_ground = true;
            self.rescues += 1;
        }
    }

    fn step_once(&mut self, world: &CollisionWorld, dt: f32, wish: Vec2, jump: bool) {
        // Horizontal move, then resolve against walls.
        let mut pos = self.feet + Vec3::new(wish.x, 0.0, wish.y) * dt;
        // Only a grounded player can step over low obstacles; a jump cannot hop over a parapet.
        world.push_out_of_walls(&mut pos, self.feet, if self.on_ground { STEP_HEIGHT } else { 5.0 });

        if jump {
            self.vel_y = JUMP_SPEED;
            self.on_ground = false;
        }

        // Highest floor under the footprint that the player could step onto from here.
        let floor = world.footprint_floor(pos, pos.y + STEP_HEIGHT);

        if self.vel_y <= 0.0 {
            // Standing: follow the floor up and down (stairs) within the step height.
            if self.on_ground
                && let Some(f) = floor
                && f >= pos.y - STEP_HEIGHT
            {
                pos.y = f;
                self.vel_y = 0.0;
                self.feet = pos;
                return;
            }
            // Falling: land on a floor we reach or cross during this step.
            self.vel_y -= GRAVITY * dt;
            pos.y += self.vel_y * dt;
            match floor {
                Some(f) if f >= pos.y => {
                    pos.y = f;
                    self.vel_y = 0.0;
                    self.on_ground = true;
                }
                _ => self.on_ground = false,
            }
        } else {
            self.vel_y -= GRAVITY * dt;
            pos.y += self.vel_y * dt;
            self.on_ground = false;
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

    /// A 2000x2000 floor at y=0 with a 300-high wall across z=500.
    fn room() -> CollisionWorld {
        let mut t = Vec::new();
        t.extend(quad(Vec3::new(-1000.0, 0.0, -1000.0), Vec3::new(-1000.0, 0.0, 1000.0), Vec3::new(1000.0, 0.0, 1000.0), Vec3::new(1000.0, 0.0, -1000.0)));
        t.extend(quad(Vec3::new(-1000.0, 0.0, 500.0), Vec3::new(1000.0, 0.0, 500.0), Vec3::new(1000.0, 300.0, 500.0), Vec3::new(-1000.0, 300.0, 500.0)));
        CollisionWorld::from_triangles(t)
    }

    fn settle(w: &CollisionWorld, p: &mut Player, secs: f32) {
        for _ in 0..(secs * 60.0) as usize {
            p.step(w, 1.0 / 60.0, Vec2::ZERO, false);
        }
    }

    #[test]
    fn falls_and_stands_on_floor() {
        let w = room();
        let mut p = Player::new(Vec3::new(0.0, 400.0, 0.0));
        settle(&w, &mut p, 2.0);
        assert!(p.on_ground);
        assert!(p.feet.y.abs() < 0.01, "feet at {}", p.feet.y);
    }

    #[test]
    fn wall_blocks_walking_and_fast_movement() {
        let w = room();
        let mut p = Player::new(Vec3::ZERO);
        settle(&w, &mut p, 0.5);
        // Walk (and sprint) straight into the wall for several seconds.
        for speed in [300.0, 3000.0] {
            for _ in 0..300 {
                p.step(&w, 1.0 / 60.0, Vec2::new(0.0, speed), false);
            }
            assert!(p.feet.z < 500.0, "passed through the wall at speed {speed}: z = {}", p.feet.z);
            assert!(p.feet.z > 500.0 - PLAYER_RADIUS - 2.0 || speed > 0.0);
        }
    }

    #[test]
    fn jump_cannot_clear_a_tall_wall_and_lands_again() {
        let w = room();
        let mut p = Player::new(Vec3::ZERO);
        settle(&w, &mut p, 0.5);
        let mut apex = 0.0f32;
        for i in 0..240 {
            p.step(&w, 1.0 / 60.0, Vec2::new(0.0, 300.0), i == 0);
            apex = apex.max(p.feet.y);
        }
        assert!(apex > 50.0 && apex < 150.0, "jump apex {apex}");
        assert!(p.feet.z < 500.0);
        assert!(p.on_ground);
    }

    #[test]
    fn steps_up_low_ledges_but_not_high_ones() {
        let mut t = Vec::new();
        t.extend(quad(Vec3::new(-500.0, 0.0, -500.0), Vec3::new(-500.0, 0.0, 500.0), Vec3::new(500.0, 0.0, 500.0), Vec3::new(500.0, 0.0, -500.0)));
        // 30-high step at z=100 (walkable), 120-high block at z=300 (not).
        t.extend(quad(Vec3::new(-500.0, 30.0, 100.0), Vec3::new(500.0, 30.0, 100.0), Vec3::new(500.0, 30.0, 500.0), Vec3::new(-500.0, 30.0, 500.0)));
        t.extend(quad(Vec3::new(-500.0, 0.0, 100.0), Vec3::new(500.0, 0.0, 100.0), Vec3::new(500.0, 30.0, 100.0), Vec3::new(-500.0, 30.0, 100.0)));
        t.extend(quad(Vec3::new(-500.0, 120.0, 300.0), Vec3::new(500.0, 120.0, 300.0), Vec3::new(500.0, 120.0, 500.0), Vec3::new(-500.0, 120.0, 500.0)));
        t.extend(quad(Vec3::new(-500.0, 0.0, 300.0), Vec3::new(500.0, 0.0, 300.0), Vec3::new(500.0, 120.0, 300.0), Vec3::new(-500.0, 120.0, 300.0)));
        let w = CollisionWorld::from_triangles(t);
        let mut p = Player::new(Vec3::new(0.0, 0.0, -200.0));
        settle(&w, &mut p, 0.5);
        for _ in 0..300 {
            p.step(&w, 1.0 / 60.0, Vec2::new(0.0, 200.0), false);
        }
        assert!(p.feet.z < 300.0 && p.feet.z > 100.0, "z = {}", p.feet.z);
        assert!((p.feet.y - 30.0).abs() < 0.1, "should be standing on the 30-high step, y = {}", p.feet.y);
    }

    #[test]
    fn falling_out_of_the_world_is_rescued() {
        let w = room();
        let mut p = Player::new(Vec3::ZERO);
        settle(&w, &mut p, 0.5);
        let mut lowest = 0.0f32;
        for _ in 0..1200 {
            p.step(&w, 1.0 / 60.0, Vec2::new(300.0, 0.0), false); // keep walking off the +x edge
            lowest = lowest.min(p.feet.y);
        }
        assert!(p.rescues >= 2, "rescues: {}", p.rescues);
        // Never ends up more than one step's fall beyond the rescue depth.
        assert!(lowest > -(RESCUE_DEPTH + 400.0), "fell to {lowest}");
    }

    fn floor_only() -> CollisionWorld {
        CollisionWorld::from_triangles(quad(
            Vec3::new(-1000.0, 0.0, -1000.0),
            Vec3::new(-1000.0, 0.0, 1000.0),
            Vec3::new(1000.0, 0.0, 1000.0),
            Vec3::new(1000.0, 0.0, -1000.0),
        ))
    }

    fn walk_into(w: &CollisionWorld, p: &mut Player, secs: f32) {
        for _ in 0..(secs * 60.0) as usize {
            p.step(w, 1.0 / 60.0, Vec2::new(0.0, 300.0), false);
        }
    }

    #[test]
    fn closed_door_obstacle_blocks_and_open_door_does_not() {
        let mut w = floor_only();
        // A door: a 200-wide, 230-high vertical panel across z = 300.
        let door = w
            .add_obstacle(quad(
                Vec3::new(-100.0, 0.0, 300.0),
                Vec3::new(100.0, 0.0, 300.0),
                Vec3::new(100.0, 230.0, 300.0),
                Vec3::new(-100.0, 230.0, 300.0),
            ))
            .unwrap();
        let mut p = Player::new(Vec3::ZERO);
        settle(&w, &mut p, 0.5);
        walk_into(&w, &mut p, 4.0);
        assert!(p.feet.z < 300.0, "walked through a closed door: z = {}", p.feet.z);
        assert!(p.feet.z > 300.0 - PLAYER_RADIUS - 5.0, "should be pressed against it: z = {}", p.feet.z);

        w.set_obstacle_enabled(door, false);
        assert!(!w.obstacle_enabled(door));
        walk_into(&w, &mut p, 3.0);
        assert!(p.feet.z > 400.0, "should walk through an open door: z = {}", p.feet.z);
    }

    #[test]
    fn obstacle_does_not_block_what_is_not_in_the_way() {
        let mut w = floor_only();
        w.add_obstacle(quad(
            Vec3::new(500.0, 0.0, 0.0),
            Vec3::new(500.0, 0.0, 200.0),
            Vec3::new(500.0, 200.0, 200.0),
            Vec3::new(500.0, 200.0, 0.0),
        ));
        let mut p = Player::new(Vec3::new(0.0, 0.0, -500.0));
        settle(&w, &mut p, 0.5);
        walk_into(&w, &mut p, 3.0);
        assert!(p.feet.z > 300.0);
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
            let mut tris: Vec<[Vec3; 3]> = floor_only().tris.iter().map(|t| [t.a, t.b, t.c]).collect();
            let wall = if flip { [[a, c, b], [a, d, c]] } else { [[a, b, c], [a, c, d]] };
            tris.extend(wall);
            let w = CollisionWorld::from_triangles(tris);
            let mut p = Player::new(Vec3::ZERO);
            settle(&w, &mut p, 0.5);
            walk_into(&w, &mut p, 5.0);
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
}
