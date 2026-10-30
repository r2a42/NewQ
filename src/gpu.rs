//! GPU setup and the output compositor, which draws video/image layers with
//! opacity onto output windows.

use crate::color;
use crate::fx::VideoFx;
use crate::media::ffmpeg::ColorMatrix;
use crate::media::hap::HapFormat;
use crate::mesh::{self, Mesh};
use crate::show::{ColorAdjust, EdgeBlend, Fit, LayerTransform};
use anyhow::{Result, anyhow};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use winit::window::Window;

/// Whether the graphics device has gone (a driver reset or crash, the card
/// removed). Everything made on it is then useless and must be rebuilt.
#[derive(Default)]
pub struct DeviceLost(parking_lot::Mutex<Option<String>>);

impl DeviceLost {
    /// Start watching `device`, and keep its errors out of the log once it
    /// has gone (every call on it fails then). An error in the GPU's drivers
    /// is logged, not a reason to take the whole program down: the default is
    /// to panic the thread that hit it.
    fn watch(device: &wgpu::Device) -> Arc<DeviceLost> {
        let lost = Arc::new(DeviceLost::default());
        let seen = lost.clone();
        device.on_uncaptured_error(Arc::new(move |e| {
            if seen.why().is_none() {
                log::error!("GPU error: {e}")
            }
        }));
        let reported = lost.clone();
        device.set_device_lost_callback(move |reason, message| {
            log::error!("GPU device lost ({reason:?}): {message}");
            reported.set(format!("{reason:?}: {message}"));
        });
        lost
    }

    fn set(&self, why: String) {
        self.0.lock().get_or_insert(why);
    }

    pub fn why(&self) -> Option<String> {
        self.0.lock().clone()
    }
}

/// The graphics adapter in use, for the Test Mode report.
pub static GPU_NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// What NewQ has asked the graphics card for, in MB (what wgpu has allocated:
/// textures and buffers), as the main loop last looked; `u64::MAX` until it
/// has, or where the graphics backend can't say. A leak on the card shows here
/// and in no process's memory figure.
static GPU_ALLOCATED_MB: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(u64::MAX);

pub fn gpu_allocated_mb() -> Option<u64> {
    match GPU_ALLOCATED_MB.load(std::sync::atomic::Ordering::Relaxed) {
        u64::MAX => None,
        mb => Some(mb),
    }
}

/// Shared GPU context. Cheap to clone; render threads each hold a copy.
#[derive(Clone)]
pub struct Gpu {
    pub lost: Arc<DeviceLost>,
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub compositor: Arc<Compositor>,
}

impl Gpu {
    /// Create the GPU context, using `window` to pick a compatible adapter.
    pub fn new(
        instance: wgpu::Instance,
        window: Arc<Window>,
    ) -> Result<(Gpu, wgpu::Surface<'static>)> {
        let surface = instance.create_surface(window)?;
        let adapter =
            pollster::block_on(
                instance.request_adapter(&wgpu::RequestAdapterOptions {
                    // WGPU_POWER_PREF=low selects the integrated GPU on hybrid laptops.
                    power_preference: wgpu::PowerPreference::from_env()
                        .unwrap_or(wgpu::PowerPreference::HighPerformance),
                    force_fallback_adapter: false,
                    compatible_surface: Some(&surface),
                    apply_limit_buckets: false,
                }),
            )
            .map_err(|e| anyhow!("no suitable GPU adapter: {e}"))?;
        log::info!("GPU: {:?}", adapter.get_info());
        {
            let i = adapter.get_info();
            let _ = GPU_NAME.set(format!("{} ({:?}, {:?})", i.name, i.backend, i.device_type));
        }
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("newq"),
                // Hap's textures are block compressed, which most cards (and
                // Metal on a Mac) can read; the rest decode Hap as other video.
                required_features: adapter.features() & wgpu::Features::TEXTURE_COMPRESSION_BC,
                required_limits:
                    wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
                ..Default::default()
            }))?;
        crate::media::video::set_bc_supported(
            device
                .features()
                .contains(wgpu::Features::TEXTURE_COMPRESSION_BC),
        );
        let lost = DeviceLost::watch(&device);
        let compositor = Arc::new(Compositor::new(&device));
        Ok((
            Gpu {
                lost,
                instance,
                adapter,
                device,
                queue,
                compositor,
            },
            surface,
        ))
    }

    /// Note how much the graphics card has been asked for (see `gpu_allocated_mb`).
    pub fn note_allocated(&self) {
        let mb = self
            .device
            .generate_allocator_report()
            .map_or(u64::MAX, |r| r.total_allocated_bytes >> 20);
        GPU_ALLOCATED_MB.store(mb, std::sync::atomic::Ordering::Relaxed);
    }

    /// Let go of the memory of the texture uploads made so far. `write_texture`
    /// keeps each upload's staging copy until the next `submit`, and nothing
    /// else submits while no window is being drawn.
    pub fn flush_uploads(&self) {
        self.queue.submit(std::iter::empty());
    }

    /// A GPU with no window (for the benchmark and the tests), if this machine has
    /// one, or a software one. Not tied to a surface.
    pub fn headless() -> Option<Gpu> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle().with_env());
        let adapter =
            pollster::block_on(
                instance.request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::from_env()
                        .unwrap_or(wgpu::PowerPreference::HighPerformance),
                    force_fallback_adapter: false,
                    compatible_surface: None,
                    apply_limit_buckets: false,
                }),
            )
            .ok()?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("headless"),
            required_features: adapter.features() & wgpu::Features::TEXTURE_COMPRESSION_BC,
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            ..Default::default()
        }))
        .ok()?;
        let compositor = Arc::new(Compositor::new(&device));
        Some(Gpu {
            lost: DeviceLost::watch(&device),
            instance,
            adapter,
            device,
            queue,
            compositor,
        })
    }

    /// Whether the device reads block-compressed textures (Hap).
    pub fn reads_bc(&self) -> bool {
        self.device
            .features()
            .contains(wgpu::Features::TEXTURE_COMPRESSION_BC)
    }

    /// A surface configuration in a non-sRGB format, so that blending (fades and
    /// crossfades) happens in gamma space like a video mixer. None if this
    /// adapter can't draw to the surface at all (it reports no formats), as
    /// can happen for a window on a display driven by another graphics card.
    /// Configure a surface and say whether it took. Configuring waits for the
    /// device to go idle, which fails if another window is in the middle of a
    /// frame, so a window opened while the others are drawing tries again.
    pub fn configure_checked(
        &self,
        surface: &wgpu::Surface,
        config: &wgpu::SurfaceConfiguration,
    ) -> bool {
        for attempt in 0..40 {
            let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
            surface.configure(&self.device, config);
            if pollster::block_on(scope.pop()).is_none() {
                return true;
            }
            log::debug!("surface not configured (try {attempt}); trying again");
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        false
    }

    pub fn surface_config(
        &self,
        surface: &wgpu::Surface,
        size: (u32, u32),
    ) -> Option<wgpu::SurfaceConfiguration> {
        let caps = surface.get_capabilities(&self.adapter);
        log::info!(
            "surface supports formats {:?}, alpha {:?}, present {:?}",
            caps.formats,
            caps.alpha_modes,
            caps.present_modes
        );
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .or_else(|| caps.formats.first().copied())?;
        Some(wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.0.max(1),
            height: size.1.max(1),
            // Each window presents from its own thread, so blocking on vsync is fine.
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes.first().copied()?,
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        })
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct LayerUniform {
    /// x0, y0 (top-left), x1, y1 (bottom-right) in NDC.
    rect: [f32; 4],
    opacity: f32,
    mode: u32,
    /// Whether the chroma key is on (0 or 1).
    key_on: u32,
    _pad: u32,
    /// YUV→RGB rows: (y, u, v, offset).
    r: [f32; 4],
    g: [f32; 4],
    b: [f32; 4],
    /// Canvas → screen homography rows (see [`corner_pin`]).
    h: Homography,
    /// Canvas → the output's display or sub display, for clipping to its edges.
    pin: Homography,
    /// The cue's own placement: rows (m00, m01, tx) and (m10, m11, ty) applied
    /// to the layer's NDC corners (see [`layer_placement`]).
    xf: [[f32; 4]; 2],
    /// The part of the texture drawn: u0, v0, u1, v1.
    uvr: [f32; 4],
    /// Colour adjustment rows: (r, g, b, offset) (see [`color::color_matrix`]).
    cm: [[f32; 4]; 3],
    /// The key: (cb, cr, tolerance, softness) and (mask r, g, b, spill)
    /// (see [`color::key_uniforms`]).
    kc: [f32; 4],
    ks: [f32; 4],
    /// The mesh warp: cells drawn per side of the layer (1 without a mesh),
    /// whether a mesh is on, and its columns and rows.
    grid: [u32; 4],
    /// Where the mesh's columns and rows lie across the picture.
    mx: [[f32; 4]; 3],
    my: [[f32; 4]; 3],
    /// The mesh's points, two to a row of four numbers.
    mesh: [[f32; 4]; MESH_VECS],
    /// Where the mesh sees the picture, for a display showing part of a
    /// bigger one: (left, top, 1 / width, 1 / height) of the part.
    slice: [f32; 4],
    /// Picture effects: (blur radius, sharpen, block size, negative), and
    /// (vignette, its softness, 1 / texture width, 1 / texture height)
    /// (see [`VideoFx`]).
    fx0: [f32; 4],
    fx1: [f32; 4],
}

/// Room in the shader for a mesh of `MAX_LINES` by `MAX_LINES` points, two
/// to a vector.
const MESH_VECS: usize = mesh::MAX_LINES * mesh::MAX_LINES / 2 + 1;
/// How finely a layer is cut up to follow a mesh, per side.
pub const MESH_CELLS: u32 = 48;

const SHADER: &str = r#"
struct Params {
    rect: vec4<f32>,
    opacity: f32,
    mode: u32,
    key_on: u32,
    pad: u32,
    r: vec4<f32>,
    g: vec4<f32>,
    b: vec4<f32>,
    // Corner pin: maps output-canvas coordinates (0..1) to screen coordinates (0..1).
    h0: vec4<f32>,
    h1: vec4<f32>,
    h2: vec4<f32>,
    // The output's own corner pin: canvas to its display or sub display (0..1).
    c0: vec4<f32>,
    c1: vec4<f32>,
    c2: vec4<f32>,
    // The cue's own transform on the NDC rect: rows (m00, m01, tx), (m10, m11, ty).
    x0: vec4<f32>,
    x1: vec4<f32>,
    // Texture sub-rectangle drawn (crop): u0, v0, u1, v1.
    uvr: vec4<f32>,
    // Colour adjustment: rows (r, g, b, offset).
    cm0: vec4<f32>,
    cm1: vec4<f32>,
    cm2: vec4<f32>,
    // Chroma key: (cb, cr, tolerance, softness), and (mask rgb, spill).
    kc: vec4<f32>,
    ks: vec4<f32>,
    // Mesh warp: (cells per side, mesh on, columns, rows), where its columns
    // and rows lie, and its points two to a vector.
    grid: vec4<u32>,
    mx: array<vec4<f32>, 3>,
    my: array<vec4<f32>, 3>,
    mesh: array<vec4<f32>, 41>,
    // Where the mesh sees the picture: (left, top, 1 / width, 1 / height).
    slice: vec4<f32>,
    // Picture effects: (blur radius, sharpen, block size, negative), and
    // (vignette, softness, texel width, texel height).
    fx0: vec4<f32>,
    fx1: vec4<f32>,
};
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var t0: texture_2d<f32>;
@group(0) @binding(2) var t1: texture_2d<f32>;
@group(0) @binding(3) var t2: texture_2d<f32>;
@group(0) @binding(4) var s: sampler;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // Position on the output canvas (0..1), for clipping to the pinned shape.
    @location(1) canvas: vec2<f32>,
    // Homogeneous position on the output's display or sub display, for clipping to
    // its edges. Linear in the canvas position, so it interpolates exactly.
    @location(2) display: vec3<f32>,
};

