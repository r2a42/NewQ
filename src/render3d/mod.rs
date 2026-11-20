//! The 3D stage's renderer (0.25.0 previz, `.claude/notes/stage3d-plan.md`):
//! draws a `Scene` of neutral parts (`scene.rs`) seen by a `Camera` into any
//! texture view, with a depth buffer.
//!
//! - Lit parts (the venue): a headlight from the camera, a soft sky/ground
//!   ambient, Lambert shading of the part's colour or picture, both faces lit,
//!   a "lift" so a dark room stays readable, optional fog, and an x-ray
//!   (`Options::venue_opacity`) that lets the screens show through walls.
//! - Unlit parts (the screens): a display's picture, `Paint::Output`, at full
//!   brightness, both faces; a display with no picture yet is dark grey with
//!   a thin edge.
//! - An optional floor grid (1 m lines, stronger every 5 m) and the world's
//!   axes at the origin.
//!
//! `camera.rs` is the pure camera maths (`Orbit`, zoom to fit) and `pick.rs`
//! the click-to-select ray test.
// Not wired into the app yet: the integration (the Preview pane's 3D mode,
// the 3D view as a display source) uses this API.
#![allow(dead_code, unused_imports)]

pub mod camera;
pub mod maths;
pub mod pick;
pub mod scene;
#[cfg(test)]
mod tests;

pub use camera::{Bounds, Orbit, Ray, scene_bounds};
pub use pick::{Hit, pick};
pub use scene::{Camera, Mesh, Paint, Part, Scene};

use crate::gpu::Gpu;
use crate::show::OutputId;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use wgpu::util::DeviceExt;

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Meshes and pictures not drawn for this long are let go.
pub const FORGET_AFTER: Duration = Duration::from_secs(10);
/// How often the caches are looked over (when something is drawn).
const TIDY_EVERY: Duration = Duration::from_secs(2);
/// The largest picture side kept for a `Paint::Image`; bigger ones are
/// shrunk on the way in (a preview has no use for more).
const MAX_IMAGE_SIDE: u32 = 4096;

/// How much drawing a render may cost: what a preview governor turns down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quality {
    /// 4× multisampling, where the adapter offers it for the target's format.
    pub msaa: bool,
    /// The picture is drawn at this fraction of the target's size (0.1 to 1)
    /// and scaled up.
    pub scale: f32,
}

impl Default for Quality {
    fn default() -> Quality {
        Quality {
            msaa: true,
            scale: 1.0,
        }
    }
}

impl Quality {
    /// The cheapest: no multisampling, a quarter of the pixels.
    pub const LOW: Quality = Quality {
        msaa: false,
        scale: 0.5,
    };

    fn scale(&self) -> f32 {
        if self.scale.is_finite() {
            self.scale.clamp(0.1, 1.0)
        } else {
            1.0
        }
    }

    /// This quality with its scale multiplied by `governor` (a preview
    /// governor's 1, 0.5 or 0.25 that frame), and multisampling dropped below
    /// full scale. A preview passes its own quality through this; a 3D view
    /// that is a display's source (an output, not a preview) never does.
    pub fn scaled_by(self, governor: f32) -> Quality {
        let g = if governor.is_finite() {
            governor.clamp(0.0, 1.0)
        } else {
            1.0
        };
        Quality {
            msaa: self.msaa && g >= 1.0,
            scale: self.scale() * g,
        }
    }

    /// The size drawn for a target of `size`.
    pub fn drawn_size(&self, size: (u32, u32)) -> (u32, u32) {
        let s = self.scale();
        let f = |v: u32| ((v as f32 * s).round() as u32).max(1).min(v.max(1));
        (f(size.0), f(size.1))
    }
}

/// How the view looks. Colours are display values (0..1, as the outputs'
/// pictures are).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    pub quality: Quality,
    pub grid: bool,
    /// Half the width of the floor grid, metres (it fades out towards it).
    pub grid_extent: f32,
    pub axes: bool,
    /// The axes' length, metres.
    pub axes_length: f32,
    pub background: [f32; 3],
    /// Light from the camera.
    pub headlight: f32,
    /// Light from all round, from `sky` above to `ground` below.
    pub ambient: f32,
    pub sky: [f32; 3],
    pub ground: [f32; 3],
    /// The lit parts' brightness (1 as is).
    pub brightness: f32,
    /// Grey added to a lit part's colour, the more the darker it is (0 none,
    /// 1 at most): a black-box venue stays readable.
    pub lift: f32,
    /// The lit parts' opacity: below 1 the venue is see-through ("x-ray"),
    /// drawn after the screens, so they show through its walls.
    pub venue_opacity: f32,
    /// Fog's density per metre (0, the default, is none); towards
    /// `background`.
    pub fog: f32,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            quality: Quality::default(),
            grid: false,
            grid_extent: 50.0,
            axes: false,
            axes_length: 1.0,
            background: [0.06, 0.065, 0.075],
            headlight: 0.75,
            ambient: 0.45,
            sky: [1.0, 1.0, 1.0],
            ground: [0.55, 0.52, 0.5],
            brightness: 1.0,
            lift: 0.2,
            venue_opacity: 1.0,
            fog: 0.0,
        }
    }
}

