//! `.zoo` map/save files (reverse-engineered, partial).
//!
//! Header (little endian):
//! ```text
//! "TZFB" u8 version ('F','G' base game; 'R','S' Dinosaur Digs; 'h','i','j' Marine Mania) 00 00 00
//! u32 language id (0x409)
//! [version >= 'R': u32 unknown]
//! u32 width, u32 height      (tiles)
//! u32 unknown, u32 unknown
//! u32 exhibit count, then exhibits: u32 x, u32 y, string name, 64 bytes (F/G layout;
//!     Marine Mania's longer layout with tank settings: see `parse_mm_exhibits`)
//! u32 unknown (small), u32 unknown (checksum?)       (F/G; newer versions add more here)
//! width*height tile records of 10 bytes, row-major by y
//! u32 count, object records ...
//! ```
//!
//! Object records start with 2–4 length-prefixed identifiers: a group
//! (`fences`, `paths`, `objects`, `building`, `animals`, `ambient`, …), usually a
//! class (`foliage`, `building`, `paths`), the type (`zoobars`, `gingkos`) and,
//! for types with subtypes, a one-letter subtype (`f`/`g` fence/gate,
//! `m`/`f`/`y` animals). Marine Mania tank walls are `tankwall` + type +
//! subtype (no class), placed at the tank floor (e.g. z = −64 for the default
//! 4-step sink). Scenery, fences and paths then share a header:
//! ```text
//! u32 unknown (type code?), u32 unknown, [u8], i32 x, i32 y, i32 z, [u8 | u32],
//! u32 facing (0, 2, 4, 6), u32 object id, string name, class-specific data
//! ```
//! x and y are in 1/64 tile, z in 1/16 elevation step (= screen pixels). The
//! optional bytes depend on the class, so [`scan_objects`] tries each layout
//! and validates it. Animal, guest and staff records first carry several
//! hundred bytes of unit state (behaviour scripts, needs, …); the same
//! x/y/z/facing/id/name block follows, and is found by searching forward.
//! Strings are `u32 length` + bytes (no terminator).
//!
//! Tile record: `i32 height, u8 shape, u8 terrain, u8 edges, u8[3] unknown`.
//! `height` is the elevation of the tile's NW corner in 16-pixel steps. `shape`
//! holds four base-4 digits, LSB first: NW, SW, SE, NE (the keys of
//! `tile.cfg`); corner elevation = height + digit(corner) − digit(NW).
//! `edges` looks derived from the shape (per-edge slope flags).
//!
//! Grid convention: tile (x, y) has corners NW = vertex (x, y), NE = (x+1, y),
//! SE = (x+1, y+1), SW = (x, y+1). At rotation 0, NW is the left corner of the
//! on-screen diamond, NE the top, SE the right and SW the bottom.

use crate::{FormatError, Reader, Result};

