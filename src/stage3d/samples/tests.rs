use super::*;
use crate::show::Show;
use crate::stage3d::load::{Bounds, load_venue};
use crate::stage3d::scene::Paint;
use crate::stage3d::{Feed, LengthUnit, UpAxis};
use std::path::{Path as FilePath, PathBuf};

fn loaded(kind: SampleVenue) -> LoadedVenue {
    sample_venue(kind, &Venue::sample(kind)).unwrap()
}

/// Every triangle of the venue, in the world (samples have no transform).
fn triangles(v: &LoadedVenue) -> Vec<[[f32; 3]; 3]> {
    let mut out = Vec::new();
    for p in &v.parts {
        for t in p.mesh.indices.chunks(3) {
            out.push([0, 1, 2].map(|i| p.mesh.positions[t[i] as usize]));
        }
    }
    out
}

/// The expected outer size (x, y, z) of each, give or take a metre.
fn expected_size(kind: SampleVenue) -> [f32; 3] {
    match kind {
        SampleVenue::Arena => [92.6, 24.0, 122.6],
        SampleVenue::Proscenium => [28.0, 25.0 + PIT_DEPTH, 40.0],
        SampleVenue::Thrust => [37.0, 11.0, 29.0],
        SampleVenue::BlackBox => [15.0, 7.0, 12.0],
        SampleVenue::Club => [22.0, 5.0, 22.0],
        SampleVenue::Festival => [120.0, 21.2, 115.0],
        SampleVenue::Ballroom => [30.0, 6.0, 24.0],
    }
}

#[test]
fn every_sample_is_made_within_its_budget_on_the_floor_and_its_size() {
    for kind in SampleVenue::ALL {
        let v = loaded(kind);
        let name = kind.label();
        assert!(v.triangles > 100, "{name}: {}", v.triangles);
        assert!(v.triangles < 50_000, "{name}: {}", v.triangles);
        assert!(v.warnings.is_empty(), "{name}: {:?}", v.warnings);
        assert!(v.parts.len() >= 3, "{name}: one colour only");
        let b = v.bounds.unwrap();
        assert!(b.min.iter().chain(&b.max).all(|c| c.is_finite()), "{name}");
        // On the floor, nothing under it but a pit.
        assert!((b.min[1] - kind.lowest()).abs() < 1e-4, "{name}: {b:?}");
        let size = b.size();
        let want = expected_size(kind);
        for i in 0..3 {
            assert!(
                (size[i] - want[i]).abs() <= 1.0,
                "{name}: size {size:?}, expected {want:?}"
            );
        }
        // The floor faces up at y = 0 (something does).
        let floor_up = v.parts.iter().any(|p| {
            p.mesh
                .positions
                .iter()
                .zip(&p.mesh.normals)
                .any(|(q, n)| q[1].abs() < 1e-5 && n[1] > 0.99)
        });
        assert!(floor_up, "{name}: no floor");
        for (i, p) in v.parts.iter().enumerate() {
            // A colour per material, so a written .glb reads back part by part.
            assert!(
                v.parts[..i].iter().all(|q| q.paint != p.paint),
                "{name}: two materials of one colour"
            );
            let Paint::Colour(c) = p.paint else {
                panic!("{name}: a sample is plain colours")
            };
            assert!(c.iter().all(|v| (0.0..=1.0).contains(v)), "{name}");
            assert_eq!(p.mesh.positions.len(), p.mesh.normals.len());
            assert_eq!(p.mesh.positions.len(), p.mesh.uvs.len());
        }
    }
}

#[test]
fn every_triangle_has_area_and_faces_the_way_its_normals_say() {
    for kind in SampleVenue::ALL {
        let v = loaded(kind);
        for p in &v.parts {
            let m = &p.mesh;
            for t in m.indices.chunks(3) {
                let [a, b, c] = [0, 1, 2].map(|i| m.positions[t[i] as usize]);
                assert!(a.iter().chain(&b).chain(&c).all(|v| v.is_finite()));
                let g = geometry::cross(geometry::sub(b, a), geometry::sub(c, a));
                let len = geometry::dot(g, g).sqrt();
                assert!(len > 1e-6, "{}: a triangle with no area", kind.label());
                for i in t {
                    let n = m.normals[*i as usize];
                    let along = geometry::dot(g, n) / len;
                    assert!(
                        along > 0.999,
                        "{}: a normal against its triangle ({along})",
                        kind.label()
                    );
                }
            }
        }
    }
}

