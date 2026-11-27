//! A mapped projector in the light (mapping plan 4.1, round 2): its mapped
//! picture drawn on the stage thread (`ThrowJob::mapped`) and thrown back
//! onto the screen it was mapped for. The round trip: content laid straight
//! on a curved screen against the same content mapped through a baked lens
//! map and thrown by the simulation, within a tolerance away from edges
//! (lavapipe under `WGPU_BACKEND=vulkan`; skipped with no GPU adapter
//! unless `NEWQ_REQUIRE_GPU` is set).

use super::tests::{gpu, group, shown};
use super::*;
use crate::mapped::{FeedFrame, MappedFrame};
use crate::render3d::bake::Bake;
use crate::show::{OutputId, Show};
use crate::stage3d::lensmap::{feeds_for, map_key, still_key, still_scene};
use crate::stage3d::{Feed, SurfaceShape};

/// The canvas's picture: smooth, so a place a little off is a colour a
/// little off (red across, green down, blue even).
const CANVAS: (u32, u32) = (64, 36);
/// The projector's picture (the throw and its map).
const THROW: (u32, u32) = (256, 144);
/// The view of the stage.
const VIEW: (u32, u32) = (192, 108);

/// How far the content thrown back may be from the content laid straight
/// on, in display values (0..1), away from edges: on average...
/// (Found on lavapipe: 0.003 on average, 0.006 for 95 in a hundred, 0.009
/// at the worst: the 8-bit pictures' rounding. The same content thrown
/// unmapped is off by several times the line.)
pub(crate) const ROUND_TRIP_MEAN: f32 = 0.01;
/// ...for 95 of a hundred pixels...
pub(crate) const ROUND_TRIP_P95: f32 = 0.02;
/// ...and at the worst.
pub(crate) const ROUND_TRIP_WORST: f32 = 0.05;

fn gradient(gpu: &Gpu) -> Arc<LayerTexture> {
    let mut px = Vec::with_capacity((CANVAS.0 * CANVAS.1 * 4) as usize);
    for y in 0..CANVAS.1 {
        for x in 0..CANVAS.0 {
            let u = (x as f32 + 0.5) / CANVAS.0 as f32;
            let v = (y as f32 + 0.5) / CANVAS.1 as f32;
            px.extend_from_slice(&[
                ((0.3 + 0.65 * u) * 255.0).round() as u8,
                ((0.3 + 0.65 * v) * 255.0).round() as u8,
                (0.6f32 * 255.0).round() as u8,
                255,
            ]);
        }
    }
    let t = LayerTexture::new_rgba(gpu, CANVAS.0, CANVAS.1);
    t.upload_rgba(&gpu.queue, &px);
    Arc::new(t)
}

fn plain(gpu: &Gpu, v: u8) -> Arc<LayerTexture> {
    let t = LayerTexture::new_rgba(gpu, 2, 2);
    t.upload_rgba(&gpu.queue, &[[v, v, v, 255]; 4].concat());
    Arc::new(t)
}

/// A concave curved screen 5 m along its arc fed by canvas `a`, and a
/// projector above and to one side of the middle, mapped, filling it.
pub(crate) fn curved_show() -> (Show, OutputId, crate::stage3d::ProjectorId) {
    let mut show = Show::default();
    let a = show.outputs[0].id;
    let s = show.add_surface(Some(SurfaceShape::Curved {
        width: 5.0,
        radius: 3.5,
        height: 2.2,
        convex: false,
    }));
    show.stage3d.surface_mut(s).unwrap().feed = Feed::Display {
        output: a,
        region: [0.0, 0.0, 1.0, 1.0],
    };
    let p = show.add_projector();
    let pr = show.projector_mut(p).unwrap();
    pr.lens.aspect = Some(16.0 / 9.0);
    pr.lens.throw = 1.0;
    pr.lumens = 8000.0;
    pr.mapped = true;
    pr.placement.position = [0.4, 0.6, 5.5];
    pr.aim_at([0.0, 0.0, 0.0], 16.0 / 9.0);
    (show, a, p)
}

/// What the round trip found: pixels compared, and their errors.
#[derive(Debug)]
pub(crate) struct RoundTrip {
    pub compared: usize,
    pub mean: f32,
    pub p95: f32,
    pub worst: f32,
}

