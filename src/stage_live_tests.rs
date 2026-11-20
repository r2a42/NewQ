use super::*;
use crate::show::Corners;
use crate::stage3d::{Feed, StageSource, WHOLE};

fn temp_dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("newq-stage-live-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn a_target_is_never_bigger_than_the_card_allows() {
    assert_eq!(capped((1920, 1080)), (1920, 1080));
    assert_eq!(capped((8192, 2160)), (4096, 1080));
    assert_eq!(capped((0, 0)), (1, 1));
}

#[test]
fn the_pictures_a_stage_needs_include_whole_pictures_and_sub_displays() {
    let mut show = Show::default();
    let a = show.outputs[0].id;
    let sub = show.add_sub_display(a);
    let (wide, left, _right) = show.add_blended_pair(1920, 1080, 0.1);
    let s1 = show.add_surface(None);
    let s2 = show.add_surface(None);
    show.stage3d.surface_mut(s1).unwrap().feed = Feed::Display {
        output: a,
        region: WHOLE,
    };
    show.stage3d.surface_mut(s2).unwrap().feed = Feed::Display {
        output: left,
        region: WHOLE,
    };
    let scene = show.stage3d.scene(&show, None);
    let wanted = feeds_wanted(&show, &scene);
    for o in [a, sub, left, wide] {
        assert!(wanted.contains(&o), "{o:?}");
    }
    // The projector's screen is drawn from the whole picture's layers, with
    // nothing of its own keystone; the sub display in its area.
    let layer = |canvas| PictureLayers {
        layers: Vec::new(),
        canvas,
    };
    let mut pictures = HashMap::new();
    pictures.insert(wide, layer((3456, 1080)));
    pictures.insert(a, layer((1920, 1080)));
    pictures.insert(sub, layer((960, 540)));
    let (groups, canvas) = feed_groups(&show, left, &pictures).unwrap();
    assert_eq!(canvas, (3456, 1080));
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].geometry.warp, crate::gpu::IDENTITY);
    let (groups, _) = feed_groups(&show, a, &pictures).unwrap();
    assert_eq!(groups.len(), 2, "the display and its sub display");
    let area = show.output(sub).unwrap().area;
    assert_eq!(groups[1].geometry.warp, crate::gpu::corner_pin(&area.0));
    let _ = Corners::default();
    // A display that isn't drawn here gives no picture (the screen is dark).
    pictures.remove(&wide);
    assert!(feed_groups(&show, left, &pictures).is_none());
    let jobs = feed_jobs(&show, &scene, &pictures, 1.0);
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].output, a);
    assert_eq!(jobs[0].size, (1024, 576));
}

