//! Mapped projectors' pictures (`.claude/notes/mapping-plan.md`, 4.1 and
//! 4.5): a projector with `mapped` set shows, at every pixel, the content of
//! the spot on a screen that pixel lands on. The content is on the canvases
//! (displays with no monitor that a screen shows); which spot of which canvas
//! each pixel sees is a lens map (`LensMap`), made off the output path on the
//! stage-render thread (`render3d::bake`) and drawn here.
//!
//! One drawer, three users: an output window (`render::output_loop`), a
//! stream or recording (`render::capture_loop`) and, later, the light in a
//! 3D view (`stage_render`). Each keeps a `Drawer` and calls `draw` for each
//! `MappedFrame` it is given, before its own composite, which then shows the
//! frame's `target` as an ordinary layer (so the fine-tune, the hand edge
//! blend, sub displays and cards all work as they do for any display).
//!
//! The map's pictures, byte for byte (`pack_place`, `pack_how`,
//! `pack_depth`, and `WGSL_UNPACK` for the shaders, tested against each
//! other):
//! - `place` (`Rgba8Unorm`, read with `textureLoad`): where in the feed's
//!   picture the pixel looks, `(0, 0)` its top-left corner and `(1, 1)` its
//!   bottom-right, x right and y down (the compositor's uv). x in r and g, y
//!   in b and a, each 16 bits with the high byte first; 65535 is 1.0.
//! - `how` (`Rgba8Unorm`): r the share of light (1 until computed blend), g
//!   which feed (0 none, black; n is `LensMap::feeds[n - 1]`), b the coverage
//!   at a screen's edge, a 255.
//! - `depth` (`Rgba8Unorm`, only when the show has a movable screen): the
//!   distance from the lens to the nearest thing that stands still, over
//!   `LensMap::depth_range`, in 24 bits, high byte first in r, g, b; a 255.
//!   Nothing there is 1.0 (as far as the range goes).

#![allow(dead_code)]

use crate::gpu::{Gpu, LayerTexture};
use crate::render::OutputGroup;
use crate::show::{NodeId, OutputId, Show};
use crate::stage3d::SurfaceId;
use crate::stage3d::projector::ProjectorId;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// At most this many canvases feed one mapped projector (said in words in
/// the UI).
pub const MAX_FEEDS: usize = 4;
/// At most this many mapped frames one owner draws in one go (a window has
/// one; the rest is room).
pub const MAX_FRAMES: usize = 4;
/// Every picture of a map, and every target and canvas the drawer draws.
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// The largest number `pack_depth` writes (24 bits).
pub const DEPTH_STEPS: u32 = (1 << 24) - 1;

// ----- Packing (pure) --------------------------------------------------------

/// A place in a feed's picture (x, y in 0..1, y down) as `place`'s bytes.
/// Outside 0..1 is kept to the edge; a number that isn't one is 0.
pub fn pack_place(at: [f32; 2]) -> [u8; 4] {
    let q = |v: f32| -> u16 {
        if v.is_finite() {
            (v.clamp(0.0, 1.0) * 65535.0).round() as u16
        } else {
            0
        }
    };
    let (x, y) = (q(at[0]), q(at[1]));
    [(x >> 8) as u8, x as u8, (y >> 8) as u8, y as u8]
}

/// `pack_place` undone.
pub fn unpack_place(px: [u8; 4]) -> [f32; 2] {
    let x = u16::from(px[0]) << 8 | u16::from(px[1]);
    let y = u16::from(px[2]) << 8 | u16::from(px[3]);
    [f32::from(x) / 65535.0, f32::from(y) / 65535.0]
}

/// A distance (metres from the lens) over `range` (nearest, farthest) as
/// `depth`'s bytes. Past the range is kept to its ends; a distance that
/// isn't a number, or a range that isn't one, is the far end.
pub fn pack_depth(distance: f32, range: (f32, f32)) -> [u8; 4] {
    let (near, far) = range;
    let t = if distance.is_finite() && near.is_finite() && far.is_finite() && far > near {
        ((distance - near) / (far - near)).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let v = (f64::from(t) * f64::from(DEPTH_STEPS)).round() as u32;
    [(v >> 16) as u8, (v >> 8) as u8, v as u8, 255]
}

/// `pack_depth` undone (to metres from the lens).
pub fn unpack_depth(px: [u8; 4], range: (f32, f32)) -> f32 {
    let v = u32::from(px[0]) << 16 | u32::from(px[1]) << 8 | u32::from(px[2]);
    let t = v as f32 / DEPTH_STEPS as f32;
    range.0 + t * (range.1 - range.0)
}

/// A pixel's `how`, unpacked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct How {
    /// The share of the light this projector gives here (0..1).
    pub share: f32,
    /// Which of the map's feeds it shows (an index into `LensMap::feeds`);
    /// None: nothing, black.
    pub feed: Option<u8>,
    /// How much of the pixel the screen covers (0..1), at its edge.
    pub coverage: f32,
}

