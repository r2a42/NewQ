//! The live pass (mapping plan 4.2): a screen that can move is never in a
//! lens map; it is drawn here, every frame, in the thread that draws the
//! mapped picture, over the map pass, hidden where the map's `depth` says
//! something that stands still is nearer. One small draw per piece of a
//! moving screen (`MovingScreen`); no 3D renderer, no depth buffer, nothing
//! made per frame on the card but one uniform write.
//!
//! - Where: the piece's corners through `MappedFrame::lens` times
//!   `MovingScreen::matrix`, so the screen is where the frame's tick put it
//!   (position and content come in one job).
//! - Order: the main thread sorts the pieces far to near (`moving_for`), so
//!   a nearer one is drawn over a farther one; two passing *through* each
//!   other would be wrong, which scenery can't do.
//! - Hidden: `depth` is the distance along the ray from the lens (the
//!   lens's position, `eye_of(lens)`) to the nearest still thing, as
//!   `render3d::pick`'s `distance`; 1.0 (all 255) is nothing there and hides
//!   nothing. A piece loses to what is nearer than it by more than
//!   `lensmap::SCREEN_SLACK` of its distance (the bake's rule for a screen on
//!   a wall). A map without `depth` hides nothing.
//! - Edges: no multisampling; a screen of one piece fades over its last
//!   pixel by its uv's distance to its edge (derivatives first, before
//!   anything that branches). A faceted screen's pieces don't fade: their
//!   inner edges would show as seams, so its outer edge is a hard one.
//! - A front screen met from behind, and a piece that shows none of the
//!   frame's canvases (unfed, an LED screen, a fifth canvas), are black and
//!   still in the way, as the bake has them.

use super::{FORMAT, MovingScreen};
use crate::gpu::Gpu;
use crate::render3d::maths::{self, Mat4, Vec3};
use crate::render3d::{Paint, Scene};
use crate::show::{OutputId, Show};
use crate::stage3d::scene::Mesh;
use crate::stage3d::{Placement, SurfaceId};
use std::collections::HashMap;
use std::sync::Arc;

/// At most this many pieces of moving screens in one mapped frame: past it,
/// the farthest are left out (a faceted screen is many pieces).
pub const MAX_PIECES: usize = 256;

/// The live pass's shader (after `mapped::WGSL_UNPACK`).
pub const LIVE_SHADER: &str = r#"
struct LiveFrame {
    eye: vec4<f32>,
    map_size: vec2<f32>,
    target_size: vec2<f32>,
    depth_range: vec2<f32>,
    has_depth: u32,
    _a: u32,
};
struct LivePiece {
    clip: mat4x4<f32>,
    model: mat4x4<f32>,
    region: vec4<f32>,
    // feed: 0 none (black), n the frame's feed n - 1; front_only; fade.
    feed: u32,
    front_only: u32,
    fade: u32,
    _a: u32,
};
@group(0) @binding(0) var<uniform> f: LiveFrame;
@group(0) @binding(1) var depth_t: texture_2d<f32>;
@group(0) @binding(2) var feed0: texture_2d<f32>;
@group(0) @binding(3) var feed1: texture_2d<f32>;
@group(0) @binding(4) var feed2: texture_2d<f32>;
@group(0) @binding(5) var feed3: texture_2d<f32>;
@group(0) @binding(6) var samp: sampler;
@group(0) @binding(7) var<uniform> piece: LivePiece;

struct LiveOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world: vec3<f32>,
};

@vertex
fn vs_live(@location(0) p: vec3<f32>, @location(1) uv: vec2<f32>) -> LiveOut {
    var out: LiveOut;
    out.pos = piece.clip * vec4<f32>(p, 1.0);
    out.uv = uv;
    let w = piece.model * vec4<f32>(p, 1.0);
    out.world = w.xyz / w.w;
    return out;
}

