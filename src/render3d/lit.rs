//! The Light view: projectors throwing their pictures into the room
//! (`Scene::throws`, step P2 of `.claude/notes/projection-plan.md`).
//!
//! What it adds to the renderer, all of it optional: a GPU that won't make
//! any of it (its shader module, its pipelines, the shadow pipeline) leaves
//! the light out, says so once in the log and in `FrameStats::light_left_out`,
//! and the view is drawn as it is without light. It never blanks the view.
//!
//! - Its own shader module (`shader.wgsl` followed by `light.wgsl`), so a
//!   module the GPU refuses doesn't take the plain view with it, and its own
//!   fragment entry `fs_lit` with a fourth bind group (`group 3`): the
//!   throws' numbers (a uniform array of at most `MAX_THROWS`), their
//!   pictures (a 2D array, one layer each, display values), and their shadow
//!   maps (a depth array, one layer each, with a comparison sampler).
//! - One loop in the fragment shader over the throws, summing light in lux
//!   (`stage3d::light`'s sums), shadows by the shadow maps, then shown as a
//!   picture (nits against a white point), or in false colour: brightness at
//!   full white, how many projectors overlap, the size of a pixel.
//! - Shadow maps drawn with a depth-only pipeline over the same meshes, again
//!   only when the scene or that throw's lens has changed. The throw's frustum
//!   is the light's, so there is nothing to fit. Depth is reversed as in the
//!   view (near 1): lit means at least as near as what the map holds, with a
//!   push along the normal and a tolerance in proportion to the distance.

