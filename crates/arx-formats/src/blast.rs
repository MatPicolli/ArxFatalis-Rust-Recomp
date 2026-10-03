//! Decoder for the PKWare Data Compression Library (DCL "implode") stream used by `.pak` files.
//!
//! Port of Mark Adler's `blast.c` (zlib license) via ArxLibertatis' `io/Blast.cpp`.

use thiserror::Error;

const MAXBITS: usize = 13;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BlastError {
    #[error("invalid literal flag in header")]
    InvalidLiteralFlag,
    #[error("invalid dictionary size in header")]
    InvalidDictSize,
    #[error("distance points before the start of the output")]
    InvalidOffset,
    #[error("compressed stream is truncated")]
    Truncated,
    #[error("invalid huffman code")]
    InvalidCode,
}

const LITLEN: &[u8] = &[
    11, 124, 8, 7, 28, 7, 188, 13, 76, 4, 10, 8, 12, 10, 12, 10, 8, 23, 8, 9, 7, 6, 7, 8, 7, 6, 55,
    8, 23, 24, 12, 11, 7, 9, 11, 12, 6, 7, 22, 5, 7, 24, 6, 11, 9, 6, 7, 22, 7, 11, 38, 7, 9, 8,
    25, 11, 8, 11, 9, 12, 8, 12, 5, 38, 5, 38, 5, 11, 7, 5, 6, 21, 6, 10, 53, 8, 7, 24, 10, 27,
    44, 253, 253, 253, 252, 252, 252, 13, 12, 45, 12, 45, 12, 61, 12, 45, 44, 173,
];
const LENLEN: &[u8] = &[2, 35, 36, 53, 38, 23];
const DISTLEN: &[u8] = &[2, 20, 53, 230, 247, 151, 248];
const LEN_BASE: [u16; 16] = [3, 2, 4, 5, 6, 7, 8, 9, 10, 12, 16, 24, 40, 72, 136, 264];
const LEN_EXTRA: [u8; 16] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8];

struct Huffman {
    count: [i16; MAXBITS + 1],
    symbol: Vec<i16>,
}

impl Huffman {
    /// `rep` is a list of bytes: low nibble = code length, high nibble + 1 = repeat count.
    fn new(rep: &[u8]) -> Self {
        let mut length = Vec::with_capacity(256);
        for &b in rep {
            for _ in 0..=(b >> 4) {
                length.push(b & 15);
            }
        }
        let mut count = [0i16; MAXBITS + 1];
        for &l in &length {
            count[l as usize] += 1;
        }
        let mut offs = [0i16; MAXBITS + 2];
        for len in 1..MAXBITS {
            offs[len + 1] = offs[len] + count[len];
        }
        let mut symbol = vec![0i16; length.len()];
        for (sym, &l) in length.iter().enumerate() {
            if l != 0 {
                symbol[offs[l as usize] as usize] = sym as i16;
                offs[l as usize] += 1;
            }
        }
        Self { count, symbol }
    }
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
    bitbuf: u32,
    bitcnt: u32,
}

impl Reader<'_> {
    fn bits(&mut self, need: u32) -> Result<u32, BlastError> {
        while self.bitcnt < need {
            let b = *self.data.get(self.pos).ok_or(BlastError::Truncated)?;
            self.pos += 1;
            self.bitbuf |= (b as u32) << self.bitcnt;
            self.bitcnt += 8;
        }
        let val = self.bitbuf & ((1u32 << need) - 1);
        self.bitbuf >>= need;
        self.bitcnt -= need;
        Ok(val)
    }

    /// Codes are stored bit-reversed and inverted; see blast.c.
    fn decode(&mut self, h: &Huffman) -> Result<usize, BlastError> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..=MAXBITS {
            code |= (self.bits(1)? as i32) ^ 1;
            let count = h.count[len] as i32;
            if code < first + count {
                return Ok(h.symbol[(index + (code - first)) as usize] as usize);
            }
            index += count;
            first += count;
            first <<= 1;
            code <<= 1;
        }
        Err(BlastError::InvalidCode)
    }
}

/// Decompress a DCL stream. `size_hint` is the expected output size (used for pre-allocation).
pub fn blast(input: &[u8], size_hint: usize) -> Result<Vec<u8>, BlastError> {
    let lit_code = Huffman::new(LITLEN);
    let len_code = Huffman::new(LENLEN);
    let dist_code = Huffman::new(DISTLEN);
    let mut r = Reader { data: input, pos: 0, bitbuf: 0, bitcnt: 0 };
    let mut out = Vec::with_capacity(size_hint);

    let coded_literals = r.bits(8)?;
    if coded_literals > 1 {
        return Err(BlastError::InvalidLiteralFlag);
    }
    let dict = r.bits(8)?;
    if !(4..=6).contains(&dict) {
        return Err(BlastError::InvalidDictSize);
    }

    loop {
        if r.bits(1)? == 1 {
            let sym = r.decode(&len_code)?;
            let len = LEN_BASE[sym] as usize + r.bits(LEN_EXTRA[sym] as u32)? as usize;
            if len == 519 {
                break;
            }
            let shift = if len == 2 { 2 } else { dict };
            let dist = ((r.decode(&dist_code)? as usize) << shift) + r.bits(shift)? as usize + 1;
            if dist > out.len() {
                return Err(BlastError::InvalidOffset);
            }
            // Byte-wise copy: overlapping copies (len > dist) are intentional.
            let start = out.len() - dist;
            for i in 0..len {
                out.push(out[start + i]);
            }
        } else {
            let byte = if coded_literals == 1 { r.decode(&lit_code)? as u8 } else { r.bits(8)? as u8 };
            out.push(byte);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test vector from the blast.c documentation: decompresses to "AIAIAIAIAIAIA".
    #[test]
    fn documented_example() {
        let data = [0x00, 0x04, 0x82, 0x24, 0x25, 0x8f, 0x80, 0x7f];
        assert_eq!(blast(&data, 13).unwrap(), b"AIAIAIAIAIAIA");
    }

    #[test]
    fn truncated() {
        assert_eq!(blast(&[0x00, 0x04, 0x82], 0), Err(BlastError::Truncated));
    }
}
