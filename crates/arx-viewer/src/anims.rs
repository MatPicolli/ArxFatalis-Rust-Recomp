//! Finding animations for a model: folder conventions and bone-count compatibility.

use arx_formats::{PakSet, tea::Tea};
use std::sync::Arc;

/// NPCs (and the player) use `anims/npc`, everything else `anims/fix_inter`.
pub fn anim_dir(class_or_model: &str) -> &'static str {
    if class_or_model.contains("/npc/") { "graph/obj3d/anims/npc" } else { "graph/obj3d/anims/fix_inter" }
}

pub fn load_anim(pak: &PakSet, path: &str) -> Option<Arc<Tea>> {
    let bytes = pak.read(path).ok()?;
    Tea::parse(&bytes).map_err(|e| eprintln!("{path}: {e}")).ok().map(Arc::new)
}

/// Number of bone groups an animation drives (read straight from the header).
fn group_count(pak: &PakSet, path: &str) -> Option<usize> {
    let bytes = pak.read(path).ok()?;
    let n = i32::from_le_bytes(bytes.get(284..288)?.try_into().ok()?);
    usize::try_from(n).ok()
}

/// All animations in the model's folder that drive exactly `bones` bones, sorted.
pub fn compatible_anims(pak: &PakSet, model: &str, bones: usize) -> Vec<String> {
    let dir = anim_dir(model);
    pak.list(dir)
        .into_iter()
        .filter(|p| p.ends_with(".tea") && p[dir.len() + 1..].find('/').is_none())
        .filter(|p| group_count(pak, p) == Some(bones))
        .map(str::to_owned)
        .collect()
}
