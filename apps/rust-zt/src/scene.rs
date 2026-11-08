//! The loaded map and everything needed to draw it.

use anyhow::{Context, Result};
use glam::Vec2;
use zt_formats::zoo::ZooMap;
use zt_render::sprites::SpriteRenderer;
use zt_render::terrain::{Lighting, TerrainRenderer};
use zt_render::{Camera, Globals, Gpu, wgpu};
use zt_vfs::Vfs;

use crate::assets::{self, TerrainTypes};
use crate::objects::{self, ObjectSet};

pub struct Scene {
    pub terrain_types: TerrainTypes,
    pub lighting: Lighting,
    pub globals: Globals,
    pub terrain: TerrainRenderer,
    pub sprites: SpriteRenderer,
    pub objects: Option<ObjectSet>,
    pub map: Option<ZooMap>,
    pub map_name: String,
    pub camera: Camera,
}

impl Scene {
    pub fn new(gpu: &Gpu, vfs: &Vfs) -> Result<Scene> {
        let terrain_types = TerrainTypes::load(vfs)?;
        let textures = terrain_types.textures(vfs)?;
        let globals = Globals::new(&gpu.device);
        let terrain = TerrainRenderer::new(&gpu.device, &gpu.queue, gpu.config.format, &globals, &textures);
        let sprites = SpriteRenderer::new(&gpu.device, gpu.config.format, &globals);
        let viewport = Vec2::new(gpu.config.width as f32, gpu.config.height as f32);
        Ok(Scene {
            terrain_types,
            lighting: assets::lighting(vfs),
            globals,
            terrain,
            sprites,
            objects: None,
            map: None,
            map_name: String::new(),
            camera: Camera::new(1, 1, viewport),
        })
    }

    pub fn load_map(&mut self, gpu: &Gpu, vfs: &Vfs, path: &str) -> Result<()> {
        // A file on disk (e.g. a saved game) or a resource path.
        let bytes = if std::path::Path::new(path).is_file() { std::fs::read(path)? } else { vfs.read(path)? };
        let map = ZooMap::parse(&bytes).with_context(|| format!("parsing {path}"))?;
        let types = &self.terrain_types;
        let cliff = types.cliff_layer();
        self.terrain.set_map(&gpu.device, &gpu.queue, &map, &self.lighting, &|t| types.blends(t), cliff);
        let (zoom, rotation, viewport) = (self.camera.zoom, self.camera.rotation, self.camera.viewport);
        self.camera = Camera::new(map.width, map.height, viewport);
        self.camera.zoom = zoom;
        self.camera.turn(rotation as i32);
        let t = std::time::Instant::now();
        let set = objects::load(vfs, &map);
        self.sprites.set_atlas(&gpu.device, &gpu.queue, &set.atlas);
        log::info!(
            "loaded {path}: {}x{} version {}, {} of {} objects drawable, {} atlas pages ({:.0?})",
            map.width,
            map.height,
            map.version as char,
            set.drawables.len(),
            map.object_count,
            set.atlas.pages.len(),
            t.elapsed()
        );
        if !set.missing.is_empty() {
            log::warn!("no graphics for: {}", set.missing.join(", "));
        }
        self.objects = Some(set);
        self.map = Some(map);
        self.map_name = path.to_string();
        Ok(())
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.camera.viewport = Vec2::new(width as f32, height as f32);
    }

    pub fn render(
        &mut self,
        gpu: &Gpu,
        enc: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        time: f32,
    ) {
        self.globals.update(&gpu.queue, &self.camera.uniform(time));
        if let Some(set) = &self.objects {
            let m = self.camera.map_size;
            let inst = objects::instances(set, self.camera.rotation, m.x, m.y, (time * 1000.0) as u32);
            self.sprites.set_instances(&gpu.device, &gpu.queue, &inst);
        }
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        self.terrain.draw(&mut pass, &self.globals);
        self.sprites.draw(&mut pass, &self.globals);
    }
}