// The mesh, as mesh.rs works it out: a cubic Hermite spline through the
// points, one axis and then the other.
fn mesh_node(c: i32, r: i32) -> vec2<f32> {
    let cols = i32(p.grid.z);
    let rows = i32(p.grid.w);
    let idx = u32(clamp(r, 0, rows - 1) * cols + clamp(c, 0, cols - 1));
    let v = p.mesh[idx / 2u];
    return select(v.xy, v.zw, (idx & 1u) == 1u);
}

fn mesh_kx(k: i32) -> f32 {
    let c = clamp(k, 0, i32(p.grid.z) - 1);
    return p.mx[c / 4][c % 4];
}

fn mesh_ky(k: i32) -> f32 {
    let c = clamp(k, 0, i32(p.grid.w) - 1);
    return p.my[c / 4][c % 4];
}

fn hermite(ka: f32, kb: f32, kc: f32, kd: f32,
           a: vec2<f32>, b: vec2<f32>, c: vec2<f32>, d: vec2<f32>, u: f32) -> vec2<f32> {
    let h = kc - kb;
    let t = (u - kb) / h;
    let t2 = t * t;
    let t3 = t2 * t;
    let mb = (c - a) / (kc - ka);
    let mc = (d - b) / (kd - kb);
    return (2.0 * t3 - 3.0 * t2 + 1.0) * b + (t3 - 2.0 * t2 + t) * h * mb
        + (-2.0 * t3 + 3.0 * t2) * c + (t3 - t2) * h * mc;
}

fn mesh_inside(u: f32, v: f32) -> vec2<f32> {
    let cols = i32(p.grid.z);
    let rows = i32(p.grid.w);
    var i = 0;
    var j = 0;
    for (var k = 1; k < cols - 1; k = k + 1) {
        if (u >= mesh_kx(k)) { i = k; }
    }
    for (var k = 1; k < rows - 1; k = k + 1) {
        if (v >= mesh_ky(k)) { j = k; }
    }
    var q: array<vec2<f32>, 4>;
    for (var m = 0; m < 4; m = m + 1) {
        let r = j - 1 + m;
        q[m] = hermite(
            mesh_kx(i - 1), mesh_kx(i), mesh_kx(i + 1), mesh_kx(i + 2),
            mesh_node(i - 1, r), mesh_node(i, r), mesh_node(i + 1, r), mesh_node(i + 2, r), u);
    }
    return hermite(
        mesh_ky(j - 1), mesh_ky(j), mesh_ky(j + 1), mesh_ky(j + 2), q[0], q[1], q[2], q[3], v);
}

// Where picture position (u, v) lands on the display; past the picture's edge
// it carries on in a straight line.
fn mesh_eval(u: f32, v: f32) -> vec2<f32> {
    let uc = clamp(u, 0.0, 1.0);
    let vc = clamp(v, 0.0, 1.0);
    let e = 0.01;
    var o = mesh_inside(uc, vc);
    if (u > 1.0) {
        o = o + (u - uc) * (mesh_inside(1.0, vc) - mesh_inside(1.0 - e, vc)) / e;
    } else if (u < 0.0) {
        o = o + (u - uc) * (mesh_inside(e, vc) - mesh_inside(0.0, vc)) / e;
    }
    if (v > 1.0) {
        o = o + (v - vc) * (mesh_inside(uc, 1.0) - mesh_inside(uc, 1.0 - e)) / e;
    } else if (v < 0.0) {
        o = o + (v - vc) * (mesh_inside(uc, e) - mesh_inside(uc, 0.0)) / e;
    }
    return o;
}

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> VOut {
    // The layer is a grid of n by n cells, two triangles each (one cell
    // without a mesh warp).
    let n = p.grid.x;
    let cell = vi / 6u;
    var tri = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0));
    let c = (vec2<f32>(f32(cell % n), f32(cell / n)) + tri[vi % 6u]) / f32(n);
    let n0 = vec2<f32>(mix(p.rect.x, p.rect.z, c.x), mix(p.rect.y, p.rect.w, c.y));
    let ndc = vec2<f32>(
        p.x0.x * n0.x + p.x0.y * n0.y + p.x0.z,
        p.x1.x * n0.x + p.x1.y * n0.y + p.x1.z);
    let cu = vec3<f32>((ndc.x + 1.0) * 0.5, (1.0 - ndc.y) * 0.5, 1.0);
    var o: VOut;
    if (p.grid.y != 0u) {
        // The mesh puts the picture on the display; the placement of the
        // display on the screen follows.
        let su = (cu.x - p.slice.x) * p.slice.z;
        let sv = (cu.y - p.slice.y) * p.slice.w;
        let dsp = vec3<f32>(mesh_eval(su, sv), 1.0);
        let q = vec3<f32>(dot(p.h0.xyz, dsp), dot(p.h1.xyz, dsp), dot(p.h2.xyz, dsp));
        o.pos = vec4<f32>(2.0 * q.x - q.z, q.z - 2.0 * q.y, 0.0, q.z);
        o.display = dsp;
    } else {
        // Projective transform; the GPU's perspective divide by w (with
        // perspective-correct interpolation) does the rest.
        let q = vec3<f32>(dot(p.h0.xyz, cu), dot(p.h1.xyz, cu), dot(p.h2.xyz, cu));
        o.pos = vec4<f32>(2.0 * q.x - q.z, q.z - 2.0 * q.y, 0.0, q.z);
        o.display = vec3<f32>(dot(p.c0.xyz, cu), dot(p.c1.xyz, cu), dot(p.c2.xyz, cu));
    }
    o.uv = mix(p.uvr.xy, p.uvr.zw, c);
    o.canvas = cu.xy;
    return o;
}

