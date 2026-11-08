//! Developer tool for inspecting and verifying game data.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use zt_formats::{Ani, Ini, Palette, Sprite, sprite, strings::Strings};
use zt_vfs::{GameLayout, Vfs};

#[derive(Parser)]
#[command(about = "Inspect and verify game data for rust-zt")]
struct Cli {
    /// Game folder (disc copy or install). Defaults to $RUST_ZT_DATA.
    #[arg(long, global = true, env = "RUST_ZT_DATA")]
    data: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show detected layers, archives and DLLs.
    Layers,
    /// List resource paths (optionally under a prefix).
    Ls { prefix: Option<String> },
    /// Write a resource to stdout.
    Cat { path: String },
    /// Show which container provides a resource and what it overrides.
    Origin { path: String },
    /// Print strings (all, or the given IDs).
    Strings { ids: Vec<u32> },
    /// Print a parsed INI-style file.
    Ini { path: String },
    /// Show sprite / animation info.
    Info { path: String },
    /// Export every frame of a sprite (or every direction of an .ani) to PNG.
    Png { path: String, out: PathBuf },
    /// Parse every file of the known formats and check sprite round trips.
    Check,
    /// Parse .zoo maps (all under maps/ if none given, or files on disk) and summarise them.
    Zoo {
        paths: Vec<String>,
        /// List every object record.
        #[arg(short, long)]
        verbose: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let data = cli.data.context("no game folder: pass --data or set RUST_ZT_DATA")?;
    let layout = GameLayout::detect(&data)
        .with_context(|| format!("no game data found under {}", data.display()))?;

    if let Cmd::Layers = cli.cmd {
        println!("{:?} layout at {}", layout.kind, layout.root.display());
        for l in &layout.layers {
            println!("[{}]", l.name);
            for a in &l.archives {
                println!("  ztd   {}", rel(&layout.root, a));
            }
            for (d, p) in &l.loose {
                println!("  dir   {} -> {p}/", rel(&layout.root, d));
            }
            for d in &l.dlls {
                println!("  dll   {}", rel(&layout.root, d));
            }
        }
        return Ok(());
    }

    let vfs = Vfs::from_layout(&layout)?;
    match cli.cmd {
        Cmd::Layers => unreachable!(),
        Cmd::Ls { prefix } => {
            for p in vfs.paths_with_prefix(prefix.as_deref().unwrap_or("")) {
                println!("{p}");
            }
        }
        Cmd::Cat { path } => {
            use std::io::Write;
            std::io::stdout().write_all(&vfs.read(&path)?)?;
        }
        Cmd::Origin { path } => {
            let o = vfs.origin(&path).context("not found")?;
            println!("{} : {}", rel(&layout.root, &o.container), o.name);
            for c in vfs.override_chain(&path) {
                println!("  chain: {}", rel(&layout.root, &c));
            }
        }
        Cmd::Strings { ids } => {
            let s = load_strings(&layout)?;
            if ids.is_empty() {
                for (id, t) in &s.map {
                    println!("{id}\t{}", t.replace('\n', "\\n").replace('\r', ""));
                }
            } else {
                for id in ids {
                    println!("{id}\t{}", s.get(id).unwrap_or("<missing>"));
                }
            }
        }
        Cmd::Ini { path } => {
            let ini = Ini::parse(&vfs.read_text(&path)?);
            for s in &ini.sections {
                println!("[{}]", s.name);
                for e in &s.entries {
                    match &e.value {
                        Some(v) => println!("  {} = {v}", e.key),
                        None => println!("  {}", e.key),
                    }
                }
            }
        }
        Cmd::Info { path } => info(&vfs, &path)?,
        Cmd::Png { path, out } => {
            if path.to_ascii_lowercase().ends_with(".ani") {
                let ani = Ani::parse(&vfs.read_text(&path)?);
                for (i, a) in ani.animations.iter().enumerate() {
                    export_png(&vfs, &ani.sprite_path(i).unwrap(), &out.join(a))?;
                }
            } else {
                export_png(&vfs, &path, &out)?;
            }
        }
        Cmd::Check => check(&layout, &vfs)?,
        Cmd::Zoo { paths, verbose } => zoo_summary(&vfs, paths, verbose)?,
    }
    Ok(())
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).display().to_string()
}

fn load_strings(layout: &GameLayout) -> Result<Strings> {
    let mut s = Strings::default();
    for dll in layout.string_dlls() {
        let bytes = std::fs::read(dll)?;
        s.add_dll(&bytes).with_context(|| format!("reading {}", dll.display()))?;
    }
    Ok(s)
}

