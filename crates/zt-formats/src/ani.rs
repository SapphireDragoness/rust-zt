//! `.ani` files: an INI section naming the per-direction sprite files of an
//! animation, plus the bounding box of all frames relative to the anchor.

use crate::ini::Ini;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ani {
    /// Folder components below the resource root (`dir0`, `dir1`, …).
    pub dirs: Vec<String>,
    /// One sprite file per direction, e.g. `["N", "NE", "E", "SE", "S"]`.
    pub animations: Vec<String>,
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Ani {
    pub fn parse(text: &str) -> Ani {
        let ini = Ini::parse(text);
        let mut ani = Ani::default();
        let mut dirs: Vec<(u32, String)> = Vec::new();
        for s in &ini.sections {
            for e in &s.entries {
                let Some(v) = e.value.as_deref() else { continue };
                let v = v.split(';').next().unwrap_or("").trim();
                let k = e.key.to_ascii_lowercase();
                if let Some(n) = k.strip_prefix("dir").and_then(|n| n.parse().ok()) {
                    dirs.push((n, v.to_string()));
                } else {
                    let iv = || crate::ini::parse_int(v).unwrap_or(0) as i32;
                    match k.as_str() {
                        "animation" => ani.animations.push(v.to_string()),
                        "x0" => ani.x0 = iv(),
                        "y0" => ani.y0 = iv(),
                        "x1" => ani.x1 = iv(),
                        "y1" => ani.y1 = iv(),
                        _ => {}
                    }
                }
            }
        }
        dirs.sort_by_key(|d| d.0);
        ani.dirs = dirs.into_iter().map(|d| d.1).collect();
        ani
    }

    pub fn folder(&self) -> String {
        self.dirs.join("/")
    }

    /// Resource path of the sprite for direction slot `i`.
    pub fn sprite_path(&self, i: usize) -> Option<String> {
        self.animations.get(i).map(|a| {
            if self.dirs.is_empty() { a.clone() } else { format!("{}/{a}", self.folder()) }
        })
    }
}
