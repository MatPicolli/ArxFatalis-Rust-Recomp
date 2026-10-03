//! Reader for Arx Fatalis `.pak` archives and the virtual file system layered on top of them.
//!
//! Archive layout: `u32` offset of the FAT at byte 0. At that offset: `u32` FAT size followed by the
//! FAT, XOR-obfuscated with a repeating key. The FAT is a list of directories:
//! `cstr dirname, u32 nfiles, nfiles * { cstr filename, u32 offset, u32 flags, u32 size, u32 stored }`.
//! Files with flag bit 0 set (and a non-zero stored size) are DCL-compressed.

use crate::blast::{self, BlastError};
use memmap2::Mmap;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PakError {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{0}: malformed FAT (wrong key?)")]
    BadFat(PathBuf),
    #[error("{0}: file data lies outside the archive")]
    OutOfBounds(String),
    #[error("{path}: decompression failed: {source}")]
    Decompress { path: String, source: BlastError },
    #[error("{path}: decompressed to {got} bytes, expected {expected}")]
    SizeMismatch { path: String, got: usize, expected: usize },
    #[error("no such file: {0}")]
    NotFound(String),
}

const KEY_FULL: &[u8] = b"AVQF3FCKE50GRIAYXJP2AMEYO5QGA0JGIIH2NHBTVOA1VOGGU5H3GSSIARKPRQPQKKYEOIAQG1XRX0J4F5OEAEFI4DD3LL45VJTVOA1VOGGUKE50GRIAYX";
const KEY_DEMO: &[u8] = b"NSIARKPRQPHBTE50GRIH3AYXJP2AMF3FCEYAVQO5QGA0JGIIH2AYXKVOA1VOGGU5GSQKKYEOIAQG1XRX0J4F5OEAEFI4DD3LL45VJTVOA1VOGGUKE50GRI";

const MAGIC_FULL: u32 = 0x4651_5641; // "AVQF"
const MAGIC_DEMO: u32 = 0x4149_534E; // "NSIA"
const FLAG_COMPRESSED: u32 = 1;

#[derive(Debug, Clone)]
enum Source {
    Archive { index: usize, offset: usize, stored: usize, compressed: bool },
    Loose(PathBuf),
}

/// A file in the virtual file system.
#[derive(Debug, Clone)]
pub struct PakEntry {
    source: Source,
    /// Uncompressed size in bytes (for loose files: the on-disk size).
    pub size: usize,
}

impl PakEntry {
    pub fn is_compressed(&self) -> bool {
        matches!(self.source, Source::Archive { compressed: true, .. })
    }
    /// Name of the archive file (or `"<loose>"`) this entry comes from.
    pub fn origin<'a>(&self, set: &'a PakSet) -> &'a str {
        match &self.source {
            Source::Archive { index, .. } => &set.archive_names[*index],
            Source::Loose(_) => "<loose>",
        }
    }
}

/// Normalise a resource path: lowercase, `/` separators, no leading, trailing or repeated separators.
pub fn normalize(path: &str) -> String {
    path.replace('\\', "/").trim_matches('/').to_ascii_lowercase()
}

#[derive(Default)]
pub struct PakSet {
    archives: Vec<Mmap>,
    archive_names: Vec<String>,
    files: HashMap<String, PakEntry>,
}

struct Cursor<'a> {
    data: &'a [u8],
}

impl<'a> Cursor<'a> {
    fn u32(&mut self) -> Option<u32> {
        let (head, rest) = self.data.split_first_chunk::<4>()?;
        self.data = rest;
        Some(u32::from_le_bytes(*head))
    }
    fn cstr(&mut self) -> Option<&'a [u8]> {
        let end = self.data.iter().position(|&b| b == 0)?;
        let s = &self.data[..end];
        self.data = &self.data[end + 1..];
        Some(s)
    }
}

/// Decode a legacy (Windows-1252-ish) byte string; file names are ASCII in practice.
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

