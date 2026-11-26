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

#[test]
fn a_display_shows_the_stage_only_when_asked_and_old_files_load() {
    let mut show = Show::default();
    let d = show.outputs[0].id;
    let json = serde_json::to_string(&show).unwrap();
    assert!(!json.contains("\"source\""), "unwritten when None: {json}");
    show.outputs[0].source = Some(StageSource::default());
    let json = serde_json::to_string(&show).unwrap();
    assert!(json.contains("\"source\":{}"), "{json}");
    let back: Show = serde_json::from_str(&json).unwrap();
    assert_eq!(back.outputs[0].source, Some(StageSource::default()));
    show.outputs[0].source = Some(StageSource {
        viewpoint: Some("FOH".into()),
    });
    let back: Show = serde_json::from_str(&serde_json::to_string(&show).unwrap()).unwrap();
    assert_eq!(
        back.outputs[0]
            .source
            .as_ref()
            .unwrap()
            .viewpoint
            .as_deref(),
        Some("FOH")
    );
    // A display from 0.24 (no source) shows its cues.
    let old: crate::show::OutputConfig =
        serde_json::from_str(r#"{"id":5,"name":"Old","enabled":true}"#).unwrap();
    assert!(old.source.is_none());
    assert!(show.shows_stage(d));
}

#[test]
fn a_display_showing_the_stage_is_never_a_picture_on_it() {
    let mut show = Show::default();
    let a = show.outputs[0].id;
    let b = show.add_display();
    let s1 = show.add_surface(None);
    let s2 = show.add_surface(None);
    show.stage3d.surface_mut(s1).unwrap().feed = Feed::whole_picture(&show, a);
    show.stage3d.surface_mut(s2).unwrap().feed = Feed::whole_picture(&show, b);
    let scene = show.stage3d.scene(&show, None);
    assert_eq!(outputs_in(&scene), vec![a, b]);
    // B shows the stage: its surface is unfed, A's still shows A.
    show.outputs.iter_mut().find(|o| o.id == b).unwrap().source = Some(StageSource::default());
    let scene = show.stage3d.scene(&show, None);
    assert_eq!(outputs_in(&scene), vec![a]);
    assert!(
        scene
            .parts
            .iter()
            .any(|p| p.paint == Paint::Colour(UNFED_GREY))
    );
    // A projector that shows part of a picture that shows the stage is
    // that picture too.
    let (wide, left, _right) = show.add_blended_pair(1920, 1080, 0.1);
    assert_eq!(show.whole_picture(left), wide);
    show.stage3d.surface_mut(s2).unwrap().feed = Feed::Display {
        output: left,
        region: WHOLE,
    };
    assert_eq!(outputs_in(&show.stage3d.scene(&show, None)), vec![a, left]);
    show.outputs
        .iter_mut()
        .find(|o| o.id == wide)
        .unwrap()
        .source = Some(StageSource::default());
    assert!(show.shows_stage(left));
    assert_eq!(outputs_in(&show.stage3d.scene(&show, None)), vec![a]);
}

#[test]
fn a_display_sees_the_stage_from_its_viewpoint_or_all_of_it() {
    let mut show = Show::default();
    show.stage3d.viewpoints.push(Viewpoint {
        name: "Balcony".into(),
        eye: [0.0, 8.0, 20.0],
        target: [0.0, 2.0, 0.0],
        fov_deg: 40.0,
    });
    let named = StageSource {
        viewpoint: Some("Balcony".into()),
    };
    let cam = show.stage_camera(&named, None, 16.0 / 9.0);
    assert_eq!(cam.eye, [0.0, 8.0, 20.0]);
    assert_eq!(cam.fov_y_deg, 40.0);
    // A name no longer there, or none: from in front, all of it in view.
    let gone = StageSource {
        viewpoint: Some("Gone".into()),
    };
    let bounds = Some(([-10.0, 0.0, -2.0], [10.0, 6.0, 2.0]));
    for s in [gone, StageSource::default()] {
        let cam = show.stage_camera(&s, bounds, 16.0 / 9.0);
        assert_eq!(cam.target, [0.0, 3.0, 0.0]);
        assert!(cam.eye[2] > 12.0 && cam.eye[1] > 3.0, "{cam:?}");
        assert!(cam.eye.iter().all(|v| v.is_finite()));
    }
    // Nothing to frame: the default camera.
    let cam = show.stage_camera(&StageSource::default(), None, 0.0);
    assert_eq!(cam, scene::Camera::default());
}

#[test]
fn a_later_newqs_stage_loads_keeping_what_this_one_doesnt_know() {
    // A shape, a feed, a unit, an up axis and a display's source from a
    // later NewQ: the show still opens.
    let mut show = Show::default();
    let display = show.outputs[0].id;
    let known = show.add_surface(None);
    show.stage3d.surfaces[0].feed = Feed::whole_picture(&show, display);
    let mut json: serde_json::Value = serde_json::to_value(&show).unwrap();
    let stage = &mut json["stage3d"];
    stage["units"] = "Furlongs".into();
    let sphere = serde_json::json!({ "Sphere": { "radius": 3.0, "segments": 64 } });
    let hologram = serde_json::json!({ "Hologram": { "output": 7 } });
    stage["surfaces"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "id": 900, "name": "Dome", "shape": sphere, "feed": hologram, "glow": "Led", "kind": "Hologram"
        }));
    stage["surfaces"][0]["track"] = serde_json::json!({ "psn": 3 });
    stage["moves"] = serde_json::json!([{ "to": [1, 2, 3] }]);
    stage["venue"] = serde_json::json!({ "path": "room.obj", "up": "W", "units": "Leagues" });
    json["outputs"][0]["source"] = "Camera".into();
    let text = serde_json::to_string(&json).unwrap();
    let back: Show = serde_json::from_str(&text).expect("a later NewQ's stage loads");
    let st = &back.stage3d;
    assert_eq!(st.units, LengthUnit::Metres);
    let v = st.venue.as_ref().unwrap();
    assert_eq!((v.units, v.up), (LengthUnit::Metres, UpAxis::Y));
    assert_eq!(back.outputs[0].source, None);
    assert_eq!(st.surfaces.len(), 2);
    assert_eq!(st.surfaces[0].id, known);
    let dome = &st.surfaces[1];
    assert!(!dome.shape.is_known());
    assert_eq!(dome.feed.output(), None);
    // Drawn as nothing; the known screen is still drawn and picked.
    let scene = st.scene(&back, None);
    assert_eq!(scene.parts.len(), 1);
    assert_eq!(st.part_surface(&scene, 0), Some(known));
    // Saved again, the later shape and feed are written as they were.
    let again: serde_json::Value = serde_json::to_value(&back).unwrap();
    assert_eq!(again["stage3d"]["surfaces"][1]["shape"], sphere);
    assert_eq!(again["stage3d"]["surfaces"][1]["feed"], hologram);
    // And a later NewQ's fields on a screen and on the stage.
    assert_eq!(again["stage3d"]["surfaces"][1]["glow"], "Led");
    // A later kind of screen is kept as written, and worked out here.
    assert_eq!(again["stage3d"]["surfaces"][1]["kind"], "Hologram");
    assert_eq!(st.surfaces[1].chosen_kind(), None);
    assert_eq!(again["stage3d"]["surfaces"][0]["track"]["psn"], 3);
    assert_eq!(again["stage3d"]["moves"][0]["to"][2], 3);
    // Known ones still read as themselves, not as unknown.
    assert!(st.surfaces[0].shape.is_known());
    assert!(matches!(st.surfaces[0].feed, Feed::Display { .. }));
    let none: Feed = serde_json::from_str("\"None\"").unwrap();
    assert_eq!(none, Feed::None);
    let source: Option<StageSource> = serde_json::from_str(r#"{"viewpoint":"Desk"}"#).unwrap();
    assert_eq!(source.unwrap().viewpoint.as_deref(), Some("Desk"));
}

