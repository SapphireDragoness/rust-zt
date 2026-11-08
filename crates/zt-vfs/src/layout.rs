//! Locating the game's data layers on disk.
//!
//! Two layouts are supported:
//!
//! * **Disc**: the CD contents copied out, i.e. `ZOO_TYCN/DATA/...` for the
//!   base game and `MARINA/DATA/{DINODIGS,MANIA,DUPDATE,CONTENT}/...` for the
//!   Complete Collection expansion disc.
//! * **Installed**: a normal install folder with `zoo.ini`, the base `.ztd`
//!   files and update/expansion subfolders.
//!
//! Layers are ordered lowest to highest priority; later layers override
//! earlier ones. Installed copies follow `zoo.ini`'s `[resource] path=`; the
//! disc layout mirrors the Complete Collection's list
//! (`./dlupdate;./updates;./xpack2/loc;./xpack2;./dupdate;./zupdate1;./xpack1/loc;./xpack1;./zupdate;./loc;.`).

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default)]
pub struct Layer {
    pub name: String,
    /// `.ztd` archives, mounted in this order.
    pub archives: Vec<PathBuf>,
    /// Loose directories and the resource prefix they are mounted under.
    pub loose: Vec<(PathBuf, String)>,
    /// Resource DLLs (`lang*.dll`, `res*.dll`) holding string tables.
    pub dlls: Vec<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct GameLayout {
    pub root: PathBuf,
    pub kind: LayoutKind,
    pub layers: Vec<Layer>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutKind {
    Disc,
    Installed,
}

/// Case-insensitive child lookup.
pub fn child_ci(dir: &Path, name: &str) -> Option<PathBuf> {
    let direct = dir.join(name);
    if direct.exists() {
        return Some(direct);
    }
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name))
        .map(|e| e.path())
}

/// Case-insensitive multi-component lookup, e.g. `"MARINA/DATA/MANIA"`.
pub fn path_ci(root: &Path, rel: &str) -> Option<PathBuf> {
    rel.split('/').filter(|s| !s.is_empty()).try_fold(root.to_path_buf(), |d, c| child_ci(&d, c))
}

fn files_with_ext(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension().is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case(ext))
        })
        .collect();
    v.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()));
    v
}

fn dlls(dir: &Path) -> Vec<PathBuf> {
    files_with_ext(dir, "dll")
        .into_iter()
        .filter(|p| {
            let n = p.file_name().unwrap().to_string_lossy().to_ascii_lowercase();
            n.starts_with("lang") || n.starts_with("res")
        })
        .collect()
}

impl Layer {
    fn new(name: &str) -> Self {
        Layer { name: name.into(), ..Default::default() }
    }

    /// Add the archives of `dir` and of its `loc` subfolder.
    fn archives_from(&mut self, dir: &Path) {
        self.archives.extend(files_with_ext(dir, "ztd"));
        if let Some(loc) = child_ci(dir, "loc") {
            self.archives.extend(files_with_ext(&loc, "ztd"));
        }
    }

    fn maps_from(&mut self, dir: &Path) {
        if let Some(maps) = child_ci(dir, "maps") {
            self.loose.push((maps, "maps".into()));
        }
    }

    fn is_empty(&self) -> bool {
        self.archives.is_empty() && self.loose.is_empty() && self.dlls.is_empty()
    }
}

/// The `path=` entry of the `[resource]` section of a `zoo.ini`.
pub fn resource_path(ini: &str) -> Option<Vec<String>> {
    let mut in_resource = false;
    for line in ini.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_resource = l.eq_ignore_ascii_case("[resource]");
        } else if in_resource {
            if let Some((k, v)) = l.split_once('=') {
                if k.trim().eq_ignore_ascii_case("path") {
                    return Some(v.split(';').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect());
                }
            }
        }
    }
    None
}

impl GameLayout {
    /// Detect the layout under `root`. `root` may be the folder containing
    /// `ZOO_TYCN`/`MARINA`, an install folder, or a parent of either.
    pub fn detect(root: &Path) -> Option<GameLayout> {
        if let Some(l) = Self::detect_disc(root).or_else(|| Self::detect_installed(root)) {
            return Some(l);
        }
        // Look one level down (e.g. ".../Zoo Tycoon Complete Collection ISO").
        for e in std::fs::read_dir(root).ok()?.flatten() {
            if e.path().is_dir() {
                if let Some(l) =
                    Self::detect_disc(&e.path()).or_else(|| Self::detect_installed(&e.path()))
                {
                    return Some(l);
                }
            }
        }
        None
    }

