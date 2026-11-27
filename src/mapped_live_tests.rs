//! The live pass on a graphics card (lavapipe in CI: `WGPU_BACKEND=vulkan`,
//! `NEWQ_REQUIRE_GPU=1` makes having none a failure). Each test draws the
//! same thing two ways: the screen baked into a map as if it stood still
//! there (`lensmap::reference` at that placement, through the map pass,
//! tested in `mapped_tests.rs`), and the screen left out of the map and
//! drawn live, over a map of what stands still with its depth (made up here
//! as the bake is to make it: `pick`'s distance along each pixel's ray).
//! The two pictures agree away from edges.

use super::gpu_tests::{H, W, gpu, layer, picture, pixel, plain, read, shown, window_target};
use super::live::{eye_of, movable_pieces, moving_for};
use super::*;
use crate::gpu::Slot;
use crate::render::OutputGroup;
use crate::render3d::{Part, Scene};
use crate::show::{CueKind, CueType};
use crate::stage3d::lensmap::{self, tests::box_part, tests::flat_show};
use crate::stage3d::{Feed, Placement, SurfaceId, SurfaceShape};

/// What depth range the made-up depth pictures are packed over.
const RANGE: (f32, f32) = (0.05, 100.0);

/// Make screen `s` movable: a screen cue names it.
fn movable(show: &mut Show, s: SurfaceId) {
    let mut c = show.make_cue(CueType::Screen, None, 0);
    if let CueKind::Screen(sc) = &mut c.kind {
        sc.screen = Some(s);
    }
    show.cues.push(c);
    assert!(show.movable(s));
}

/// The still scene of `show`, with `extra` parts (a box) first.
fn still(show: &Show, extra: &[Part]) -> Scene {
    let mut scene = lensmap::still_scene(show, &show.stage3d.scene(show, None));
    for (i, p) in extra.iter().enumerate() {
        scene.parts.insert(i, p.clone());
    }
    scene
}

/// `show` with every screen in `places` standing still there (no cue moves
/// it): what the bake would make of it.
fn standing(show: &Show, places: &[(SurfaceId, Placement)]) -> Show {
    let mut s = show.clone();
    s.cues.retain(|c| !matches!(c.kind, CueKind::Screen(_)));
    for (id, at) in places {
        s.stage3d.surface_mut(*id).unwrap().placement = *at;
    }
    s
}

/// The depth picture of `scene` from projector `p`: each pixel's distance
/// along its ray to the nearest thing, packed over `RANGE`.
fn depth_of(scene: &Scene, p: &crate::stage3d::projector::Projector) -> Vec<[u8; 4]> {
    let aspect = p.aspect((W, H));
    let mut out = Vec::new();
    for y in 0..H {
        for x in 0..W {
            let u = (x as f32 + 0.5) / W as f32;
            let v = (y as f32 + 0.5) / H as f32;
            let ray = p.ray(u, v, aspect);
            let d = crate::render3d::pick(scene, &ray, &|_, _| true)
                .map_or(f32::INFINITY, |h| h.distance);
            out.push(pack_depth(d, RANGE));
        }
    }
    out
}

/// A canvas showing `px` (a `W`×`H` picture) as display `output`.
fn canvas(gpu: &Gpu, output: OutputId, px: &[u8]) -> FeedFrame {
    let mut f = super::gpu_tests::feed(0, &layer(gpu, px));
    f.output = output;
    f
}

/// Draw `frame` by `drawer` as `owner`, then show its target as a window
/// (or a stream) of `W`×`H` does, and read that back.
fn drawn(gpu: &Gpu, drawer: &mut Option<Drawer>, owner: MapOwner, frame: &MappedFrame) -> Vec<u8> {
    let (tex, view) = window_target(gpu);
    let pipeline = gpu.compositor.pipeline(&gpu.device, FORMAT);
    let mut cmds = draw_mapped(drawer, gpu, owner, std::slice::from_ref(frame));
    let slot = match owner {
        MapOwner::Window(n) => Slot::Window(n),
        MapOwner::Capture(n) => Slot::Capture(n),
        MapOwner::Throw(_) => Slot::Stage,
    };
    let groups = [shown(frame, plain((W, H)))];
    let draw: Vec<_> = groups.iter().map(OutputGroup::draw).collect();
    cmds.push(gpu.composite(&pipeline, slot, &view, &draw, None));
    read(gpu, &tex, cmds)
}