// The layer's picture at `uv`, in RGB.
fn layer_at(uv: vec2<f32>) -> vec4<f32> {
    if (p.mode == 0u) {
        return textureSampleLevel(t0, s, uv, 0.0);
    }
    if (p.mode == 2u) {
        // Hap Q: scaled YCoCg in a BC3 texture (Co red, Cg green, the scale in
        // blue, Y in alpha).
        let q = textureSampleLevel(t0, s, uv, 0.0);
        let k = 1.0 / (q.b * (255.0 / 8.0) + 1.0);
        let co = (q.r - 128.0 / 255.0) * k;
        let cg = (q.g - 128.0 / 255.0) * k;
        return vec4<f32>(clamp(vec3<f32>(q.a + co - cg, q.a + cg, q.a - co - cg),
                               vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
    }
    let yuv = vec4<f32>(
        textureSampleLevel(t0, s, uv, 0.0).r,
        textureSampleLevel(t1, s, uv, 0.0).r,
        textureSampleLevel(t2, s, uv, 0.0).r,
        1.0);
    return vec4<f32>(clamp(vec3<f32>(dot(p.r, yuv), dot(p.g, yuv), dot(p.b, yuv)),
                           vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}

@fragment
fn fs(in: VOut) -> @location(0) vec4<f32> {
    // Layers that overhang the canvas (fill/crop) must not show outside the pinned shape.
    if (any(in.canvas < vec2<f32>(0.0)) || any(in.canvas > vec2<f32>(1.0))) {
        discard;
    }
    // A corner pin past the edge of a sub display is cut off there, as a
    // real display's edge would.
    let d = in.display.xy / in.display.z;
    if (any(d < vec2<f32>(0.0)) || any(d > vec2<f32>(1.0))) {
        discard;
    }
    let texel = p.fx1.zw;
    var uv = in.uv;
    // Pixelate: every pixel of a block looks at the block's middle.
    if (p.fx0.z >= 2.0) {
        let cell = p.fx0.z * texel;
        uv = (floor(uv / cell) + 0.5) * cell;
    }
    var rgba = layer_at(uv);
    // Blur: the average of samples spread over a disc, turned a different
    // way at each pixel so the pattern doesn't show (fx::blur_taps).
    if (p.fx0.x >= 0.25) {
        let n = clamp(u32(round(p.fx0.x * 0.7)), 8u, 48u);
        let turn = fract(52.9829189 * fract(dot(in.pos.xy, vec2<f32>(0.06711056, 0.00583715)))) * 6.2831853;
        var acc = rgba;
        for (var i = 0u; i < n; i = i + 1u) {
            let r = sqrt((f32(i) + 0.5) / f32(n)) * p.fx0.x;
            let a = f32(i) * 2.39996323 + turn;
            acc = acc + layer_at(uv + vec2<f32>(cos(a), sin(a)) * r * texel);
        }
        rgba = acc / f32(n + 1u);
    }
    // Sharpen: push each pixel away from the average of its four neighbours.
    if (p.fx0.y >= 0.01) {
        let near = (layer_at(uv + vec2<f32>(texel.x, 0.0)) + layer_at(uv - vec2<f32>(texel.x, 0.0))
            + layer_at(uv + vec2<f32>(0.0, texel.y)) + layer_at(uv - vec2<f32>(0.0, texel.y))) * 0.25;
        rgba = vec4<f32>(clamp(rgba.rgb + p.fx0.y * (rgba.rgb - near.rgb), vec3<f32>(0.0), vec3<f32>(1.0)), rgba.a);
    }
    // Key out the screen colour (and pull its spill from what stays), then
    // correct the colour. This is what color::apply does, number for number.
    var rgb = rgba.rgb;
    var key_a = 1.0;
    if (p.key_on != 0u) {
        let cbcr = vec2<f32>(
            dot(rgb, vec3<f32>(-0.168736, -0.331264, 0.5)),
            dot(rgb, vec3<f32>(0.5, -0.418688, -0.081312)));
        key_a = smoothstep(p.kc.z, p.kc.z + p.kc.w, distance(cbcr, p.kc.xy));
        let m = p.ks.xyz;
        let other = max(max(rgb.r * (1.0 - m.r), rgb.g * (1.0 - m.g)), rgb.b * (1.0 - m.b));
        let dom = dot(rgb, m);
        rgb = rgb + m * (min(dom, other) - dom) * p.ks.w;
    }
    rgb = clamp(
        vec3<f32>(
            dot(p.cm0.xyz, rgb) + p.cm0.w,
            dot(p.cm1.xyz, rgb) + p.cm1.w,
            dot(p.cm2.xyz, rgb) + p.cm2.w),
        vec3<f32>(0.0), vec3<f32>(1.0));
    rgb = mix(rgb, vec3<f32>(1.0) - rgb, p.fx0.w);
    if (p.fx1.x > 0.0) {
        // Darker towards the corners of the picture (fx::vignette_factor).
        let c = (in.uv - p.uvr.xy) / (p.uvr.zw - p.uvr.xy);
        let r = length((c - vec2<f32>(0.5)) * 2.0) / 1.41421356;
        let soft = clamp(p.fx1.y, 0.05, 1.0);
        let t = clamp((r - (1.0 - soft)) / soft, 0.0, 1.0);
        rgb = rgb * (1.0 - p.fx1.x * t * t * (3.0 - 2.0 * t));
    }
    let a = rgba.a * key_a * p.opacity;
    return vec4<f32>(rgb * a, a);
}
"#;

/// Where a layer draws once its fit, crop and transform are applied.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerPlacement {
    /// NDC rectangle (x0, y0 top-left, x1, y1 bottom-right) after cropping.
    pub rect: [f32; 4],
    /// Rows (m00, m01, tx) and (m10, m11, ty) mapping the rect's NDC corners
    /// to their final NDC positions.
    pub xf: [[f32; 4]; 2],
    /// The part of the texture drawn: u0, v0, u1, v1.
    pub uvr: [f32; 4],
}

/// Combine a layer's fitted NDC rect with its cue's transform. Scale and
/// rotation act about the centre of the uncropped picture; rotation is done
/// in pixel proportions so it doesn't shear on a non-square canvas.
pub fn layer_placement(fitted: [f32; 4], canvas: (u32, u32), t: &LayerTransform) -> LayerPlacement {
    // Crops are limited so something always remains.
    let cl = t.crop[0].clamp(0.0, 0.99);
    let ct = t.crop[1].clamp(0.0, 0.99);
    let cr = t.crop[2].clamp(0.0, 0.99 - cl);
    let cb = t.crop[3].clamp(0.0, 0.99 - ct);
    let [x0, y0, x1, y1] = fitted;
    let (w, h) = (x1 - x0, y1 - y0);
    let rect = [x0 + w * cl, y0 + h * ct, x1 - w * cr, y1 - h * cb];
    let aspect = canvas.0.max(1) as f32 / canvas.1.max(1) as f32;
    let (s, c) = t.rotation.to_radians().sin_cos();
    let xf = [
        [c * t.scale_x, s * t.scale_y / aspect, 2.0 * t.x, 0.0],
        [-s * t.scale_x * aspect, c * t.scale_y, -2.0 * t.y, 0.0],
    ];
    LayerPlacement {
        rect,
        xf,
        uvr: [cl, ct, 1.0 - cr, 1.0 - cb],
    }
}

/// What the shader needs to draw through `mesh`: the grid settings, where its
/// columns and rows lie, and its points.
#[allow(clippy::type_complexity)]
fn mesh_uniforms(
    mesh: Option<&Mesh>,
) -> (
    [u32; 4],
    [[f32; 4]; 3],
    [[f32; 4]; 3],
    [[f32; 4]; MESH_VECS],
) {
    let mut mx = [[0.0; 4]; 3];
    let mut my = [[0.0; 4]; 3];
    let mut points = [[0.0; 4]; MESH_VECS];
    let Some(m) = mesh.filter(|m| m.is_valid()) else {
        return ([1, 0, 0, 0], mx, my, points);
    };
    for (k, v) in m.xs.iter().enumerate() {
        mx[k / 4][k % 4] = *v;
    }
    for (k, v) in m.ys.iter().enumerate() {
        my[k / 4][k % 4] = *v;
    }
    for (k, p) in m.points.iter().enumerate() {
        points[k / 2][k % 2 * 2..k % 2 * 2 + 2].copy_from_slice(p);
    }
    (
        [MESH_CELLS, 1, m.cols() as u32, m.rows() as u32],
        mx,
        my,
        points,
    )
}

/// A 3×3 projective transform stored as three padded rows.
pub type Homography = [[f32; 4]; 3];

pub const IDENTITY: Homography = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
];

/// The homography taking the unit square (0,0)-(1,1) to `corners` (top-left,
/// top-right, bottom-right, bottom-left), after Heckbert's square-to-quad
/// mapping. Returns the identity for degenerate corners.
pub fn corner_pin(corners: &[[f32; 2]; 4]) -> Homography {
    let [[x0, y0], [x1, y1], [x2, y2], [x3, y3]] = corners.map(|c| c.map(f64::from));
    let (dx1, dx2, dx3) = (x1 - x2, x3 - x2, x0 - x1 + x2 - x3);
    let (dy1, dy2, dy3) = (y1 - y2, y3 - y2, y0 - y1 + y2 - y3);
    let den = dx1 * dy2 - dx2 * dy1;
    if den.abs() < 1e-12 {
        return IDENTITY;
    }
    let g = (dx3 * dy2 - dx2 * dy3) / den;
    let h = (dx1 * dy3 - dx3 * dy1) / den;
    let row = |a: f64, b: f64, c: f64| [a as f32, b as f32, c as f32, 0.0];
    [
        row(x1 - x0 + g * x1, x3 - x0 + h * x3, x0),
        row(y1 - y0 + g * y1, y3 - y0 + h * y3, y0),
        row(g, h, 1.0),
    ]
}

/// The homography applying `first`, then `then`.
pub fn compose(then: &Homography, first: &Homography) -> Homography {
    let mut out = [[0.0; 4]; 3];
    for (r, row) in out.iter_mut().enumerate() {
        for (c, v) in row.iter_mut().take(3).enumerate() {
            *v = (0..3).map(|k| then[r][k] * first[k][c]).sum();
        }
    }
    out
}

/// Apply a homography to a point.
pub fn project(h: &Homography, u: f32, v: f32) -> [f32; 2] {
    let w = h[2][0] * u + h[2][1] * v + h[2][2];
    [
        (h[0][0] * u + h[0][1] * v + h[0][2]) / w,
        (h[1][0] * u + h[1][1] * v + h[1][2]) / w,
    ]
}

/// Limited-range YUV → full-range RGB coefficients.
fn yuv_rows(matrix: ColorMatrix) -> [[f32; 4]; 3] {
    let (rv, gu, gv, bu) = match matrix {
        ColorMatrix::Bt601 => (1.402, -0.344_136, -0.714_136, 1.772),
        ColorMatrix::Bt709 => (1.5748, -0.187_324, -0.468_124, 1.8556),
        ColorMatrix::Bt2020 => (1.4746, -0.164_553, -0.571_353, 1.8814),
    };
    let ys = 255.0 / 219.0;
    let cs = 255.0 / 224.0;
    let y0 = 16.0 / 255.0;
    let c0 = 128.0 / 255.0;
    let row = |ku: f32, kv: f32| [ys, ku * cs, kv * cs, -ys * y0 - ku * cs * c0 - kv * cs * c0];
    [row(0.0, rv), row(gu, gv), row(bu, 0.0)]
}

pub struct Compositor {
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    shader: wgpu::ShaderModule,
    sampler: wgpu::Sampler,
    dummy: wgpu::TextureView,
}

impl Compositor {
    fn new(device: &wgpu::Device) -> Compositor {
        let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("layer"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                tex_entry(1),
                tex_entry(2),
                tex_entry(3),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("layer"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("compositor"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("layer"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let dummy = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("dummy"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        Compositor {
            layout,
            pipeline_layout,
            shader,
            sampler,
            dummy,
        }
    }

    pub fn pipeline(
        &self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> wgpu::RenderPipeline {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("compositor"),
            layout: Some(&self.pipeline_layout),
            vertex: wgpu::VertexState {
                module: &self.shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &self.shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    }
}

const BLEND_SHADER: &str = r#"
struct Blend {
    // Left, right, top and bottom, as fractions of the display; then gamma.
    edges: vec4<f32>,
    gamma: vec4<f32>,
};
@group(0) @binding(0) var<uniform> b: Blend;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VOut {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    var o: VOut;
    o.pos = vec4<f32>(p[i], 0.0, 1.0);
    o.uv = vec2<f32>((p[i].x + 1.0) * 0.5, (1.0 - p[i].y) * 0.5);
    return o;
}

// blend.rs: a smoothstep share of the light, as a signal for the projector's gamma.
fn edge(pos: f32, width: f32) -> f32 {
    if (width <= 0.0) {
        return 1.0;
    }
    let t = clamp(pos / width, 0.0, 1.0);
    return pow(t * t * (3.0 - 2.0 * t), 1.0 / max(b.gamma.x, 1.0));
}

@fragment
fn fs(in: VOut) -> @location(0) vec4<f32> {
    let m = edge(in.uv.x, b.edges.x) * edge(1.0 - in.uv.x, b.edges.y)
        * edge(in.uv.y, b.edges.z) * edge(1.0 - in.uv.y, b.edges.w);
    return vec4<f32>(m, m, m, 1.0);
}
"#;

/// The fade at the edges of a window's picture: a pass drawn last that
/// multiplies what is there by the fade (see `blend.rs`).
pub struct BlendPass {
    pipeline: wgpu::RenderPipeline,
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl BlendPass {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> BlendPass {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blend"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("blend"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blend"),
            source: wgpu::ShaderSource::Wgsl(BLEND_SHADER.into()),
        });
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("blend"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blend"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        // What is there times the fade (colour), and what was there (alpha).
        let multiply = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::Src,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("blend"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(multiply),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        BlendPass {
            pipeline,
            buffer,
            bind_group,
        }
    }

    /// Set the fade to draw.
    pub fn set(&self, queue: &wgpu::Queue, blend: &EdgeBlend) {
        let b = blend.clamped();
        let data = [b.left, b.right, b.top, b.bottom, b.gamma, 0.0, 0.0, 0.0];
        queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&data));
    }
}

enum Planes {
    Yuv([wgpu::Texture; 3]),
    Rgba(wgpu::Texture),
    /// A block-compressed texture, as a Hap frame holds one.
    Bc(wgpu::Texture, HapFormat),
}

/// Who is drawing a layer. An output window and its previews in the main
/// window draw the same layer at different sizes and warps from different
/// threads, so each gets its own uniform buffer (placement) and bind group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    /// The output's own preview.
    Preview,
    /// The preview of a whole physical display, with its sub displays.
    Screen,
    /// A fullscreen output window (by the number of its display). Windows
    /// that show the same picture, as projectors sharing a wide one do,
    /// must not share a slot.
    Window(u8),
    /// A stream being sent out (by the number of the stream).
    Capture(u8),
}

/// How many windows can draw the same layer at once without sharing a slot.
const WINDOW_SLOTS: usize = 8;
/// And how many streams.
pub const CAPTURE_SLOTS: usize = 2;
const SLOTS: usize = 2 + WINDOW_SLOTS + CAPTURE_SLOTS;

impl Slot {
    fn index(self) -> usize {
        match self {
            Slot::Preview => 0,
            Slot::Screen => 1,
            Slot::Window(n) => 2 + n as usize % WINDOW_SLOTS,
            Slot::Capture(n) => 2 + WINDOW_SLOTS + n as usize % CAPTURE_SLOTS,
        }
    }
}

/// GPU textures for one playing layer, plus per-[`Slot`] uniforms.
pub struct LayerTexture {
    planes: Planes,
    pub width: u32,
    pub height: u32,
    matrix: ColorMatrix,
    slots: Vec<(wgpu::Buffer, wgpu::BindGroup)>,
    /// How many cells per side each slot was last set to draw.
    cells: Vec<AtomicU32>,
}

fn plane(device: &wgpu::Device, w: u32, h: u32, format: wgpu::TextureFormat) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("plane"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn write_plane(queue: &wgpu::Queue, tex: &wgpu::Texture, data: &[u8], bytes_per_pixel: u32) {
    let size = tex.size();
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size.width * bytes_per_pixel),
            rows_per_image: Some(size.height),
        },
        size,
    );
}

impl LayerTexture {
    pub fn new_yuv(gpu: &Gpu, w: u32, h: u32, matrix: ColorMatrix) -> LayerTexture {
        let d = &gpu.device;
        let planes = [
            plane(d, w, h, wgpu::TextureFormat::R8Unorm),
            plane(d, w / 2, h / 2, wgpu::TextureFormat::R8Unorm),
            plane(d, w / 2, h / 2, wgpu::TextureFormat::R8Unorm),
        ];
        let views: Vec<_> = planes
            .iter()
            .map(|t| t.create_view(&Default::default()))
            .collect();
        Self::finish(
            gpu,
            Planes::Yuv(planes),
            [&views[0], &views[1], &views[2]],
            w,
            h,
            matrix,
        )
    }

    pub fn new_rgba(gpu: &Gpu, w: u32, h: u32) -> LayerTexture {
        let tex = plane(&gpu.device, w, h, wgpu::TextureFormat::Rgba8Unorm);
        let view = tex.create_view(&Default::default());
        let dummy = &gpu.compositor.dummy;
        Self::finish(
            gpu,
            Planes::Rgba(tex),
            [&view, dummy, dummy],
            w,
            h,
            ColorMatrix::Bt709,
        )
    }

    /// A texture for Hap frames of `format`, `w` by `h` (both a multiple of 4).
    /// Needs a device that reads block-compressed textures (see `Gpu::new`).
    pub fn new_bc(gpu: &Gpu, w: u32, h: u32, format: HapFormat) -> LayerTexture {
        let bc = match format {
            HapFormat::Dxt1 => wgpu::TextureFormat::Bc1RgbaUnorm,
            HapFormat::Dxt5 | HapFormat::Dxt5YCoCg => wgpu::TextureFormat::Bc3RgbaUnorm,
        };
        let tex = plane(&gpu.device, w, h, bc);
        let view = tex.create_view(&Default::default());
        let dummy = &gpu.compositor.dummy;
        Self::finish(
            gpu,
            Planes::Bc(tex, format),
            [&view, dummy, dummy],
            w,
            h,
            ColorMatrix::Bt709,
        )
    }

    /// Whether this is a texture for Hap frames of `format`.
    pub fn is_bc(&self, format: HapFormat) -> bool {
        matches!(&self.planes, Planes::Bc(_, f) if *f == format)
    }

    /// Whether this is a texture for Hap frames of any format.
    pub fn is_bc_any(&self) -> bool {
        matches!(self.planes, Planes::Bc(..))
    }

    /// Whether this holds a plain RGBA picture.
    pub fn is_rgba(&self) -> bool {
        matches!(self.planes, Planes::Rgba(_))
    }

    /// Upload a Hap frame's texture data (its blocks, row by row).
    pub fn upload_bc(&self, queue: &wgpu::Queue, data: &[u8]) {
        let Planes::Bc(t, format) = &self.planes else {
            return;
        };
        let (bw, bh) = (self.width / 4, self.height / 4);
        if data.len() < format.texture_len(self.width, self.height) {
            return;
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: t,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bw * format.block_bytes() as u32),
                rows_per_image: Some(bh),
            },
            t.size(),
        );
    }

    fn finish(
        gpu: &Gpu,
        planes: Planes,
        views: [&wgpu::TextureView; 3],
        width: u32,
        height: u32,
        matrix: ColorMatrix,
    ) -> LayerTexture {
        let c = &gpu.compositor;
        let slot = || {
            let uniform = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("layer uniform"),
                size: std::mem::size_of::<LayerUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind_group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("layer"),
                layout: &c.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(views[0]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(views[1]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(views[2]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::Sampler(&c.sampler),
                    },
                ],
            });
            (uniform, bind_group)
        };
        LayerTexture {
            planes,
            width,
            height,
            matrix,
            slots: (0..SLOTS).map(|_| slot()).collect(),
            cells: (0..SLOTS).map(|_| AtomicU32::new(1)).collect(),
        }
    }

    /// Upload a yuv420p frame (Y plane followed by U and V).
    pub fn upload_yuv(&self, queue: &wgpu::Queue, data: &[u8]) {
        let Planes::Yuv(p) = &self.planes else { return };
        let ylen = (self.width * self.height) as usize;
        let clen = ((self.width / 2) * (self.height / 2)) as usize;
        if data.len() < ylen + 2 * clen {
            return;
        }
        write_plane(queue, &p[0], &data[..ylen], 1);
        write_plane(queue, &p[1], &data[ylen..ylen + clen], 1);
        write_plane(queue, &p[2], &data[ylen + clen..ylen + 2 * clen], 1);
    }

    /// Set where (on an output placed by `geometry`) and how opaquely this
    /// layer draws for `slot`.
    pub fn write_uniform(
        &self,
        queue: &wgpu::Queue,
        slot: Slot,
        geometry: &Geometry,
        opacity: f32,
        look: &Look,
    ) {
        let Look {
            fit,
            transform,
            color,
            fx,
        } = look;
        let rows = yuv_rows(self.matrix);
        let (kc, ks) = color
            .key
            .as_ref()
            .map_or(([0.0; 4], [0.0; 4]), color::key_uniforms);
        let place = layer_placement(
            fit_rect((self.width, self.height), geometry.canvas, *fit),
            geometry.canvas,
            transform,
        );
        let (grid, mx, my, points) = mesh_uniforms(geometry.mesh.as_deref());
        self.cells[slot.index()].store(grid[0], Ordering::Relaxed);
        let u = LayerUniform {
            rect: place.rect,
            opacity,
            mode: match &self.planes {
                Planes::Yuv(_) => 1,
                Planes::Bc(_, HapFormat::Dxt5YCoCg) => 2,
                _ => 0,
            },
            key_on: u32::from(color.key.is_some()),
            _pad: 0,
            r: rows[0],
            g: rows[1],
            b: rows[2],
            h: geometry.warp,
            pin: geometry.pin,
            xf: place.xf,
            uvr: place.uvr,
            cm: color::color_matrix(color),
            kc,
            ks,
            grid,
            mx,
            my,
            mesh: points,
            slice: geometry.slice,
            fx0: [fx.blur, fx.sharpen, fx.pixel, fx.negative],
            fx1: [
                fx.vignette,
                fx.softness,
                1.0 / self.width.max(1) as f32,
                1.0 / self.height.max(1) as f32,
            ],
        };
        queue.write_buffer(&self.slots[slot.index()].0, 0, bytemuck::bytes_of(&u));
    }

    /// Draw for `slot` with the compositor pipeline already set on `pass`.
    pub fn draw(&self, slot: Slot, pass: &mut wgpu::RenderPass) {
        pass.set_bind_group(0, &self.slots[slot.index()].1, &[]);
        let n = self.cells[slot.index()].load(Ordering::Relaxed);
        pass.draw(0..6 * n * n, 0..1);
    }

    pub fn upload_rgba(&self, queue: &wgpu::Queue, data: &[u8]) {
        let Planes::Rgba(t) = &self.planes else {
            return;
        };
        if data.len() >= (self.width * self.height * 4) as usize {
            write_plane(queue, t, data, 4);
        }
    }
}

/// How a layer is fitted, moved and coloured on its output.
#[derive(Clone, Copy)]
pub struct Look {
    pub fit: Fit,
    pub transform: LayerTransform,
    pub color: ColorAdjust,
    pub fx: VideoFx,
}

pub struct DrawLayer<'a> {
    pub texture: &'a LayerTexture,
    pub opacity: f32,
    pub fit: Fit,
    pub transform: LayerTransform,
    pub color: ColorAdjust,
    pub fx: VideoFx,
}

