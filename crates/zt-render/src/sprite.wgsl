// Sprites: instanced, anchored at a world position, alpha-tested; optionally 50% translucent.

@group(1) @binding(0) var atlas: texture_2d_array<f32>;
@group(1) @binding(1) var atlas_sampler: sampler;

struct Instance {
    @location(0) world: vec3<f32>,
    @location(1) layer: u32,
    // Top-left corner relative to the anchor, in pixels (unflipped).
    @location(2) offset: vec2<f32>,
    @location(3) size: vec2<f32>,
    @location(4) uv: vec4<f32>,
    @location(5) depth_bias: f32,
    @location(6) flags: u32,   // 1 = mirrored, 2 = translucent, bits 2-3 = depth plane
    @location(7) tint: vec4<f32>, // rgb, amount
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) layer: u32,
    @location(2) @interpolate(flat) alpha: f32,
    @location(3) @interpolate(flat) tint: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Instance) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    let c = corners[vi];
    var offset = inst.offset;
    var tc = c;
    if ((inst.flags & 1u) != 0u) {
        // Mirror around the anchor (west-facing views reuse the east sprites).
        offset.x = -(inst.offset.x + inst.size.x);
        tc.x = 1.0 - c.x;
    }
    let screen = project(inst.world) + offset + c * inst.size;
    var key = depth_key(inst.world);
    let plane = (inst.flags >> 2u) & 3u;
    if (plane != 0u) {
        // Depth of the point of a plane through the anchor that is seen at
        // this screen position (linear, so per-vertex is exact). With rotated
        // grid (u, v) and d = v - u: screen.x = 32 (u + v),
        // screen.y = 16 d - 16 z, key = d + z / 2.
        let a = rotate(inst.world.xy, cam.rotation, cam.map_size);
        var d: f32;
        var z: f32;
        if (plane == 3u) {
            z = inst.world.z;
            d = (screen.y + ELEV_PX * z) / 16.0;
        } else {
            // A quarter turn swaps constant u and constant v.
            let u_const = (plane == 1u) != ((cam.rotation & 1u) == 1u);
            if (u_const) {
                d = screen.x / 32.0 - 2.0 * a.x;
            } else {
                d = 2.0 * a.y - screen.x / 32.0;
            }
            z = (16.0 * d - screen.y) / ELEV_PX;
        }
        key = d + z * 0.5;
    }
    var out: VsOut;
    out.clip = to_clip(screen, key + inst.depth_bias);
    out.uv = mix(inst.uv.xy, inst.uv.zw, tc);
    out.layer = inst.layer;
    // The original blends water and glass 50/50 with what is behind.
    out.alpha = select(1.0, 0.5, (inst.flags & 2u) != 0u);
    out.tint = inst.tint;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSampleLevel(atlas, atlas_sampler, in.uv, i32(in.layer), 0.0);
    if (c.a < 0.5) {
        discard;
    }
    // Translucent sprites: fitted to the original's tank water, which is
    // the sprite at ~93% brightness blended 50/50 with what's behind.
    let k = select(1.0, 0.93, in.alpha < 1.0);
    let rgb = mix(c.rgb, in.tint.rgb, in.tint.a);
    return vec4<f32>(rgb * k, in.alpha);
}