/// Slope shapes that occur in shipped data (`tile.cfg` sections).
pub const SHAPES: [u8; 19] = [0, 1, 4, 5, 16, 17, 20, 21, 25, 64, 65, 68, 69, 70, 80, 81, 84, 100, 145];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tile {
    pub height: i32,
    pub shape: u8,
    pub terrain: u8,
    pub edges: u8,
    pub extra: [u8; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corner {
    NW = 0,
    NE = 1,
    SE = 2,
    SW = 3,
}

impl Tile {
    fn parse(b: &[u8]) -> Tile {
        Tile {
            height: i32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            shape: b[4],
            terrain: b[5],
            edges: b[6],
            extra: [b[7], b[8], b[9]],
        }
    }

    fn digit(&self, i: u32) -> i32 {
        ((self.shape >> (2 * i)) & 3) as i32
    }

    /// Corner elevations in the order NW, NE, SE, SW.
    pub fn corners(&self) -> [i32; 4] {
        let base = self.height - self.digit(0);
        [self.height, base + self.digit(3), base + self.digit(2), base + self.digit(1)]
    }

    pub fn corner(&self, c: Corner) -> i32 {
        self.corners()[c as usize]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exhibit {
    pub x: u32,
    pub y: u32,
    pub name: String,
    /// Base-game layout: the 64 bytes after the name (undecoded).
    pub data: Vec<u8>,
    /// Marine Mania layout: gate tile and facing.
    pub gate: Option<(u32, u32, u32)>,
    /// Marine Mania: tank settings, if the exhibit is a tank.
    pub tank: Option<Tank>,
}

/// Tank settings stored with a Marine Mania exhibit (`tanks.cfg` names the
/// defaults: wall height 5, water level one step below the top).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tank {
    /// Show tank (trick shows); these store a variable-length show block.
    pub show: bool,
    /// Wall height in elevation steps, from the tank floor.
    pub height: u32,
    /// Water surface in elevation steps above the tank floor.
    pub water_level: u32,
    /// `initialFillState`: 1 filling, 0 emptying.
    pub filling: bool,
    /// 100 = salt water, 0 = fresh water.
    pub salinity: u32,
    /// Second salinity-like value (target salinity?), same as `salinity` in saves seen so far.
    pub salinity2: u32,
    /// Water purity 0–100 (scum images below 60, 20 and 1).
    pub purity: u32,
}

#[derive(Clone, Debug)]
pub struct ZooMap {
    pub version: u8,
    pub language: u32,
    pub width: u32,
    pub height: u32,
    pub exhibits: Vec<Exhibit>,
    pub tiles: Vec<Tile>,
    /// File offset of the tile array.
    pub tiles_offset: usize,
    /// File offset of the object section (the `u32` before the first record).
    pub objects_offset: usize,
    /// The `u32` at `objects_offset` (number of placed objects).
    pub object_count: u32,
    pub objects: Vec<ObjectRecord>,
}

/// A placed object from the object section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectRecord {
    pub offset: usize,
    /// Group string (`fences`, `objects`, `building`, `paths`, `animals`, …).
    pub group: String,
    pub type_name: String,
    pub subtype: Option<String>,
    /// Decoded header, if the layout was recognised.
    pub placement: Option<Placement>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    /// 1/64 tile.
    pub x: i32,
    pub y: i32,
    /// 1/16 elevation step.
    pub z: i32,
    pub facing: u32,
    pub id: u32,
    pub name: String,
}

/// Group identifiers that start an object record (the section names of the
/// top-level `.cfg` files; `tankwall` is Marine Mania's tank walls).
pub const GROUPS: [&str; 9] =
    ["objects", "building", "fences", "tankwall", "paths", "animals", "guests", "staff", "ambient"];

impl ZooMap {
    pub fn tile(&self, x: u32, y: u32) -> Option<&Tile> {
        (x < self.width && y < self.height).then(|| &self.tiles[(y * self.width + x) as usize])
    }

    /// Whether the version uses the base-game header layout.
    pub fn is_base_layout(version: u8) -> bool {
        version < b'R'
    }

    pub fn parse(data: &[u8]) -> Result<ZooMap> {
        let mut r = Reader::new(data);
        if r.bytes(4)? != b"TZFB" {
            return Err(FormatError::Invalid("not a .zoo file (missing TZFB)".into()));
        }
        let version = r.u8()?;
        r.bytes(3)?;
        let language = r.u32()?;
        if !Self::is_base_layout(version) {
            r.u32()?;
        }
        let width = r.u32()?;
        let height = r.u32()?;
        if width == 0 || height == 0 || width > 1024 || height > 1024 {
            return Err(FormatError::Invalid(format!("bad map size {width}x{height}")));
        }
        let n_tiles = (width * height) as usize;
        r.u32()?;
        r.u32()?;

        let mut exhibits = Vec::new();
        let tiles_offset = if Self::is_base_layout(version) {
            let count = r.u32()?;
            for _ in 0..count {
                let x = r.u32()?;
                let y = r.u32()?;
                let name = read_string(&mut r)?;
                let data = r.bytes(64)?.to_vec();
                exhibits.push(Exhibit { x, y, name, data, gate: None, tank: None });
            }
            r.u32()?;
            r.u32()?;
            r.pos()
        } else {
            if version >= b'h' {
                // Best effort: a map whose exhibit list does not decode still loads.
                exhibits = parse_mm_exhibits(data, r.pos(), width, height).unwrap_or_default();
            }
            find_tiles_by_objects(data, r.pos(), n_tiles)?
        };

        let end = tiles_offset + n_tiles * 10;
        let raw = data.get(tiles_offset..end).ok_or(FormatError::Truncated("tile array"))?;
        let tiles: Vec<Tile> = raw.chunks_exact(10).map(Tile::parse).collect();
        if !tiles_plausible(&tiles) {
            return Err(FormatError::Invalid(format!(
                "implausible tile data at {tiles_offset:#x} (version {:?})",
                version as char
            )));
        }
        let object_count = Reader::new(data.get(end..).unwrap_or(&[])).u32().unwrap_or(0);
        let objects = scan_objects(data, end + 4, width, height);
        Ok(ZooMap {
            version,
            language,
            width,
            height,
            exhibits,
            tiles,
            tiles_offset,
            objects_offset: end,
            object_count,
            objects,
        })
    }
}

fn u32_at(data: &[u8], pos: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(pos..pos + 4)?.try_into().ok()?))
}

