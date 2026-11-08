//! Sprite atlas and instanced sprite drawing.

use bytemuck::{Pod, Zeroable};

use crate::globals::{Globals, shader_source};

pub const PAGE_SIZE: u32 = 2048;

/// Location of an image in the atlas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtlasRect {
    pub layer: u32,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl AtlasRect {
    /// Normalised `[u0, v0, u1, v1]`.
    pub fn uv(&self) -> [f32; 4] {
        let s = PAGE_SIZE as f32;
        [self.x as f32 / s, self.y as f32 / s, (self.x + self.w) as f32 / s, (self.y + self.h) as f32 / s]
    }
}

/// CPU-side shelf packer producing RGBA pages.
pub struct AtlasBuilder {
    pub pages: Vec<Vec<u8>>,
    // Per page: cursor x, shelf top, shelf height.
    shelves: Vec<(u32, u32, u32)>,
}

impl Default for AtlasBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl AtlasBuilder {
    pub fn new() -> Self {
        AtlasBuilder { pages: Vec::new(), shelves: Vec::new() }
    }

    /// Add an RGBA image (`w * h * 4` bytes). Images larger than a page are clipped.
    pub fn add(&mut self, w: u32, h: u32, rgba: &[u8]) -> AtlasRect {
        let (w, h) = (w.clamp(1, PAGE_SIZE - 2), h.clamp(1, PAGE_SIZE - 2));
        let (pw, ph) = (w + 1, h + 1); // 1px gutter
        let mut slot = None;
        for (i, (x, top, sh)) in self.shelves.iter_mut().enumerate() {
            // The current shelf is the last one on its page, so it may grow downward.
            if *x + pw <= PAGE_SIZE && *top + (*sh).max(ph) <= PAGE_SIZE {
                slot = Some((i, *x, *top));
                *x += pw;
                *sh = (*sh).max(ph);
                break;
            }
            if *top + *sh + ph <= PAGE_SIZE {
                *top += *sh;
                *sh = ph;
                *x = pw;
                slot = Some((i, 0, *top));
                break;
            }
        }
        let (layer, x, y) = match slot {
            Some(s) => s,
            None => {
                self.pages.push(vec![0; (PAGE_SIZE * PAGE_SIZE * 4) as usize]);
                self.shelves.push((pw, 0, ph));
                (self.pages.len() - 1, 0, 0)
            }
        };
        let page = &mut self.pages[layer];
        for row in 0..h {
            let src = &rgba[(row * w * 4) as usize..((row + 1) * w * 4) as usize];
            let dst = (((y + row) * PAGE_SIZE + x) * 4) as usize;
            page[dst..dst + src.len()].copy_from_slice(src);
        }
        AtlasRect { layer: layer as u32, x, y, w, h }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct SpriteInstance {
    /// Grid u, grid v, elevation (steps).
    pub world: [f32; 3],
    pub layer: u32,
    /// Top-left corner relative to the anchor (pixels).
    pub offset: [f32; 2],
    pub size: [f32; 2],
    pub uv: [f32; 4],
    /// Added to the depth key (in tile units) so sprites win against the ground they stand on.
    pub depth_bias: f32,
    /// [`FLAG_FLIP`], [`FLAG_TRANSLUCENT`], one of the `FLAG_PLANE_*`.
    pub flags: u32,
    /// Blend toward this colour: rgb (0–1) and amount (0 = none). Used for
    /// the underwater parts of swimming animals.
    pub tint: [f32; 4],
}

/// Mirror the sprite around its anchor (west-facing views reuse east sprites).
pub const FLAG_FLIP: u32 = 1;
/// Draw at 50% opacity (tank water and glass).
pub const FLAG_TRANSLUCENT: u32 = 2;
/// Depth from a vertical plane through the anchor with constant grid u
/// (a wall on an edge running along v), instead of one depth per sprite.
pub const FLAG_PLANE_U: u32 = 4;
/// Depth from a vertical plane with constant grid v.
pub const FLAG_PLANE_V: u32 = 8;
/// Depth from the horizontal plane at the anchor's elevation.
pub const FLAG_PLANE_H: u32 = 12;

pub struct SpriteRenderer {
    pipeline: wgpu::RenderPipeline,
    tex_bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    atlas_bg: Option<wgpu::BindGroup>,
    instances: Option<wgpu::Buffer>,
    capacity: usize,
    count: u32,
}

impl SpriteRenderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, globals: &Globals) -> SpriteRenderer {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sprites"),
            source: wgpu::ShaderSource::Wgsl(shader_source(include_str!("sprite.wgsl")).into()),
        });
        let tex_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sprite atlas"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sprites"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sprites"),
            bind_group_layouts: &[Some(&globals.layout), Some(&tex_bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sprites"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<SpriteInstance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3, 1 => Uint32, 2 => Float32x2, 3 => Float32x2,
                        4 => Float32x4, 5 => Float32, 6 => Uint32, 7 => Float32x4
                    ],
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            // Test against the terrain (hills hide sprites behind them) but
            // don't write: sprites are painter-sorted among themselves, which
            // also orders the translucent ones.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: crate::DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        SpriteRenderer { pipeline, tex_bgl, sampler, atlas_bg: None, instances: None, capacity: 0, count: 0 }
    }

    pub fn set_atlas(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, atlas: &AtlasBuilder) {
        let layers = atlas.pages.len().max(1) as u32;
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sprite atlas"),
            size: wgpu::Extent3d { width: PAGE_SIZE, height: PAGE_SIZE, depth_or_array_layers: layers },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (i, page) in atlas.pages.iter().enumerate() {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: 0, z: i as u32 },
                    aspect: wgpu::TextureAspect::All,
                },
                page,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * PAGE_SIZE), rows_per_image: Some(PAGE_SIZE) },
                wgpu::Extent3d { width: PAGE_SIZE, height: PAGE_SIZE, depth_or_array_layers: 1 },
            );
        }
        let view = tex.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        self.atlas_bg = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sprite atlas"),
            layout: &self.tex_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        }));
    }

    /// Upload instances, already sorted back to front.
    pub fn set_instances(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, inst: &[SpriteInstance]) {
        if inst.len() > self.capacity || self.instances.is_none() {
            self.capacity = inst.len().next_power_of_two().max(256);
            self.instances = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("sprite instances"),
                size: (self.capacity * std::mem::size_of::<SpriteInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        if !inst.is_empty() {
            queue.write_buffer(self.instances.as_ref().unwrap(), 0, bytemuck::cast_slice(inst));
        }
        self.count = inst.len() as u32;
    }

    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, globals: &'a Globals) {
        let (Some(bg), Some(buf)) = (&self.atlas_bg, &self.instances) else { return };
        if self.count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &globals.bind_group, &[]);
        pass.set_bind_group(1, bg, &[]);
        pass.set_vertex_buffer(0, buf.slice(..));
        pass.draw(0..6, 0..self.count);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atlas_packs_without_overlap() {
        let mut a = AtlasBuilder::new();
        let mut rects = Vec::new();
        for i in 0..400u32 {
            let (w, h) = (10 + i % 90, 5 + (i * 7) % 120);
            rects.push(a.add(w, h, &vec![255; (w * h * 4) as usize]));
        }
        for (i, r) in rects.iter().enumerate() {
            assert!(r.x + r.w <= PAGE_SIZE && r.y + r.h <= PAGE_SIZE);
            for s in &rects[..i] {
                let overlap = r.layer == s.layer
                    && r.x < s.x + s.w
                    && s.x < r.x + r.w
                    && r.y < s.y + s.h
                    && s.y < r.y + r.h;
                assert!(!overlap, "{r:?} overlaps {s:?}");
            }
        }
    }
}
