//! Little-endian binary cursor shared by the format parsers.

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
#[error("file is truncated")]
pub struct Truncated;

pub type Vec3 = [f32; 3];

pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    pub fn pos(&self) -> usize {
        self.pos
    }
    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }
    pub fn seek(&mut self, pos: usize) {
        self.pos = pos;
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], Truncated> {
        let end = self.pos.checked_add(n).ok_or(Truncated)?;
        let s = self.data.get(self.pos..end).ok_or(Truncated)?;
        self.pos = end;
        Ok(s)
    }
    pub fn skip(&mut self, n: usize) -> Result<(), Truncated> {
        self.take(n).map(|_| ())
    }
    pub fn u8(&mut self) -> Result<u8, Truncated> {
        Ok(self.take(1)?[0])
    }
    pub fn i16(&mut self) -> Result<i16, Truncated> {
        Ok(i16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn u16(&mut self) -> Result<u16, Truncated> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn i32(&mut self) -> Result<i32, Truncated> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32, Truncated> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn f32(&mut self) -> Result<f32, Truncated> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn vec3(&mut self) -> Result<Vec3, Truncated> {
        Ok([self.f32()?, self.f32()?, self.f32()?])
    }
    /// Fixed-size NUL-padded string field (legacy 8-bit encoding, decoded as Latin-1).
    pub fn string(&mut self, n: usize) -> Result<String, Truncated> {
        let b = self.take(n)?;
        let end = b.iter().position(|&c| c == 0).unwrap_or(n);
        Ok(b[..end].iter().map(|&c| c as char).collect())
    }
}
