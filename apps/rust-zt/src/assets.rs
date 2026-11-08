//! Loading game assets for rendering.

use anyhow::{Context, Result};
use glam::Vec3;
use zt_formats::Ini;
use zt_render::terrain::{Lighting, TerrainTextures};
use zt_vfs::Vfs;

pub const TERRAIN_TEX_SIZE: u32 = 256;

/// Texture of vertical terrain faces (banks, pits, map sides). The original
/// draws drops in a dark earth texture that matches the asphalt texture.
pub const CLIFF_TEXTURE: &str = "terrain/aphalt.tga";

/// Terrain type table from `terrain/tiletex*.cfg`.
#[derive(Clone, Debug, Default)]
pub struct TerrainTypes {
    /// Indexed by terrain type id.
    pub entries: Vec<Option<TerrainType>>,
}

#[derive(Clone, Debug)]
pub struct TerrainType {
    #[allow(dead_code)] // shown in UI later
    pub name: String,
    pub texture: String,
    pub blend: bool,
}

impl TerrainTypes {
    pub fn load(vfs: &Vfs) -> Result<TerrainTypes> {
        let mut t = TerrainTypes::default();
        for path in vfs.paths_with_prefix("terrain/tiletex") {
            if !path.ends_with(".cfg") {
                continue;
            }
            let ini = Ini::parse(&vfs.read_text(path)?);
            for s in &ini.sections {
                let (Some(id), Some(tex)) = (s.get_int("type"), s.get("texture")) else { continue };
                let id = id as usize;
                if t.entries.len() <= id {
                    t.entries.resize(id + 1, None);
                }
                t.entries[id] = Some(TerrainType {
                    name: s.name.clone(),
                    texture: tex.to_string(),
                    blend: s.get_int("blend").unwrap_or(1) != 0,
                });
            }
        }
        anyhow::ensure!(!t.entries.is_empty(), "no terrain types found");
        Ok(t)
    }

    pub fn blends(&self, id: u8) -> bool {
        self.entries.get(id as usize).and_then(|e| e.as_ref()).is_none_or(|e| e.blend)
    }

    /// Texture layer of the cliff texture (after the per-type layers).
    pub fn cliff_layer(&self) -> u32 {
        self.entries.len() as u32
    }

    /// One texture layer per type id (magenta for unknown ids), then the
    /// cliff texture.
    pub fn textures(&self, vfs: &Vfs) -> Result<TerrainTextures> {
        let n = TERRAIN_TEX_SIZE;
        let load = |path: &str| -> Result<Vec<u8>> {
            let bytes = vfs.read(path).with_context(|| path.to_string())?;
            let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Tga)
                .with_context(|| format!("decoding {path}"))?
                .to_rgba8();
            Ok(image::imageops::resize(&img, n, n, image::imageops::FilterType::Triangle).into_raw())
        };
        let mut layers = Vec::new();
        for e in &self.entries {
            layers.push(match e {
                Some(e) => load(&e.texture)?,
                None => image::RgbaImage::from_pixel(n, n, image::Rgba([255, 0, 255, 255])).into_raw(),
            });
        }
        layers.push(load(CLIFF_TEXTURE)?);
        Ok(TerrainTextures { layers, size: n })
    }
}

/// Lights from `terrain/tilevar.cfg`, falling back to the shipped defaults.
pub fn lighting(vfs: &Vfs) -> Lighting {
    let Ok(text) = vfs.read_text("terrain/tilevar.cfg") else { return Lighting::default() };
    let ini = Ini::parse(&text);
    let f = |sec: &str, k: &str| ini.get_float(sec, k).unwrap_or(0.0) as f32;
    let mut lights = Vec::new();
    for i in 0.. {
        let dir = format!("light{i}.direction");
        if ini.section(&dir).is_none() {
            break;
        }
        let diffuse = format!("light{i}.diffuse");
        let d = Vec3::new(f(&dir, "X"), f(&dir, "Y"), f(&dir, "Z"));
        // Use the green channel as the (grey) intensity.
        lights.push((d, f(&diffuse, "G") * f("mtrl.diffuse", "G").max(0.0)));
    }
    if lights.is_empty() {
        return Lighting::default();
    }
    let ambient = (0..lights.len())
        .map(|i| f(&format!("light{i}.ambient"), "G"))
        .sum::<f32>()
        * f("mtrl.ambient", "G");
    Lighting { ambient, lights }
}

/// All `.zoo` maps, sorted.
pub fn map_list(vfs: &Vfs) -> Vec<String> {
    vfs.paths_with_prefix("maps/").into_iter().filter(|p| p.ends_with(".zoo")).map(String::from).collect()
}

/// Resolve a user-supplied map name (`nile`, `NILE.ZOO`, `maps/nile.zoo`).
pub fn resolve_map(vfs: &Vfs, name: &str) -> Option<String> {
    let n = name.to_ascii_lowercase();
    let n = n.strip_suffix(".zoo").unwrap_or(&n);
    let n = n.rsplit('/').next().unwrap_or(n);
    map_list(vfs).into_iter().find(|p| p == &format!("maps/{n}.zoo"))
}