fn frame_of(
    gpu: &Gpu,
    map: &CpuMap,
    feeds: &[FeedFrame],
    moving: Vec<MovingScreen>,
    lens: [[f32; 4]; 4],
) -> MappedFrame {
    MappedFrame {
        target: Arc::new(LayerTexture::new_target(gpu, W, H)),
        map: Arc::new(LensMap::from_cpu(gpu, map, 1)),
        feeds: feeds.to_vec(),
        moving,
        lens,
        shares: false,
        gamma: 2.2,
    }
}

/// What was compared: pixels where the baked picture shows a canvas, and
/// how many of those were compared.
struct Agreed {
    fed: usize,
    compared: usize,
}

/// The pixels of `got` and `want` agree, within `within` of a step, where
/// `map` (the baked reference) is the same feed all round and its places
/// run on smoothly (away from a screen's edge and a facet's seam).
fn agree(map: &CpuMap, want: &[u8], got: &[u8], within: i16, what: &str) -> Agreed {
    let feed = |x: u32, y: u32| unpack_how(map.how[(y * W + x) as usize]).feed;
    let place = |x: u32, y: u32| unpack_place(map.place[(y * W + x) as usize]);
    let mut a = Agreed {
        fed: 0,
        compared: 0,
    };
    for y in 0..H {
        for x in 0..W {
            if feed(x, y).is_some() {
                a.fed += 1;
            }
            let inside = (1..W - 1).contains(&x) && (1..H - 1).contains(&y);
            let smooth = inside
                && (-1i32..=1).all(|dy| {
                    (-1i32..=1).all(|dx| {
                        let (nx, ny) = ((x as i32 + dx) as u32, (y as i32 + dy) as u32);
                        let (p, q) = (place(x, y), place(nx, ny));
                        feed(nx, ny) == feed(x, y)
                            && (p[0] - q[0]).abs() < 0.1
                            && (p[1] - q[1]).abs() < 0.1
                    })
                });
            if !smooth {
                continue;
            }
            a.compared += 1;
            let (g, w) = (pixel(got, x, y), pixel(want, x, y));
            let near = (0..3).all(|k| (i16::from(g[k]) - i16::from(w[k])).abs() <= within);
            assert!(
                near,
                "{what}: pixel ({x},{y}) is {g:?} live, {w:?} baked (feed {:?})",
                feed(x, y)
            );
        }
    }
    a
}

/// Draw `show` from projector `p` both ways with the screens at `places`,
/// `extra` parts (a box) in the room, showing `feeds` (each a canvas with a
/// picture of its own); returns the baked reference and how they agreed.
fn both_ways(
    gpu: &Gpu,
    show: &Show,
    p: crate::stage3d::ProjectorId,
    places: &[(SurfaceId, Placement)],
    extra: &[Part],
    feeds: &[OutputId],
    what: &str,
) -> (CpuMap, Agreed) {
    let pr = show.stage3d.projector(p).unwrap();
    let canvases: Vec<FeedFrame> = feeds
        .iter()
        .enumerate()
        .map(|(i, o)| canvas(gpu, *o, &picture(40 + 120 * i as u8)))
        .collect();
    let lens = lensmap::lens_matrix(pr, pr.aspect((W, H)));
    // Baked: the screens standing where they are.
    let baked = lensmap::reference(&still(&standing(show, places), extra), pr, (W, H), feeds);
    let want = drawn(
        gpu,
        &mut None,
        MapOwner::Window(0),
        &frame_of(gpu, &baked, &canvases, Vec::new(), lens),
    );
    // Live: the map of what stands still, with its depth, and the movable
    // screens drawn over it.
    let room = still(show, extra);
    let mut map = lensmap::reference(&room, pr, (W, H), feeds);
    map.depth = Some(depth_of(&room, pr));
    map.depth_range = RANGE;
    let pieces = movable_pieces(
        show,
        &show.stage3d.scene(show, None),
        &show.movable_screens(),
    );
    assert!(!pieces.is_empty(), "{what}: there are movable screens");
    let moving = moving_for(show, &pieces, places, feeds, pr.position());
    let got = drawn(
        gpu,
        &mut None,
        MapOwner::Window(0),
        &frame_of(gpu, &map, &canvases, moving, lens),
    );
    let a = agree(&baked, &want, &got, 3, what);
    // (The live side's map has none of the movable screens in it.)
    for (part, shows) in room.parts.iter().map(|q| (q, lensmap::part_feed(q, feeds))) {
        if shows.is_some() {
            assert!(
                pieces
                    .iter()
                    .all(|m| m.mesh_key != part.mesh_key || !Arc::ptr_eq(&m.mesh, &part.mesh)),
                "{what}: the map leaves the moving screens out"
            );
        }
    }
    (baked, a)
}

