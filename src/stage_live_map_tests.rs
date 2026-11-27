//! Lens maps as the app keeps them (`StageLive::lens_map`): baked on the
//! stage thread for real (lavapipe under `WGPU_BACKEND=vulkan`; skipped with
//! no GPU adapter unless `NEWQ_REQUIRE_GPU` is set).

use super::*;
use crate::stage3d::lensmap::tests::flat_show;

/// Long enough for anything to happen.
const LONG: Duration = Duration::from_secs(20);

type Ask = (ProjectorId, (u32, u32), MapUse);

/// One pass of the app's as far as the lens maps go: `begin`, the asks,
/// `send` with nothing else to draw but something playing (`fresh`, every
/// display's pictures given): nothing a cue changes may bake; what each ask
/// was given.
fn map_pass(
    live: &mut StageLive,
    gpu: &Gpu,
    show: &Show,
    shared: &mut StageShared,
    asks: &[Ask],
) -> Vec<Option<Arc<LensMap>>> {
    live.begin(show, shared, &mut Vec::new());
    let maps = asks
        .iter()
        .map(|&(p, size, use_)| live.lens_map(p, size, use_))
        .collect();
    let pictures: HashMap<OutputId, PictureLayers> = show
        .outputs
        .iter()
        .map(|o| {
            (
                o.id,
                PictureLayers {
                    layers: Vec::new(),
                    canvas: (1920, 1080),
                },
            )
        })
        .collect();
    live.send(gpu, show, shared, &[], &pictures, &[], &[], true);
    maps
}

