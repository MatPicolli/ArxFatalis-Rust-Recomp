//! Polygon type flags shared by `.ftl` faces and `.fts` scene polygons.

pub const NO_SHADOW: u32 = 1 << 0;
pub const DOUBLESIDED: u32 = 1 << 1;
pub const TRANS: u32 = 1 << 2;
pub const WATER: u32 = 1 << 3;
pub const GLOW: u32 = 1 << 4;
pub const IGNORE: u32 = 1 << 5;
pub const QUAD: u32 = 1 << 6;
pub const TILED: u32 = 1 << 7;
pub const METAL: u32 = 1 << 8;
pub const HIDE: u32 = 1 << 9;
pub const STONE: u32 = 1 << 10;
pub const WOOD: u32 = 1 << 11;
pub const GRAVEL: u32 = 1 << 12;
pub const EARTH: u32 = 1 << 13;
pub const NOCOL: u32 = 1 << 14;
pub const LAVA: u32 = 1 << 15;
pub const CLIMB: u32 = 1 << 16;
pub const FALL: u32 = 1 << 17;
pub const NOPATH: u32 = 1 << 18;
pub const NODRAW: u32 = 1 << 19;
pub const PRECISE_PATH: u32 = 1 << 20;
pub const NO_CLIMB: u32 = 1 << 21;
pub const ANGULAR: u32 = 1 << 22;
pub const LATE_MIP: u32 = 1 << 27;
