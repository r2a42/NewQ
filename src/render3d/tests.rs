//! The renderer drawn for real, off screen, and its pixels read back. Skipped
//! where there is no GPU adapter (`NEWQ_REQUIRE_GPU` makes that a failure);
//! under Xvfb `WGPU_BACKEND=vulkan` gives mesa's lavapipe.

use super::maths::{self, Mat4};
use super::*;
use std::sync::Arc;

fn test_gpu() -> Option<Gpu> {
    let gpu = Gpu::headless();
    if gpu.is_none() && std::env::var_os("NEWQ_REQUIRE_GPU").is_some() {
        panic!("NEWQ_REQUIRE_GPU is set but there is no GPU adapter");
    }
    if gpu.is_none() {
        eprintln!("no GPU adapter here; skipping the 3D renderer test");
    }
    gpu
}

/// A texture to draw into and read from.
fn target(gpu: &Gpu, size: (u32, u32), format: wgpu::TextureFormat) -> wgpu::Texture {
    gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("3d test target"),
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
    })
}

/// The texture's pixels, RGBA, rows packed.
fn read(gpu: &Gpu, tex: &wgpu::Texture) -> Vec<u8> {
    let (w, h) = (tex.width(), tex.height());
    let row = (w * 4).next_multiple_of(256);
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("3d test readback"),
        size: u64::from(row * h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    encoder.copy_texture_to_buffer(
        tex.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(h),
            },
        },
        tex.size(),
    );
    gpu.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let data = slice.get_mapped_range().expect("mapped");
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        let start = (y * row) as usize;
        out.extend_from_slice(&data[start..start + (w * 4) as usize]);
    }
    out
}

fn px(pixels: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * w + x) * 4) as usize;
    [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
}

/// Draw and read back, into a fresh target.
#[allow(clippy::too_many_arguments)]
fn draw<'a>(
    gpu: &Gpu,
    r: &mut Renderer,
    scene: &Scene,
    camera: &Camera,
    outputs: &dyn Fn(OutputId) -> Option<&'a wgpu::TextureView>,
    options: &Options,
    size: (u32, u32),
    format: wgpu::TextureFormat,
) -> (Vec<u8>, FrameStats) {
    draw_for(0, gpu, r, scene, camera, outputs, options, size, format)
}

/// `draw` as a view of its own (`Target::owner`).
#[allow(clippy::too_many_arguments)]
fn draw_for<'a>(
    owner: u64,
    gpu: &Gpu,
    r: &mut Renderer,
    scene: &Scene,
    camera: &Camera,
    outputs: &dyn Fn(OutputId) -> Option<&'a wgpu::TextureView>,
    options: &Options,
    size: (u32, u32),
    format: wgpu::TextureFormat,
) -> (Vec<u8>, FrameStats) {
    let tex = target(gpu, size, format);
    let view = tex.create_view(&Default::default());
    let stats = r.render(
        gpu,
        &Target {
            view: &view,
            format,
            size,
            owner,
        },
        scene,
        camera,
        outputs,
        options,
    );
    (read(gpu, &tex), stats)
}

fn no_outputs(_: OutputId) -> Option<&'static wgpu::TextureView> {
    None
}

/// A 1 m square in the XY plane facing +Z, uv (0, 0) at its top-left.
fn square() -> Arc<Mesh> {
    Arc::new(Mesh {
        positions: vec![
            [-0.5, 0.5, 0.0],
            [0.5, 0.5, 0.0],
            [0.5, -0.5, 0.0],
            [-0.5, -0.5, 0.0],
        ],
        normals: vec![[0.0, 0.0, 1.0]; 4],
        uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        indices: vec![0, 1, 2, 0, 2, 3],
    })
}

