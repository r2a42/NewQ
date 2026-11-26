// The lens map's bake (render3d/bake.rs): the still parts drawn from a
// mapped projector's lens at twice the map's size, each sample writing where
// in its feed's picture it is (`place`) and which feed (`how`), packed as
// mapped.rs's header says; then a resolve to the map's size. Follows
// mapped::WGSL_UNPACK (map_byte, unpack_place, unpack_feed).

struct Frame {
    // World to the tile's part of the lens (reversed depth).
    view_proj: mat4x4<f32>,
    // xyz the lens.
    eye: vec4<f32>,
    // x what a fed screen's clip z is moved by: nearer by `SCREEN_SLACK` of
    // its distance, so one on a wall wins.
    extra: vec4<f32>,
};
@group(0) @binding(0) var<uniform> f: Frame;

struct Part {
    model: mat4x4<f32>,
    n0: vec4<f32>,
    n1: vec4<f32>,
    n2: vec4<f32>,
    // (left, top, width, height) of the feed's picture uv 0..1 covers.
    region: vec4<f32>,
    // x which feed it shows (1 to 4; 0 none: only in the way), y 1 when
    // only its front counts (a front screen).
    flags: vec4<f32>,
};
@group(1) @binding(0) var<uniform> p: Part;

struct VIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

@vertex
fn vs_bake(v: VIn) -> VOut {
    let w = p.model * vec4<f32>(v.pos, 1.0);
    let nm = mat3x3<f32>(p.n0.xyz, p.n1.xyz, p.n2.xyz);
    var o: VOut;
    o.clip = f.view_proj * w;
    if p.flags.x > 0.5 {
        o.clip.z = o.clip.z + f.extra.x;
    }
    o.world = w.xyz / w.w;
    o.normal = nm * v.normal;
    o.uv = v.uv;
    return o;
}

struct Baked {
    @location(0) place: vec4<f32>,
    @location(1) how: vec4<f32>,
};

// A 0..1 number as two bytes, high first (mapped::pack_place).
fn byte_pair(v: f32) -> vec2<f32> {
    let q = u32(round(clamp(v, 0.0, 1.0) * 65535.0));
    return vec2<f32>(f32(q >> 8u), f32(q & 255u)) / 255.0;
}

fn pack_place(at: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(byte_pair(at.x), byte_pair(at.y));
}

fn nothing() -> Baked {
    var o: Baked;
    o.place = vec4<f32>(0.0);
    o.how = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    return o;
}

@fragment
fn fs_bake(i: VOut) -> Baked {
    let feed = round(p.flags.x);
    // A front screen met from behind takes no light (and is in the way).
    let seen = p.flags.y < 0.5 || dot(i.normal, f.eye.xyz - i.world) > 0.0;
    if feed < 0.5 || !seen {
        return nothing();
    }
    var o: Baked;
    o.place = pack_place(p.region.xy + i.uv * p.region.zw);
    o.how = vec4<f32>(1.0, feed / 255.0, 1.0, 1.0);
    return o;
}

// ----- The resolve: four samples to one pixel ----------------------------------

@group(0) @binding(1) var big_place: texture_2d<f32>;
@group(0) @binding(2) var big_how: texture_2d<f32>;
struct Tile {
    // xy where the tile starts in the map (pixels).
    origin: vec4<i32>,
};
@group(0) @binding(3) var<uniform> tile: Tile;

@vertex
fn vs_whole(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

// The feed most of the four samples show (the first of a tie), the average
// of their places (the pixel's middle when all four agree) and the share of
// the samples that show it as the coverage.
@fragment
fn fs_resolve(@builtin(position) pos: vec4<f32>) -> Baked {
    let local = vec2<i32>(floor(pos.xy)) - tile.origin.xy;
    var counts = array<u32, 5>(0u, 0u, 0u, 0u, 0u);
    var feeds = array<u32, 4>(0u, 0u, 0u, 0u);
    var places = array<vec2<f32>, 4>(vec2<f32>(0.0), vec2<f32>(0.0), vec2<f32>(0.0), vec2<f32>(0.0));
    for (var k = 0; k < 4; k++) {
        let s = local * 2 + vec2<i32>(k & 1, k >> 1u);
        let fd = min(unpack_feed(textureLoad(big_how, s, 0)), 4u);
        feeds[k] = fd;
        places[k] = unpack_place(textureLoad(big_place, s, 0));
        counts[fd] = counts[fd] + 1u;
    }
    var best = 0u;
    for (var fd = 1u; fd <= 4u; fd++) {
        if counts[fd] > counts[best] || (best == 0u && counts[fd] > 0u) {
            best = fd;
        }
    }
    if best == 0u {
        return nothing();
    }
    var sum = vec2<f32>(0.0);
    for (var k = 0; k < 4; k++) {
        if feeds[k] == best {
            sum = sum + places[k];
        }
    }
    let n = f32(counts[best]);
    var o: Baked;
    o.place = pack_place(sum / n);
    o.how = vec4<f32>(1.0, f32(best) / 255.0, n / 4.0, 1.0);
    return o;
}