/// Run `begin` until the venue settles (or `limit` passes).
fn settle(live: &mut StageLive, show: &Show, shared: &mut StageShared) -> Vec<String> {
    let mut words = Vec::new();
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let mut said = Vec::new();
        live.begin(show, shared, &mut said);
        words.extend(said.into_iter().map(|s| match s {
            Said::Note(m) | Said::Problem(m) => m,
        }));
        if !live.venue_busy() || Instant::now() > end {
            return words;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn the_venue_loads_on_a_thread_and_again_only_when_it_must() {
    let dir = temp_dir("venue");
    let path = dir.join("room.obj");
    std::fs::write(
        &path,
        "v -5 0 -5\nv 5 0 -5\nv 5 0 5\nv -5 0 5\nf 1 2 3\nf 1 3 4\n",
    )
    .unwrap();
    let mut show = Show::default();
    show.add_surface(None);
    show.stage3d.venue = Some(Venue::new(path.clone()));
    let mut live = StageLive::default();
    let mut shared = StageShared::default();
    let mut said = Vec::new();
    live.begin(&show, &mut shared, &mut said);
    assert!(
        matches!(shared.venue, VenueStatus::Loading(_)),
        "{:?}",
        shared.venue
    );
    let words = settle(&mut live, &show, &mut shared);
    assert!(
        words.iter().any(|w| w.starts_with("Venue loaded")),
        "{words:?}"
    );
    assert!(
        matches!(shared.venue, VenueStatus::Loaded { triangles: 2, .. }),
        "{:?}",
        shared.venue
    );
    // The floor is in the scene, lit, before the screen.
    assert!(shared.scene.parts[0].lit);
    assert_eq!(shared.scene.parts.iter().filter(|p| !p.lit).count(), 1);
    // The same stage: the scene isn't made again (nor its key worked out).
    let scene = shared.scene.clone();
    live.begin(&show, &mut shared, &mut said);
    assert!(Arc::ptr_eq(&scene, &shared.scene));
    // Framed by the venue's box and the screen's corners.
    let b = shared.bounds.unwrap();
    assert_eq!((b.min, b.max), ([-5.0, -1.125, -5.0], [5.0, 1.125, 5.0]));
    let key = shared.scene_key;
    // Moving it needs no reading again, only a new scene.
    show.stage3d.venue.as_mut().unwrap().placement.position[0] = 1.0;
    live.begin(&show, &mut shared, &mut said);
    assert!(!live.venue_busy());
    assert_ne!(shared.scene_key, key);
    // A brightness change waits for the slider to rest, then reads it again.
    show.stage3d.venue.as_mut().unwrap().brightness = 0.5;
    live.begin(&show, &mut shared, &mut said);
    assert!(live.venue_busy());
    assert!(live.venue.loading.is_none(), "not at once");
    settle(&mut live, &show, &mut shared);
    assert!(matches!(shared.venue, VenueStatus::Loaded { .. }));
    // A file that can't be read says so once, and isn't tried every pass.
    show.stage3d.venue = Some(Venue::new(dir.join("missing.glb")));
    let words = settle(&mut live, &show, &mut shared);
    assert!(
        matches!(shared.venue, VenueStatus::Failed(_)),
        "{:?}",
        shared.venue
    );
    assert_eq!(words.len(), 1, "{words:?}");
    let mut again = Vec::new();
    live.begin(&show, &mut shared, &mut again);
    assert!(again.is_empty() && !live.venue_busy());
    // Without a venue: none.
    show.stage3d.venue = None;
    live.begin(&show, &mut shared, &mut again);
    assert_eq!(shared.venue, VenueStatus::None);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A display that shows the stage gets a picture drawn into its layer, at
/// its size, with no preview open; drawn again only when something changed.
#[test]
fn a_display_that_shows_the_stage_gets_its_picture() {
    let Some(gpu) = crate::gpu::Gpu::headless() else {
        assert!(std::env::var_os("NEWQ_REQUIRE_GPU").is_none(), "no GPU");
        return;
    };
    let mut show = Show::default();
    let a = show.outputs[0].id;
    let b = show.add_display();
    let s = show.add_surface(None);
    show.stage3d.surface_mut(s).unwrap().feed = Feed::Display {
        output: a,
        region: WHOLE,
    };
    show.outputs.iter_mut().find(|o| o.id == b).unwrap().source = Some(StageSource::default());
    let mut live = StageLive::default();
    let mut shared = StageShared::default();
    let mut said = Vec::new();
    live.begin(&show, &mut shared, &mut said);
    assert!(live.feeds_a_display(a), "A's pictures are a display's now");
    let layer = live.output_layer(&gpu, b, (320, 180));
    assert_eq!((layer.texture.width, layer.texture.height), (320, 180));
    let camera = show.stage_camera(
        &StageSource::default(),
        shared.bounds.map(|b| (b.min, b.max)),
        16.0 / 9.0,
    );
    let pictures: HashMap<OutputId, PictureLayers> = HashMap::new();
    live.send(
        &gpu,
        &show,
        &mut shared,
        &[seen(b, camera)],
        &pictures,
        &[],
        false,
    );
    wait(&mut live);
    assert!(layer.texture.is_filled());
    // Nothing changed and nothing plays: not drawn again.
    let serial = live.serial;
    assert!(!live.send(
        &gpu,
        &show,
        &mut shared,
        &[seen(b, camera)],
        &pictures,
        &[],
        false
    ));
    assert_eq!(live.serial, serial);
    // Something plays: drawn again.
    live.send(
        &gpu,
        &show,
        &mut shared,
        &[seen(b, camera)],
        &pictures,
        &[],
        true,
    );
    assert_eq!(live.serial, serial + 1, "a job was sent");
    wait(&mut live);
    // The display stops showing the stage: its target goes.
    live.send(&gpu, &show, &mut shared, &[], &pictures, &[], false);
    assert!(live.target_of(ViewKey::Output(b)).is_none());
    // The device is lost: everything on it goes, the thread stops.
    if let Some(h) = live.forget_device(&mut shared) {
        h.join().unwrap();
    }
}

/// Until what was handed over is drawn.
fn wait(live: &mut StageLive) {
    let end = Instant::now() + Duration::from_secs(20);
    while live.busy() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(10));
    }
    live.landed();
}

fn seen(id: OutputId, camera: Camera) -> StageOutput {
    StageOutput {
        id,
        camera,
        seen: true,
        due: true,
    }
}

/// A show with display A on a screen and display B showing the stage.
fn stage_show() -> (Show, OutputId, OutputId) {
    let mut show = Show::default();
    let a = show.outputs[0].id;
    let b = show.add_display();
    let s = show.add_surface(None);
    show.stage3d.surface_mut(s).unwrap().feed = Feed::Display {
        output: a,
        region: WHOLE,
    };
    show.outputs.iter_mut().find(|o| o.id == b).unwrap().source = Some(StageSource::default());
    (show, a, b)
}

/// A display showing the stage is drawn again when a picture on the stage
/// changes though nothing plays (a test pattern, a still), and not when
/// nothing does.
#[test]
fn a_still_change_on_a_screen_redraws_a_display_showing_the_stage() {
    let Some(gpu) = crate::gpu::Gpu::headless() else {
        assert!(std::env::var_os("NEWQ_REQUIRE_GPU").is_none(), "no GPU");
        return;
    };
    let (show, a, b) = stage_show();
    let mut live = StageLive::default();
    let mut shared = StageShared::default();
    live.begin(&show, &mut shared, &mut Vec::new());
    live.output_layer(&gpu, b, (160, 90));
    let camera = Camera::default();
    let mut pictures: HashMap<OutputId, PictureLayers> = HashMap::new();
    pictures.insert(
        a,
        PictureLayers {
            layers: Vec::new(),
            canvas: (1920, 1080),
        },
    );
    live.send(
        &gpu,
        &show,
        &mut shared,
        &[seen(b, camera)],
        &pictures,
        &[],
        false,
    );
    wait(&mut live);
    let serial = live.serial;
    live.send(
        &gpu,
        &show,
        &mut shared,
        &[seen(b, camera)],
        &pictures,
        &[],
        false,
    );
    assert_eq!(live.serial, serial, "nothing changed");
    // A test pattern (any layer) turned on on A: drawn again, once.
    let pattern = Arc::new(LayerTexture::new_rgba(&gpu, 4, 4));
    pictures.get_mut(&a).unwrap().layers.push(OutputLayer {
        texture: pattern,
        opacity: 1.0,
        fit: crate::show::Fit::Stretch,
        transform: Default::default(),
        color: Default::default(),
        fx: Default::default(),
    });
    live.send(
        &gpu,
        &show,
        &mut shared,
        &[seen(b, camera)],
        &pictures,
        &[],
        false,
    );
    assert_eq!(live.serial, serial + 1);
    wait(&mut live);
    live.send(
        &gpu,
        &show,
        &mut shared,
        &[seen(b, camera)],
        &pictures,
        &[],
        false,
    );
    assert_eq!(live.serial, serial + 1);
    // Turned off again: drawn again.
    pictures.get_mut(&a).unwrap().layers.clear();
    live.send(
        &gpu,
        &show,
        &mut shared,
        &[seen(b, camera)],
        &pictures,
        &[],
        false,
    );
    assert_eq!(live.serial, serial + 2);
    wait(&mut live);
    if let Some(h) = live.forget_device(&mut shared) {
        h.join().unwrap();
    }
}

/// A drawing thread that stopped (a panic) isn't waited for for ever: the
/// main loop isn't kept busy, and the still stage is drawn again by a new
/// thread once it may start (`wake_at`).
#[test]
fn after_the_drawing_thread_stops_a_still_stage_is_drawn_again() {
    let Some(gpu) = crate::gpu::Gpu::headless() else {
        assert!(std::env::var_os("NEWQ_REQUIRE_GPU").is_none(), "no GPU");
        return;
    };
    let (show, _a, b) = stage_show();
    let mut live = StageLive::default();
    let mut shared = StageShared::default();
    live.begin(&show, &mut shared, &mut Vec::new());
    let layer = live.output_layer(&gpu, b, (160, 90));
    let pictures = HashMap::new();
    let out = [seen(b, Camera::default())];
    live.send(&gpu, &show, &mut shared, &out, &pictures, &[], false);
    wait(&mut live);
    assert!(layer.texture.is_filled());
    live.panic_thread();
    // Something to draw (it plays), with the thread gone: not busy, and the
    // loop is to come back when another may start.
    let busy = live.send(&gpu, &show, &mut shared, &out, &pictures, &[], true);
    assert!(!busy, "a stopped thread is never busy");
    assert!(!live.has_thread(), "not started again within RESTART_EVERY");
    let at = live.wake_at().expect("a time to start it again");
    assert!(at > Instant::now() && at <= Instant::now() + RESTART_EVERY);
    // Nothing changes from now on (a still stage): once the time has come
    // it is drawn all the same.
    std::thread::sleep(at.saturating_duration_since(Instant::now()));
    let serial = live.serial;
    live.send(&gpu, &show, &mut shared, &out, &pictures, &[], false);
    assert!(live.has_thread());
    assert_eq!(live.serial, 1, "drawn by the new thread (was {serial})");
    wait(&mut live);
    assert!(
        live.wake_at()
            .is_some_and(|t| t > Instant::now() + RESTART_EVERY)
    );
    if let Some(h) = live.forget_device(&mut shared) {
        h.join().unwrap();
    }
}

/// With no 3D view and no display showing the stage for `IDLE_RELEASE`,
/// the thread goes with all it kept on the card; a view asked for later
/// starts it again at once. A display only the previews show is drawn at
/// their pace.
#[test]
fn an_idle_stage_lets_go_of_the_graphics_card() {
    let Some(gpu) = crate::gpu::Gpu::headless() else {
        assert!(std::env::var_os("NEWQ_REQUIRE_GPU").is_none(), "no GPU");
        return;
    };
    let (show, _a, b) = stage_show();
    let mut live = StageLive::default();
    let mut shared = StageShared::default();
    live.begin(&show, &mut shared, &mut Vec::new());
    live.output_layer(&gpu, b, (160, 90));
    let pictures = HashMap::new();
    // Only the previews show B, and they are held back: not drawn.
    let mut governed = seen(b, Camera::default());
    governed.seen = false;
    governed.due = false;
    live.send(&gpu, &show, &mut shared, &[governed], &pictures, &[], true);
    assert_eq!(live.serial, 0);
    assert!(!live.has_thread());
    // B's own pictures are then preview work too.
    live.begin(&show, &mut shared, &mut Vec::new());
    assert!(!live.feeds_a_display(_a));
    governed.due = true;
    live.send(&gpu, &show, &mut shared, &[governed], &pictures, &[], true);
    assert_eq!(live.serial, 1);
    wait(&mut live);
    // B stops showing the stage; nothing asks for a while.
    live.send(&gpu, &show, &mut shared, &[], &pictures, &[], false);
    assert!(live.has_thread(), "not at once");
    live.as_if_idle();
    live.send(&gpu, &show, &mut shared, &[], &pictures, &[], false);
    assert!(!live.has_thread());
    assert_eq!(live.wake_at(), None, "nothing to come back for");
    // Asked for again: started at once.
    live.output_layer(&gpu, b, (160, 90));
    live.send(
        &gpu,
        &show,
        &mut shared,
        &[seen(b, Camera::default())],
        &pictures,
        &[],
        false,
    );
    assert!(live.has_thread());
    wait(&mut live);
    if let Some(h) = live.forget_device(&mut shared) {
        h.join().unwrap();
    }
}

/// On a show network's computer: until the venue's files have come from the
/// primary, the screens are drawn without the room and the venue is said to
/// be on its way (never a failure); it is read once they are here.
#[test]
fn a_venue_still_arriving_is_read_once_it_has_come() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static COME: AtomicBool = AtomicBool::new(false);
    fn store(p: &std::path::Path) -> crate::stage3d::load::Found {
        use crate::stage3d::load::Found;
        if p.extension().is_some_and(|e| e == "obj") && !COME.load(Ordering::SeqCst) {
            Found::Arriving
        } else {
            Found::Here(p.to_path_buf())
        }
    }
    let dir = temp_dir("arriving");
    let path = dir.join("room.obj");
    std::fs::write(&path, "v -5 0 -5\nv 5 0 -5\nv 5 0 5\nf 1 2 3\n").unwrap();
    let (mut show, _a, _b) = stage_show();
    show.stage3d.venue = Some(Venue::new(path.clone()));
    let mut live = StageLive::default();
    live.locate_with(store);
    let mut shared = StageShared::default();
    let mut said = Vec::new();
    for _ in 0..3 {
        live.begin(&show, &mut shared, &mut said);
    }
    assert!(said.is_empty(), "{}", said.len());
    assert_eq!(shared.venue, VenueStatus::Arriving("room.obj".into()));
    assert!(!live.venue_busy());
    assert!(live.wake_at().is_some(), "looked at again");
    assert!(shared.scene.parts.iter().all(|p| !p.lit), "screens only");
    assert_eq!(shared.scene.parts.len(), 1);
    COME.store(true, Ordering::SeqCst);
    std::thread::sleep(ARRIVING_LOOK);
    let words = settle(&mut live, &show, &mut shared);
    assert!(
        words.iter().any(|w| w.starts_with("Venue loaded")),
        "{words:?}"
    );
    assert!(shared.scene.parts[0].lit, "the room is in the scene");
    let _ = std::fs::remove_dir_all(&dir);
}