fn info(vfs: &Vfs, path: &str) -> Result<()> {
    let bytes = vfs.read(path)?;
    if path.to_ascii_lowercase().ends_with(".ani") {
        let ani = Ani::parse(&zt_vfs::latin1(&bytes));
        println!("{ani:#?}");
        for i in 0..ani.animations.len() {
            let sp = ani.sprite_path(i).unwrap();
            match vfs.read(&sp).map_err(anyhow::Error::from).and_then(|b| Ok(Sprite::parse(&b)?)) {
                Ok(s) => println!(
                    "  {sp}: {} frames, {} ms, bounds {:?}",
                    s.frames.len(),
                    s.frame_ms,
                    s.bounds()
                ),
                Err(e) => println!("  {sp}: {e}"),
            }
        }
    } else {
        let s = Sprite::parse(&bytes)?;
        println!(
            "palette {}  {} ms/frame  fatz={}  background={}",
            s.palette_path,
            s.frame_ms,
            s.fatz,
            s.background.is_some()
        );
        for (i, f) in s.frames.iter().enumerate() {
            println!(
                "  frame {i}: {}x{} anchor ({},{}) flags {:#x}{}",
                f.width, f.height, f.anchor_x, f.anchor_y, f.flags, if f.mask { " MASK" } else { "" }
            );
        }
    }
    Ok(())
}

fn export_png(vfs: &Vfs, path: &str, out: &Path) -> Result<()> {
    std::fs::create_dir_all(out)?;
    let s = if path.to_ascii_lowercase().ends_with(".lle") {
        let frame = sprite::Frame::parse_body(&vfs.read(path)?)?;
        let pal = format!("{}.pal", &path[..path.len() - 4]);
        Sprite { palette_path: pal, frame_ms: 0, frames: vec![frame], fatz: false, background: None }
    } else {
        Sprite::parse(&vfs.read(path)?)?
    };
    let pal = Palette::parse(&vfs.read(&s.palette_path).with_context(|| s.palette_path.clone())?)?
        .rgba256();
    let frames: Vec<_> = s.frames.iter().chain(s.background.as_ref()).collect();
    for (i, f) in frames.iter().enumerate() {
        let w = f.width as usize;
        let mut img = image::RgbaImage::new(f.width.max(1) as u32, f.height.max(1) as u32);
        for (j, &p) in f.decode().iter().enumerate() {
            let c = match p {
                sprite::TRANSPARENT => continue,
                sprite::MASKED => [0, 0, 0, 128],
                p => pal[p as usize],
            };
            img.put_pixel((j % w) as u32, (j / w) as u32, image::Rgba(c));
        }
        let name =
            if i == s.frames.len() { "background.png".to_string() } else { format!("{i:03}.png") };
        img.save(out.join(name))?;
    }
    println!("{path}: {} frames -> {}", frames.len(), out.display());
    Ok(())
}

#[derive(Default)]
struct Tally {
    checked: usize,
    failed: usize,
}

fn check(layout: &GameLayout, vfs: &Vfs) -> Result<()> {
    let mut stats = std::collections::BTreeMap::<&str, Tally>::new();
    for path in vfs.paths() {
        let file = path.rsplit('/').next().unwrap_or(path);
        let ext = file.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
        let kind = match ext {
            "ai" | "cfg" | "scn" | "lyt" | "uca" | "ucb" | "ucs" | "ini" => "ini",
            "ani" => "ani",
            "pal" => "pal",
            "" => "sprite",
            _ => continue,
        };
        let bytes = match vfs.read(path) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("FAIL {kind} {path}: {e}");
                stats.entry(kind).or_default().failed += 1;
                continue;
            }
        };
        if kind == "sprite" && !sprite::is_sprite(&bytes) {
            continue;
        }
        let result: std::result::Result<(), String> = match kind {
            "ini" => {
                let ini = Ini::parse(&zt_vfs::latin1(&bytes));
                let text = zt_vfs::latin1(&bytes);
                let has_content = text.lines().map(str::trim).any(|l| {
                    !l.is_empty() && !l.starts_with(';') && !l.starts_with('#') && l != "\u{1a}"
                });
                if ini.sections.is_empty() && has_content {
                    Err("no sections".into())
                } else {
                    Ok(())
                }
            }
            "ani" => {
                let a = Ani::parse(&zt_vfs::latin1(&bytes));
                let missing: Vec<_> = (0..a.animations.len())
                    .map(|i| a.sprite_path(i).unwrap())
                    .filter(|p| !vfs.exists(p))
                    .collect();
                if missing.is_empty() { Ok(()) } else { Err(format!("missing {missing:?}")) }
            }
            "pal" => match Palette::parse(&bytes) {
                Ok(p) if p.to_bytes() == bytes => Ok(()),
                Ok(_) => Err("round trip differs".into()),
                Err(e) => Err(e.to_string()),
            },
            _ => check_sprite(vfs, &bytes),
        };
        let t = stats.entry(kind).or_default();
        t.checked += 1;
        if let Err(e) = result {
            t.failed += 1;
            eprintln!("FAIL {kind} {path}: {e}");
        }
    }
    let strings = load_strings(layout)?;
    println!("resources: {}", vfs.len());
    for (k, t) in &stats {
        println!("{k:8} {:7} checked, {} failed", t.checked, t.failed);
    }
    println!("strings: {}", strings.map.len());
    if stats.values().any(|t| t.failed > 0) {
        bail!("some checks failed");
    }
    Ok(())
}

