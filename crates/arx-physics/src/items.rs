//! Items in the world that the player moves around: where a dragged item goes (the engine's
//! `findSpotForDraggedEntity`, simplified to a ray cast) and how a thrown or dropped one falls and settles.

use crate::CollisionWorld;
use glam::Vec3;

/// How far from the player an item can be put down (`Sphere limit(player.pos, 300.f)`).
pub const DRAG_REACH: f32 = 300.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragStatus {
    /// Nothing sensible to do (no direction).
    Invalid,
    /// Nothing within reach: the item will be thrown.
    Throw,
    /// On a wall or the ceiling, or hovering: the item will fall.
    Drop,
    /// On a floor: the item will be put there.
    OnGround,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DragSpot {
    /// Where the item's origin goes (y-up).
    pub pos: Vec3,
    pub status: DragStatus,
}

/// Distance along the ray (`dir` unit) at which it leaves the sphere, or 0 if it never is inside.
fn ray_exit(origin: Vec3, dir: Vec3, center: Vec3, radius: f32) -> f32 {
    let to = origin - center;
    let b = to.dot(dir);
    let c = to.length_squared() - radius * radius;
    let disc = b * b - c;
    if disc < 0.0 {
        return 0.0;
    }
    (-b + disc.sqrt()).max(0.0)
}

/// Where a dragged item is shown: the ray from the camera through the cursor, followed until it meets level
/// geometry or leaves `DRAG_REACH` of the player. On a floor the item rests there; against a wall it hangs
/// `radius` off it (and will fall); in open space at the limit it is about to be thrown.
pub fn drag_spot(world: &CollisionWorld, origin: Vec3, dir: Vec3, player: Vec3, radius: f32) -> DragSpot {
    let dir = dir.normalize_or_zero();
    if dir == Vec3::ZERO {
        return DragSpot { pos: origin, status: DragStatus::Invalid };
    }
    let reach = ray_exit(origin, dir, player, DRAG_REACH);
    match world.raycast(origin, dir, reach.max(1.0)) {
        Some(hit) if hit.normal.y > 0.5 => DragSpot { pos: origin + dir * hit.t, status: DragStatus::OnGround },
        Some(hit) => DragSpot { pos: origin + dir * hit.t + hit.normal * radius, status: DragStatus::Drop },
        None => DragSpot { pos: origin + dir * reach, status: DragStatus::Throw },
    }
}

const GRAVITY: f32 = 2000.0;
/// Fraction of the downward speed kept when bouncing, and of the sideways speed lost on each bounce.
const BOUNCE: f32 = 0.3;
const FRICTION: f32 = 0.55;
/// Below this vertical speed a bounce is a landing.
const REST_SPEED: f32 = 120.0;

/// An item flying or falling.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ItemBody {
    pub pos: Vec3,
    pub vel: Vec3,
    pub resting: bool,
}

impl ItemBody {
    pub fn thrown(pos: Vec3, vel: Vec3) -> Self {
        ItemBody { pos, vel, resting: false }
    }