/// While the thread is behind with the displays (a job waiting behind one
/// whose displays are still being drawn) the previews wait, keeping their
/// pictures; one job under way is normal, and they are drawn then.
#[test]
fn previews_give_way_while_the_thread_is_behind_with_the_displays() {
    let Some(gpu) = crate::gpu::Gpu::headless() else {
        assert!(std::env::var_os("NEWQ_REQUIRE_GPU").is_none(), "no GPU");
        return;
    };
    let (show, _a, b) = stage_show();
    let mut live = StageLive::default();
    let mut shared = StageShared::default();
    live.begin(&show, &mut shared, &mut Vec::new());
    live.output_layer(&gpu, b, (160, 90));
    let pictures = HashMap::new();
    let preview = PreviewId::Stage(1);
    let ask = |shared: &mut StageShared| {
        shared.requests.push(crate::ui::stage_view::ViewRequest {
            id: preview,
            size: (64, 36),
            camera: Camera::default(),
            options: Options::default(),
            lines: Default::default(),
        })
    };
    let display = [seen(b, Camera::default())];
    live.set_behind(true);
    ask(&mut shared);
    live.send(&gpu, &show, &mut shared, &display, &pictures, &[], true);
    assert_eq!(live.serial, 1, "the display is drawn");
    assert!(live.drawn_with.contains_key(&ViewKey::Output(b)));
    assert!(!live.drawn_with.contains_key(&ViewKey::Preview(preview)));
    assert!(shared.targets.contains_key(&preview), "its picture is kept");
    wait(&mut live);
    // Not behind: drawn.
    live.set_behind(false);
    ask(&mut shared);
    live.send(&gpu, &show, &mut shared, &display, &pictures, &[], true);
    assert_eq!(live.serial, 2);
    assert!(live.drawn_with.contains_key(&ViewKey::Preview(preview)));
    wait(&mut live);
    if let Some(h) = live.forget_device(&mut shared) {
        h.join().unwrap();
    }
}

