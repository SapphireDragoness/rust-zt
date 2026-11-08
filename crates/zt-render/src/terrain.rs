//! Terrain mesh generation and drawing.

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use wgpu::util::DeviceExt;
use zt_formats::zoo::ZooMap;

use crate::globals::{Globals, shader_source};

/// Horizontal size of a tile in world units, relative to one elevation step.
/// A 2:1 dimetric view puts the camera ~30° above the ground: a tile side is
/// 64/√2 ≈ 45.25 px of ground, an elevation step 16/cos 30° ≈ 18.5 px.
pub const ELEV_PER_TILE: f32 = 18.475 / 45.255;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub shade: f32,
    pub material: u32,
    /// Walls: texture layer the wall shows (the cliff layer).
    pub layer: u32,
    /// Walls: grid-space direction the face points to.
    pub normal: [f32; 2],
}

pub const MATERIAL_SURFACE: u32 = 0;
pub const MATERIAL_WALL: u32 = 1;

/// Directional lights (from `terrain/tilevar.cfg`). Directions use Direct3D
/// conventions: +Y is up; X and Z are taken as grid u and v.
#[derive(Clone, Debug)]
pub struct Lighting {
    pub ambient: f32,
    pub lights: Vec<(Vec3, f32)>,
}

impl Default for Lighting {
    fn default() -> Self {
        Lighting {
            ambient: 0.0,
            lights: vec![(Vec3::new(-1.0, -10.0, 0.0), 0.7), (Vec3::new(1.0, -1.0, 0.0), 0.4)],
        }
    }
}

impl Lighting {
    /// Brightness of a surface with world-space normal `n` (Y up).
    pub fn shade(&self, n: Vec3) -> f32 {
        let n = n.normalize_or_zero();
        self.ambient + self.lights.iter().map(|(d, c)| n.dot(-d.normalize()).max(0.0) * c).sum::<f32>()
    }
}

/// Build the terrain triangle list: two triangles per tile, plus vertical
/// walls at height discontinuities (water banks, tank pits). The map's outer
/// sides get no walls: the original shows a black void there. Walls show the `cliff_layer` texture: in the original, drops
/// show a dark earth texture that matches `terrain/aphalt.tga` (at ~0.72
/// brightness on a left-facing drop), not the terrain of either side.
///
/// Surfaces are not shaded by slope: the game's pre-rendered terrain tiles
/// (`ztatb`) show slopes at the same brightness as flat ground, so every
/// surface gets the flat-ground value from `tilevar.cfg`'s lights.
pub fn build_mesh(map: &ZooMap, lighting: &Lighting, cliff_layer: u32) -> Vec<Vertex> {
    let (w, h) = (map.width as i32, map.height as i32);
    let mut out = Vec::with_capacity((w * h * 6) as usize);
    let corners = |x: i32, y: i32| map.tile(x as u32, y as u32).map(|t| t.corners());
    let flat = lighting.shade(Vec3::Y);

    let surface = |out: &mut Vec<Vertex>, ps: [[f32; 3]; 3]| {
        for p in ps {
            out.push(Vertex { pos: p, shade: flat, material: MATERIAL_SURFACE, layer: 0, normal: [0.0, 0.0] });
        }
    };
    let wall = |out: &mut Vec<Vertex>, q: [[f32; 3]; 4], layer: u32, normal: [f32; 2]| {
        for p in [q[0], q[1], q[2], q[0], q[2], q[3]] {
            out.push(Vertex { pos: p, shade: flat, material: MATERIAL_WALL, layer, normal });
        }
    };

    for y in 0..h {
        for x in 0..w {
            let [nw, ne, se, sw] = corners(x, y).unwrap();
            let (fx, fy) = (x as f32, y as f32);
            let p_nw = [fx, fy, nw as f32];
            let p_ne = [fx + 1.0, fy, ne as f32];
            let p_se = [fx + 1.0, fy + 1.0, se as f32];
            let p_sw = [fx, fy + 1.0, sw as f32];
            // Fold along the diagonal whose ends are level, if any.
            if ne == sw && nw != se {
                surface(&mut out, [p_nw, p_ne, p_sw]);
                surface(&mut out, [p_ne, p_se, p_sw]);
            } else {
                surface(&mut out, [p_nw, p_ne, p_se]);
                surface(&mut out, [p_nw, p_se, p_sw]);
            }

            // Wall to the +u neighbour (shared edge NE–SE).
            if let Some([r_nw, _, _, r_sw]) = corners(x + 1, y)
                && (r_nw != ne || r_sw != se)
            {
                let e = fx + 1.0;
                let mine_higher = ne + se >= r_nw + r_sw;
                wall(
                    &mut out,
                    [[e, fy, ne as f32], [e, fy + 1.0, se as f32], [e, fy + 1.0, r_sw as f32], [e, fy, r_nw as f32]],
                    cliff_layer,
                    if mine_higher { [1.0, 0.0] } else { [-1.0, 0.0] },
                );
            }
            // Wall to the +v neighbour (shared edge SW–SE).
            if let Some([d_nw, d_ne, _, _]) = corners(x, y + 1)
                && (d_nw != sw || d_ne != se)
            {
                let e = fy + 1.0;
                let mine_higher = sw + se >= d_nw + d_ne;
                wall(
                    &mut out,
                    [[fx, e, sw as f32], [fx + 1.0, e, se as f32], [fx + 1.0, e, d_ne as f32], [fx, e, d_nw as f32]],
                    cliff_layer,
                    if mine_higher { [0.0, 1.0] } else { [0.0, -1.0] },
                );
            }
        }
    }
    out
}

