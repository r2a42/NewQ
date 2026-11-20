
// ----- The Light view (render3d/lit.rs): projectors throwing their pictures -----
// Appended to shader.wgsl in a module of its own, so a GPU that refuses this
// leaves the plain view drawn. Plain on purpose (DirectX 12 and Metal): the
// arrays are only read, textures are read with the Level samplers (legal in
// the loop), nothing writes a vector's component at a run-time index.

struct ThrowU {
    view_proj: mat4x4<f32>,
    // xyz the lens; w the lux one metre out.
    eye: vec4<f32>,
    // xyz the axis; w the gamma.
    axis: vec4<f32>,
    // x 1 when it throws its picture (else white), y a pixel one metre out
    // (metres), z its picture's layer, w 1 when on.
    more: vec4<f32>,
};

struct Lights {
    // x how many, y the view (1 picture, 2 brightness, 3 overlap, 4 pixel
    // size), z the white point in nits, w the shadow maps' side.
    head: vec4<f32>,
    // x the room's light kept, y the push along the normal per metre, z the
    // depth slack.
    more: vec4<f32>,
    bright: vec4<f32>,
    pixel: vec4<f32>,
    scale: array<vec4<f32>, 5>,
    overlap: array<vec4<f32>, 3>,
    t: array<ThrowU, 16>,
};

@group(3) @binding(0) var<uniform> lights: Lights;
@group(3) @binding(1) var throws: texture_2d_array<f32>;
@group(3) @binding(2) var throw_samp: sampler;
@group(3) @binding(3) var shadows: texture_depth_2d_array;
@group(3) @binding(4) var shadow_cmp: sampler_comparison;

// Which of four steps a value is past (0 to 4).
fn band(v: f32, s: vec4<f32>) -> u32 {
    return u32(v >= s.x) + u32(v >= s.y) + u32(v >= s.z) + u32(v >= s.w);
}

@fragment
fn fs_lit(in: VOut) -> @location(0) vec4<f32> {
    // Sampled in uniform control flow, before anything branches.
    let b = base(in);
    let wi = u32(p.flags.w + 0.5);
    let front_only = (wi & 1u) == 1u;
    let reached = wi >= 2u;
    let venue = p.flags.x > 0.5;
    // A screen no projector reaches keeps its own picture (an LED wall).
    if (!venue && !reached) {
        return finish(b.rgb, b.a, in.world);
    }
    let to_eye = normalize(f.eye.xyz - in.world);
    var n = in.normal;
    if (dot(n, n) < 1e-12) {
        n = to_eye;
    }
    n = normalize(n);
    var seen_n = n;
    if (dot(seen_n, to_eye) < 0.0) {
        seen_n = -n;
    }
    // The room's own light, turned down so the projected light shows.
    let hemi = mix(f.ground.rgb, f.sky.rgb, seen_n.y * 0.5 + 0.5);
    let room = (hemi * f.light.y + vec3<f32>(f.light.x * max(dot(seen_n, to_eye), 0.0)))
        * f.light.z * lights.more.x;
    // What it gives back: the venue its colour, a screen its gain (white).
    var reflect = vec3<f32>(clamp(p.flags.z, 0.0, 10.0));
    if (venue) {
        reflect = to_linear(b.rgb);
    }

    var sum = vec3<f32>(0.0);
    var white = 0.0;
    var count = 0u;
    var finest = 1e9;
    let total = min(u32(lights.head.x + 0.5), 16u);
    for (var i = 0u; i < total; i = i + 1u) {
        let t = lights.t[i];
        if (t.more.w < 0.5) {
            continue;
        }
        let to_lens = t.eye.xyz - in.world;
        let dist = length(to_lens);
        if (dist < 1e-4) {
            continue;
        }
        let l = to_lens / dist;
        let facing = dot(n, l);
        if (front_only && facing <= 0.0) {
            continue;
        }
        let incidence = abs(facing);
        let depth = dot(in.world - t.eye.xyz, t.axis.xyz);
        if (depth <= 1e-4 || incidence <= 1e-6) {
            continue;
        }
        let c = t.view_proj * vec4<f32>(in.world, 1.0);
        if (c.w <= 1e-6) {
            continue;
        }
        let ndc = c.xy / c.w;
        if (abs(ndc.x) > 1.0 || abs(ndc.y) > 1.0) {
            continue;
        }
        let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
        // The shadow: the point pushed a little towards the lens along its
        // normal, and taken as a little nearer than it is.
        var towards = n;
        if (facing < 0.0) {
            towards = -n;
        }
        let pushed = in.world + towards * depth * lights.more.y;
        let cs = t.view_proj * vec4<f32>(pushed, 1.0);
        let suv = vec2<f32>(cs.x / cs.w * 0.5 + 0.5, 0.5 - cs.y / cs.w * 0.5);
        let sref = cs.z / cs.w * (1.0 + lights.more.z);
        let open = textureSampleCompareLevel(shadows, shadow_cmp, suv, i, sref);
        if (open <= 0.0) {
            continue;
        }
        var signal = vec3<f32>(1.0);
        if (t.more.x > 0.5) {
            let s = textureSampleLevel(throws, throw_samp, uv, i32(t.more.z + 0.5), 0.0).rgb;
            signal = pow(max(s, vec3<f32>(0.0)), vec3<f32>(t.axis.w));
        }
        let off_axis = max(depth / dist, 1e-3);
        let e = t.eye.w / (depth * depth) * incidence / off_axis * open;
        sum = sum + signal * e;
        white = white + e;
        if (open > 0.5) {
            count = count + 1u;
            finest = min(finest, t.more.y * depth * sqrt(off_axis / incidence));
        }
    }

    // Light seen: nits per channel (candela per square metre).
    let nits = reflect * sum / 3.14159265;
    let grey = vec3<f32>(dot(reflect, vec3<f32>(0.2126, 0.7152, 0.0722)));
    let shade = 0.75 + 0.25 * max(dot(seen_n, to_eye), 0.0);
    let mode = u32(lights.head.y + 0.5);
    var colour = vec3<f32>(0.0);
    if (mode == 2u) {
        // Brightness at full white, against the steps.
        let at_white = dot(reflect, vec3<f32>(0.2126, 0.7152, 0.0722)) * white / 3.14159265;
        if (count == 0u) {
            colour = to_display(grey * room);
        } else {
            colour = lights.scale[band(at_white, lights.bright)].rgb * shade;
        }
    } else if (mode == 3u) {
        if (count == 0u) {
            colour = to_display(grey * room);
        } else {
            colour = lights.overlap[min(count, 3u) - 1u].rgb * shade;
        }
    } else if (mode == 4u) {
        if (count == 0u) {
            colour = to_display(grey * room);
        } else {
            colour = lights.scale[band(finest * 1000.0, lights.pixel)].rgb * shade;
        }
    } else {
        // The picture: nits against the white point, over the room's light.
        colour = to_display(nits / max(lights.head.z, 0.01) + reflect * room);
    }
    var a = b.a;
    if (venue) {
        a = b.a * f.sky.w;
    }
    return finish(colour, a, in.world);
}