/// A proscenium theatre (the sample) with a wide picture blended from two
/// projectors on its screen: the picture's display shows the test pattern,
/// each projector's display its half, fading over a quarter of its width.
/// The projectors stand in the stalls, square to the screen. (The light's
/// demonstration show, and its tests'.)
pub(crate) fn blended_pair_show() -> (Show, [crate::stage3d::ProjectorId; 2]) {
    let mut show = Show::default();
    let overlap = 0.25;
    let (wide, left, right) = show.add_blended_pair(1920, 1080, overlap);
    if let Some(o) = show.outputs.iter_mut().find(|o| o.id == wide) {
        o.test_pattern = true;
    }
    let screens = crate::ui::stage_edit::use_sample(
        &mut show,
        crate::stage3d::samples::SampleVenue::Proscenium,
        true,
    );
    for s in &screens {
        if let Some(s) = show.stage3d.surface_mut(*s) {
            s.feed = Feed::Display {
                output: wide,
                region: WHOLE,
            };
        }
    }
    let screen = show.stage3d.surface(screens[0]).unwrap().clone();
    let crate::stage3d::SurfaceShape::Flat { width, .. } = screen.shape else {
        panic!("the proscenium's screen is flat");
    };
    // Each projector's picture: the screen's width over the two less the
    // overlap.
    let each = width / (2.0 - overlap);
    let mut ids = [crate::stage3d::ProjectorId(0); 2];
    for (k, (display, side)) in [(left, -1.0f32), (right, 1.0)].into_iter().enumerate() {
        let id = show.add_projector();
        let p = show.projector_mut(id).unwrap();
        p.name = if side < 0.0 { "Left" } else { "Right" }.into();
        p.display = Some(display);
        p.lumens = 12_000.0;
        let distance = p.distance_for(each);
        let x = side * (width - each) / 2.0;
        let [sx, sy, sz] = screen.placement.position;
        p.placement.position = [sx + x, sy, sz + distance];
        p.placement.yaw = screen.placement.yaw;
        ids[k] = id;
    }
    (show, ids)
}