fn enough(a: &Agreed, fed: usize, what: &str) {
    assert!(
        a.fed >= fed && a.compared >= fed / 2,
        "{what}: {} fed pixels, {} compared",
        a.fed,
        a.compared
    );
}

fn moved(at: [f32; 3], yaw: f32, roll: f32) -> Placement {
    Placement {
        position: at,
        yaw,
        pitch: 0.0,
        roll,
    }
}

#[test]
fn the_lens_is_where_its_matrix_says() {
    let (show, _, _, p) = flat_show();
    let mut pr = show.stage3d.projector(p).unwrap().clone();
    pr.placement = moved([1.5, 2.0, 7.0], 20.0, 4.0);
    pr.placement.pitch = -10.0;
    let e = eye_of(&lensmap::lens_matrix(&pr, 16.0 / 10.0)).expect("an eye");
    let want = pr.position();
    assert!(
        (0..3).all(|k| (e[k] - want[k]).abs() < 1e-3),
        "{e:?} {want:?}"
    );
    assert_eq!(eye_of(&[[0.0; 4]; 4]), None);
}

#[test]
fn a_moved_flat_curved_and_faceted_screen_is_drawn_where_the_reference_has_it() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter here; skipping the live pass test");
        return;
    };
    let shapes = [
        ("flat", None),
        (
            "curved",
            Some(SurfaceShape::Curved {
                width: 5.0,
                radius: 3.0,
                height: 2.0,
                convex: false,
            }),
        ),
        (
            "faceted",
            Some(SurfaceShape::Faceted {
                facets: crate::stage3d::geometry::angled_wall(3, 1, 1.5, 2.0, 25.0),
            }),
        ),
    ];
    for (what, shape) in shapes {
        let (mut show, a, s, p) = flat_show();
        if let Some(shape) = shape {
            show.stage3d.surface_mut(s).unwrap().shape = shape;
        }
        movable(&mut show, s);
        // Off the middle and turned: a flip or a matrix the wrong way round
        // would show.
        let places = [(s, moved([0.5, -0.3, 0.7], 14.0, 5.0))];
        let (_, agreed) = both_ways(&gpu, &show, p, &places, &[], &[a], what);
        enough(&agreed, 600, what);
        // At home, never moved: still drawn (the map leaves it out).
        let (_, agreed) = both_ways(&gpu, &show, p, &[], &[], &[a], what);
        enough(&agreed, 900, &format!("{what} at home"));
    }
}

