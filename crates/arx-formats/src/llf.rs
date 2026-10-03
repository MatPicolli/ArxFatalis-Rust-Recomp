//! `.llf` — a level's static lights and baked per-vertex lighting.
//!
//! The file is DCL-compressed when the level's `.dlf` version is >= 1.44 (use [`Llf::parse`] with
//! `compressed` set accordingly). Layout: 7464-byte header, `nb_lights` x 296-byte lights, a
//! 16-byte lighting header, then one `u32` BGRA colour per polygon vertex (3 per triangle, 4 per
//! quad), in the same order as [`crate::fts::Fts::polys`].

use crate::blast::{self, BlastError};
use crate::reader::{Reader, Truncated, Vec3};
use thiserror::Error;

pub const DLF_COMPRESSED_VERSION: f32 = 1.44;

#[derive(Debug, Error)]
pub enum LlfError {
    #[error("file is truncated")]
    Truncated(#[from] Truncated),
    #[error("decompression failed: {0}")]
    Blast(#[from] BlastError),
    #[error("invalid {0}")]
    Invalid(&'static str),
}

#[derive(Debug, Clone)]
pub struct Light {
    /// Level-local position; add the FTS `scene_pos` to get world coordinates.
    pub pos: Vec3,
    pub rgb: Vec3,
    pub fall_start: f32,
    pub fall_end: f32,
    pub intensity: f32,
    pub ex_flicker: Vec3,
    pub ex_radius: f32,
    pub ex_frequency: f32,
    pub ex_size: f32,
    pub ex_speed: f32,
    pub ex_flare_size: f32,
    /// `ExtrasType` flags (fire, flicker, start-extinguished, ...).
    pub extras: u32,
}

#[derive(Debug, Clone)]
pub struct Llf {
    pub lights: Vec<Light>,
    /// Baked colours as `[r, g, b]` bytes, one per polygon vertex.
    pub colors: Vec<[u8; 3]>,
}

/// Read one 296-byte `DANAE_LS_LIGHT`.
pub fn read_light(r: &mut Reader) -> Result<Light, Truncated> {
    let pos = r.vec3()?;
    let rgb = r.vec3()?;
    let fall_start = r.f32()?;
    let fall_end = r.f32()?;
    let intensity = r.f32()?;
    r.skip(4)?; // i
    let ex_flicker = r.vec3()?;
    let ex_radius = r.f32()?;
    let ex_frequency = r.f32()?;
    let ex_size = r.f32()?;
    let ex_speed = r.f32()?;
    let ex_flare_size = r.f32()?;
    r.skip(24 * 4)?;
    let extras = r.u32()?;
    r.skip(31 * 4)?;
    Ok(Light {
        pos,
        rgb,
        fall_start,
        fall_end,
        intensity,
        ex_flicker,
        ex_radius,
        ex_frequency,
        ex_size,
        ex_speed,
        ex_flare_size,
        extras,
    })
}

impl Llf {
    pub fn parse(file: &[u8], compressed: bool) -> Result<Self, LlfError> {
        let decompressed;
        let data: &[u8] = if compressed {
            decompressed = blast::blast(file, file.len() * 4)?;
            &decompressed
        } else {
            file
        };
        let mut r = Reader::new(data);
        r.skip(4 + 16 + 256 + 4)?; // version, ident, lastuser, time
        let nb_lights = usize::try_from(r.i32()?).map_err(|_| LlfError::Invalid("light count"))?;
        r.skip(7464 - (4 + 16 + 256 + 4 + 4))?;

        let mut lights = Vec::with_capacity(nb_lights);
        for _ in 0..nb_lights {
            lights.push(read_light(&mut r)?);
        }

        let nb_values = usize::try_from(r.i32()?).map_err(|_| LlfError::Invalid("colour count"))?;
        r.skip(12)?;
        let mut colors = Vec::with_capacity(nb_values);
        for _ in 0..nb_values {
            let bgra = r.u32()?;
            colors.push([(bgra >> 16) as u8, (bgra >> 8) as u8, bgra as u8]);
        }
        Ok(Llf { lights, colors })
    }
}
