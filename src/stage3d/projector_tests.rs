use super::*;
use crate::stage3d::{Feed, Stage3d, Surface, SurfaceId, WHOLE};

const HD: f32 = 16.0 / 9.0;

fn close(a: [f32; 3], b: [f32; 3], tol: f32) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() <= tol)
}

fn projector(position: [f32; 3]) -> Projector {
    let mut p = Projector::new(ProjectorId(1), "P".into());
    p.placement.position = position;
    p
}

fn flat(w: f32, h: f32, placement: Placement) -> Surface {
    Surface {
        id: SurfaceId(5),
        name: "Screen".into(),
        visible: true,
        placement,
        shape: SurfaceShape::Flat {
            width: w,
            height: h,
        },
        feed: Feed::None,
        gain: 1.0,
        rear: false,
    }
}

/// The scene of just these screens (what a footprint is found on).
fn scene_of(surfaces: Vec<Surface>) -> Scene {
    let stage = Stage3d {
        surfaces,
        ..Stage3d::default()
    };
    stage.scene(&Show::default(), None)
}

fn all(_: usize, _: &Part) -> bool {
    true
}

#[test]
fn the_lens_looks_down_its_own_minus_z_with_up_its_pictures_top() {
    let mut p = projector([1.0, 2.0, 3.0]);
    assert!(close(p.axis(), [0.0, 0.0, -1.0], 1e-6));
    assert!(close(p.up(), [0.0, 1.0, 0.0], 1e-6));
    assert!(close(p.right(), [1.0, 0.0, 0.0], 1e-6));
    // Positive pitch tips the lens up; positive yaw turns it to the left
    // (+Z towards +X, so -Z towards -X).
    p.placement.pitch = 30.0;
    assert!(p.axis()[1] > 0.49 && p.axis()[1] < 0.51);
    p.placement.pitch = 0.0;
    p.placement.yaw = 90.0;
    assert!(close(p.axis(), [-1.0, 0.0, 0.0], 1e-6));
    // A point straight ahead is in the middle of the picture, at its depth.
    let p = projector([0.0; 3]);
    let ([u, v], depth) = p.in_picture([0.0, 0.0, -7.0], HD).unwrap();
    assert!((u - 0.5).abs() < 1e-6 && (v - 0.5).abs() < 1e-6 && (depth - 7.0).abs() < 1e-5);
    assert!(p.in_picture([0.0, 0.0, 7.0], HD).is_none(), "behind");
}

#[test]
fn the_throw_ratio_is_distance_over_width() {
    let mut p = projector([0.0; 3]);
    p.lens.throw = 2.0;
    let (w, h) = p.picture_at(10.0, HD);
    assert!((w - 5.0).abs() < 1e-5 && (h - 2.8125).abs() < 1e-5);
    assert!((p.distance_for(5.0) - 10.0).abs() < 1e-5);
    // A ray through the picture's right edge, 10 m out, is 2.5 m across.
    let r = p.ray(1.0, 0.5, HD);
    let t = 10.0 / -r.dir[2];
    assert!((r.dir[0] * t - 2.5).abs() < 1e-4);
    // Silly numbers are kept within bounds; a range written backwards is put
    // right before it is used.
    p.lens.throw = f32::NAN;
    assert_eq!(p.lens.throw(), DEFAULT_THROW);
    p.lens.throw = 0.0;
    assert_eq!(p.lens.throw(), MIN_THROW);
    p.lens.zoom = Some([4.0, 2.0]);
    assert_eq!(p.lens.zoom(), Some((2.0, 4.0)));
    assert_eq!(p.lens.nearest(9.0), 4.0);
    assert!(p.lens.reaches(3.0) && !p.lens.reaches(1.0));
    p.lens.zoom = Some([f32::NAN, 2.0]);
    assert_eq!(p.lens.zoom(), None);
    p.lumens = -5.0;
    assert_eq!(p.lumens(), 0.0);
    p.lumens = f32::INFINITY;
    assert_eq!(p.lumens(), DEFAULT_LUMENS);
    p.lens.shift = [9.0, f32::NAN];
    assert_eq!(p.lens.shift(), [MAX_SHIFT, 0.0]);
}