/// Per-tile `[layer, blends]` texels for the fragment shader.
pub fn tile_texels(map: &ZooMap, blends: &dyn Fn(u8) -> bool) -> Vec<u8> {
    map.tiles.iter().flat_map(|t| [t.terrain, blends(t.terrain) as u8]).collect()
}

pub struct TerrainTextures {
    /// Square RGBA8 images, one per terrain type id (index = type).
    pub layers: Vec<Vec<u8>>,
    pub size: u32,
}

pub struct TerrainRenderer {
    pipeline: wgpu::RenderPipeline,
    tex_bgl: wgpu::BindGroupLayout,
    tex_view: wgpu::TextureView,
    sampler: wgpu::Sampler,
    map_bg: Option<wgpu::BindGroup>,
    vertices: Option<(wgpu::Buffer, u32)>,
}

impl TerrainRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        globals: &Globals,
        textures: &TerrainTextures,
    ) -> TerrainRenderer {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terrain"),
            source: wgpu::ShaderSource::Wgsl(shader_source(include_str!("terrain.wgsl")).into()),
        });
        let tex_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain textures"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });

        let n = textures.layers.len().max(1) as u32;
        let size = wgpu::Extent3d { width: textures.size, height: textures.size, depth_or_array_layers: n };
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("terrain layers"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (i, layer) in textures.layers.iter().enumerate() {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: 0, z: i as u32 },
                    aspect: wgpu::TextureAspect::All,
                },
                layer,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * textures.size),
                    rows_per_image: Some(textures.size),
                },
                wgpu::Extent3d { width: textures.size, height: textures.size, depth_or_array_layers: 1 },
            );
        }
        let tex_view = tex.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("terrain"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain"),
            bind_group_layouts: &[Some(&globals.layout), Some(&tex_bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terrain"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32, 2 => Uint32, 3 => Uint32, 4 => Float32x2],
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: crate::DEPTH_FORMAT,
                depth_write_enabled: Some(true),
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
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        TerrainRenderer {
            pipeline,
            tex_bgl,
            tex_view,
            sampler,
            map_bg: None,
            vertices: None,
        }
    }

    /// Upload the terrain of `map`.
    pub fn set_map(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        map: &ZooMap,
        lighting: &Lighting,
        blends: &dyn Fn(u8) -> bool,
        cliff_layer: u32,
    ) {
        let verts = build_mesh(map, lighting, cliff_layer);
        let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("terrain mesh"),
            contents: bytemuck::cast_slice(&verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        self.vertices = Some((buf, verts.len() as u32));

        let size = wgpu::Extent3d { width: map.width, height: map.height, depth_or_array_layers: 1 };
        let types = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("tile types"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rg8Uint,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &tile_texels(map, blends),
        );
        let types_view = types.create_view(&Default::default());
        self.map_bg = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain map"),
            layout: &self.tex_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&self.tex_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&types_view) },
            ],
        }));
    }

    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, globals: &'a Globals) {
        let (Some((buf, n)), Some(bg)) = (&self.vertices, &self.map_bg) else { return };
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &globals.bind_group, &[]);
        pass.set_bind_group(1, bg, &[]);
        pass.set_vertex_buffer(0, buf.slice(..));
        pass.draw(0..*n, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zt_formats::zoo::Tile;

    fn map(tiles: Vec<Tile>, w: u32, h: u32) -> ZooMap {
        ZooMap {
            version: b'F',
            language: 0x409,
            width: w,
            height: h,
            exhibits: vec![],
            tiles,
            tiles_offset: 0,
            objects_offset: 0,
            object_count: 0,
            objects: vec![],
        }
    }

    fn flat(height: i32) -> Tile {
        Tile { height, shape: 0, terrain: 0, edges: 0, extra: [0; 3] }
    }

    #[test]
    fn slopes_are_not_shaded() {
        let mut t = flat(0);
        t.shape = 0x41;
        let v = build_mesh(&map(vec![t], 1, 1), &Lighting::default(), 0);
        let shades: Vec<f32> =
            v.iter().filter(|v| v.material == MATERIAL_SURFACE).map(|v| v.shade).collect();
        assert!(shades.iter().all(|s| (s - shades[0]).abs() < 1e-6));
    }

    #[test]
    fn flat_map_has_no_walls() {
        // No walls around the map border either (the original shows a void).
        let m = map(vec![flat(0); 4], 2, 2);
        let v = build_mesh(&m, &Lighting::default(), 0);
        assert!(v.iter().all(|v| v.material == MATERIAL_SURFACE));
        assert_eq!(v.len(), 4 * 6);
    }

    #[test]
    fn cliff_gets_a_wall() {
        let walls = |tiles| {
            build_mesh(&map(tiles, 2, 1), &Lighting::default(), 0)
                .iter()
                .filter(|v| v.material == MATERIAL_WALL)
                .count()
        };
        assert_eq!(walls(vec![flat(0), flat(3)]) - walls(vec![flat(0), flat(0)]), 6);
    }

    #[test]
    fn flat_shading_matches_tilevar_lights() {
        let s = Lighting::default().shade(Vec3::Y);
        assert!((s - 0.98).abs() < 0.01, "{s}");
    }
}