/// Writes the light's demonstration show to `NEWQ_DEMO_SHOW` (by hand, to
/// look at it headless). `NEWQ_DEMO_NO_BLEND` takes the blends off;
/// `NEWQ_DEMO_SIDE` stands the right projector well off to the side, aimed
/// at the screen's middle, so the stage's walls cast shadows.
#[test]
#[ignore]
fn write_the_lights_demonstration_show() {
    let Some(path) = std::env::var_os("NEWQ_DEMO_SHOW") else {
        return;
    };
    let (mut show, [_, right]) = blended_pair_show();
    if std::env::var_os("NEWQ_DEMO_NO_BLEND").is_some() {
        for o in &mut show.outputs {
            o.blend = Default::default();
        }
    }
    if std::env::var_os("NEWQ_DEMO_SIDE").is_some() {
        let middle = show.stage3d.surfaces[0].placement.position;
        let p = show.projector_mut(right).unwrap();
        p.placement.position[0] += 9.0;
        p.placement.position[1] -= 2.0;
        p.aim_at(middle, 16.0 / 9.0);
    }
    show.save(std::path::Path::new(&path)).unwrap();
}

/// Each projector as the app hands it over: 16:9, and what its display
/// sends (`app::throw_picture`).
fn projector_throws(show: &Show, groups: &HashMap<OutputId, OutputGroup>) -> Vec<ProjectorThrow> {
    show.stage3d
        .projectors
        .iter()
        .map(|p| ProjectorThrow {
            id: p.id,
            pixels: (1920, 1080),
            aspect: 16.0 / 9.0,
            picture: p
                .display
                .and_then(|d| crate::app::throw_picture(show, d, groups)),
        })
        .collect()
}