fn viewpoint(name: &str) -> Viewpoint {
    Viewpoint {
        name: name.into(),
        eye: [0.0, 2.0, 10.0],
        target: [0.0; 3],
        fov_deg: 50.0,
    }
}

#[test]
fn renaming_a_viewpoint_takes_its_displays_with_it_and_names_stay_unique() {
    let mut show = Show::default();
    let a = show.outputs[0].id;
    let b = show.add_display();
    show.stage3d.viewpoints = vec![viewpoint("Desk"), viewpoint("Balcony")];
    let from = |n: &str| {
        Some(StageSource {
            viewpoint: Some(n.into()),
        })
    };
    show.outputs[0].source = from("Desk");
    show.outputs.iter_mut().find(|o| o.id == b).unwrap().source = from("Balcony");
    // Typed a letter at a time: each step renames, and A follows.
    for typed in ["Des", "Desk", "Desk l", "Desk left"] {
        assert!(show.rename_viewpoint(0, typed));
    }
    assert_eq!(show.stage3d.viewpoints[0].name, "Desk left");
    assert_eq!(show.output(a).unwrap().source, from("Desk left"));
    assert_eq!(show.output(b).unwrap().source, from("Balcony"));
    // A name another has (on the way to another name) is refused: the two
    // displays never end up on one viewpoint.
    assert!(!show.rename_viewpoint(0, "Balcony"));
    assert!(!show.rename_viewpoint(0, "  "));
    assert_eq!(show.stage3d.viewpoints[0].name, "Desk left");
    assert_eq!(show.output(a).unwrap().source, from("Desk left"));
    assert!(!show.rename_viewpoint(5, "Nowhere"));
    // A file with two of one name: the second is named again on loading.
    show.stage3d.viewpoints.push(viewpoint("Balcony"));
    show.sanitize_stage();
    let names: Vec<&str> = show
        .stage3d
        .viewpoints
        .iter()
        .map(|v| v.name.as_str())
        .collect();
    assert_eq!(names, ["Desk left", "Balcony", "Balcony 2"]);
    assert_eq!(show.output(b).unwrap().source, from("Balcony"));
}