#[test]
fn a_moving_screen_is_hidden_behind_a_baked_box() {
    let Some(gpu) = gpu() else { return };
    let (mut show, a, s, p) = flat_show();
    movable(&mut show, s);
    let places = [(s, moved([0.2, 0.1, 0.4], -8.0, 0.0))];
    let block = box_part([0.3, -0.2, 1.6], [0.6, 0.4, 0.3]);
    let (open, _) = both_ways(&gpu, &show, p, &places, &[], &[a], "no box");
    let (boxed, agreed) = both_ways(&gpu, &show, p, &places, &[block], &[a], "behind a box");
    enough(&agreed, 600, "behind a box");
    // The box hides a good part of it (compared: black on both).
    let hidden = (0..(W * H) as usize)
        .filter(|i| {
            unpack_how(open.how[*i]).feed.is_some() && unpack_how(boxed.how[*i]).feed.is_none()
        })
        .count();
    assert!(hidden > 150, "the box hides {hidden} pixels");
}

#[test]
fn a_moving_screen_in_front_of_a_static_one_covers_it_and_behind_it_is_hidden() {
    let Some(gpu) = gpu() else { return };
    let (mut show, a, _, p) = flat_show();
    let b = show.add_display();
    let small = show.add_surface(Some(SurfaceShape::Flat {
        width: 1.6,
        height: 1.0,
    }));
    show.stage3d.surface_mut(small).unwrap().feed = Feed::Display {
        output: b,
        region: [0.0, 0.0, 1.0, 1.0],
    };
    movable(&mut show, small);
    let feeds = [a, b];
    let front = [(small, moved([-0.4, 0.2, 1.0], 20.0, 0.0))];
    let (baked, agreed) = both_ways(&gpu, &show, p, &front, &[], &feeds, "in front");
    enough(&agreed, 1500, "in front");
    let of = |m: &CpuMap, f: u8| {
        (0..(W * H) as usize)
            .filter(|i| unpack_how(m.how[*i]).feed == Some(f))
            .count()
    };
    assert!(of(&baked, 1) > 150, "the small screen shows in front");
    // Behind the big one: hidden by the baked depth.
    let behind = [(small, moved([0.0, 0.0, -1.0], 0.0, 0.0))];
    let (baked, agreed) = both_ways(&gpu, &show, p, &behind, &[], &feeds, "behind");
    enough(&agreed, 1500, "behind");
    assert_eq!(of(&baked, 1), 0);
}

#[test]
fn a_front_screen_turned_away_is_black_and_in_the_way_and_a_rear_one_shows() {
    let Some(gpu) = gpu() else { return };
    let (mut show, a, _, p) = flat_show();
    let b = show.add_display();
    let small = show.add_surface(Some(SurfaceShape::Flat {
        width: 1.6,
        height: 1.0,
    }));
    show.stage3d.surface_mut(small).unwrap().feed = Feed::Display {
        output: b,
        region: [0.0, 0.0, 1.0, 1.0],
    };
    movable(&mut show, small);
    let feeds = [a, b];
    let away = [(small, moved([0.3, 0.0, 1.0], 160.0, 0.0))];
    let (baked, agreed) = both_ways(&gpu, &show, p, &away, &[], &feeds, "turned away");
    enough(&agreed, 1500, "turned away");
    let none = (0..(W * H) as usize)
        .filter(|i| unpack_how(baked.how[*i]).feed.is_none())
        .count();
    assert!(none > 150, "its back is black over the big screen ({none})");
    show.stage3d.surface_mut(small).unwrap().rear = true;
    let (baked, agreed) = both_ways(&gpu, &show, p, &away, &[], &feeds, "rear");
    enough(&agreed, 1500, "rear");
    let shows = (0..(W * H) as usize)
        .filter(|i| unpack_how(baked.how[*i]).feed == Some(1))
        .count();
    assert!(shows > 150, "a rear screen shows its picture ({shows})");
}

