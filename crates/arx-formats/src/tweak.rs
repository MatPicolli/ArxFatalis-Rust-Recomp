//! Mesh tweaks: a character built from parts of two models (`EERIE_MESH_TWEAK_Do`). Humanoid models mark their
//! vertices with three selections, `head`, `chest` and `leggings`; a tweak takes one of these parts from another
//! model and the other two from the base. That is how one `human_base` becomes a prisoner, a guard or a priest, and
//! how armour shows on the hero. Vertices are matched between the models by position, as the engine does.

use crate::ftl::{Action, Face, Ftl, Group, Selection, Vertex};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BodyPart {
    Head,
    Torso,
    Legs,
}

impl BodyPart {
    fn selection(self) -> &'static str {
        match self {
            BodyPart::Head => "head",
            BodyPart::Torso => "chest",
            BodyPart::Legs => "leggings",
        }
    }
}

/// A position as something that can be looked up exactly.
fn key(pos: [f32; 3]) -> [u32; 3] {
    // -0.0 and 0.0 are the same place.
    pos.map(|c| if c == 0.0 { 0 } else { c.to_bits() })
}

/// The model being put together.
#[derive(Default)]
struct Work {
    vertices: Vec<Vertex>,
    by_pos: HashMap<[u32; 3], u32>,
    faces: Vec<Face>,
    seen_faces: HashSet<[[u32; 3]; 3]>,
    textures: Vec<String>,
    actions: Vec<Action>,
}

impl Work {
    fn find(&self, pos: [f32; 3]) -> Option<u32> {
        self.by_pos.get(&key(pos)).copied()
    }

    /// The vertex at that place, added if there is none yet.
    fn add_vertex(&mut self, v: &Vertex) -> u32 {
        if let Some(i) = self.find(v.pos) {
            return i;
        }
        let i = self.vertices.len() as u32;
        self.vertices.push(*v);
        self.by_pos.insert(key(v.pos), i);
        i
    }

    fn add_action(&mut self, name: &str, v: &Vertex) {
        let vertex = self.add_vertex(v);
        if !self.actions.iter().any(|a| a.name == name) {
            self.actions.push(Action { name: name.to_owned(), vertex });
        }
    }

    fn add_texture(&mut self, name: &str) {
        if !self.textures.iter().any(|t| t == name) {
            self.textures.push(name.to_owned());
        }
    }

    fn add_face(&mut self, face: &Face, src: &Ftl) {
        let corners = face.vid.map(|i| key(src.vertices[i as usize].pos));
        if !self.seen_faces.insert(corners) {
            return;
        }
        let mut new = face.clone();
        new.vid = face.vid.map(|i| self.add_vertex(&src.vertices[i as usize]) as u16);
        // The same texture, wherever it now is in the list (the first one if it is not there, as the engine does).
        new.material = face.material.map(|m| {
            let name = src.textures.get(m as usize);
            self.textures.iter().position(|t| Some(t) == name).unwrap_or(0) as u16
        });
        self.faces.push(new);
    }
}

fn selection(ftl: &Ftl, name: &str) -> Option<HashSet<u32>> {
    ftl.selections.iter().find(|s| s.name.eq_ignore_ascii_case(name)).map(|s| s.vertices.iter().copied().collect())
}

fn has_action(ftl: &Ftl, name: &str) -> bool {
    ftl.actions.iter().any(|a| a.name.eq_ignore_ascii_case(name))
}

