//! Sprite ("graphic") files: the extension-less `N`, `NE`, `E`, `SE`, `S` files.
//!
//! Layout (little endian, see also `ztedit/ztedit/sprite.py`):
//! ```text
//! [optional] "FATZ" 00 00 00 00 <has_background: u8>
//! u32 frame_ms, u32 len, char palette_path[len]   (NUL-terminated)
//! u32 frame_count
//! frames (frame_count, +1 background frame if has_background)
//! ```
//! Frame: `u32 size, u16 height, u16 width, i16 anchor_y, i16 anchor_x,
//! u16 flags`, then per row `u8 n` and `n × (u8 skip, u8 run, run × u8 index)`.
//!
//! If bit 15 of `height` is set (Marine Mania surface/underwater animations),
//! the frame is a *mask*: rows are `n × (u8 skip, u8 run)` with no pixel
//! bytes, and covered pixels are drawn as a single shade. In all shipped mask
//! frames `flags` is `0x74c8`; its meaning (probably the shade) is unknown.

use crate::{FormatError, Reader, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub width: u16,
    /// Pixel height (bit 15 of the stored field removed, see [`Frame::mask`]).
    pub height: u16,
    /// Mask frame: runs carry no pixel data.
    pub mask: bool,
    /// Anchor point (ground contact) measured from the frame's left edge.
    pub anchor_x: i16,
    /// Anchor point measured from the frame's top edge.
    pub anchor_y: i16,
    pub flags: u16,
    /// Encoded row data, kept for byte-exact round trips.
    pub rows: Vec<u8>,
}

/// Index value used for transparent pixels by [`Frame::decode`].
pub const TRANSPARENT: u16 = 0xFFFF;
/// Value used for covered pixels of mask frames by [`Frame::decode`].
pub const MASKED: u16 = 0xFFFE;

const MASK_BIT: u16 = 0x8000;

impl Frame {
    fn parse(r: &mut Reader) -> Result<Frame> {
        let size = r.u32()? as usize;
        Frame::parse_body(r.bytes(size)?)
    }

    /// Parse a frame without its `u32 size` prefix. This is also the whole
    /// content of a `.lle` terrain tile (whose palette is the sibling `.pal`).
    pub fn parse_body(body: &[u8]) -> Result<Frame> {
        let mut b = Reader::new(body);
        let raw_height = b.u16()?;
        let (height, mask) = (raw_height & !MASK_BIT, raw_height & MASK_BIT != 0);
        let width = b.u16()?;
        let anchor_y = b.i16()?;
        let anchor_x = b.i16()?;
        let flags = b.u16()?;
        let rows = b.rest().to_vec();
        let f = Frame { width, height, mask, anchor_x, anchor_y, flags, rows };
        f.validate()?;
        Ok(f)
    }

    fn validate(&self) -> Result<()> {
        let mut q = 0usize;
        let d = &self.rows;
        for _ in 0..self.height {
            let n = *d.get(q).ok_or(FormatError::Truncated("sprite row"))? as usize;
            q += 1;
            for _ in 0..n {
                let run = *d.get(q + 1).ok_or(FormatError::Truncated("sprite run"))? as usize;
                q += if self.mask { 2 } else { 2 + run };
            }
        }
        if q != d.len() {
            return Err(FormatError::Invalid(format!(
                "frame row data length mismatch ({q} != {})",
                d.len()
            )));
        }
        Ok(())
    }

    /// Decode into `width * height` palette indices, [`TRANSPARENT`] for holes.
    pub fn decode(&self) -> Vec<u16> {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut out = vec![TRANSPARENT; w * h];
        let d = &self.rows;
        let mut q = 0;
        for y in 0..h {
            let n = d[q] as usize;
            q += 1;
            let mut col = 0usize;
            for _ in 0..n {
                let (skip, run) = (d[q] as usize, d[q + 1] as usize);
                q += 2;
                col += skip;
                for k in 0..run {
                    if col + k < w {
                        out[y * w + col + k] = if self.mask { MASKED } else { d[q + k] as u16 };
                    }
                }
                if !self.mask {
                    q += run;
                }
                col += run;
            }
        }
        out
    }