#[test]
fn half_a_picture_of_shift_up_puts_its_bottom_edge_on_the_axis() {
    let mut p = projector([0.0; 3]);
    p.lens.shift = [0.0, 0.5];
    let f = p.frustum(HD);
    assert!(f.bottom.abs() < 1e-6);
    let bottom_middle = p.ray(0.5, 1.0, HD);
    assert!(close(bottom_middle.dir, [0.0, 0.0, -1.0], 1e-6));
    let ([u, v], _) = p.in_picture([0.0, 0.0, -4.0], HD).unwrap();
    assert!((u - 0.5).abs() < 1e-5 && (v - 1.0).abs() < 1e-5);
    // A quarter to the right: the axis a quarter of the way in from the left.
    p.lens.shift = [0.25, 0.0];
    let ([u, v], _) = p.in_picture([0.0, 0.0, -4.0], HD).unwrap();
    assert!((u - 0.25).abs() < 1e-5 && (v - 0.5).abs() < 1e-5);
}

#[test]
fn the_look_through_camera_sees_the_lenss_picture_corner_for_corner() {
    let mut p = projector([2.0, 5.0, 14.0]);
    p.lens.throw = 1.8;
    p.lens.shift = [0.1, 0.3];
    p.placement.yaw = 8.0;
    p.placement.pitch = -12.0;
    p.placement.roll = 3.0;
    let size = (1920, 1080);
    let cam = p.camera(HD);
    for (u, v) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.3, 0.7)] {
        let r = p.ray(u, v, HD);
        let at = [
            r.origin[0] + r.dir[0] * 9.0,
            r.origin[1] + r.dir[1] * 9.0,
            r.origin[2] + r.dir[2] * 9.0,
        ];
        let px = cam.project(at, size).unwrap();
        assert!(
            (px[0] - u * 1920.0).abs() < 0.5 && (px[1] - v * 1080.0).abs() < 0.5,
            "({u}, {v}) -> {px:?}"
        );
    }
}

#[test]
fn fit_to_a_flat_screen_lands_the_pictures_corners_on_its_corners() {
    let placements = [
        Placement {
            position: [0.0, 3.0, -5.0],
            ..Placement::default()
        },
        Placement {
            position: [4.0, 6.0, -8.0],
            yaw: 30.0,
            pitch: 10.0,
            roll: 0.0,
        },
        Placement {
            position: [-3.0, 2.0, 1.0],
            yaw: -50.0,
            pitch: -5.0,
            roll: 4.0,
        },
    ];
    for (k, place) in placements.into_iter().enumerate() {
        for rear in [false, true] {
            let mut s = flat(6.0, 3.375, place);
            s.rear = rear;
            // (A touch bigger to be found on: a ray down the exact edge may
            // pass either side of it.)
            let scene = scene_of(vec![flat(6.01, 3.385, place)]);
            let mut p = projector([0.0; 3]);
            p.lens.throw = 1.5;
            p.lens.shift = [0.05, 0.2];
            p.placement = p.fit_to(&s, HD).unwrap();
            let fp = p.footprint(&scene, &all, HD, 4);
            let m = s.placement.matrix();
            // Seen from the projector's side, its top-left corner.
            let x = if rear { 3.0 } else { -3.0 };
            let want = [
                geometry::point(&m, [x, 1.6875, 0.0]),
                geometry::point(&m, [-x, 1.6875, 0.0]),
                geometry::point(&m, [-x, -1.6875, 0.0]),
                geometry::point(&m, [x, -1.6875, 0.0]),
            ];
            for (i, w) in want.iter().enumerate() {
                let got = fp.corners[i].unwrap_or_else(|| panic!("{k} {rear}: corner {i}"));
                assert!(close(got, *w, 0.01), "{k} {rear}: corner {i} {got:?} {w:?}");
            }
            // Square on and 9 m away (6 m × 1.5).
            let d = p.to_lens(geometry::point(&m, [0.0; 3]));
            assert!((-d[2] - 9.0).abs() < 1e-3, "{k} {rear}: {d:?}");
            let on = p.on_flat(&s, HD).unwrap();
            assert!(on.fills > 0.999, "{}", on.fills);
            assert!(on.spill.iter().all(|v| *v < 1e-3), "{:?}", on.spill);
        }
    }
}