#[test]
fn a_sub_display_never_shows_the_stage_and_a_projector_shows_its_pictures() {
    let mut show = Show::default();
    let a = show.outputs[0].id;
    let sub = show.add_sub_display(a);
    // Written by hand: a sub display with a source would draw the stage
    // within a picture the stage may show.
    show.outputs
        .iter_mut()
        .find(|o| o.id == sub)
        .unwrap()
        .source = Some(StageSource::default());
    assert!(!show.shows_stage(sub));
    assert!(show.stage_source(sub).is_none());
    assert!(show.stage_source_refused(sub).is_some());
    let dir = temp_dir("sub-source");
    let path = dir.join("sub.newq");
    std::fs::write(&path, serde_json::to_string(&show).unwrap()).unwrap();
    let loaded = Show::load(&path).unwrap();
    assert_eq!(loaded.output(sub).unwrap().source, None);
    let _ = std::fs::remove_dir_all(&dir);
    // A projector of a blended pair takes the whole picture's, and may not
    // be given its own; the whole picture may.
    let (wide, left, _right) = show.add_blended_pair(1920, 1080, 0.1);
    assert!(show.stage_source_refused(left).is_some());
    assert!(show.stage_source_refused(wide).is_none());
    assert!(show.stage_source_refused(a).is_none());
    show.outputs
        .iter_mut()
        .find(|o| o.id == left)
        .unwrap()
        .source = Some(StageSource::default());
    assert!(!show.shows_stage(left), "its own is ignored");
    show.outputs
        .iter_mut()
        .find(|o| o.id == wide)
        .unwrap()
        .source = Some(StageSource {
        viewpoint: Some("Desk".into()),
    });
    assert_eq!(
        show.stage_source(left).and_then(|s| s.viewpoint.as_deref()),
        Some("Desk")
    );
}
