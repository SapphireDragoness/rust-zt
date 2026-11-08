//! rust-zt: map viewer (milestone 1).

mod assets;
mod objects;
mod scene;
mod viewer;
mod water;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use zt_vfs::{GameLayout, Vfs};

struct Args {
    data: PathBuf,
    map: Option<String>,
    screenshot: Option<PathBuf>,
    size: (u32, u32),
    rotation: u32,
    zoom: f32,
    center: Option<(f32, f32)>,
}

const USAGE: &str = "usage: rust-zt [--data DIR] [MAP] [--screenshot OUT.png [--size WxH] [--rotation 0-3] [--zoom Z] [--center U,V]]

  DIR defaults to $RUST_ZT_DATA. MAP is a map name such as `nile`, `maps/nile.zoo`,
  or a path to a .zoo file (e.g. a saved game).

Controls: arrows/WASD scroll, right/middle drag pan, wheel zoom, Q/E rotate,
PageUp/PageDown previous/next map, F11 fullscreen, Esc quit.";

fn parse_args() -> Result<Args> {
    let mut args = std::env::args().skip(1);
    let mut data = std::env::var_os("RUST_ZT_DATA").map(PathBuf::from);
    let mut out = Args { data: PathBuf::new(), map: None, screenshot: None, size: (1280, 800), rotation: 0, zoom: 1.0, center: None };
    while let Some(a) = args.next() {
        let mut val = || args.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--data" => data = Some(val()?.into()),
            "--screenshot" => out.screenshot = Some(val()?.into()),
            "--size" => {
                let v = val()?;
                let (w, h) = v.split_once('x').context("--size expects WxH")?;
                out.size = (w.parse()?, h.parse()?);
            }
            "--rotation" => out.rotation = val()?.parse()?,
            "--zoom" => out.zoom = val()?.parse()?,
            "--center" => {
                let v = val()?;
                let (u, w) = v.split_once(',').context("--center expects U,V (tiles)")?;
                out.center = Some((u.parse()?, w.parse()?));
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            s if s.starts_with('-') => bail!("unknown option {s}\n\n{USAGE}"),
            s => out.map = Some(s.to_string()),
        }
    }
    out.data = data.context("no game folder: pass --data or set RUST_ZT_DATA")?;
    Ok(out)
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn,naga=warn"),
    )
    .init();
    let args = parse_args()?;
    let layout = GameLayout::detect(&args.data)
        .with_context(|| format!("no game data found under {}", args.data.display()))?;
    let vfs = Vfs::from_layout(&layout)?;
    log::info!("{:?} layout, {} resources", layout.kind, vfs.len());

    let maps = assets::map_list(&vfs);
    anyhow::ensure!(!maps.is_empty(), "no maps found");
    let map = match &args.map {
        Some(m) if std::path::Path::new(m).is_file() => m.clone(),
        Some(m) => assets::resolve_map(&vfs, m).with_context(|| format!("map {m} not found"))?,
        None => maps.iter().find(|m| m.ends_with("/nile.zoo")).unwrap_or(&maps[0]).clone(),
    };

    if let Some(out) = &args.screenshot {
        return screenshot(&vfs, &map, out, &args);
    }
    viewer::run(vfs, maps, map)
}

fn screenshot(vfs: &Vfs, map: &str, out: &std::path::Path, args: &Args) -> Result<()> {
    let (w, h) = args.size;
    let gpu = pollster::block_on(zt_render::Gpu::headless(w, h, zt_render::wgpu::TextureFormat::Rgba8Unorm))?;
    let mut scene = scene::Scene::new(&gpu, vfs)?;
    scene.resize(w, h);
    scene.camera.zoom = args.zoom;
    scene.load_map(&gpu, vfs, map)?;
    scene.camera.turn(args.rotation as i32);
    if let Some((u, v)) = args.center {
        scene.camera.center = scene.camera.project(glam::Vec2::new(u, v), 0.0);
    }
    let pixels = gpu.render_to_rgba(|enc, color, depth| scene.render(&gpu, enc, color, depth, 0.0));
    image::RgbaImage::from_raw(w, h, pixels).context("bad image size")?.save(out)?;
    println!("{map} -> {}", out.display());
    Ok(())
}
