//! GPU setup and the output compositor, which draws video/image layers with
//! opacity onto output windows.

use crate::media::ffmpeg::ColorMatrix;
use crate::show::{Fit, LayerTransform};
use anyhow::{Result, anyhow};
use std::sync::Arc;
use winit::window::Window;

/// Shared GPU context. Cheap to clone; render threads each hold a copy.
#[derive(Clone)]
pub struct Gpu {
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
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("newq"),
                required_limits:
                    wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
                ..Default::default()
            }))?;
        let compositor = Arc::new(Compositor::new(&device));
        Ok((
            Gpu {
                instance,
                adapter,
                device,
                queue,
                compositor,
            },
            surface,
        ))
    }

    /// A surface configuration in a non-sRGB format, so that blending (fades and
    /// crossfades) happens in gamma space like a video mixer.
    pub fn surface_config(
        &self,
        surface: &wgpu::Surface,
        size: (u32, u32),
    ) -> wgpu::SurfaceConfiguration {
        let caps = surface.get_capabilities(&self.adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .unwrap_or(caps.formats[0]);
        wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.0.max(1),
            height: size.1.max(1),
            // Each window presents from its own thread, so blocking on vsync is fine.
            present_mode: wgpu::PresentMode::AutoVsync,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            color_space: wgpu::SurfaceColorSpace::Auto,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct LayerUniform {
    /// x0, y0 (top-left), x1, y1 (bottom-right) in NDC.
    rect: [f32; 4],
    opacity: f32,
    mode: u32,
    _pad: [f32; 2],
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
}

const SHADER: &str = r#"
struct Params {
    rect: vec4<f32>,
    opacity: f32,
    mode: u32,
    pad: vec2<f32>,
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

@vertex
fn vs(@builtin(vertex_index) i: u32) -> VOut {
    var corners = array<vec2<f32>, 4>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 1.0));
    let c = corners[i];
    let n0 = vec2<f32>(mix(p.rect.x, p.rect.z, c.x), mix(p.rect.y, p.rect.w, c.y));
    let ndc = vec2<f32>(
        p.x0.x * n0.x + p.x0.y * n0.y + p.x0.z,
        p.x1.x * n0.x + p.x1.y * n0.y + p.x1.z);
    let cu = vec3<f32>((ndc.x + 1.0) * 0.5, (1.0 - ndc.y) * 0.5, 1.0);
    // Projective transform; the GPU's perspective divide by w (with
    // perspective-correct interpolation) does the rest.
    let q = vec3<f32>(dot(p.h0.xyz, cu), dot(p.h1.xyz, cu), dot(p.h2.xyz, cu));
    var o: VOut;
    o.pos = vec4<f32>(2.0 * q.x - q.z, q.z - 2.0 * q.y, 0.0, q.z);
    o.uv = mix(p.uvr.xy, p.uvr.zw, c);
    o.canvas = cu.xy;
    o.display = vec3<f32>(dot(p.c0.xyz, cu), dot(p.c1.xyz, cu), dot(p.c2.xyz, cu));
    return o;
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
    var rgba: vec4<f32>;
    if (p.mode == 0u) {
        rgba = textureSample(t0, s, in.uv);
    } else {
        let yuv = vec4<f32>(
            textureSample(t0, s, in.uv).r,
            textureSample(t1, s, in.uv).r,
            textureSample(t2, s, in.uv).r,
            1.0);
        rgba = vec4<f32>(clamp(vec3<f32>(dot(p.r, yuv), dot(p.g, yuv), dot(p.b, yuv)),
                               vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
    }
    let a = rgba.a * p.opacity;
    return vec4<f32>(rgba.rgb * a, a);
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
#[cfg(test)]
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
                topology: wgpu::PrimitiveTopology::TriangleStrip,
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

enum Planes {
    Yuv([wgpu::Texture; 3]),
    Rgba(wgpu::Texture),
}

/// Who is drawing a layer. An output window and its previews in the main
/// window draw the same layer at different sizes and warps from different
/// threads, so each gets its own uniform buffer (placement) and bind group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Output = 0,
    /// The output's own preview.
    Preview = 1,
    /// The preview of a whole physical display, with its sub displays.
    Screen = 2,
}

/// GPU textures for one playing layer, plus per-[`Slot`] uniforms.
pub struct LayerTexture {
    planes: Planes,
    pub width: u32,
    pub height: u32,
    matrix: ColorMatrix,
    slots: [(wgpu::Buffer, wgpu::BindGroup); 3],
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
            slots: [slot(), slot(), slot()],
        }
    }

    pub fn is_yuv(&self) -> bool {
        matches!(self.planes, Planes::Yuv(_))
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
        fit: Fit,
        transform: &LayerTransform,
    ) {
        let rows = yuv_rows(self.matrix);
        let place = layer_placement(
            fit_rect((self.width, self.height), geometry.canvas, fit),
            geometry.canvas,
            transform,
        );
        let u = LayerUniform {
            rect: place.rect,
            opacity,
            mode: u32::from(self.is_yuv()),
            _pad: [0.0; 2],
            r: rows[0],
            g: rows[1],
            b: rows[2],
            h: geometry.warp,
            pin: geometry.pin,
            xf: place.xf,
            uvr: place.uvr,
        };
        queue.write_buffer(&self.slots[slot as usize].0, 0, bytemuck::bytes_of(&u));
    }

    /// Draw for `slot` with the compositor pipeline already set on `pass`.
    pub fn draw(&self, slot: Slot, pass: &mut wgpu::RenderPass) {
        pass.set_bind_group(0, &self.slots[slot as usize].1, &[]);
        pass.draw(0..4, 0..1);
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

pub struct DrawLayer<'a> {
    pub texture: &'a LayerTexture,
    pub opacity: f32,
    pub fit: Fit,
    pub transform: LayerTransform,
}

/// Where an output's picture (its canvas) lands on a screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    /// Pixel size of the output's canvas, which layers are fitted to.
    pub canvas: (u32, u32),
    /// Maps the canvas onto the screen (sub display area and corner pin).
    pub warp: Homography,
    /// The output's own corner pin, onto its display or sub display,
    /// whose edges clip it.
    pub pin: Homography,
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
    /// Draw layers (bottom to top) over black into `target`.
    pub fn composite(
        &self,
        pipeline: &wgpu::RenderPipeline,
        target: &wgpu::TextureView,
        groups: &[DrawGroup],
    ) -> wgpu::CommandBuffer {
        for g in groups {
            for l in &g.layers {
                l.texture.write_uniform(
                    &self.queue,
                    Slot::Output,
                    &g.geometry,
                    l.opacity,
                    l.fit,
                    &l.transform,
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
                    l.texture.draw(Slot::Output, &mut pass);
                }
            }
        }
        encoder.finish()
    }
}

/// Generate an RGBA test pattern (colour bars, grid, centre cross and a big
/// output number) for identifying displays.
pub fn test_pattern(w: u32, h: u32, number: usize) -> Vec<u8> {
    const BARS: [[u8; 3]; 7] = [
        [192, 192, 192],
        [192, 192, 0],
        [0, 192, 192],
        [0, 192, 0],
        [192, 0, 192],
        [192, 0, 0],
        [0, 0, 192],
    ];
    let mut px = vec![0u8; (w * h * 4) as usize];
    let digits: Vec<u32> = number
        .to_string()
        .chars()
        .filter_map(|c| c.to_digit(10))
        .collect();
    // As tall as fits, also on narrow sub displays.
    let n = digits.len() as f32;
    let dh = (h as f32 * 0.4).min(w as f32 * 0.7 / (n * 0.55 + (n - 1.0).max(0.0) * 0.165));
    let dw = dh * 0.55;
    let gap = dw * 0.3;
    let total = digits.len() as f32 * dw + (digits.len().saturating_sub(1)) as f32 * gap;
    let x0 = (w as f32 - total) / 2.0;
    let y0 = (h as f32 - dh) / 2.0;
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let mut c = BARS[(x * 7 / w) as usize];
            if x % (w / 16).max(1) == 0 || y % (h / 9).max(1) == 0 {
                c = [255, 255, 255];
            }
            if x < 4 || y < 4 || x >= w - 4 || y >= h - 4 {
                c = [255, 0, 0];
            }
            let (fx, fy) = (x as f32, y as f32);
            for (n, d) in digits.iter().enumerate() {
                let dx = x0 + n as f32 * (dw + gap);
                if fx >= dx - dw * 0.15
                    && fx < dx + dw * 1.15
                    && fy >= y0 - dw * 0.15
                    && fy < y0 + dh + dw * 0.15
                {
                    c = [0, 0, 0];
                    if seven_segment(*d, (fx - dx) / dw, (fy - y0) / dh) {
                        c = [255, 255, 255];
                    }
                }
            }
            px[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }
    px
}

fn seven_segment(digit: u32, u: f32, v: f32) -> bool {
    // Segments: a (top), b (top-right), c (bottom-right), d (bottom), e (bottom-left),
    // f (top-left), g (middle).
    const SEGS: [u8; 10] = [
        0b0111111, 0b0000110, 0b1011011, 0b1001111, 0b1100110, 0b1101101, 0b1111101, 0b0000111,
        0b1111111, 0b1101111,
    ];
    let on = SEGS[digit as usize % 10];
    let t = 0.16; // stroke thickness relative to width
    let tv = t * 0.55; // ... and relative to height
    let horiz = |cy: f32| (0.0..=1.0).contains(&u) && (v - cy).abs() < tv / 2.0;
    let vert = |cx: f32, y0: f32, y1: f32| (u - cx).abs() < t / 2.0 && v >= y0 && v <= y1;
    let segs = [
        horiz(tv / 2.0),
        vert(1.0 - t / 2.0, 0.0, 0.5),
        vert(1.0 - t / 2.0, 0.5, 1.0),
        horiz(1.0 - tv / 2.0),
        vert(t / 2.0, 0.5, 1.0),
        vert(t / 2.0, 0.0, 0.5),
        horiz(0.5),
    ];
    segs.iter()
        .enumerate()
        .any(|(i, &hit)| hit && on & (1 << i) != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