impl Ftl {
    /// This model with one part replaced by `other`'s (`CreateIntermediaryMesh`). `None` if either model is not
    /// built for it: both need the three part selections and the `head2chest` and `chest2leggings` seam vertices.
    pub fn with_part_from(&self, other: &Ftl, part: BodyPart) -> Option<Ftl> {
        let parts = [BodyPart::Head, BodyPart::Torso, BodyPart::Legs];
        // Vertices of this model that stay (the two other parts), and of the other model that come in.
        let mut kept: HashSet<u32> = HashSet::new();
        for p in parts {
            let sel = selection(self, p.selection())?;
            selection(other, p.selection())?;
            if p != part {
                kept.extend(sel);
            }
        }
        let replaced = selection(self, part.selection())?;
        let taken = selection(other, part.selection())?;
        for seam in ["head2chest", "chest2leggings"] {
            if !has_action(self, seam) || !has_action(other, seam) {
                return None;
            }
        }

        let mut w = Work::default();
        let origin = if replaced.contains(&self.origin) {
            w.add_vertex(other.vertices.get(other.origin as usize)?)
        } else {
            w.add_vertex(self.vertices.get(self.origin as usize)?)
        };
        // Named vertices (where things attach) of the parts that make up the result, and the seams of both.
        let is_seam = |name: &str| name.eq_ignore_ascii_case("head2chest") || name.eq_ignore_ascii_case("chest2leggings");
        for a in &self.actions {
            if kept.contains(&a.vertex) || is_seam(&a.name) {
                w.add_action(&a.name, &self.vertices[a.vertex as usize]);
            }
        }
        for a in &other.actions {
            if taken.contains(&a.vertex) || is_seam(&a.name) {
                w.add_action(&a.name, &other.vertices[a.vertex as usize]);
            }
        }
        for (i, v) in self.vertices.iter().enumerate() {
            if kept.contains(&(i as u32)) {
                w.add_vertex(v);
            }
        }
        for (i, v) in other.vertices.iter().enumerate() {
            if taken.contains(&(i as u32)) {
                w.add_vertex(v);
            }
        }
        // Faces: of this model those wholly in what stays, of the other those touching what comes in.
        for f in &self.faces {
            if f.vid.iter().all(|&i| kept.contains(&u32::from(i))) {
                if let Some(name) = f.material.and_then(|m| self.textures.get(m as usize)) {
                    w.add_texture(name);
                }
                w.add_face(f, self);
            }
        }
        for f in &other.faces {
            if f.vid.iter().any(|&i| taken.contains(&u32::from(i))) {
                if let Some(name) = f.material.and_then(|m| other.textures.get(m as usize)) {
                    w.add_texture(name);
                }
                w.add_face(f, other);
            }
        }

        // Bones: the same list (the longer of the two), each with its origin and vertices from whichever model has
        // that part of the body.
        let count = self.groups.len().max(other.groups.len());
        let mut groups: Vec<Group> = (0..count).map(|_| Group { name: String::new(), origin: 0, indices: Vec::new(), blob_shadow_size: 0.0 }).collect();
        for (g, src) in self.groups.iter().enumerate() {
            groups[g].name = src.name.clone();
            if let Some(v) = self.vertices.get(src.origin as usize).and_then(|v| w.find(v.pos)) {
                groups[g].blob_shadow_size = src.blob_shadow_size;
                if kept.contains(&src.origin) {
                    groups[g].origin = v;
                }
            }
        }
        for (g, src) in other.groups.iter().enumerate() {
            if g >= self.groups.len() {
                groups[g].name = src.name.clone();
            }
            if let Some(v) = other.vertices.get(src.origin as usize).and_then(|v| w.find(v.pos)) {
                groups[g].blob_shadow_size = src.blob_shadow_size;
                if taken.contains(&src.origin) {
                    groups[g].origin = v;
                }
            }
        }
        for (model, list) in [(self, &self.groups), (other, &other.groups)] {
            for (g, src) in list.iter().enumerate() {
                let mut have: HashSet<u32> = groups[g].indices.iter().copied().collect();
                for &i in &src.indices {
                    if let Some(v) = model.vertices.get(i as usize).and_then(|v| w.find(v.pos))
                        && have.insert(v)
                    {
                        groups[g].indices.push(v);
                    }
                }
            }
        }

        // Selections: the three parts first (so the result can be tweaked again), then every other one, merged.
        let copy = |w: &Work, model: &Ftl, name: &str, into: &mut Vec<u32>| {
            let Some(sel) = model.selections.iter().find(|s| s.name.eq_ignore_ascii_case(name)) else { return };
            for &i in &sel.vertices {
                if let Some(v) = model.vertices.get(i as usize).and_then(|v| w.find(v.pos))
                    && !into.contains(&v)
                {
                    into.push(v);
                }
            }
        };
        let mut selections: Vec<Selection> = Vec::new();
        for p in parts {
            let mut vertices = Vec::new();
            copy(&w, if p == part { other } else { self }, p.selection(), &mut vertices);
            selections.push(Selection { name: p.selection().to_owned(), vertices });
        }
        for model in [self, other] {
            for s in &model.selections {
                if selections.iter().any(|done| done.name.eq_ignore_ascii_case(&s.name)) {
                    continue;
                }
                let mut vertices = Vec::new();
                copy(&w, self, &s.name, &mut vertices);
                copy(&w, other, &s.name, &mut vertices);
                selections.push(Selection { name: s.name.clone(), vertices });
            }
        }

        Some(Ftl { name: self.name.clone(), origin, vertices: w.vertices, faces: w.faces, textures: w.textures, groups, actions: w.actions, selections })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A body of three stacked triangles (head, chest, legs) that share their seam vertices; `x` tells two apart.
    fn body(x: f32, texture: &str) -> Ftl {
        let v = |pos: [f32; 3]| Vertex { pos, norm: [0.0, 0.0, 1.0] };
        // 0,1: top of the head; 2: head/chest seam; 3: chest side; 4: chest/legs seam; 5,6: feet.
        let vertices = vec![v([x, -30.0, 0.0]), v([x + 1.0, -30.0, 0.0]), v([0.0, -20.0, 0.0]), v([x, -15.0, 0.0]), v([0.0, -10.0, 0.0]), v([x, 0.0, 0.0]), v([x + 1.0, 0.0, 0.0])];
        let face = |vid: [u16; 3]| Face { facetype: 1, material: Some(0), vid, u: [0.0; 3], v: [0.0; 3], transval: 0.0, norm: [0.0, 0.0, 1.0] };
        Ftl {
            name: "body".into(),
            origin: 4,
            vertices,
            faces: vec![face([0, 1, 2]), face([2, 3, 4]), face([4, 5, 6])],
            textures: vec![texture.into()],
            groups: vec![
                Group { name: "all".into(), origin: 4, indices: vec![2, 3, 4, 5, 6], blob_shadow_size: 1.0 },
                Group { name: "head".into(), origin: 2, indices: vec![0, 1], blob_shadow_size: 0.5 },
            ],
            actions: vec![
                Action { name: "head2chest".into(), vertex: 2 },
                Action { name: "chest2leggings".into(), vertex: 4 },
                Action { name: "view_attach".into(), vertex: 0 },
                Action { name: "primary_attach".into(), vertex: 3 },
            ],
            selections: vec![
                Selection { name: "head".into(), vertices: vec![0, 1] },
                Selection { name: "chest".into(), vertices: vec![2, 3] },
                Selection { name: "leggings".into(), vertices: vec![4, 5, 6] },
            ],
        }
    }

    #[test]
    fn a_head_from_another_model_joins_the_body_at_the_seam() {
        let (base, kultar) = (body(1.0, "hero"), body(5.0, "old_man"));
        let made = base.with_part_from(&kultar, BodyPart::Head).expect("both have the parts and seams");
        // Chest and legs of the base (5 vertices, the seam shared) and the other head's two top vertices.
        assert_eq!(made.vertices.len(), 7);
        assert_eq!(made.faces.len(), 3);
        let pos = |i: u16| made.vertices[i as usize].pos;
        let head = made.faces.iter().find(|f| f.vid.iter().any(|&i| pos(i)[1] == -30.0)).unwrap();
        assert!(head.vid.iter().any(|&i| pos(i) == [5.0, -30.0, 0.0]), "the head is the other model's");
        assert!(made.faces.iter().any(|f| f.vid.iter().any(|&i| pos(i) == [1.0, 0.0, 0.0])), "the feet are the base's");
        // Each face keeps its own texture.
        assert_eq!(made.textures, ["hero", "old_man"]);
        assert_eq!(made.textures[head.material.unwrap() as usize], "old_man");
        // The head's bone holds the new head's vertices; the body's bone its own; the origin is still the hips.
        let at = |g: &Group| g.indices.iter().map(|&i| made.vertices[i as usize].pos[0]).fold(f32::MIN, f32::max);
        assert_eq!((made.groups[1].indices.len(), at(&made.groups[1])), (2, 6.0));
        assert_eq!(made.groups[0].indices.len(), 5);
        assert_eq!(made.vertices[made.origin as usize].pos, [0.0, -10.0, 0.0]);
        assert_eq!(made.vertices[made.groups[1].origin as usize].pos, [0.0, -20.0, 0.0]);
        // Named vertices follow their part: the eyes are on the new head, the hand on the old chest.
        let named = |n: &str| made.vertices[made.actions.iter().find(|a| a.name == n).unwrap().vertex as usize].pos;
        assert_eq!(named("view_attach"), [5.0, -30.0, 0.0]);
        assert_eq!(named("primary_attach"), [1.0, -15.0, 0.0]);
        // The result has the three parts again, so legs can be swapped next.
        let again = made.with_part_from(&kultar, BodyPart::Legs).expect("can be tweaked again");
        assert!(again.faces.iter().any(|f| f.vid.iter().any(|&i| again.vertices[i as usize].pos == [5.0, 0.0, 0.0])));
        // A face on the seam comes with the part that is brought in (it touches it), so nothing is left open.
        assert_eq!(again.faces.len(), 3);
    }

    #[test]
    fn models_without_the_parts_are_left_alone() {
        let base = body(1.0, "hero");
        let mut barrel = body(2.0, "wood");
        barrel.selections.clear();
        assert!(base.with_part_from(&barrel, BodyPart::Head).is_none());
        let mut no_seam = body(2.0, "wood");
        no_seam.actions.retain(|a| a.name != "head2chest");
        assert!(base.with_part_from(&no_seam, BodyPart::Torso).is_none());
    }
}
