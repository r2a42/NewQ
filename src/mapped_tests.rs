//! The map pass on a graphics card (lavapipe in CI: `WGPU_BACKEND=vulkan`,
//! `NEWQ_REQUIRE_GPU=1` makes having none a failure), against made-up maps
//! whose answer is known pixel for pixel: identity, a flip, a known warp,
//! two feeds; the fine-tune on top; and the shader through naga's writers.

use super::*;
use crate::gpu::{Geometry, Slot};
use crate::render::{OutputGroup, OutputLayer};
use crate::show::{ColorAdjust, Fit, LayerTransform};

pub(crate) const W: u32 = 64;
pub(crate) const H: u32 = 36;

pub(crate) fn gpu() -> Option<Gpu> {
    let gpu = Gpu::headless();
    if gpu.is_none() && std::env::var_os("NEWQ_REQUIRE_GPU").is_some() {
        panic!("NEWQ_REQUIRE_GPU is set but there is no GPU adapter");
    }
    gpu
}

/// A `W`×`H` picture with a colour of its own at every pixel: red across,
/// green down, blue `seed`.
pub(crate) fn picture(seed: u8) -> Vec<u8> {
    let mut px = Vec::new();
    for y in 0..H {
        for x in 0..W {
            px.extend_from_slice(&[(x * 4) as u8, (y * 7) as u8, seed, 255]);
        }
    }
    px
}

pub(crate) fn pixel(px: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * W + x) * 4) as usize;
    [px[i], px[i + 1], px[i + 2], px[i + 3]]
}

pub(crate) fn layer(gpu: &Gpu, px: &[u8]) -> Arc<LayerTexture> {
    let t = LayerTexture::new_rgba(gpu, W, H);
    t.upload_rgba(&gpu.queue, px);
    Arc::new(t)
}

pub(crate) fn stretched(texture: Arc<LayerTexture>) -> OutputLayer {
    OutputLayer {
        texture,
        opacity: 1.0,
        fit: Fit::Stretch,
        transform: LayerTransform::default(),
        color: ColorAdjust::default(),
        fx: Default::default(),
    }
}

pub(crate) fn plain(canvas: (u32, u32)) -> Geometry {
    Geometry {
        canvas,
        warp: crate::gpu::IDENTITY,
        pin: crate::gpu::IDENTITY,
        mesh: None,
        slice: Geometry::WHOLE,
    }
}

/// A canvas `output` showing `texture`, `W`×`H`.
pub(crate) fn feed(output: u64, texture: &Arc<LayerTexture>) -> FeedFrame {
    FeedFrame {
        output: OutputId(output),
        groups: vec![OutputGroup {
            layers: vec![stretched(texture.clone())],
            geometry: plain((W, H)),
        }],
        canvas: (W, H),
    }
}

/// A map `W`×`H` where pixel (x, y) shows feed `which(x, y)`'s pixel
/// `from(x, y)` (its middle), with `how` as given.
pub(crate) fn made_up(
    feeds: usize,
    at: impl Fn(u32, u32) -> Option<(u8, (u32, u32), f32, f32)>,
) -> CpuMap {
    let mut map = CpuMap::black((W, H));
    map.feeds = (0..feeds as u64).map(|n| OutputId(100 + n)).collect();
    for y in 0..H {
        for x in 0..W {
            if let Some((feed, (fx, fy), share, coverage)) = at(x, y) {
                let place = [(fx as f32 + 0.5) / W as f32, (fy as f32 + 0.5) / H as f32];
                let how = How {
                    share,
                    feed: Some(feed),
                    coverage,
                };
                map.set(x, y, place, how);
            }
        }
    }
    map
}

pub(crate) fn frame(gpu: &Gpu, map: &CpuMap, feeds: Vec<FeedFrame>) -> MappedFrame {
    MappedFrame {
        target: Arc::new(LayerTexture::new_target(gpu, map.size.0, map.size.1)),
        map: Arc::new(LensMap::from_cpu(gpu, map, 1)),
        feeds,
        moving: Vec::new(),
        lens: [[0.0; 4]; 4],
        shares: false,
        gamma: 2.2,
    }
}

/// Read a `W`×`H` `Rgba8Unorm` texture back.
pub(crate) fn read(gpu: &Gpu, tex: &wgpu::Texture, cmds: Vec<wgpu::CommandBuffer>) -> Vec<u8> {
    let row = W * 4; // 256: aligned
    let buf = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(row * H),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    enc.copy_texture_to_buffer(
        tex.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(H),
            },
        },
        wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
    );
    gpu.submit(cmds.into_iter().chain([enc.finish()]));
    let slice = buf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    slice.get_mapped_range().expect("mapped").to_vec()
}