@fragment
fn fs_live(v: LiveOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    // How near the edge, in pixels: derivatives first, in uniform control.
    let step = max(fwidth(v.uv), vec2<f32>(1e-6, 1e-6));
    let edge = min(v.uv, vec2<f32>(1.0, 1.0) - v.uv) / step;
    var fade = clamp(min(edge.x, edge.y) + 0.5, 0.0, 1.0);
    if (piece.fade == 0u) {
        fade = 1.0;
    }
    // Hidden where something that stands still is nearer.
    if (f.has_depth != 0u) {
        let dims = vec2<i32>(textureDimensions(depth_t));
        let scale = f.map_size / max(f.target_size, vec2<f32>(1.0, 1.0));
        let at = clamp(vec2<i32>(floor(v.pos.xy * scale)), vec2<i32>(0, 0), dims - vec2<i32>(1, 1));
        let t = unpack_depth01(textureLoad(depth_t, at, 0));
        if (t < 1.0) {
            let baked = mix(f.depth_range.x, f.depth_range.y, t);
            let mine = distance(v.world, f.eye.xyz);
            if (mine / (1.0 + SLACK) >= baked) {
                return vec4<f32>(0.0, 0.0, 0.0, 0.0);
            }
        }
    }
    if (piece.front_only != 0u && !front) {
        return vec4<f32>(0.0, 0.0, 0.0, fade);
    }
    let uv = piece.region.xy + clamp(v.uv, vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0)) * piece.region.zw;
    var c = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    switch piece.feed {
        case 1u: { c = textureSampleLevel(feed0, samp, uv, 0.0); }
        case 2u: { c = textureSampleLevel(feed1, samp, uv, 0.0); }
        case 3u: { c = textureSampleLevel(feed2, samp, uv, 0.0); }
        case 4u: { c = textureSampleLevel(feed3, samp, uv, 0.0); }
        default: { c = vec4<f32>(0.0, 0.0, 0.0, 1.0); }
    }
    return vec4<f32>(c.rgb, fade);
}
"#;

/// The live pass's shader as compiled: `WGSL_UNPACK`, the slack, then
/// `LIVE_SHADER`.
pub fn live_shader_source() -> String {
    format!(
        "{}\nconst SLACK: f32 = {:?};\n{LIVE_SHADER}",
        super::WGSL_UNPACK,
        crate::stage3d::lensmap::SCREEN_SLACK
    )
}

// ----- The list (pure, on the main thread) ----------------------------------------

/// A piece of a movable screen as the show has it (made again only when the
/// scene or which screens are movable changes): what `moving_for` places
/// at each tick.
#[derive(Clone, Debug)]
pub struct MovablePiece {
    pub surface: SurfaceId,
    pub mesh: Arc<Mesh>,
    pub mesh_key: u64,
    /// The piece in its screen's own frame (a facet's place on its screen;
    /// none for a flat or curved screen).
    pub own: Mat4,
    /// The canvas it shows and the part of it (`lensmap::part_region`);
    /// None: unfed or LED (black, in the way).
    pub shows: Option<(OutputId, [f32; 4])>,
    pub front_only: bool,
    /// The middle of the mesh's box, in the piece's own frame (for the
    /// far-to-near sort).
    pub middle: Vec3,
}

/// The pieces of `show`'s movable screens (`movable`, `Show::movable_screens`)
/// in `scene` (made by `Stage3d::scene` for `show`), in the scene's order.
pub fn movable_pieces(
    show: &Show,
    scene: &Scene,
    movable: &std::collections::BTreeSet<SurfaceId>,
) -> Vec<MovablePiece> {
    let stage = &show.stage3d;
    let mut out = Vec::new();
    if movable.is_empty() {
        return out;
    }
    for (i, part) in scene.parts.iter().enumerate() {
        let Some(id) = stage.part_surface(scene, i) else {
            continue;
        };
        if !movable.contains(&id) {
            continue;
        }
        let Some(s) = stage.surface(id) else { continue };
        let home = s.placement.matrix();
        let own = maths::mul(&rigid_inverse(&home), &part.transform);
        let shows = match part.paint {
            Paint::Output { output, .. } if !part.catch.led && !part.lit => {
                Some((output, crate::stage3d::lensmap::part_region(part)))
            }
            _ => None,
        };
        out.push(MovablePiece {
            surface: id,
            mesh: part.mesh.clone(),
            mesh_key: part.mesh_key,
            own,
            shows,
            front_only: part.catch.front_only,
            middle: middle_of(&part.mesh),
        });
    }
    out
}

