use super::*;
use crate::fx::VideoFx;
use crate::gpu::Geometry;
use crate::render::OutputLayer;
use crate::show::{ColorAdjust, Fit, LayerTransform, Show};
use crate::stage3d::{Feed, StageSource, WHOLE};

#[test]
fn a_feed_is_drawn_small_and_never_bigger_than_its_picture() {
    assert_eq!(feed_size((1920, 1080), 1.0), (1024, 576));
    assert_eq!(feed_size((1080, 1920), 1.0), (576, 1024));
    assert_eq!(feed_size((1920, 1080), 0.5), (512, 288));
    // Never under the least, nor over the picture itself.
    assert_eq!(feed_size((1920, 1080), 0.0), (256, 144));
    assert_eq!(feed_size((640, 360), 1.0), (640, 360));
    assert_eq!(feed_size((0, 0), f32::NAN), (1, 1));
    let (w, h) = feed_size((7680, 1080), 1.0);
    assert_eq!(w, 1024);
    assert!(h >= 1);
}

fn gpu() -> Option<Gpu> {
    let gpu = Gpu::headless();
    if gpu.is_none() && std::env::var_os("NEWQ_REQUIRE_GPU").is_some() {
        panic!("NEWQ_REQUIRE_GPU is set but there is no GPU adapter");
    }
    gpu
}

const PICTURE: (u32, u32) = (64, 36);

/// A layer of `tex` stretched over a picture `canvas` big.
fn group(tex: &Arc<LayerTexture>, canvas: (u32, u32)) -> OutputGroup {
    OutputGroup {
        layers: vec![OutputLayer {
            texture: tex.clone(),
            opacity: 1.0,
            fit: Fit::Stretch,
            transform: LayerTransform::default(),
            color: ColorAdjust::default(),
            fx: VideoFx::default(),
        }],
        geometry: Geometry {
            canvas,
            warp: crate::gpu::IDENTITY,
            pin: crate::gpu::IDENTITY,
            mesh: None,
            slice: Geometry::WHOLE,
        },
    }
}

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];

/// A picture of four colours, one in each quarter: red top left, green top
/// right, blue bottom left, white bottom right.
fn quarters(gpu: &Gpu) -> Arc<LayerTexture> {
    let t = LayerTexture::new_rgba(gpu, 2, 2);
    t.upload_rgba(&gpu.queue, &[RED, GREEN, BLUE, WHITE].concat());
    Arc::new(t)
}

/// What a display window would show with `groups`: composited into a
/// picture `size` big and read back (RGBA rows).
fn shown(gpu: &Gpu, groups: &[OutputGroup], size: (u32, u32)) -> Vec<u8> {
    let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("shown"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());
    let pipeline = gpu.compositor.pipeline(&gpu.device, FORMAT);
    let draw: Vec<_> = groups.iter().map(OutputGroup::draw).collect();
    let cmd = gpu.composite(&pipeline, Slot::Window(7), &view, &draw, None);
    let row = size.0 * 4;
    let padded =
        row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buf = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(padded * size.1),
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
                bytes_per_row: Some(padded),
                rows_per_image: Some(size.1),
            },
        },
        wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
    );
    gpu.submit([cmd, enc.finish()]);
    let slice = buf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let data = slice.get_mapped_range().expect("mapped").to_vec();
    let mut out = Vec::with_capacity((row * size.1) as usize);
    for y in 0..size.1 as usize {
        let from = y * padded as usize;
        out.extend_from_slice(&data[from..from + row as usize]);
    }
    out
}

fn pixel(img: &[u8], size: (u32, u32), fx: f32, fy: f32) -> [u8; 4] {
    let x = ((size.0 as f32 * fx) as usize).min(size.0 as usize - 1);
    let y = ((size.1 as f32 * fy) as usize).min(size.1 as usize - 1);
    let i = (y * size.0 as usize + x) * 4;
    [img[i], img[i + 1], img[i + 2], img[i + 3]]
}

