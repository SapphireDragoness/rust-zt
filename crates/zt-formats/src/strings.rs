//! String tables from the game's resource DLLs (`lang*.dll`, `res*.dll`).
//!
//! These are ordinary Win32 PE files; strings live in `RT_STRING` resources:
//! block `b` holds IDs `(b-1)*16 ..= (b-1)*16+15`, each stored as
//! `u16 length` + UTF-16LE code units.

use std::collections::BTreeMap;

use crate::{FormatError, Reader, Result};

const RT_STRING: u32 = 6;

/// One resource from a PE file's resource tree.
#[derive(Clone, Debug)]
pub struct PeResource<'a> {
    pub type_id: ResId,
    pub name: ResId,
    pub lang: u32,
    pub data: &'a [u8],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResId {
    Id(u32),
    Name(String),
}

struct Section {
    va: u32,
    vsize: u32,
    raw_off: u32,
    raw_size: u32,
}

/// Walk all resources of a PE image.
pub fn pe_resources(data: &[u8]) -> Result<Vec<PeResource<'_>>> {
    let mut r = Reader::new(data);
    if r.bytes(2)? != b"MZ" {
        return Err(FormatError::Invalid("not an MZ executable".into()));
    }
    r.seek(0x3c)?;
    let pe = r.u32()? as usize;
    r.seek(pe)?;
    if r.bytes(4)? != b"PE\0\0" {
        return Err(FormatError::Invalid("missing PE signature".into()));
    }
    r.u16()?; // machine
    let nsec = r.u16()? as usize;
    r.bytes(12)?;
    let opt_size = r.u16()? as usize;
    r.u16()?; // characteristics
    let opt = r.pos();
    let magic = r.u16()?;
    let dd = match magic {
        0x10b => opt + 96,
        0x20b => opt + 112,
        m => return Err(FormatError::Invalid(format!("unknown optional header magic {m:#x}"))),
    };
    r.seek(dd + 2 * 8)?; // data directory 2 = resources
    let res_rva = r.u32()?;
    let res_size = r.u32()?;
    if res_rva == 0 || res_size == 0 {
        return Ok(Vec::new());
    }
    r.seek(opt + opt_size)?;
    let mut sections = Vec::with_capacity(nsec);
    for _ in 0..nsec {
        r.bytes(8)?;
        let vsize = r.u32()?;
        let va = r.u32()?;
        let raw_size = r.u32()?;
        let raw_off = r.u32()?;
        r.bytes(16)?;
        sections.push(Section { va, vsize, raw_off, raw_size });
    }
    let to_off = |rva: u32| -> Result<usize> {
        sections
            .iter()
            .find(|s| rva >= s.va && rva < s.va + s.vsize.max(s.raw_size))
            .map(|s| (rva - s.va + s.raw_off) as usize)
            .ok_or_else(|| FormatError::Invalid(format!("rva {rva:#x} outside sections")))
    };
    let base = to_off(res_rva)?;

    // (offset of directory relative to base) -> entries (id, is_dir, offset)
    let dir = |off: usize| -> Result<Vec<(ResId, bool, usize)>> {
        let mut r = Reader::new(data);
        r.seek(base + off + 12)?;
        let n = r.u16()? as usize + r.u16()? as usize;
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            let name = r.u32()?;
            let target = r.u32()?;
            let id = if name & 0x8000_0000 != 0 {
                let mut s = Reader::new(data);
                s.seek(base + (name & 0x7fff_ffff) as usize)?;
                let len = s.u16()? as usize;
                let units: Vec<u16> = (0..len).map(|_| s.u16()).collect::<Result<_>>()?;
                ResId::Name(String::from_utf16_lossy(&units))
            } else {
                ResId::Id(name)
            };
            v.push((id, target & 0x8000_0000 != 0, (target & 0x7fff_ffff) as usize));
        }
        Ok(v)
    };

    let mut out = Vec::new();
    for (type_id, is_dir, off) in dir(0)? {
        if !is_dir {
            continue;
        }
        for (name, is_dir, off) in dir(off)? {
            if !is_dir {
                continue;
            }
            for (lang, is_dir, off) in dir(off)? {
                if is_dir {
                    continue;
                }
                let mut e = Reader::new(data);
                e.seek(base + off)?;
                let rva = e.u32()?;
                let size = e.u32()? as usize;
                let start = to_off(rva)?;
                let bytes = data
                    .get(start..start + size)
                    .ok_or(FormatError::Truncated("resource data"))?;
                let lang = match lang {
                    ResId::Id(l) => l,
                    ResId::Name(_) => 0,
                };
                out.push(PeResource { type_id: type_id.clone(), name: name.clone(), lang, data: bytes });
            }
        }
    }
    Ok(out)
}

/// All `RT_STRING` strings of a DLL by ID. If a block exists in several
/// languages, US English (0x409) wins, otherwise the first one found.
pub fn string_table(dll: &[u8]) -> Result<BTreeMap<u32, String>> {
    let mut blocks: BTreeMap<u32, (u32, &[u8])> = BTreeMap::new();
    for res in pe_resources(dll)? {
        if res.type_id != ResId::Id(RT_STRING) {
            continue;
        }
        let ResId::Id(block) = res.name else { continue };
        match blocks.get(&block) {
            Some((lang, _)) if *lang == 0x409 || res.lang != 0x409 => {}
            _ => {
                blocks.insert(block, (res.lang, res.data));
            }
        }
    }
    let mut out = BTreeMap::new();
    for (block, (_, data)) in blocks {
        let mut r = Reader::new(data);
        for i in 0..16u32 {
            let Ok(len) = r.u16() else { break };
            let units: Vec<u16> = (0..len).map(|_| r.u16()).collect::<Result<_>>()?;
            if len > 0 {
                out.insert((block - 1) * 16 + i, String::from_utf16_lossy(&units));
            }
        }
    }
    Ok(out)
}

/// Merged string tables of several DLLs; later DLLs override earlier ones.
#[derive(Clone, Debug, Default)]
pub struct Strings {
    pub map: BTreeMap<u32, String>,
}

impl Strings {
    pub fn add_dll(&mut self, dll: &[u8]) -> Result<usize> {
        let t = string_table(dll)?;
        let n = t.len();
        self.map.extend(t);
        Ok(n)
    }

    pub fn get(&self, id: u32) -> Option<&str> {
        self.map.get(&id).map(String::as_str)
    }
}
