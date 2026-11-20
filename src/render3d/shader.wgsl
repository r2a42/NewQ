// The 3D view (render3d/mod.rs). Colours are worked in display values (gamma),
// as the compositor's are, so a screen shows its display's picture as the
// outputs do; lighting is done in linear light and turned back.

struct Frame {
    view_proj: mat4x4<f32>,
    // xyz the eye; w the fog's density per metre (0: none).
    eye: vec4<f32>,
    // x the headlight, y the ambient, z the brightness, w the lift.
    light: vec4<f32>,
    // rgb the sky's ambient colour; w the venue's opacity (x-ray below 1).
    sky: vec4<f32>,
    // rgb the ground's ambient colour; w 1 when the target is sRGB.
    ground: vec4<f32>,
    // rgb the fog's colour; w the floor grid's half width in metres.
    fog: vec4<f32>,
    // x the axes' length, y the grid's strength.
    extra: vec4<f32>,
};
@group(0) @binding(0) var<uniform> f: Frame;

struct Part {
    model: mat4x4<f32>,
    n0: vec4<f32>,
    n1: vec4<f32>,
    n2: vec4<f32>,
    colour: vec4<f32>,
    // (left, top, width, height) of the picture that uv 0..1 covers.
    region: vec4<f32>,
    // x 1 when lit; y what it shows: 0 its colour, 1 its picture (times the
    // colour), 2 a missing picture.
    flags: vec4<f32>,
};
@group(1) @binding(0) var<uniform> p: Part;

@group(2) @binding(0) var tex: texture_2d<f32>;
@group(2) @binding(1) var samp: sampler;

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
fn vs_part(v: VIn) -> VOut {
    let w = p.model * vec4<f32>(v.pos, 1.0);
    let nm = mat3x3<f32>(p.n0.xyz, p.n1.xyz, p.n2.xyz);
    var o: VOut;
    o.clip = f.view_proj * w;
    o.world = w.xyz / w.w;
    o.normal = nm * v.normal;
    o.uv = v.uv;
    return o;
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    return pow(max(c, vec3<f32>(0.0)), vec3<f32>(2.2));
}

fn to_display(c: vec3<f32>) -> vec3<f32> {
    return pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let v = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    return select(pow((v + 0.055) / 1.055, vec3<f32>(2.4)), v / 12.92, v <= vec3<f32>(0.04045));
}

// Fog, then the target's encoding.
fn finish(c: vec3<f32>, a: f32, world: vec3<f32>) -> vec4<f32> {
    let d = distance(world, f.eye.xyz);
    let fogged = clamp(1.0 - exp(-f.eye.w * d), 0.0, 1.0);
    var out = mix(c, f.fog.rgb, fogged);
    if (f.ground.w > 0.5) {
        // An sRGB target turns light back into display values with the sRGB
        // curve: hand it the exact inverse, so the picture comes out as it went in.
        out = srgb_to_linear(out);
    }
    return vec4<f32>(out, a);
}

// What the part is painted with, before light.
fn base(in: VOut) -> vec4<f32> {
    let st = p.region.xy + in.uv * p.region.zw;
    // Sampled in uniform control flow, whatever the part shows.
    let picture = textureSample(tex, samp, st);
    let fw = max(fwidth(in.uv), vec2<f32>(1e-6));
    let kind = p.flags.y;
    if (kind < 0.5) {
        return p.colour;
    }
    if (kind < 1.5) {
        return picture * p.colour;
    }
    // A missing picture: dark grey with a thin lighter edge.
    let edge = min(min(in.uv.x, 1.0 - in.uv.x) / fw.x, min(in.uv.y, 1.0 - in.uv.y) / fw.y);
    let line = 1.0 - clamp(edge - 1.0, 0.0, 1.0);
    return vec4<f32>(mix(vec3<f32>(0.12), vec3<f32>(0.45), line), 1.0);
}

