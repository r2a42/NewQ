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
    let wait = |live: &mut StageLive| {
        let end = Instant::now() + Duration::from_secs(20);
        while !live.landed() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    live.send(&gpu, &show, &mut shared, &[(b, camera)], &pictures, false);
    wait(&mut live);
    assert!(layer.texture.is_filled());
    // Nothing changed and nothing plays: not drawn again.
    let serial = live.serial;
    assert!(!live.send(&gpu, &show, &mut shared, &[(b, camera)], &pictures, false));
    assert_eq!(live.serial, serial);
    // Something plays: drawn again.
    live.send(&gpu, &show, &mut shared, &[(b, camera)], &pictures, true);
    assert_eq!(live.serial, serial + 1, "a job was sent");
    wait(&mut live);
    // The display stops showing the stage: its target goes.
    live.send(&gpu, &show, &mut shared, &[], &pictures, false);
    assert!(live.target_of(ViewKey::Output(b)).is_none());
    // The device is lost: everything on it goes, the thread stops.
    if let Some(h) = live.forget_device(&mut shared) {
        h.join().unwrap();
    }
}
