//! The bake drawn for real and read back, against the CPU reference
//! (`stage3d::lensmap::reference`). Skipped where there is no GPU adapter
//! (`NEWQ_REQUIRE_GPU` makes that a failure); under Xvfb or headless,
//! `WGPU_BACKEND=vulkan` with lavapipe.

use super::*;
use crate::mapped::{CpuMap, unpack_how, unpack_place};
use crate::render3d::{Options, Target};
use crate::show::Show;
use crate::stage3d::lensmap::tests::{box_part, flat_show, still_of};
use crate::stage3d::lensmap::{map_key, reference_with, still_key};
use crate::stage3d::{Facet, Feed, Placement, ProjectorId, SurfaceShape, geometry};

/// How far a GPU place may be from the reference's, away from edges: a
/// share of the feed's picture (the packing's own step is 1/65535)...
const PLACE_WITHIN: f32 = 3e-5;
/// ...and a share of one projector pixel's step in the feed's picture
/// there. (The bake averages four samples a quarter of a pixel off the
/// middle; where a curved or faceted screen bends within the pixel, the
/// average is a little off the middle's place, by a share of a pixel
/// whatever the map's size.)
const PIXEL_WITHIN: f32 = 0.02;

fn test_gpu() -> Option<Gpu> {
    let gpu = Gpu::headless();
    if gpu.is_none() && std::env::var_os("NEWQ_REQUIRE_GPU").is_some() {
        panic!("NEWQ_REQUIRE_GPU is set but there is no GPU adapter");
    }
    if gpu.is_none() {
        eprintln!("no GPU adapter here; skipping the lens map bake test");
    }
    gpu
}

/// A texture's pixels, rows packed.
fn read(gpu: &Gpu, tex: &wgpu::Texture) -> Vec<[u8; 4]> {
    let (w, h) = (tex.width(), tex.height());
    let row = (w * 4).next_multiple_of(256);
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("lens map readback"),
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
    let mut out = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        let start = (y * row) as usize;
        for x in 0..w as usize {
            let i = start + x * 4;
            out.push([data[i], data[i + 1], data[i + 2], data[i + 3]]);
        }
    }
    out
}

/// The map read back from the card.
fn read_map(gpu: &Gpu, map: &LensMap) -> CpuMap {
    CpuMap {
        size: map.size,
        place: read(gpu, &map.place),
        how: read(gpu, &map.how),
        depth: None,
        depth_range: map.depth_range,
        feeds: map.feeds.clone(),
    }
}

const SIZE: (u32, u32) = (64, 36);

/// Bake projector `p`'s map of `scene` and its reference, at `SIZE`.
fn both(
    gpu: &Gpu,
    renderer: &mut Renderer,
    show: &Show,
    scene: &Scene,
    p: ProjectorId,
) -> (CpuMap, CpuMap) {
    let pr = show.projector(p).expect("there");
    let aspect = pr.aspect(SIZE);
    let key = map_key(still_key(scene), pr, aspect, SIZE, false);
    let baked = renderer
        .bake(
            gpu,
            &Bake {
                scene,
                projector: pr,
                aspect,
                size: SIZE,
                key,
            },
        )
        .expect("baked");
    assert_eq!(baked.key, key);
    assert_eq!(baked.size, SIZE);
    (
        read_map(gpu, &baked),
        reference_with(scene, pr, SIZE, aspect),
    )
}

/// What the card's map and the reference agree on, pixel for pixel away
/// from the edges of what the reference sees: (pixels compared, of them
/// showing a feed). Fails at the first one off.
fn agree(gpu_map: &CpuMap, cpu: &CpuMap) -> (usize, usize) {
    assert_eq!(gpu_map.feeds, cpu.feeds, "the same canvases, in order");
    let (w, h) = cpu.size;
    let feed = |m: &CpuMap, x: u32, y: u32| unpack_how(m.how[(y * w + x) as usize]).feed;
    let mut compared = 0;
    let mut shown = 0;
    let mut worst = 0.0f32;
    for y in 0..h {
        for x in 0..w {
            let want = feed(cpu, x, y);
            // An edge: a neighbour that sees something else (or none, at the
            // picture's own edge).
            let edge = x == 0
                || y == 0
                || x + 1 == w
                || y + 1 == h
                || (y - 1..y + 2)
                    .flat_map(|ny| (x - 1..x + 2).map(move |nx| (nx, ny)))
                    .any(|(nx, ny)| feed(cpu, nx, ny) != want);
            if edge {
                continue;
            }
            let i = (y * w + x) as usize;
            let got = unpack_how(gpu_map.how[i]);
            assert_eq!(got.feed, want, "the feed at ({x}, {y})");
            compared += 1;
            if want.is_some() {
                shown += 1;
                assert_eq!(got.coverage, 1.0, "covered at ({x}, {y})");
                assert_eq!(got.share, 1.0);
                let place =
                    |m: &CpuMap, x: u32, y: u32| unpack_place(m.place[(y * w + x) as usize]);
                let (a, b) = (place(gpu_map, x, y), place(cpu, x, y));
                // One pixel's step in the feed's picture here.
                let step = [place(cpu, x + 1, y), place(cpu, x, y + 1)]
                    .iter()
                    .map(|n| (n[0] - b[0]).abs().max((n[1] - b[1]).abs()))
                    .fold(0.0f32, f32::max);
                let off = (a[0] - b[0]).abs().max((a[1] - b[1]).abs());
                worst = worst.max(off / step.max(1e-9));
                assert!(
                    off <= PLACE_WITHIN + PIXEL_WITHIN * step,
                    "the place at ({x}, {y}): {a:?} on the card, {b:?} worked out"
                );
            }
        }
    }
    eprintln!(
        "compared {compared} pixels, {shown} showing a feed; worst place off by {worst} of a pixel"
    );
    (compared, shown)
}

