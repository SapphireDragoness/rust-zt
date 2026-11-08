//! Rendering for rust-zt (wgpu).

pub mod camera;
pub mod globals;
pub mod gpu;
pub mod sprites;
pub mod terrain;

pub use camera::Camera;
pub use globals::Globals;
pub use gpu::Gpu;
pub use wgpu;

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
