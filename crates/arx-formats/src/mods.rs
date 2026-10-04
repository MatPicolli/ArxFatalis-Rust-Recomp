//! Mods: drop something into the `mods` folder and the game uses it.
//!
//! A mod is either a **folder** or a **`.pak` archive** in the mods folder. Inside, files sit at the same paths the
//! game's own files have (`graph/obj3d/textures/...`, `graph/obj3d/interactive/.../x.asl`, `game/graph/levels/...`,
//! `sfx/...`, `speech/english/...`, `localisation/...`, `misc/...`); a file a mod provides replaces the game's file of
//! that name, and files the game does not have are simply added. Mods are applied in alphabetical order, so of two
//! mods that provide the same file the later one wins.
//!
//! A mod may describe itself in a `mod.ini` at its top:
//!
//! ```ini
//! name = Brighter torches
//! author = Somebody
//! version = 1.2
//! description = Torches reach twice as far.
//! ```
//!
//! Which mods are switched off is remembered in the user's settings folder ([`config_dir`]), not in the mods folder,
//! so that a mod stays a thing one can copy around untouched.

use crate::pak::{PakError, PakSet, normalize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// What a mod is on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModKind {
    Folder,
    Archive,
}

/// A mod found in the mods folder.
#[derive(Debug, Clone, PartialEq)]
pub struct Mod {
    /// The name of its folder or archive in the mods folder (without `.pak`), lowercase: what identifies it.
    pub id: String,
    /// What it calls itself (`name` in `mod.ini`), or its file name.
    pub name: String,
    pub author: String,
    pub version: String,
    pub description: String,
    pub path: PathBuf,
    pub kind: ModKind,
    pub enabled: bool,
    /// Files it holds (not counting `mod.ini`), and how many of them replace a file the game already had.
    pub files: usize,
    pub replaces: usize,
}

/// The `key = value` lines of a `mod.ini` (keys lowercase; `;` and `#` start a comment line).
pub fn parse_ini(text: &str) -> Vec<(String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with([';', '#', '[']))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().trim_matches('"').to_owned()))
        .collect()
}

/// The user's settings folder for this game (options, which mods are off): `%APPDATA%\arx-fatalis-rust` on Windows,
/// `~/.config/arx-fatalis-rust` elsewhere.
pub fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("arx-fatalis-rust"))
}

fn disabled_file() -> Option<PathBuf> {
    config_dir().map(|d| d.join("mods-off.cfg"))
}

/// The ids of the mods the user switched off.
pub fn read_disabled() -> BTreeSet<String> {
    disabled_file()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| t.lines().map(|l| l.trim().to_ascii_lowercase()).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default()
}

pub fn write_disabled(off: &BTreeSet<String>) -> std::io::Result<()> {
    let Some(path) = disabled_file() else { return Ok(()) };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, off.iter().map(|id| format!("{id}\n")).collect::<String>())
}

/// What is in the mods folder, in the order it is applied (alphabetical). Nothing is loaded yet.
pub fn discover(dir: &Path, disabled: &BTreeSet<String>) -> Vec<Mod> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut mods: Vec<Mod> = rd
        .filter_map(Result::ok)
        .filter_map(|e| {
            let file = e.file_name().to_string_lossy().into_owned();
            let lower = file.to_ascii_lowercase();
            if lower.starts_with('.') {
                return None;
            }
            let ty = e.file_type().ok()?;
            let (id, kind) = if ty.is_dir() {
                (lower, ModKind::Folder)
            } else if let Some(stem) = lower.strip_suffix(".pak") {
                (stem.to_owned(), ModKind::Archive)
            } else {
                return None;
            };
            let name = if kind == ModKind::Archive { file[..file.len() - 4].to_owned() } else { file };
            let enabled = !disabled.contains(&id);
            Some(Mod { id, name, author: String::new(), version: String::new(), description: String::new(), path: e.path(), kind, enabled, files: 0, replaces: 0 })
        })
        .collect();
    mods.sort_by(|a, b| a.id.cmp(&b.id));
    mods
}

impl Mod {
    fn describe(&mut self, ini: &str) {
        for (key, value) in parse_ini(ini) {
            match key.as_str() {
                "name" if !value.is_empty() => self.name = value,
                "author" => self.author = value,
                "version" => self.version = value,
                "description" => self.description = value,
                _ => {}
            }
        }
    }

    /// One line about it: version and author, where it gives them.
    pub fn byline(&self) -> String {
        match (self.version.is_empty(), self.author.is_empty()) {
            (false, false) => format!("{} - {}", self.version, self.author),
            (false, true) => self.version.clone(),
            (true, false) => self.author.clone(),
            (true, true) => String::new(),
        }
    }
}