fn near(a: [u8; 4], b: [u8; 4]) -> bool {
    (0..3).all(|i| a[i].abs_diff(b[i]) <= 40)
}

/// A show with one 4 x 2.25 m screen at the origin facing the audience,
/// showing display `output`, and a camera straight in front of it.
fn screen_show() -> (Show, OutputId, Camera) {
    let mut show = Show::default();
    let output = show.outputs[0].id;
    let s = show.add_surface(None);
    show.stage3d.surface_mut(s).unwrap().feed = Feed::Display {
        output,
        region: WHOLE,
    };
    let camera = Camera {
        eye: [0.0, 0.0, 4.0],
        target: [0.0, 0.0, 0.0],
        ..Camera::default()
    };
    (show, output, camera)
}

/// A display's picture on a screen in the stage, seen from in front, then
/// shown on a display that shows the stage: each colour stays in its corner
/// (nothing upside down or mirrored on the way).
#[test]
fn a_displays_picture_is_on_its_screen_the_right_way_round() {
    let Some(gpu) = gpu() else { return };
    let (show, output, camera) = screen_show();
    let scene = Arc::new(show.stage3d.scene(&show, None));
    assert_eq!(crate::stage3d::outputs_in(&scene), vec![output]);
    let picture = quarters(&gpu);
    let size = (128, 72);
    let target = Arc::new(LayerTexture::new_target(&gpu, size.0, size.1));
    assert!(!target.is_filled());
    let job = StageJob {
        feeds: vec![FeedJob {
            output,
            groups: vec![group(&picture, PICTURE)],
            size: PICTURE,
        }],
        views: vec![ViewJob {
            key: 1,
            display: true,
            target: target.clone(),
            scene: scene.clone(),
            camera,
            options: Options::default(),
            lines: Default::default(),
        }],
        serial: 1,
        ..StageJob::default()
    };
    let mut drawer = StageDrawer::new(&gpu);
    let stats = drawer.draw(&gpu, &job, &mut || {});
    assert_eq!(stats.len(), 1);
    assert_eq!(stats[0].1.missing_pictures, 0, "{:?}", stats[0].1);
    assert!(target.is_filled());
    // The stage's picture as a display that shows it draws it: a layer.
    let img = shown(&gpu, &[group(&target, size)], size);
    // The screen covers the middle 60% of the view: its quarters' middles.
    assert!(
        near(pixel(&img, size, 0.36, 0.36), RED),
        "{:?}",
        pixel(&img, size, 0.36, 0.36)
    );
    assert!(
        near(pixel(&img, size, 0.64, 0.36), GREEN),
        "{:?}",
        pixel(&img, size, 0.64, 0.36)
    );
    assert!(
        near(pixel(&img, size, 0.36, 0.64), BLUE),
        "{:?}",
        pixel(&img, size, 0.36, 0.64)
    );
    assert!(
        near(pixel(&img, size, 0.64, 0.64), WHITE),
        "{:?}",
        pixel(&img, size, 0.64, 0.64)
    );
    // Round it, the room: dark, not the picture.
    let corner = pixel(&img, size, 0.02, 0.02);
    assert!(
        corner[0] < 60 && corner[1] < 60 && corner[2] < 60,
        "{corner:?}"
    );
}

/// Without its display's picture a screen is a dark panel (counted as
/// missing), and a display that shows the stage is a picture all the same.
#[test]
fn a_display_that_shows_the_stage_has_a_picture_without_any_feed() {
    let Some(gpu) = gpu() else { return };
    let (mut show, output, camera) = screen_show();
    show.outputs[0].source = Some(StageSource::default());
    // It shows the stage, so its own screen is left unfed (no loop).
    let scene = Arc::new(show.stage3d.scene(&show, None));
    assert!(crate::stage3d::outputs_in(&scene).is_empty());
    let size = (96, 54);
    let target = Arc::new(LayerTexture::new_target(&gpu, size.0, size.1));
    let job = StageJob {
        feeds: Vec::new(),
        views: vec![ViewJob {
            key: 2,
            display: true,
            target: target.clone(),
            scene,
            camera,
            lines: Default::default(),
            options: Options {
                background: [0.0, 0.0, 0.5],
                ..Options::default()
            },
        }],
        serial: 1,
        ..StageJob::default()
    };
    let mut drawer = StageDrawer::new(&gpu);
    drawer.draw(&gpu, &job, &mut || {});
    let img = shown(&gpu, &[group(&target, size)], size);
    // The background is the colour asked for; the screen in the middle is grey.
    let edge = pixel(&img, size, 0.02, 0.5);
    assert!(edge[2] > 100 && edge[0] < 30, "{edge:?}");
    let middle = pixel(&img, size, 0.5, 0.5);
    assert!(middle[2] < 100 && middle[0] > 10, "{middle:?}");
    let _ = output;
}

