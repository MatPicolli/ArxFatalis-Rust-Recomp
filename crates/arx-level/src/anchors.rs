//! The walkable graph NPCs path-find on: the level's anchors (points with a free radius and height, linked to their
//! neighbours) and the searches the engine runs on them: a weighted A* between two anchors, fleeing from a danger,
//! wandering around a spot and looking for someone.

use arx_formats::fts::Anchor;
use glam::Vec3;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet};

/// Below this a wander or search just stays where it is.
const MIN_RADIUS: f32 = 110.0;
/// How much farther than the safe distance a flee counts for, per unit.
const FLEE_DISTANCE_COST: f32 = 130.0;
pub const HEURISTIC_MIN: f32 = 0.2;
pub const HEURISTIC_MAX: f32 = 0.5;
/// Distance beyond which the search is as greedy as it gets.
const HEURISTIC_RANGE_DISTANCE: f32 = 5000.0;

/// The weight the engine gives the distance travelled (against the estimate of what remains): short trips are
/// searched greedily, long ones evenly.
pub fn heuristic_for(distance: f32) -> f32 {
    if distance < HEURISTIC_RANGE_DISTANCE {
        HEURISTIC_MIN + (HEURISTIC_MAX - HEURISTIC_MIN) * (distance / HEURISTIC_RANGE_DISTANCE)
    } else {
        HEURISTIC_MAX
    }
}

/// A small deterministic random source (xorshift) for wandering and searching.
#[derive(Debug, Clone)]
pub struct Rng(pub u32);

impl Rng {
    pub fn next_f32(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }

    /// An integer in `lo..=hi`.
    pub fn range(&mut self, lo: u32, hi: u32) -> u32 {
        if hi <= lo {
            return lo;
        }
        lo + (self.next_f32() * (hi - lo + 1) as f32) as u32
    }
}

/// One anchor: a spot with a free radius and height, linked to the anchors next to it.
#[derive(Debug, Clone)]
pub struct Node {
    pub pos: Vec3,
    pub radius: f32,
    pub height: f32,
    pub blocked: bool,
    pub links: Vec<i32>,
}

impl From<&Anchor> for Node {
    fn from(a: &Anchor) -> Self {
        Node { pos: Vec3::from(a.pos), radius: a.radius, height: a.height, blocked: a.blocked, links: a.links.clone() }
    }
}

pub struct AnchorGraph {
    pub anchors: Vec<Node>,
}

/// What passes through the graph: the size of the body that has to fit.
#[derive(Debug, Clone, Copy)]
pub struct Body {
    pub radius: f32,
    pub height: f32,
}

#[derive(PartialEq)]
struct Open {
    cost: f32,
    id: usize,
}

impl Eq for Open {}

impl Ord for Open {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap is a max-heap: the cheapest comes first.
        other.cost.total_cmp(&self.cost).then(other.id.cmp(&self.id))
    }
}

impl PartialOrd for Open {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl AnchorGraph {
    pub fn new(anchors: Vec<Node>) -> Self {
        AnchorGraph { anchors }
    }

    /// The graph of a level.
    pub fn from_fts(anchors: &[Anchor]) -> Self {
        AnchorGraph { anchors: anchors.iter().map(Node::from).collect() }
    }

    pub fn is_empty(&self) -> bool {
        self.anchors.is_empty()
    }

    fn passable(&self, id: usize, body: Body) -> bool {
        let a = &self.anchors[id];
        !a.blocked && a.height <= body.height && a.radius >= body.radius
    }

    fn neighbours(&self, id: usize) -> impl Iterator<Item = usize> + '_ {
        self.anchors[id].links.iter().filter_map(|&l| usize::try_from(l).ok()).filter(|&l| l < self.anchors.len())
    }