/// The moving screens of one mapped frame: every piece of `pieces` at
/// where its screen is (`places`, the engine's moved screens taken once
/// this tick; a screen not there is at home), showing its canvas by its
/// place in `feeds` (the map's), sorted far to near from `eye` (the lens).
/// The same `places` for every projector: the same matrices on each.
pub fn moving_for(
    show: &Show,
    pieces: &[MovablePiece],
    places: &[(SurfaceId, Placement)],
    feeds: &[OutputId],
    eye: Vec3,
) -> Vec<MovingScreen> {
    let mut out: Vec<(f32, MovingScreen)> = Vec::with_capacity(pieces.len());
    for p in pieces {
        let at = places
            .iter()
            .find(|(id, _)| *id == p.surface)
            .map(|(_, at)| *at)
            .or_else(|| show.stage3d.surface(p.surface).map(|s| s.placement));
        let Some(at) = at else { continue };
        let matrix = maths::mul(&at.matrix(), &p.own);
        let (feed, region) = match p.shows {
            Some((output, region)) => (
                feeds
                    .iter()
                    .take(super::MAX_FEEDS)
                    .position(|f| *f == output)
                    .map(|i| i as u8),
                region,
            ),
            None => (None, [0.0, 0.0, 1.0, 1.0]),
        };
        let far = maths::length(maths::sub(maths::transform_point(&matrix, p.middle), eye));
        out.push((
            if far.is_finite() { far } else { f32::MAX },
            MovingScreen {
                surface: p.surface,
                mesh: p.mesh.clone(),
                mesh_key: p.mesh_key,
                matrix,
                feed,
                region,
                front_only: p.front_only,
            },
        ));
    }
    // Far to near (a stable sort: pieces as far as each other keep the
    // scene's order).
    out.sort_by(|a, b| b.0.total_cmp(&a.0));
    out.into_iter().map(|(_, m)| m).collect()
}

