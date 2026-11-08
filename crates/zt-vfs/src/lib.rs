//! Layered, case-insensitive resource filesystem over `.ztd` (zip) archives
//! and loose files.
//!
//! Resource paths are relative to the resource root and use `/`, e.g.
//! `animals/zebra/zebra.pal`. Lookups ignore case, like the game on Windows.
//! Sources added later override earlier ones.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use zip::ZipArchive;

pub mod layout;
pub use layout::{GameLayout, Layer};

#[derive(Debug, thiserror::Error)]
pub enum VfsError {
    #[error("resource not found: {0}")]
    NotFound(String),
    #[error("i/o error on {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("bad archive {path}: {source}")]
    Zip { path: PathBuf, source: zip::result::ZipError },
}

pub type Result<T> = std::result::Result<T, VfsError>;

enum Source {
    Zip { path: PathBuf, archive: Mutex<ZipArchive<File>> },
    Dir(PathBuf),
}

#[derive(Clone, Debug)]
struct Entry {
    source: usize,
    loc: Loc,
}

#[derive(Clone, Debug)]
enum Loc {
    ZipIndex(usize),
    /// Path relative to the source directory.
    Rel(String),
}

/// Where a resource came from.
#[derive(Clone, Debug)]
pub struct Origin {
    pub container: PathBuf,
    pub name: String,
    pub in_archive: bool,
}

#[derive(Default)]
pub struct Vfs {
    sources: Vec<Source>,
    index: HashMap<String, Entry>,
    /// Every source that had each overridden path, oldest first.
    shadowed: HashMap<String, Vec<usize>>,
}

/// Normalise a resource path: `\` → `/`, strip `./` and leading `/`, lowercase.
pub fn normalize(path: &str) -> String {
    let mut s = path.replace('\\', "/");
    loop {
        if let Some(r) = s.strip_prefix("./") {
            s = r.to_string();
        } else if let Some(r) = s.strip_prefix('/') {
            s = r.to_string();
        } else {
            break;
        }
    }
    s.to_ascii_lowercase()
}