pub(crate) fn window_target(gpu: &Gpu) -> (wgpu::Texture, wgpu::TextureView) {
    let t = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("window"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let v = t.create_view(&Default::default());
    (t, v)
}

/// What a window of `W`×`H` shows: the mapped frames drawn by `drawer`, then
/// the window's own composite of `groups` (as `render::output_loop` does).
pub(crate) fn window(
    gpu: &Gpu,
    drawer: &mut Option<Drawer>,
    n: u8,
    groups: &[OutputGroup],
    frames: &[MappedFrame],
) -> Vec<u8> {
    let (tex, view) = window_target(gpu);
    let pipeline = gpu.compositor.pipeline(&gpu.device, FORMAT);
    let mut cmds = draw_mapped(drawer, gpu, MapOwner::Window(n), frames);
    let draw: Vec<_> = groups.iter().map(OutputGroup::draw).collect();
    cmds.push(gpu.composite(&pipeline, Slot::Window(n), &view, &draw, None));
    read(gpu, &tex, cmds)
}

/// A frame's target as a window's one layer, placed by `geometry`.
pub(crate) fn shown(frame: &MappedFrame, geometry: Geometry) -> OutputGroup {
    OutputGroup {
        layers: vec![stretched(frame.target.clone())],
        geometry,
    }
}

/// Every pixel of `got` within one step of `want(x, y)` (the canvases are
/// sampled with a filter, at a place packed in 16 bits).
fn each_pixel(got: &[u8], what: &str, want: impl Fn(u32, u32) -> [u8; 4]) {
    for y in 0..H {
        for x in 0..W {
            let (g, w) = (pixel(got, x, y), want(x, y));
            let near = (0..3).all(|k| (i16::from(g[k]) - i16::from(w[k])).abs() <= 1);
            assert!(
                near && g[3] == 255,
                "{what}: pixel ({x},{y}) is {g:?}, not {w:?}"
            );
        }
    }
}

#[test]
fn the_map_pass_draws_made_up_maps_pixel_for_pixel() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter here; skipping the map pass test");
        return;
    };
    let a = picture(40);
    let b = picture(200);
    let (ta, tb) = (layer(&gpu, &a), layer(&gpu, &b));
    let mut drawer = None;
    let run = |drawer: &mut Option<Drawer>, map: &CpuMap, feeds: Vec<FeedFrame>, shares| {
        let mut f = frame(&gpu, map, feeds);
        f.shares = shares;
        let out = window(&gpu, drawer, 0, &[shown(&f, plain((W, H)))], &[f]);
        out
    };
    // Identity: the canvas as it is.
    let identity = made_up(1, |x, y| Some((0, (x, y), 1.0, 1.0)));
    let out = run(&mut drawer, &identity, vec![feed(100, &ta)], false);
    each_pixel(&out, "identity", |x, y| pixel(&a, x, y));
    assert!(drawer.as_ref().is_some_and(Drawer::works));
    // A flip, left to right and top to bottom.
    let flip = made_up(1, |x, y| Some((0, (W - 1 - x, H - 1 - y), 1.0, 1.0)));
    let out = run(&mut drawer, &flip, vec![feed(100, &ta)], false);
    each_pixel(&out, "flip", |x, y| pixel(&a, W - 1 - x, H - 1 - y));
    // A known warp: every pixel from somewhere else.
    let warp = |x: u32, y: u32| ((x * 3 + y) % W, (y * 5 + x / 7) % H);
    let warped = made_up(1, |x, y| Some((0, warp(x, y), 1.0, 1.0)));
    let out = run(&mut drawer, &warped, vec![feed(100, &ta)], false);
    each_pixel(&out, "warp", |x, y| {
        let (fx, fy) = warp(x, y);
        pixel(&a, fx, fy)
    });
    // Two feeds: the left half one canvas, the right half the other
    // mirrored, a band of nothing, and a column half covered.
    let two = made_up(2, |x, y| match (x, y) {
        (_, 0..=3) => None,
        (0..=31, _) => Some((0, (x, y), 1.0, if x == 10 { 0.5 } else { 1.0 })),
        _ => Some((1, (W - 1 - x, y), 1.0, 1.0)),
    });
    let out = run(
        &mut drawer,
        &two,
        vec![feed(100, &ta), feed(101, &tb)],
        false,
    );
    each_pixel(&out, "two feeds", |x, y| match (x, y) {
        (_, 0..=3) => [0, 0, 0, 255],
        (10, _) => {
            let p = pixel(&a, x, y);
            let half = |v: u8| ((f32::from(v) * (128.0 / 255.0)).round()) as u8;
            [half(p[0]), half(p[1]), half(p[2]), 255]
        }
        (0..=31, _) => pixel(&a, x, y),
        _ => pixel(&b, W - 1 - x, y),
    });
    // A feed the frame lacks (the map names two, the frame has one) is
    // black, never another canvas's picture.
    let out = run(&mut drawer, &two, vec![feed(100, &ta)], false);
    assert_eq!(pixel(&out, 50, 20), [0, 0, 0, 255]);
    // Shares: a quarter of the light, through gamma; off for a stream.
    let quarter = made_up(1, |x, y| Some((0, (x, y), 0.25, 1.0)));
    let white = layer(&gpu, &[255u8; (W * H * 4) as usize]);
    let shared = run(&mut drawer, &quarter, vec![feed(100, &white)], true);
    let want = (0.25f32.powf(1.0 / 2.2) * 255.0).round();
    let got = f32::from(pixel(&shared, 20, 20)[0]);
    assert!((got - want).abs() <= 2.0, "{got} for {want}");
    let unshared = run(&mut drawer, &quarter, vec![feed(100, &white)], false);
    assert_eq!(pixel(&unshared, 20, 20), [255, 255, 255, 255]);
}