impl How {
    /// A pixel that shows nothing.
    pub const NONE: How = How {
        share: 0.0,
        feed: None,
        coverage: 0.0,
    };

    /// All of feed `feed`, wholly covered, all of the light.
    pub fn feed(feed: u8) -> How {
        How {
            share: 1.0,
            feed: Some(feed),
            coverage: 1.0,
        }
    }
}

fn unit_byte(v: f32) -> u8 {
    if v.is_finite() {
        (v.clamp(0.0, 1.0) * 255.0).round() as u8
    } else {
        0
    }
}

/// `how`'s bytes. A feed past `MAX_FEEDS` shows nothing.
pub fn pack_how(h: How) -> [u8; 4] {
    let g = match h.feed {
        Some(f) if usize::from(f) < MAX_FEEDS => f + 1,
        _ => 0,
    };
    [unit_byte(h.share), g, unit_byte(h.coverage), 255]
}

/// `pack_how` undone.
pub fn unpack_how(px: [u8; 4]) -> How {
    How {
        share: f32::from(px[0]) / 255.0,
        feed: (px[1] > 0 && usize::from(px[1]) <= MAX_FEEDS).then(|| px[1] - 1),
        coverage: f32::from(px[2]) / 255.0,
    }
}

/// The same unpacking for a shader that has read a map's pixel with
/// `textureLoad` (each byte as a 0..1 float). Included by the map pass and
/// the bake's tests.
pub const WGSL_UNPACK: &str = r#"
fn map_byte(v: f32) -> u32 {
    return u32(round(clamp(v, 0.0, 1.0) * 255.0));
}
// `place`'s pixel: where in the feed's picture (0..1, y down).
fn unpack_place(c: vec4<f32>) -> vec2<f32> {
    let x = map_byte(c.r) * 256u + map_byte(c.g);
    let y = map_byte(c.b) * 256u + map_byte(c.a);
    return vec2<f32>(f32(x), f32(y)) / 65535.0;
}
// `how`'s pixel: which feed (0 none, else the feed's index + 1).
fn unpack_feed(h: vec4<f32>) -> u32 {
    return map_byte(h.g);
}
// `depth`'s pixel: 0..1 over the map's depth range.
fn unpack_depth01(c: vec4<f32>) -> f32 {
    let v = map_byte(c.r) * 65536u + map_byte(c.g) * 256u + map_byte(c.b);
    return f32(v) / 16777215.0;
}
"#;

// ----- The lens map -------------------------------------------------------------

/// Every picture written into a map counted, so two maps never look alike to
/// `render::Shot` (an `Arc`'s address may be used again once freed).
static MAPS: AtomicU64 = AtomicU64::new(0);

/// A map made on the CPU (`stage3d::lensmap::reference`, and the tests' made
/// up maps), row by row from the top-left, in `place`/`how`/`depth`'s bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct CpuMap {
    pub size: (u32, u32),
    pub place: Vec<[u8; 4]>,
    pub how: Vec<[u8; 4]>,
    pub depth: Option<Vec<[u8; 4]>>,
    pub depth_range: (f32, f32),
    pub feeds: Vec<OutputId>,
}

impl CpuMap {
    /// A map of `size` that shows nothing anywhere.
    pub fn black(size: (u32, u32)) -> CpuMap {
        let n = size.0 as usize * size.1 as usize;
        CpuMap {
            size,
            place: vec![[0; 4]; n],
            how: vec![pack_how(How::NONE); n],
            depth: None,
            depth_range: (0.0, 1.0),
            feeds: Vec::new(),
        }
    }

    /// Set pixel (x, y), if it is in the map.
    pub fn set(&mut self, x: u32, y: u32, place: [f32; 2], how: How) {
        if x < self.size.0 && y < self.size.1 {
            let i = (y * self.size.0 + x) as usize;
            self.place[i] = pack_place(place);
            self.how[i] = pack_how(how);
        }
    }
}