#[test]
fn a_screen_of_another_shape_is_filled_without_spilling() {
    // A 4:3 screen: the 16:9 picture fills its width; a tall one, its height.
    let s = flat(4.0, 3.0, Placement::default());
    let mut p = projector([0.0; 3]);
    p.placement = p.fit_to(&s, HD).unwrap();
    let on = p.on_flat(&s, HD).unwrap();
    assert!((on.size.0 - 4.0).abs() < 1e-3 && (on.size.1 - 2.25).abs() < 1e-3);
    assert!((on.fills - 0.75).abs() < 1e-3);
    let tall = flat(2.0, 3.0, Placement::default());
    p.placement = p.fit_to(&tall, HD).unwrap();
    let on = p.on_flat(&tall, HD).unwrap();
    assert!((on.size.0 - 2.0).abs() < 1e-3, "{:?}", on.size);
    // A curved wall: the projector on its axis, in front.
    let curve = Surface {
        shape: SurfaceShape::Curved {
            width: 12.0,
            radius: 8.0,
            height: 3.4,
            convex: false,
        },
        ..flat(1.0, 1.0, Placement::default())
    };
    let placed = p.fit_to(&curve, HD).unwrap();
    assert!(placed.position[0].abs() < 1e-3 && placed.position[2] > 5.0);
    let unknown = Surface {
        shape: SurfaceShape::Unknown(serde_json::json!({"Dome": {}})),
        ..flat(1.0, 1.0, Placement::default())
    };
    assert!(p.fit_to(&unknown, HD).is_none());
}

#[test]
fn spill_and_fill_on_a_flat_screen_are_measured() {
    // Square on, 12 m back with a throw of 1.5: a picture 8 m by 4.5 m on a
    // 4 m by 2.25 m screen spills 2 m left and right, 1.125 m top and bottom.
    let s = flat(4.0, 2.25, Placement::default());
    let p = projector([0.0, 0.0, 12.0]);
    let on = p.on_flat(&s, HD).unwrap();
    assert!((on.fills - 1.0).abs() < 1e-4);
    for (got, want) in on.spill.iter().zip([2.0, 2.0, 1.125, 1.125]) {
        assert!((got - want).abs() < 1e-3, "{:?}", on.spill);
    }
    // 3 m back: 2 m by 1.125 m, a quarter of the screen, no spill.
    let p = projector([0.0, 0.0, 3.0]);
    let on = p.on_flat(&s, HD).unwrap();
    assert!((on.fills - 0.25).abs() < 1e-3, "{}", on.fills);
    assert!(on.spill.iter().all(|v| *v == 0.0));
    // Moved 1.5 m right: a quarter of it (0.5 m) over the right edge.
    let p = projector([1.5, 0.0, 3.0]);
    let on = p.on_flat(&s, HD).unwrap();
    assert!((on.spill[1] - 0.5).abs() < 1e-3, "{:?}", on.spill);
    assert!((on.fills - 0.1875).abs() < 1e-3, "{}", on.fills);
    // Turned away: its picture never reaches the screen's plane.
    let mut away = projector([0.0, 0.0, 3.0]);
    away.placement.yaw = 180.0;
    assert!(away.on_flat(&s, HD).is_none());
}