    /// Advance by `dt` seconds. Returns the speed of the impact if the item hit the floor this step.
    pub fn step(&mut self, world: &CollisionWorld, dt: f32) -> Option<f32> {
        if self.resting {
            return None;
        }
        let dt = dt.min(0.05);
        self.vel.y -= GRAVITY * dt;
        let mut next = self.pos + self.vel * dt;
        // Walls: stop at what the movement runs into and lose most of the sideways speed.
        let moved = next - self.pos;
        if let Some(hit) = world.raycast(self.pos + Vec3::Y * 5.0, moved, moved.length() + 4.0)
            && hit.normal.y < 0.5
        {
            let reflected = self.vel - 1.6 * self.vel.dot(hit.normal) * hit.normal;
            self.vel = Vec3::new(reflected.x * 0.5, self.vel.y.min(0.0), reflected.z * 0.5);
            next = self.pos;
        }
        let floor = world.floor_height(next.x, next.z, next.y.max(self.pos.y) + 40.0);
        let mut impact = None;
        match floor {
            Some(f) if next.y <= f && self.vel.y <= 0.0 => {
                next.y = f;
                let speed = -self.vel.y;
                impact = Some(speed);
                if speed < REST_SPEED {
                    self.vel = Vec3::ZERO;
                    self.resting = true;
                } else {
                    self.vel = Vec3::new(self.vel.x * FRICTION, speed * BOUNCE, self.vel.z * FRICTION);
                }
            }
            _ => {}
        }
        self.pos = next;
        impact
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn floor() -> CollisionWorld {
        let q = |a: Vec3, b: Vec3, c: Vec3, d: Vec3| [[a, b, c], [a, c, d]];
        let mut t = Vec::new();
        t.extend(q(Vec3::new(-1000.0, 0.0, -1000.0), Vec3::new(-1000.0, 0.0, 1000.0), Vec3::new(1000.0, 0.0, 1000.0), Vec3::new(1000.0, 0.0, -1000.0)));
        // A wall at z = 500.
        t.extend(q(Vec3::new(-1000.0, 0.0, 500.0), Vec3::new(1000.0, 0.0, 500.0), Vec3::new(1000.0, 300.0, 500.0), Vec3::new(-1000.0, 300.0, 500.0)));
        CollisionWorld::from_triangles(t)
    }

    #[test]
    fn dragging_over_the_floor_rests_there_over_a_wall_hangs_and_into_the_distance_throws() {
        let w = floor();
        let eye = Vec3::new(0.0, 160.0, 0.0);
        let player = eye;
        // Looking down at the floor 160 ahead: the item rests on it.
        let down = Vec3::new(0.0, -1.0, 1.0).normalize();
        let s = drag_spot(&w, eye, down, player, 20.0);
        assert_eq!(s.status, DragStatus::OnGround);
        assert!(s.pos.y.abs() < 1e-3 && (s.pos.z - 160.0).abs() < 1e-2, "{s:?}");
        // Looking at the wall at z = 500, which is beyond the 300 reach: the item is thrown.
        let s = drag_spot(&w, eye, Vec3::Z, player, 20.0);
        assert_eq!(s.status, DragStatus::Throw);
        assert!((s.pos.z - 300.0).abs() < 1.0, "{s:?}");
        // From closer, the wall is within reach: the item hangs off it, facing the player.
        let near = Vec3::new(0.0, 160.0, 300.0);
        let s = drag_spot(&w, near, Vec3::Z, near, 20.0);
        assert_eq!(s.status, DragStatus::Drop);
        assert!((s.pos.z - 480.0).abs() < 1e-2, "{s:?}");
        assert_eq!(drag_spot(&w, eye, Vec3::ZERO, player, 20.0).status, DragStatus::Invalid);
    }

    #[test]
    fn a_dropped_item_falls_bounces_and_comes_to_rest_on_the_floor() {
        let w = floor();
        let mut b = ItemBody::thrown(Vec3::new(0.0, 150.0, 0.0), Vec3::ZERO);
        let mut impacts = Vec::new();
        for _ in 0..600 {
            if let Some(v) = b.step(&w, 1.0 / 60.0) {
                impacts.push(v);
            }
        }
        assert!(b.resting && b.pos.y.abs() < 1e-3, "{b:?}");
        assert!(impacts.len() >= 2, "it bounces before it settles: {impacts:?}");
        assert!(impacts.windows(2).all(|p| p[1] < p[0]), "each bounce is weaker: {impacts:?}");
    }

    #[test]
    fn a_thrown_item_flies_forward_and_stops_at_a_wall() {
        let w = floor();
        let mut b = ItemBody::thrown(Vec3::new(0.0, 100.0, 0.0), Vec3::new(0.0, 100.0, 900.0));
        for _ in 0..600 {
            b.step(&w, 1.0 / 60.0);
        }
        assert!(b.resting);
        assert!(b.pos.z > 100.0, "it travelled: {:?}", b.pos);
        assert!(b.pos.z < 500.0, "but not through the wall: {:?}", b.pos);
    }
}