/// Where to draw: a view of a texture with `RENDER_ATTACHMENT` usage, its
/// format and size in pixels.
pub struct Target<'a> {
    pub view: &'a wgpu::TextureView,
    pub format: wgpu::TextureFormat,
    pub size: (u32, u32),
    /// Whose picture it is (a view, a display): the depth and multisampled
    /// textures are kept per owner, and an owner's are let go when its size
    /// changes (a pane being resized makes no pile of them).
    pub owner: u64,
}

/// What a render drew, for the status line.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameStats {
    pub draw_calls: u32,
    pub triangles: u64,
    /// Parts drawn (those with no triangles, or not where they can be, aren't).
    pub parts: u32,
    /// `Paint::Output` parts whose display had no picture.
    pub missing_pictures: u32,
    /// 4 with multisampling, else 1.
    pub samples: u32,
    /// The size actually drawn (before scaling up to the target).
    pub drawn: (u32, u32),
    pub meshes_cached: usize,
    pub images_cached: usize,
    /// The processor time `render` took (making the frame and submitting it;
    /// the graphics card's own time is not measured).
    pub cpu_time: std::time::Duration,
}

impl FrameStats {
    /// Samples shaded: the drawn size times the multisampling, a rough
    /// measure of the card's work for the frame beside `triangles`.
    pub fn samples_shaded(&self) -> u64 {
        u64::from(self.drawn.0) * u64::from(self.drawn.1) * u64::from(self.samples.max(1))
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct FrameUniform {
    view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    light: [f32; 4],
    sky: [f32; 4],
    ground: [f32; 4],
    fog: [f32; 4],
    extra: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct PartUniform {
    model: [[f32; 4]; 4],
    n: [[f32; 4]; 3],
    colour: [f32; 4],
    region: [f32; 4],
    flags: [f32; 4],
}

const SHOWS_COLOUR: f32 = 0.0;
const SHOWS_PICTURE: f32 = 1.0;
const SHOWS_MISSING: f32 = 2.0;

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
    /// For noticing a key reused for another mesh.
    vertex_count: usize,
    used: Instant,
}

struct GpuImage {
    _texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    size: (u32, u32),
    used: Instant,
}

/// The pipelines for one target format and sample count.
struct Pipelines {
    opaque: wgpu::RenderPipeline,
    see_through: wgpu::RenderPipeline,
    grid: wgpu::RenderPipeline,
    axes: wgpu::RenderPipeline,
}

/// The textures drawn into for one owner, size, format and sample count.
type TargetKey = (u64, (u32, u32), wgpu::TextureFormat, u32, bool);

struct Targets {
    used: Instant,
    /// The size of the target they were for (the drawn size is smaller
    /// when a governor scales the view).
    full: (u32, u32),
    depth: wgpu::TextureView,
    /// Multisampled colour, resolved into the target (or `small`).
    msaa: Option<wgpu::TextureView>,
    /// The picture drawn smaller, then scaled up into the target.
    small: Option<(wgpu::TextureView, wgpu::BindGroup)>,
}

/// Draws scenes on one GPU device. Build one per `Gpu` and keep it: the
/// pipelines, meshes and pictures it makes are kept for the next render.
pub struct Renderer {
    frame_layout: wgpu::BindGroupLayout,
    part_layout: wgpu::BindGroupLayout,
    tex_layout: wgpu::BindGroupLayout,
    parts_pipeline_layout: wgpu::PipelineLayout,
    frame_pipeline_layout: wgpu::PipelineLayout,
    blit_pipeline_layout: wgpu::PipelineLayout,
    shader: wgpu::ShaderModule,
    blit_shader: wgpu::ShaderModule,
    /// Repeats (a venue's textures tile).
    repeat: wgpu::Sampler,
    /// Clamps (a display's picture, and scaling up).
    clamp: wgpu::Sampler,
    frame_buffer: wgpu::Buffer,
    frame_group: wgpu::BindGroup,
    part_buffer: wgpu::Buffer,
    part_group: wgpu::BindGroup,
    part_capacity: u64,
    part_stride: u64,
    /// A white pixel, for a part painted with a colour.
    white: wgpu::BindGroup,
    pipelines: HashMap<(wgpu::TextureFormat, u32), Pipelines>,
    blits: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    /// By size, format, sample count and scaling: several views (preview
    /// panes, a display source) can share one renderer.
    targets: HashMap<TargetKey, Targets>,
    meshes: HashMap<u64, GpuMesh>,
    images: HashMap<u64, GpuImage>,
    /// The largest buffer the device takes (`max_buffer_size`; smaller in
    /// a test).
    max_buffer: u64,
    /// Meshes too big for it, by key: skipped (and said once), never made
    /// again each render.
    too_big: std::collections::HashSet<u64>,
    /// When the caches were last looked over.
    tidied: Instant,
    last: FrameStats,
}

impl Renderer {
    pub fn new(gpu: &Gpu) -> Renderer {
        let device = &gpu.device;
        let uniform = |binding, visibility, dynamic, size: usize| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: dynamic,
                min_binding_size: wgpu::BufferSize::new(size as u64),
            },
            count: None,
        };
        let vf = wgpu::ShaderStages::VERTEX_FRAGMENT;
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("3d frame"),
            entries: &[uniform(0, vf, false, size_of::<FrameUniform>())],
        });
        let part_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("3d part"),
            entries: &[uniform(0, vf, true, size_of::<PartUniform>())],
        });
        let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("3d picture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let parts_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("3d parts"),
                bind_group_layouts: &[Some(&frame_layout), Some(&part_layout), Some(&tex_layout)],
                immediate_size: 0,
            });
        let frame_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("3d grid"),
                bind_group_layouts: &[Some(&frame_layout)],
                immediate_size: 0,
            });
        let blit_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("3d scale up"),
            bind_group_layouts: &[Some(&tex_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("3d"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("3d scale up"),
            source: wgpu::ShaderSource::Wgsl(include_str!("blit.wgsl").into()),
        });
        let sampler = |label, mode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                address_mode_u: mode,
                address_mode_v: mode,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            })
        };
        let repeat = sampler("3d repeat", wgpu::AddressMode::Repeat);
        let clamp = sampler("3d clamp", wgpu::AddressMode::ClampToEdge);
        let frame_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("3d frame"),
            size: size_of::<FrameUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let frame_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("3d frame"),
            layout: &frame_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: frame_buffer.as_entire_binding(),
            }],
        });
        let part_stride = u64::from(device.limits().min_uniform_buffer_offset_alignment)
            .max(size_of::<PartUniform>() as u64)
            .next_multiple_of(256);
        let part_capacity = 64;
        let (part_buffer, part_group) =
            Self::part_buffer(device, &part_layout, part_stride, part_capacity);
        let white_tex = device.create_texture_with_data(
            &gpu.queue,
            &wgpu::TextureDescriptor {
                label: Some("3d white"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &[255; 4],
        );
        let white = Self::picture_group(
            device,
            &tex_layout,
            &white_tex.create_view(&Default::default()),
            &clamp,
        );
        Renderer {
            frame_layout,
            part_layout,
            tex_layout,
            parts_pipeline_layout,
            frame_pipeline_layout,
            blit_pipeline_layout,
            shader,
            blit_shader,
            repeat,
            clamp,
            frame_buffer,
            frame_group,
            part_buffer,
            part_group,
            part_capacity,
            part_stride,
            white,
            pipelines: HashMap::new(),
            blits: HashMap::new(),
            targets: HashMap::new(),
            meshes: HashMap::new(),
            images: HashMap::new(),
            max_buffer: device.limits().max_buffer_size,
            too_big: std::collections::HashSet::new(),
            tidied: Instant::now(),
            last: FrameStats::default(),
        }
    }

    /// What the last render drew.
    pub fn last_stats(&self) -> FrameStats {
        self.last
    }

    /// The multisampling a target of `format` gets with `msaa` asked for: 4
    /// where the adapter can draw and resolve that format and the depth
    /// buffer at 4×, else 1.
    pub fn samples_for(gpu: &Gpu, format: wgpu::TextureFormat, msaa: bool) -> u32 {
        if !msaa {
            return 1;
        }
        let specific = gpu
            .device
            .features()
            .contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES);
        let flags = |f: wgpu::TextureFormat| {
            let offered = gpu.adapter.get_texture_format_features(f).flags;
            if specific {
                offered
            } else {
                offered & f.guaranteed_format_features(gpu.device.features()).flags
            }
        };
        let colour = flags(format);
        let ok = colour.sample_count_supported(4)
            && colour.contains(wgpu::TextureFormatFeatureFlags::MULTISAMPLE_RESOLVE)
            && flags(DEPTH_FORMAT).sample_count_supported(4);
        if ok { 4 } else { 1 }
    }

    /// Draw `scene` as `camera` sees it into `target`, and submit (through
    /// `Gpu::submit`). `outputs` gives a display's current picture (a view of
    /// a filterable colour texture with `TEXTURE_BINDING` usage, not the
    /// target); a display with none draws as a dark grey panel.
    ///
    /// A frame not wanted (a preview governor's "don't draw") is simply not
    /// rendered: nothing here runs by itself, so it costs nothing, and the
    /// target keeps the last picture drawn into it.
    ///
    /// Cheap at any size: the depth and multisampled textures are kept for
    /// the next render of the same size, the pipelines per format, the
    /// meshes by `Part::mesh_key` and pictures by `Paint::Image::key`.
    pub fn render<'a>(
        &mut self,
        gpu: &Gpu,
        target: &Target<'_>,
        scene: &Scene,
        camera: &Camera,
        outputs: &dyn Fn(OutputId) -> Option<&'a wgpu::TextureView>,
        options: &Options,
    ) -> FrameStats {
        let started = Instant::now();
        if self.tidied.elapsed() >= TIDY_EVERY {
            self.tidy();
        }
        if target.size.0 == 0 || target.size.1 == 0 {
            self.last = FrameStats::default();
            return self.last;
        }
        let device = &gpu.device;
        let drawn = options.quality.drawn_size(target.size);
        let scaled = drawn != target.size;
        let samples = Self::samples_for(gpu, target.format, options.quality.msaa);
        let target_key = self.ensure_targets(device, target, drawn, samples, scaled);
        self.ensure_pipelines(device, target.format, samples);
        if scaled {
            self.ensure_blit(device, target.format);
        }
        let srgb = target.format.is_srgb();

        // The frame's numbers.
        let cam = camera.sane();
        let aspect = drawn.0 as f32 / drawn.1 as f32;
        let num = |v: f32, or: f32| if v.is_finite() { v } else { or };
        let frame = FrameUniform {
            view_proj: cam.view_projection(aspect),
            eye: [
                cam.eye[0],
                cam.eye[1],
                cam.eye[2],
                num(options.fog, 0.0).max(0.0),
            ],
            light: [
                num(options.headlight, 0.75).max(0.0),
                num(options.ambient, 0.45).max(0.0),
                num(options.brightness, 1.0).max(0.0),
                num(options.lift, 0.0).clamp(0.0, 1.0),
            ],
            sky: with_w(options.sky, num(options.venue_opacity, 1.0).clamp(0.0, 1.0)),
            ground: with_w(options.ground, if srgb { 1.0 } else { 0.0 }),
            fog: with_w(options.background, num(options.grid_extent, 50.0).max(1.0)),
            extra: [num(options.axes_length, 1.0).max(0.0), 1.0, 0.0, 0.0],
        };
        gpu.queue
            .write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(&frame));

        // Each part's numbers and what it is drawn with.
        let now = Instant::now();
        let see_through = frame.sky[3] < 1.0;
        let mut uniforms: Vec<PartUniform> = Vec::with_capacity(scene.parts.len());
        // (index into `uniforms`, mesh key, picture, see-through)
        let mut draws: Vec<(u32, u64, Picture, bool)> = Vec::with_capacity(scene.parts.len());
        let mut output_groups: HashMap<OutputId, Option<wgpu::BindGroup>> = HashMap::new();
        let mut missing = 0;
        for part in &scene.parts {
            if !part.transform.iter().flatten().all(|v| v.is_finite()) {
                continue;
            }
            if !self.ensure_mesh(device, part) {
                continue;
            }
            let (colour, shows, region, picture) = match &part.paint {
                Paint::Colour(c) => (*c, SHOWS_COLOUR, WHOLE, Picture::White),
                Paint::Image {
                    rgba,
                    width,
                    height,
                    key,
                } => {
                    if self.ensure_image(gpu, *key, rgba, *width, *height) {
                        ([1.0; 4], SHOWS_PICTURE, WHOLE, Picture::Image(*key))
                    } else {
                        ([0.5, 0.5, 0.5, 1.0], SHOWS_COLOUR, WHOLE, Picture::White)
                    }
                }
                Paint::Output { output, region } => {
                    let group = output_groups.entry(*output).or_insert_with(|| {
                        outputs(*output).map(|view| {
                            Self::picture_group(device, &self.tex_layout, view, &self.clamp)
                        })
                    });
                    if group.is_some() {
                        (
                            [1.0; 4],
                            SHOWS_PICTURE,
                            sane_region(*region),
                            Picture::Output(*output),
                        )
                    } else {
                        missing += 1;
                        ([1.0; 4], SHOWS_MISSING, WHOLE, Picture::White)
                    }
                }
            };
            let colour = colour.map(|v| num(v, 1.0).clamp(0.0, 1.0));
            let n = maths::normal_matrix(&part.transform);
            uniforms.push(PartUniform {
                model: part.transform,
                n: n.map(|c| [c[0], c[1], c[2], 0.0]),
                colour,
                region,
                flags: [if part.lit { 1.0 } else { 0.0 }, shows, 0.0, 0.0],
            });
            let blended = (part.lit && see_through) || colour[3] < 1.0;
            draws.push((uniforms.len() as u32 - 1, part.mesh_key, picture, blended));
        }
        self.ensure_part_capacity(device, uniforms.len() as u64);
        if !uniforms.is_empty() {
            let stride = self.part_stride as usize;
            let mut bytes = vec![0u8; uniforms.len() * stride];
            for (i, u) in uniforms.iter().enumerate() {
                bytes[i * stride..i * stride + size_of::<PartUniform>()]
                    .copy_from_slice(bytemuck::bytes_of(u));
            }
            gpu.queue.write_buffer(&self.part_buffer, 0, &bytes);
        }
        // See-through parts are drawn last, farthest first.
        let eye = cam.eye;
        let far_first = |u: &PartUniform| {
            let o = [u.model[3][0], u.model[3][1], u.model[3][2]];
            -maths::length(maths::sub(o, eye))
        };
        draws.sort_by(|a, b| {
            a.3.cmp(&b.3).then_with(|| {
                if a.3 {
                    far_first(&uniforms[a.0 as usize])
                        .total_cmp(&far_first(&uniforms[b.0 as usize]))
                } else {
                    std::cmp::Ordering::Equal
                }
            })
        });

        let mut stats = FrameStats {
            samples,
            drawn,
            missing_pictures: missing,
            ..FrameStats::default()
        };
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("3d") });
        {
            let targets = &self.targets[&target_key];
            let pipes = &self.pipelines[&(target.format, samples)];
            let into = targets.small.as_ref().map_or(target.view, |(v, _)| v);
            let (view, resolve) = match &targets.msaa {
                Some(m) => (m, Some(into)),
                None => (into, None),
            };
            let bg = options.background.map(|v| {
                let v = num(v, 0.0).clamp(0.0, 1.0);
                f64::from(if srgb { srgb_to_linear(v) } else { v })
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("3d"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: resolve,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: bg[0],
                            g: bg[1],
                            b: bg[2],
                            a: 1.0,
                        }),
                        store: if resolve.is_some() {
                            wgpu::StoreOp::Discard
                        } else {
                            wgpu::StoreOp::Store
                        },
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let draw_parts =
                |pass: &mut wgpu::RenderPass, stats: &mut FrameStats, blended: bool| {
                    pass.set_pipeline(if blended {
                        &pipes.see_through
                    } else {
                        &pipes.opaque
                    });
                    pass.set_bind_group(0, &self.frame_group, &[]);
                    for (slot, key, picture, b) in &draws {
                        if *b != blended {
                            continue;
                        }
                        let Some(mesh) = self.meshes.get(key) else {
                            continue;
                        };
                        let group = match picture {
                            Picture::White => &self.white,
                            Picture::Image(k) => {
                                self.images.get(k).map_or(&self.white, |i| &i.bind_group)
                            }
                            Picture::Output(o) => output_groups[o].as_ref().unwrap_or(&self.white),
                        };
                        let offset = (u64::from(*slot) * self.part_stride) as u32;
                        pass.set_bind_group(1, &self.part_group, &[offset]);
                        pass.set_bind_group(2, group, &[]);
                        pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                        pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                        pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                        stats.draw_calls += 1;
                        stats.triangles += u64::from(mesh.index_count / 3);
                        stats.parts += 1;
                    }
                };
            draw_parts(&mut pass, &mut stats, false);
            if options.grid {
                pass.set_pipeline(&pipes.grid);
                pass.set_bind_group(0, &self.frame_group, &[]);
                pass.draw(0..6, 0..1);
                stats.draw_calls += 1;
                stats.triangles += 2;
            }
            if options.axes {
                pass.set_pipeline(&pipes.axes);
                pass.set_bind_group(0, &self.frame_group, &[]);
                pass.draw(0..6, 0..1);
                stats.draw_calls += 1;
            }
            if draws.iter().any(|d| d.3) {
                draw_parts(&mut pass, &mut stats, true);
            }
        }
        if let Some((_, group)) = self.targets.get(&target_key).and_then(|t| t.small.as_ref()) {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("3d scale up"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target.view,
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
            pass.set_pipeline(&self.blits[&target.format]);
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
            stats.draw_calls += 1;
        }
        gpu.submit([encoder.finish()]);
        for (_, key, picture, _) in &draws {
            if let Some(m) = self.meshes.get_mut(key) {
                m.used = now;
            }
            if let Picture::Image(k) = picture
                && let Some(i) = self.images.get_mut(k)
            {
                i.used = now;
            }
        }
        stats.meshes_cached = self.meshes.len();
        stats.images_cached = self.images.len();
        stats.cpu_time = started.elapsed();
        self.last = stats;
        stats
    }

    /// Let go of meshes, pictures and render targets not used for
    /// `FORGET_AFTER` (done by itself every `TIDY_EVERY` while drawing).
    pub fn tidy(&mut self) {
        self.tidied = Instant::now();
        self.tidy_older_than(FORGET_AFTER);
    }

    /// Let go of the meshes and pictures none of `scenes` has (a venue read
    /// again at another brightness has new pictures: the old ones go at
    /// once, not after `FORGET_AFTER`).
    pub fn keep_only(&mut self, scenes: &[&Scene]) {
        let mut meshes = std::collections::HashSet::new();
        let mut images = std::collections::HashSet::new();
        for p in scenes.iter().flat_map(|s| &s.parts) {
            meshes.insert(p.mesh_key);
            if let Paint::Image { key, .. } = &p.paint {
                images.insert(*key);
            }
        }
        self.meshes.retain(|k, _| meshes.contains(k));
        self.images.retain(|k, _| images.contains(k));
        self.too_big.retain(|k| meshes.contains(k));
    }

    /// What is kept: meshes, pictures and sets of render targets.
    pub fn kept(&self) -> (usize, usize, usize) {
        (self.meshes.len(), self.images.len(), self.targets.len())
    }

    fn tidy_older_than(&mut self, age: Duration) {
        self.meshes.retain(|_, m| m.used.elapsed() < age);
        self.images.retain(|_, i| i.used.elapsed() < age);
        self.targets.retain(|_, t| t.used.elapsed() < age);
    }

    /// Take buffers of at most `bytes` (a test's stand-in for a device's
    /// `max_buffer_size`).
    #[cfg(test)]
    pub fn limit_buffers(&mut self, bytes: u64) {
        self.max_buffer = bytes.min(self.max_buffer);
    }

    /// Let go of everything made for drawing (the device stays).
    pub fn forget_all(&mut self) {
        self.meshes.clear();
        self.images.clear();
        self.targets.clear();
    }

    fn part_buffer(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        stride: u64,
        capacity: u64,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("3d parts"),
            size: stride * capacity,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("3d parts"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(size_of::<PartUniform>() as u64),
                }),
            }],
        });
        (buffer, group)
    }

    fn ensure_part_capacity(&mut self, device: &wgpu::Device, wanted: u64) {
        if wanted <= self.part_capacity {
            return;
        }
        let capacity = wanted.next_power_of_two();
        let (buffer, group) =
            Self::part_buffer(device, &self.part_layout, self.part_stride, capacity);
        self.part_buffer = buffer;
        self.part_group = group;
        self.part_capacity = capacity;
    }

    fn picture_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("3d picture"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
    }

    /// The part's mesh on the GPU; false when it has no whole triangle.
    fn ensure_mesh(&mut self, device: &wgpu::Device, part: &Part) -> bool {
        let mesh = &part.mesh;
        if self
            .meshes
            .get(&part.mesh_key)
            .is_some_and(|m| m.vertex_count == mesh.positions.len())
        {
            return true;
        }
        if self.too_big.contains(&part.mesh_key) {
            return false;
        }
        // A buffer bigger than the device takes is an error that ends the
        // drawing thread: such a part is left out (the loader cuts a
        // venue's into pieces that fit, so this is a last guard).
        let bytes = |n: usize, each: usize| (n as u64).saturating_mul(each as u64);
        let wanted = bytes(mesh.positions.len(), size_of::<Vertex>())
            .max(bytes(mesh.indices.len(), size_of::<u32>()));
        if wanted > self.max_buffer {
            log::warn!(
                "3D stage: a part of {} corners is too big for the graphics card \
                 ({wanted} bytes, at most {}); it is left out",
                mesh.positions.len(),
                self.max_buffer
            );
            self.too_big.insert(part.mesh_key);
            return false;
        }
        // (One with nothing to draw is left alone: another part may already
        // be drawing what the key holds.)
        let Some((vertices, indices)) = gpu_vertices(mesh) else {
            return false;
        };
        let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("3d vertices"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("3d indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        self.meshes.insert(
            part.mesh_key,
            GpuMesh {
                vertices: vb,
                indices: ib,
                index_count: indices.len() as u32,
                vertex_count: mesh.positions.len(),
                used: Instant::now(),
            },
        );
        true
    }

    /// The picture on the GPU; false when its pixels don't fill its size.
    fn ensure_image(&mut self, gpu: &Gpu, key: u64, rgba: &[u8], w: u32, h: u32) -> bool {
        if self.images.get(&key).is_some_and(|i| i.size == (w, h)) {
            return true;
        }
        let Some((pixels, size)) = fitted_image(rgba, w, h, MAX_IMAGE_SIDE) else {
            return false;
        };
        let texture = gpu.device.create_texture_with_data(
            &gpu.queue,
            &wgpu::TextureDescriptor {
                label: Some("3d image"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                // Display values, as the outputs' pictures are.
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &pixels,
        );
        let bind_group = Self::picture_group(
            &gpu.device,
            &self.tex_layout,
            &texture.create_view(&Default::default()),
            &self.repeat,
        );
        self.images.insert(
            key,
            GpuImage {
                _texture: texture,
                bind_group,
                size: (w, h),
                used: Instant::now(),
            },
        );
        true
    }

    fn ensure_targets(
        &mut self,
        device: &wgpu::Device,
        target: &Target,
        size: (u32, u32),
        samples: u32,
        scaled: bool,
    ) -> TargetKey {
        let (owner, format, full) = (target.owner, target.format, target.size);
        let key = (owner, size, format, samples, scaled);
        if let Some(t) = self.targets.get_mut(&key) {
            t.used = Instant::now();
            return key;
        }
        // Its owner at another size is gone (a resize): only the governor's
        // steps at this size are kept beside each other.
        self.targets
            .retain(|k, t| k.0 != owner || (t.full == full && k.2 == format));
        let texture = |label, format, samples, usage| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: size.0,
                        height: size.1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let draw = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let depth = texture("3d depth", DEPTH_FORMAT, samples, draw);
        let msaa = (samples > 1).then(|| texture("3d msaa", format, samples, draw));
        let small = scaled.then(|| {
            let view = texture(
                "3d small",
                format,
                1,
                draw | wgpu::TextureUsages::TEXTURE_BINDING,
            );
            let group = Self::picture_group(device, &self.tex_layout, &view, &self.clamp);
            (view, group)
        });
        self.targets.insert(
            key,
            Targets {
                used: Instant::now(),
                full,
                depth,
                msaa,
                small,
            },
        );
        key
    }

    fn ensure_pipelines(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        samples: u32,
    ) {
        if self.pipelines.contains_key(&(format, samples)) {
            return;
        }
        let multisample = wgpu::MultisampleState {
            count: samples,
            ..Default::default()
        };
        let depth = |write: bool, compare| {
            Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(write),
                depth_compare: Some(compare),
                stencil: Default::default(),
                bias: Default::default(),
            })
        };
        let attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2];
        let part_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &attributes,
        })];
        let make = |label,
                    layout: &wgpu::PipelineLayout,
                    vs,
                    fs,
                    buffers: &[Option<wgpu::VertexBufferLayout>],
                    topology,
                    blend,
                    depth_stencil| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: &self.shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers,
                },
                primitive: wgpu::PrimitiveState {
                    topology,
                    // Nothing culled: imported venues are wound either way,
                    // and screens are seen from both sides.
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil,
                multisample,
                fragment: Some(wgpu::FragmentState {
                    module: &self.shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        // The target's alpha stays the clear's 1: a picture's
                        // own transparency never leaves holes in the view.
                        write_mask: wgpu::ColorWrites::COLOR,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        use wgpu::CompareFunction::{Greater, GreaterEqual};
        use wgpu::PrimitiveTopology::{LineList, TriangleList};
        let alpha = Some(wgpu::BlendState::ALPHA_BLENDING);
        let pipes = Pipelines {
            opaque: make(
                "3d parts",
                &self.parts_pipeline_layout,
                "vs_part",
                "fs_part",
                &part_buffers,
                TriangleList,
                None,
                depth(true, Greater),
            ),
            see_through: make(
                "3d see-through",
                &self.parts_pipeline_layout,
                "vs_part",
                "fs_part",
                &part_buffers,
                TriangleList,
                alpha,
                depth(false, Greater),
            ),
            grid: make(
                "3d grid",
                &self.frame_pipeline_layout,
                "vs_grid",
                "fs_grid",
                &[],
                TriangleList,
                alpha,
                depth(false, GreaterEqual),
            ),
            axes: make(
                "3d axes",
                &self.frame_pipeline_layout,
                "vs_axes",
                "fs_axes",
                &[],
                LineList,
                None,
                depth(true, GreaterEqual),
            ),
        };
        self.pipelines.insert((format, samples), pipes);
    }

    fn ensure_blit(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) {
        if self.blits.contains_key(&format) {
            return;
        }
        let p = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("3d scale up"),
            layout: Some(&self.blit_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &self.blit_shader,
                entry_point: Some("vs_blit"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &self.blit_shader,
                entry_point: Some("fs_blit"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        self.blits.insert(format, p);
    }
}

/// What a part's picture comes from this render.
#[derive(Clone, Copy)]
enum Picture {
    White,
    Image(u64),
    Output(OutputId),
}

const WHOLE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

fn with_w(c: [f32; 3], w: f32) -> [f32; 4] {
    let c = c.map(|v| if v.is_finite() { v.max(0.0) } else { 0.0 });
    [c[0], c[1], c[2], w]
}

/// A region that is numbers, else the whole picture.
fn sane_region(r: [f32; 4]) -> [f32; 4] {
    if r.iter().all(|v| v.is_finite()) {
        r
    } else {
        WHOLE
    }
}

/// The mesh as the GPU takes it: normals made where the mesh has none (or
/// not one per vertex), uv 0 where it has no uvs, positions that aren't
/// numbers at the origin, and only the triangles whose indices are all in
/// range. None when no triangle is left.
fn gpu_vertices(mesh: &Mesh) -> Option<(Vec<Vertex>, Vec<u32>)> {
    let n = mesh.positions.len();
    let indices: Vec<u32> = mesh
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|t| t.iter().all(|&i| (i as usize) < n))
        .flatten()
        .copied()
        .collect();
    if indices.is_empty() {
        return None;
    }
    let pos: Vec<[f32; 3]> = mesh
        .positions
        .iter()
        .map(|p| if maths::finite(*p) { *p } else { [0.0; 3] })
        .collect();
    let normals = if mesh.normals.len() == n {
        mesh.normals.clone()
    } else {
        smooth_normals(&pos, &indices)
    };
    let vertices = (0..n)
        .map(|i| Vertex {
            pos: pos[i],
            normal: normals[i].map(|v| if v.is_finite() { v } else { 0.0 }),
            uv: if mesh.uvs.len() == n {
                mesh.uvs[i].map(|v| if v.is_finite() { v } else { 0.0 })
            } else {
                [0.0; 2]
            },
        })
        .collect();
    Some((vertices, indices))
}

/// Each vertex's normal as the sum of its triangles' (weighted by area).
fn smooth_normals(pos: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut out = vec![[0.0f32; 3]; pos.len()];
    for t in indices.as_chunks::<3>().0 {
        let [a, b, c] = t.map(|i| pos[i as usize]);
        let n = maths::cross(maths::sub(b, a), maths::sub(c, a));
        for &i in t {
            out[i as usize] = maths::add(out[i as usize], n);
        }
    }
    out.into_iter()
        .map(|n| maths::normalize(n).unwrap_or([0.0; 3]))
        .collect()
}

/// The picture's pixels at no more than `max` a side (every k-th pixel of a
/// bigger one), with its size; None when `rgba` is shorter than its size says.
fn fitted_image(rgba: &[u8], w: u32, h: u32, max: u32) -> Option<(Vec<u8>, (u32, u32))> {
    let len = (w as usize).checked_mul(h as usize)?.checked_mul(4)?;
    if w == 0 || h == 0 || rgba.len() < len {
        return None;
    }
    let step = w.max(h).div_ceil(max).max(1);
    if step == 1 {
        return Some((rgba[..len].to_vec(), (w, h)));
    }
    let (nw, nh) = (w.div_ceil(step), h.div_ceil(step));
    let mut out = Vec::with_capacity(nw as usize * nh as usize * 4);
    for y in 0..nh {
        for x in 0..nw {
            let i = ((y * step) as usize * w as usize + (x * step) as usize) * 4;
            out.extend_from_slice(&rgba[i..i + 4]);
        }
    }
    Some((out, (nw, nh)))
}

/// A display value's light (the sRGB curve), for clearing an sRGB target.
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}