#[test]
fn pointing_at_a_spot_puts_the_pictures_middle_there_even_when_shifted() {
    for shift in [[0.0, 0.0], [0.0, 0.5], [0.3, -0.4]] {
        let mut p = projector([3.0, 8.0, 15.0]);
        p.lens.shift = shift;
        p.placement.roll = 2.0;
        let target = [-1.0, 2.5, -6.0];
        assert!(p.aim_at(target, HD));
        let r = p.ray(0.5, 0.5, HD);
        let to = geometry::normalize(geometry::sub(target, r.origin));
        assert!(close(r.dir, to, 1e-4), "{shift:?}: {:?} {to:?}", r.dir);
        assert_eq!(p.placement.roll, 2.0, "the roll is kept");
    }
    let mut p = projector([1.0; 3]);
    assert!(!p.aim_at([1.0; 3], HD), "not at itself");
}

#[test]
fn which_lens_from_here() {
    // 12 m from a 4 m wide 16:9 screen: a throw of 3, a long zoom.
    let s = flat(4.0, 2.25, Placement::default());
    let p = projector([0.0, 0.0, 12.0]);
    let t = p.throw_for(&s, HD).unwrap();
    assert!((t - 3.0).abs() < 1e-4, "{t}");
    assert_eq!(LensPreset::for_throw(t), Some(LensPreset::LongZoom));
    assert_eq!(LensPreset::for_throw(0.26), Some(LensPreset::UltraShort));
    assert_eq!(LensPreset::for_throw(1.0), None, "between the usual ones");
    assert_eq!(LensPreset::for_throw(f32::NAN), None);
    // A preset set to a throw it can reach keeps it; a fixed lens is itself.
    let lens = LensPreset::LongZoom.lens(Some(t));
    assert_eq!((lens.throw, lens.zoom), (3.0, Some([2.0, 4.0])));
    assert_eq!(LensPreset::of(&lens), Some(LensPreset::LongZoom));
    let fixed = LensPreset::Short.lens(Some(3.0));
    assert_eq!((fixed.throw, fixed.zoom), (0.5, None));
    assert_eq!(LensPreset::of(&fixed), Some(LensPreset::Short));
    assert_eq!(LensPreset::of(&Lens::default()), None);
}

#[test]
fn a_pixel_is_the_picture_width_over_its_pixels() {
    // A picture 6 m wide over 1920 pixels: 3.125 mm a pixel.
    let mut p = projector([0.0, 0.0, 9.0]);
    p.lens.throw = 1.5;
    let px = p
        .pixel_size_at([0.0; 3], Some([0.0, 0.0, 1.0]), (1920, 1080), HD)
        .unwrap();
    assert!((px - 0.003125).abs() < 1e-6, "{px}");
    assert_eq!(p.pixel_size_at([0.0; 3], None, (1920, 1080), HD), Some(px));
    // On a wall turned 60°, a pixel covers twice the area: √2 the side.
    let n = [60f32.to_radians().sin(), 0.0, 60f32.to_radians().cos()];
    let slant = p
        .pixel_size_at([0.0; 3], Some(n), (1920, 1080), HD)
        .unwrap();
    assert!((slant / px - 2f32.sqrt()).abs() < 1e-3, "{}", slant / px);
    assert!(
        p.pixel_size_at([0.0, 0.0, 20.0], None, (1920, 1080), HD)
            .is_none()
    );
    // Its pixels: the display's where known, else its own, else 1920 × 1080.
    let mut q = projector([0.0; 3]);
    assert_eq!(q.pixels(None), DEFAULT_PIXELS);
    q.resolution = Some((1920, 1200));
    assert_eq!(q.pixels(None), (1920, 1200));
    assert_eq!(q.pixels(Some((3840, 2160))), (3840, 2160));
    q.resolution = Some((0, 5));
    assert_eq!(q.pixels(None), DEFAULT_PIXELS);
    assert!((q.aspect((1920, 1200)) - 1.6).abs() < 1e-6);
    q.lens.aspect = Some(2.39);
    assert_eq!(q.aspect((1920, 1080)), 2.39);
}