#[test]
fn content_mapped_and_thrown_back_is_the_content_laid_straight_on_a_curved_screen() {
    let Some(gpu) = gpu() else { return };
    let (show, a, p) = curved_show();
    let pr = show.projector(p).unwrap().clone();
    let aspect = 16.0 / 9.0;
    let still = still_scene(&show, &show.stage3d.scene(&show, None));
    let feeds = feeds_for(&show, p);
    assert_eq!(feeds, vec![a]);
    let mut drawer = StageDrawer::new(&gpu);
    let key = map_key(still_key(&still), &pr, aspect, THROW, false, &feeds);
    let map = Arc::new(
        drawer
            .renderer
            .bake(
                &gpu,
                &Bake {
                    scene: &still,
                    projector: &pr,
                    aspect,
                    size: THROW,
                    key,
                    feeds: &feeds,
                    depth: false,
                },
            )
            .expect("baked"),
    );
    let content = gradient(&gpu);
    let mapped_target = Arc::new(LayerTexture::new_target(&gpu, THROW.0, THROW.1));
    let frame = MappedFrame {
        target: mapped_target.clone(),
        map,
        feeds: vec![FeedFrame {
            output: a,
            groups: vec![group(&content, CANVAS)],
            canvas: CANVAS,
        }],
        moving: Vec::new(),
        lens: crate::render3d::maths::IDENTITY,
        shares: false,
        gamma: pr.lens.gamma(),
    };
    let mut scene = show.stage3d.scene(&show, None);
    scene.throws = vec![crate::stage3d::light::throw_of(&pr, aspect, THROW, 0, true)];
    let scene = Arc::new(scene);
    let camera = Camera {
        eye: [-1.2, 0.4, 4.0],
        target: [0.0, 0.0, -0.5],
        ..Camera::default()
    };
    let white_nits = 1.8
        * crate::stage3d::light::luminance(
            crate::stage3d::light::illuminance(
                &pr,
                aspect,
                [0.0, 0.0, 0.0],
                None,
                crate::stage3d::light::Facing::Front,
            ),
            1.0,
        );
    let target = Arc::new(LayerTexture::new_target(&gpu, VIEW.0, VIEW.1));
    let view = |light: LightView, throw: ThrowJob| StageJob {
        feeds: vec![FeedJob {
            output: a,
            groups: vec![group(&content, CANVAS)],
            size: CANVAS,
        }],
        throws: vec![throw],
        throw_size: THROW,
        views: vec![ViewJob {
            key: 9,
            display: false,
            target: target.clone(),
            scene: scene.clone(),
            camera,
            lines: Default::default(),
            options: Options {
                light,
                white_nits,
                headlight: 0.0,
                ambient: 0.0,
                ..Options::default()
            },
        }],
        serial: 1,
        ..StageJob::default()
    };
    let flat = |tex: &Arc<LayerTexture>| ThrowJob {
        layer: 0,
        groups: vec![group(tex, THROW)],
        blend: EdgeBlend::default(),
        mapped: None,
    };
    let mut look = |job: StageJob| {
        drawer.draw(&gpu, &job, &mut || {});
        shown(&gpu, &[group(&target, VIEW)], VIEW)
    };
    let (white, black) = (plain(&gpu, 255), plain(&gpu, 0));
    let straight = look(view(LightView::Off, flat(&white)));
    let lit_white = look(view(LightView::Picture, flat(&white)));
    let lit_black = look(view(LightView::Picture, flat(&black)));
    let lit_mapped = look(view(
        LightView::Picture,
        ThrowJob {
            layer: 0,
            groups: vec![group(&mapped_target, THROW)],
            blend: EdgeBlend::default(),
            mapped: Some(frame),
        },
    ));
    // The content thrown as it is, unmapped: what the test would see if the
    // map did nothing.
    let lit_flat = look(view(LightView::Picture, flat(&content)));
    assert!(mapped_target.is_filled(), "the mapped picture was drawn");
    let found = compare(
        &straight,
        &lit_white,
        &lit_black,
        &lit_mapped,
        pr.lens.gamma(),
    );
    let unmapped = compare(
        &straight,
        &lit_white,
        &lit_black,
        &lit_flat,
        pr.lens.gamma(),
    );
    eprintln!("round trip: {found:?}; the content thrown unmapped: {unmapped:?}");
    assert!(
        unmapped.mean > 3.0 * ROUND_TRIP_MEAN,
        "the test tells a map from none: {unmapped:?}"
    );
    if let Some(dir) = std::env::var_os("NEWQ_ROUND_TRIP_PNG") {
        let dir = std::path::PathBuf::from(dir);
        let _ = std::fs::create_dir_all(&dir);
        for (name, img) in [
            ("straight", &straight),
            ("lit_white", &lit_white),
            ("lit_mapped", &lit_mapped),
        ] {
            let _ = image::save_buffer(
                dir.join(format!("{name}.png")),
                img,
                VIEW.0,
                VIEW.1,
                image::ExtendedColorType::Rgba8,
            );
        }
    }
    assert!(found.compared > 3000, "{found:?}");
    assert!(found.mean <= ROUND_TRIP_MEAN, "{found:?}");
    assert!(found.p95 <= ROUND_TRIP_P95, "{found:?}");
    assert!(found.worst <= ROUND_TRIP_WORST, "{found:?}");
}