    /// Re-encode decoded pixels with the game's run layout. For mask frames
    /// pass `mask = true`; any non-transparent pixel counts as covered.
    pub fn encode_rows(width: u16, height: u16, mask: bool, pixels: &[u16]) -> Result<Vec<u8>> {
        let w = width as usize;
        let mut out = Vec::new();
        for y in 0..height as usize {
            let row = &pixels[y * w..(y + 1) * w];
            let mut instr: Vec<(u8, Vec<u8>)> = Vec::new();
            let mut col = 0;
            while col < w {
                let mut skip = 0u8;
                while col < w && row[col] == TRANSPARENT && skip < 255 {
                    skip += 1;
                    col += 1;
                }
                let mut run = Vec::new();
                while col < w && row[col] != TRANSPARENT && run.len() < 255 {
                    run.push(row[col] as u8);
                    col += 1;
                }
                instr.push((skip, run));
            }
            if instr.len() > 255 {
                return Err(FormatError::Invalid("row too complex (>255 runs)".into()));
            }
            out.push(instr.len() as u8);
            for (skip, run) in instr {
                out.push(skip);
                out.push(run.len() as u8);
                if !mask {
                    out.extend_from_slice(&run);
                }
            }
        }
        Ok(out)
    }

    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&((10 + self.rows.len()) as u32).to_le_bytes());
        let h = self.height | if self.mask { MASK_BIT } else { 0 };
        out.extend_from_slice(&h.to_le_bytes());
        out.extend_from_slice(&self.width.to_le_bytes());
        out.extend_from_slice(&self.anchor_y.to_le_bytes());
        out.extend_from_slice(&self.anchor_x.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        out.extend_from_slice(&self.rows);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sprite {
    /// Resource path of the palette, e.g. `animals/zebra/zebra.pal`.
    pub palette_path: String,
    pub frame_ms: u32,
    pub frames: Vec<Frame>,
    pub fatz: bool,
    /// Extra frame drawn underneath (FATZ sprites only).
    pub background: Option<Frame>,
}

impl Sprite {
    pub fn parse(data: &[u8]) -> Result<Sprite> {
        let mut r = Reader::new(data);
        let fatz = data.starts_with(b"FATZ");
        let mut has_bg = false;
        if fatz {
            r.bytes(8)?;
            has_bg = r.u8()? != 0;
        }
        let frame_ms = r.u32()?;
        let plen = r.u32()? as usize;
        if plen == 0 || plen > 1024 {
            return Err(FormatError::Invalid(format!("bad palette path length {plen}")));
        }
        let p = r.bytes(plen)?;
        let end = p.iter().position(|&c| c == 0).unwrap_or(p.len());
        let palette_path = p[..end].iter().map(|&b| b as char).collect();
        let count = r.u32()? as usize;
        if count > 4096 {
            return Err(FormatError::Invalid(format!("bad frame count {count}")));
        }
        let frames = (0..count).map(|_| Frame::parse(&mut r)).collect::<Result<Vec<_>>>()?;
        let background = if has_bg { Some(Frame::parse(&mut r)?) } else { None };
        Ok(Sprite { palette_path, frame_ms, frames, fatz, background })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        if self.fatz {
            out.extend_from_slice(b"FATZ\0\0\0\0");
            out.push(self.background.is_some() as u8);
        }
        out.extend_from_slice(&self.frame_ms.to_le_bytes());
        out.extend_from_slice(&((self.palette_path.len() + 1) as u32).to_le_bytes());
        out.extend(self.palette_path.chars().map(|c| c as u8));
        out.push(0);
        out.extend_from_slice(&(self.frames.len() as u32).to_le_bytes());
        for f in &self.frames {
            f.write(&mut out);
        }
        if self.fatz {
            if let Some(bg) = &self.background {
                bg.write(&mut out);
            }
        }
        out
    }

    /// `(x0, y0, x1, y1)` of all frames relative to the anchor.
    pub fn bounds(&self) -> (i32, i32, i32, i32) {
        let fs = self.frames.iter().chain(self.background.as_ref());
        let mut b: Option<(i32, i32, i32, i32)> = None;
        for f in fs {
            let (x0, y0) = (-(f.anchor_x as i32), -(f.anchor_y as i32));
            let (x1, y1) = (f.width as i32 + x0, f.height as i32 + y0);
            b = Some(match b {
                None => (x0, y0, x1, y1),
                Some(o) => (o.0.min(x0), o.1.min(y0), o.2.max(x1), o.3.max(y1)),
            });
        }
        b.unwrap_or((0, 0, 0, 0))
    }
}

/// Cheap sniff test for sprite data.
pub fn is_sprite(data: &[u8]) -> bool {
    let pos = if data.starts_with(b"FATZ") { 9 } else { 0 };
    let Some(h) = data.get(pos..pos + 8) else { return false };
    let plen = u32::from_le_bytes([h[4], h[5], h[6], h[7]]) as usize;
    if !(1..260).contains(&plen) {
        return false;
    }
    let Some(name) = data.get(pos + 8..pos + 8 + plen) else { return false };
    name.ends_with(&[0]) && name.to_ascii_lowercase().windows(4).any(|w| w == b".pal")
}