#[test]
fn two_projectors_built_from_one_tick_put_the_screen_in_the_same_place() {
    let Some(gpu) = gpu() else { return };
    let (mut show, a, s, p) = flat_show();
    movable(&mut show, s);
    let q = show.add_projector();
    let first = show.stage3d.projector(p).unwrap().clone();
    let pq = show.projector_mut(q).unwrap();
    pq.lens = first.lens.clone();
    pq.placement = moved([1.2, 0.4, first.placement.position[2] * 1.1], 9.0, 0.0);
    let places = [(s, moved([0.3, 0.1, 0.5], 10.0, 2.0))];
    let pieces = movable_pieces(
        &show,
        &show.stage3d.scene(&show, None),
        &show.movable_screens(),
    );
    let lists: Vec<Vec<MovingScreen>> = [p, q]
        .iter()
        .map(|id| {
            let pr = show.stage3d.projector(*id).unwrap();
            moving_for(&show, &pieces, &places, &[a], pr.position())
        })
        .collect();
    assert_eq!(
        moving_shots(&lists[0]),
        moving_shots(&lists[1]),
        "the same place on both"
    );
    for (id, what) in [(p, "first"), (q, "second")] {
        let (_, agreed) = both_ways(&gpu, &show, id, &places, &[], &[a], what);
        enough(&agreed, 400, what);
    }
}

#[test]
fn sixteen_moving_screens_are_drawn_far_to_near() {
    let Some(gpu) = gpu() else { return };
    let (mut show, a, big, p) = flat_show();
    let b = show.add_display();
    let mut places = Vec::new();
    for i in 0..16 {
        let s = show.add_surface(Some(SurfaceShape::Flat {
            width: 0.7,
            height: 0.45,
        }));
        show.stage3d.surface_mut(s).unwrap().feed = Feed::Display {
            output: if i % 2 == 0 { a } else { b },
            region: [(i % 4) as f32 / 4.0, (i / 4) as f32 / 4.0, 0.25, 0.25],
        };
        movable(&mut show, s);
        // A grid in front of the big screen, each a little nearer than the
        // last, overlapping its neighbour.
        let (cx, cy) = ((i % 4) as f32 * 0.8 - 1.2, (i / 4) as f32 * 0.5 - 0.75);
        places.push((
            s,
            moved([cx, cy, 0.3 + 0.05 * i as f32], 6.0 * (i % 3) as f32, 0.0),
        ));
    }
    movable(&mut show, big);
    places.push((big, moved([0.0, 0.0, -0.5], 0.0, 0.0)));
    let pieces = movable_pieces(
        &show,
        &show.stage3d.scene(&show, None),
        &show.movable_screens(),
    );
    assert_eq!(pieces.len(), 17);
    let pr = show.stage3d.projector(p).unwrap();
    let list = moving_for(&show, &pieces, &places, &[a, b], pr.position());
    assert_eq!(list[0].surface, big, "the farthest first");
    let (_, agreed) = both_ways(&gpu, &show, p, &places, &[], &[a, b], "sixteen");
    enough(&agreed, 1500, "sixteen");
}

#[test]
fn with_no_moving_screens_the_picture_is_what_it_was_byte_for_byte() {
    let Some(gpu) = gpu() else { return };
    let (mut show, a, s, p) = flat_show();
    let b = show.add_display();
    let small = show.add_surface(Some(SurfaceShape::Flat {
        width: 1.6,
        height: 1.0,
    }));
    show.stage3d.surface_mut(small).unwrap().feed = Feed::Display {
        output: b,
        region: [0.0, 0.0, 1.0, 1.0],
    };
    movable(&mut show, small);
    let _ = s;
    let pr = show.stage3d.projector(p).unwrap();
    let lens = lensmap::lens_matrix(pr, pr.aspect((W, H)));
    let feeds = [a, b];
    let canvases = [
        canvas(&gpu, a, &picture(40)),
        canvas(&gpu, b, &picture(160)),
    ];
    let room = still(&show, &[]);
    let mut map = lensmap::reference(&room, pr, (W, H), &feeds);
    map.depth = Some(depth_of(&room, pr));
    map.depth_range = RANGE;
    let plain = frame_of(&gpu, &map, &canvases, Vec::new(), lens);
    // Before: a drawer that never saw a moving screen.
    let before = drawn(&gpu, &mut None, MapOwner::Window(0), &plain);
    // A moving screen wholly behind the lens draws nothing.
    let pieces = movable_pieces(
        &show,
        &show.stage3d.scene(&show, None),
        &show.movable_screens(),
    );
    let behind = [(small, moved([0.0, 0.0, 40.0], 0.0, 0.0))];
    let gone = moving_for(&show, &pieces, &behind, &feeds, pr.position());
    let mut with = plain.clone();
    with.moving = gone;
    let mut drawer = None;
    assert_eq!(drawn(&gpu, &mut drawer, MapOwner::Window(0), &with), before);
    // One in sight changes the picture; and then none again on the same
    // drawer (its live pass made and kept) is the picture of before.
    let mut seen = plain.clone();
    seen.moving = moving_for(
        &show,
        &pieces,
        &[(small, moved([0.0, 0.0, 1.0], 0.0, 0.0))],
        &feeds,
        pr.position(),
    );
    assert_ne!(drawn(&gpu, &mut drawer, MapOwner::Window(0), &seen), before);
    assert_eq!(
        drawn(&gpu, &mut drawer, MapOwner::Window(0), &plain),
        before
    );
    // As a stream draws it (its own owner's slots): the same picture.
    let as_window = drawn(&gpu, &mut drawer, MapOwner::Window(0), &seen);
    assert_eq!(
        drawn(&gpu, &mut None, MapOwner::Capture(0), &seen),
        as_window
    );
}