/// The fine-tune is the compositor's mesh warp on the mapped layer, checked
/// as `gpu`'s mesh test checks it: a picture's spots land where the mesh's
/// sums say.
#[test]
fn the_fine_tune_bends_the_mapped_picture_as_the_mesh_says() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter here; skipping the fine-tune test");
        return;
    };
    // A picture whose red is how far across and green how far down.
    let mut px = Vec::new();
    for y in 0..H {
        for x in 0..W {
            let (u, v) = ((x as f32 + 0.5) / W as f32, (y as f32 + 0.5) / H as f32);
            px.extend_from_slice(&[(u * 255.0) as u8, (v * 255.0) as u8, 0, 255]);
        }
    }
    let tex = layer(&gpu, &px);
    let identity = made_up(1, |x, y| Some((0, (x, y), 1.0, 1.0)));
    let f = frame(&gpu, &identity, vec![feed(100, &tex)]);
    let mut mesh = crate::mesh::Mesh::grid(4, 3);
    *mesh.point_mut(2, 1) = [0.62, 0.45];
    *mesh.point_mut(1, 2) = [0.28, 0.92];
    *mesh.point_mut(0, 0) = [0.04, 0.03];
    assert!(mesh.is_valid());
    let geometry = Geometry {
        mesh: Some(Arc::new(mesh.clone())),
        ..plain((W, H))
    };
    let out = window(&gpu, &mut None, 1, &[shown(&f, geometry)], &[f]);
    for (u, v) in [(0.15, 0.2), (0.4, 0.3), (0.5, 0.5), (0.7, 0.45), (0.3, 0.8)] {
        let d = mesh.eval(u, v);
        let (x, y) = (
            ((d[0] * W as f32).floor() as u32).min(W - 1),
            ((d[1] * H as f32).floor() as u32).min(H - 1),
        );
        let p = pixel(&out, x, y);
        let want = [u * 255.0, v * 255.0];
        assert!(
            (f32::from(p[0]) - want[0]).abs() < 14.0 && (f32::from(p[1]) - want[1]).abs() < 14.0,
            "({u}, {v}) should be at {d:?} as {want:?}, found {p:?}"
        );
    }
    // Where the mesh's corner is pulled in, black.
    assert_eq!(pixel(&out, 0, 0)[..3], [0, 0, 0]);
}

