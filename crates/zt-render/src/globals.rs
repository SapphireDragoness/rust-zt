//! Per-frame uniforms shared by all pipelines (bind group 0).

use crate::camera::CameraUniform;

pub struct Globals {
    pub buffer: wgpu::Buffer,
    pub layout: wgpu::BindGroupLayout,
    pub bind_group: wgpu::BindGroup,
}

/// Prepend the shared WGSL (camera + projection) to a shader source.
pub fn shader_source(body: &str) -> String {
    format!("{}\n{}", include_str!("common.wgsl"), body)
}

impl Globals {
    pub fn new(device: &wgpu::Device) -> Globals {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
        });
        Globals { buffer, layout, bind_group }
    }

    pub fn update(&self, queue: &wgpu::Queue, u: &CameraUniform) {
        queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(u));
    }
}