    /// The anchor nearest to `pos` that a body of this size can use (`except` is left out).
    pub fn nearest(&self, pos: Vec3, body: Body, except: Option<usize>) -> Option<usize> {
        let mut best: Option<(f32, usize)> = None;
        for (i, a) in self.anchors.iter().enumerate() {
            if Some(i) == except || a.links.is_empty() || a.blocked || a.height > body.height || a.radius < body.radius {
                continue;
            }
            let d = a.pos.distance_squared(pos);
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, i));
            }
        }
        best.map(|(_, i)| i)
    }

    /// The anchor nearest to `pos` that has any link and is not blocked, whatever the body (for searches).
    fn nearest_any(&self, pos: Vec3) -> usize {
        let mut best = (f32::MAX, 0);
        for (i, a) in self.anchors.iter().enumerate() {
            let d = a.pos.distance_squared(pos);
            if d < best.0 && !a.links.is_empty() && !a.blocked {
                best = (d, i);
            }
        }
        best.1
    }

    fn build(parents: &[Option<usize>], mut at: usize) -> Vec<usize> {
        let mut out = vec![at];
        while let Some(p) = parents[at] {
            out.push(p);
            at = p;
        }
        out.reverse();
        out
    }

    /// Weighted A* from `from` to `to`; the path includes both ends. `heuristic` is the weight of the distance
    /// already travelled (`0.5` is plain A*, less is greedier).
    pub fn path(&self, from: usize, to: usize, body: Body, heuristic: f32) -> Option<Vec<usize>> {
        if from == to {
            return Some(vec![to]);
        }
        let n = self.anchors.len();
        let mut parents: Vec<Option<usize>> = vec![None; n];
        let mut best_g = vec![f32::INFINITY; n];
        let mut closed = vec![false; n];
        let mut open = BinaryHeap::new();
        best_g[from] = 0.0;
        open.push(Open { cost: 0.0, id: from });
        let target = self.anchors[to].pos;
        while let Some(Open { id, .. }) = open.pop() {
            if closed[id] {
                continue;
            }
            closed[id] = true;
            if id == to {
                return Some(Self::build(&parents, id));
            }
            for c in self.neighbours(id) {
                if closed[c] || !self.passable(c, body) {
                    continue;
                }
                let g = best_g[id] + self.anchors[c].pos.distance(self.anchors[id].pos) * heuristic;
                if g < best_g[c] {
                    best_g[c] = g;
                    parents[c] = Some(id);
                    let remaining = (1.0 - heuristic) * self.anchors[c].pos.distance(target);
                    open.push(Open { cost: g + remaining, id: c });
                }
            }
        }
        None
    }

    /// Run away from `danger` until `safe_distance` from it: the path to the first anchor that is far enough.
    pub fn flee(&self, from: usize, danger: Vec3, safe_distance: f32, body: Body) -> Option<Vec<usize>> {
        if self.anchors[from].pos.distance(danger) >= safe_distance {
            return Some(vec![from]);
        }
        let n = self.anchors.len();
        let mut parents: Vec<Option<usize>> = vec![None; n];
        let mut best_g = vec![f32::INFINITY; n];
        let mut closed = vec![false; n];
        let mut open = BinaryHeap::new();
        best_g[from] = 0.0;
        open.push(Open { cost: 0.0, id: from });
        let remaining_of = |id: usize| (safe_distance - self.anchors[id].pos.distance(danger)).max(0.0) * FLEE_DISTANCE_COST;
        while let Some(Open { id, .. }) = open.pop() {
            if closed[id] {
                continue;
            }
            closed[id] = true;
            if remaining_of(id) == 0.0 {
                return Some(Self::build(&parents, id));
            }
            for c in self.neighbours(id) {
                if closed[c] || !self.passable(c, body) {
                    continue;
                }
                let g = best_g[id] + self.anchors[c].pos.distance(self.anchors[id].pos);
                if g < best_g[c] {
                    best_g[c] = g;
                    parents[c] = Some(id);
                    open.push(Open { cost: g + remaining_of(c), id: c });
                }
            }
        }
        None
    }

    /// A random patrol around `from`: a handful of stops within about `radius`, each reached by the shortest way and
    /// the whole route closed by coming back. Consecutive stops repeat the anchor they share, which is where the
    /// engine's characters pause.
    pub fn wander(&self, from: usize, radius: f32, body: Body, rng: &mut Rng) -> Option<Vec<usize>> {
        if self.anchors[from].links.is_empty() {
            return None;
        }
        if radius <= MIN_RADIUS {
            return Some(vec![from]);
        }
        let mut out = Vec::new();
        let mut last = from;
        let steps = rng.range(4, 8);
        let mut attempts = 0;
        let mut i = 0;
        while i < steps && attempts < 40 {
            attempts += 1;
            let mut next = from;
            let walk = rng.range(0, (radius / 50.0) as u32);
            for _ in 0..walk {
                if self.anchors[next].links.is_empty() {
                    break;
                }
                for _ in 0..4 {
                    let links = &self.anchors[next].links;
                    let candidate = links[rng.range(0, links.len() as u32 - 1) as usize];
                    let Ok(c) = usize::try_from(candidate) else { continue };
                    if c < self.anchors.len() && self.passable(c, body) && !self.anchors[c].links.is_empty() {
                        next = c;
                        break;
                    }
                }
            }
            match self.path(last, next, body, HEURISTIC_MAX) {
                Some(p) => {
                    out.extend(p);
                    last = next;
                    i += 1;
                }
                None => {}
            }
        }
        if out.is_empty() {
            return None;
        }
        out.extend(self.path(last, from, body, HEURISTIC_MAX)?);
        Some(out)
    }

    /// Search around `pos` (where somebody was last seen): a few stops at random places within `radius` of it.
    pub fn look_for(&self, from: usize, pos: Vec3, radius: f32, body: Body, rng: &mut Rng) -> Option<Vec<usize>> {
        if radius <= MIN_RADIUS {
            return Some(vec![from]);
        }
        let to = self.nearest_any(pos);
        let mut out = Vec::new();
        let mut last = from;
        let steps = rng.range(4, 8);
        let mut attempts = 0;
        let mut i = 0;
        while i < steps && attempts < 40 {
            attempts += 1;
            let jitter = Vec3::new(rng.next_f32() * 2.0 - 1.0, rng.next_f32() * 2.0 - 1.0, rng.next_f32() * 2.0 - 1.0) * radius;
            let next = self.nearest_any(self.anchors[to].pos + jitter);
            if let Some(p) = self.path(last, next, body, HEURISTIC_MAX) {
                out.extend(p);
                i += 1;
            }
            last = next;
        }
        (!out.is_empty()).then_some(out)
    }

    /// Anchors reachable from `from` (for checking that a level's graph is connected).
    pub fn reachable(&self, from: usize) -> HashSet<usize> {
        let mut seen = HashSet::from([from]);
        let mut stack = vec![from];
        while let Some(a) = stack.pop() {
            for c in self.neighbours(a) {
                if !self.anchors[c].blocked && seen.insert(c) {
                    stack.push(c);
                }
            }
        }
        seen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line of anchors 100 apart along x, with a detour: 0-1-2-3-4, and a shortcut 1-5-3 where 5 is blocked.
    fn graph() -> AnchorGraph {
        let node = |x: f32, z: f32, links: &[i32], blocked: bool| Node { pos: Vec3::new(x, 0.0, z), radius: 50.0, height: 100.0, blocked, links: links.to_vec() };
        AnchorGraph::new(vec![
            node(0.0, 0.0, &[1], false),
            node(100.0, 0.0, &[0, 2, 5], false),
            node(200.0, 0.0, &[1, 3], false),
            node(300.0, 0.0, &[2, 4, 5], false),
            node(400.0, 0.0, &[3], false),
            node(200.0, 10.0, &[1, 3], true),
        ])
    }

    const BODY: Body = Body { radius: 40.0, height: 170.0 };

    #[test]
    fn a_path_goes_through_the_links_and_around_blocked_anchors() {
        let g = graph();
        assert_eq!(g.path(0, 4, BODY, 0.5).unwrap(), vec![0, 1, 2, 3, 4]);
        assert_eq!(g.path(2, 2, BODY, 0.5).unwrap(), vec![2]);
        // A body too wide for the anchors finds nothing.
        assert!(g.path(0, 4, Body { radius: 80.0, height: 170.0 }, 0.5).is_none());
    }

    #[test]
    fn the_nearest_anchor_must_fit_the_body() {
        let g = graph();
        assert_eq!(g.nearest(Vec3::new(210.0, 0.0, 5.0), BODY, None), Some(2), "the blocked anchor 5 is closer but unusable");
        assert_eq!(g.nearest(Vec3::new(210.0, 0.0, 5.0), BODY, Some(2)), Some(3), "leaving 2 out, 3 is the next closest");
        assert_eq!(g.nearest(Vec3::ZERO, Body { radius: 90.0, height: 170.0 }, None), None);
    }

    #[test]
    fn fleeing_ends_far_enough_from_the_danger() {
        let g = graph();
        let p = g.flee(2, Vec3::new(0.0, 0.0, 0.0), 350.0, BODY).unwrap();
        assert_eq!(*p.first().unwrap(), 2);
        assert!(g.anchors[*p.last().unwrap()].pos.distance(Vec3::ZERO) >= 350.0, "{p:?}");
        // Already safe: stay.
        assert_eq!(g.flee(4, Vec3::ZERO, 350.0, BODY).unwrap(), vec![4]);
    }

    #[test]
    fn wandering_comes_back_and_repeats_the_anchors_where_it_pauses() {
        let g = graph();
        let mut rng = Rng(12345);
        let p = g.wander(2, 300.0, BODY, &mut rng).unwrap();
        assert_eq!(*p.first().unwrap(), 2);
        assert_eq!(*p.last().unwrap(), 2, "the patrol is closed");
        assert!(p.windows(2).any(|w| w[0] == w[1]), "pauses show up as repeated anchors: {p:?}");
        assert_eq!(g.wander(2, 50.0, BODY, &mut rng).unwrap(), vec![2], "too small a radius: stay");
    }

    #[test]
    fn heuristic_is_greedier_for_short_trips() {
        assert_eq!(heuristic_for(0.0), HEURISTIC_MIN);
        assert_eq!(heuristic_for(9000.0), HEURISTIC_MAX);
        assert!(heuristic_for(1000.0) < heuristic_for(3000.0));
    }

    #[test]
    fn reachability_skips_blocked_anchors() {
        let g = graph();
        let r = g.reachable(0);
        assert!(r.contains(&4) && !r.contains(&5));
    }
}