/// Two windows showing one canvas (two projectors on one screen) draw it
/// through slots of their own: drawn in one submit, each still shows its
/// own placement of the layer, not the other's.
#[test]
fn two_windows_drawing_one_canvas_dont_share_uniforms() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter here; skipping");
        return;
    };
    let a = picture(90);
    let tex = layer(&gpu, &a);
    let identity = made_up(1, |x, y| Some((0, (x, y), 1.0, 1.0)));
    // The same layer on the same canvas, placed differently in each
    // window's frame (as two frames built a pass apart might be).
    let mut moved = feed(100, &tex);
    moved.groups[0].layers[0].transform.scale_x = -1.0;
    let left = frame(&gpu, &identity, vec![feed(100, &tex)]);
    let right = frame(&gpu, &identity, vec![moved]);
    let (t0, v0) = window_target(&gpu);
    let (t1, v1) = window_target(&gpu);
    let pipeline = gpu.compositor.pipeline(&gpu.device, FORMAT);
    let (mut d0, mut d1) = (None, None);
    // Both windows' commands recorded before either is submitted: uniforms
    // written by the second would overwrite the first's if they shared.
    let mut cmds = draw_mapped(&mut d0, &gpu, MapOwner::Window(0), &[left.clone()]);
    cmds.extend(draw_mapped(
        &mut d1,
        &gpu,
        MapOwner::Window(1),
        &[right.clone()],
    ));
    let g0 = [shown(&left, plain((W, H)))];
    let g1 = [shown(&right, plain((W, H)))];
    let draw0: Vec<_> = g0.iter().map(OutputGroup::draw).collect();
    let draw1: Vec<_> = g1.iter().map(OutputGroup::draw).collect();
    cmds.push(gpu.composite(&pipeline, Slot::Window(0), &v0, &draw0, None));
    cmds.push(gpu.composite(&pipeline, Slot::Window(1), &v1, &draw1, None));
    gpu.submit(cmds);
    let out0 = read(&gpu, &t0, Vec::new());
    let out1 = read(&gpu, &t1, Vec::new());
    each_pixel(&out0, "window 0", |x, y| pixel(&a, x, y));
    each_pixel(&out1, "window 1", |x, y| pixel(&a, W - 1 - x, y));
    // The canvas's slots are the map's, not a window's or a preview's.
    assert_ne!(MapOwner::Window(0).slot(0, 0), Slot::Window(0));
    assert_ne!(
        MapOwner::Window(0).slot(0, 0).index(),
        MapOwner::Window(1).slot(0, 0).index()
    );
}

/// No `Slot::Map` index is a `Slot::View`'s, whatever the host.
#[test]
fn map_slots_are_past_every_view_slot() {
    let views: std::collections::HashSet<usize> = (0..=255u8)
        .flat_map(|h| (0..=255u8).map(move |p| Slot::View(h, p).index()))
        .collect();
    for owner in 0..crate::gpu::MAP_OWNERS as u8 {
        for place in 0..crate::gpu::MAP_PLACES as u8 {
            assert!(!views.contains(&Slot::Map(owner, place).index()));
        }
    }
}

/// The map pass goes through naga's HLSL and MSL writers, as wgpu does on
/// Windows and the Mac: a first check that those systems take it.
#[test]
fn the_map_pass_shader_is_written_for_direct3d_and_metal() {
    let source = map_shader_source();
    let module = naga::front::wgsl::parse_str(&source).expect("the map pass's shader reads");
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .expect("the map pass's shader is valid");
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
            .unwrap_or_else(|e| panic!("HLSL {model:?}: {e}"));
    }
    naga::back::msl::write_string(&module, &info, &Default::default(), &Default::default())
        .expect("MSL");
}

/// The shader reads `how.g` as `unpack_how` does: a column with no feed is
/// black, its neighbours (at places all over the canvas) are not.
#[test]
fn a_pixel_the_map_gives_no_feed_is_black_among_ones_it_does() {
    let Some(gpu) = gpu() else {
        return;
    };
    // Pixel (x, y)'s place packs x * 1031 + y * 7 across and its reverse
    // down; which feed by column.
    let white = layer(&gpu, &[255u8; (W * H * 4) as usize]);
    let mut map = CpuMap::black((W, H));
    map.feeds = vec![OutputId(1)];
    for y in 0..H {
        for x in 0..W {
            let u = ((x * 1031 + y * 7) % 65536) as f32 / 65535.0;
            map.set(x, y, [u, 1.0 - u], How::feed(0));
            if x % 9 == 0 {
                map.how[(y * W + x) as usize] = pack_how(How::NONE);
            }
        }
    }
    let f = frame(&gpu, &map, vec![feed(1, &white)]);
    let out = window(&gpu, &mut None, 2, &[shown(&f, plain((W, H)))], &[f]);
    for y in 0..H {
        for x in 0..W {
            let p = pixel(&out, x, y);
            let want = if x % 9 == 0 { 0 } else { 255 };
            assert_eq!(p[0], want, "({x},{y})");
        }
    }
    let _ = WGSL_UNPACK;
}