use super::{DEPTH_FORMAT, LightView, Options, Part, Scene};
use crate::stage3d::light::{
    BRIGHTNESS_STEPS, MAX_THROWS, OVERLAP, PIXEL_STEPS_MM, ROOM_LIGHT, SCALE,
};
use crate::stage3d::scene::Throw;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// A shadow map's side at full quality, and below it (a governed preview).
pub const SHADOW_SIDE: u32 = 1024;
pub const SHADOW_SIDE_LOW: u32 = 512;
/// The push along the normal before a shadow map is looked at, per metre of
/// depth, times 1024 over its side (about one and a half of its texels).
const NORMAL_PUSH: f32 = 0.0025;
/// How much nearer than it is a point is taken to be against a shadow map
/// (a share of its distance).
const DEPTH_SLACK: f32 = 0.003;
/// A set of shadow maps not used for this long is let go.
const SHADOWS_KEPT: Duration = Duration::from_secs(10);

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct ThrowUniform {
    view_proj: [[f32; 4]; 4],
    /// xyz the lens; w the lux one metre out.
    eye: [f32; 4],
    /// xyz the axis; w the gamma.
    axis: [f32; 4],
    /// x 1 when it throws its picture (else white), y one pixel one metre out
    /// (metres), z its picture's layer, w 1 when on.
    more: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct LightsUniform {
    /// x how many throws, y the view (`LightView::number`), z the white
    /// point (nits), w the shadow maps' side.
    head: [f32; 4],
    /// x the room's light kept, y the push along the normal, z the depth
    /// slack.
    more: [f32; 4],
    bright: [f32; 4],
    pixel: [f32; 4],
    scale: [[f32; 4]; 5],
    overlap: [[f32; 4]; 3],
    throws: [ThrowUniform; MAX_THROWS],
}

/// The numbers the shader is given for `throws` (the first `MAX_THROWS`),
/// shadow maps `side` square.
/// `pictures` false: no pictures were given, so every throw is white.
pub(super) fn lights_uniform(
    throws: &[Throw],
    options: &Options,
    side: u32,
    pictures: bool,
) -> LightsUniform {
    let num = |v: f32, or: f32| if v.is_finite() { v } else { or };
    let mut t = [ThrowUniform::zeroed(); MAX_THROWS];
    for (i, th) in throws.iter().take(MAX_THROWS).enumerate() {
        let ok = th.view_proj.iter().flatten().all(|v| v.is_finite())
            && th.eye.iter().chain(&th.axis).all(|v| v.is_finite());
        t[i] = ThrowUniform {
            view_proj: th.view_proj,
            eye: [
                th.eye[0],
                th.eye[1],
                th.eye[2],
                num(th.lux_at_one_metre, 0.0).max(0.0),
            ],
            axis: [
                th.axis[0],
                th.axis[1],
                th.axis[2],
                num(th.gamma, 2.2).clamp(1.0, 3.0),
            ],
            more: [
                if th.picture && pictures { 1.0 } else { 0.0 },
                num(th.pixel_at_one_metre, 0.0).max(0.0),
                th.layer.min(MAX_THROWS as u32 - 1) as f32,
                if th.on && ok { 1.0 } else { 0.0 },
            ],
        };
    }
    let four = |s: [f32; 4]| s;
    let rgb = |c: [f32; 3]| [c[0], c[1], c[2], 1.0];
    LightsUniform {
        head: [
            throws.len().min(MAX_THROWS) as f32,
            options.light.number(),
            num(options.white_nits, 100.0).clamp(0.01, 1e7),
            side as f32,
        ],
        more: [
            ROOM_LIGHT,
            NORMAL_PUSH * SHADOW_SIDE as f32 / side.max(1) as f32,
            DEPTH_SLACK,
            0.0,
        ],
        bright: four(BRIGHTNESS_STEPS),
        pixel: four(PIXEL_STEPS_MM),
        scale: SCALE.map(rgb),
        overlap: OVERLAP.map(rgb),
        throws: t,
    }
}

use bytemuck::Zeroable;

/// Whether a box (its eight corners in the world) can be in a throw's
/// picture: false only when all of it is past one edge of the frustum, or
/// behind the lens. (A screen no projector reaches keeps its own picture:
/// an LED wall.)
pub fn box_meets(view_proj: &[[f32; 4]; 4], corners: &[[f32; 3]; 8]) -> bool {
    let c: Vec<[f32; 4]> = corners
        .iter()
        .map(|p| super::maths::transform4(view_proj, [p[0], p[1], p[2], 1.0]))
        .collect();
    let all = |f: &dyn Fn(&[f32; 4]) -> bool| c.iter().all(f);
    !(all(&|v| v[3] <= 1e-6)
        || all(&|v| v[0] > v[3])
        || all(&|v| v[0] < -v[3])
        || all(&|v| v[1] > v[3])
        || all(&|v| v[1] < -v[3]))
}

/// The eight corners of a mesh's box `(min, max)` placed by `transform`.
pub fn box_corners(min: [f32; 3], max: [f32; 3], transform: &[[f32; 4]; 4]) -> [[f32; 3]; 8] {
    std::array::from_fn(|i| {
        let p = [
            if i & 1 == 0 { min[0] } else { max[0] },
            if i & 2 == 0 { min[1] } else { max[1] },
            if i & 4 == 0 { min[2] } else { max[2] },
        ];
        super::maths::transform_point(transform, p)
    })
}

/// A key for what casts shadows: every part's mesh and where it is.
pub(super) fn casters_key(scene: &Scene) -> u64 {
    let mut h = crate::stage3d::scene::Fnv::new();
    for p in &scene.parts {
        h.bytes(&p.mesh_key.to_le_bytes());
        p.transform.iter().flatten().for_each(|v| h.f32(*v));
    }
    h.0
}

/// One throw's shadow map's key: what casts, and the lens.
pub(super) fn shadow_key(casters: u64, t: &Throw) -> u64 {
    let mut h = crate::stage3d::scene::Fnv(casters);
    t.view_proj.iter().flatten().for_each(|v| h.f32(*v));
    h.bytes(&[u8::from(t.on)]);
    h.0
}

/// Whether part `p` (unlit) is a screen some throw can reach. An LED
/// screen (`Catch::led`) never is: it keeps its own picture.
pub(super) fn reached(p: &Part, bounds: ([f32; 3], [f32; 3]), throws: &[Throw]) -> bool {
    if p.catch.led {
        return false;
    }
    let corners = box_corners(bounds.0, bounds.1, &p.transform);
    throws
        .iter()
        .take(MAX_THROWS)
        .any(|t| t.on && box_meets(&t.view_proj, &corners))
}

/// How many layers an array for `count` throws has: a power of two, at
/// least one, at most `MAX_THROWS`.
pub fn layers_for(count: usize) -> usize {
    count
        .clamp(1, MAX_THROWS)
        .next_power_of_two()
        .min(MAX_THROWS)
}

/// A set of shadow maps, one layer per throw.
pub(super) struct ShadowMaps {
    _texture: wgpu::Texture,
    pub layers: Vec<wgpu::TextureView>,
    pub array: wgpu::TextureView,
    /// What each layer was drawn for (`shadow_key`; 0 never drawn).
    pub keys: [u64; MAX_THROWS],
    used: Instant,
}

/// What the light needs on the GPU, made at the first view with its light on.
pub(super) struct LightKit {
    pub layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    module: wgpu::ShaderModule,
    /// By target format and sample count: (opaque, see-through); None where
    /// this GPU wouldn't make them.
    pipelines:
        HashMap<(wgpu::TextureFormat, u32), Option<(wgpu::RenderPipeline, wgpu::RenderPipeline)>>,
    pub shadow_pipeline: wgpu::RenderPipeline,
    pub uniform: wgpu::Buffer,
    /// Each shadow layer's view of the lens: the view's own frame uniform
    /// with the throw's matrix, one slice of this buffer each.
    pub shadow_frames: wgpu::Buffer,
    pub shadow_groups: Vec<wgpu::BindGroup>,
    pub frame_stride: u64,
    /// One white layer, for throws with no pictures given.
    pub white: wgpu::TextureView,
    _white_texture: wgpu::Texture,
    pub compare: wgpu::Sampler,
    shadows: HashMap<u32, ShadowMaps>,
}

/// The light's shader: the view's, then its own.
pub(super) const SOURCE: &str = concat!(include_str!("shader.wgsl"), include_str!("light.wgsl"));
/// A shader that won't compile: what a GPU refusing the light looks like (a
/// test's, `Renderer::refuse_light`).
#[cfg(test)]
pub(super) const BROKEN: &str =
    "@fragment fn fs_lit() -> @location(0) vec4<f32> { return undefined_thing; }";

impl LightKit {
    /// Make it, or None (said in the log) when this GPU won't.
    pub fn new(
        device: &wgpu::Device,
        frame_layout: &wgpu::BindGroupLayout,
        part_layout: &wgpu::BindGroupLayout,
        tex_layout: &wgpu::BindGroupLayout,
        main_shader: &wgpu::ShaderModule,
        frame_size: u64,
        source: &str,
    ) -> Option<LightKit> {
        let module = super::checked(device, "light", || {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("3d light"),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            })
        })?;
        let frag = wgpu::ShaderStages::FRAGMENT;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("3d light"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: frag,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(size_of::<LightsUniform>() as u64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: frag,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: frag,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: frag,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: frag,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("3d lit parts"),
            bind_group_layouts: &[
                Some(frame_layout),
                Some(part_layout),
                Some(tex_layout),
                Some(&layout),
            ],
            immediate_size: 0,
        });
        let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("3d shadows"),
            bind_group_layouts: &[Some(frame_layout), Some(part_layout)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2];
        let shadow_pipeline = super::checked(device, "shadows", || {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("3d shadows"),
                layout: Some(&shadow_layout),
                vertex: wgpu::VertexState {
                    module: main_shader,
                    entry_point: Some("vs_part"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: size_of::<super::Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attributes,
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Greater),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                fragment: None,
                multiview_mask: None,
                cache: None,
            })
        })?;
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("3d lights"),
            size: size_of::<LightsUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let frame_stride = u64::from(device.limits().min_uniform_buffer_offset_alignment)
            .max(frame_size)
            .next_multiple_of(256);
        let shadow_frames = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("3d shadow frames"),
            size: frame_stride * MAX_THROWS as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shadow_groups = (0..MAX_THROWS as u64)
            .map(|i| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("3d shadow frame"),
                    layout: frame_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &shadow_frames,
                            offset: i * frame_stride,
                            size: wgpu::BufferSize::new(frame_size),
                        }),
                    }],
                })
            })
            .collect();
        let white_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("3d white throw"),
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
        });
        let white = white_texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let compare = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("3d shadow compare"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            // Reversed depth: lit when at least as near as the map.
            compare: Some(wgpu::CompareFunction::GreaterEqual),
            ..Default::default()
        });
        Some(LightKit {
            layout,
            pipeline_layout,
            module,
            pipelines: HashMap::new(),
            shadow_pipeline,
            uniform,
            shadow_frames,
            shadow_groups,
            frame_stride,
            white,
            _white_texture: white_texture,
            compare,
            shadows: HashMap::new(),
        })
    }

    /// The white layer's pixel (written once, after it is made).
    pub fn fill_white(&self, queue: &wgpu::Queue) {
        queue.write_texture(
            self._white_texture.as_image_copy(),
            &[255; 4],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
    }

    /// The lit pipelines for a target, made the first time (None: this GPU
    /// won't make them).
    pub fn pipelines(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        samples: u32,
    ) -> Option<&(wgpu::RenderPipeline, wgpu::RenderPipeline)> {
        let module = &self.module;
        let layout = &self.pipeline_layout;
        self.pipelines
            .entry((format, samples))
            .or_insert_with(|| {
                let attributes =
                    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2];
                let make = |label, blend, write: bool| {
                    super::checked(device, label, || {
                        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                            label: Some(label),
                            layout: Some(layout),
                            vertex: wgpu::VertexState {
                                module,
                                entry_point: Some("vs_part"),
                                compilation_options: Default::default(),
                                buffers: &[Some(wgpu::VertexBufferLayout {
                                    array_stride: size_of::<super::Vertex>() as u64,
                                    step_mode: wgpu::VertexStepMode::Vertex,
                                    attributes: &attributes,
                                })],
                            },
                            primitive: wgpu::PrimitiveState {
                                cull_mode: None,
                                ..Default::default()
                            },
                            depth_stencil: Some(wgpu::DepthStencilState {
                                format: DEPTH_FORMAT,
                                depth_write_enabled: Some(write),
                                depth_compare: Some(wgpu::CompareFunction::Greater),
                                stencil: Default::default(),
                                bias: Default::default(),
                            }),
                            multisample: wgpu::MultisampleState {
                                count: samples,
                                ..Default::default()
                            },
                            fragment: Some(wgpu::FragmentState {
                                module,
                                entry_point: Some("fs_lit"),
                                compilation_options: Default::default(),
                                targets: &[Some(wgpu::ColorTargetState {
                                    format,
                                    blend,
                                    write_mask: wgpu::ColorWrites::COLOR,
                                })],
                            }),
                            multiview_mask: None,
                            cache: None,
                        })
                    })
                };
                let opaque = make("3d lit", None, true)?;
                let see_through = make(
                    "3d lit see-through",
                    Some(wgpu::BlendState::ALPHA_BLENDING),
                    false,
                )?;
                Some((opaque, see_through))
            })
            .as_ref()
    }

    /// The shadow maps `side` square, at least `count` of them (made the
    /// first time, and again with more layers when more are wanted: in
    /// powers of two, so one projector doesn't hold sixteen maps).
    pub fn shadows(&mut self, device: &wgpu::Device, side: u32, count: usize) -> &mut ShadowMaps {
        let now = Instant::now();
        let count = layers_for(count);
        self.shadows.retain(|s, m| {
            (*s == side && m.layers.len() >= count)
                || (*s != side && now.saturating_duration_since(m.used) < SHADOWS_KEPT)
        });
        let maps = self.shadows.entry(side).or_insert_with(|| {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("3d shadow maps"),
                size: wgpu::Extent3d {
                    width: side,
                    height: side,
                    depth_or_array_layers: count as u32,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let layers = (0..count as u32)
                .map(|i| {
                    texture.create_view(&wgpu::TextureViewDescriptor {
                        label: Some("3d shadow map"),
                        dimension: Some(wgpu::TextureViewDimension::D2),
                        base_array_layer: i,
                        array_layer_count: Some(1),
                        ..Default::default()
                    })
                })
                .collect();
            let array = texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("3d shadow maps"),
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            });
            ShadowMaps {
                _texture: texture,
                layers,
                array,
                keys: [0; MAX_THROWS],
                used: now,
            }
        });
        maps.used = now;
        maps
    }

    /// The shadow maps `side` square, once made by `shadows`.
    pub fn maps(&self, side: u32) -> Option<&ShadowMaps> {
        self.shadows.get(&side)
    }

    /// The lit pipelines for a target, if made.
    pub fn pipelines_made(
        &self,
        format: wgpu::TextureFormat,
        samples: u32,
    ) -> Option<(&wgpu::RenderPipeline, &wgpu::RenderPipeline)> {
        self.pipelines
            .get(&(format, samples))
            .and_then(|p| p.as_ref())
            .map(|(o, s)| (o, s))
    }

    /// Let go of the shadow maps (the device stays).
    pub fn forget(&mut self) {
        self.shadows.clear();
    }

    /// How many sets of shadow maps are kept.
    pub fn kept(&self) -> usize {
        self.shadows.len()
    }
}