/// The content each lit pixel shows, worked back from the light (the
/// mapped throw over a white one, the room's own light taken off both,
/// undone by the projector's gamma), against the content laid straight on;
/// only where the white throw is bright and so are all its neighbours two
/// pixels round (away from the screen's and the picture's edges).
fn compare(straight: &[u8], white: &[u8], black: &[u8], mapped: &[u8], gamma: f32) -> RoundTrip {
    let (w, h) = (VIEW.0 as usize, VIEW.1 as usize);
    let lin = |v: u8| (f32::from(v) / 255.0).powf(2.2);
    let bright = |x: usize, y: usize| {
        let i = (y * w + x) * 4;
        lin(white[i + 1]) - lin(black[i + 1]) > 0.12
    };
    let mut errors = Vec::new();
    for y in 2..h - 2 {
        for x in 2..w - 2 {
            if !(y - 2..=y + 2).all(|yy| (x - 2..=x + 2).all(|xx| bright(xx, yy))) {
                continue;
            }
            let i = (y * w + x) * 4;
            let mut worst = 0.0f32;
            for c in 0..3 {
                let wl = lin(white[i + c]) - lin(black[i + c]);
                let ml = (lin(mapped[i + c]) - lin(black[i + c])).max(0.0);
                let back = (ml / wl).clamp(0.0, 1.0).powf(1.0 / gamma);
                let want = f32::from(straight[i + c]) / 255.0;
                worst = worst.max((back - want).abs());
            }
            errors.push(worst);
        }
    }
    errors.sort_by(f32::total_cmp);
    let n = errors.len();
    RoundTrip {
        compared: n,
        mean: if n == 0 {
            f32::NAN
        } else {
            errors.iter().sum::<f32>() / n as f32
        },
        p95: errors.get(n * 95 / 100).copied().unwrap_or(f32::NAN),
        worst: errors.last().copied().unwrap_or(f32::NAN),
    }
}

/// A mapped throw draws its mapped picture through slots of its own, and a
/// throw that isn't mapped makes no drawer at all (drawn as before).
#[test]
fn only_a_mapped_throw_makes_a_drawer() {
    let Some(gpu) = gpu() else { return };
    let mut drawer = StageDrawer::new(&gpu);
    let picture = plain(&gpu, 200);
    let job = StageJob {
        throws: vec![ThrowJob {
            layer: 0,
            groups: vec![group(&picture, (2, 2))],
            blend: EdgeBlend::default(),
            mapped: None,
        }],
        throw_size: (64, 36),
        views: vec![ViewJob {
            key: 1,
            display: false,
            target: Arc::new(LayerTexture::new_target(&gpu, 32, 18)),
            scene: Arc::new(Scene::default()),
            camera: Camera::default(),
            lines: Default::default(),
            options: Options {
                light: LightView::Picture,
                ..Options::default()
            },
        }],
        serial: 1,
        ..StageJob::default()
    };
    drawer.draw(&gpu, &job, &mut || {});
    assert!(drawer.mapped.is_none());
}