/// A 1 m cube about the origin, flat-shaded (each face its own vertices).
fn cube() -> Arc<Mesh> {
    let mut m = Mesh {
        positions: vec![],
        normals: vec![],
        uvs: vec![],
        indices: vec![],
    };
    let faces: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([0.0, 0.0, -1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
        ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
        ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
    ];
    for (n, u, v) in faces {
        let base = m.positions.len() as u32;
        for (a, b) in [(-1.0, 1.0), (1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
            let p = maths::add(
                maths::scale(n, 0.5),
                maths::add(maths::scale(u, a * 0.5), maths::scale(v, b * 0.5)),
            );
            m.positions.push(p);
            m.normals.push(n);
            m.uvs.push([(a + 1.0) / 2.0, (1.0 - b) / 2.0]);
        }
        m.indices
            .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    Arc::new(m)
}

fn moved(to: [f32; 3], s: f32) -> Mat4 {
    let mut m = maths::IDENTITY;
    m[0][0] = s;
    m[1][1] = s;
    m[2][2] = s;
    m[3] = [to[0], to[1], to[2], 1.0];
    m
}

fn turned_y(deg: f32, to: [f32; 3]) -> Mat4 {
    let (s, c) = deg.to_radians().sin_cos();
    let mut m = maths::IDENTITY;
    m[0] = [c, 0.0, -s, 0.0];
    m[2] = [s, 0.0, c, 0.0];
    m[3] = [to[0], to[1], to[2], 1.0];
    m
}

fn part(mesh: Arc<Mesh>, key: u64, transform: Mat4, paint: Paint, lit: bool) -> Part {
    Part {
        mesh,
        mesh_key: key,
        transform,
        paint,
        lit,
    }
}

fn looking_down_z(distance: f32) -> Camera {
    Camera {
        eye: [0.0, 0.0, distance],
        target: [0.0, 0.0, 0.0],
        up: [0.0, 1.0, 0.0],
        fov_y_deg: 60.0,
        near: 0.1,
        far: 100.0,
        ..Camera::default()
    }
}

const RGBA: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

fn near(a: [u8; 4], b: [u8; 3], tol: i32) -> bool {
    (0..3).all(|i| (i32::from(a[i]) - i32::from(b[i])).abs() <= tol)
}

#[test]
fn a_governor_scales_a_preview_and_drops_its_multisampling() {
    let full = Quality::default();
    assert_eq!(full.scaled_by(1.0), full);
    let half = full.scaled_by(0.5);
    assert_eq!((half.msaa, half.scale), (false, 0.5));
    assert_eq!(half.drawn_size((1920, 1080)), (960, 540));
    let quarter = Quality {
        msaa: true,
        scale: 0.5,
    }
    .scaled_by(0.25);
    assert_eq!(quarter.drawn_size((1920, 1080)), (240, 135));
    // Nonsense from the governor leaves the quality as it was.
    assert_eq!(full.scaled_by(f32::NAN), full);
    // Never down to nothing.
    assert_eq!(full.scaled_by(0.0).drawn_size((100, 10)), (10, 1));
}

#[test]
fn an_unlit_red_square_covers_the_middle_at_every_quality() {
    let Some(gpu) = test_gpu() else { return };
    let mut r = Renderer::new(&gpu);
    let scene = Scene {
        throws: Vec::new(),
        parts: vec![part(
            square(),
            1,
            maths::IDENTITY,
            Paint::Colour([1.0, 0.0, 0.0, 1.0]),
            false,
        )],
    };
    let options = Options {
        background: [0.0, 0.0, 0.0],
        ..Options::default()
    };
    let size = (64, 48);
    for quality in [
        Quality::default(),
        Quality::LOW,
        Quality {
            msaa: true,
            scale: 0.5,
        },
    ] {
        let options = Options { quality, ..options };
        let (pixels, stats) = draw(
            &gpu,
            &mut r,
            &scene,
            &looking_down_z(2.0),
            &no_outputs,
            &options,
            size,
            RGBA,
        );
        assert!(
            near(px(&pixels, 64, 32, 24), [255, 0, 0], 2),
            "{quality:?}: {:?}",
            px(&pixels, 64, 32, 24)
        );
        assert!(near(px(&pixels, 64, 1, 1), [0, 0, 0], 2), "{quality:?}");
        assert_eq!((stats.parts, stats.triangles), (1, 2));
        assert_eq!(stats.drawn, quality.drawn_size(size));
        assert_eq!(stats.meshes_cached, 1);
        // Scaling up is one more draw.
        assert_eq!(stats.draw_calls, if stats.drawn == size { 1 } else { 2 });
        assert_eq!(
            stats.samples,
            Renderer::samples_for(&gpu, RGBA, quality.msaa)
        );
        eprintln!("{quality:?}: {} samples", stats.samples);
        assert_eq!(
            stats.samples_shaded(),
            u64::from(stats.drawn.0 * stats.drawn.1 * stats.samples)
        );
        assert!(stats.cpu_time > std::time::Duration::ZERO);
    }
    // A zero-sized target draws nothing and doesn't fail.
    let tex = target(&gpu, (4, 4), RGBA);
    let view = tex.create_view(&Default::default());
    let stats = r.render(
        &gpu,
        &Target {
            view: &view,
            format: RGBA,
            size: (0, 0),
            owner: 0,
        },
        &scene,
        &looking_down_z(2.0),
        &no_outputs,
        &options,
    );
    assert_eq!(stats.draw_calls, 0);
}

#[test]
fn a_nearer_square_hides_a_farther_one_whatever_the_order() {
    let Some(gpu) = test_gpu() else { return };
    let mut r = Renderer::new(&gpu);
    let red = part(
        square(),
        1,
        moved([0.0, 0.0, -1.0], 1.0),
        Paint::Colour([1.0, 0.0, 0.0, 1.0]),
        false,
    );
    let green = part(
        square(),
        1,
        moved([0.0, 0.0, 0.5], 0.3),
        Paint::Colour([0.0, 1.0, 0.0, 1.0]),
        false,
    );
    for scene in [
        Scene {
            throws: Vec::new(),
            parts: vec![green, red],
        },
        Scene {
            throws: Vec::new(),
            parts: vec![
                part(
                    square(),
                    1,
                    moved([0.0, 0.0, -1.0], 1.0),
                    Paint::Colour([1.0, 0.0, 0.0, 1.0]),
                    false,
                ),
                part(
                    square(),
                    1,
                    moved([0.0, 0.0, 0.5], 0.3),
                    Paint::Colour([0.0, 1.0, 0.0, 1.0]),
                    false,
                ),
            ],
        },
    ] {
        let (pixels, stats) = draw(
            &gpu,
            &mut r,
            &scene,
            &looking_down_z(3.0),
            &no_outputs,
            &Options::default(),
            (64, 64),
            RGBA,
        );
        assert!(near(px(&pixels, 64, 32, 32), [0, 255, 0], 2));
        // Beyond the small green one, the red one shows.
        let red_only = looking_down_z(3.0)
            .project([0.4, 0.0, -1.0], (64, 64))
            .unwrap();
        assert!(near(
            px(&pixels, 64, red_only[0] as u32, red_only[1] as u32),
            [255, 0, 0],
            2
        ));
        // One mesh, drawn twice.
        assert_eq!((stats.meshes_cached, stats.draw_calls), (1, 2));
    }
}

fn brightness(p: [u8; 4]) -> i32 {
    i32::from(p[0]) + i32::from(p[1]) + i32::from(p[2])
}

#[test]
fn a_lit_cube_is_lighter_where_it_faces_the_camera() {
    let Some(gpu) = test_gpu() else { return };
    let mut r = Renderer::new(&gpu);
    // Turned 25° about Y: its front face is 25° from the view, its side 65°.
    let scene = Scene {
        throws: Vec::new(),
        parts: vec![part(
            cube(),
            2,
            turned_y(25.0, [0.0; 3]),
            Paint::Colour([0.5, 0.5, 0.5, 1.0]),
            true,
        )],
    };
    let camera = looking_down_z(3.0);
    let size = (128, 128);
    let (pixels, _) = draw(
        &gpu,
        &mut r,
        &scene,
        &camera,
        &no_outputs,
        &Options::default(),
        size,
        RGBA,
    );
    let m = turned_y(25.0, [0.0; 3]);
    let at = |p: [f32; 3]| {
        let s = camera.project(maths::transform_point(&m, p), size).unwrap();
        px(&pixels, size.0, s[0] as u32, s[1] as u32)
    };
    let front = at([0.0, 0.0, 0.5]);
    // The side that can be seen (turned towards +X, the -X face shows).
    let side = at([-0.5, 0.0, 0.0]);
    assert!(
        brightness(front) > brightness(side) + 30,
        "front {front:?}, side {side:?}"
    );
    // Even the side at an angle is well clear of the background (readable).
    assert!(brightness(side) > 3 * 60, "{side:?}");

    // A near-black venue is still readable thanks to the lift.
    let dark = Scene {
        throws: Vec::new(),
        parts: vec![part(
            cube(),
            2,
            turned_y(25.0, [0.0; 3]),
            Paint::Colour([0.03, 0.03, 0.03, 1.0]),
            true,
        )],
    };
    let (pixels, _) = draw(
        &gpu,
        &mut r,
        &dark,
        &camera,
        &no_outputs,
        &Options::default(),
        size,
        RGBA,
    );
    let s = camera
        .project(maths::transform_point(&m, [0.0, 0.0, 0.5]), size)
        .unwrap();
    let p = px(&pixels, size.0, s[0] as u32, s[1] as u32);
    assert!(brightness(p) > 3 * 45, "{p:?}");
    // Brighter when asked.
    let (pixels, _) = draw(
        &gpu,
        &mut r,
        &dark,
        &camera,
        &no_outputs,
        &Options {
            brightness: 2.0,
            ..Options::default()
        },
        size,
        RGBA,
    );
    assert!(brightness(px(&pixels, size.0, s[0] as u32, s[1] as u32)) > brightness(p) + 30);
}

/// A 16×16 picture in quarters: red, green / blue, a dark orange (mid and
/// dark tones, which a wrong gamma would change).
fn four_colours(gpu: &Gpu) -> wgpu::TextureView {
    use wgpu::util::DeviceExt;
    let mut pixels = Vec::with_capacity(16 * 16 * 4);
    for y in 0..16 {
        for x in 0..16 {
            pixels.extend_from_slice(match (x < 8, y < 8) {
                (true, true) => &[255, 0, 0, 255],
                (false, true) => &[0, 255, 0, 255],
                (true, false) => &[0, 0, 255, 255],
                (false, false) => &[128, 64, 20, 255],
            });
        }
    }
    gpu.device
        .create_texture_with_data(
            &gpu.queue,
            &wgpu::TextureDescriptor {
                label: Some("four colours"),
                size: wgpu::Extent3d {
                    width: 16,
                    height: 16,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: RGBA,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &pixels,
        )
        .create_view(&Default::default())
}

#[test]
fn a_screen_shows_its_display_the_right_way_up_and_its_region() {
    let Some(gpu) = test_gpu() else { return };
    let mut r = Renderer::new(&gpu);
    let picture = four_colours(&gpu);
    let display = OutputId(7);
    let lookup = |o: OutputId| (o == display).then_some(&picture);
    let screen = |region| Scene {
        throws: Vec::new(),
        parts: vec![part(
            square(),
            3,
            moved([0.0; 3], 2.0),
            Paint::Output {
                output: display,
                region,
            },
            false,
        )],
    };
    let camera = looking_down_z(2.0);
    let size = (100, 100);
    let quarter = |pixels: &[u8], x: f32, y: f32| {
        let s = camera.project([x, y, 0.0], size).unwrap();
        px(pixels, size.0, s[0] as u32, s[1] as u32)
    };
    for format in [
        RGBA,
        wgpu::TextureFormat::Bgra8Unorm,
        wgpu::TextureFormat::Rgba8UnormSrgb,
    ] {
        let (mut pixels, stats) = draw(
            &gpu,
            &mut r,
            &screen([0.0, 0.0, 1.0, 1.0]),
            &camera,
            &lookup,
            &Options::default(),
            size,
            format,
        );
        if format == wgpu::TextureFormat::Bgra8Unorm {
            for p in pixels.as_chunks_mut::<4>().0 {
                p.swap(0, 2);
            }
        }
        assert_eq!(stats.missing_pictures, 0);
        assert!(
            near(quarter(&pixels, -0.5, 0.5), [255, 0, 0], 3),
            "{format:?} top-left"
        );
        assert!(
            near(quarter(&pixels, 0.5, 0.5), [0, 255, 0], 3),
            "{format:?} top-right"
        );
        assert!(
            near(quarter(&pixels, -0.5, -0.5), [0, 0, 255], 3),
            "{format:?} bottom-left"
        );
        assert!(
            near(quarter(&pixels, 0.5, -0.5), [128, 64, 20], 3),
            "{format:?} bottom-right"
        );
    }
    // The top-right quarter of the picture over the whole screen.
    let (pixels, _) = draw(
        &gpu,
        &mut r,
        &screen([0.5, 0.0, 0.5, 0.5]),
        &camera,
        &lookup,
        &Options::default(),
        size,
        RGBA,
    );
    for (x, y) in [(-0.6, 0.6), (0.6, -0.6), (0.0, 0.0)] {
        assert!(near(quarter(&pixels, x, y), [0, 255, 0], 3));
    }
    // Seen from behind it is still drawn (mirrored).
    let behind = Camera {
        eye: [0.0, 0.0, -2.0],
        ..looking_down_z(2.0)
    };
    let (pixels, _) = draw(
        &gpu,
        &mut r,
        &screen([0.0, 0.0, 1.0, 1.0]),
        &behind,
        &lookup,
        &Options::default(),
        size,
        RGBA,
    );
    assert!(brightness(px(&pixels, 100, 50, 50)) > 3 * 40);
    // A display with no picture: dark grey, a lighter edge, and counted.
    let (pixels, stats) = draw(
        &gpu,
        &mut r,
        &screen([0.0, 0.0, 1.0, 1.0]),
        &camera,
        &no_outputs,
        &Options::default(),
        size,
        RGBA,
    );
    assert_eq!(stats.missing_pictures, 1);
    let middle = px(&pixels, 100, 50, 50);
    assert!(near(middle, [31, 31, 31], 4), "{middle:?}");
}

#[test]
fn images_and_meshes_are_kept_by_key_and_let_go() {
    let Some(gpu) = test_gpu() else { return };
    let mut r = Renderer::new(&gpu);
    let image = |key| Paint::Image {
        rgba: Arc::new([0u8, 0, 255, 255].repeat(4)),
        width: 2,
        height: 2,
        key,
    };
    let scene = Scene {
        throws: Vec::new(),
        parts: vec![
            part(square(), 1, maths::IDENTITY, image(10), false),
            part(square(), 1, moved([0.0, 0.0, -1.0], 1.0), image(10), false),
            // Bad: pixels short of the size; drawn grey, not kept.
            part(
                square(),
                1,
                moved([0.0, 0.0, -2.0], 1.0),
                Paint::Image {
                    rgba: Arc::new(vec![0; 3]),
                    width: 2,
                    height: 2,
                    key: 11,
                },
                false,
            ),
            // No triangles at all: skipped.
            part(
                Arc::new(Mesh {
                    positions: vec![[0.0; 3]],
                    normals: vec![],
                    uvs: vec![],
                    indices: vec![0, 1, 2],
                }),
                2,
                maths::IDENTITY,
                image(10),
                true,
            ),
        ],
    };
    let (pixels, stats) = draw(
        &gpu,
        &mut r,
        &scene,
        &looking_down_z(2.0),
        &no_outputs,
        &Options::default(),
        (32, 32),
        RGBA,
    );
    assert!(near(px(&pixels, 32, 16, 16), [0, 0, 255], 2));
    assert_eq!(stats.parts, 3);
    assert_eq!((stats.meshes_cached, stats.images_cached), (1, 1));
    // Kept while in use; let go once unused for long enough.
    r.tidy();
    assert_eq!((r.meshes.len(), r.images.len(), r.targets.len()), (1, 1, 1));
    r.tidy_older_than(std::time::Duration::ZERO);
    assert!(r.meshes.is_empty() && r.images.is_empty() && r.targets.is_empty());
    // A second view of another size keeps its own targets beside the first.
    let view = |r: &mut Renderer, owner, size, quality: Quality| {
        draw_for(
            owner,
            &gpu,
            r,
            &scene,
            &looking_down_z(2.0),
            &no_outputs,
            &Options {
                quality,
                ..Options::default()
            },
            size,
            RGBA,
        )
    };
    let full = Quality::default();
    view(&mut r, 1, (32, 32), full);
    view(&mut r, 2, (40, 20), full);
    assert_eq!(r.targets.len(), 2);
    // The governor's steps at one size are kept side by side...
    view(&mut r, 2, (40, 20), full.scaled_by(0.5));
    assert_eq!(r.targets.len(), 3);
    // ...but a view resized lets its old size go (a pane dragged wider
    // makes no pile of depth and multisampled textures).
    for w in [44, 48, 52, 56, 60] {
        view(&mut r, 2, (w, 20), full);
    }
    assert_eq!(r.targets.len(), 2);
    // A venue read again (new pictures and meshes): the old go at once.
    let other = Scene {
        throws: Vec::new(),
        parts: vec![part(cube(), 3, maths::IDENTITY, image(12), true)],
    };
    draw(
        &gpu,
        &mut r,
        &other,
        &looking_down_z(2.0),
        &no_outputs,
        &Options::default(),
        (32, 32),
        RGBA,
    );
    assert_eq!((r.meshes.len(), r.images.len()), (2, 2));
    r.keep_only(&[&other]);
    assert_eq!((r.meshes.len(), r.images.len()), (1, 1));
    assert!(r.meshes.contains_key(&3) && r.images.contains_key(&12));
    r.forget_all();
    assert!(r.meshes.is_empty() && r.images.is_empty() && r.targets.is_empty());
}

#[test]
fn a_mesh_too_big_for_the_device_is_left_out_not_made() {
    let Some(gpu) = test_gpu() else { return };
    let mut r = Renderer::new(&gpu);
    // The square (4 corners, 128 bytes) fits; the cube (24 corners, 768
    // bytes) doesn't.
    r.limit_buffers(256);
    let scene = Scene {
        throws: Vec::new(),
        parts: vec![
            part(
                cube(),
                7,
                moved([0.0, 0.0, -1.0], 3.0),
                Paint::Colour([1.0, 0.0, 0.0, 1.0]),
                false,
            ),
            part(
                square(),
                1,
                maths::IDENTITY,
                Paint::Colour([0.0, 0.0, 1.0, 1.0]),
                false,
            ),
        ],
    };
    for _ in 0..3 {
        let (pixels, stats) = draw(
            &gpu,
            &mut r,
            &scene,
            &looking_down_z(2.0),
            &no_outputs,
            &Options::default(),
            (32, 32),
            RGBA,
        );
        assert_eq!(stats.parts, 1);
        assert!(near(px(&pixels, 32, 16, 16), [0, 0, 255], 2));
    }
    assert_eq!(r.meshes.len(), 1);
    assert!(r.too_big.contains(&7));
}

#[test]
fn see_through_venues_show_the_screen_behind_a_wall() {
    let Some(gpu) = test_gpu() else { return };
    let mut r = Renderer::new(&gpu);
    let scene = Scene {
        throws: Vec::new(),
        parts: vec![
            // A wall in front...
            part(
                square(),
                1,
                moved([0.0, 0.0, 0.5], 2.0),
                Paint::Colour([0.6, 0.6, 0.6, 1.0]),
                true,
            ),
            // ... of a red screen.
            part(
                square(),
                1,
                moved([0.0, 0.0, -0.5], 1.0),
                Paint::Colour([1.0, 0.0, 0.0, 1.0]),
                false,
            ),
        ],
    };
    let draw_with = |r: &mut Renderer, venue_opacity| {
        draw(
            &gpu,
            r,
            &scene,
            &looking_down_z(3.0),
            &no_outputs,
            &Options {
                venue_opacity,
                ..Options::default()
            },
            (32, 32),
            RGBA,
        )
        .0
    };
    let solid = px(&draw_with(&mut r, 1.0), 32, 16, 16);
    assert!(i32::from(solid[0]) - i32::from(solid[1]) < 10, "{solid:?}");
    let xray = px(&draw_with(&mut r, 0.3), 32, 16, 16);
    assert!(i32::from(xray[0]) - i32::from(xray[1]) > 80, "{xray:?}");
}

/// A curved strip of `columns` quads: part of a cylinder of `radius` about
/// the Y axis, `angle` degrees wide and `height` high, facing the axis (its
/// inside), uv across it.
fn curved_strip(columns: u32, radius: f32, angle: f32, height: f32) -> Mesh {
    let mut m = Mesh {
        positions: vec![],
        normals: vec![],
        uvs: vec![],
        indices: vec![],
    };
    for i in 0..=columns {
        let u = i as f32 / columns as f32;
        let a = (u - 0.5) * angle.to_radians();
        let (s, c) = a.sin_cos();
        for (y, v) in [(height, 0.0), (0.0, 1.0)] {
            m.positions.push([radius * s, y, -radius * c]);
            m.normals.push([-s, 0.0, c]);
            m.uvs.push([u, v]);
        }
    }
    for i in 0..columns {
        let b = i * 2;
        m.indices.extend([b, b + 2, b + 3, b, b + 3, b + 1]);
    }
    m
}

/// The little scene drawn to a PNG to look at (`NEWQ_RENDER3D_PNG=<path>`):
/// a floor grid, two coloured blocks and a curved screen showing a test
/// picture.
#[test]
fn a_little_stage_draws() {
    let Some(gpu) = test_gpu() else { return };
    let mut r = Renderer::new(&gpu);
    let (pw, ph) = (480u32, 270u32);
    let bars = crate::pattern::render(crate::show::PatternKind::default(), pw, ph, 1);
    use wgpu::util::DeviceExt;
    let picture = gpu
        .device
        .create_texture_with_data(
            &gpu.queue,
            &wgpu::TextureDescriptor {
                label: Some("test picture"),
                size: wgpu::Extent3d {
                    width: pw,
                    height: ph,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: RGBA,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &bars,
        )
        .create_view(&Default::default());
    let lookup = |o: OutputId| (o == OutputId(1)).then_some(&picture);
    let scene = Scene {
        throws: Vec::new(),
        parts: vec![
            part(
                cube(),
                2,
                {
                    let mut m = turned_y(20.0, [-3.0, 0.75, -2.0]);
                    m[0] = m[0].map(|v| v * 1.5);
                    m[1] = m[1].map(|v| v * 1.5);
                    m[2] = m[2].map(|v| v * 1.5);
                    m
                },
                Paint::Colour([0.75, 0.3, 0.2, 1.0]),
                true,
            ),
            part(
                cube(),
                2,
                turned_y(-30.0, [3.0, 0.5, -1.0]),
                Paint::Colour([0.2, 0.45, 0.8, 1.0]),
                true,
            ),
            // A dark stage deck.
            part(
                cube(),
                2,
                {
                    let mut m = maths::IDENTITY;
                    m[0][0] = 10.0;
                    m[1][1] = 0.4;
                    m[2][2] = 4.0;
                    m[3] = [0.0, 0.2, -4.0, 1.0];
                    m
                },
                Paint::Colour([0.05, 0.05, 0.06, 1.0]),
                true,
            ),
            part(
                Arc::new(curved_strip(24, 9.0, 50.0, 4.0)),
                4,
                moved([0.0, 1.0, 3.0], 1.0),
                Paint::Output {
                    output: OutputId(1),
                    region: [0.0, 0.0, 1.0, 1.0],
                },
                false,
            ),
        ],
    };
    let size = (960, 540);
    let mut orbit = Orbit {
        yaw: 0.35,
        pitch: 0.3,
        ..Orbit::default()
    };
    assert!(orbit.frame_scene(&scene, size.0 as f32 / size.1 as f32));
    orbit.dolly(1.0);
    let options = Options {
        grid: true,
        axes: true,
        ..Options::default()
    };
    // Each of the overlays alone first, with any GPU error caught, so a
    // failure says which part of the frame went wrong (and why).
    let mut seen = Vec::new();
    for (grid, axes) in [(false, false), (true, false), (false, true), (true, true)] {
        let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let (pixels, _) = draw(
            &gpu,
            &mut r,
            &scene,
            &orbit.camera(),
            &lookup,
            &Options {
                grid,
                axes,
                ..options
            },
            size,
            RGBA,
        );
        let error = pollster::block_on(scope.pop());
        let mid = orbit.camera().project([0.0, 4.5, -6.0], size).unwrap();
        let p = px(&pixels, size.0, mid[0] as u32, mid[1] as u32);
        let lit = pixels.chunks(4).filter(|p| p[3] != 0).count();
        seen.push(format!(
            "grid {grid} axes {axes}: middle {p:?}, {lit} pixels written, error {error:?}"
        ));
    }
    let report = format!(
        "{} ({:?})\n{}",
        gpu.adapter.get_info().name,
        gpu.adapter.get_info().backend,
        seen.join("\n")
    );
    let (pixels, stats) = draw(
        &gpu,
        &mut r,
        &scene,
        &orbit.camera(),
        &lookup,
        &options,
        size,
        RGBA,
    );
    if let Some(path) = std::env::var_os("NEWQ_RENDER3D_PNG") {
        image::RgbaImage::from_raw(size.0, size.1, pixels.clone())
            .unwrap()
            .save(path)
            .unwrap();
    }
    assert_eq!(stats.parts, 4);
    assert_eq!(stats.draw_calls, 6);
    assert_eq!(stats.triangles, 12 * 3 + 48 + 2);
    // The screen shows the picture (the bars at its top, not the missing grey).
    let mid = orbit.camera().project([0.0, 4.5, -6.0], size).unwrap();
    let p = px(&pixels, size.0, mid[0] as u32, mid[1] as u32);
    assert!(brightness(p) > 3 * 60, "{p:?}\n{report}");
}

/// Whether a pixel within one of `at` (in a picture `w` wide) is near `colour`.
fn line_near(pixels: &[u8], w: u32, h: u32, at: [f32; 2], colour: [u8; 3]) -> bool {
    let (x, y) = (at[0].round() as i64, at[1].round() as i64);
    (-1..=1).any(|dy| {
        (-1..=1).any(|dx| {
            let (qx, qy) = (x + dx, y + dy);
            qx >= 0
                && qy >= 0
                && (qx as u32) < w
                && (qy as u32) < h
                && near(px(pixels, w, qx as u32, qy as u32), colour, 40)
        })
    })
}

/// Black, no multisampling: a line is its own colour on its pixels.
fn plain() -> Options {
    Options {
        quality: Quality {
            msaa: false,
            scale: 1.0,
        },
        background: [0.0, 0.0, 0.0],
        ..Options::default()
    }
}

#[test]
fn lines_are_drawn_over_everything_where_the_camera_says() {
    let Some(gpu) = test_gpu() else { return };
    let mut r = Renderer::new(&gpu);
    // A wall in front of the line: the line still shows, over it.
    let scene = Scene {
        throws: Vec::new(),
        parts: vec![part(
            square(),
            1,
            moved([0.0, 0.0, 1.0], 3.0),
            Paint::Colour([0.2, 0.2, 0.2, 1.0]),
            false,
        )],
    };
    let line = Line {
        a: [-1.0, 0.5, -1.0],
        b: [1.0, 0.5, -1.0],
        colour: [1.0, 0.0, 0.0, 1.0],
    };
    let camera = looking_down_z(4.0);
    let size = (160u32, 120u32);
    for format in [RGBA, wgpu::TextureFormat::Rgba8UnormSrgb] {
        let tex = target(&gpu, size, format);
        let view = tex.create_view(&Default::default());
        let stats = r.render_with_lines(
            &gpu,
            &Target {
                view: &view,
                format,
                size,
                owner: 0,
            },
            &scene,
            &camera,
            &no_outputs,
            &plain(),
            &[line],
        );
        assert_eq!(stats.lines, 1);
        let pixels = read(&gpu, &tex);
        for t in [0.1f32, 0.5, 0.9] {
            let p = [-1.0 + 2.0 * t, 0.5, -1.0];
            let at = camera.project(p, size).unwrap();
            assert!(
                line_near(&pixels, size.0, size.1, at, [255, 0, 0]),
                "{format:?}: no line at {at:?}"
            );
        }
        // Away from it, the wall.
        let off = camera.project([0.0, -0.3, 1.0], size).unwrap();
        assert!(!line_near(&pixels, size.0, size.1, off, [255, 0, 0]));
    }
    // Without lines, nothing is drawn for them.
    let (_, stats) = draw(
        &gpu,
        &mut r,
        &scene,
        &camera,
        &no_outputs,
        &plain(),
        size,
        RGBA,
    );
    assert_eq!(stats.lines, 0);
}

/// A show with a flat screen and a projector made to fill it, its lens
/// shifted up, and the scene of it.
fn fitted_projector() -> (crate::show::Show, crate::stage3d::Projector, Scene) {
    let mut show = crate::show::Show::default();
    let s = show.add_surface(None);
    show.stage3d.surface_mut(s).unwrap().placement.position = [0.0, 3.0, -4.0];
    let id = show.add_projector();
    let surface = show.stage3d.surface(s).unwrap().clone();
    let p = show.projector_mut(id).unwrap();
    p.lens.shift = [0.0, 0.25];
    p.placement = p.fit_to(&surface, 16.0 / 9.0).unwrap();
    let p = p.clone();
    let scene = show.stage3d.scene(&show, None);
    (show, p, scene)
}

#[test]
fn a_projectors_cone_and_outline_are_drawn_where_its_picture_lands() {
    let Some(gpu) = test_gpu() else { return };
    let mut r = Renderer::new(&gpu);
    let (show, p, scene) = fitted_projector();
    let aspect = 16.0 / 9.0;
    let from = show.stage3d.projectors_from(&scene);
    // (The screen a touch bigger to be found on: a ray down its exact edge
    // may pass either side.)
    let mut bigger = show.clone();
    if let crate::stage3d::SurfaceShape::Flat { width, height } =
        &mut bigger.stage3d.surfaces[0].shape
    {
        *width += 0.01;
        *height += 0.01;
    }
    let found_on = bigger.stage3d.scene(&bigger, None);
    let fp = p.footprint(&found_on, &|i, _| i < from, aspect, 8);
    assert!(fp.corners.iter().all(Option::is_some), "{:?}", fp.corners);
    let yellow = [1.0, 0.85, 0.2, 1.0];
    let mut lines: Vec<Line> = p
        .cone(&fp, aspect, 30.0)
        .into_iter()
        .map(|[a, b]| Line {
            a,
            b,
            colour: yellow,
        })
        .collect();
    for piece in &fp.outline {
        for w in piece.windows(2) {
            lines.push(Line {
                a: w[0],
                b: w[1],
                colour: yellow,
            });
        }
    }
    // Seen from the side and above, so the cone isn't edge on.
    let camera = Camera {
        eye: [9.0, 8.0, 6.0],
        target: [0.0, 3.0, 0.0],
        ..Camera::default()
    };
    let size = (320u32, 240u32);
    let tex = target(&gpu, size, RGBA);
    let view = tex.create_view(&Default::default());
    let stats = r.render_with_lines(
        &gpu,
        &Target {
            view: &view,
            format: RGBA,
            size,
            owner: 1,
        },
        &scene,
        &camera,
        &no_outputs,
        &plain(),
        &lines,
    );
    assert_eq!(stats.lines as usize, lines.len());
    let pixels = read(&gpu, &tex);
    let y = [255, 217, 51];
    // The picture's corners (on the screen's corners) and the lens.
    for c in fp.corners.iter().flatten() {
        let at = camera.project(*c, size).unwrap();
        assert!(
            line_near(&pixels, size.0, size.1, at, y),
            "corner {c:?} at {at:?}"
        );
    }
    let at = camera.project(p.position(), size).unwrap();
    assert!(line_near(&pixels, size.0, size.1, at, y), "lens at {at:?}");
    // Half way along a cone edge.
    let c = fp.corners[2].unwrap();
    let m = maths::scale(maths::add(c, p.position()), 0.5);
    let at = camera.project(m, size).unwrap();
    assert!(line_near(&pixels, size.0, size.1, at, y), "edge at {at:?}");
}

#[test]
fn looking_through_a_shifted_lens_sees_the_screen_it_fills_letterboxed() {
    let Some(gpu) = test_gpu() else { return };
    let mut r = Renderer::new(&gpu);
    let (_show, p, scene) = fitted_projector();
    let camera = p.camera(16.0 / 9.0);
    // A 4:3 target: the 16:9 picture with a band above and below.
    let size = (160u32, 120u32);
    let (pixels, _) = draw(
        &gpu,
        &mut r,
        &scene,
        &camera,
        &no_outputs,
        &plain(),
        size,
        RGBA,
    );
    let [l, t, rr, b] = camera.picture_in(size);
    assert_eq!((l, rr), (0.0, 160.0));
    assert!((t - 15.0).abs() < 1e-3 && (b - 105.0).abs() < 1e-3);
    let grey = (crate::stage3d::UNFED_GREY[0] * 255.0).round() as u8;
    let is_screen = |x: u32, y: u32| near(px(&pixels, size.0, x, y), [grey; 3], 4);
    // The screen fills the picture, to a pixel or two of each corner...
    for (x, y) in [(80, 60), (2, 17), (157, 17), (2, 102), (157, 102)] {
        assert!(is_screen(x, y), "{x},{y}: {:?}", px(&pixels, size.0, x, y));
    }
    // ...and not the bands.
    for (x, y) in [(80, 5), (80, 114), (5, 12), (150, 109)] {
        assert!(!is_screen(x, y), "{x},{y}: {:?}", px(&pixels, size.0, x, y));
    }
}