#[test]
fn a_movable_screens_pieces_are_its_own_and_led_shows_nothing() {
    let (mut show, a, s, _) = flat_show();
    let facets = crate::stage3d::geometry::angled_wall(3, 1, 1.5, 2.0, 25.0);
    show.stage3d.surface_mut(s).unwrap().shape = SurfaceShape::Faceted { facets };
    show.stage3d.surface_mut(s).unwrap().placement = moved([1.0, 2.0, 3.0], 30.0, 0.0);
    // (Out of the beam at home: said to be lit by projectors, not worked out.)
    show.stage3d.surface_mut(s).unwrap().kind = Some(crate::stage3d::SurfaceKind::Projection);
    let other = show.add_surface(None);
    movable(&mut show, s);
    let scene = show.stage3d.scene(&show, None);
    let pieces = movable_pieces(&show, &scene, &show.movable_screens());
    assert_eq!(pieces.len(), 3, "one per facet, not the other screen");
    assert!(
        pieces
            .iter()
            .all(|q| q.surface == s && q.shows.is_some_and(|(o, _)| o == a))
    );
    // At home, each piece is where the scene has it.
    let at_home = moving_for(&show, &pieces, &[], &[a], [0.0, 0.0, 10.0]);
    for m in &at_home {
        let part = scene.parts.iter().find(|q| {
            q.mesh_key == m.mesh_key && {
                (0..4).all(|c| (0..4).all(|r| (q.transform[c][r] - m.matrix[c][r]).abs() < 1e-4))
            }
        });
        assert!(part.is_some(), "a piece at home is the scene's part");
        assert_eq!(m.feed, Some(0));
    }
    // A canvas the map doesn't name, or an LED screen: black.
    assert!(
        moving_for(&show, &pieces, &[], &[], [0.0; 3])
            .iter()
            .all(|m| m.feed.is_none())
    );
    show.stage3d.surface_mut(s).unwrap().kind = Some(crate::stage3d::SurfaceKind::Led);
    let scene = show.stage3d.scene(&show, None);
    let pieces = movable_pieces(&show, &scene, &show.movable_screens());
    assert!(
        moving_for(&show, &pieces, &[], &[a], [0.0; 3])
            .iter()
            .all(|m| m.feed.is_none())
    );
    let _ = other;
}

/// The live pass goes through naga's HLSL and MSL writers, as wgpu does on
/// Windows and the Mac.
#[test]
fn the_live_pass_shader_is_written_for_direct3d_and_metal() {
    let source = live::live_shader_source();
    let module = naga::front::wgsl::parse_str(&source).expect("the live pass's shader reads");
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .expect("the live pass's shader is valid");
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