/// Most of the picture compared, and some of it showing a feed.
fn enough(got: (usize, usize), shown_at_least: usize) {
    let all = (SIZE.0 * SIZE.1) as usize;
    assert!(got.0 > all / 2, "only {} of {all} compared", got.0);
    assert!(got.1 >= shown_at_least, "only {} showed a feed", got.1);
}

#[test]
fn a_flat_screen_bakes_as_the_reference_works_it_out() {
    let Some(gpu) = test_gpu() else { return };
    let mut r = Renderer::new(&gpu);
    // Filled, then from further off and to one side (the screen inside the
    // picture: its edges are inside it).
    let (mut show, _, _, p) = flat_show();
    let (g, c) = both(&gpu, &mut r, &show, &still_of(&show), p);
    enough(agree(&g, &c), 2000);
    let pr = show.projector_mut(p).unwrap();
    pr.placement.position = [0.8, 0.5, pr.placement.position[2] * 1.7];
    pr.placement.yaw = 6.0;
    pr.placement.roll = 3.0;
    let (g, c) = both(&gpu, &mut r, &show, &still_of(&show), p);
    enough(agree(&g, &c), 500);
}

#[test]
fn a_curved_screen_bakes_as_the_reference_works_it_out() {
    let Some(gpu) = test_gpu() else { return };
    let mut r = Renderer::new(&gpu);
    let (mut show, _, s, p) = flat_show();
    for convex in [false, true] {
        show.stage3d.surface_mut(s).unwrap().shape = SurfaceShape::Curved {
            width: 5.0,
            radius: 3.0,
            height: 2.0,
            convex,
        };
        let pr = show.projector_mut(p).unwrap();
        pr.placement.position = [0.3, 0.2, 5.0];
        let (g, c) = both(&gpu, &mut r, &show, &still_of(&show), p);
        enough(agree(&g, &c), 500);
    }
}

#[test]
fn a_faceted_screen_bakes_as_the_reference_works_it_out() {
    let Some(gpu) = test_gpu() else { return };
    let mut r = Renderer::new(&gpu);
    let (mut show, _, s, p) = flat_show();
    // A wall of three panels bent towards the audience, each its share of
    // the picture; and a box of screens seen from a corner.
    let walls: [Vec<Facet>; 2] = [
        geometry::angled_wall(3, 1, 1.5, 2.0, 25.0),
        geometry::jumbotron(4, 2.0, 1.5, false, true),
    ];
    for facets in walls {
        show.stage3d.surface_mut(s).unwrap().shape = SurfaceShape::Faceted { facets };
        let pr = show.projector_mut(p).unwrap();
        pr.placement = Placement {
            position: [1.0, 0.5, 6.0],
            yaw: 8.0,
            pitch: -4.0,
            ..Placement::default()
        };
        let (g, c) = both(&gpu, &mut r, &show, &still_of(&show), p);
        enough(agree(&g, &c), 300);
    }
}

#[test]
fn a_box_in_the_way_is_baked_as_a_hole_and_a_screen_on_a_wall_wins() {
    let Some(gpu) = test_gpu() else { return };
    let mut r = Renderer::new(&gpu);
    let (show, _, _, p) = flat_show();
    let mut scene = still_of(&show);
    scene
        .parts
        .insert(0, box_part([0.3, -0.2, 0.6], [0.6, 0.4, 0.3]));
    let (g, c) = both(&gpu, &mut r, &show, &scene, p);
    enough(agree(&g, &c), 1000);
    // The box's shadow is there: the middle is black on the card too.
    let i = (18 * SIZE.0 + 36) as usize;
    assert_eq!(unpack_how(g.how[i]).feed, None);
    // A wall flush behind the screen: the screen everywhere, no fighting.
    let mut scene = still_of(&show);
    scene
        .parts
        .insert(0, box_part([0.0, 0.0, -0.5], [6.0, 6.0, 0.5]));
    let (g, c) = both(&gpu, &mut r, &show, &scene, p);
    enough(agree(&g, &c), 2000);
    assert!(
        g.how.iter().all(|h| unpack_how(*h).feed == Some(0)),
        "every pixel the screen"
    );
}

