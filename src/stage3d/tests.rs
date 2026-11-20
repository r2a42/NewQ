use super::*;
use crate::show::Show;

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("newq-stage3d-show-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn a_show_without_a_stage_writes_none_and_old_shows_load() {
    let show = Show::default();
    let json = serde_json::to_string(&show).unwrap();
    assert!(!json.contains("stage3d"), "{json}");
    let back: Show = serde_json::from_str(&json).unwrap();
    assert!(back.stage3d.is_empty());
    // A show from 0.24 (no stage3d at all) loads, from a file too.
    let dir = temp_dir("old");
    let path = dir.join("old.newq");
    std::fs::write(
        &path,
        r#"{"version":2,"name":"Old","outputs":[],"cues":[],"next_id":1}"#,
    )
    .unwrap();
    let old = Show::load(&path).unwrap();
    assert!(old.stage3d.is_empty());
    old.save(&path).unwrap();
    assert!(!std::fs::read_to_string(&path).unwrap().contains("stage3d"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_stage_round_trips_writing_only_what_isnt_its_default() {
    let mut show = Show::default();
    let display = show.outputs[0].id;
    let flat = show.add_surface(None);
    let curved = show.add_surface(Some(SurfaceShape::Curved {
        width: 12.0,
        radius: 8.0,
        height: 3.0,
        convex: false,
    }));
    let boxed = show.add_surface(Some(SurfaceShape::Faceted {
        facets: geometry::jumbotron(4, 6.0, 3.5, true, true),
    }));
    show.stage3d.surface_mut(flat).unwrap().feed = Feed::Display {
        output: display,
        region: WHOLE,
    };
    show.stage3d.surface_mut(curved).unwrap().feed = Feed::Display {
        output: display,
        region: [0.0, 0.0, 0.5, 1.0],
    };
    show.stage3d.surface_mut(curved).unwrap().placement = Placement {
        position: [0.0, 4.0, -10.0],
        yaw: 15.0,
        ..Placement::default()
    };
    show.stage3d.viewpoints.push(Viewpoint {
        name: "Front of house".into(),
        eye: [0.0, 1.7, 20.0],
        target: [0.0, 3.0, 0.0],
        fov_deg: 50.0,
    });
    show.stage3d.venue = Some(Venue::new("/shows/arena.glb".into()));
    let json = serde_json::to_string(&show).unwrap();
    let back: Show = serde_json::from_str(&json).unwrap();
    assert_eq!(back.stage3d, show.stage3d);
    // Defaults left out: a whole region, visible, no turn, Y up in metres,
    // the field of view, a facet's whole picture, convex false.
    let stage = serde_json::to_string(&show.stage3d).unwrap();
    for absent in [
        "visible",
        "region\":[0.0,0.0,1.0",
        "roll",
        "units",
        "\"up\"",
        "fov",
        "brightness",
        "convex",
    ] {
        assert!(!stage.contains(absent), "{absent} in {stage}");
    }
    assert!(stage.contains("\"yaw\":15.0"));
    // Ids are the show's, so they never meet a cue's or a display's.
    assert!(flat.0 > display.0 && curved.0 > flat.0 && boxed.0 > curved.0);
    assert_eq!(show.stage3d.surface(boxed).unwrap().name, "Screen 3");
}

#[test]
fn the_venue_file_is_kept_beside_the_show_as_cues_files_are() {
    let dir = temp_dir("venue");
    let path = dir.join("show.newq");
    let mut show = Show::default();
    show.stage3d.venue = Some(Venue::new(dir.join("models/arena.glb")));
    show.save(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"path\": \"models/arena.glb\""), "{text}");
    let back = Show::load(&path).unwrap();
    assert_eq!(
        back.stage3d.venue.unwrap().path,
        dir.join("models/arena.glb")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_removed_display_leaves_its_screens_showing_nothing() {
    let mut show = Show::default();
    let first = show.outputs[0].id;
    let second = show.add_display();
    let sub = show.add_sub_display(second);
    let a = show.add_surface(None);
    let b = show.add_surface(None);
    let c = show.add_surface(None);
    show.stage3d.surface_mut(a).unwrap().feed = Feed::Display {
        output: first,
        region: WHOLE,
    };
    show.stage3d.surface_mut(b).unwrap().feed = Feed::Display {
        output: second,
        region: WHOLE,
    };
    show.stage3d.surface_mut(c).unwrap().feed = Feed::Display {
        output: sub,
        region: WHOLE,
    };
    // Removing a display takes its sub displays with it.
    show.remove_output(second);
    assert_eq!(show.stage3d.surface(a).unwrap().feed.output(), Some(first));
    assert_eq!(show.stage3d.surface(b).unwrap().feed, Feed::None);
    assert_eq!(show.stage3d.surface(c).unwrap().feed, Feed::None);
}

#[test]
fn a_hand_edited_file_is_put_right_on_loading() {
    let dir = temp_dir("hand");
    let path = dir.join("show.newq");
    let mut show = Show::default();
    let s = show.add_surface(None);
    show.stage3d.surface_mut(s).unwrap().feed = Feed::Display {
        output: OutputId(999),
        region: WHOLE,
    };
    let mut json = serde_json::to_value(&show).unwrap();
    // A stale counter, below the surface's id.
    json["next_id"] = serde_json::json!(1);
    std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
    let back = Show::load(&path).unwrap();
    assert_eq!(back.stage3d.surfaces[0].feed, Feed::None);
    assert!(back.next_id > s.0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_blended_groups_screen_shows_the_whole_picture() {
    let mut show = Show::default();
    let (picture, left, right) = show.add_blended_pair(1920, 1080, 0.2);
    assert_eq!(show.whole_picture(left), picture);
    assert_eq!(show.whole_picture(right), picture);
    assert_eq!(show.whole_picture(picture), picture);
    let first = show.outputs[0].id;
    assert_eq!(show.whole_picture(first), first);
    assert_eq!(
        Feed::whole_picture(&show, left),
        Feed::Display {
            output: picture,
            region: WHOLE
        }
    );
}

#[test]
fn the_scene_has_screens_unlit_with_their_pictures_and_grey_without() {
    let mut show = Show::default();
    let display = show.outputs[0].id;
    let fed = show.add_surface(None);
    let _unfed = show.add_surface(None);
    let gone = show.add_surface(None);
    let hidden = show.add_surface(None);
    let wall = show.add_surface(Some(SurfaceShape::Faceted {
        facets: geometry::angled_wall(3, 2, 0.5, 0.5, 5.0),
    }));
    let set = |show: &mut Show, id, feed| show.stage3d.surface_mut(id).unwrap().feed = feed;
    set(
        &mut show,
        fed,
        Feed::Display {
            output: display,
            region: WHOLE,
        },
    );
    set(
        &mut show,
        gone,
        Feed::Display {
            output: OutputId(12345),
            region: WHOLE,
        },
    );
    set(
        &mut show,
        wall,
        Feed::Display {
            output: display,
            region: [0.5, 0.0, 0.5, 1.0],
        },
    );
    show.stage3d.surface_mut(hidden).unwrap().visible = false;
    show.stage3d.surface_mut(fed).unwrap().placement.position = [1.0, 2.0, 3.0];
    let scene = show.stage3d.scene(&show, None);
    // Fed, unfed, gone, and the wall's six panels; the hidden one not.
    assert_eq!(scene.parts.len(), 3 + 6);
    assert!(scene.parts.iter().all(|p| !p.lit));
    assert_eq!(
        scene.parts[0].paint,
        Paint::Output {
            output: display,
            region: WHOLE
        }
    );
    assert_eq!(scene.parts[0].transform[3], [1.0, 2.0, 3.0, 1.0]);
    assert_eq!(scene.parts[1].paint, Paint::Colour(UNFED_GREY));
    assert_eq!(scene.parts[2].paint, Paint::Colour(UNFED_GREY));
    // The wall's panels: their shares of the right half of the picture,
    // sharing one mesh.
    let panels = &scene.parts[3..];
    let Paint::Output { region, .. } = panels[4].paint else {
        panic!()
    };
    let want = [0.5 + 0.5 / 3.0, 0.5, 0.5 / 3.0, 0.5];
    assert!(
        region.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-6),
        "{region:?}"
    );
    assert!(panels.iter().all(|p| Arc::ptr_eq(&p.mesh, &panels[0].mesh)));
    assert!(panels.iter().all(|p| p.mesh_key == panels[0].mesh_key));
    // Two flat screens of one size share a key too; the scene doesn't
    // change from one call to the next.
    assert_eq!(scene.parts[0].mesh_key, scene.parts[1].mesh_key);
    assert_eq!(show.stage3d.scene(&show, None), scene);
}

#[test]
fn a_hidden_venue_isnt_drawn_and_a_viewpoint_gives_a_sensible_camera() {
    let mut stage = Stage3d {
        venue: Some(Venue::new("x.glb".into())),
        ..Stage3d::default()
    };
    let loaded = load::LoadedVenue {
        parts: vec![load::VenuePart {
            mesh: Arc::new(geometry::quad(1.0, 1.0)),
            mesh_key: 7,
            paint: Paint::Colour([1.0; 4]),
        }],
        bounds: None,
        triangles: 2,
        warnings: Vec::new(),
        loaded_for: Venue::new("x.glb".into()),
    };
    let show = Show::default();
    assert_eq!(stage.scene(&show, Some(&loaded)).parts.len(), 1);
    assert!(stage.scene(&show, None).parts.is_empty());
    stage.venue.as_mut().unwrap().visible = false;
    assert!(stage.scene(&show, Some(&loaded)).parts.is_empty());
    assert!(stage.venue_bounds(&loaded).is_none());

    let v = Viewpoint {
        name: "Desk".into(),
        eye: [0.0, 2.0, 15.0],
        target: [0.0, 2.0, 0.0],
        fov_deg: 400.0,
    };
    let cam = v.camera();
    assert_eq!(
        (cam.eye, cam.target, cam.fov_y_deg),
        ([0.0, 2.0, 15.0], [0.0, 2.0, 0.0], 150.0)
    );
    let bad = Viewpoint {
        eye: [f32::NAN; 3],
        fov_deg: f32::NAN,
        ..v
    };
    assert_eq!(bad.camera(), scene::Camera::default());
}

#[test]
fn brightness_and_placement_numbers_from_a_file_are_kept_sane() {
    let mut v = Venue::new("a.glb".into());
    v.brightness = f32::NAN;
    assert_eq!(v.brightness(), 1.0);
    v.brightness = 100.0;
    assert_eq!(v.brightness(), 4.0);
    v.brightness = -1.0;
    assert_eq!(v.brightness(), 0.05);
    let p = Placement {
        position: [f32::NAN, 1.0, f32::INFINITY],
        yaw: f32::NAN,
        pitch: 0.0,
        roll: f32::NEG_INFINITY,
    };
    assert!(p.matrix().iter().flatten().all(|v| v.is_finite()));
    for u in LengthUnit::ALL {
        assert!(u.metres() > 0.0 && !u.label().is_empty());
    }
}

#[test]
fn the_shape_key_follows_the_stage() {
    let mut show = Show::default();
    let before = show.stage3d.shape_key();
    let s = show.add_surface(None);
    let after = show.stage3d.shape_key();
    assert_ne!(before, after);
    show.stage3d.surface_mut(s).unwrap().placement.yaw = 1.0;
    assert_ne!(show.stage3d.shape_key(), after);
}