/// The middle of a mesh's box.
fn middle_of(mesh: &Mesh) -> Vec3 {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in &mesh.positions {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    if mesh.positions.is_empty() {
        return [0.0; 3];
    }
    std::array::from_fn(|k| (lo[k] + hi[k]) * 0.5)
}

/// The inverse of a turn then a move (a `Placement::matrix`).
pub fn rigid_inverse(m: &Mat4) -> Mat4 {
    // Column-major: m[c][r]. The turn's transpose, and minus it times the move.
    let mut out = maths::IDENTITY;
    for c in 0..3 {
        for r in 0..3 {
            out[c][r] = m[r][c];
        }
    }
    let t = [m[3][0], m[3][1], m[3][2]];
    for r in 0..3 {
        out[3][r] = -(0..3).map(|k| out[k][r] * t[k]).sum::<f32>();
    }
    out
}

/// Where the lens is, from its matrix (world to clip): the one point whose
/// clip x, y and w are all 0. None for a matrix that has none.
pub fn eye_of(lens: &Mat4) -> Option<Vec3> {
    // Rows 0, 1 and 3 of `lens`: a[r] . e = -lens[3][r].
    let row = |r: usize| -> [f64; 4] {
        [
            f64::from(lens[0][r]),
            f64::from(lens[1][r]),
            f64::from(lens[2][r]),
            f64::from(lens[3][r]),
        ]
    };
    let (a, b, c) = (row(0), row(1), row(3));
    let det3 = |m: [[f64; 3]; 3]| {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let m = [[a[0], a[1], a[2]], [b[0], b[1], b[2]], [c[0], c[1], c[2]]];
    let d = det3(m);
    if !d.is_finite() || d.abs() < 1e-12 {
        return None;
    }
    let rhs = [-a[3], -b[3], -c[3]];
    let solve = |k: usize| {
        let mut mk = m;
        for (r, v) in rhs.iter().enumerate() {
            mk[r][k] = *v;
        }
        (det3(mk) / d) as f32
    };
    let e = [solve(0), solve(1), solve(2)];
    e.iter().all(|v| v.is_finite()).then_some(e)
}

// ----- On the card (the drawing thread) -------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct FrameUniform {
    eye: [f32; 4],
    map_size: [f32; 2],
    target_size: [f32; 2],
    depth_range: [f32; 2],
    has_depth: u32,
    _a: u32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct PieceUniform {
    clip: [[f32; 4]; 4],
    model: [[f32; 4]; 4],
    region: [f32; 4],
    feed: u32,
    front_only: u32,
    fade: u32,
    _a: u32,
}

/// The live pass's pipeline, made at the first frame with a moving screen.
pub(super) struct LiveKit {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    /// Each piece's uniform's place in the pieces' buffer.
    stride: u64,
}

/// A mesh on the card, kept by its key.
pub(super) struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    count: u32,
    /// The drawer's `generation` it was last drawn at.
    used: u64,
}

/// What one frame's live pass keeps from frame to frame.
pub(super) struct FrameLive {
    frame: wgpu::Buffer,
    pieces: wgpu::Buffer,
    /// The pieces' uniforms as written (kept, so a frame allocates nothing).
    staging: Vec<u8>,
    /// The bind group, and the map and canvases it was made for.
    group: Option<(
        Arc<super::LensMap>,
        [u64; super::MAX_FEEDS],
        wgpu::BindGroup,
    )>,
}

impl LiveKit {
    pub(super) fn new(gpu: &Gpu) -> LiveKit {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("live pass"),
            source: wgpu::ShaderSource::Wgsl(live_shader_source().into()),
        });
        let texture = |binding: u32, filterable: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let uniform = |binding: u32, dynamic: bool, size: usize| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: dynamic,
                min_binding_size: wgpu::BufferSize::new(size as u64),
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("live pass"),
            entries: &[
                uniform(0, false, std::mem::size_of::<FrameUniform>()),
                texture(1, false),
                texture(2, true),
                texture(3, true),
                texture(4, true),
                texture(5, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                uniform(7, true, std::mem::size_of::<PieceUniform>()),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("live pass"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("live pass"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_live"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 20,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 12,
                            shader_location: 1,
                        },
                    ],
                })],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_live"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FORMAT,
                    // Over what the map pass drew by the fade; its alpha kept.
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::SrcAlpha,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::Zero,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let align = u64::from(device.limits().min_uniform_buffer_offset_alignment.max(1));
        let size = std::mem::size_of::<PieceUniform>() as u64;
        LiveKit {
            pipeline,
            layout,
            stride: size.div_ceil(align) * align,
        }
    }

    pub(super) fn frame_live(&self, gpu: &Gpu) -> FrameLive {
        let buffer = |label: &str, size: u64| {
            gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        FrameLive {
            frame: buffer(
                "live pass frame",
                std::mem::size_of::<FrameUniform>() as u64,
            ),
            pieces: buffer("live pass pieces", self.stride * MAX_PIECES as u64),
            staging: Vec::with_capacity((self.stride as usize) * 16),
            group: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn group(
        &self,
        gpu: &Gpu,
        live: &FrameLive,
        depth: &wgpu::TextureView,
        feeds: [&wgpu::TextureView; super::MAX_FEEDS],
        sampler: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        fn tex(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::TextureView(view),
            }
        }
        gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("live pass"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: live.frame.as_entire_binding(),
                },
                tex(1, depth),
                tex(2, feeds[0]),
                tex(3, feeds[1]),
                tex(4, feeds[2]),
                tex(5, feeds[3]),
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &live.pieces,
                        offset: 0,
                        size: wgpu::BufferSize::new(std::mem::size_of::<PieceUniform>() as u64),
                    }),
                },
            ],
        })
    }
}

/// The pieces of `moving` a frame draws: at most `MAX_PIECES`, the nearest
/// (the list is far to near).
pub(super) fn drawn(moving: &[MovingScreen]) -> &[MovingScreen] {
    &moving[moving.len().saturating_sub(MAX_PIECES)..]
}