#[test]
fn the_cone_goes_to_where_the_picture_lands_or_out_into_the_room() {
    let s = flat(6.0, 3.375, Placement::default());
    let scene = scene_of(vec![flat(6.01, 3.385, Placement::default())]);
    let mut p = projector([0.0; 3]);
    p.placement = p.fit_to(&s, HD).unwrap();
    let fp = p.footprint(&scene, &all, HD, 8);
    assert_eq!(fp.outline.len(), 1, "one piece all the way round");
    assert_eq!(fp.outline[0].len(), 33);
    assert!(
        fp.outline[0].iter().all(|q| q[2].abs() < 1e-3),
        "on the screen"
    );
    let cone = p.cone(&fp, HD, 20.0);
    assert_eq!(cone.len(), 4, "four edges, the outline is the far end");
    // Turned away from everything: no outline, a cone 20 m long.
    p.placement.yaw += 180.0;
    let fp = p.footprint(&scene, &all, HD, 8);
    assert!(fp.outline.is_empty() && fp.corners.iter().all(Option::is_none));
    let cone = p.cone(&fp, HD, 20.0);
    assert_eq!(cone.len(), 8);
    for [a, b] in &cone[..4] {
        let d = geometry::sub(*b, *a);
        assert!((geometry::dot(d, d).sqrt() - 20.0).abs() < 1e-3);
    }
}

#[test]
fn projectors_are_written_only_with_what_isnt_their_default() {
    let mut show = Show::default();
    let display = show.outputs[0].id;
    let id = show.add_projector();
    let json = serde_json::to_string(&show.stage3d).unwrap();
    for absent in [
        "visible",
        "lens",
        "lumens",
        "display",
        "resolution",
        "model",
        "gain",
        "rear",
    ] {
        assert!(!json.contains(absent), "{absent} in {json}");
    }
    assert!(json.contains("\"name\":\"Projector 1\""), "{json}");
    let p = show.projector_mut(id).unwrap();
    p.lens.throw = 2.5;
    p.lens.zoom = Some([2.0, 4.0]);
    p.lumens = 12_000.0;
    p.display = Some(display);
    let json = serde_json::to_string(&show).unwrap();
    let back: Show = serde_json::from_str(&json).unwrap();
    assert_eq!(back.stage3d.projectors, show.stage3d.projectors);
    let stage = serde_json::to_string(&back.stage3d).unwrap();
    assert!(
        !stage.contains("shift") && !stage.contains("gamma"),
        "{stage}"
    );
    // Ids are the show's.
    assert!(id.0 > display.0);
    assert_eq!(show.add_projector(), ProjectorId(id.0 + 1));
    assert_eq!(show.stage3d.projectors[1].name, "Projector 2");
}

/// A stage as 0.25 writes it (fields in its order, no projectors).
const STAGE_025: &str = r#"{"venue":{"sample":"BlackBox"},"surfaces":[{"id":4,"name":"Screen 1","placement":{"position":[0.0,1.125,0.0]},"shape":{"Flat":{"width":4.0,"height":2.25}},"feed":{"Display":{"output":1}}},{"id":5,"name":"Wall","visible":false,"shape":{"Curved":{"width":12.0,"radius":8.0,"height":3.4}}}],"viewpoints":[{"name":"Desk","eye":[0.0,2.0,15.0],"target":[0.0,2.0,0.0]}],"units":"Feet"}"#;

#[test]
fn a_025_stage_is_written_back_byte_for_byte() {
    let stage: Stage3d = serde_json::from_str(STAGE_025).unwrap();
    assert!(stage.projectors.is_empty());
    assert_eq!(stage.surfaces[0].gain, 1.0);
    assert!(!stage.surfaces[1].rear);
    assert_eq!(serde_json::to_string(&stage).unwrap(), STAGE_025);
}