/// A lens map on the graphics card: what one mapped projector's pixels see,
/// for one size. Made by the stage-render thread (or `from_cpu`), held by
/// the app (`StageLive::lens_map`) and shared with the threads that draw it.
pub struct LensMap {
    pub size: (u32, u32),
    pub place: wgpu::Texture,
    pub how: wgpu::Texture,
    pub depth: Option<wgpu::Texture>,
    /// What `depth`'s 0 and 1 are, in metres from the lens.
    pub depth_range: (f32, f32),
    /// The canvases its `how.g` names, in order: feed n is `feeds[n - 1]`.
    pub feeds: Vec<OutputId>,
    /// What it was made for (the scene of still things, the projector's
    /// placement and lens, the feeds' regions, the size): the same key, the
    /// same map.
    pub key: u64,
    stamp: u64,
    place_view: wgpu::TextureView,
    how_view: wgpu::TextureView,
    depth_view: Option<wgpu::TextureView>,
}

impl LensMap {
    /// What every picture of a map can be used for: read by the map pass,
    /// written by an upload or drawn into by the bake, read back by tests.
    pub const USAGE: wgpu::TextureUsages = wgpu::TextureUsages::TEXTURE_BINDING
        .union(wgpu::TextureUsages::COPY_DST)
        .union(wgpu::TextureUsages::RENDER_ATTACHMENT)
        .union(wgpu::TextureUsages::COPY_SRC);

    /// One picture of a map (`place`, `how` or `depth`), `size` big.
    pub fn texture(gpu: &Gpu, size: (u32, u32), label: &str) -> wgpu::Texture {
        gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size.0.max(1),
                height: size.1.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: Self::USAGE,
            view_formats: &[],
        })
    }

    /// A map from pictures already drawn (made with `texture`).
    pub fn new(
        size: (u32, u32),
        place: wgpu::Texture,
        how: wgpu::Texture,
        depth: Option<(wgpu::Texture, (f32, f32))>,
        feeds: Vec<OutputId>,
        key: u64,
    ) -> LensMap {
        let view = |t: &wgpu::Texture| t.create_view(&Default::default());
        let (depth, depth_range) = match depth {
            Some((t, r)) => (Some(t), r),
            None => (None, (0.0, 1.0)),
        };
        LensMap {
            size,
            place_view: view(&place),
            how_view: view(&how),
            depth_view: depth.as_ref().map(view),
            place,
            how,
            depth,
            depth_range,
            feeds,
            key,
            stamp: MAPS.fetch_add(1, Ordering::Relaxed) + 1,
        }
    }

    /// A map made on the CPU, uploaded.
    pub fn from_cpu(gpu: &Gpu, map: &CpuMap, key: u64) -> LensMap {
        let size = (map.size.0.max(1), map.size.1.max(1));
        let up = |label: &str, px: &[[u8; 4]]| {
            let t = Self::texture(gpu, size, label);
            let n = size.0 as usize * size.1 as usize;
            let mut bytes: Vec<u8> = px.iter().take(n).flatten().copied().collect();
            bytes.resize(n * 4, 0);
            gpu.queue.write_texture(
                t.as_image_copy(),
                &bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(size.0 * 4),
                    rows_per_image: Some(size.1),
                },
                t.size(),
            );
            t
        };
        let place = up("lens map place", &map.place);
        let how = up("lens map how", &map.how);
        let depth = map
            .depth
            .as_ref()
            .map(|d| (up("lens map depth", d), map.depth_range));
        Self::new(size, place, how, depth, map.feeds.clone(), key)
    }

    /// Unique to this map, never 0.
    pub fn stamp(&self) -> u64 {
        self.stamp
    }

    pub fn place_view(&self) -> &wgpu::TextureView {
        &self.place_view
    }

    pub fn how_view(&self) -> &wgpu::TextureView {
        &self.how_view
    }

    pub fn depth_view(&self) -> Option<&wgpu::TextureView> {
        self.depth_view.as_ref()
    }
}

// ----- What is drawn ------------------------------------------------------------

/// What a lens map is made for: a window, a stream, or a 3D view's light.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MapUse {
    /// A mapped projector's window on this computer, at its monitor's size:
    /// output work.
    Window,
    /// A stream or recording of a mapped display (the primary's), at the
    /// stream's size: output work.
    Stream,
    /// A 3D view's light, at the throw's size: preview work, governed.
    Preview,
}

/// Who draws a mapped picture: each has uniforms of its own for the
/// canvases (`gpu::Slot::Map`), so a window and a stream of the same
/// projector, or two windows showing one canvas, never share them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MapOwner {
    /// An output window, by its render slot's number (`Slot::Window`).
    Window(u8),
    /// A stream, by its number (`Slot::Capture`).
    Capture(u8),
    /// The stage-render thread's throws.
    Throw(u8),
}