/// Through the thread: a job is drawn and said done; a job not yet taken is
/// merged with the next, so no view asked for is lost.
#[test]
fn the_thread_draws_what_it_is_given_and_says_so() {
    let Some(gpu) = gpu() else { return };
    let (show, output, camera) = screen_show();
    let scene = Arc::new(show.stage3d.scene(&show, None));
    let picture = quarters(&gpu);
    let a = Arc::new(LayerTexture::new_target(&gpu, 32, 18));
    let b = Arc::new(LayerTexture::new_target(&gpu, 48, 27));
    let view = |t: &Arc<LayerTexture>, key| ViewJob {
        key,
        display: key == 2,
        target: t.clone(),
        scene: scene.clone(),
        camera,
        options: Options::default(),
        lines: Default::default(),
    };
    let first = StageJob {
        feeds: vec![FeedJob {
            output,
            groups: vec![group(&picture, PICTURE)],
            size: PICTURE,
        }],
        views: vec![view(&a, 1)],
        serial: 1,
        ..StageJob::default()
    };
    let second = StageJob {
        feeds: Vec::new(),
        views: vec![view(&b, 2)],
        serial: 2,
        ..StageJob::default()
    };
    let both = first.clone().merged(second.clone());
    assert_eq!(both.serial, 2);
    assert_eq!(both.views.len(), 2);
    assert_eq!(both.feeds.len(), 1);
    // A display's view is drawn before a preview's, whichever came first.
    let order = |j: &StageJob| j.views.iter().map(|v| v.key).collect::<Vec<_>>();
    assert_eq!(order(&both), [2, 1]);
    assert_eq!(order(&second.clone().merged(first.clone())), [2, 1]);
    // Said to have landed once the display's is drawn, before the preview's.
    let (pa, pb) = (
        Arc::new(LayerTexture::new_target(&gpu, 32, 18)),
        Arc::new(LayerTexture::new_target(&gpu, 48, 27)),
    );
    let count = Counters::for_test();
    let mut drawer = StageDrawer::counting(&gpu, count);
    let mut at_displays = None;
    // (In the order asked; merging puts the display's first.)
    let job = StageJob {
        feeds: first.feeds.clone(),
        views: vec![view(&pa, 1), view(&pb, 2)],
        serial: 2,
        ..StageJob::default()
    }
    .merged(StageJob::default());
    drawer.draw(&gpu, &job, &mut || {
        at_displays = Some((pa.is_filled(), pb.is_filled()))
    });
    assert_eq!(at_displays, Some((false, true)));
    assert!(pa.is_filled());
    // Each view keeps its own render targets.
    assert_eq!(drawer.kept().2, 2);
    // Counted: one job, its feed, a display's view and a preview's.
    let t = count.0.totals();
    assert_eq!(
        (t.jobs, t.feeds, t.displays, t.views),
        (1, 1, 1, 1),
        "{t:?}"
    );
    assert_eq!((t.missing_pictures, t.light_left_out), (0, 0), "{t:?}");
    let count = Counters::for_test();
    let thread = StageThread::spawn(gpu.clone(), count).unwrap();
    thread.submit(first);
    thread.submit(second);
    let end = Instant::now() + Duration::from_secs(20);
    while thread.done() < 2 && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(thread.done(), 2);
    assert!(a.is_filled() && b.is_filled());
    assert!(thread.stats(2).is_some());
    let _ = thread.quit().join();
    // The thread counts what it drew: two jobs, or one if merged, with the
    // same two views either way.
    let t = count.0.totals();
    assert!((1..=2).contains(&t.jobs), "{t:?}");
    assert_eq!((t.displays, t.views), (1, 1), "{t:?}");
}