impl PakSet {
    /// Find the mods in `dir` and put the files of those that are switched on over the game's. Every mod found is
    /// returned, described and counted, whether it is on or not.
    pub fn apply_mods(&mut self, dir: &Path, disabled: &BTreeSet<String>) -> Vec<Mod> {
        let mut mods = discover(dir, disabled);
        for m in &mut mods {
            // Look inside it on its own first: what it holds, and what it says about itself.
            let mut own = PakSet::new();
            let opened: Result<(), PakError> = match m.kind {
                ModKind::Folder => {
                    own.add_loose_dir(&m.path, "");
                    Ok(())
                }
                ModKind::Archive => own.add_archive(&m.path),
            };
            if let Err(e) = opened {
                m.description = format!("cannot be read: {e}");
                m.enabled = false;
                continue;
            }
            if let Ok(ini) = own.read("mod.ini") {
                m.describe(&String::from_utf8_lossy(&ini));
            }
            let files: Vec<String> = own.iter().map(|(path, _)| path.to_owned()).filter(|p| p != "mod.ini").collect();
            m.files = files.len();
            m.replaces = files.iter().filter(|p| self.contains(p)).count();
            if !m.enabled {
                continue;
            }
            match m.kind {
                ModKind::Folder => self.add_loose_dir(&m.path, ""),
                ModKind::Archive => {
                    let _ = self.add_archive(&m.path);
                }
            }
            self.forget("mod.ini");
            self.mod_files.extend(files.into_iter().map(|f| (normalize(&f), m.id.clone())));
        }
        mods
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pak::write_archive;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("arx-mods-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn put(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn a_mod_says_who_it_is() {
        let ini = parse_ini("; a comment\n[mod]\nName = Brighter torches\nauthor=Somebody\n VERSION = \"1.2\" \nnonsense\n");
        assert_eq!(ini, [("name".to_owned(), "Brighter torches".to_owned()), ("author".to_owned(), "Somebody".to_owned()), ("version".to_owned(), "1.2".to_owned())]);
    }

    #[test]
    fn folders_and_archives_dropped_in_replace_and_add_files_in_order() {
        let root = scratch("order");
        // "The game": two files in an archive.
        let game = root.join("data.pak");
        std::fs::write(&game, write_archive(&[("graph/a.txt".into(), b"game a".to_vec()), ("graph/b.txt".into(), b"game b".to_vec())])).unwrap();
        let mods = root.join("mods");
        // A folder mod that replaces one file and adds another, and says who it is.
        put(&mods.join("Alpha/graph/a.txt"), b"alpha a");
        put(&mods.join("Alpha/Graph/New/C.TXT"), b"alpha c");
        put(&mods.join("Alpha/mod.ini"), b"name = The Alpha mod\nversion = 2\nauthor = me\ndescription = Changes a.\n");
        // An archive mod, later in the alphabet, that replaces the same file and one more.
        std::fs::write(mods.join("beta.pak"), write_archive(&[("graph/a.txt".into(), b"beta a".to_vec()), ("graph/b.txt".into(), b"beta b".to_vec()), ("mod.ini".into(), b"name=Beta\n".to_vec())])).unwrap();
        // Things that are not mods.
        put(&mods.join("readme.txt"), b"hello");
        put(&mods.join(".hidden/graph/a.txt"), b"no");

        let open = |off: &[&str]| {
            let mut set = PakSet::new();
            set.add_archive(&game).unwrap();
            let found = set.apply_mods(&mods, &off.iter().map(|s| (*s).to_owned()).collect());
            (set, found)
        };
        let text = |set: &PakSet, path: &str| String::from_utf8(set.read(path).unwrap().into_owned()).unwrap();

        let (set, found) = open(&[]);
        assert_eq!(found.iter().map(|m| (m.id.as_str(), m.name.as_str(), m.kind, m.files, m.replaces)).collect::<Vec<_>>(), [("alpha", "The Alpha mod", ModKind::Folder, 2, 1), ("beta", "Beta", ModKind::Archive, 2, 2)]);
        assert_eq!(found[0].byline(), "2 - me");
        assert_eq!((text(&set, "graph/a.txt"), text(&set, "graph/b.txt"), text(&set, "graph/new/c.txt")), ("beta a".to_owned(), "beta b".to_owned(), "alpha c".to_owned()));
        assert!(!set.contains("mod.ini"), "a mod's own description is not a game file");
        assert_eq!((set.mod_of("graph/a.txt"), set.mod_of("graph/new/c.txt"), set.mod_of("graph/zzz")), (Some("beta"), Some("alpha"), None));

        // Switched off, a mod is still listed but changes nothing.
        let (set, found) = open(&["beta"]);
        assert_eq!(found.iter().map(|m| m.enabled).collect::<Vec<_>>(), [true, false]);
        assert_eq!((text(&set, "graph/a.txt"), text(&set, "graph/b.txt")), ("alpha a".to_owned(), "game b".to_owned()));
        let (set, _) = open(&["alpha", "beta"]);
        assert_eq!(text(&set, "graph/a.txt"), "game a");
        assert!(!set.contains("graph/new/c.txt"));

        // No mods folder at all is not an error.
        let mut set = PakSet::new();
        assert!(set.apply_mods(&root.join("nothing-here"), &BTreeSet::new()).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
