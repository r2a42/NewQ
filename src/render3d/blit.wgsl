// ----- Scaling a picture drawn smaller up to the target -----------------------

struct BOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

@vertex
fn vs_blit(@builtin(vertex_index) i: u32) -> BOut {
    var c = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var o: BOut;
    o.pos = vec4<f32>(c[i], 0.0, 1.0);
    o.uv = vec2<f32>((c[i].x + 1.0) * 0.5, (1.0 - c[i].y) * 0.5);
    return o;
}

@fragment
fn fs_blit(in: BOut) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, in.uv);
}
