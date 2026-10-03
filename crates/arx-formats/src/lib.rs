//! File formats of Arx Fatalis (2002, Arkane Studios).
//!
//! Format knowledge is derived from the GPLv3 ArxLibertatis project, so this crate is GPLv3 too.

pub mod blast;
pub mod dlf;
pub mod fts;
pub mod ftl;
pub mod llf;
pub mod pak;
pub mod poly;
pub mod reader;
pub mod skeleton;
pub mod tea;
pub mod wav;

pub use pak::{PakEntry, PakError, PakSet};