/// Each display's group, as `update_outputs` makes them (here with no
/// layers): its own keystone and part, placed on a monitor's area.
fn groups_of(show: &Show) -> HashMap<OutputId, OutputGroup> {
    show.outputs
        .iter()
        .map(|o| {
            let part = show.part_of(o.id).map(|p| p.area).unwrap_or(WHOLE);
            let pin = crate::gpu::corner_pin(&[
                [part[0], part[1]],
                [part[0] + part[2], part[1]],
                [part[0] + part[2], part[1] + part[3]],
                [part[0], part[1] + part[3]],
            ]);
            let placed = crate::gpu::compose(
                &crate::gpu::corner_pin(&[[0.5, 0.0], [1.0, 0.0], [1.0, 1.0], [0.5, 1.0]]),
                &pin,
            );
            (
                o.id,
                OutputGroup {
                    layers: Vec::new(),
                    geometry: crate::gpu::Geometry {
                        canvas: (1920, 1080),
                        warp: placed,
                        pin,
                        mesh: None,
                        slice: crate::gpu::Geometry::WHOLE,
                    },
                },
            )
        })
        .collect()
}

#[test]
fn a_projector_throws_what_its_window_sends_with_its_blend_not_where_it_sits() {
    let (mut show, [left, _]) = blended_pair_show();
    let groups = groups_of(&show);
    let display = show.projector(left).unwrap().display.unwrap();
    let t = crate::app::throw_picture(&show, display, &groups).unwrap();
    // Its keystone and part, not its place on the monitor.
    assert_eq!(t.groups.len(), 1);
    assert_eq!(t.groups[0].geometry.warp, groups[&display].geometry.pin);
    assert_ne!(t.groups[0].geometry.warp, groups[&display].geometry.warp);
    // Its edge blend: the left projector fades at its right.
    assert_eq!(t.blend.right, 0.25);
    assert_eq!(t.blend.left, 0.0);
    // A sub display is in it, in its area.
    let sub = show.add_sub_display(display);
    let groups = groups_of(&show);
    let t = crate::app::throw_picture(&show, display, &groups).unwrap();
    assert_eq!(t.groups.len(), 2);
    let _ = sub;
    // A display that shows the stage throws none (never drawn from itself),
    // nor one that isn't on.
    let wide = show.part_of(display).unwrap().display;
    show.outputs
        .iter_mut()
        .find(|o| o.id == wide)
        .unwrap()
        .source = Some(StageSource::default());
    assert!(crate::app::throw_picture(&show, display, &groups).is_none());
    assert!(crate::app::throw_picture(&show, OutputId(999_999), &groups).is_none());
}

