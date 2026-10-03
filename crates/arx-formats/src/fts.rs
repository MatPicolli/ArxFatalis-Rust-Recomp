//! `fast.fts` — a level's static geometry, collision anchors, rooms and portals.
//!
//! File: `UNIQUE_HEADER` (280 bytes), `count` x 768 bytes of source scene names (ignored), then the
//! scene data, DCL-compressed when `uncompressedsize != 0`. Scene data: header, texture table, a
//! `sizex * sizez` grid of tiles (each with polygons), anchors, portals, rooms, room distances.

use crate::blast::{self, BlastError};
use crate::poly;
use crate::reader::{Reader, Truncated, Vec3};
use std::collections::HashMap;
use thiserror::Error;

pub const FTS_VERSION: f32 = 0.141;

#[derive(Debug, Error)]
pub enum FtsError {
    #[error("file is truncated")]
    Truncated(#[from] Truncated),
    #[error("decompression failed: {0}")]
    Blast(#[from] BlastError),
    #[error("unexpected version {0}")]
    BadVersion(f32),
    #[error("invalid {0}")]
    Invalid(&'static str),
}

/// Position is in Arx coordinates (+Y down).
#[derive(Debug, Clone, Copy)]
pub struct PolyVertex {
    pub pos: Vec3,
    pub uv: [f32; 2],
}

#[derive(Debug, Clone)]
pub struct Poly {
    /// Tile column/row this polygon is stored in.
    pub tile: (u16, u16),
    /// 3 or 4 meaningful vertices, see [`Poly::vertex_count`].
    pub verts: [PolyVertex; 4],
    /// Key into [`Fts::textures`]; 0 = untextured.
    pub tex: i32,
    pub norm: Vec3,
    pub norm2: Vec3,
    pub vertex_normals: [Vec3; 4],
    pub transval: f32,
    pub area: f32,
    /// `poly::*` flags.
    pub flags: u32,
    /// Room index, or negative for none.
    pub room: i16,
}

impl Poly {
    pub fn is_quad(&self) -> bool {
        self.flags & poly::QUAD != 0
    }
    pub fn vertex_count(&self) -> usize {
        if self.is_quad() { 4 } else { 3 }
    }
}

#[derive(Debug, Clone)]
pub struct Anchor {
    pub pos: Vec3,
    pub radius: f32,
    pub height: f32,
    pub blocked: bool,
    pub links: Vec<i32>,
}

#[derive(Debug, Clone)]
pub struct Portal {
    pub corners: [Vec3; 4],
    pub room_a: i32,
    pub room_b: i32,
    pub use_portal: i16,
}

#[derive(Debug, Clone, Default)]
pub struct Room {
    pub portals: Vec<i32>,
    /// (tile x, tile y, polygon index within the tile)
    pub polys: Vec<(i16, i16, i16)>,
}

#[derive(Debug, Clone)]
pub struct Fts {
    pub size: (i32, i32),
    pub player_pos: Vec3,
    pub scene_pos: Vec3,
    /// Texture id used by [`Poly::tex`] -> texture path as stored in the file.
    pub textures: HashMap<i32, String>,
    pub polys: Vec<Poly>,
    pub anchors: Vec<Anchor>,
    pub portals: Vec<Portal>,
    /// `nb_rooms + 1` entries (index 0 is the "no room" bucket).
    pub rooms: Vec<Room>,
    /// Bytes left unread after parsing everything (expected: 0).
    pub trailing: usize,
}

fn count(r: &mut Reader, what: &'static str) -> Result<usize, FtsError> {
    usize::try_from(r.i32()?).map_err(|_| FtsError::Invalid(what))
}

impl Fts {
    pub fn parse(file: &[u8]) -> Result<Self, FtsError> {
        let mut r = Reader::new(file);
        // UNIQUE_HEADER
        r.skip(256)?;
        let n_sources = count(&mut r, "source count")?;
        let version = r.f32()?;
        if version != FTS_VERSION {
            return Err(FtsError::BadVersion(version));
        }
        let uncompressed = count(&mut r, "uncompressed size")?;
        r.skip(12)?;
        r.skip(n_sources.checked_mul(768).ok_or(FtsError::Invalid("source count"))?)?;
        let rest = r.take(r.remaining())?;

        let decompressed;
        let data: &[u8] = if uncompressed != 0 {
            decompressed = blast::blast(rest, uncompressed)?;
            &decompressed
        } else {
            rest
        };
        Self::parse_scene(data)
    }

    fn parse_scene(data: &[u8]) -> Result<Self, FtsError> {
        let mut r = Reader::new(data);
        let version = r.f32()?;
        if version != FTS_VERSION {
            return Err(FtsError::BadVersion(version));
        }
        let size_x = r.i32()?;
        let size_z = r.i32()?;
        let nb_textures = count(&mut r, "texture count")?;
        let _nb_polys = r.i32()?;
        let nb_anchors = count(&mut r, "anchor count")?;
        let player_pos = r.vec3()?;
        let scene_pos = r.vec3()?;
        let nb_portals = count(&mut r, "portal count")?;
        let nb_rooms = count(&mut r, "room count")?;
        if !(0..=4096).contains(&size_x) || !(0..=4096).contains(&size_z) {
            return Err(FtsError::Invalid("scene size"));
        }

        let mut textures = HashMap::new();
        for _ in 0..nb_textures {
            let id = r.i32()?;
            r.skip(4)?;
            textures.insert(id, r.string(256)?);
        }

        let mut polys = Vec::new();
        for ty in 0..size_z as u16 {
            for tx in 0..size_x as u16 {
                let nb_poly = count(&mut r, "tile polygon count")?;
                let nb_tile_anchors = count(&mut r, "tile anchor count")?;
                for _ in 0..nb_poly {
                    let mut verts = [PolyVertex { pos: [0.0; 3], uv: [0.0; 2] }; 4];
                    for v in &mut verts {
                        let y = r.f32()?;
                        let x = r.f32()?;
                        let z = r.f32()?;
                        v.pos = [x, y, z];
                        v.uv = [r.f32()?, r.f32()?];
                    }
                    let tex = r.i32()?;
                    let norm = r.vec3()?;
                    let norm2 = r.vec3()?;
                    let vertex_normals = [r.vec3()?, r.vec3()?, r.vec3()?, r.vec3()?];
                    let transval = r.f32()?;
                    let area = r.f32()?;
                    let flags = r.i32()? as u32;
                    let room = r.i16()?;
                    r.skip(2)?;
                    if room as i32 >= nb_rooms as i32 + 1 {
                        return Err(FtsError::Invalid("polygon room index"));
                    }
                    polys.push(Poly {
                        tile: (tx, ty),
                        verts,
                        tex,
                        norm,
                        norm2,
                        vertex_normals,
                        transval,
                        area,
                        flags,
                        room,
                    });
                }
                r.skip(nb_tile_anchors * 4)?;
            }
        }

        let mut anchors = Vec::with_capacity(nb_anchors);
        for _ in 0..nb_anchors {
            let pos = r.vec3()?;
            let radius = r.f32()?;
            let height = r.f32()?;
            let nb_linked = r.i16()?;
            let flags = r.i16()?;
            let mut links = Vec::new();
            for _ in 0..nb_linked.max(0) {
                links.push(r.i32()?);
            }
            anchors.push(Anchor { pos, radius, height, blocked: flags & (1 << 3) != 0, links });
        }

        let mut portals = Vec::with_capacity(nb_portals);
        for _ in 0..nb_portals {
            // SAVE_EERIEPOLY (384 bytes): type, min, max, norm, norm2, then 4 vertices of 32 bytes.
            let ty = r.i32()?;
            r.skip(12 * 4)?;
            let mut corners = [[0.0; 3]; 4];
            for c in &mut corners {
                *c = r.vec3()?;
                r.skip(20)?; // rhw, color, specular, tu, tv
            }
            r.skip(384 - (4 + 48 + 128))?;
            let room_a = r.i32()?;
            let room_b = r.i32()?;
            let use_portal = r.i16()?;
            r.skip(2)?;
            if ty != 64 && ty != 0 {
                return Err(FtsError::Invalid("portal polygon type"));
            }
            let in_range = |v: i32| v >= 0 && (v as usize) <= nb_rooms;
            if !in_range(room_a) || !in_range(room_b) {
                return Err(FtsError::Invalid("portal room index"));
            }
            portals.push(Portal { corners, room_a, room_b, use_portal });
        }

        let mut rooms = Vec::with_capacity(nb_rooms + 1);
        for _ in 0..=nb_rooms {
            let n_portals = count(&mut r, "room portal count")?;
            let n_polys = count(&mut r, "room polygon count")?;
            r.skip(24)?;
            let mut room = Room::default();
            for _ in 0..n_portals {
                room.portals.push(r.i32()?);
            }
            for _ in 0..n_polys {
                let (px, py, idx) = (r.i16()?, r.i16()?, r.i16()?);
                r.skip(2)?;
                room.polys.push((px, py, idx));
            }
            rooms.push(room);
        }

        // Room distance matrix: (rooms)^2 * { f32 distance, vec3 start, vec3 end }
        let n = rooms.len();
        r.skip(n.checked_mul(n).and_then(|v| v.checked_mul(28)).ok_or(FtsError::Invalid("room count"))?)?;

        Ok(Fts {
            size: (size_x, size_z),
            player_pos,
            scene_pos,
            textures,
            polys,
            anchors,
            portals,
            rooms,
            trailing: r.remaining(),
        })
    }
}