/// Start of a Marine Mania exhibit record at `pos`: `u32 x, u32 y, string
/// name, u32 gate x, u32 gate y, u32 gate facing`. Returns (x, y, name, gate,
/// end of the gate fields).
fn mm_exhibit_head(data: &[u8], pos: usize, w: u32, h: u32) -> Option<(u32, u32, String, (u32, u32, u32), usize)> {
    let (x, y) = (u32_at(data, pos)?, u32_at(data, pos + 4)?);
    if x >= w || y >= h {
        return None;
    }
    let (name, q) = name_at(data, pos + 8)?;
    let gate = (u32_at(data, q)?, u32_at(data, q + 4)?, u32_at(data, q + 8)?);
    if gate.0 >= w || gate.1 >= h || gate.2 > 7 {
        return None;
    }
    Some((x, y, name, gate, q + 12))
}

/// Size of the tank fields that end a tank exhibit.
const TANK_FIELDS: usize = 21;

/// Tank fields at `pos`: `u32 height, u32 water level, u8 filling,
/// u32 salinity, u32 salinity2, u32 purity`, if plausible.
fn tank_fields(data: &[u8], pos: usize, show: bool) -> Option<Tank> {
    let height = u32_at(data, pos)?;
    let water_level = u32_at(data, pos + 4)?;
    let filling = *data.get(pos + 8)?;
    let (salinity, salinity2, purity) = (u32_at(data, pos + 9)?, u32_at(data, pos + 13)?, u32_at(data, pos + 17)?);
    let ok = (1..=64).contains(&height)
        && water_level <= height
        && filling <= 1
        && salinity <= 100
        && salinity2 <= 100
        && purity <= 100;
    ok.then_some(Tank { show, height, water_level, filling: filling == 1, salinity, salinity2, purity })
}