impl PakSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Open the game's resources the way the original engine does: `data`, `loc`, `data2`, `sfx`,
    /// `speech` archives in that order (each as `name_default.pak` then `name.pak`), then loose
    /// directories on top. Later sources override earlier ones.
    pub fn open_game_dir(dir: &Path) -> Result<Self, PakError> {
        let mut set = Self::new();
        let io = |source| PakError::Io { path: dir.to_owned(), source };
        let names: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
            .map_err(io)?
            .filter_map(Result::ok)
            .map(|e| (e.file_name().to_string_lossy().to_ascii_lowercase(), e.path()))
            .collect();

        for group in ["data", "loc", "data2", "sfx", "speech"] {
            let mut found: Vec<&(String, PathBuf)> = names
                .iter()
                .filter(|(n, _)| {
                    n.strip_suffix(".pak").is_some_and(|stem| {
                        stem == group || stem.strip_prefix(group).is_some_and(|r| r.starts_with('_'))
                    })
                })
                .collect();
            // name_default.pak first, plain name.pak last
            found.sort_by_key(|(n, _)| (n == &format!("{group}.pak"), n.clone()));
            for (_, path) in found {
                set.add_archive(path)?;
            }
        }
        for sub in ["editor", "game", "graph", "localisation", "misc", "sfx", "speech"] {
            if let Some((_, path)) = names.iter().find(|(n, p)| n == sub && p.is_dir()) {
                set.add_loose_dir(path, sub);
            }
        }
        Ok(set)
    }

    pub fn add_archive(&mut self, path: &Path) -> Result<(), PakError> {
        let io = |source| PakError::Io { path: path.to_owned(), source };
        let file = File::open(path).map_err(io)?;
        // SAFETY: the archive is treated as read-only game data; we accept the usual mmap caveat
        // that external modification while mapped is undefined.
        let map = unsafe { Mmap::map(&file) }.map_err(io)?;
        let bad = || PakError::BadFat(path.to_owned());

        let fat_offset = Cursor { data: &map }.u32().ok_or_else(bad)? as usize;
        let mut c = Cursor { data: map.get(fat_offset..).ok_or_else(bad)? };
        let fat_size = c.u32().ok_or_else(bad)? as usize;
        let mut fat = c.data.get(..fat_size).ok_or_else(bad)?.to_vec();

        let magic = fat.first_chunk::<4>().map(|b| u32::from_le_bytes(*b));
        let key = match magic {
            Some(MAGIC_FULL) => Some(KEY_FULL),
            Some(MAGIC_DEMO) => Some(KEY_DEMO),
            _ => None,
        };
        if let Some(key) = key {
            for (b, k) in fat.iter_mut().zip(key.iter().cycle()) {
                *b ^= k;
            }
        }

        let index = self.archives.len();
        let mut c = Cursor { data: &fat };
        let mut entries = Vec::new();
        while !c.data.is_empty() {
            let dir = normalize(&latin1(c.cstr().ok_or_else(bad)?));
            let nfiles = c.u32().ok_or_else(bad)?;
            for _ in 0..nfiles {
                let name = latin1(c.cstr().ok_or_else(bad)?).to_ascii_lowercase();
                let offset = c.u32().ok_or_else(bad)? as usize;
                let flags = c.u32().ok_or_else(bad)?;
                let uncompressed = c.u32().ok_or_else(bad)? as usize;
                let stored = c.u32().ok_or_else(bad)? as usize;
                let compressed = flags & FLAG_COMPRESSED != 0 && stored != 0;
                // Uncompressed files are stored verbatim: their length is the 4th field and the
                // 3rd field is meaningless.
                let size = if compressed { uncompressed } else { stored };
                let full = if dir.is_empty() { name } else { format!("{dir}/{name}") };
                entries.push((full, PakEntry { source: Source::Archive { index, offset, stored, compressed }, size }));
            }
        }
        self.archives.push(map);
        self.archive_names
            .push(path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        self.files.extend(entries);
        Ok(())
    }

    /// Mount a directory of loose files below the virtual path `mount`.
    pub fn add_loose_dir(&mut self, dir: &Path, mount: &str) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.filter_map(Result::ok) {
            let name = e.file_name().to_string_lossy().to_ascii_lowercase();
            if name.starts_with('.') {
                continue;
            }
            let vpath = format!("{mount}/{name}");
            let Ok(ty) = e.file_type() else { continue };
            if ty.is_dir() {
                self.add_loose_dir(&e.path(), &vpath);
            } else if ty.is_file() {
                let size = e.metadata().map(|m| m.len() as usize).unwrap_or(0);
                self.files.insert(vpath, PakEntry { source: Source::Loose(e.path()), size });
            }
        }
    }

    /// Find a texture by name the way the engine does: strip any extension, then try
    /// `.png`, `.jpg`, `.jpeg`, `.bmp`, `.tga` in that order. Returns the resolved virtual path.
    pub fn find_texture(&self, name: &str) -> Option<String> {
        let base = normalize(name);
        let stem = match base.rsplit_once('.') {
            Some((s, ext)) if !ext.contains('/') => s,
            _ => base.as_str(),
        };
        ["png", "jpg", "jpeg", "bmp", "tga"]
            .iter()
            .map(|ext| format!("{stem}.{ext}"))
            .find(|p| self.files.contains_key(p))
    }

    /// Load a level's baked lighting (`levelN.llf`), handling its version-dependent compression.
    pub fn load_llf(&self, level: u32) -> Option<crate::llf::Llf> {
        let base = format!("graph/levels/level{level}/level{level}");
        let dlf = self.read(&format!("{base}.dlf")).ok()?;
        let version = f32::from_le_bytes(dlf.get(..4)?.try_into().ok()?);
        let llf = self.read(&format!("{base}.llf")).ok()?;
        crate::llf::Llf::parse(&llf, version >= crate::llf::DLF_COMPRESSED_VERSION).ok()
    }

    /// Load a level's scene definition (`levelN.dlf`).
    pub fn load_dlf(&self, level: u32) -> Result<crate::dlf::Dlf, String> {
        let path = format!("graph/levels/level{level}/level{level}.dlf");
        let bytes = self.read(&path).map_err(|e| e.to_string())?;
        crate::dlf::Dlf::parse(&bytes).map_err(|e| format!("{path}: {e}"))
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
    pub fn contains(&self, path: &str) -> bool {
        self.files.contains_key(&normalize(path))
    }
    pub fn entry(&self, path: &str) -> Option<&PakEntry> {
        self.files.get(&normalize(path))
    }
    /// All files, in no particular order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &PakEntry)> {
        self.files.iter().map(|(k, v)| (k.as_str(), v))
    }
    /// All file paths below `prefix`, sorted.
    pub fn list(&self, prefix: &str) -> Vec<&str> {
        let p = normalize(prefix);
        let mut v: Vec<&str> = self
            .files
            .keys()
            .map(String::as_str)
            .filter(|k| p.is_empty() || k.strip_prefix(p.as_str()).is_some_and(|r| r.starts_with('/')))
            .collect();
        v.sort_unstable();
        v
    }

    /// Read a file. Uncompressed archive files are returned without copying.
    pub fn read(&self, path: &str) -> Result<Cow<'_, [u8]>, PakError> {
        let key = normalize(path);
        let entry = self.files.get(&key).ok_or_else(|| PakError::NotFound(path.to_owned()))?;
        self.read_entry(&key, entry)
    }

    pub fn read_entry<'a>(&'a self, name: &str, entry: &'a PakEntry) -> Result<Cow<'a, [u8]>, PakError> {
        match &entry.source {
            Source::Loose(p) => std::fs::read(p)
                .map(Cow::Owned)
                .map_err(|source| PakError::Io { path: p.clone(), source }),
            Source::Archive { index, offset, stored, compressed } => {
                let raw = self.archives[*index]
                    .get(*offset..offset.saturating_add(*stored))
                    .ok_or_else(|| PakError::OutOfBounds(name.to_owned()))?;
                if !compressed {
                    return Ok(Cow::Borrowed(raw));
                }
                let out = blast::blast(raw, entry.size)
                    .map_err(|source| PakError::Decompress { path: name.to_owned(), source })?;
                if out.len() != entry.size {
                    return Err(PakError::SizeMismatch { path: name.to_owned(), got: out.len(), expected: entry.size });
                }
                Ok(Cow::Owned(out))
            }
        }
    }
}
