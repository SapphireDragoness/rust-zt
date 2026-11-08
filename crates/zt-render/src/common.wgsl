// Shared camera and projection. Prepended to the other shaders.

struct Camera {
    // Screen-pixel position of the view centre (after rotation).
    center: vec2<f32>,
    // Viewport size in pixels divided by zoom, halved.
    half_extent: vec2<f32>,
    // Map size in tiles.
    map_size: vec2<f32>,
    // 0..3, quarter turns.
    rotation: u32,
    time: f32,
};

@group(0) @binding(0) var<uniform> cam: Camera;

const ELEV_PX: f32 = 16.0;

// Rotate grid coordinates by quarter turns around the map.
fn rotate(p: vec2<f32>, r: u32, size: vec2<f32>) -> vec2<f32> {
    switch r {
        case 1u: { return vec2<f32>(size.y - p.y, p.x); }
        case 2u: { return vec2<f32>(size.x - p.x, size.y - p.y); }
        case 3u: { return vec2<f32>(p.y, size.x - p.x); }
        default: { return p; }
    }
}

// Grid (u, v, elevation) -> screen pixels relative to the map origin.
fn project(pos: vec3<f32>) -> vec2<f32> {
    let g = rotate(pos.xy, cam.rotation, cam.map_size);
    return vec2<f32>(32.0 * (g.x + g.y), 16.0 * (g.y - g.x) - ELEV_PX * pos.z);
}

// Nearer to the viewer = larger key.
fn depth_key(pos: vec3<f32>) -> f32 {
    let g = rotate(pos.xy, cam.rotation, cam.map_size);
    return (g.y - g.x) + pos.z * 0.5;
}

fn to_clip(screen: vec2<f32>, key: f32) -> vec4<f32> {
    let ndc = vec2<f32>((screen.x - cam.center.x) / cam.half_extent.x, -(screen.y - cam.center.y) / cam.half_extent.y);
    let span = cam.map_size.x + cam.map_size.y + 64.0;
    return vec4<f32>(ndc, clamp(0.5 - key / (2.0 * span), 0.0, 1.0), 1.0);
}