impl MapOwner {
    /// The owner's number in `Slot::Map`.
    pub fn number(self) -> u8 {
        match self {
            MapOwner::Window(n) => n % 8,
            MapOwner::Capture(n) => 8 + n % 8,
            MapOwner::Throw(n) => 16 + n % 16,
        }
    }

    /// The slot feed `feed` of frame `frame` is drawn through.
    pub fn slot(self, frame: usize, feed: usize) -> crate::gpu::Slot {
        let place = (frame % MAX_FRAMES) * MAX_FEEDS + feed % MAX_FEEDS;
        crate::gpu::Slot::Map(self.number(), place as u8)
    }
}

/// One canvas a mapped frame shows: its layers on its plain picture, as
/// `stage_live::feed_groups` makes them (no keystone, mesh or blend; its sub
/// displays in their areas).
#[derive(Clone)]
pub struct FeedFrame {
    pub output: OutputId,
    /// Empty for a canvas with nothing to show (it is black, and keeps its
    /// place in the list).
    pub groups: Vec<OutputGroup>,
    /// Its picture's size.
    pub canvas: (u32, u32),
}

/// A screen that can move, drawn live over the map (round 3; none yet).
#[derive(Clone, Debug, PartialEq)]
pub struct MovingScreen {
    pub surface: SurfaceId,
    /// Where the screen is this frame (column-major, its own frame to the
    /// room's).
    pub matrix: [[f32; 4]; 4],
    /// Which of the frame's feeds it shows.
    pub feed: u8,
    /// The part of that feed's picture (`[x, y, w, h]`).
    pub region: [f32; 4],
}

/// One mapped picture to draw: `map` looked up at every pixel of `target`,
/// each pixel showing its feed's picture at the place the map gives.
#[derive(Clone)]
pub struct MappedFrame {
    /// Where the mapped picture is drawn (made by the main thread, the
    /// map's size for a window; shown by the window's own composite as the
    /// mapped display's one layer).
    pub target: Arc<LayerTexture>,
    /// Everything that stands still.
    pub map: Arc<LensMap>,
    /// The canvases, in `map.feeds`' order: `feeds[i]` is `map.feeds[i]`.
    pub feeds: Vec<FeedFrame>,
    /// Round 3: far to near. Empty until then.
    pub moving: Vec<MovingScreen>,
    /// The projector's matrix, for the moving screens.
    pub lens: [[f32; 4]; 4],
    /// Light shares applied (`how.r ^ (1 / gamma)`): false for a stream.
    pub shares: bool,
    /// The projector's gamma, for the shares.
    pub gamma: f32,
}

/// The map pass's shader (with `WGSL_UNPACK` in front): one triangle over
/// the whole target; each pixel looks the map up at its own place (one map
/// pixel per picture pixel when the target is the map's size, as a
/// window's is) and samples its feed's canvas at the place the map gives.
/// No derivatives (`textureSampleLevel`), so a branch per feed is fine.
pub const MAP_SHADER: &str = r#"
struct MapUniform {
    map_size: vec2<f32>,
    target_size: vec2<f32>,
    shares: u32,
    inv_gamma: f32,
    _a: u32,
    _b: u32,
};
@group(0) @binding(0) var<uniform> u: MapUniform;
@group(0) @binding(1) var place_t: texture_2d<f32>;
@group(0) @binding(2) var how_t: texture_2d<f32>;
@group(0) @binding(3) var feed0: texture_2d<f32>;
@group(0) @binding(4) var feed1: texture_2d<f32>;
@group(0) @binding(5) var feed2: texture_2d<f32>;
@group(0) @binding(6) var feed3: texture_2d<f32>;
@group(0) @binding(7) var samp: sampler;