#[test]
fn front_and_rear_screens_bake_from_either_side_as_the_reference_says() {
    let Some(gpu) = test_gpu() else { return };
    let mut r = Renderer::new(&gpu);
    let (mut show, _, s, p) = flat_show();
    let pr = show.projector_mut(p).unwrap();
    pr.placement.position = [0.2, 0.1, -pr.placement.position[2] * 1.3];
    pr.placement.yaw = 180.0;
    // A front screen from behind: nothing at all.
    let (g, c) = both(&gpu, &mut r, &show, &still_of(&show), p);
    assert!(g.how.iter().all(|h| unpack_how(*h).feed.is_none()));
    agree(&g, &c);
    // A rear screen: its picture, mirrored, as worked out.
    show.stage3d.surface_mut(s).unwrap().rear = true;
    let (g, c) = both(&gpu, &mut r, &show, &still_of(&show), p);
    enough(agree(&g, &c), 1000);
}

#[test]
fn two_canvases_bake_as_two_feeds_in_the_scenes_order() {
    let Some(gpu) = test_gpu() else { return };
    let mut r = Renderer::new(&gpu);
    let (mut show, a, s, p) = flat_show();
    let b = show.add_display();
    show.stage3d.surface_mut(s).unwrap().placement.position = [-1.0, 0.0, 0.0];
    let right = show.add_surface(Some(SurfaceShape::Flat {
        width: 2.0,
        height: 2.0,
    }));
    let sr = show.stage3d.surface_mut(right).unwrap();
    sr.placement.position = [1.2, 0.0, 0.3];
    sr.feed = Feed::Display {
        output: b,
        region: [0.0, 0.0, 1.0, 1.0],
    };
    let (g, c) = both(&gpu, &mut r, &show, &still_of(&show), p);
    assert_eq!(g.feeds, vec![a, b]);
    enough(agree(&g, &c), 1500);
    let feeds: std::collections::HashSet<_> =
        g.how.iter().filter_map(|h| unpack_how(*h).feed).collect();
    assert_eq!(feeds.len(), 2);
}

#[test]
fn a_map_baked_in_tiles_is_the_map_baked_whole() {
    let Some(gpu) = test_gpu() else { return };
    let (mut show, _, _, p) = flat_show();
    let pr = show.projector_mut(p).unwrap();
    pr.placement.position = [0.4, 0.3, pr.placement.position[2] * 1.4];
    pr.placement.roll = 10.0;
    let scene = still_of(&show);
    let mut r = Renderer::new(&gpu);
    // Tiles that don't divide the picture: edge tiles are smaller.
    r.bake_in_tiles(10);
    let (g, c) = both(&gpu, &mut r, &show, &scene, p);
    enough(agree(&g, &c), 800);
}

#[test]
fn a_card_that_refuses_the_bake_refuses_it_and_still_draws_the_stage() {
    let Some(gpu) = test_gpu() else { return };
    let mut r = Renderer::new(&gpu);
    r.refuse_bake();
    let (show, _, _, p) = flat_show();
    let scene = still_of(&show);
    let pr = show.projector(p).unwrap();
    let job = Bake {
        scene: &scene,
        projector: pr,
        aspect: pr.aspect(SIZE),
        size: SIZE,
        key: 1,
    };
    assert!(r.bake(&gpu, &job).is_err());
    assert!(r.bake_refused());
    assert!(r.bake(&gpu, &job).is_err(), "and never tried again");
    // The stage is still drawn.
    let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 32,
            height: 18,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());
    let stats = r.render(
        &gpu,
        &Target {
            view: &view,
            format: wgpu::TextureFormat::Rgba8Unorm,
            size: (32, 18),
            owner: 1,
        },
        &scene,
        &pr.camera(pr.aspect(SIZE)),
        &|_| None,
        &Options::default(),
    );
    assert_eq!(stats.parts, 1);
}

/// The bake's shader as DirectX 12's HLSL and a Mac's MSL, written by naga
/// as wgpu does there (as `lit.rs`'s test does for the light's).
#[test]
fn the_bake_shader_is_written_for_directx_and_metal() {
    let src = source();
    let module = naga::front::wgsl::parse_str(&src).expect("the bake's shader reads");
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .expect("the bake's shader is valid");
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
        assert!(hlsl.contains("fs_bake") && hlsl.contains("fs_resolve"));
    }
    let (msl, _) =
        naga::back::msl::write_string(&module, &info, &Default::default(), &Default::default())
            .expect("written as MSL");
    assert!(msl.contains("fs_bake") && msl.contains("fs_resolve"));
}

#[test]
fn the_bakes_numbers_are_packed_as_its_shader_reads_them() {
    assert_eq!(size_of::<BakeFrame>(), 96);
    assert_eq!(size_of::<BakePart>(), 144);
    assert_eq!(size_of::<TileUniform>(), 16);
}