/// Where an output's picture (its canvas) lands on a screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Geometry {
    /// Pixel size of the output's canvas, which layers are fitted to.
    pub canvas: (u32, u32),
    /// Maps the canvas onto the screen (sub display area and corner pin).
    /// With a mesh it is only the display's placement on the screen.
    pub warp: Homography,
    /// The output's own corner pin, onto its display or sub display,
    /// whose edges clip it.
    pub pin: Homography,
    /// The output's mesh warp, which does the work of `pin` when there is one.
    pub mesh: Option<Arc<Mesh>>,
    /// Where the mesh sees the picture, for a display showing part of a
    /// bigger one: (left, top, 1 / width, 1 / height) of the part. The whole
    /// picture is `WHOLE`.
    pub slice: [f32; 4],
}

impl Geometry {
    /// The part of the picture a mesh sees when it sees all of it.
    pub const WHOLE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
}

/// One output's layers as drawn on a screen.
pub struct DrawGroup<'a> {
    pub layers: Vec<DrawLayer<'a>>,
    pub geometry: Geometry,
}

/// NDC rectangle for a `tex`-sized image placed in a `win`-sized window.
pub fn fit_rect(tex: (u32, u32), win: (u32, u32), fit: Fit) -> [f32; 4] {
    let (tw, th) = (tex.0.max(1) as f32, tex.1.max(1) as f32);
    let (ww, wh) = (win.0.max(1) as f32, win.1.max(1) as f32);
    let (sx, sy) = match fit {
        Fit::Stretch => (1.0, 1.0),
        Fit::Contain | Fit::Cover => {
            let s = if fit == Fit::Contain {
                (ww / tw).min(wh / th)
            } else {
                (ww / tw).max(wh / th)
            };
            (tw * s / ww, th * s / wh)
        }
    };
    [-sx, sy, sx, -sy]
}