/// Where a ray from `from` towards `to` first meets a triangle, as a share
/// of the way (Möller-Trumbore).
fn hit(from: [f32; 3], to: [f32; 3], t: &[[f32; 3]; 3]) -> Option<f32> {
    let d = geometry::sub(to, from);
    let e1 = geometry::sub(t[1], t[0]);
    let e2 = geometry::sub(t[2], t[0]);
    let p = geometry::cross(d, e2);
    let det = geometry::dot(e1, p);
    if det.abs() < 1e-9 {
        return None;
    }
    let s = geometry::sub(from, t[0]);
    let u = geometry::dot(s, p) / det;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = geometry::cross(s, e1);
    let v = geometry::dot(d, q) / det;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let k = geometry::dot(e2, q) / det;
    (k > 0.0 && k < 1.0).then_some(k)
}

fn inside(b: &Bounds, p: [f32; 3]) -> bool {
    (0..3).all(|i| p[i] >= b.min[i] - 1e-3 && p[i] <= b.max[i] + 1e-3)
}

#[test]
fn the_starter_screens_are_in_the_venue_facing_and_seen_from_the_first_viewpoint() {
    for kind in SampleVenue::ALL {
        let name = kind.label();
        let v = loaded(kind);
        let b = v.bounds.unwrap();
        let tris = triangles(&v);
        let views = viewpoints(kind);
        assert!(views.len() >= 2, "{name}");
        for w in &views {
            assert!(inside(&b, w.eye), "{name}: {} outside", w.name);
            assert_ne!(w.eye, w.target);
            assert_eq!(w.camera().eye, w.eye);
        }
        let eye = views[0].eye;
        let list = screens(kind);
        assert!(!list.is_empty(), "{name}");
        for s in list {
            let place = s.placement.matrix();
            // The surface's front faces the audience.
            let centre = s.placement.position;
            let front = geometry::direction(&place, [0.0, 0.0, 1.0]);
            assert!(
                geometry::dot(front, geometry::sub(eye, centre)) > 0.0,
                "{name}: {} faces away",
                s.name
            );
            // Every corner is in the venue, and some piece facing the
            // viewpoint is seen from it, nothing of the venue between.
            let mut seen = false;
            for piece in geometry::surface_mesh(&s.shape) {
                let m = geometry::mul(&place, &piece.transform);
                for p in &piece.mesh.positions {
                    let w = geometry::point(&m, *p);
                    assert!(inside(&b, w), "{name}: {} sticks out at {w:?}", s.name);
                }
                // A piece's middle is its origin, facing +Z (a curve's too).
                let c = geometry::point(&m, [0.0, 0.0, 0.0]);
                let n = geometry::direction(&m, [0.0, 0.0, 1.0]);
                if geometry::dot(n, geometry::sub(eye, c)) <= 0.0 {
                    continue;
                }
                let start = [0, 1, 2].map(|i| c[i] + n[i] * 0.01);
                if tris.iter().all(|t| hit(start, eye, t).is_none()) {
                    seen = true;
                }
            }
            assert!(
                seen,
                "{name}: {} can't be seen from {}",
                s.name, views[0].name
            );
        }
    }
}