/// Passes until `done` says yes to what the asks were given (at most
/// `LONG`); the last answers.
fn passes_until(
    live: &mut StageLive,
    gpu: &Gpu,
    show: &Show,
    shared: &mut StageShared,
    asks: &[Ask],
    done: impl Fn(&[Option<Arc<LensMap>>]) -> bool,
) -> Vec<Option<Arc<LensMap>>> {
    let end = Instant::now() + LONG;
    loop {
        let maps = map_pass(live, gpu, show, shared, asks);
        if done(&maps) || Instant::now() > end {
            return maps;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn all_there(maps: &[Option<Arc<LensMap>>]) -> bool {
    maps.iter().all(Option::is_some)
}

/// Passes until nothing is in hand: every map asked for is up to date and
/// the thread has drawn all it was given (or the time is up).
fn passes_still(
    live: &mut StageLive,
    gpu: &Gpu,
    show: &Show,
    shared: &mut StageShared,
    asks: &[Ask],
) -> Vec<Option<Arc<LensMap>>> {
    let end = Instant::now() + LONG;
    loop {
        let maps = map_pass(live, gpu, show, shared, asks);
        if (!live.busy() && !live.maps_due(true).1) || Instant::now() > end {
            return maps;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn map_gpu() -> Option<Gpu> {
    let gpu = Gpu::headless();
    if gpu.is_none() {
        assert!(std::env::var_os("NEWQ_REQUIRE_GPU").is_none(), "no GPU");
    }
    gpu
}

fn same(a: &Option<Arc<LensMap>>, b: &Arc<LensMap>) -> bool {
    a.as_ref().is_some_and(|a| Arc::ptr_eq(a, b))
}

#[test]
fn a_bake_wanted_starts_the_thread_and_its_map_outlives_the_idle_release() {
    let Some(gpu) = map_gpu() else { return };
    let (show, a, _, p) = flat_show();
    let mut live = StageLive::default();
    let count = crate::stage_render::Counters::for_test();
    live.count_into(count);
    let mut shared = StageShared::default();
    let asks = [(p, (64, 36), MapUse::Window)];
    // Asked for: black until it is made, and the thread started for it
    // though no view wants it.
    let first = map_pass(&mut live, &gpu, &show, &mut shared, &asks);
    assert!(first[0].is_none(), "black until the first map");
    assert!(live.has_thread());
    assert!(live.feeds_output(a), "its canvas is output work already");
    let maps = passes_until(&mut live, &gpu, &show, &mut shared, &asks, all_there);
    let map = maps[0].clone().expect("baked");
    assert_eq!(map.size, (64, 36));
    assert_eq!(map.feeds, vec![a]);
    assert!(live.landed(), "the windows are drawn again");
    // Up to date and asked for every pass: the thread still idles out, and
    // the map stays.
    passes_still(&mut live, &gpu, &show, &mut shared, &asks);
    live.as_if_idle();
    let after = map_pass(&mut live, &gpu, &show, &mut shared, &asks);
    assert!(!live.has_thread(), "let go");
    assert!(same(&after[0], &map));
    assert_eq!(live.wake_at(), None, "nothing to come back for");
    let t = count.0.totals();
    assert_eq!((t.maps_baked, t.idle_releases), (1, 1), "{t:?}");
    // Not asked for a while: let go of too.
    for e in live.maps.values_mut() {
        e.asked_at = Instant::now().checked_sub(MAP_KEPT);
    }
    live.send(
        &gpu,
        &show,
        &mut shared,
        &[],
        &HashMap::new(),
        &[],
        &[],
        false,
    );
    assert!(live.maps.is_empty());
    assert!(!live.feeds_output(a));
}

#[test]
fn a_still_scene_bakes_once_and_a_fine_tune_or_a_line_up_bakes_nothing() {
    let Some(gpu) = map_gpu() else { return };
    let (mut show, _, _, p) = flat_show();
    let mut live = StageLive::default();
    let count = crate::stage_render::Counters::for_test();
    live.count_into(count);
    let mut shared = StageShared::default();
    let asks = [(p, (64, 36), MapUse::Window), (p, (32, 18), MapUse::Stream)];
    passes_until(&mut live, &gpu, &show, &mut shared, &asks, all_there);
    let maps = passes_still(&mut live, &gpu, &show, &mut shared, &asks);
    let (window, stream) = (maps[0].clone().unwrap(), maps[1].clone().unwrap());
    for _ in 0..20 {
        let again = map_pass(&mut live, &gpu, &show, &mut shared, &asks);
        assert!(same(&again[0], &window) && same(&again[1], &stream));
    }
    // A fine-tune dragged, a line-up point placed: no new scene, no bake.
    let scene = shared.scene.clone();
    let pr = show.projector_mut(p).unwrap();
    pr.fine = Some(crate::mesh::Mesh::grid(3, 3));
    pr.line_up = Some(Default::default());
    let again = passes_still(&mut live, &gpu, &show, &mut shared, &asks);
    assert!(same(&again[0], &window));
    assert!(Arc::ptr_eq(&shared.scene, &scene), "no new scene");
    let t = count.0.totals();
    assert_eq!(t.maps_baked, 2, "one each size, once: {t:?}");
    if let Some(h) = live.forget_device(&mut shared) {
        h.join().unwrap();
    }
}

#[test]
fn a_moved_projector_re_bakes_its_own_map_and_no_other() {
    let Some(gpu) = map_gpu() else { return };
    let (mut show, _, _, p) = flat_show();
    let q = show.add_projector();
    let d = show.projector(p).unwrap().placement.position[2];
    show.projector_mut(q).unwrap().placement.position = [0.5, 0.2, d * 1.2];
    let mut live = StageLive::default();
    let count = crate::stage_render::Counters::for_test();
    live.count_into(count);
    let mut shared = StageShared::default();
    let asks = [(p, (64, 36), MapUse::Window), (q, (64, 36), MapUse::Window)];
    passes_until(&mut live, &gpu, &show, &mut shared, &asks, all_there);
    let maps = passes_still(&mut live, &gpu, &show, &mut shared, &asks);
    let (mp, mq) = (maps[0].clone().unwrap(), maps[1].clone().unwrap());
    // P moves (its body moves in the scene too): its old map is used until
    // the new one lands.
    show.projector_mut(p).unwrap().placement.position[0] = 0.3;
    let next = map_pass(&mut live, &gpu, &show, &mut shared, &asks);
    assert!(next[0].is_some(), "the old map meanwhile, never black");
    passes_until(&mut live, &gpu, &show, &mut shared, &asks, |m| {
        !same(&m[0], &mp)
    });
    let still = passes_still(&mut live, &gpu, &show, &mut shared, &asks);
    assert!(!same(&still[0], &mp), "P's baked again");
    assert!(same(&still[1], &mq), "Q's kept");
    assert_eq!(count.0.totals().maps_baked, 3);
    if let Some(h) = live.forget_device(&mut shared) {
        h.join().unwrap();
    }
}

#[test]
fn a_3d_views_map_waits_for_a_lit_view_the_governor_lets_draw() {
    let Some(gpu) = map_gpu() else { return };
    let (show, a, _, p) = flat_show();
    let mut live = StageLive::default();
    let mut shared = StageShared::default();
    let asks = [(p, (64, 36), MapUse::Preview)];
    for _ in 0..3 {
        let maps = map_pass(&mut live, &gpu, &show, &mut shared, &asks);
        assert!(maps[0].is_none());
    }
    assert!(!live.has_thread(), "no lit view drawn: no bake, no thread");
    assert!(
        !live.feeds_output(a),
        "a preview's canvas isn't output work"
    );
    // A lit view the governor let draw: baked with it.
    let end = Instant::now() + LONG;
    let mut map = None;
    while map.is_none() && Instant::now() < end {
        live.begin(&show, &mut shared, &mut Vec::new());
        map = live.lens_map_shaped(p, (64, 36), 16.0 / 9.0, MapUse::Preview);
        shared.requests.push(crate::ui::stage_view::ViewRequest {
            id: PreviewId::Stage(1),
            size: (64, 36),
            camera: Camera::default(),
            options: Options {
                light: crate::render3d::LightView::Picture,
                ..Options::default()
            },
            lines: Default::default(),
        });
        live.send(
            &gpu,
            &show,
            &mut shared,
            &[],
            &HashMap::new(),
            &[],
            &[],
            false,
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(map.is_some());
    if let Some(h) = live.forget_device(&mut shared) {
        h.join().unwrap();
    }
}

#[test]
fn a_lost_device_forgets_the_maps_and_they_are_made_again() {
    let Some(gpu) = map_gpu() else { return };
    let (show, _, _, p) = flat_show();
    let mut live = StageLive::default();
    let count = crate::stage_render::Counters::for_test();
    live.count_into(count);
    let mut shared = StageShared::default();
    let asks = [(p, (64, 36), MapUse::Window)];
    passes_until(&mut live, &gpu, &show, &mut shared, &asks, all_there);
    if let Some(h) = live.forget_device(&mut shared) {
        h.join().unwrap();
    }
    live.begin(&show, &mut shared, &mut Vec::new());
    assert!(live.lens_map(p, (64, 36), MapUse::Window).is_none());
    let maps = passes_until(&mut live, &gpu, &show, &mut shared, &asks, all_there);
    assert!(maps[0].is_some());
    assert_eq!(count.0.totals().maps_baked, 2);
    if let Some(h) = live.forget_device(&mut shared) {
        h.join().unwrap();
    }
}

#[test]
fn a_venue_that_lands_later_is_baked_into_the_map_again() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static COME: AtomicBool = AtomicBool::new(false);
    fn store(p: &std::path::Path) -> Found {
        if p.extension().is_some_and(|e| e == "obj") && !COME.load(Ordering::SeqCst) {
            Found::Arriving
        } else {
            Found::Here(p.to_path_buf())
        }
    }
    let Some(gpu) = map_gpu() else { return };
    let dir = std::env::temp_dir().join(format!("newq-stage-live-map-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("room.obj");
    // A wall across the screen's top half, a metre in front of it.
    std::fs::write(
        &path,
        "v -5 0 1\nv 5 0 1\nv 5 5 1\nv -5 5 1\nf 1 2 3\nf 1 3 4\n",
    )
    .unwrap();
    let (mut show, _, _, p) = flat_show();
    show.stage3d.venue = Some(Venue::new(path.clone()));
    let mut live = StageLive::default();
    live.locate_with(store);
    let mut shared = StageShared::default();
    let asks = [(p, (32, 18), MapUse::Window)];
    // A follower without its venue yet: baked without it.
    let maps = passes_until(&mut live, &gpu, &show, &mut shared, &asks, all_there);
    let first = maps[0].clone().unwrap();
    assert_eq!(shared.venue, VenueStatus::Arriving("room.obj".into()));
    COME.store(true, Ordering::SeqCst);
    std::thread::sleep(ARRIVING_LOOK);
    let maps = passes_until(&mut live, &gpu, &show, &mut shared, &asks, |m| {
        !same(&m[0], &first)
    });
    let again = maps[0].clone().unwrap();
    assert!(!Arc::ptr_eq(&again, &first), "baked again with the venue");
    assert_ne!(again.key, first.key);
    if let Some(h) = live.forget_device(&mut shared) {
        h.join().unwrap();
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_map_given_by_a_test_is_asked_for_as_any_other_and_never_baked() {
    let Some(gpu) = map_gpu() else { return };
    let (show, a, _, p) = flat_show();
    let mut live = StageLive::default();
    let count = crate::stage_render::Counters::for_test();
    live.count_into(count);
    let mut shared = StageShared::default();
    let mut made = crate::mapped::CpuMap::black((16, 9));
    made.feeds = vec![a];
    let given = Arc::new(LensMap::from_cpu(&gpu, &made, 7));
    live.give_lens_map(p, given.clone());
    let asks = [(p, (16, 9), MapUse::Window)];
    for _ in 0..3 {
        let maps = map_pass(&mut live, &gpu, &show, &mut shared, &asks);
        assert!(same(&maps[0], &given));
    }
    assert!(live.feeds_output(a), "its canvas is output work");
    assert!(!live.has_thread());
    assert_eq!(count.0.totals().maps_baked, 0);
}

#[test]
fn a_stream_and_a_views_throw_of_one_size_and_two_shapes_are_two_maps() {
    let (show, _, _, p) = flat_show();
    let mut live = StageLive::default();
    let mut shared = StageShared::default();
    live.begin(&show, &mut shared, &mut Vec::new());
    assert!(live.lens_map(p, (64, 36), MapUse::Stream).is_none());
    assert!(
        live.lens_map_shaped(p, (64, 36), 1.6, MapUse::Preview)
            .is_none()
    );
    assert_eq!(live.maps.len(), 2);
    let wants: HashSet<u64> = live
        .maps
        .values()
        .filter_map(|e| e.want)
        .map(|w| w.0)
        .collect();
    assert_eq!(wants.len(), 2, "two keys");
    if let Some(h) = live.forget_device(&mut shared) {
        h.join().unwrap();
    }
}

#[test]
fn a_refused_bake_is_said_once_and_not_baked_again() {
    let (show, _, _, p) = flat_show();
    let mut live = StageLive::default();
    let mut shared = StageShared::default();
    live.begin(&show, &mut shared, &mut Vec::new());
    assert!(live.lens_map(p, (16, 9), MapUse::Window).is_none());
    let (jobs, due) = live.maps_due(false);
    assert!(due);
    let job = &jobs[0];
    // The card refused it (as `StageDrawer::bake` lands a refusal).
    live.baked.lock().push(crate::stage_render::Baked {
        projector: p,
        size: job.size,
        aspect: job.aspect,
        key: job.key,
        map: None,
    });
    let mut said = Vec::new();
    live.begin(&show, &mut shared, &mut said);
    assert_eq!(said.len(), 1);
    assert!(matches!(&said[0], Said::Problem(w) if w.contains("stays black")));
    assert!(live.lens_map(p, (16, 9), MapUse::Window).is_none(), "black");
    let (jobs, due) = live.maps_due(false);
    assert!(jobs.is_empty() && !due, "not baked again");
    let mut said = Vec::new();
    live.begin(&show, &mut shared, &mut said);
    assert!(said.is_empty(), "said once");
}

/// The canvases a baked map names are exactly what the model says the
/// projector maps (`Show::mapped_feeds(p).used`, its row on the page), and
/// what `feeds_output` keeps at an output's pace: with an LED screen in
/// the beam showing a canvas of its own, five canvases (one past the most),
/// and a window of another shape than the projector's own `resolution`.
#[test]
fn a_baked_maps_canvases_are_the_ones_the_model_says_it_maps() {
    use crate::stage3d::{Feed, SurfaceKind, SurfaceShape};
    let Some(gpu) = map_gpu() else { return };
    let (mut show, a, s, p) = flat_show();
    show.stage3d.surface_mut(s).unwrap().placement.position = [-1.5, 0.0, 0.0];
    show.projector_mut(p).unwrap().resolution = Some((1920, 1200));
    // An LED screen in front (first of the rest), showing a canvas no other
    // screen shows.
    let led_canvas = show.add_display();
    let led = show.add_surface(Some(SurfaceShape::Flat {
        width: 0.5,
        height: 0.5,
    }));
    let l = show.stage3d.surface_mut(led).unwrap();
    l.placement.position = [1.2, -0.6, 0.3];
    l.feed = Feed::Display {
        output: led_canvas,
        region: [0.0, 0.0, 1.0, 1.0],
    };
    l.kind = Some(SurfaceKind::Led);
    // Four more screens side by side across the picture, a canvas each.
    let mut canvases = vec![a];
    for i in 0..4 {
        let c = show.add_display();
        let id = show.add_surface(Some(SurfaceShape::Flat {
            width: 0.5,
            height: 0.5,
        }));
        let sf = show.stage3d.surface_mut(id).unwrap();
        sf.placement.position = [-0.6 + i as f32 * 0.6, 0.6, 0.2];
        sf.feed = Feed::Display {
            output: c,
            region: [0.0, 0.0, 1.0, 1.0],
        };
        canvases.push(c);
    }
    let model = show.mapped_feeds(p);
    assert_eq!(model.used, canvases[..4].to_vec(), "{model:?}");
    assert_eq!(model.left_out, vec![canvases[4]]);
    let mut live = StageLive::default();
    let mut shared = StageShared::default();
    // A 4:3 window, not the 16:10 of the projector's own pixels.
    let asks = [(p, (64, 48), MapUse::Window)];
    let maps = passes_until(&mut live, &gpu, &show, &mut shared, &asks, all_there);
    let map = maps[0].clone().expect("baked");
    assert_eq!(map.feeds, model.used);
    for c in &model.used {
        assert!(live.feeds_output(*c), "{c:?} is output work");
    }
    assert!(
        !live.feeds_output(canvases[4]),
        "left out: not drawn for it"
    );
    assert!(!live.feeds_output(led_canvas), "an LED screen is no feed");
    // Chosen as projection, the LED screen's canvas would be a feed: the
    // map's key changes with what the model says.
    let key = map.key;
    show.stage3d.surface_mut(led).unwrap().kind = Some(SurfaceKind::Projection);
    let maps = passes_until(&mut live, &gpu, &show, &mut shared, &asks, |m| {
        m[0].as_ref().is_some_and(|m| m.key != key)
    });
    assert_eq!(maps[0].as_ref().unwrap().feeds, show.mapped_feeds(p).used);
}

/// A GO of a screen cue bakes nothing (mapping plan 4.2): making a screen
/// movable is a show edit and re-bakes the map once, with its depth; then
/// the cue plays, the engine moves the screen, every pass is handed where it
/// is, and the bake counter doesn't move, nor is a new scene made.
#[test]
fn a_go_of_a_screen_cue_bakes_nothing() {
    let Some(gpu) = map_gpu() else { return };
    let (show, a, _, m, p) = crate::stage3d::lensmap::tests::movable_show();
    let mut plain = show.clone();
    plain.cues.clear();
    let mut live = StageLive::default();
    let count = crate::stage_render::Counters::for_test();
    live.count_into(count);
    let mut shared = StageShared::default();
    let asks = [(p, (64, 36), MapUse::Window)];
    // Without the cue: a map with no depth.
    let maps = passes_until(&mut live, &gpu, &plain, &mut shared, &asks, all_there);
    let first = maps[0].clone().expect("baked");
    assert!(first.depth.is_none());
    passes_still(&mut live, &gpu, &plain, &mut shared, &asks);
    assert_eq!(count.0.totals().maps_baked, 1);
    // The cue added: baked once more, with depth, the same canvas.
    let maps = passes_until(&mut live, &gpu, &show, &mut shared, &asks, |m| {
        m[0].as_ref().is_some_and(|m| !Arc::ptr_eq(m, &first))
    });
    let map = maps[0].clone().expect("baked again");
    assert!(map.depth.is_some(), "with its depth");
    assert_eq!(map.depth_range, crate::stage3d::lensmap::DEPTH_RANGE);
    assert_eq!(map.feeds, vec![a]);
    passes_still(&mut live, &gpu, &show, &mut shared, &asks);
    let baked = count.0.totals().maps_baked;
    assert_eq!(baked, 2, "once for the edit");
    let (scene, still) = (live.scene.clone(), live.still.clone());
    // GO, and the cue plays out on the engine's clock.
    let mut engine = crate::engine::Engine::for_tests();
    let cue = show.cues[0].id;
    engine.fire(&show, cue);
    let mut moved_once = false;
    for _ in 0..40 {
        engine.tick(&show, 0.25);
        let at = engine.moved_screens();
        moved_once |= at.iter().any(|(s, _)| *s == m);
        live.begin(&show, &mut shared, &mut Vec::new());
        let now = live.lens_map(p, (64, 36), MapUse::Window);
        assert!(
            now.as_ref().is_some_and(|n| Arc::ptr_eq(n, &map)),
            "the same map"
        );
        live.send(
            &gpu,
            &show,
            &mut shared,
            &[],
            &HashMap::new(),
            &[],
            &at,
            true,
        );
    }
    assert!(moved_once, "the engine moved the screen");
    assert!(
        engine.screen_at(m).is_some(),
        "and it stays where the cue left it"
    );
    passes_still(&mut live, &gpu, &show, &mut shared, &asks);
    assert_eq!(count.0.totals().maps_baked, baked, "a GO bakes nothing");
    assert!(Arc::ptr_eq(&live.scene, &scene) && Arc::ptr_eq(&live.still, &still));
}
