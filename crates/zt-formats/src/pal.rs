//! `.pal` palettes: `u16 count, u16 0`, then RGBA entries.
//!
//! A few shipped files hold more entries than `count`; all entries are kept.
//! Sprite transparency comes from skipped pixels, not from palette alpha.

use crate::{FormatError, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    pub colors: Vec<[u8; 4]>,
    /// Header count (may differ from `colors.len()`).
    pub count: u16,
}

impl Palette {
    pub fn parse(data: &[u8]) -> Result<Palette> {
        if data.len() < 4 {
            return Err(FormatError::Truncated("palette header"));
        }
        let count = u16::from_le_bytes([data[0], data[1]]);
        let colors = data[4..].chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect();
        Ok(Palette { colors, count })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + self.colors.len() * 4);
        out.extend_from_slice(&self.count.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        for c in &self.colors {
            out.extend_from_slice(c);
        }
        out
    }

    /// 256 opaque RGBA colours (missing entries black), as used for drawing.
    pub fn rgba256(&self) -> [[u8; 4]; 256] {
        let mut out = [[0, 0, 0, 255]; 256];
        for (o, c) in out.iter_mut().zip(&self.colors) {
            *o = [c[0], c[1], c[2], 255];
        }
        out
    }
}