fn check_sprite(vfs: &Vfs, bytes: &[u8]) -> std::result::Result<(), String> {
    let s = Sprite::parse(bytes).map_err(|e| e.to_string())?;
    if s.to_bytes() != bytes {
        return Err("round trip differs".into());
    }
    for f in s.frames.iter().chain(s.background.as_ref()) {
        let re = sprite::Frame::encode_rows(f.width, f.height, f.mask, &f.decode()).map_err(|e| e.to_string())?;
        if re != f.rows {
            return Err("re-encoded rows differ".into());
        }
    }
    if !vfs.exists(&s.palette_path) {
        return Err(format!("missing palette {}", s.palette_path));
    }
    Ok(())
}

fn zoo_summary(vfs: &Vfs, paths: Vec<String>, verbose: bool) -> Result<()> {
    use zt_formats::zoo::ZooMap;
    let paths: Vec<String> = if paths.is_empty() {
        vfs.paths().into_iter().filter(|p| p.ends_with(".zoo")).map(String::from).collect()
    } else {
        paths
    };
    let mut failed = 0;
    for p in &paths {
        // Files on disk (e.g. user saves) or resources.
        let bytes = if std::path::Path::new(p).is_file() { std::fs::read(p)? } else { vfs.read(p)? };
        match ZooMap::parse(&bytes) {
            Ok(m) => {
                let (lo, hi) = m.tiles.iter().fold((i32::MAX, i32::MIN), |(lo, hi), t| {
                    (lo.min(t.height), hi.max(t.height))
                });
                let mut terr: Vec<u8> = m.tiles.iter().map(|t| t.terrain).collect();
                terr.sort_unstable();
                terr.dedup();
                let placed = m.objects.iter().filter(|o| o.placement.is_some()).count();
                let mut groups = std::collections::BTreeMap::<&str, usize>::new();
                for o in &m.objects {
                    *groups.entry(o.group.as_str()).or_default() += 1;
                }
                println!(
                    "{p}: v{} {}x{} exhibits {} tiles@{:#x} height {lo}..{hi} terrain {terr:?} \
                     objects {}/{} found ({placed} placed) {groups:?}",
                    m.version as char, m.width, m.height, m.exhibits.len(), m.tiles_offset,
                    m.objects.len(), m.object_count
                );
                if verbose {
                    for e in &m.exhibits {
                        print!("  exhibit {:?} at ({}, {})", e.name, e.x, e.y);
                        if let Some((gx, gy, gf)) = e.gate {
                            print!(" gate ({gx}, {gy}) f{gf}");
                        }
                        if let Some(t) = e.tank {
                            print!(
                                " {}tank height {} water {} salinity {} purity {}{}",
                                if t.show { "show " } else { "" },
                                t.height, t.water_level, t.salinity, t.purity,
                                if t.filling { "" } else { " (emptying)" }
                            );
                        }
                        println!();
                    }
                    for o in &m.objects {
                        match &o.placement {
                            Some(pl) => println!(
                                "  {:#08x} {:9} {:12} {:2} ({:7.2},{:7.2},{:4}) f{} id {} {:?}",
                                o.offset, o.group, o.type_name, o.subtype.as_deref().unwrap_or(""),
                                pl.x as f32 / 64.0, pl.y as f32 / 64.0, pl.z, pl.facing, pl.id, pl.name
                            ),
                            None => println!("  {:#08x} {:9} {:12} {:2} (no placement)", o.offset, o.group, o.type_name, o.subtype.as_deref().unwrap_or("")),
                        }
                    }
                }
            }
            Err(e) => {
                failed += 1;
                println!("{p}: FAIL {e}");
            }
        }
    }
    println!("{} maps, {failed} failed", paths.len());
    Ok(())
}