#[test]
fn a_later_newqs_projector_keeps_what_this_one_doesnt_know() {
    let mut show = Show::default();
    let id = show.add_projector();
    let mut json: serde_json::Value = serde_json::to_value(&show).unwrap();
    let p = &mut json["stage3d"]["projectors"][0];
    p["mapped"] = true.into();
    p["line_up"] = serde_json::json!({ "points": [[1.0, 2.0]] });
    p["lens"] = serde_json::json!({ "throw": 2.0, "distortion": [0.1, 0.0] });
    // A second one written with almost nothing: every field has a default.
    json["stage3d"]["projectors"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({ "hologram": 1 }));
    let text = serde_json::to_string(&json).unwrap();
    let dir = std::env::temp_dir().join(format!("newq-projectors-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("later.newq");
    std::fs::write(&path, &text).unwrap();
    let back = Show::load(&path).expect("a later NewQ's projectors load");
    let ps = &back.stage3d.projectors;
    assert_eq!(ps.len(), 2);
    assert_eq!(ps[0].id, id);
    assert_eq!(ps[0].lens.throw, 2.0);
    // The bare one got an id of its own and a default lens.
    assert_ne!(ps[1].id, ProjectorId(0));
    assert_ne!(ps[1].id, id);
    assert!(back.next_id > ps[1].id.0);
    assert_eq!(ps[1].lumens, DEFAULT_LUMENS);
    // Saved again, the later fields are written as they were.
    back.save(&path).unwrap();
    let again: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let p = &again["stage3d"]["projectors"][0];
    assert_eq!(p["mapped"], true);
    assert_eq!(p["line_up"]["points"][0][1], 2.0);
    assert_eq!(p["lens"]["distortion"][0], 0.1);
    assert_eq!(again["stage3d"]["projectors"][1]["hologram"], 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_display_is_one_projector_at_most_and_never_a_sub_display() {
    let mut show = Show::default();
    let d = show.outputs[0].id;
    let a = show.add_projector();
    let b = show.add_projector();
    show.projector_mut(a).unwrap().display = Some(d);
    assert!(show.projector_display_refused(d, a).is_none());
    assert!(show.projector_display_refused(d, b).is_some());
    // A hand-edited file giving it to both: the second lets it go.
    show.projector_mut(b).unwrap().display = Some(d);
    show.sanitize_stage();
    assert_eq!(show.projector(a).unwrap().display, Some(d));
    assert_eq!(show.projector(b).unwrap().display, None);
    // A display that shows the stage can't be one.
    show.outputs[0].source = Some(crate::stage3d::StageSource::default());
    assert!(show.projector_display_refused(d, a).is_some());
    show.sanitize_stage();
    assert_eq!(show.projector(a).unwrap().display, None);
    show.outputs[0].source = None;
    // A display removed leaves its projector with none.
    let e = show.add_display();
    show.projector_mut(a).unwrap().display = Some(e);
    show.remove_output(e);
    assert_eq!(show.projector(a).unwrap().display, None);
}

#[test]
fn projectors_bodies_come_after_the_screens_and_stay_behind_the_lens() {
    let mut show = Show::default();
    let s = show.add_surface(None);
    show.stage3d.surface_mut(s).unwrap().feed = Feed::Display {
        output: show.outputs[0].id,
        region: WHOLE,
    };
    let a = show.add_projector();
    let b = show.add_projector();
    show.projector_mut(b).unwrap().placement.position = [4.0, 2.0, 10.0];
    let scene = show.stage3d.scene(&show, None);
    assert_eq!(scene.parts.len(), 3);
    assert_eq!(show.stage3d.projectors_from(&scene), 1);
    assert_eq!(show.stage3d.part_surface(&scene, 0), Some(s));
    assert_eq!(show.stage3d.part_surface(&scene, 1), None);
    assert_eq!(show.stage3d.part_projector(&scene, 1), Some(a));
    assert_eq!(show.stage3d.part_projector(&scene, 2), Some(b));
    assert_eq!(show.stage3d.part_projector(&scene, 0), None);
    assert!(scene.parts[1].lit);
    // Its box is behind the lens: looking through it never sees inside.
    let body = crate::stage3d::projector_body();
    assert!(body.positions.iter().all(|p| p[2] >= 0.03));
    // A hidden one isn't drawn.
    show.projector_mut(a).unwrap().visible = false;
    let scene = show.stage3d.scene(&show, None);
    assert_eq!(scene.parts.len(), 2);
    assert_eq!(show.stage3d.part_projector(&scene, 1), Some(b));
}