#[test]
fn a_sample_venue_is_written_by_name_and_an_unknown_one_still_loads() {
    let dir = std::env::temp_dir().join(format!("newq-samples-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("show.newq");
    let mut show = Show::default();
    show.stage3d.venue = Some(Venue::sample(SampleVenue::Arena));
    show.save(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"sample\": \"Arena\""), "{text}");
    assert!(!text.contains("\"path\""), "{text}");
    let back = Show::load(&path).unwrap();
    assert_eq!(back.stage3d, show.stage3d);
    let venue = back.stage3d.venue.unwrap();
    assert_eq!(venue.path, PathBuf::new());

    // A sample a later NewQ has reads as the default, the black box.
    std::fs::write(&path, text.replace("\"Arena\"", "\"Stadium\"")).unwrap();
    let later = Show::load(&path).unwrap();
    assert_eq!(
        later.stage3d.venue.unwrap().sample,
        Some(SampleVenue::BlackBox)
    );
    // A venue file from before samples loads as before; nothing new written.
    let file = Venue::new(dir.join("hall.glb"));
    let json = serde_json::to_string(&file).unwrap();
    assert!(!json.contains("\"sample\""), "{json}");
    let _ = std::fs::remove_dir_all(&dir);

    // Every sample's name reads back as itself.
    for kind in SampleVenue::ALL {
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(serde_json::from_str::<SampleVenue>(&json).unwrap(), kind);
        assert!(!kind.label().is_empty());
    }
}

#[test]
fn a_sample_is_loaded_in_place_of_the_file_and_only_its_placement_counts() {
    let mut venue = Venue::sample(SampleVenue::Club);
    // Units and up axis are a file's: a sample ignores them.
    venue.units = LengthUnit::Feet;
    venue.up = UpAxis::Z;
    assert_eq!(venue.matrix(), Placement::default().matrix());
    venue.placement.position = [1.0, 0.0, 2.0];
    assert_eq!(venue.matrix(), venue.placement.matrix());

    // load_venue makes it, whatever path it is given.
    let made = load_venue(FilePath::new("/nowhere/at/all.glb"), &venue).unwrap();
    assert_eq!(made.loaded_for, venue);
    assert_eq!(made.triangles, loaded(SampleVenue::Club).triangles);
    // Brightness is baked into the colours, as for a file.
    venue.brightness = 2.0;
    let bright = load_venue(FilePath::new(""), &venue).unwrap();
    let (Paint::Colour(a), Paint::Colour(b)) = (&made.parts[0].paint, &bright.parts[0].paint)
    else {
        panic!()
    };
    assert!((b[0] - a[0] * 2.0).abs() < 1e-6);
    assert!(venue.needs_reload(&made.loaded_for));
    // Another sample, or a file, is loaded again; a move isn't.
    let mut other = made.loaded_for.clone();
    other.sample = Some(SampleVenue::Arena);
    assert!(other.needs_reload(&made.loaded_for));
    let mut moved = made.loaded_for.clone();
    moved.placement.yaw = 90.0;
    assert!(!moved.needs_reload(&made.loaded_for));
    // A model is drawn in the scene like a file's, lit.
    let mut show = Show::default();
    show.stage3d.venue = Some(made.loaded_for.clone());
    let scene = show.stage3d.scene(&show, Some(&made));
    assert_eq!(scene.parts.len(), made.parts.len());
    assert!(scene.parts.iter().all(|p| p.lit));
}

#[test]
fn the_usual_screens_and_viewpoints_are_added_where_the_venue_stands() {
    let mut show = Show::default();
    let mut venue = Venue::sample(SampleVenue::Festival);
    venue.placement.position = [10.0, 0.0, -5.0];
    venue.placement.yaw = 90.0;
    show.stage3d.venue = Some(venue);
    let ids = show.add_sample_screens(SampleVenue::Festival);
    let want = screens(SampleVenue::Festival);
    assert_eq!(ids.len(), want.len());
    let mut sorted = ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), ids.len(), "ids are unique");
    assert!(ids.iter().all(|id| id.0 > show.outputs[0].id.0));
    for (id, w) in ids.iter().zip(&want) {
        let s = show.stage3d.surface(*id).unwrap();
        assert_eq!(s.name, w.name);
        assert_eq!(s.feed, Feed::None);
        // Where the venue puts it: the same as the venue's matrix times the
        // screen's own.
        let world = geometry::point(
            &show.stage3d.venue.as_ref().unwrap().matrix(),
            w.placement.position,
        );
        for (got, want) in s.placement.position.iter().zip(world) {
            assert!((got - want).abs() < 1e-4);
        }
        assert_eq!(s.placement.yaw, w.placement.yaw + 90.0);
    }
    // Viewpoints likewise, once each.
    show.stage3d.add_sample_viewpoints(SampleVenue::Festival);
    show.stage3d.add_sample_viewpoints(SampleVenue::Festival);
    let views = viewpoints(SampleVenue::Festival);
    assert_eq!(show.stage3d.viewpoints.len(), views.len());
    let m = show.stage3d.venue.as_ref().unwrap().matrix();
    let eye = geometry::point(&m, views[0].eye);
    assert!((0..3).all(|i| (show.stage3d.viewpoints[0].eye[i] - eye[i]).abs() < 1e-4));
    // And the whole show round-trips.
    let json = serde_json::to_string(&show).unwrap();
    let back: Show = serde_json::from_str(&json).unwrap();
    assert_eq!(back.stage3d, show.stage3d);
}