/// Marine Mania (versions `h`–`j`) exhibit list at `pos` (`u32 count`, then
/// the records):
/// ```text
/// u32 x, u32 y, string name, u32 gate x, u32 gate y, u32 gate facing
/// 37 bytes (two f32 ratings at +8 and +20, rest zero so far), u64 timestamp,
/// 7 bytes, u32 unknown, u8 is_tank, u8 is_show
/// [show tanks: a variable-length show block]
/// [tanks: u32 wall height, u32 water level, u8 filling, u32 salinity,
///  u32 salinity2, u32 purity]
/// ```
/// After the list comes `u32, u32, u16 year (2000/2001), …` and then the tiles.
fn parse_mm_exhibits(data: &[u8], pos: usize, w: u32, h: u32) -> Option<Vec<Exhibit>> {
    let count = u32_at(data, pos)? as usize;
    if count > 1024 {
        return None;
    }
    let mut p = pos + 4;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let (x, y, name, gate, q) = mm_exhibit_head(data, p, w, h)?;
        let q = q + 37 + 8 + 7 + 4;
        let (is_tank, is_show) = (*data.get(q)? != 0, *data.get(q + 1)? != 0);
        let mut q = q + 2;
        let last = i + 1 == count;
        if is_show {
            // Find where the tank fields end: at the next exhibit, or at the
            // trailer (its year field is 9 bytes in).
            let next_ok = |s: usize| {
                let e = s + TANK_FIELDS;
                tank_fields(data, s, true).is_some()
                    && if last {
                        matches!(data.get(e + 8..e + 10), Some([0xd0 | 0xd1 | 0xd2, 0x07]))
                    } else {
                        mm_exhibit_head(data, e, w, h).is_some()
                    }
            };
            q = (q..data.len().min(q + 65536)).find(|&s| next_ok(s))?;
        }
        let tank = if is_tank {
            let t = tank_fields(data, q, is_show)?;
            q += TANK_FIELDS;
            Some(t)
        } else {
            None
        };
        out.push(Exhibit { x, y, name, data: Vec::new(), gate: Some(gate), tank });
        p = q;
    }
    Some(out)
}

pub fn read_string(r: &mut Reader) -> Result<String> {
    let n = r.u32()? as usize;
    if n > 4096 {
        return Err(FormatError::Invalid(format!("string length {n} too large")));
    }
    Ok(r.bytes(n)?.iter().map(|&b| b as char).collect())
}

/// Length-prefixed identifier (`[a-z0-9_]`, 1..=24 chars) at `pos`.
fn ident_at(data: &[u8], pos: usize) -> Option<(&str, usize)> {
    let n = u32::from_le_bytes(data.get(pos..pos + 4)?.try_into().ok()?) as usize;
    if !(1..=24).contains(&n) {
        return None;
    }
    let s = data.get(pos + 4..pos + 4 + n)?;
    if !s.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_') {
        return None;
    }
    Some((std::str::from_utf8(s).ok()?, pos + 4 + n))
}

/// Printable name string (1..=80 chars) at `pos`.
fn name_at(data: &[u8], pos: usize) -> Option<(String, usize)> {
    let n = u32::from_le_bytes(data.get(pos..pos + 4)?.try_into().ok()?) as usize;
    if !(1..=80).contains(&n) {
        return None;
    }
    let s = data.get(pos + 4..pos + 4 + n)?;
    if !s.iter().all(|c| (32..127).contains(c) || *c >= 160) {
        return None;
    }
    Some((s.iter().map(|&b| b as char).collect(), pos + 4 + n))
}

fn i32_at(data: &[u8], pos: usize) -> Option<i32> {
    Some(i32::from_le_bytes(data.get(pos..pos + 4)?.try_into().ok()?))
}

/// Try the known header layouts after the identifiers at `pos`.
fn decode_placement(data: &[u8], pos: usize, w: u32, h: u32) -> Option<(Placement, usize)> {
    // (bytes before x, bytes between z and facing)
    for (pre, mid) in [(8, 0), (9, 1), (8, 4), (9, 0), (8, 1)] {
        let x = i32_at(data, pos + pre)?;
        let y = i32_at(data, pos + pre + 4)?;
        let z = i32_at(data, pos + pre + 8)?;
        let f = pos + pre + 12 + mid;
        let facing = i32_at(data, f)? as u32;
        let id = i32_at(data, f + 4)? as u32;
        if !(0..=(w as i32 * 64)).contains(&x) || !(0..=(h as i32 * 64)).contains(&y) || facing > 7 {
            continue;
        }
        if let Some((name, end)) = name_at(data, f + 8) {
            return Some((Placement { x, y, z, facing, id, name }, end));
        }
    }
    None
}

/// Position of the next group identifier at or after `pos`.
fn next_group(data: &[u8], pos: usize) -> Option<usize> {
    (pos..data.len().saturating_sub(8)).find(|&p| ident_at(data, p).is_some_and(|(g, _)| GROUPS.contains(&g)))
}

