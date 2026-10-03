//! `.ftl` — pre-computed 3D object files (items, creatures, props).
//!
//! Layout: 8-byte primary header (`"FTL\0"`, f32 version), 512 bytes of checksum, then a secondary
//! header of six `i32` offsets, then the data blocks. Files in the archives are DCL-compressed
//! as a whole; compressed files do not start with `"FTL"`.

use crate::blast::{self, BlastError};
use crate::reader::{Reader, Truncated};
use thiserror::Error;

pub const FTL_VERSION: f32 = 0.83257;

#[derive(Debug, Error)]
pub enum FtlError {
    #[error("file is truncated")]
    Truncated(#[from] Truncated),
    #[error("decompression failed: {0}")]
    Blast(#[from] BlastError),
    #[error("bad signature")]
    BadSignature,
    #[error("unexpected version {0}")]
    BadVersion(f32),
    #[error("invalid {0}")]
    Invalid(&'static str),
}

/// Positions are in Arx coordinates (+Y points down).
#[derive(Debug, Clone, Copy)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub norm: [f32; 3],
}

#[derive(Debug, Clone)]
pub struct Face {
    /// Bit flags: see [`Face::DOUBLE_SIDED`].
    pub facetype: i32,
    /// Index into [`Ftl::textures`], if the face is textured.
    pub material: Option<u16>,
    pub vid: [u16; 3],
    pub u: [f32; 3],
    pub v: [f32; 3],
    pub transval: f32,
    pub norm: [f32; 3],
}

impl Face {
    pub const DOUBLE_SIDED: i32 = crate::poly::DOUBLESIDED as i32;
}

#[derive(Debug, Clone)]
pub struct Group {
    pub name: String,
    pub origin: u32,
    pub indices: Vec<u32>,
    pub blob_shadow_size: f32,
}

#[derive(Debug, Clone)]
pub struct Action {
    pub name: String,
    pub vertex: u32,
}

#[derive(Debug, Clone)]
pub struct Selection {
    pub name: String,
    pub vertices: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct Ftl {
    pub name: String,
    pub origin: u32,
    pub vertices: Vec<Vertex>,
    pub faces: Vec<Face>,
    /// Texture paths as stored in the file (may be empty strings).
    pub textures: Vec<String>,
    pub groups: Vec<Group>,
    pub actions: Vec<Action>,
    pub selections: Vec<Selection>,
}

fn count(r: &mut Reader, what: &'static str) -> Result<usize, FtlError> {
    usize::try_from(r.i32()?).map_err(|_| FtlError::Invalid(what))
}

impl Ftl {
    pub fn parse(file: &[u8]) -> Result<Self, FtlError> {
        let decompressed;
        let data: &[u8] = if file.starts_with(b"FTL") {
            file
        } else {
            decompressed = blast::blast(file, file.len() * 4)?;
            &decompressed
        };

        let mut r = Reader::new(data);
        if r.take(3)? != b"FTL" {
            return Err(FtlError::BadSignature);
        }
        r.skip(1)?;
        let version = r.f32()?;
        if version != FTL_VERSION {
            return Err(FtlError::BadVersion(version));
        }
        r.skip(512)?;
        // Secondary header: offset of the 3D data block comes first.
        let offset_3d = usize::try_from(r.i32()?).map_err(|_| FtlError::Invalid("3D data offset"))?;
        r.seek(offset_3d);

        let nb_vertex = count(&mut r, "vertex count")?;
        let nb_faces = count(&mut r, "face count")?;
        let nb_maps = count(&mut r, "material count")?;
        let nb_groups = count(&mut r, "group count")?;
        let nb_actions = count(&mut r, "action count")?;
        let nb_selections = count(&mut r, "selection count")?;
        let origin = count(&mut r, "origin")? as u32;
        let name = r.string(256)?;
        if origin as usize >= nb_vertex {
            return Err(FtlError::Invalid("origin vertex"));
        }

        let mut vertices = Vec::with_capacity(nb_vertex);
        for _ in 0..nb_vertex {
            r.skip(32)?; // legacy fields
            vertices.push(Vertex { pos: r.vec3()?, norm: r.vec3()? });
        }

        let mut faces = Vec::with_capacity(nb_faces);
        for _ in 0..nb_faces {
            let facetype = r.i32()?;
            r.skip(12)?; // rgb[3]
            let vid = [r.u16()?, r.u16()?, r.u16()?];
            let texid = r.i16()?;
            let u = r.vec3()?;
            let v = r.vec3()?;
            r.skip(12)?; // ou[3], ov[3] (s16)
            let transval = r.f32()?;
            let norm = r.vec3()?;
            r.skip(36 + 4)?; // nrmls[3], temp
            if vid.iter().any(|&i| i as usize >= nb_vertex) {
                return Err(FtlError::Invalid("face vertex"));
            }
            let material = match texid {
                -1 => None,
                t if t >= 0 && (t as usize) < nb_maps => Some(t as u16),
                _ => return Err(FtlError::Invalid("face material")),
            };
            faces.push(Face { facetype, material, vid, u, v, transval, norm });
        }

        let textures = (0..nb_maps).map(|_| r.string(256)).collect::<Result<Vec<_>, _>>()?;

        let mut groups = Vec::with_capacity(nb_groups);
        let mut group_sizes = Vec::with_capacity(nb_groups);
        for _ in 0..nb_groups {
            let name = r.string(256)?.to_ascii_lowercase();
            let origin = r.i32()?;
            let nb_index = count(&mut r, "group size")?;
            r.skip(4)?; // indexes (pointer)
            let siz = r.f32()?;
            if origin < 0 || origin as usize >= nb_vertex || nb_index > nb_vertex {
                return Err(FtlError::Invalid("group"));
            }
            group_sizes.push(nb_index);
            groups.push(Group { name, origin: origin as u32, indices: Vec::new(), blob_shadow_size: siz });
        }
        for (g, n) in groups.iter_mut().zip(group_sizes) {
            for _ in 0..n {
                let v = r.i32()?;
                if v < 0 || v as usize >= nb_vertex {
                    return Err(FtlError::Invalid("group vertex"));
                }
                g.indices.push(v as u32);
            }
        }

        let mut actions = Vec::with_capacity(nb_actions);
        for _ in 0..nb_actions {
            let name = r.string(256)?.to_ascii_lowercase();
            let idx = r.i32()?;
            r.skip(8)?; // action, sfx
            if idx < 0 || idx as usize >= nb_vertex {
                return Err(FtlError::Invalid("action vertex"));
            }
            actions.push(Action { name, vertex: idx as u32 });
        }

        let mut selections = Vec::with_capacity(nb_selections);
        let mut sel_sizes = Vec::with_capacity(nb_selections);
        for _ in 0..nb_selections {
            let name = r.string(64)?.to_ascii_lowercase();
            let n = count(&mut r, "selection size")?;
            r.skip(4)?; // selected (pointer)
            if n == 0 || n > nb_vertex {
                return Err(FtlError::Invalid("selection size"));
            }
            sel_sizes.push(n);
            selections.push(Selection { name, vertices: Vec::new() });
        }
        for (s, n) in selections.iter_mut().zip(sel_sizes) {
            for _ in 0..n {
                let v = r.i32()?;
                if v < 0 || v as usize >= nb_vertex {
                    return Err(FtlError::Invalid("selection vertex"));
                }
                s.vertices.push(v as u32);
            }
        }

        Ok(Ftl { name, origin, vertices, faces, textures, groups, actions, selections })
    }
}
