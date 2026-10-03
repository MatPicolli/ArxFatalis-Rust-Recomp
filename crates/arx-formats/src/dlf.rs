//! `.dlf` — a level's scene definition: where the scene geometry comes from, placed entities
//! (items, NPCs, fixtures), fogs, and paths/zones.
//!
//! Layout: 8520-byte header; the remainder is DCL-compressed when `version >= 1.44`.
//! Body: scene refs, entities, (legacy) baked lighting block, lights, fogs, nodes (skipped), paths.
//!
//! Positions are relative to the scene origin; add [`crate::fts::Fts::scene_pos`] to get world
//! coordinates. All positions/angles here are in Arx convention (+Y down).

use crate::blast::{self, BlastError};
use crate::llf::Light;
use crate::reader::{Reader, Truncated, Vec3};
use thiserror::Error;

pub const DLF_VERSION: f32 = 1.44;
const HEADER_SIZE: usize = 8520;

#[derive(Debug, Error)]
pub enum DlfError {
    #[error("file is truncated")]
    Truncated(#[from] Truncated),
    #[error("decompression failed: {0}")]
    Blast(#[from] BlastError),
    #[error("not a DANAE file")]
    BadIdent,
    #[error("unsupported version {0}")]
    BadVersion(f32),
    #[error("invalid {0}")]
    Invalid(&'static str),
}

#[derive(Debug, Clone)]
pub struct Entity {
    /// Class path as stored (lowercased, trimmed to start at `graph`, extension removed).
    pub class: String,
    pub pos: Vec3,
    /// (pitch, yaw, roll) in degrees.
    pub angle: Vec3,
    /// Instance number within the class.
    pub instance: i32,
    pub flags: i32,
}

#[derive(Debug, Clone)]
pub struct Fog {
    pub pos: Vec3,
    pub rgb: Vec3,
    pub size: f32,
    pub directional: bool,
    pub scale: f32,
    pub angle: Vec3,
    pub speed: f32,
    pub rotate_speed: f32,
    /// Lifetime in milliseconds.
    pub duration_ms: i32,
    pub blend: i32,
    pub frequency: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct Pathway {
    pub pos: Vec3,
    pub flag: i32,
    pub time_ms: u32,
}

/// A path (height == 0) or a zone (height != 0).
#[derive(Debug, Clone)]
pub struct Path {
    pub name: String,
    pub flags: i16,
    pub pos: Vec3,
    pub rgb: Vec3,
    pub farclip: f32,
    pub reverb: f32,
    pub amb_max_vol: f32,
    pub height: i32,
    pub ambiance: String,
    pub pathways: Vec<Pathway>,
}

impl Path {
    pub fn is_zone(&self) -> bool {
        self.height != 0
    }
}

#[derive(Debug, Clone)]
pub struct Dlf {
    pub version: f32,
    /// Editor camera / player start (level-local).
    pub player_pos: Vec3,
    pub player_angle: Vec3,
    /// Scene reference (`graph/levels/levelN` style path), if any.
    pub scene: Option<String>,
    pub entities: Vec<Entity>,
    /// Only meaningful for old files without an `.llf`.
    pub lights: Vec<Light>,
    pub fogs: Vec<Fog>,
    pub paths: Vec<Path>,
    pub trailing: usize,
}

fn count(r: &mut Reader, what: &'static str) -> Result<usize, DlfError> {
    usize::try_from(r.i32()?).map_err(|_| DlfError::Invalid(what))
}

/// `name` as stored is a Windows path to a .teo/.ftl; keep the part starting at "graph".
fn class_path(name: &str) -> String {
    let n = name.to_ascii_lowercase().replace('\\', "/");
    let n = n.find("graph").map_or(n.as_str(), |i| &n[i..]);
    match n.rsplit_once('.') {
        Some((stem, ext)) if !ext.contains('/') => stem.to_owned(),
        _ => n.to_owned(),
    }
}

impl Dlf {
    pub fn parse(file: &[u8]) -> Result<Self, DlfError> {
        let mut r = Reader::new(file);
        let version = r.f32()?;
        if version > DLF_VERSION {
            return Err(DlfError::BadVersion(version));
        }
        let ident = r.string(16)?;
        if ident != "DANAE_FILE" {
            return Err(DlfError::BadIdent);
        }
        r.skip(256 + 4)?; // lastuser, time
        let player_pos = r.vec3()?;
        let player_angle = r.vec3()?;
        let nb_scn = count(&mut r, "scene count")?;
        let nb_inter = count(&mut r, "entity count")?;
        let nb_nodes = count(&mut r, "node count")?;
        let nb_nodeslinks = count(&mut r, "node link count")?;
        let _nb_zones = r.i32()?;
        let lighting = r.i32()?;
        r.skip(256 * 4)?;
        let nb_lights = count(&mut r, "light count")?;
        let nb_fogs = count(&mut r, "fog count")?;
        r.skip(3 * 4)?;
        let nb_paths = count(&mut r, "path count")?;
        r.seek(HEADER_SIZE);

        let rest = r.take(r.remaining())?;
        let decompressed;
        let body: &[u8] = if version >= DLF_VERSION {
            decompressed = blast::blast(rest, rest.len() * 6)?;
            &decompressed
        } else {
            rest
        };
        let mut r = Reader::new(body);

        let mut scene = None;
        if nb_scn > 0 {
            scene = Some(r.string(512)?);
            r.skip(128)?;
            // Only the first scene reference is used by the engine.
        }

        let mut entities = Vec::with_capacity(nb_inter);
        for _ in 0..nb_inter {
            let name = r.string(512)?;
            let pos = r.vec3()?;
            let angle = r.vec3()?;
            let instance = r.i32()?;
            let flags = r.i32()?;
            r.skip(14 * 4 + 16 * 4)?;
            entities.push(Entity { class: class_path(&name), pos, angle, instance, flags });
        }

        if lighting != 0 {
            let nb_values = count(&mut r, "lighting size")?;
            r.skip(12)?;
            r.skip(nb_values * if version > 1.001 { 4 } else { 12 })?;
        }

        let nb_lights = if version < 1.003 { 0 } else { nb_lights };
        let mut lights = Vec::with_capacity(nb_lights);
        for _ in 0..nb_lights {
            lights.push(crate::llf::read_light(&mut r)?);
        }

        let mut fogs = Vec::with_capacity(nb_fogs);
        for _ in 0..nb_fogs {
            let pos = r.vec3()?;
            let rgb = r.vec3()?;
            let size = r.f32()?;
            let special = r.i32()?;
            let scale = r.f32()?;
            r.skip(12)?; // move
            let angle = r.vec3()?;
            let speed = r.f32()?;
            let rotate_speed = r.f32()?;
            let duration_ms = r.i32()?;
            let blend = r.i32()?;
            let frequency = r.f32()?;
            r.skip(32 * 4 + 32 * 4 + 256)?;
            fogs.push(Fog {
                pos,
                rgb,
                size,
                directional: special & 1 != 0,
                scale,
                angle,
                speed,
                rotate_speed,
                duration_ms,
                blend,
                frequency,
            });
        }

        if version >= 1.001 {
            let per_node = 204usize
                .checked_add(nb_nodeslinks.checked_mul(64).ok_or(DlfError::Invalid("node links"))?)
                .ok_or(DlfError::Invalid("node links"))?;
            r.skip(nb_nodes.checked_mul(per_node).ok_or(DlfError::Invalid("node count"))?)?;
        }

        let mut paths = Vec::with_capacity(nb_paths);
        for _ in 0..nb_paths {
            let name = r.string(64)?.to_ascii_lowercase();
            r.skip(2)?; // idx
            let flags = r.i16()?;
            r.skip(12)?; // initpos
            let pos = r.vec3()?;
            let nb_pathways = count(&mut r, "pathway count")?;
            let rgb = r.vec3()?;
            let farclip = r.f32()?;
            let reverb = r.f32()?;
            let amb_max_vol = r.f32()?;
            r.skip(26 * 4)?;
            let height = r.i32()?;
            r.skip(31 * 4)?;
            let ambiance = r.string(128)?;
            r.skip(128)?;
            let mut pathways = Vec::with_capacity(nb_pathways);
            for _ in 0..nb_pathways {
                let pos = r.vec3()?;
                let flag = r.i32()?;
                let time_ms = r.u32()?;
                r.skip(2 * 4 + 2 * 4 + 32)?;
                pathways.push(Pathway { pos, flag, time_ms });
            }
            paths.push(Path { name, flags, pos, rgb, farclip, reverb, amb_max_vol, height, ambiance, pathways });
        }

        Ok(Dlf {
            version,
            player_pos,
            player_angle,
            scene,
            entities,
            lights,
            fogs,
            paths,
            trailing: r.remaining(),
        })
    }
}
