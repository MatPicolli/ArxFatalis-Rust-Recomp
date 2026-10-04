//! The game's material sound tables (`localisation/snd_step.ini`, `snd_weapon.ini`, `snd_armor.ini`) and how a floor
//! texture decides which material it is.
//!
//! ```text
//! [Foot_shoe]
//! STONE=FootStep_shoe_stone_step
//! WOOD=FootStep_shoe_wood_step
//! ```
//!
//! A section names what hits (a shoe, a sword) and each key what it hits; the value is the base name of the sample
//! in `sfx/`. Variants are `<name>.wav`, `<name>1.wav`, ... `<name>4.wav` (or `<name>_1.wav`), whichever exist.

use std::collections::HashMap;

#[derive(Debug, Default, Clone)]
pub struct SoundMap {
    /// section (lowercase) -> key (lowercase) -> sample base name (lowercase).
    sections: HashMap<String, HashMap<String, String>>,
}

impl SoundMap {
    pub fn parse(text: &str) -> Self {
        let mut map = SoundMap::default();
        let mut current: Option<String> = None;
        for raw in text.lines() {
            let line = raw.split(';').next().unwrap_or("").trim();
            if let Some(name) = line.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
                current = Some(name.trim().to_ascii_lowercase());
                continue;
            }
            let (Some(section), Some((key, value))) = (&current, line.split_once('=')) else { continue };
            map.sections
                .entry(section.clone())
                .or_default()
                .insert(key.trim().to_ascii_lowercase(), value.trim().to_ascii_lowercase());
        }
        map
    }

    /// Add what another file defines.
    pub fn merge(&mut self, other: SoundMap) {
        for (section, keys) in other.sections {
            self.sections.entry(section).or_default().extend(keys);
        }
    }

    /// Base name of the sample for `hitter` (a section such as `foot_shoe`) hitting `surface` (`stone`).
    pub fn sample(&self, hitter: &str, surface: &str) -> Option<&str> {
        self.sections.get(&hitter.to_ascii_lowercase())?.get(&surface.to_ascii_lowercase()).map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }
}

/// The material of a floor from its texture's name (`GetMaterialString`), or `unknown`.
pub fn floor_material(texture: &str) -> &'static str {
    let t = texture.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| t.contains(w));
    if has(&["stone", "marble", "rock"]) {
        "stone"
    } else if has(&["wood"]) {
        "wood"
    } else if has(&["wet", "mud", "blood", "bone", "flesh", "shit"]) {
        "wet"
    } else if has(&["soil", "gravel", "earth", "dust", "sand", "straw"]) {
        "gravel"
    } else if has(&["metal", "iron", "glass", "rust"]) {
        "metal"
    } else if has(&["ice"]) {
        "ice"
    } else if has(&["fabric", "moss"]) {
        "carpet"
    } else {
        "unknown"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sections_case_insensitively() {
        let m = SoundMap::parse("[Foot_bare]\nSTONE=FootStep_bare_stone_step\nWOOD = FootStep_bare_wood_step ; note\n\n[Foot_shoe]\nStone=shoe\n");
        assert_eq!(m.sample("foot_bare", "stone"), Some("footstep_bare_stone_step"));
        assert_eq!(m.sample("FOOT_BARE", "Wood"), Some("footstep_bare_wood_step"));
        assert_eq!(m.sample("foot_shoe", "stone"), Some("shoe"));
        assert_eq!(m.sample("foot_shoe", "wood"), None);
        let mut a = m.clone();
        a.merge(SoundMap::parse("[foot_shoe]\nwood=w\n"));
        assert_eq!(a.sample("foot_shoe", "wood"), Some("w"));
        assert_eq!(a.sample("foot_shoe", "stone"), Some("shoe"));
    }

    #[test]
    fn textures_decide_the_floor_material() {
        assert_eq!(floor_material("graph/obj3d/textures/(stone)_floor02.jpg"), "stone");
        assert_eq!(floor_material("L1_WOOD_planks"), "wood");
        assert_eq!(floor_material("graph/levels/level1/Mud_blood"), "wet");
        assert_eq!(floor_material("sand"), "gravel");
        assert_eq!(floor_material("rustyiron"), "metal", "rust and iron are metal");
        assert_eq!(floor_material("moss_wall"), "carpet");
        assert_eq!(floor_material("tapestry"), "unknown");
    }
}