@fragment
fn fs_part(in: VOut) -> @location(0) vec4<f32> {
    let b = base(in);
    if (p.flags.x < 0.5) {
        return finish(b.rgb, b.a, in.world);
    }
    let to_eye = normalize(f.eye.xyz - in.world);
    var n = in.normal;
    if (dot(n, n) < 1e-12) {
        n = to_eye;
    }
    n = normalize(n);
    // Both faces lit: imported venues are wound either way.
    if (dot(n, to_eye) < 0.0) {
        n = -n;
    }
    let hemi = mix(f.ground.rgb, f.sky.rgb, n.y * 0.5 + 0.5);
    let light = hemi * f.light.y + vec3<f32>(f.light.x * max(dot(n, to_eye), 0.0));
    // The lift adds grey to dark colours (the darker, the more; bright ones
    // keep theirs) so a black-box room is still readable.
    let lum = dot(b.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    let albedo = to_linear(b.rgb + vec3<f32>(f.light.w * (1.0 - lum) * (1.0 - lum)));
    let lit = to_display(albedo * light * f.light.z);
    return finish(lit, b.a * f.sky.w, in.world);
}

// ----- The floor grid: one big square drawn as lines by the fragment shader ---

struct GOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
};

@vertex
fn vs_grid(@builtin(vertex_index) i: u32) -> GOut {
    var c = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0)
    );
    let e = f.fog.w;
    // A millimetre above the floor, so a venue's own floor doesn't hide it.
    let world = vec3<f32>(c[i].x * e, 0.002, c[i].y * e);
    var o: GOut;
    o.clip = f.view_proj * vec4<f32>(world, 1.0);
    o.world = world;
    return o;
}

// How much of a line `width` pixels wide every `step` metres covers here.
fn lines(at: vec2<f32>, step: f32, width: f32) -> f32 {
    let q = at / step;
    let fw = max(fwidth(q), vec2<f32>(1e-6));
    let g = abs(fract(q - 0.5) - 0.5) / fw;
    let cover = 1.0 - clamp(min(g.x, g.y) - (width - 1.0) * 0.5, 0.0, 1.0);
    // Lines closer than a few pixels apart fade out rather than shimmer.
    return cover * (1.0 - smoothstep(0.2, 0.5, max(fw.x, fw.y)));
}

@fragment
fn fs_grid(in: GOut) -> @location(0) vec4<f32> {
    let metre = lines(in.world.xz, 1.0, 1.0);
    let five = lines(in.world.xz, 5.0, 2.0);
    let fade = 1.0 - smoothstep(f.fog.w * 0.6, f.fog.w, length(in.world.xz - f.eye.xz));
    let a = max(metre * 0.3, five * 0.6) * fade * f.extra.y;
    return finish(vec3<f32>(0.6), a, in.world);
}

// ----- The world's axes at the origin: X red, Y green, Z blue ------------------

struct AOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) colour: vec3<f32>,
    @location(1) world: vec3<f32>,
};

@vertex
fn vs_axes(@builtin(vertex_index) i: u32) -> AOut {
    let axis = i / 2u;
    // Picked by comparison, not by writing a component at a run-time index:
    // Windows' DirectX 12 refused that pipeline (the axes blanked the view).
    let dir = vec3<f32>(f32(axis == 0u), f32(axis == 1u), f32(axis == 2u));
    let world = dir * f.extra.x * f32(i % 2u) + vec3<f32>(0.0, 0.003, 0.0);
    var o: AOut;
    o.clip = f.view_proj * vec4<f32>(world, 1.0);
    o.colour = vec3<f32>(0.95, 0.25, 0.2);
    if (axis == 1u) {
        o.colour = vec3<f32>(0.3, 0.9, 0.3);
    } else if (axis == 2u) {
        o.colour = vec3<f32>(0.3, 0.5, 1.0);
    }
    o.world = world;
    return o;
}

@fragment
fn fs_axes(in: AOut) -> @location(0) vec4<f32> {
    return finish(in.colour, 1.0, in.world);
}

// ----- Lines over the view: a projector's cone and its picture's outline -------

struct LIn {
    @location(0) pos: vec3<f32>,
    @location(1) colour: vec4<f32>,
};

struct LOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) colour: vec4<f32>,
};

@vertex
fn vs_line(v: LIn) -> LOut {
    var o: LOut;
    // A line passing by the eye is cut at the near plane by the clipper.
    o.clip = f.view_proj * vec4<f32>(v.pos, 1.0);
    o.colour = v.colour;
    return o;
}

@fragment
fn fs_line(in: LOut) -> @location(0) vec4<f32> {
    // No fog: an overlay, not part of the room.
    var c = in.colour.rgb;
    if (f.ground.w > 0.5) {
        c = srgb_to_linear(c);
    }
    return vec4<f32>(c, in.colour.a);
}