impl LightView {
    /// The shader's number for it.
    pub fn number(self) -> f32 {
        match self {
            LightView::Off => 0.0,
            LightView::Picture => 1.0,
            LightView::Brightness => 2.0,
            LightView::Overlap => 3.0,
            LightView::PixelSize => 4.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_meets_a_throw_unless_all_of_it_is_past_one_edge() {
        let mut p = crate::stage3d::Projector::new(crate::stage3d::ProjectorId(1), "P".into());
        p.placement.position = [0.0, 0.0, 10.0];
        let t = crate::stage3d::light::throw_of(&p, 16.0 / 9.0, (1920, 1080), 0, false);
        let at = |c: [f32; 3], s: f32| {
            box_corners(
                [c[0] - s, c[1] - s, c[2] - s],
                [c[0] + s, c[1] + s, c[2] + s],
                &crate::render3d::scene::IDENTITY,
            )
        };
        assert!(box_meets(&t.view_proj, &at([0.0, 0.0, 0.0], 0.5)));
        // Far to the side, above, behind the lens: not.
        assert!(!box_meets(&t.view_proj, &at([30.0, 0.0, 0.0], 0.5)));
        assert!(!box_meets(&t.view_proj, &at([0.0, 30.0, 0.0], 0.5)));
        assert!(!box_meets(&t.view_proj, &at([0.0, 0.0, 20.0], 0.5)));
        // A big box round the lens: it does.
        assert!(box_meets(&t.view_proj, &at([0.0, 0.0, 10.0], 3.0)));
        // Straddling the picture's right edge.
        let (w, _) = p.picture_at(10.0, 16.0 / 9.0);
        assert!(box_meets(&t.view_proj, &at([w / 2.0, 0.0, 0.0], 0.2)));
    }

    /// The light's shader as DirectX 12's HLSL and a Mac's MSL, written by
    /// naga as wgpu does there: a first check that those systems will take it
    /// (their own compilers, which refused one of the view's before, run only
    /// on those systems).
    #[test]
    fn the_light_shader_is_written_for_directx_and_metal() {
        let module = naga::front::wgsl::parse_str(SOURCE).expect("the light's shader reads");
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect("the light's shader is valid");
        for model in [
            naga::back::hlsl::ShaderModel::V5_1,
            naga::back::hlsl::ShaderModel::V6_0,
        ] {
            let options = naga::back::hlsl::Options {
                shader_model: model,
                ..Default::default()
            };
            let mut hlsl = String::new();
            naga::back::hlsl::Writer::new(&mut hlsl, &options, &Default::default())
                .write(&module, &info, None)
                .expect("written as HLSL");
            assert!(hlsl.contains("fs_lit"));
        }
        let (msl, _) =
            naga::back::msl::write_string(&module, &info, &Default::default(), &Default::default())
                .expect("written as MSL");
        assert!(msl.contains("fs_lit"));
    }

    #[test]
    fn arrays_hold_a_power_of_two_of_layers() {
        assert_eq!(layers_for(0), 1);
        assert_eq!(layers_for(1), 1);
        assert_eq!(layers_for(2), 2);
        assert_eq!(layers_for(3), 4);
        assert_eq!(layers_for(9), 16);
        assert_eq!(layers_for(40), 16);
    }

    #[test]
    fn the_shaders_numbers_are_packed_as_it_reads_them() {
        assert_eq!(size_of::<ThrowUniform>(), 112);
        assert_eq!(size_of::<LightsUniform>() % 16, 0);
        let mut p = crate::stage3d::Projector::new(crate::stage3d::ProjectorId(1), "P".into());
        p.placement.position = [1.0, 2.0, 3.0];
        let mut t = crate::stage3d::light::throw_of(&p, 1.5, (1920, 1280), 40, true);
        let options = Options {
            light: LightView::Overlap,
            white_nits: 250.0,
            ..Options::default()
        };
        let u = lights_uniform(&[t; 20], &options, 512, true);
        assert_eq!(u.head, [16.0, 3.0, 250.0, 512.0]);
        // A layer past the array is kept in it; a matrix that isn't numbers
        // switches the throw off.
        assert_eq!(u.throws[0].more[2], 15.0);
        assert_eq!(u.throws[0].more[3], 1.0);
        t.view_proj[0][0] = f32::NAN;
        let u = lights_uniform(&[t], &options, 1024, true);
        assert_eq!(u.throws[0].more[3], 0.0);
        // No pictures given: white.
        assert_eq!(
            lights_uniform(&[t], &options, 1024, false).throws[0].more[0],
            0.0
        );
        assert_eq!(u.throws[0].more[0], 1.0);
        assert_eq!(u.bright, BRIGHTNESS_STEPS);
    }
}