/// Each view drawn is counted, with its processor time and the pictures its
/// screens lacked; the program's totals count it too.
#[test]
fn every_view_drawn_is_counted() {
    let Some(gpu) = gpu() else { return };
    let (show, output, camera) = screen_show();
    let scene = Arc::new(show.stage3d.scene(&show, None));
    let target = Arc::new(LayerTexture::new_target(&gpu, 64, 36));
    let job = |feeds: Vec<FeedJob>| StageJob {
        feeds,
        views: vec![ViewJob {
            key: 9,
            display: true,
            target: target.clone(),
            scene: scene.clone(),
            camera,
            options: Options::default(),
            lines: Default::default(),
        }],
        serial: 1,
        ..StageJob::default()
    };
    let picture = quarters(&gpu);
    let fed = job(vec![FeedJob {
        output,
        groups: vec![group(&picture, PICTURE)],
        size: PICTURE,
    }]);
    let count = Counters::for_test();
    let everyone = totals();
    let mut drawer = StageDrawer::counting(&gpu, count);
    drawer.draw(&gpu, &fed, &mut || {});
    let once = count.0.totals();
    drawer.draw(&gpu, &fed, &mut || {});
    let t = count.0.totals();
    let twice = t.since(&once);
    assert_eq!(
        (t.jobs, t.displays, t.views, t.feeds),
        (2, 2, 0, 2),
        "{t:?}"
    );
    assert_eq!((twice.jobs, twice.displays), (1, 1));
    assert!(t.cpu_us_displays > 0 && t.cpu_us_worst > 0, "{t:?}");
    assert!(t.cpu_us_worst <= t.cpu_us_displays);
    assert_eq!(t.frames_by_ms.iter().sum::<u64>(), 2);
    assert!(t.worst_band_ms().is_some());
    assert_eq!(t.display_missing_pictures, 0);
    // Without its display's picture the screen is counted as missing (a
    // drawer of its own: this one keeps the feed for a while).
    StageDrawer::counting(&gpu, count).draw(&gpu, &job(Vec::new()), &mut || {});
    let t = count.0.totals();
    assert_eq!((t.missing_pictures, t.display_missing_pictures), (1, 1));
    // The program's totals saw all three (and maybe others' beside them).
    let all = totals().since(&everyone);
    assert!(all.displays >= 3 && all.jobs >= 3, "{all:?}");
}

/// A card that won't draw the light: the lit view is drawn without it, and
/// that is counted.
#[test]
fn a_view_drawn_without_its_light_is_counted() {
    let Some(gpu) = gpu() else { return };
    let (mut show, _output, camera) = screen_show();
    let s = show.stage3d.surfaces[0].clone();
    let id = show.add_projector();
    let p = show.projector_mut(id).unwrap();
    p.placement = p.fit_to(&s, 16.0 / 9.0).unwrap();
    let p = p.clone();
    let mut scene = show.stage3d.scene(&show, None);
    scene.throws = vec![crate::stage3d::light::throw_of(
        &p,
        16.0 / 9.0,
        (1920, 1080),
        0,
        true,
    )];
    let scene = Arc::new(scene);
    let picture = quarters(&gpu);
    let target = Arc::new(LayerTexture::new_target(&gpu, 64, 36));
    let job = StageJob {
        throws: vec![ThrowJob {
            layer: 0,
            groups: vec![group(&picture, PICTURE)],
            blend: EdgeBlend::default(),
        }],
        throw_size: (64, 36),
        views: vec![ViewJob {
            key: 6,
            display: false,
            target: target.clone(),
            scene,
            camera,
            lines: Default::default(),
            options: Options {
                light: LightView::Picture,
                ..Options::default()
            },
        }],
        serial: 1,
        ..StageJob::default()
    };
    let count = Counters::for_test();
    let mut drawer = StageDrawer::counting(&gpu, count);
    drawer.refuse_light();
    let stats = drawer.draw(&gpu, &job, &mut || {});
    assert!(stats[0].1.light_left_out);
    assert!(target.is_filled(), "the frame lands all the same");
    let t = count.0.totals();
    assert_eq!((t.views, t.light_left_out, t.throws), (1, 1, 1), "{t:?}");
    // A card that draws it: nothing left out.
    let count = Counters::for_test();
    let mut drawer = StageDrawer::counting(&gpu, count);
    drawer.draw(&gpu, &job, &mut || {});
    let t = count.0.totals();
    assert_eq!((t.views, t.light_left_out, t.throws), (1, 0, 1), "{t:?}");
}

