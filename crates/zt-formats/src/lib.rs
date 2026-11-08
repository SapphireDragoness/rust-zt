//! Parsers for the game's data formats. Pure functions over byte slices; no I/O.

pub mod ani;
pub mod ini;
pub mod pal;
pub mod sprite;
pub mod strings;
pub mod zoo;

pub use ani::Ani;
pub use ini::Ini;
pub use pal::Palette;
pub use sprite::{Frame, Sprite};

#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    #[error("truncated data ({0})")]
    Truncated(&'static str),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, FormatError>;

/// Little-endian cursor over a byte slice.
#[derive(Clone)]
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub fn seek(&mut self, pos: usize) -> Result<()> {
        if pos > self.data.len() {
            return Err(FormatError::Truncated("seek"));
        }
        self.pos = pos;
        Ok(())
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or(FormatError::Truncated("length overflow"))?;
        let s = self.data.get(self.pos..end).ok_or(FormatError::Truncated("read"))?;
        self.pos = end;
        Ok(s)
    }

    pub fn rest(&mut self) -> &'a [u8] {
        let s = &self.data[self.pos.min(self.data.len())..];
        self.pos = self.data.len();
        s
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16> {
        let b = self.bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn i16(&mut self) -> Result<i16> {
        Ok(self.u16()? as i16)
    }

    pub fn u32(&mut self) -> Result<u32> {
        let b = self.bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }

    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }
}