/// Everything the live pass of one frame needs before its render pass
/// begins: the meshes on the card, the uniforms written, the bind group.
/// Returns false when there is nothing to draw.
#[allow(clippy::too_many_arguments)]
pub(super) fn prepare(
    gpu: &Gpu,
    kit: &LiveKit,
    live: &mut FrameLive,
    meshes: &mut HashMap<u64, GpuMesh>,
    generation: u64,
    frame: &super::MappedFrame,
    feeds: [&wgpu::TextureView; super::MAX_FEEDS],
    serials: [u64; super::MAX_FEEDS],
    blank_depth: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> bool {
    let moving = drawn(&frame.moving);
    if moving.is_empty() {
        return false;
    }
    let mut made = false;
    for m in moving {
        if let Some(g) = meshes.get_mut(&m.mesh_key) {
            g.used = generation;
            continue;
        }
        if let Some(mut g) = upload(gpu, &m.mesh) {
            g.used = generation;
            meshes.insert(m.mesh_key, g);
            made = true;
        }
    }
    // Let go of meshes no frame drew this time, only when one was made (so
    // a still show makes and drops nothing).
    if made {
        meshes.retain(|_, g| g.used + 1 >= generation);
    }
    let eye = eye_of(&frame.lens);
    let depth = frame.map.depth_view().filter(|_| eye.is_some());
    let u = FrameUniform {
        eye: match eye {
            Some(e) => [e[0], e[1], e[2], 1.0],
            None => [0.0; 4],
        },
        map_size: [frame.map.size.0 as f32, frame.map.size.1 as f32],
        target_size: [frame.target.width as f32, frame.target.height as f32],
        depth_range: [frame.map.depth_range.0, frame.map.depth_range.1],
        has_depth: u32::from(depth.is_some()),
        _a: 0,
    };
    gpu.queue
        .write_buffer(&live.frame, 0, bytemuck::bytes_of(&u));
    let stride = kit.stride as usize;
    live.staging.clear();
    live.staging.resize(moving.len() * stride, 0);
    for (i, m) in moving.iter().enumerate() {
        let pieces = frame
            .moving
            .iter()
            .filter(|o| o.surface == m.surface)
            .take(2)
            .count();
        let p = PieceUniform {
            clip: maths::mul(&frame.lens, &m.matrix),
            model: m.matrix,
            region: m.region,
            feed: m.feed.map_or(0, |f| u32::from(f) + 1),
            front_only: u32::from(m.front_only),
            fade: u32::from(pieces == 1),
            _a: 0,
        };
        live.staging[i * stride..i * stride + std::mem::size_of::<PieceUniform>()]
            .copy_from_slice(bytemuck::bytes_of(&p));
    }
    gpu.queue.write_buffer(&live.pieces, 0, &live.staging);
    let depth_view = depth.unwrap_or(blank_depth);
    let stale = live
        .group
        .as_ref()
        .is_none_or(|(m, s, _)| !Arc::ptr_eq(m, &frame.map) || *s != serials);
    if stale {
        let group = kit.group(gpu, live, depth_view, feeds, sampler);
        live.group = Some((frame.map.clone(), serials, group));
    }
    true
}

/// Record the live pass's draws into the map pass's `pass` (after its
/// triangle): one per piece, `prepare`d.
pub(super) fn record(
    pass: &mut wgpu::RenderPass<'_>,
    kit: &LiveKit,
    live: &FrameLive,
    meshes: &HashMap<u64, GpuMesh>,
    frame: &super::MappedFrame,
) {
    let Some((_, _, group)) = &live.group else {
        return;
    };
    pass.set_pipeline(&kit.pipeline);
    for (i, m) in drawn(&frame.moving).iter().enumerate() {
        let Some(g) = meshes.get(&m.mesh_key) else {
            continue;
        };
        pass.set_bind_group(0, group, &[(i as u64 * kit.stride) as u32]);
        pass.set_vertex_buffer(0, g.vertices.slice(..));
        pass.set_index_buffer(g.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..g.count, 0, 0..1);
    }
}

/// A mesh's corners and uvs, and its triangles, on the card (None for one
/// with nothing to draw).
fn upload(gpu: &Gpu, mesh: &Mesh) -> Option<GpuMesh> {
    use wgpu::util::DeviceExt;
    let n = mesh.positions.len();
    let indices: Vec<u32> = mesh
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|t| t.iter().all(|i| (*i as usize) < n))
        .flatten()
        .copied()
        .collect();
    if indices.is_empty() {
        return None;
    }
    let mut vertices: Vec<f32> = Vec::with_capacity(n * 5);
    for (i, p) in mesh.positions.iter().enumerate() {
        let uv = mesh.uvs.get(i).copied().unwrap_or([0.0, 0.0]);
        vertices.extend_from_slice(&[p[0], p[1], p[2], uv[0], uv[1]]);
    }
    let vertices = gpu
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("live pass mesh"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
    let index_buffer = gpu
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("live pass mesh"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
    Some(GpuMesh {
        vertices,
        indices: index_buffer,
        count: indices.len() as u32,
        used: 0,
    })
}