/// One layer of an array texture's pixels (RGBA rows).
fn read_layer(gpu: &Gpu, tex: &wgpu::Texture, layer: u32, size: (u32, u32)) -> Vec<u8> {
    let row = size.0 * 4;
    let padded =
        row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buf = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("layer readback"),
        size: u64::from(padded * size.1),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: 0,
                y: 0,
                z: layer,
            },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(size.1),
            },
        },
        wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
    );
    gpu.submit([enc.finish()]);
    let slice = buf.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let data = slice.get_mapped_range().expect("mapped").to_vec();
    let mut out = Vec::with_capacity((row * size.1) as usize);
    for y in 0..size.1 as usize {
        let from = y * padded as usize;
        out.extend_from_slice(&data[from..from + row as usize]);
    }
    out
}

#[test]
fn a_throw_is_drawn_small_and_sixteen_by_nine() {
    assert_eq!(throw_size(1.0), (960, 540));
    assert_eq!(throw_size(0.5), (480, 270));
    assert_eq!(throw_size(0.0), (FEED_SIDE_LEAST, 144));
    assert_eq!(throw_size(f32::NAN), (960, 540));
}

/// A projector's picture is drawn as its display sends it, its edge blend
/// fading its side, into its layer; a view with its light on throws it onto
/// the screen the right way round.
#[test]
fn a_projectors_picture_is_drawn_with_its_blend_and_thrown_the_right_way_round() {
    let Some(gpu) = gpu() else { return };
    let (mut show, _output, camera) = screen_show();
    let s = show.stage3d.surfaces[0].clone();
    let id = show.add_projector();
    let p = show.projector_mut(id).unwrap();
    p.placement = p.fit_to(&s, 16.0 / 9.0).unwrap();
    let p = p.clone();
    let throw = crate::stage3d::light::throw_of(&p, 16.0 / 9.0, (1920, 1080), 0, true);
    let mut scene = show.stage3d.scene(&show, None);
    scene.throws = vec![throw];
    let scene = Arc::new(scene);
    let picture = quarters(&gpu);
    let blend = EdgeBlend {
        right: 0.3,
        ..EdgeBlend::default()
    };
    let size = (128, 72);
    let target = Arc::new(LayerTexture::new_target(&gpu, size.0, size.1));
    let white = crate::stage3d::light::luminance(
        crate::stage3d::light::illuminance(
            &p,
            16.0 / 9.0,
            [0.0; 3],
            None,
            crate::stage3d::light::Facing::Front,
        ),
        1.0,
    );
    let job = |blend: EdgeBlend| StageJob {
        throws: vec![ThrowJob {
            layer: 0,
            groups: vec![group(&picture, PICTURE)],
            blend,
        }],
        throw_size: (64, 36),
        views: vec![ViewJob {
            key: 5,
            display: false,
            target: target.clone(),
            scene: scene.clone(),
            camera,
            lines: Default::default(),
            options: Options {
                light: LightView::Picture,
                white_nits: white,
                headlight: 0.0,
                ambient: 0.0,
                ..Options::default()
            },
        }],
        serial: 1,
        ..StageJob::default()
    };
    let mut drawer = StageDrawer::new(&gpu);
    let stats = drawer.draw(&gpu, &job(blend), &mut || {});
    assert_eq!(stats[0].1.throws, 1);
    let (tex, tsize) = drawer.throws().unwrap();
    assert_eq!(tsize, (64, 36));
    let layer = read_layer(&gpu, tex, 0, tsize);
    // Its picture, red at the top left, green at the top right, fading out
    // over the right 30% (the blend).
    assert!(
        near(pixel(&layer, tsize, 0.2, 0.2), RED),
        "{:?}",
        pixel(&layer, tsize, 0.2, 0.2)
    );
    let green = pixel(&layer, tsize, 0.75, 0.2);
    assert!(green[1] > 150 && green[0] < 60, "{green:?}");
    let edge = pixel(&layer, tsize, 0.97, 0.2);
    assert!(edge[1] < 120, "the blend fades the right edge: {edge:?}");
    // On the screen, as the window would show it, the right way round.
    let img = shown(&gpu, &[group(&target, size)], size);
    let at = |x: f32, y: f32| {
        let s = camera.project([x, y, 0.0], size).unwrap();
        pixel(&img, size, s[0] / size.0 as f32, s[1] / size.1 as f32)
    };
    assert!(near(at(-1.0, 0.55), RED), "{:?}", at(-1.0, 0.55));
    assert!(near(at(1.0, 0.55), GREEN), "{:?}", at(1.0, 0.55));
    assert!(near(at(-1.0, -0.55), BLUE), "{:?}", at(-1.0, -0.55));
    assert!(at(1.9, 0.55)[1] < 140, "faded: {:?}", at(1.9, 0.55));
    // Without the blend, its edge is whole.
    drawer.draw(&gpu, &job(EdgeBlend::default()), &mut || {});
    let (tex, tsize) = drawer.throws().unwrap();
    let layer = read_layer(&gpu, tex, 0, tsize);
    assert!(near(pixel(&layer, tsize, 0.97, 0.2), GREEN));
}