/// The x/y/z/facing/id/name block inside unit records (no padding).
fn decode_unit_block(data: &[u8], p: usize, w: u32, h: u32) -> Option<(Placement, usize)> {
    let x = i32_at(data, p)?;
    let y = i32_at(data, p + 4)?;
    // Half a tile minimum rules out runs of small integers before strings like "Male".
    if !(32..(w as i32 * 64)).contains(&x) || !(32..(h as i32 * 64)).contains(&y) {
        return None;
    }
    let z = i32_at(data, p + 8)?;
    let facing = i32_at(data, p + 12)? as u32;
    if facing > 7 || z.abs() > 16 * 64 {
        return None;
    }
    let id = i32_at(data, p + 16)? as u32;
    let (name, end) = name_at(data, p + 20)?;
    // Unit names are capitalised ("Plains Zebra 1", "Guest 28").
    if !name.starts_with(|c: char| c.is_ascii_uppercase()) || name.len() < 3 {
        return None;
    }
    Some((Placement { x, y, z, facing, id, name }, end))
}

/// Find object records by their leading identifiers and decode what we can.
pub fn scan_objects(data: &[u8], start: usize, w: u32, h: u32) -> Vec<ObjectRecord> {
    let mut out = Vec::new();
    let mut pos = start;
    while pos + 8 < data.len() {
        let Some((group, mut q)) = ident_at(data, pos).filter(|(g, _)| GROUPS.contains(g)) else {
            pos += 1;
            continue;
        };
        let mut idents = Vec::new();
        while idents.len() < 3 {
            match ident_at(data, q) {
                Some((s, e)) => {
                    idents.push(s.to_string());
                    q = e;
                }
                None => break,
            }
        }
        if idents.is_empty() {
            pos += 1;
            continue;
        }
        let subtype = (idents.len() >= 2 && idents.last().unwrap().len() == 1).then(|| idents.pop().unwrap());
        let type_name = idents.pop().unwrap();
        let found = decode_placement(data, q, w, h).or_else(|| {
            // Unit records: search forward, but not past the next record.
            let limit = next_group(data, q).unwrap_or(data.len()).min(q + 4096);
            (q..limit.saturating_sub(24)).find_map(|p| decode_unit_block(data, p, w, h))
        });
        let (placement, next) = match found {
            Some((p, end)) => (Some(p), end),
            None => (None, q),
        };
        out.push(ObjectRecord { offset: pos, group: group.to_string(), type_name, subtype, placement });
        pos = next;
    }
    out
}

fn tiles_plausible(tiles: &[Tile]) -> bool {
    tiles.iter().all(|t| SHAPES.contains(&t.shape) && t.terrain < 64 && t.height.abs() < 4096)
}

/// For versions whose pre-tile header is not fully understood: the object
/// section starts with `u32 count` and a length-prefixed lowercase identifier.
/// Try each such position and keep the first whose preceding tile array
/// validates.
fn find_tiles_by_objects(data: &[u8], min: usize, n_tiles: usize) -> Result<usize> {
    let need = n_tiles * 10 + 4;
    let mut pos = min + need;
    while pos + 8 < data.len() {
        let len = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        if (3..=16).contains(&len) {
            if let Some(s) = data.get(pos + 4..pos + 4 + len) {
                if s.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_') {
                    let start = pos - need;
                    let tiles: Vec<Tile> =
                        data[start..start + n_tiles * 10].chunks_exact(10).map(Tile::parse).collect();
                    if tiles_plausible(&tiles) {
                        return Ok(start);
                    }
                }
            }
        }
        pos += 1;
    }
    Err(FormatError::Invalid("could not locate tile array".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corner_model() {
        // 0x46 = digits (NW=2, SW=1, SE=0, NE=1): NW highest.
        let t = Tile { height: 5, shape: 0x46, terrain: 0, edges: 0, extra: [0; 3] };
        assert_eq!(t.corners(), [5, 4, 3, 4]);
        let flat = Tile { height: -1, shape: 0, terrain: 0, edges: 0, extra: [0; 3] };
        assert_eq!(flat.corners(), [-1; 4]);
    }
}