impl Gpu {
    /// Draw layers (bottom to top) over black into `target`, then fade the
    /// edges if `blend` says so. `slot` is the window's own: windows showing
    /// the same layers must pass different ones.
    pub fn composite(
        &self,
        pipeline: &wgpu::RenderPipeline,
        slot: Slot,
        target: &wgpu::TextureView,
        groups: &[DrawGroup],
        blend: Option<&BlendPass>,
    ) -> wgpu::CommandBuffer {
        for g in groups {
            for l in &g.layers {
                l.texture.write_uniform(
                    &self.queue,
                    slot,
                    &g.geometry,
                    l.opacity,
                    &Look {
                        fit: l.fit,
                        transform: l.transform,
                        color: l.color,
                        fx: l.fx,
                    },
                );
            }
        }
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("composite"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            for l in groups.iter().flat_map(|g| &g.layers) {
                if l.opacity > 0.0 {
                    l.texture.draw(slot, &mut pass);
                }
            }
            if let Some(b) = blend {
                pass.set_pipeline(&b.pipeline);
                pass.set_bind_group(0, &b.bind_group, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        encoder.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rss_mb() -> u64 {
        let mut sys = sysinfo::System::new();
        let me = sysinfo::Pid::from_u32(std::process::id());
        sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[me]), true);
        sys.process(me).map_or(0, |p| p.memory() >> 20)
    }

    /// Uploads are held until the next submit, so with no window drawing (a
    /// Mac's background window) a minute of video was gigabytes. `flush_uploads`
    /// lets them go; without it 200 frames of 1080p held about 1.5 GB.
    #[test]
    fn flushing_uploads_keeps_memory_from_piling_up_when_nothing_is_drawn() {
        let Some(gpu) = Gpu::headless() else { return };
        let tex = LayerTexture::new_rgba(&gpu, 1920, 1080);
        let frame = vec![7u8; 1920 * 1080 * 4];
        let before = rss_mb();
        for _ in 0..200 {
            tex.upload_rgba(&gpu.queue, &frame);
            gpu.flush_uploads();
        }
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        let grew = rss_mb().saturating_sub(before);
        assert!(
            grew < 400,
            "memory grew by {grew} MB over 200 uploaded frames"
        );
    }

    #[test]
    fn fit_rect_letterboxes_and_crops() {
        // 16:9 video in a 4:3 window → letterbox top/bottom.
        let r = fit_rect((1920, 1080), (1024, 768), Fit::Contain);
        assert!((r[2] - 1.0).abs() < 1e-5);
        assert!(r[1] < 1.0);
        let r = fit_rect((1920, 1080), (1024, 768), Fit::Cover);
        assert!((r[1] - 1.0).abs() < 1e-5);
        assert!(r[2] > 1.0);
        assert_eq!(
            fit_rect((10, 10), (100, 50), Fit::Stretch),
            [-1.0, 1.0, 1.0, -1.0]
        );
    }

    /// Apply a placement's transform to an NDC point.
    fn xf(p: &LayerPlacement, x: f32, y: f32) -> (f32, f32) {
        (
            p.xf[0][0] * x + p.xf[0][1] * y + p.xf[0][2],
            p.xf[1][0] * x + p.xf[1][1] * y + p.xf[1][2],
        )
    }

    fn near(a: (f32, f32), b: (f32, f32)) {
        assert!(
            (a.0 - b.0).abs() < 1e-4 && (a.1 - b.1).abs() < 1e-4,
            "{a:?} != {b:?}"
        );
    }

    #[test]
    fn default_transform_changes_nothing() {
        let fitted = fit_rect((1920, 1080), (1920, 1080), Fit::Contain);
        let p = layer_placement(fitted, (1920, 1080), &LayerTransform::default());
        assert_eq!(p.rect, fitted);
        assert_eq!(p.uvr, [0.0, 0.0, 1.0, 1.0]);
        near(xf(&p, 0.3, -0.7), (0.3, -0.7));
    }

    #[test]
    fn scale_and_offset_move_the_picture() {
        let t = LayerTransform {
            x: 0.25,
            y: -0.1,
            scale_x: 0.5,
            scale_y: 0.5,
            ..Default::default()
        };
        let p = layer_placement([-1.0, 1.0, 1.0, -1.0], (1920, 1080), &t);
        // The top-left corner of a full-canvas picture: halved, then shifted
        // a quarter of the width right and a tenth of the height up.
        near(xf(&p, -1.0, 1.0), (-0.5 + 0.5, 0.5 + 0.2));
        // A negative scale mirrors.
        let m = LayerTransform {
            scale_x: -1.0,
            ..Default::default()
        };
        let p = layer_placement([-1.0, 1.0, 1.0, -1.0], (1920, 1080), &m);
        near(xf(&p, -1.0, 1.0), (1.0, 1.0));
    }

    #[test]
    fn rotation_is_clockwise_and_keeps_pixel_proportions() {
        let t = LayerTransform {
            rotation: 90.0,
            ..Default::default()
        };
        let canvas = (1920, 1080);
        let p = layer_placement([-1.0, 1.0, 1.0, -1.0], canvas, &t);
        let a = 1920.0 / 1080.0;
        // The middle of the right edge swings down to the bottom, a full
        // half-width (in pixels) below the centre.
        near(xf(&p, 1.0, 0.0), (0.0, -a));
        // The top-middle swings to the right, half a height from the centre in pixels.
        near(xf(&p, 0.0, 1.0), (1.0 / a, 0.0));
        // Lengths in pixels are preserved: the pixel distance from the centre is unchanged.
        let (x, y) = xf(&p, 0.6, 0.3);
        let before = (0.6 * a).hypot(0.3);
        let after = (x * a).hypot(y);
        assert!((before - after).abs() < 1e-4);
    }

    #[test]
    fn crop_trims_the_rect_and_the_texture_together() {
        let t = LayerTransform {
            crop: [0.1, 0.2, 0.3, 0.0],
            ..Default::default()
        };
        let p = layer_placement([-1.0, 1.0, 1.0, -1.0], (1920, 1080), &t);
        assert_eq!(p.uvr, [0.1, 0.2, 0.7, 1.0]);
        let [x0, y0, x1, y1] = p.rect;
        assert!((x0 - -0.8).abs() < 1e-5 && (x1 - 0.4).abs() < 1e-5);
        assert!((y0 - 0.6).abs() < 1e-5 && (y1 - -1.0).abs() < 1e-5);
        // Cropping everything still leaves a sliver rather than nothing.
        let all = LayerTransform {
            crop: [1.0, 1.0, 1.0, 1.0],
            ..Default::default()
        };
        let p = layer_placement([-1.0, 1.0, 1.0, -1.0], (1920, 1080), &all);
        assert!(p.uvr[2] > p.uvr[0] && p.uvr[3] > p.uvr[1]);
    }

    #[test]
    fn corner_pin_maps_the_unit_square_onto_the_corners() {
        let close =
            |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5;
        let id = corner_pin(&[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]);
        assert!(close(project(&id, 0.3, 0.7), [0.3, 0.7]));
        for c in [
            [[0.1, 0.05], [0.95, 0.0], [1.0, 1.0], [0.0, 0.9]],
            [[0.2, 0.0], [0.8, 0.0], [1.0, 1.0], [0.0, 1.0]], // classic keystone
            [[0.1, 0.1], [0.6, 0.1], [0.6, 0.4], [0.1, 0.4]], // affine (scale + move)
        ] {
            let h = corner_pin(&c);
            assert!(close(project(&h, 0.0, 0.0), c[0]));
            assert!(close(project(&h, 1.0, 0.0), c[1]));
            assert!(close(project(&h, 1.0, 1.0), c[2]));
            assert!(close(project(&h, 0.0, 1.0), c[3]));
        }
        // Perspective, not a stretch: the picture's centre lands where the
        // quad's diagonals cross. For this keystone (narrow at the top) that is
        // y = 2/7, well above the corners' average height of 0.5.
        let h = corner_pin(&[[0.3, 0.0], [0.7, 0.0], [1.0, 1.0], [0.0, 1.0]]);
        assert!(close(project(&h, 0.5, 0.5), [0.5, 2.0 / 7.0]));
        // Degenerate corners fall back to the identity.
        assert_eq!(corner_pin(&[[0.5, 0.5]; 4]), IDENTITY);
    }

    #[test]
    fn corner_pin_inside_a_sub_display_composes() {
        let close =
            |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5;
        // A sub display on the right half of the screen...
        let area = [[0.5, 0.0], [1.0, 0.0], [1.0, 1.0], [0.5, 1.0]];
        let region = corner_pin(&area);
        // ...with an un-pinned output fills exactly that half.
        let full = compose(&region, &IDENTITY);
        for (i, c) in [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]
            .iter()
            .enumerate()
        {
            assert!(close(project(&full, c[0], c[1]), area[i]));
        }
        // A keystone pin is applied within the sub display.
        let pin = [[0.2, 0.0], [0.8, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let h = compose(&region, &corner_pin(&pin));
        assert!(close(project(&h, 0.0, 0.0), [0.6, 0.0]));
        assert!(close(project(&h, 1.0, 0.0), [0.9, 0.0]));
        assert!(close(project(&h, 1.0, 1.0), [1.0, 1.0]));
        let mid = project(&corner_pin(&pin), 0.3, 0.6);
        assert!(close(
            project(&h, 0.3, 0.6),
            project(&region, mid[0], mid[1])
        ));
        // A non-rectangular area keeps its perspective too.
        let trapezoid = [[0.1, 0.1], [0.4, 0.2], [0.4, 0.8], [0.1, 0.9]];
        let h = compose(&corner_pin(&trapezoid), &IDENTITY);
        assert!(close(project(&h, 1.0, 1.0), trapezoid[2]));
    }

    #[test]
    fn yuv_matrix_maps_black_and_white() {
        for m in [ColorMatrix::Bt601, ColorMatrix::Bt709, ColorMatrix::Bt2020] {
            let rows = yuv_rows(m);
            let eval = |y: f32, u: f32, v: f32| rows.map(|r| r[0] * y + r[1] * u + r[2] * v + r[3]);
            let black = eval(16.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0);
            let white = eval(235.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0);
            for c in black {
                assert!(c.abs() < 1e-4);
            }
            for c in white {
                assert!((c - 1.0).abs() < 1e-4);
            }
        }
    }

    // ----- The shader, run for real --------------------------------------------------

    /// A GPU with no window, if this machine has one (or a software one).
    /// `NEWQ_REQUIRE_GPU` makes having none a failure instead of a skip.
    fn test_gpu() -> Option<Gpu> {
        let try_make = || -> Option<Gpu> {
            let instance = wgpu::Instance::new(
                wgpu::InstanceDescriptor::new_without_display_handle().with_env(),
            );
            let adapter =
                pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    force_fallback_adapter: false,
                    compatible_surface: None,
                    apply_limit_buckets: false,
                }))
                .ok()?;
            let (device, queue) =
                pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                    label: Some("test"),
                    required_features: adapter.features() & wgpu::Features::TEXTURE_COMPRESSION_BC,
                    required_limits:
                        wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
                    ..Default::default()
                }))
                .ok()?;
            let compositor = Arc::new(Compositor::new(&device));
            Some(Gpu {
                lost: DeviceLost::watch(&device),
                instance,
                adapter,
                device,
                queue,
                compositor,
            })
        };
        let gpu = try_make();
        if gpu.is_none() && std::env::var_os("NEWQ_REQUIRE_GPU").is_some() {
            panic!("NEWQ_REQUIRE_GPU is set but there is no GPU adapter");
        }
        gpu
    }

    /// A 4x4 BC1 block of one colour (565), the picture a Hap frame is.
    fn bc1_solid(rgb565: u16) -> [u8; 8] {
        let c = rgb565.to_le_bytes();
        [c[0], c[1], c[0], c[1], 0, 0, 0, 0]
    }

    #[test]
    fn hap_textures_are_drawn_as_the_colours_they_hold() {
        let Some(gpu) = test_gpu() else { return };
        if !gpu
            .device
            .features()
            .contains(wgpu::Features::TEXTURE_COMPRESSION_BC)
        {
            // No block-compressed textures here: Hap plays as other video does.
            return;
        }
        let plain = ColorAdjust::default();
        // BC1: pure red, and a blue.
        let tex = LayerTexture::new_bc(&gpu, 4, 4, HapFormat::Dxt1);
        assert!(tex.is_bc(HapFormat::Dxt1) && tex.is_bc_any() && !tex.is_rgba());
        tex.upload_bc(&gpu.queue, &bc1_solid(0xF800));
        let px = render_tex(&gpu, &tex, (4, 4), 1.0, &plain);
        assert!(
            px[0] >= 250 && px[1] <= 4 && px[2] <= 4 && px[3] == 255,
            "{px:?}"
        );
        tex.upload_bc(&gpu.queue, &bc1_solid(0x001F));
        let px = render_tex(&gpu, &tex, (4, 4), 1.0, &plain);
        assert!(px[2] >= 250 && px[0] <= 4, "{px:?}");
        // BC3 with alpha: half transparent green over black comes out half as bright.
        let tex = LayerTexture::new_bc(&gpu, 4, 4, HapFormat::Dxt5);
        let mut block = [0u8; 16];
        block[0] = 128; // alpha 128 for every pixel (both endpoints the same)
        block[1] = 128;
        block[8..10].copy_from_slice(&0x07E0u16.to_le_bytes());
        block[10..12].copy_from_slice(&0x07E0u16.to_le_bytes());
        tex.upload_bc(&gpu.queue, &block);
        let px = render_tex(&gpu, &tex, (4, 4), 1.0, &plain);
        assert!((i32::from(px[1]) - 128).abs() <= 3 && px[0] <= 3, "{px:?}");
        // Hap Q: YCoCg. Y = 0.5 in alpha, Co = Cg = 0 (128/255 stored), scale 0:
        // a grey of one half.
        let tex = LayerTexture::new_bc(&gpu, 4, 4, HapFormat::Dxt5YCoCg);
        let mut block = [0u8; 16];
        block[0] = 128;
        block[1] = 128;
        // Colour endpoints: red 128/255 and green 128/255 (565: 16/31, 32/63), blue 0.
        let c = (16u16 << 11) | (32 << 5);
        block[8..10].copy_from_slice(&c.to_le_bytes());
        block[10..12].copy_from_slice(&c.to_le_bytes());
        tex.upload_bc(&gpu.queue, &block);
        let px = render_tex(&gpu, &tex, (4, 4), 1.0, &plain);
        for v in &px[..3] {
            assert!((i32::from(*v) - 128).abs() <= 6, "{px:?}");
        }
        // A texture kind is not another's.
        assert!(!tex.is_bc(HapFormat::Dxt1));
    }

    #[test]
    fn a_destroyed_device_is_noticed() {
        let Some(gpu) = test_gpu() else { return };
        assert!(gpu.lost.why().is_none());
        gpu.device.destroy();
        let t0 = std::time::Instant::now();
        while gpu.lost.why().is_none() && t0.elapsed().as_secs() < 5 {
            let _ = gpu.device.poll(wgpu::PollType::Poll);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(gpu.lost.why().is_some_and(|w| w.contains("Destroyed")));
    }

    /// Draw a 2×2 layer of one colour over black through the real compositor
    /// and read back a pixel.
    fn render_solid(gpu: &Gpu, px: [u8; 4], opacity: f32, color: &ColorAdjust) -> [u8; 4] {
        let tex = LayerTexture::new_rgba(gpu, 2, 2);
        tex.upload_rgba(&gpu.queue, &px.repeat(4));
        render_tex(gpu, &tex, (2, 2), opacity, color)
    }

    /// Draw `tex` (a picture of `size`, stretched over a target of that size) and
    /// read back its first pixel.
    fn render_tex(
        gpu: &Gpu,
        tex: &LayerTexture,
        size: (u32, u32),
        opacity: f32,
        color: &ColorAdjust,
    ) -> [u8; 4] {
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("target"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let pipeline = gpu.compositor.pipeline(&gpu.device, format);
        let pin = corner_pin(&[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]);
        let groups = [DrawGroup {
            layers: vec![DrawLayer {
                texture: tex,
                opacity,
                fit: Fit::Stretch,
                transform: LayerTransform::default(),
                color: *color,
                fx: VideoFx::default(),
            }],
            geometry: Geometry {
                canvas: size,
                warp: pin,
                pin,
                mesh: None,
                slice: Geometry::WHOLE,
            },
        }];
        let draw = gpu.composite(&pipeline, Slot::Window(0), &view, &groups, None);
        let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: 256 * u64::from(size.1),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(size.1),
                },
            },
            wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
        );
        gpu.queue.submit([draw, encoder.finish()]);
        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
        gpu.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        let data = slice.get_mapped_range().expect("mapped");
        [data[0], data[1], data[2], data[3]]
    }

    /// What the reference maths says the pixel should come out as over black.
    fn expected(px: [u8; 4], opacity: f32, color: &ColorAdjust) -> [u8; 3] {
        let rgb = [px[0], px[1], px[2]].map(|v| f32::from(v) / 255.0);
        let (out, alpha) = color::apply(rgb, color);
        out.map(|v| (v * alpha * opacity * 255.0).round() as u8)
    }

    #[test]
    fn the_shader_adjusts_and_keys_as_the_reference_maths_says() {
        use crate::show::ChromaKey;
        let Some(gpu) = test_gpu() else {
            eprintln!("no GPU adapter here; skipping the shader test");
            return;
        };
        let adjust = |f: &dyn Fn(&mut ColorAdjust)| {
            let mut c = ColorAdjust::default();
            f(&mut c);
            c
        };
        let green = Some(ChromaKey {
            color: [0, 255, 0],
            tolerance: 0.2,
            softness: 0.1,
            spill: 0.8,
        });
        let cases: Vec<([u8; 4], f32, ColorAdjust)> = vec![
            ([200, 100, 50, 255], 1.0, ColorAdjust::default()),
            ([200, 100, 50, 255], 0.5, ColorAdjust::default()),
            ([200, 100, 50, 255], 1.0, adjust(&|c| c.brightness = 0.2)),
            ([200, 100, 50, 255], 1.0, adjust(&|c| c.contrast = 1.5)),
            ([200, 100, 50, 255], 1.0, adjust(&|c| c.saturation = 0.0)),
            ([200, 100, 50, 255], 1.0, adjust(&|c| c.hue = 120.0)),
            (
                [90, 160, 200, 255],
                1.0,
                adjust(&|c| {
                    c.brightness = -0.1;
                    c.contrast = 1.3;
                    c.saturation = 1.6;
                    c.hue = -40.0;
                }),
            ),
            ([0, 255, 0, 255], 1.0, adjust(&|c| c.key = green)),
            ([230, 160, 130, 255], 1.0, adjust(&|c| c.key = green)),
            ([180, 220, 120, 255], 1.0, adjust(&|c| c.key = green)),
            (
                [60, 190, 70, 255],
                0.8,
                adjust(&|c| {
                    c.key = green;
                    c.saturation = 1.4;
                }),
            ),
        ];
        for (px, opacity, color) in &cases {
            let got = render_solid(&gpu, *px, *opacity, color);
            let want = expected(*px, *opacity, color);
            for i in 0..3 {
                assert!(
                    (i16::from(got[i]) - i16::from(want[i])).abs() <= 3,
                    "{px:?} opacity {opacity} {color:?}: got {got:?}, want {want:?}"
                );
            }
        }
        // And the key really does clear the screen colour: black remains.
        let keyed = render_solid(&gpu, [0, 255, 0, 255], 1.0, &adjust(&|c| c.key = green));
        assert!(keyed[0] < 3 && keyed[1] < 3 && keyed[2] < 3, "{keyed:?}");
    }

    /// Draw a `size`×`size` layer (size a multiple of 64) through `geometry`
    /// over black and read the whole picture back.
    fn render_picture(gpu: &Gpu, pixels: &[u8], size: u32, geometry: Geometry) -> Vec<u8> {
        render_faded(gpu, pixels, size, geometry, None)
    }

    /// A picture drawn with picture effects.
    fn render_fx(gpu: &Gpu, pixels: &[u8], size: u32, fx: VideoFx) -> Vec<u8> {
        render_all(gpu, pixels, size, identity_geometry(size), None, fx)
    }

    /// `render_picture`, with the edges faded.
    fn render_faded(
        gpu: &Gpu,
        pixels: &[u8],
        size: u32,
        geometry: Geometry,
        blend: Option<EdgeBlend>,
    ) -> Vec<u8> {
        render_all(gpu, pixels, size, geometry, blend, VideoFx::default())
    }

    fn render_all(
        gpu: &Gpu,
        pixels: &[u8],
        size: u32,
        geometry: Geometry,
        blend: Option<EdgeBlend>,
        fx: VideoFx,
    ) -> Vec<u8> {
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let tex = LayerTexture::new_rgba(gpu, size, size);
        tex.upload_rgba(&gpu.queue, pixels);
        let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("target"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let pipeline = gpu.compositor.pipeline(&gpu.device, format);
        let groups = [DrawGroup {
            layers: vec![DrawLayer {
                texture: &tex,
                opacity: 1.0,
                fit: Fit::Stretch,
                transform: LayerTransform::default(),
                color: ColorAdjust::default(),
                fx,
            }],
            geometry,
        }];
        let fade = BlendPass::new(&gpu.device, format);
        let faded = blend.map(|b| {
            fade.set(&gpu.queue, &b);
            &fade
        });
        let draw = gpu.composite(&pipeline, Slot::Window(0), &view, &groups, faded);
        let row = size * 4;
        let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: u64::from(row * size),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(size),
                },
            },
            wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
        );
        gpu.queue.submit([draw, encoder.finish()]);
        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
        gpu.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        let data = slice.get_mapped_range().expect("mapped");
        data.to_vec()
    }

    #[test]
    fn the_shader_puts_the_picture_where_the_mesh_maths_says() {
        let Some(gpu) = test_gpu() else {
            eprintln!("no GPU adapter here; skipping the mesh shader test");
            return;
        };
        const SIZE: u32 = 64;
        // A picture whose red is how far across and green how far down.
        let mut pixels = Vec::new();
        for y in 0..SIZE {
            for x in 0..SIZE {
                let (u, v) = (
                    (x as f32 + 0.5) / SIZE as f32,
                    (y as f32 + 0.5) / SIZE as f32,
                );
                pixels.extend_from_slice(&[(u * 255.0) as u8, (v * 255.0) as u8, 0, 255]);
            }
        }
        let identity = corner_pin(&[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]);
        // A mesh with lines in odd places, and bent.
        let mut mesh = Mesh::grid(4, 3);
        mesh.insert_column(0.2).unwrap();
        *mesh.point_mut(2, 1) = [0.62, 0.45];
        *mesh.point_mut(3, 1) = [0.78, 0.55];
        *mesh.point_mut(1, 2) = [0.28, 0.92];
        *mesh.point_mut(0, 0) = [0.04, 0.03];
        assert!(mesh.is_valid());
        let out = render_picture(
            &gpu,
            &pixels,
            SIZE,
            Geometry {
                canvas: (SIZE, SIZE),
                warp: identity,
                pin: identity,
                mesh: Some(Arc::new(mesh.clone())),
                slice: Geometry::WHOLE,
            },
        );
        let at = |x: u32, y: u32| {
            let i = ((y * SIZE + x) * 4) as usize;
            [out[i], out[i + 1], out[i + 2], out[i + 3]]
        };
        let mut checked = 0;
        for (u, v) in [
            (0.15, 0.2),
            (0.4, 0.3),
            (0.5, 0.5),
            (0.7, 0.45),
            (0.85, 0.6),
            (0.3, 0.8),
            (0.6, 0.85),
            (0.9, 0.15),
        ] {
            let d = mesh.eval(u, v);
            let (x, y) = (
                (d[0] * SIZE as f32).floor() as u32,
                (d[1] * SIZE as f32).floor() as u32,
            );
            let px = at(x, y);
            let want = [u * 255.0, v * 255.0];
            assert!(
                (f32::from(px[0]) - want[0]).abs() < 12.0
                    && (f32::from(px[1]) - want[1]).abs() < 12.0
                    && px[3] == 255,
                "picture ({u}, {v}) should be at display {d:?} (pixel {x},{y}) as {want:?}, found {px:?}"
            );
            checked += 1;
        }
        assert_eq!(checked, 8);
        // Outside the mesh's shape, where its corner is pulled in, it is black.
        assert_eq!(at(0, 0)[..3], [0, 0, 0]);
    }

    /// A `SIZE`-square picture whose red is how far across and green how far down.
    fn gradient(size: u32) -> Vec<u8> {
        let mut pixels = Vec::new();
        for y in 0..size {
            for x in 0..size {
                let (u, v) = (
                    (x as f32 + 0.5) / size as f32,
                    (y as f32 + 0.5) / size as f32,
                );
                pixels.extend_from_slice(&[(u * 255.0) as u8, (v * 255.0) as u8, 0, 255]);
            }
        }
        pixels
    }

    fn identity_geometry(size: u32) -> Geometry {
        let identity = corner_pin(&[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]);
        Geometry {
            canvas: (size, size),
            warp: identity,
            pin: identity,
            mesh: None,
            slice: Geometry::WHOLE,
        }
    }

    #[test]
    fn the_edges_fade_as_the_blend_maths_says() {
        let Some(gpu) = test_gpu() else {
            eprintln!("no GPU adapter here; skipping the blend shader test");
            return;
        };
        const SIZE: u32 = 64;
        let white = vec![255u8; (SIZE * SIZE * 4) as usize];
        let blend = EdgeBlend {
            left: 0.25,
            right: 0.1,
            top: 0.2,
            bottom: 0.0,
            gamma: 2.2,
        };
        let out = render_faded(&gpu, &white, SIZE, identity_geometry(SIZE), Some(blend));
        let at = |x: u32, y: u32| out[((y * SIZE + x) * 4) as usize];
        for (x, y) in [
            (0, 32),
            (3, 32),
            (8, 32),
            (15, 32),
            (32, 32),
            (60, 32),
            (63, 32),
            (32, 0),
            (32, 5),
            (32, 63),
            (2, 3),
        ] {
            let want = crate::blend::mask(
                &blend,
                (x as f32 + 0.5) / SIZE as f32,
                (y as f32 + 0.5) / SIZE as f32,
            ) * 255.0;
            let got = f32::from(at(x, y));
            assert!(
                (got - want).abs() <= 3.0,
                "pixel ({x},{y}): {got} but {want}"
            );
        }
        // Alpha is left alone, and no blend leaves the picture as it was.
        assert_eq!(out[3], 255);
        let plain = render_faded(&gpu, &white, SIZE, identity_geometry(SIZE), None);
        assert!(plain.iter().all(|&b| b == 255));
    }

    #[test]
    fn a_display_shows_its_part_of_a_picture_with_or_without_a_mesh() {
        let Some(gpu) = test_gpu() else {
            eprintln!("no GPU adapter here; skipping the slice shader test");
            return;
        };
        const SIZE: u32 = 64;
        let pixels = gradient(SIZE);
        // The part of the picture from 0.5 across for half its width, all its height.
        let area = [0.5f32, 0.0, 0.5, 1.0];
        let part = [
            [1.0 / area[2], 0.0, -area[0] / area[2], 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ];
        for meshed in [false, true] {
            let mut g = identity_geometry(SIZE);
            if meshed {
                g.mesh = Some(Arc::new(Mesh::grid(3, 3)));
                g.slice = [area[0], area[1], 1.0 / area[2], 1.0 / area[3]];
            } else {
                g.pin = part;
                g.warp = part;
            }
            let out = render_picture(&gpu, &pixels, SIZE, g);
            for (x, y) in [(2u32, 10u32), (20, 30), (40, 50), (60, 5)] {
                let i = ((y * SIZE + x) * 4) as usize;
                // The display's pixel x is at 0.5 + 0.5 * x / SIZE of the picture.
                let want_u = area[0] + area[2] * (x as f32 + 0.5) / SIZE as f32;
                let want_v = (y as f32 + 0.5) / SIZE as f32;
                let (r, g) = (f32::from(out[i]), f32::from(out[i + 1]));
                assert!(
                    (r - want_u * 255.0).abs() <= 8.0 && (g - want_v * 255.0).abs() <= 8.0,
                    "mesh={meshed} pixel ({x},{y}): {r},{g} but {},{}",
                    want_u * 255.0,
                    want_v * 255.0
                );
            }
        }
    }

    /// Stream a red picture for 12 frames through real FFmpeg, drawing each
    /// frame `slow` longer than it needs, and check the last frame is red.
    fn stream_a_red_picture(slow: std::time::Duration) {
        use crate::media::ffmpeg::Ffmpeg;
        use crate::render::{CaptureThread, OutputGroup, OutputLayer};
        use crate::show::StreamKind;
        let Some(gpu) = test_gpu() else {
            eprintln!("no GPU adapter here; skipping the stream test");
            return;
        };
        let Some(ff) = Ffmpeg::locate() else {
            if std::env::var_os("NEWQ_REQUIRE_MEDIA").is_some() {
                panic!("NEWQ_REQUIRE_MEDIA is set but ffmpeg was not found");
            }
            eprintln!("no ffmpeg; skipping the stream test");
            return;
        };
        let dir = std::env::temp_dir().join(format!("newq_test_stream_{}", slow.as_millis()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("out.rgba");
        let _ = std::fs::remove_file(&out);
        let size = (64u32, 36u32);
        let kind = StreamKind::Custom {
            args: format!(
                "-frames:v 12 -f rawvideo -pix_fmt rgba \"{}\"",
                out.display()
            ),
        };
        let thread =
            CaptureThread::spawn_slow(gpu.clone(), Slot::Capture(0), size, 30, ff, kind, slow);
        // A red picture, drawn at the stream's size from a 2×2 layer.
        let tex = LayerTexture::new_rgba(&gpu, 2, 2);
        tex.upload_rgba(&gpu.queue, &[255, 0, 0, 255].repeat(4));
        let identity = corner_pin(&[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]);
        thread.submit(vec![OutputGroup {
            layers: vec![OutputLayer {
                texture: Arc::new(tex),
                opacity: 1.0,
                fit: Fit::Stretch,
                transform: LayerTransform::default(),
                color: ColorAdjust::default(),
                fx: VideoFx::default(),
            }],
            geometry: Geometry {
                canvas: (1920, 1080),
                warp: identity,
                pin: identity,
                mesh: None,
                slice: Geometry::WHOLE,
            },
        }]);
        let want = (size.0 * size.1 * 4 * 12) as u64;
        let t0 = std::time::Instant::now();
        while std::fs::metadata(&out).map_or(0, |m| m.len()) < want {
            assert!(
                t0.elapsed().as_secs() < 15,
                "no frames arrived (failure: {:?})",
                thread.failure()
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        thread.stop();
        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(bytes.len() as u64, want, "twelve whole frames");
        // The last frame is the red picture, all over.
        let last = &bytes[bytes.len() - (size.0 * size.1 * 4) as usize..];
        for px in last.chunks(4).step_by(97) {
            assert_eq!(px, [255, 0, 0, 255], "{px:?}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_stream_sends_the_picture_to_ffmpeg() {
        stream_a_red_picture(std::time::Duration::ZERO);
    }

    /// Drawing slower than a frame lasts leaves the loop no time to wait, and
    /// it once never took up the picture, so the stream stayed black.
    #[test]
    fn a_stream_still_takes_up_its_picture_when_drawing_is_slower_than_a_frame() {
        stream_a_red_picture(std::time::Duration::from_millis(90));
    }

    #[test]
    fn a_stream_that_ffmpeg_refuses_is_reported() {
        use crate::media::ffmpeg::Ffmpeg;
        use crate::render::CaptureThread;
        use crate::show::StreamKind;
        let Some(gpu) = test_gpu() else { return };
        let Some(ff) = Ffmpeg::locate() else { return };
        let size = (64u32, 36u32);
        let kind = StreamKind::Custom {
            args: "-f no_such_format nowhere".into(),
        };
        // FFmpeg may refuse at once or as soon as a frame arrives; either way
        // the reason comes back.
        let thread = CaptureThread::spawn(gpu, Slot::Capture(1), size, 30, ff, kind);
        let t0 = std::time::Instant::now();
        let reason = loop {
            if let Some(why) = thread.failure() {
                break why;
            }
            assert!(t0.elapsed().as_secs() < 15, "nothing was reported");
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert!(!reason.is_empty());
        assert!(
            !reason.contains("Broken pipe") || reason.contains("FFmpeg"),
            "{reason}"
        );
    }

    /// A picture of `size` square: black on the left half, white on the right.
    fn half_and_half(size: u32) -> Vec<u8> {
        let mut pixels = Vec::new();
        for _y in 0..size {
            for x in 0..size {
                let v = if x < size / 2 { 0 } else { 255 };
                pixels.extend_from_slice(&[v, v, v, 255]);
            }
        }
        pixels
    }

    fn px(out: &[u8], size: u32, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * size + x) * 4) as usize;
        [out[i], out[i + 1], out[i + 2]]
    }

    #[test]
    fn blur_softens_an_edge_and_leaves_flat_colour_alone() {
        let Some(gpu) = test_gpu() else {
            eprintln!("no GPU adapter here; skipping the effects shader test");
            return;
        };
        const SIZE: u32 = 64;
        let plain = render_fx(&gpu, &half_and_half(SIZE), SIZE, VideoFx::default());
        assert_eq!(px(&plain, SIZE, SIZE / 2 - 1, 30)[0], 0);
        assert_eq!(px(&plain, SIZE, SIZE / 2, 30)[0], 255);
        let fx = VideoFx {
            blur: 10.0,
            ..Default::default()
        };
        let blurred = render_fx(&gpu, &half_and_half(SIZE), SIZE, fx);
        // Either side of the edge is now grey, in between the two.
        let (a, b) = (
            px(&blurred, SIZE, SIZE / 2 - 2, 30)[0],
            px(&blurred, SIZE, SIZE / 2 + 1, 30)[0],
        );
        assert!(a > 20 && a < 140 && b > 120 && b < 235 && a < b, "{a} {b}");
        // Well away from the edge it is as it was, and the middle is half way.
        assert!(px(&blurred, SIZE, 4, 30)[0] < 6);
        assert!(px(&blurred, SIZE, SIZE - 5, 30)[0] > 249);
        // Flat colour stays flat and the same colour.
        let flat: Vec<u8> = [90u8, 140, 200, 255].repeat((SIZE * SIZE) as usize);
        let out = render_fx(&gpu, &flat, SIZE, fx);
        for (x, y) in [(2, 2), (32, 32), (60, 10)] {
            let p = px(&out, SIZE, x, y);
            assert!(
                (i32::from(p[0]) - 90).abs() <= 2 && (i32::from(p[2]) - 200).abs() <= 2,
                "{p:?}"
            );
        }
    }

    #[test]
    fn sharpen_pushes_edges_apart_and_pixelate_makes_blocks() {
        let Some(gpu) = test_gpu() else {
            eprintln!("no GPU adapter here; skipping the effects shader test");
            return;
        };
        const SIZE: u32 = 64;
        // A grey step: dark grey then light grey.
        let mut step = Vec::new();
        for _y in 0..SIZE {
            for x in 0..SIZE {
                let v = if x < SIZE / 2 { 100 } else { 150 };
                step.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let fx = VideoFx {
            sharpen: 1.0,
            ..Default::default()
        };
        let out = render_fx(&gpu, &step, SIZE, fx);
        let (dark, light) = (
            px(&out, SIZE, SIZE / 2 - 1, 30)[0],
            px(&out, SIZE, SIZE / 2, 30)[0],
        );
        assert!(dark < 92 && light > 158, "overshoots: {dark} {light}");
        assert_eq!(px(&out, SIZE, 10, 30)[0], 100, "flat parts are unchanged");
        // Blocks of 16: every pixel in a block is the same, and different
        // blocks of a gradient differ.
        let ramp = gradient(SIZE);
        let fx = VideoFx {
            pixel: 16.0,
            ..Default::default()
        };
        let out = render_fx(&gpu, &ramp, SIZE, fx);
        assert_eq!(px(&out, SIZE, 17, 17), px(&out, SIZE, 30, 30));
        assert_ne!(px(&out, SIZE, 5, 5), px(&out, SIZE, 50, 5));
    }

    #[test]
    fn negative_and_vignette_are_what_the_reference_maths_says() {
        let Some(gpu) = test_gpu() else {
            eprintln!("no GPU adapter here; skipping the effects shader test");
            return;
        };
        const SIZE: u32 = 64;
        let flat: Vec<u8> = [200u8, 100, 50, 255].repeat((SIZE * SIZE) as usize);
        let fx = VideoFx {
            negative: 1.0,
            ..Default::default()
        };
        let out = render_fx(&gpu, &flat, SIZE, fx);
        let p = px(&out, SIZE, 20, 20);
        assert!(
            (i32::from(p[0]) - 55).abs() <= 1
                && (i32::from(p[1]) - 155).abs() <= 1
                && (i32::from(p[2]) - 205).abs() <= 1,
            "{p:?}"
        );
        // Half way between: halfway to the inverse.
        let half = render_fx(
            &gpu,
            &flat,
            SIZE,
            VideoFx {
                negative: 0.5,
                ..Default::default()
            },
        );
        assert!((i32::from(px(&half, SIZE, 20, 20)[0]) - 127).abs() <= 2);
        // The vignette: the middle clear, the corners as the maths says.
        let white: Vec<u8> = [255u8; 4].repeat((SIZE * SIZE) as usize);
        let fx = VideoFx {
            vignette: 0.8,
            softness: 0.5,
            ..Default::default()
        };
        let out = render_fx(&gpu, &white, SIZE, fx);
        assert_eq!(px(&out, SIZE, 32, 32)[0], 255);
        for (x, y) in [(0u32, 0u32), (5, 40), (60, 20), (63, 63)] {
            let c = [
                (x as f32 + 0.5) / SIZE as f32,
                (y as f32 + 0.5) / SIZE as f32,
            ];
            let want = crate::fx::vignette_factor(c, 0.8, 0.5) * 255.0;
            let got = f32::from(px(&out, SIZE, x, y)[0]);
            assert!((got - want).abs() <= 3.0, "({x},{y}) {got} vs {want}");
        }
    }
}