@vertex
fn vs_map(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

@fragment
fn fs_map(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let most = max(vec2<i32>(u.map_size) - vec2<i32>(1, 1), vec2<i32>(0, 0));
    let scale = u.map_size / max(u.target_size, vec2<f32>(1.0, 1.0));
    let at = clamp(vec2<i32>(floor(p.xy * scale)), vec2<i32>(0, 0), most);
    let place = textureLoad(place_t, at, 0);
    let how = textureLoad(how_t, at, 0);
    let uv = unpack_place(place);
    var c = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    switch unpack_feed(how) {
        case 1u: { c = textureSampleLevel(feed0, samp, uv, 0.0); }
        case 2u: { c = textureSampleLevel(feed1, samp, uv, 0.0); }
        case 3u: { c = textureSampleLevel(feed2, samp, uv, 0.0); }
        case 4u: { c = textureSampleLevel(feed3, samp, uv, 0.0); }
        default: { return vec4<f32>(0.0, 0.0, 0.0, 1.0); }
    }
    var k = how.b;
    if (u.shares != 0u) {
        k = k * pow(max(how.r, 0.0), u.inv_gamma);
    }
    return vec4<f32>(c.rgb * k, 1.0);
}
"#;

/// The map pass's shader as compiled: `WGSL_UNPACK` then `MAP_SHADER`.
pub fn map_shader_source() -> String {
    format!("{WGSL_UNPACK}{MAP_SHADER}")
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct MapUniform {
    map_size: [f32; 2],
    target_size: [f32; 2],
    shares: u32,
    inv_gamma: f32,
    _pad: [u32; 2],
}

/// What a drawer makes once: the map pass and the compositor's pipeline for
/// the canvases.
struct Kit {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    compositor: wgpu::RenderPipeline,
    /// Bound for a feed the frame doesn't have.
    blank: wgpu::TextureView,
}

/// A canvas's picture, kept from frame to frame (made again only at a new
/// size).
struct Canvas {
    view: wgpu::TextureView,
    size: (u32, u32),
    /// Which texture this is (a new one is a new bind group).
    serial: u64,
}

/// What a drawer keeps for each frame it draws in one go.
#[derive(Default)]
struct FrameKit {
    uniform: Option<wgpu::Buffer>,
    canvases: [Option<Canvas>; MAX_FEEDS],
    /// The bind group, and the map and canvases it was made for.
    group: Option<(Arc<LensMap>, [u64; MAX_FEEDS], wgpu::BindGroup)>,
    /// The target last drawn into, and a view to draw into it.
    target: Option<(Arc<LayerTexture>, wgpu::TextureView)>,
}

/// Draws mapped pictures: one per thread that draws them (a window's, a
/// stream's). Keeps its pipelines, the canvases' pictures and the bind
/// groups between frames, so a frame makes nothing new on the card unless a
/// size, a map or a target changed. Made when the first mapped frame comes:
/// a thread that never draws one never makes any of it.
pub struct Drawer {
    /// None: the graphics card refused the map pass (said once, in the log);
    /// mapped pictures stay black.
    kit: Option<Kit>,
    frames: Vec<FrameKit>,
    serial: u64,
}

impl Drawer {
    pub fn new(gpu: &Gpu) -> Drawer {
        let kit = checked(&gpu.device, || Kit::new(gpu));
        Drawer {
            kit,
            frames: Vec::new(),
            serial: 0,
        }
    }

    /// Whether the map pass was made (a refused one draws nothing).
    pub fn works(&self) -> bool {
        self.kit.is_some()
    }

    /// Record `frame`'s canvas passes and map pass, through `owner`'s slots,
    /// as its first (or only) frame. The command buffers are to be submitted
    /// together with (before) the composite that shows `frame.target`.
    pub fn draw(
        &mut self,
        gpu: &Gpu,
        owner: MapOwner,
        frame: &MappedFrame,
    ) -> Vec<wgpu::CommandBuffer> {
        self.draw_all(gpu, owner, std::slice::from_ref(frame))
    }

    /// `draw` for each of `frames` (each through slots of its own; at most
    /// `MAX_FRAMES`).
    pub fn draw_all(
        &mut self,
        gpu: &Gpu,
        owner: MapOwner,
        frames: &[MappedFrame],
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(kit) = &self.kit else {
            return Vec::new();
        };
        let mut cmds = Vec::with_capacity(frames.len() * (MAX_FEEDS + 1));
        let most = gpu.device.limits().max_texture_dimension_2d.max(1);
        for (i, frame) in frames.iter().take(MAX_FRAMES).enumerate() {
            if self.frames.len() <= i {
                self.frames.resize_with(i + 1, FrameKit::default);
            }
            let fk = &mut self.frames[i];
            // The canvases: each feed's layers on its plain picture.
            for (j, feed) in frame.feeds.iter().take(MAX_FEEDS).enumerate() {
                let size = capped(feed.canvas, most);
                if fk.canvases[j].as_ref().is_none_or(|c| c.size != size) {
                    self.serial += 1;
                    fk.canvases[j] = Some(Canvas {
                        view: canvas_texture(gpu, size).create_view(&Default::default()),
                        size,
                        serial: self.serial,
                    });
                }
                let Some(canvas) = &fk.canvases[j] else {
                    continue;
                };
                let draw: Vec<_> = feed.groups.iter().map(OutputGroup::draw).collect();
                cmds.push(gpu.composite(
                    &kit.compositor,
                    owner.slot(i, j),
                    &canvas.view,
                    &draw,
                    None,
                ));
            }
            let uniform = fk.uniform.get_or_insert_with(|| {
                gpu.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("map pass uniform"),
                    size: std::mem::size_of::<MapUniform>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            });
            let target_size = (frame.target.width, frame.target.height);
            let gamma = if frame.gamma.is_finite() && frame.gamma > 0.1 {
                frame.gamma
            } else {
                crate::stage3d::projector::DEFAULT_GAMMA
            };
            let u = MapUniform {
                map_size: [frame.map.size.0 as f32, frame.map.size.1 as f32],
                target_size: [target_size.0 as f32, target_size.1 as f32],
                shares: u32::from(frame.shares),
                inv_gamma: 1.0 / gamma,
                _pad: [0; 2],
            };
            gpu.queue.write_buffer(uniform, 0, bytemuck::bytes_of(&u));
            // The feeds the frame has, in the map's order; any it lacks blank.
            let serials: [u64; MAX_FEEDS] = std::array::from_fn(|j| {
                if j < frame.feeds.len() {
                    fk.canvases[j].as_ref().map_or(0, |c| c.serial)
                } else {
                    0
                }
            });
            let stale = fk
                .group
                .as_ref()
                .is_none_or(|(m, s, _)| !Arc::ptr_eq(m, &frame.map) || *s != serials);
            if stale {
                let views: [&wgpu::TextureView; MAX_FEEDS] =
                    std::array::from_fn(|j| match (&fk.canvases[j], serials[j]) {
                        (Some(c), s) if s != 0 => &c.view,
                        _ => &kit.blank,
                    });
                let group = kit.group(gpu, uniform, &frame.map, views);
                fk.group = Some((frame.map.clone(), serials, group));
            }
            if fk
                .target
                .as_ref()
                .is_none_or(|(t, _)| !Arc::ptr_eq(t, &frame.target))
            {
                fk.target = frame
                    .target
                    .render_view()
                    .map(|v| (frame.target.clone(), v));
            }
            let (Some((_, _, group)), Some((_, view))) = (&fk.group, &fk.target) else {
                continue;
            };
            let mut encoder = gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("map pass"),
                });
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("map pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
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
                pass.set_pipeline(&kit.pipeline);
                pass.set_bind_group(0, group, &[]);
                pass.draw(0..3, 0..1);
            }
            cmds.push(encoder.finish());
            // Filled once: its stamp stays, so `render::Shot` tells a new
            // picture by the frame's feeds and map, not by this.
            if !frame.target.is_filled() {
                frame.target.mark_drawn();
            }
        }
        cmds
    }
}