#[test]
fn merging_keeps_the_newer_pictures_of_the_projectors_and_the_older_others() {
    let t = |layer| ThrowJob {
        layer,
        groups: Vec::new(),
        blend: EdgeBlend {
            left: layer as f32 / 10.0,
            ..EdgeBlend::default()
        },
    };
    let old = StageJob {
        throws: vec![t(0), t(1)],
        throw_size: (64, 36),
        serial: 1,
        ..StageJob::default()
    };
    let new = StageJob {
        throws: vec![ThrowJob {
            blend: EdgeBlend::default(),
            ..t(1)
        }],
        throw_size: (64, 36),
        serial: 2,
        ..StageJob::default()
    };
    let both = old.clone().merged(new.clone());
    assert_eq!(both.throws.len(), 2);
    assert!(both.throws.iter().any(|j| j.layer == 1 && j.blend.is_off()));
    // None newer: the older stay.
    let kept = old.clone().merged(StageJob::default());
    assert_eq!(kept.throws.len(), 2);
    assert_eq!(kept.throw_size, (64, 36));
    // Another size: only the newer (the older were for the other size).
    let smaller = StageJob {
        throw_size: (32, 18),
        ..new
    };
    assert_eq!(old.merged(smaller).throws.len(), 1);
}

/// Two projectors overlapping by a quarter of their width, each fading its
/// side of the overlap with its edge blend: thrown on a wall, the light is as
/// even across the overlap as either alone (in light, not in pixel values).
/// Without the blends the overlap is twice as bright.
#[test]
fn a_blended_pairs_overlap_is_as_bright_as_either_alone() {
    use crate::render3d::{Paint, Part};
    use crate::stage3d::scene::Catch;
    let Some(gpu) = gpu() else { return };
    let wall = Part {
        mesh: Arc::new(crate::stage3d::geometry::quad(12.0, 6.0)),
        mesh_key: 77,
        transform: crate::stage3d::scene::IDENTITY,
        paint: Paint::Colour([0.2, 0.2, 0.2, 1.0]),
        lit: false,
        catch: Catch {
            gain: 1.0,
            front_only: true,
        },
    };
    // Each picture 4 m wide from 6 m; their middles 3 m apart: 1 m of overlap.
    let projector = |x: f32| {
        let mut p = crate::stage3d::Projector::new(crate::stage3d::ProjectorId(1), "P".into());
        p.placement.position = [x, 0.0, 6.0];
        p.lens.throw = 1.5;
        p.lumens = 5000.0;
        p
    };
    let (left, right) = (projector(-1.5), projector(1.5));
    let throw = |p: &crate::stage3d::Projector, layer| {
        crate::stage3d::light::throw_of(p, 16.0 / 9.0, (1920, 1080), layer, true)
    };
    let scene = Arc::new(Scene {
        parts: vec![wall],
        throws: vec![throw(&left, 0), throw(&right, 1)],
    });
    let one = crate::stage3d::light::luminance(
        crate::stage3d::light::illuminance(
            &left,
            16.0 / 9.0,
            [-1.5, 0.0, 0.0],
            None,
            crate::stage3d::light::Facing::Front,
        ),
        1.0,
    );
    let white = {
        let t = LayerTexture::new_rgba(&gpu, 2, 2);
        t.upload_rgba(&gpu.queue, &[WHITE; 4].concat());
        Arc::new(t)
    };
    let camera = Camera {
        eye: [0.0, 0.0, 10.0],
        target: [0.0, 0.0, 0.0],
        ..Camera::default()
    };
    let size = (480, 240);
    let target = Arc::new(LayerTexture::new_target(&gpu, size.0, size.1));
    let job = |blended: bool| StageJob {
        throws: [(0, 0.0, 0.25), (1, 0.25, 0.0)]
            .map(|(layer, l, r)| ThrowJob {
                layer,
                groups: vec![group(&white, PICTURE)],
                blend: if blended {
                    EdgeBlend {
                        left: l,
                        right: r,
                        ..EdgeBlend::default()
                    }
                } else {
                    EdgeBlend::default()
                },
            })
            .to_vec(),
        throw_size: throw_size(0.5),
        views: vec![ViewJob {
            key: 6,
            display: false,
            target: target.clone(),
            scene: scene.clone(),
            camera,
            lines: Default::default(),
            options: Options {
                light: LightView::Picture,
                // One projector alone is half of white, in light.
                white_nits: one * 2.0,
                headlight: 0.0,
                ambient: 0.0,
                ..Options::default()
            },
        }],
        serial: 1,
        ..StageJob::default()
    };
    let mut drawer = StageDrawer::new(&gpu);
    let light_along = |drawer: &mut StageDrawer, blended: bool| {
        drawer.draw(&gpu, &job(blended), &mut || {});
        let img = shown(&gpu, &[group(&target, size)], size);
        (-12..=12)
            .map(|i| {
                let x = i as f32 * 0.25;
                let s = camera.project([x, 0.0, 0.0], size).unwrap();
                let v = pixel(&img, size, s[0] / size.0 as f32, s[1] / size.1 as f32);
                (x, (f32::from(v[0]) / 255.0).powf(2.2))
            })
            .collect::<Vec<_>>()
    };
    for (x, light) in light_along(&mut drawer, true) {
        assert!(
            (light - 0.5).abs() < 0.03,
            "blended: at {x} m the light is {light}, not 0.5"
        );
    }
    let plain = light_along(&mut drawer, false);
    let at = |x: f32| {
        plain
            .iter()
            .find(|(px, _)| (*px - x).abs() < 1e-3)
            .unwrap()
            .1
    };
    assert!((at(-2.5) - 0.5).abs() < 0.03, "{}", at(-2.5));
    assert!(
        (at(0.0) - 1.0).abs() < 0.04,
        "unblended overlap: {}",
        at(0.0)
    );
}
