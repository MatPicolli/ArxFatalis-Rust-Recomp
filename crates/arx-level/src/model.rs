//! The model an entity is drawn with after its scripts changed its looks (`tweak`): body parts taken from other
//! models and textures swapped, applied in the order the scripts gave them (`ARX_INTERACTIVE_APPLY_TWEAK_INFO`).

use arx_formats::ftl::Ftl;
use arx_formats::tweak::BodyPart;
use arx_script::{Tweak, body_part};
use std::sync::Arc;

/// A model with its tweaks applied.
pub struct Tweaked {
    pub ftl: Ftl,
    /// Tweaks that could not be applied, and why.
    pub failed: Vec<String>,
}

/// A texture's name as `tweak skin` knows it: lowercase, without folder or extension.
pub fn texture_stem(path: &str) -> String {
    let path = path.to_ascii_lowercase().replace('\\', "/");
    let file = path.rsplit('/').next().unwrap_or("");
    file.rsplit_once('.').map_or(file, |(stem, _)| stem).to_owned()
}

/// Apply `tweaks` to `base`. `load` reads another model by its path (`game/....ftl`).
pub fn apply_tweaks(base: &Ftl, tweaks: &[Tweak], load: &mut dyn FnMut(&str) -> Option<Arc<Ftl>>) -> Tweaked {
    let mut ftl = base.clone();
    let mut failed = Vec::new();
    // The textures the current model had before any was swapped: a swap names those first, and only if none
    // matches what is drawn now (`EERIE_MESH_TWEAK_Skin`).
    let mut original: Option<Vec<String>> = None;
    for tweak in tweaks {
        match tweak {
            Tweak::Remove => {
                ftl = base.clone();
                original = None;
            }
            Tweak::Part { parts, mesh } => {
                let path = format!("game/{mesh}.ftl");
                let Some(other) = load(&path) else {
                    failed.push(format!("{path}: no such model"));
                    continue;
                };
                if *parts == body_part::ALL {
                    ftl = (*other).clone();
                    original = None;
                    continue;
                }
                // All the parts asked for, or none.
                let mut made = Some(ftl.clone());
                for (bit, part) in [(body_part::HEAD, BodyPart::Head), (body_part::TORSO, BodyPart::Torso), (body_part::LEGS, BodyPart::Legs)] {
                    if parts & bit != 0 {
                        made = made.and_then(|m| m.with_part_from(&other, part));
                    }
                }
                match made {
                    Some(m) => {
                        ftl = m;
                        original = None;
                    }
                    None => failed.push(format!("{path}: a model lacks the head/chest/leggings parts or their seams")),
                }
            }
            Tweak::Skin { from, to } => {
                let names = original.get_or_insert_with(|| ftl.textures.iter().map(|t| texture_stem(t)).collect());
                let new = format!("graph/obj3d/textures/{to}");
                let mut found = false;
                for (i, name) in names.iter().enumerate() {
                    if name == from {
                        ftl.textures[i] = new.clone();
                        found = true;
                    }
                }
                if !found {
                    for t in &mut ftl.textures {
                        if texture_stem(t) == *from {
                            *t = new.clone();
                            found = true;
                        }
                    }
                }
                if !found {
                    failed.push(format!("skin {from} -> {to}: the model has no such texture"));
                }
            }
        }
    }
    Tweaked { ftl, failed }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arx_formats::ftl::{Face, Vertex};

    fn model(textures: &[&str]) -> Ftl {
        Ftl {
            name: "m".into(),
            origin: 0,
            vertices: vec![Vertex { pos: [0.0; 3], norm: [0.0; 3] }; 3],
            faces: vec![Face { facetype: 1, material: Some(0), vid: [0, 1, 2], u: [0.0; 3], v: [0.0; 3], transval: 0.0, norm: [0.0; 3] }],
            textures: textures.iter().map(|t| (*t).to_owned()).collect(),
            groups: Vec::new(),
            actions: Vec::new(),
            selections: Vec::new(),
        }
    }

    #[test]
    fn skins_are_swapped_by_their_first_name_then_by_their_current_one() {
        let base = model(&["GRAPH\\OBJ3D\\TEXTURES\\NPC_GOBLIN_BASE_HEAD.BMP", "GRAPH\\OBJ3D\\TEXTURES\\NPC_GOBLIN_BASE_BODY.BMP"]);
        let skin = |from: &str, to: &str| Tweak::Skin { from: from.into(), to: to.into() };
        let mut none = |_: &str| None;
        let t = apply_tweaks(&base, &[skin("npc_goblin_base_head", "npc_goblin_grey_head")], &mut none);
        assert_eq!(t.ftl.textures[0], "graph/obj3d/textures/npc_goblin_grey_head");
        assert_eq!(t.ftl.textures[1], base.textures[1]);
        assert!(t.failed.is_empty());
        // Swapped again under its first name: that still means this texture.
        let t = apply_tweaks(&base, &[skin("npc_goblin_base_head", "a"), skin("npc_goblin_base_head", "b")], &mut none);
        assert_eq!(t.ftl.textures[0], "graph/obj3d/textures/b");
        // Or under the name it has now.
        let t = apply_tweaks(&base, &[skin("npc_goblin_base_head", "a"), skin("a", "c")], &mut none);
        assert_eq!(t.ftl.textures[0], "graph/obj3d/textures/c");
        // A texture the model does not have, and a model that does not exist, are reported and change nothing.
        let t = apply_tweaks(&base, &[skin("nothing", "a"), Tweak::Part { parts: body_part::HEAD, mesh: "x/tweaks/y".into() }], &mut none);
        assert_eq!((t.ftl.textures.clone(), t.failed.len()), (base.textures.clone(), 2));
        // `all` replaces the model; `remove` brings the first one back.
        let other = Arc::new(model(&["other"]));
        let mut load = |_: &str| Some(other.clone());
        let t = apply_tweaks(&base, &[Tweak::Part { parts: body_part::ALL, mesh: "x/tweaks/y".into() }], &mut load);
        assert_eq!(t.ftl.textures, ["other"]);
        let t = apply_tweaks(&base, &[Tweak::Part { parts: body_part::ALL, mesh: "x/tweaks/y".into() }, Tweak::Remove], &mut load);
        assert_eq!(t.ftl.textures, base.textures);
    }
}