    fn detect_disc(root: &Path) -> Option<GameLayout> {
        let base = path_ci(root, "ZOO_TYCN/DATA")?;
        let mut layers = Vec::new();

        let mut l = Layer::new("base");
        l.archives_from(&base);
        l.maps_from(&base);
        l.dlls = dlls(&base);
        layers.push(l);

        if let Some(m) = path_ci(root, "MARINA/DATA") {
            let dino = child_ci(&m, "DINODIGS");
            let mania = child_ci(&m, "MANIA");
            let sub = |d: &Option<PathBuf>, s: &str| d.as_ref().and_then(|d| child_ci(d, s));

            let mut l = Layer::new("zupdate");
            if let Some(d) = sub(&dino, "ZUPDATE") {
                l.archives_from(&d);
            }
            layers.push(l);

            let mut l = Layer::new("xpack1");
            if let Some(d) = sub(&dino, "XPACK1") {
                l.archives_from(&d);
            }
            if let Some(d) = &dino {
                l.maps_from(d);
                l.dlls = dlls(d);
            }
            layers.push(l);

            let mut l = Layer::new("zupdate1");
            if let Some(d) = sub(&mania, "ZUPDATE1") {
                l.archives_from(&d);
            }
            layers.push(l);

            let mut l = Layer::new("dupdate");
            if let Some(d) = child_ci(&m, "DUPDATE") {
                l.archives_from(&d);
            }
            layers.push(l);

            let mut l = Layer::new("xpack2");
            if let Some(d) = sub(&mania, "XPACK2") {
                l.archives_from(&d);
            }
            if let Some(d) = &mania {
                l.maps_from(d);
                l.dlls = dlls(d);
            }
            layers.push(l);

            let mut l = Layer::new("updates");
            if let Some(c) = child_ci(&m, "CONTENT") {
                if let Some(u) = child_ci(&c, "UPDATES") {
                    l.archives_from(&u);
                }
                l.maps_from(&c);
                l.dlls = dlls(&c);
            }
            layers.push(l);
        }
        layers.retain(|l| !l.is_empty());
        Some(GameLayout { root: root.into(), kind: LayoutKind::Disc, layers })
    }

    fn detect_installed(root: &Path) -> Option<GameLayout> {
        let ini = child_ci(root, "zoo.ini")?;
        if files_with_ext(root, "ztd").is_empty() {
            return None;
        }
        // `[resource] path=` lists folders highest priority first, e.g.
        // ./dlupdate;./updates;./xpack2/loc;./xpack2;…;./loc;.
        let entries = std::fs::read(&ini)
            .ok()
            .and_then(|b| resource_path(&String::from_utf8_lossy(&b)))
            .unwrap_or_else(|| {
                ["dlupdate", "updates", "xpack2", "dupdate", "zupdate1", "xpack1", "zupdate", "loc", "."]
                    .map(String::from)
                    .to_vec()
            });
        let mut layers = Vec::new();
        for entry in entries.iter().rev() {
            let rel = entry.trim().trim_start_matches("./").trim_end_matches('/');
            let dir = if rel.is_empty() || rel == "." { Some(root.to_path_buf()) } else { path_ci(root, rel) };
            let Some(dir) = dir else { continue };
            let mut l = Layer::new(if rel.is_empty() || rel == "." { "base" } else { rel });
            l.archives.extend(files_with_ext(&dir, "ztd"));
            l.maps_from(&dir);
            l.dlls = dlls(&dir);
            if !l.is_empty() {
                layers.push(l);
            }
        }
        Some(GameLayout { root: root.into(), kind: LayoutKind::Installed, layers })
    }

    /// All string-table DLLs, oldest layer first.
    pub fn string_dlls(&self) -> impl Iterator<Item = &Path> {
        self.layers.iter().flat_map(|l| l.dlls.iter().map(PathBuf::as_path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_resource_path() {
        let ini = "[user]\npath=nope\n[resource]\npath=./dlupdate;./updates;./loc;.\n";
        assert_eq!(resource_path(ini).unwrap(), ["./dlupdate", "./updates", "./loc", "."]);
    }
}