#[test]
fn the_light_puts_the_projectors_in_the_scene_and_their_pictures_in_jobs() {
    let (mut show, [left, right]) = blended_pair_show();
    let mut live = StageLive::default();
    let mut shared = StageShared::default();
    live.begin(&show, &mut shared, &mut Vec::new());
    let groups = groups_of(&show);
    let (key, jobs) = live.light_up(&show, &projector_throws(&show, &groups));
    assert_eq!(live.lit_scene.throws.len(), 2);
    assert_eq!(live.lit_scene.parts.len(), live.scene.parts.len());
    assert!(live.lit_scene.throws.iter().all(|t| t.picture && t.on));
    assert_eq!(
        jobs.iter().map(|j| j.layer).collect::<Vec<_>>(),
        [0, 1],
        "one layer each, in order"
    );
    assert_eq!(jobs[0].blend.right, 0.25);
    assert_eq!(jobs[1].blend.left, 0.25);
    // The same again: the same scene (not made again), the same key.
    let scene = live.lit_scene.clone();
    let (again, _) = live.light_up(&show, &projector_throws(&show, &groups));
    assert_eq!(again, key);
    assert!(Arc::ptr_eq(&scene, &live.lit_scene));
    // A blend's width changes what is sent, not where the light goes.
    let display = show.projector(left).unwrap().display.unwrap();
    show.outputs
        .iter_mut()
        .find(|o| o.id == display)
        .unwrap()
        .blend
        .right = 0.1;
    let (wider, _) = live.light_up(&show, &projector_throws(&show, &groups));
    assert_ne!(wider, key);
    assert!(Arc::ptr_eq(&scene, &live.lit_scene));
    // A projector moved: the scene's light is made again.
    show.projector_mut(right).unwrap().placement.position[0] += 1.0;
    live.light_up(&show, &projector_throws(&show, &groups));
    assert!(!Arc::ptr_eq(&scene, &live.lit_scene));
    // One with no display throws white; one hidden throws nothing.
    show.projector_mut(right).unwrap().display = None;
    show.projector_mut(left).unwrap().visible = false;
    let (_, jobs) = live.light_up(&show, &projector_throws(&show, &groups));
    assert!(jobs.is_empty());
    assert_eq!(live.lit_scene.throws.len(), 1);
    assert!(!live.lit_scene.throws[0].picture);
    // At most sixteen.
    for _ in 0..20 {
        show.add_projector();
    }
    live.light_up(&show, &projector_throws(&show, &groups));
    assert_eq!(
        live.lit_scene.throws.len(),
        crate::stage3d::light::MAX_THROWS
    );
}