/// A canvas's size within what the card can make, its shape kept.
fn capped(size: (u32, u32), most: u32) -> (u32, u32) {
    let (w, h) = (size.0.max(1), size.1.max(1));
    let big = w.max(h);
    if big <= most {
        return (w, h);
    }
    let s = f64::from(most) / f64::from(big);
    let f = |v: u32| ((f64::from(v) * s).round() as u32).clamp(1, most);
    (f(w), f(h))
}

fn canvas_texture(gpu: &Gpu, size: (u32, u32)) -> wgpu::Texture {
    gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("mapped canvas"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

impl Kit {
    fn new(gpu: &Gpu) -> Kit {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("map pass"),
            source: wgpu::ShaderSource::Wgsl(map_shader_source().into()),
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
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("map pass"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                texture(1, false),
                texture(2, false),
                texture(3, true),
                texture(4, true),
                texture(5, true),
                texture(6, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("map pass"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("map pass"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_map"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_map"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("map pass"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let blank = canvas_texture(gpu, (1, 1)).create_view(&Default::default());
        Kit {
            pipeline,
            layout,
            sampler,
            compositor: gpu.compositor.pipeline(device, FORMAT),
            blank,
        }
    }

    fn group(
        &self,
        gpu: &Gpu,
        uniform: &wgpu::Buffer,
        map: &LensMap,
        feeds: [&wgpu::TextureView; MAX_FEEDS],
    ) -> wgpu::BindGroup {
        fn tex(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::TextureView(view),
            }
        }
        gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("map pass"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                tex(1, map.place_view()),
                tex(2, map.how_view()),
                tex(3, feeds[0]),
                tex(4, feeds[1]),
                tex(5, feeds[2]),
                tex(6, feeds[3]),
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }
}

/// `f`, in error scopes: None (and a line in the log) if the card refused
/// what it made, so a card that won't take the map pass leaves a projector
/// black, never a broken window.
fn checked<T>(device: &wgpu::Device, f: impl FnOnce() -> T) -> Option<T> {
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
    let made = f();
    let inner = pollster::block_on(internal.pop());
    let outer = pollster::block_on(validation.pop());
    match inner.or(outer) {
        None => Some(made),
        Some(e) => {
            log::warn!("mapped projectors stay black: this GPU won't draw the map pass ({e})");
            None
        }
    }
}

/// What a window or a stream draws before its own composite: `frames`, by
/// `drawer` (made at the first one). With none, nothing, and no drawer made:
/// the thread draws as it did before mapping.
pub fn draw_mapped(
    drawer: &mut Option<Drawer>,
    gpu: &Gpu,
    owner: MapOwner,
    frames: &[MappedFrame],
) -> Vec<wgpu::CommandBuffer> {
    if frames.is_empty() {
        return Vec::new();
    }
    drawer
        .get_or_insert_with(|| Drawer::new(gpu))
        .draw_all(gpu, owner, frames)
}

// ----- Which maps this computer makes -------------------------------------------

/// A lens map this computer draws, so needs made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapWanted {
    pub projector: ProjectorId,
    /// The mapped projector's display.
    pub display: OutputId,
    pub size: (u32, u32),
    pub use_: MapUse,
}

/// The maps computer `me` draws (pure): one at the monitor's size for each
/// mapped projector whose display has a window here (`window`: its size, or
/// None when there is none), and one at the stream's size for each mapped
/// display among `streams` (the streams this computer sends, with their
/// sizes: on a show network, the primary's alone).
pub fn wanted(
    show: &Show,
    me: NodeId,
    streams: &[(OutputId, (u32, u32))],
    window: &dyn Fn(OutputId) -> Option<(u32, u32)>,
) -> Vec<MapWanted> {
    let mut out = Vec::new();
    for p in &show.stage3d.projectors {
        let Some(display) = p.display.filter(|_| p.mapped) else {
            continue;
        };
        let Some(o) = show.output(display).filter(|o| o.enabled) else {
            continue;
        };
        if o.is_on(me)
            && o.monitor.is_some()
            && let Some(size) = window(display)
        {
            out.push(MapWanted {
                projector: p.id,
                display,
                size,
                use_: MapUse::Window,
            });
        }
        for (id, size) in streams {
            if *id == display {
                out.push(MapWanted {
                    projector: p.id,
                    display,
                    size: *size,
                    use_: MapUse::Stream,
                });
            }
        }
    }
    out
}

#[cfg(test)]
#[path = "mapped_tests.rs"]
pub(crate) mod gpu_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_place_packs_and_unpacks_within_a_quarter_pixel_of_a_16k_canvas() {
        for at in [
            [0.0, 0.0],
            [1.0, 1.0],
            [0.5, 0.25],
            [0.123_456, 0.987_654],
            [1.0 / 3.0, 2.0 / 3.0],
        ] {
            let back = unpack_place(pack_place(at));
            for k in 0..2 {
                let off_px = (back[k] - at[k]).abs() * 16_000.0;
                assert!(off_px <= 0.25, "{at:?} came back as {back:?}");
            }
        }
        // The ends are exact, and the high byte comes first.
        assert_eq!(pack_place([1.0, 0.0]), [255, 255, 0, 0]);
        assert_eq!(pack_place([0.5, 0.0])[0], 128);
        assert_eq!(unpack_place([255, 255, 255, 255]), [1.0, 1.0]);
        // Off the picture is kept to its edge; a number that isn't one is 0.
        assert_eq!(pack_place([-3.0, 7.0]), pack_place([0.0, 1.0]));
        assert_eq!(pack_place([f32::NAN, f32::INFINITY]), [0, 0, 0, 0]);
    }

    #[test]
    fn a_depth_packs_and_unpacks_to_a_fraction_of_a_millimetre() {
        let range = (0.5, 80.0);
        for d in [0.5, 1.0, 7.25, 33.3, 79.999, 80.0] {
            let back = unpack_depth(pack_depth(d, range), range);
            assert!((back - d).abs() < 1e-4 * 80.0, "{d} came back as {back}");
        }
        // Past the ends is kept to them; nothing that is a number is far.
        assert_eq!(pack_depth(0.0, range), pack_depth(0.5, range));
        assert_eq!(pack_depth(1e9, range), [255, 255, 255, 255]);
        assert_eq!(pack_depth(f32::NAN, range), [255, 255, 255, 255]);
        assert_eq!(pack_depth(3.0, (5.0, 5.0)), [255, 255, 255, 255]);
        assert_eq!(pack_depth(3.0, (f32::NAN, 5.0)), [255, 255, 255, 255]);
        // Nearer is a smaller number, byte for byte.
        assert!(pack_depth(2.0, range) < pack_depth(2.001, range));
    }

    #[test]
    fn how_packs_its_feed_as_one_more_and_zero_as_none() {
        let h = How {
            share: 0.5,
            feed: Some(2),
            coverage: 0.25,
        };
        assert_eq!(pack_how(h), [128, 3, 64, 255]);
        let back = unpack_how(pack_how(h));
        assert_eq!(back.feed, Some(2));
        assert!((back.share - 0.5).abs() < 1.0 / 255.0);
        assert!((back.coverage - 0.25).abs() < 1.0 / 255.0);
        assert_eq!(pack_how(How::NONE)[1], 0);
        assert_eq!(unpack_how(pack_how(How::NONE)).feed, None);
        // A feed past the most a map can have shows nothing.
        assert_eq!(pack_how(How::feed(MAX_FEEDS as u8)).feed_byte(), 0);
        assert_eq!(unpack_how([255, 200, 255, 255]).feed, None);
        assert_eq!(pack_how(How::feed(0)), [255, 1, 255, 255]);
    }

    trait FeedByte {
        fn feed_byte(&self) -> u8;
    }
    impl FeedByte for [u8; 4] {
        fn feed_byte(&self) -> u8 {
            self[1]
        }
    }

    #[test]
    fn two_owners_and_two_feeds_never_share_a_slot() {
        let owners = [
            MapOwner::Window(0),
            MapOwner::Window(7),
            MapOwner::Capture(0),
            MapOwner::Capture(1),
            MapOwner::Throw(0),
        ];
        let mut seen = std::collections::HashSet::new();
        for o in owners {
            for frame in 0..MAX_FRAMES {
                for feed in 0..MAX_FEEDS {
                    assert!(
                        seen.insert(o.slot(frame, feed).index()),
                        "{o:?} {frame} {feed}"
                    );
                }
            }
        }
    }

    fn mapped_show() -> (Show, OutputId, OutputId) {
        let mut show = Show::default();
        let projector_display = show.outputs[0].id;
        show.outputs[0].monitor = Some(crate::show::DisplayRef {
            name: "Left".into(),
            index: 0,
            hardware: None,
        });
        let canvas = show.add_display();
        let p = show.add_projector();
        let pr = show
            .stage3d
            .projectors
            .iter_mut()
            .find(|q| q.id == p)
            .expect("added");
        pr.display = Some(projector_display);
        pr.mapped = true;
        (show, projector_display, canvas)
    }

    #[test]
    fn which_maps_each_computer_makes() {
        let (mut show, display, _) = mapped_show();
        let me = NodeId(1);
        let stage_left = NodeId(2);
        let monitor = |_: OutputId| Some((1920, 1200));
        let p = show.stage3d.projectors[0].id;
        // Alone: the window's, at the monitor's size.
        assert_eq!(
            wanted(&show, me, &[], &monitor),
            vec![MapWanted {
                projector: p,
                display,
                size: (1920, 1200),
                use_: MapUse::Window
            }]
        );
        // On a show network the projector is stage left's, and this
        // computer (the primary) streams it: the stream's map only.
        show.outputs[0].computer = Some(stage_left);
        let streams = [(display, (1152, 720))];
        assert_eq!(
            wanted(&show, me, &streams, &monitor),
            vec![MapWanted {
                projector: p,
                display,
                size: (1152, 720),
                use_: MapUse::Stream
            }]
        );
        // Stage left draws the window, and doesn't stream (streams are the
        // primary's): the window's.
        let on_stage_left = wanted(&show, stage_left, &[], &monitor);
        assert_eq!(on_stage_left.len(), 1);
        assert_eq!(on_stage_left[0].use_, MapUse::Window);
        // A third computer with neither: none.
        assert!(wanted(&show, NodeId(3), &[], &monitor).is_empty());
        // No window opened for it (the monitor isn't there): none.
        assert!(wanted(&show, stage_left, &[], &|_| None).is_empty());
        // Not mapped: none, whatever is streamed.
        show.stage3d.projectors[0].mapped = false;
        assert!(wanted(&show, me, &streams, &monitor).is_empty());
        assert!(wanted(&show, stage_left, &[], &monitor).is_empty());
    }
}