impl Vfs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a VFS from a detected game layout.
    pub fn from_layout(layout: &GameLayout) -> Result<Self> {
        let mut vfs = Vfs::new();
        for layer in &layout.layers {
            vfs.add_layer(layer)?;
        }
        Ok(vfs)
    }

    /// Mount a layer: its archives in order (later wins), then loose files.
    pub fn add_layer(&mut self, layer: &Layer) -> Result<()> {
        for ztd in &layer.archives {
            self.add_ztd(ztd)?;
        }
        for (dir, prefix) in &layer.loose {
            self.add_dir_prefixed(dir, prefix)?;
        }
        Ok(())
    }

    pub fn add_ztd(&mut self, path: &Path) -> Result<()> {
        let file = File::open(path).map_err(|source| VfsError::Io { path: path.into(), source })?;
        let archive =
            ZipArchive::new(file).map_err(|source| VfsError::Zip { path: path.into(), source })?;
        let id = self.sources.len();
        let mut names = Vec::with_capacity(archive.len());
        for i in 0..archive.len() {
            if let Some(Ok(name)) = archive.name_for_index(i) {
                if !name.ends_with('/') {
                    names.push((normalize(&name), i));
                }
            }
        }
        self.sources.push(Source::Zip { path: path.into(), archive: Mutex::new(archive) });
        for (name, i) in names {
            self.insert(name, Entry { source: id, loc: Loc::ZipIndex(i) });
        }
        Ok(())
    }

    /// Mount loose files below `dir` (recursively, skipping `.ztd` files).
    pub fn add_dir(&mut self, dir: &Path) -> Result<()> {
        self.add_dir_prefixed(dir, "")
    }

    /// Mount loose files below `dir` as `<prefix>/<relative path>`.
    pub fn add_dir_prefixed(&mut self, dir: &Path, prefix: &str) -> Result<()> {
        let id = self.sources.len();
        self.sources.push(Source::Dir(dir.into()));
        let mut stack = vec![(dir.to_path_buf(), String::new())];
        while let Some((d, rel)) = stack.pop() {
            let rd = std::fs::read_dir(&d).map_err(|source| VfsError::Io { path: d.clone(), source })?;
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let r = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
                let Ok(ft) = e.file_type() else { continue };
                if ft.is_dir() {
                    stack.push((e.path(), r));
                } else if !name.to_ascii_lowercase().ends_with(".ztd") {
                    let key = if prefix.is_empty() {
                        normalize(&r)
                    } else {
                        normalize(&format!("{prefix}/{r}"))
                    };
                    self.insert(key, Entry { source: id, loc: Loc::Rel(r) });
                }
            }
        }
        Ok(())
    }

    fn insert(&mut self, key: String, entry: Entry) {
        let s = entry.source;
        if let Some(old) = self.index.insert(key.clone(), entry) {
            let v = self.shadowed.entry(key).or_default();
            if v.is_empty() {
                v.push(old.source);
            }
            v.push(s);
        }
    }

    pub fn exists(&self, path: &str) -> bool {
        self.index.contains_key(&normalize(path))
    }

    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        let entry = self
            .index
            .get(&normalize(path))
            .ok_or_else(|| VfsError::NotFound(path.to_string()))?;
        self.read_entry(entry)
    }

    /// Read a resource as Latin-1 text (the game's configs are ASCII/Windows-1252).
    pub fn read_text(&self, path: &str) -> Result<String> {
        Ok(latin1(&self.read(path)?))
    }

    fn read_entry(&self, entry: &Entry) -> Result<Vec<u8>> {
        match (&self.sources[entry.source], &entry.loc) {
            (Source::Zip { path, archive }, Loc::ZipIndex(i)) => {
                let mut ar = archive.lock().unwrap_or_else(|e| e.into_inner());
                let mut f =
                    ar.by_index(*i).map_err(|source| VfsError::Zip { path: path.clone(), source })?;
                let mut buf = Vec::with_capacity(f.size() as usize);
                f.read_to_end(&mut buf)
                    .map_err(|source| VfsError::Io { path: path.clone(), source })?;
                Ok(buf)
            }
            (Source::Dir(dir), Loc::Rel(rel)) => {
                let p = dir.join(rel);
                std::fs::read(&p).map_err(|source| VfsError::Io { path: p, source })
            }
            _ => unreachable!("entry location does not match source kind"),
        }
    }

    pub fn origin(&self, path: &str) -> Option<Origin> {
        let e = self.index.get(&normalize(path))?;
        Some(match (&self.sources[e.source], &e.loc) {
            (Source::Zip { path, archive }, Loc::ZipIndex(i)) => {
                let ar = archive.lock().unwrap_or_else(|e| e.into_inner());
                Origin {
                    container: path.clone(),
                    name: ar.name_for_index(*i).and_then(|r| r.ok()).map(|n| n.into_owned()).unwrap_or_default(),
                    in_archive: true,
                }
            }
            (Source::Dir(dir), Loc::Rel(rel)) => {
                Origin { container: dir.clone(), name: rel.clone(), in_archive: false }
            }
            _ => unreachable!(),
        })
    }

    /// Containers (oldest first) that provided `path`, if it was overridden.
    pub fn override_chain(&self, path: &str) -> Vec<PathBuf> {
        self.shadowed
            .get(&normalize(path))
            .map(|v| v.iter().map(|&s| self.source_path(s).to_path_buf()).collect())
            .unwrap_or_default()
    }

    fn source_path(&self, s: usize) -> &Path {
        match &self.sources[s] {
            Source::Zip { path, .. } => path,
            Source::Dir(d) => d,
        }
    }

    /// All normalised resource paths, sorted.
    pub fn paths(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.index.keys().map(String::as_str).collect();
        v.sort_unstable();
        v
    }

    /// Paths starting with `prefix` (normalised), sorted.
    pub fn paths_with_prefix(&self, prefix: &str) -> Vec<&str> {
        let p = normalize(prefix);
        let mut v: Vec<&str> =
            self.index.keys().filter(|k| k.starts_with(&p)).map(String::as_str).collect();
        v.sort_unstable();
        v
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }
}

/// Decode bytes as Latin-1 (every byte maps to the same code point).
pub fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_paths() {
        assert_eq!(normalize(".\\Animals\\Zebra.PAL"), "animals/zebra.pal");
        assert_eq!(normalize("/ui/x.lyt"), "ui/x.lyt");
    }
}