/// A view with its light on draws the projectors' light, and the one beside
/// it with its light off doesn't; a view with its light on that the
/// governor holds back sends no job, so no picture is drawn for it.
#[test]
fn only_a_view_with_its_light_on_is_lit() {
    let Some(gpu) = crate::gpu::Gpu::headless() else {
        assert!(std::env::var_os("NEWQ_REQUIRE_GPU").is_none(), "no GPU");
        return;
    };
    let (show, _) = blended_pair_show();
    let mut live = StageLive::default();
    let mut shared = StageShared::default();
    live.begin(&show, &mut shared, &mut Vec::new());
    let groups = groups_of(&show);
    let throws = projector_throws(&show, &groups);
    let ask = |shared: &mut StageShared, id: PreviewId, light| {
        shared.requests.push(crate::ui::stage_view::ViewRequest {
            id,
            size: (64, 36),
            camera: Camera::default(),
            options: Options {
                light,
                ..Options::default()
            },
            lines: Default::default(),
        })
    };
    let (lit, plain) = (PreviewId::Stage(1), PreviewId::Stage(2));
    ask(&mut shared, lit, crate::render3d::LightView::Picture);
    ask(&mut shared, plain, crate::render3d::LightView::Off);
    live.send(
        &gpu,
        &show,
        &mut shared,
        &[],
        &HashMap::new(),
        &throws,
        true,
    );
    wait(&mut live);
    // Asked again, so the stats are read; still, with nothing playing, it
    // is not drawn again (a lit view that doesn't change costs nothing).
    ask(&mut shared, lit, crate::render3d::LightView::Picture);
    ask(&mut shared, plain, crate::render3d::LightView::Off);
    let drawn = live.serial;
    live.send(
        &gpu,
        &show,
        &mut shared,
        &[],
        &HashMap::new(),
        &throws,
        false,
    );
    assert_eq!(live.serial, drawn, "a still lit view isn't drawn again");
    assert_eq!(shared.stats[&lit].throws, 2);
    assert!(!shared.stats[&lit].light_left_out);
    assert_eq!(shared.stats[&plain].throws, 0);
    // Nothing asked: no job.
    let serial = live.serial;
    live.send(
        &gpu,
        &show,
        &mut shared,
        &[],
        &HashMap::new(),
        &throws,
        true,
    );
    assert_eq!(live.serial, serial);
    if let Some(h) = live.forget_device(&mut shared) {
        h.join().unwrap();
    }
}