#[test]
fn a_sample_written_as_glb_reads_back_the_same() {
    let dir = std::env::temp_dir().join(format!("newq-glb-out-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for kind in SampleVenue::ALL {
        let bytes = sample_glb(kind);
        assert_eq!(&bytes[..4], b"glTF");
        assert_eq!(bytes.len() % 4, 0);
        let len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        assert_eq!(len, bytes.len());
        let path = dir.join(format!("{kind:?}.glb"));
        std::fs::write(&path, &bytes).unwrap();
        let back = load_venue(&path, &Venue::new(path.clone())).unwrap();
        let made = loaded(kind);
        assert!(back.warnings.is_empty(), "{:?}", back.warnings);
        assert_eq!(back.triangles, made.triangles, "{}", kind.label());
        assert_eq!(back.bounds, made.bounds, "{}", kind.label());
        assert_eq!(back.parts.len(), made.parts.len());
        // The reader may order the materials its own way: match by colour
        // (every sample's colours are its own per material).
        for b in &made.parts {
            let a = back.parts.iter().find(|a| a.paint == b.paint).unwrap();
            assert_eq!(a.mesh.positions, b.mesh.positions);
            assert_eq!(a.mesh.indices, b.mesh.indices);
            for (n, m) in a.mesh.normals.iter().zip(&b.mesh.normals) {
                assert!((0..3).all(|i| (n[i] - m[i]).abs() < 1e-5));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ----- Pictures, to look at by eye ---------------------------------------------

/// Draw each sample from its viewpoints and from above, with a small
/// software rasteriser, into `NEWQ_SAMPLE_PICTURES` (a folder): to see that
/// an arena looks like one. Starter screens are drawn light blue.
#[test]
#[ignore]
fn draw_the_samples() {
    let Some(out) = std::env::var_os("NEWQ_SAMPLE_PICTURES") else {
        eprintln!("NEWQ_SAMPLE_PICTURES not set");
        return;
    };
    let out = PathBuf::from(out);
    std::fs::create_dir_all(&out).unwrap();
    for kind in SampleVenue::ALL {
        let mut show = Show::default();
        show.stage3d.venue = Some(Venue::sample(kind));
        show.add_sample_screens(kind);
        let made = loaded(kind);
        let scene = show.stage3d.scene(&show, Some(&made));
        let b = made.bounds.unwrap();
        eprintln!(
            "{}: {} triangles, {} materials, {:?} m",
            kind.label(),
            made.triangles,
            made.parts.len(),
            b.size()
        );
        let mut views = viewpoints(kind);
        let mid = [0, 1, 2].map(|i| (b.min[i] + b.max[i]) / 2.0);
        let size = b.size();
        let reach = size[0].max(size[2]);
        views.push(Viewpoint {
            name: "Overview".into(),
            eye: [
                mid[0] + reach * 0.55,
                b.max[1] + reach * 0.5,
                mid[2] + reach * 0.75,
            ],
            target: [mid[0], 0.0, mid[2]],
            fov_deg: 50.0,
        });
        for (i, v) in views.iter().enumerate() {
            let img = raster(&scene, v, 960, 540);
            let file = out.join(format!("{:?}-{i}.png", kind).to_lowercase());
            img.save(&file).unwrap();
            eprintln!("{} ({}): {}", kind.label(), v.name, file.display());
        }
    }
}

fn raster(scene: &crate::stage3d::scene::Scene, v: &Viewpoint, w: u32, h: u32) -> image::RgbImage {
    let f = geometry::normalize(geometry::sub(v.target, v.eye));
    let r = geometry::normalize(geometry::cross(f, [0.0, 1.0, 0.0]));
    let u = geometry::cross(r, f);
    let focal = (h as f32 / 2.0) / (v.fov_deg.to_radians() / 2.0).tan();
    let mut depth = vec![f32::INFINITY; (w * h) as usize];
    let mut img = image::RgbImage::from_pixel(w, h, image::Rgb([40, 44, 52]));
    let light = geometry::normalize([0.3, 1.0, 0.5]);
    for part in &scene.parts {
        let colour = match part.paint {
            Paint::Colour(c) if part.lit => c,
            _ => [0.5, 0.75, 1.0, 1.0],
        };
        let m = &part.mesh;
        let world: Vec<[f32; 3]> = m
            .positions
            .iter()
            .map(|p| geometry::point(&part.transform, *p))
            .collect();
        for t in m.indices.chunks(3) {
            let tri = [0, 1, 2].map(|i| world[t[i] as usize]);
            let n = geometry::normalize(geometry::cross(
                geometry::sub(tri[1], tri[0]),
                geometry::sub(tri[2], tri[0]),
            ));
            let shade = if part.lit {
                0.35 + 0.65 * geometry::dot(n, light).abs()
            } else {
                1.0
            };
            // To the camera, clipped crudely: a triangle with a corner
            // behind the eye is left out.
            let cam = tri.map(|p| {
                let d = geometry::sub(p, v.eye);
                [
                    geometry::dot(d, r),
                    geometry::dot(d, u),
                    geometry::dot(d, f),
                ]
            });
            if cam.iter().any(|c| c[2] < 0.1) {
                continue;
            }
            let s = cam.map(|c| {
                [
                    w as f32 / 2.0 + focal * c[0] / c[2],
                    h as f32 / 2.0 - focal * c[1] / c[2],
                    c[2],
                ]
            });
            let x0 = s
                .iter()
                .map(|p| p[0])
                .fold(f32::INFINITY, f32::min)
                .max(0.0) as i32;
            let x1 = s
                .iter()
                .map(|p| p[0])
                .fold(f32::NEG_INFINITY, f32::max)
                .min(w as f32 - 1.0) as i32;
            let y0 = s
                .iter()
                .map(|p| p[1])
                .fold(f32::INFINITY, f32::min)
                .max(0.0) as i32;
            let y1 = s
                .iter()
                .map(|p| p[1])
                .fold(f32::NEG_INFINITY, f32::max)
                .min(h as f32 - 1.0) as i32;
            let area = (s[1][0] - s[0][0]) * (s[2][1] - s[0][1])
                - (s[2][0] - s[0][0]) * (s[1][1] - s[0][1]);
            if area.abs() < 1e-6 {
                continue;
            }
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                    let w0 =
                        ((s[1][0] - px) * (s[2][1] - py) - (s[2][0] - px) * (s[1][1] - py)) / area;
                    let w1 =
                        ((s[2][0] - px) * (s[0][1] - py) - (s[0][0] - px) * (s[2][1] - py)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let z = 1.0 / (w0 / s[0][2] + w1 / s[1][2] + w2 / s[2][2]);
                    let k = (y as u32 * w + x as u32) as usize;
                    if z < depth[k] {
                        depth[k] = z;
                        let px = colour.map(|c| {
                            // Linear to sRGB, roughly; lifted so dark rooms read.
                            ((c * shade * 1.6).clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0) as u8
                        });
                        img.put_pixel(x as u32, y as u32, image::Rgb([px[0], px[1], px[2]]));
                    }
                }
            }
        }
    }
    img
}
