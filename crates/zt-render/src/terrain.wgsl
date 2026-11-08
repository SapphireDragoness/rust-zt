// Terrain: textured, flat-shaded tiles with soft blending between terrain types.

@group(1) @binding(0) var textures: texture_2d_array<f32>;
@group(1) @binding(1) var tex_sampler: sampler;
// Per-tile terrain layer index (r) and blend flag (g).
@group(1) @binding(2) var tile_types: texture_2d<u32>;

struct VsIn {
    @location(0) pos: vec3<f32>,      // grid u, grid v, elevation
    @location(1) shade: f32,          // precomputed lighting
    @location(2) material: u32,       // 0 = surface, 1 = wall
    @location(3) layer: u32,          // walls: texture layer (cliff)
    @location(4) normal: vec2<f32>,   // walls: facing direction (grid space)
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) shade: f32,
    @location(2) @interpolate(flat) material: u32,
    @location(3) elev: f32,
    @location(4) @interpolate(flat) layer: u32,
    @location(5) @interpolate(flat) wall_shade: f32,
};

// Rotate a grid-space direction by the camera's quarter turns.
fn rotate_dir(d: vec2<f32>, r: u32) -> vec2<f32> {
    switch r {
        case 1u: { return vec2<f32>(-d.y, d.x); }
        case 2u: { return -d; }
        case 3u: { return vec2<f32>(d.y, -d.x); }
        default: { return d; }
    }
}

// Visible walls face screen-left (rotated -u) or screen-right (rotated +v).
// Fitted to the original (÷ the flat-ground shade ≈ 0.98): a left-facing
// river drop shows the cliff texture at ~0.72 brightness; the walls of a
// tank pit, seen through the water, give ~0.7 left and ~1.2 right.
const WALL_LEFT_SHADE: f32 = 0.735;
const WALL_RIGHT_SHADE: f32 = 1.225;

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = to_clip(project(in.pos), depth_key(in.pos));
    out.uv = in.pos.xy;
    out.shade = in.shade;
    out.material = in.material;
    out.elev = in.pos.z;
    out.layer = in.layer;
    let n = rotate_dir(in.normal, cam.rotation);
    // Screen x of the normal is n.x + n.y: negative faces left.
    out.wall_shade = select(WALL_RIGHT_SHADE, WALL_LEFT_SHADE, n.x + n.y < 0.0);
    return out;
}

const TEX_TILES: f32 = 3.0; // a texture spans this many tiles
// Fitted against the game's pre-rendered ztatb tiles (lower = wider, softer).
const BLEND_SHARPNESS: f32 = 0.4;
const BLEND_NOISE: f32 = 0.15;

fn tile_info(t: vec2<i32>) -> vec2<u32> {
    let size = vec2<i32>(textureDimensions(tile_types));
    let c = clamp(t, vec2<i32>(0, 0), size - vec2<i32>(1, 1));
    return textureLoad(tile_types, c, 0).rg;
}

fn sample_layer(layer: u32, uv: vec2<f32>) -> vec3<f32> {
    // Level 0 explicitly: sampling happens in non-uniform control flow.
    return textureSampleLevel(textures, tex_sampler, uv / TEX_TILES, i32(layer), 0.0).rgb;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var color: vec3<f32>;
    if (in.material == 1u) {
        // Banks, cliffs, tank pits and map sides show the cliff texture.
        return vec4<f32>(sample_layer(in.layer, vec2<f32>(in.uv.x + in.uv.y, -in.elev * 0.35)).rgb
            * in.shade * in.wall_shade, 1.0);
    } else {
        // Vertex blending: each corner of the tile mixes the four tiles that
        // meet there, interpolated bilinearly across the tile. A neighbour
        // therefore reaches a whole tile into its neighbours, as in the
        // game's pre-rendered blends (whose keys include diagonal neighbours).
        let tile = vec2<i32>(floor(in.uv));
        let f = fract(in.uv);
        let own = tile_info(tile);
        // Bilinear weights of the corners (0,0), (1,0), (0,1), (1,1).
        let cw = vec4<f32>((1.0 - f.x) * (1.0 - f.y), f.x * (1.0 - f.y), (1.0 - f.x) * f.y, f.x * f.y);
        var acc = vec3<f32>(0.0);
        var wsum = 0.0;
        for (var dy = -1; dy <= 1; dy++) {
            for (var dx = -1; dx <= 1; dx++) {
                let t = tile + vec2<i32>(dx, dy);
                let info = tile_info(t);
                // Corners of this tile that the neighbour touches.
                let lx = select(1.0, 0.0, dx == 1);   // touches x=0 corners
                let hx = select(1.0, 0.0, dx == -1);  // touches x=1 corners
                let ly = select(1.0, 0.0, dy == 1);
                let hy = select(1.0, 0.0, dy == -1);
                var bw = (cw.x * lx * ly + cw.y * hx * ly + cw.z * lx * hy + cw.w * hx * hy) * 0.25;
                // Non-blending terrain (concrete, asphalt…) keeps hard edges.
                let is_own = dx == 0 && dy == 0;
                if (own.g == 0u) {
                    bw = select(0.0, 1.0, is_own);
                } else if (info.g == 0u) {
                    bw = 0.0;
                }
                if (bw <= 0.0) {
                    continue;
                }
                let c = sample_layer(info.r, in.uv);
                // Brighter texels of each texture win slightly: a speckled
                // edge rather than a smooth cross-fade.
                let lum = dot(c, vec3<f32>(0.299, 0.587, 0.114));
                let w = pow(bw, BLEND_SHARPNESS) * (BLEND_NOISE + lum);
                acc += c * w;
                wsum += w;
            }
        }
        if (wsum <= 0.0) {
            acc = sample_layer(own.r, in.uv);
            wsum = 1.0;
        }
        color = acc / max(wsum, 1e-4);
    }
    return vec4<f32>(color * in.shade, 1.0);
}
