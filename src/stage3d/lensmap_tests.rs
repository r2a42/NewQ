use super::*;
use crate::mapped::{unpack_how, unpack_place};
use crate::stage3d::scene::Catch;
use crate::stage3d::{Feed, ProjectorId, SurfaceId, SurfaceShape};
use std::sync::Arc;

const WHOLE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

/// A show with one flat 4 m by 2.25 m screen at the origin facing +Z, fed by
/// display A, and a projector square on to it `distance` metres out whose
/// picture is exactly the screen.
pub(crate) fn flat_show() -> (Show, OutputId, SurfaceId, ProjectorId) {
    let mut show = Show::default();
    let a = show.outputs[0].id;
    let s = show.add_surface(None);
    show.stage3d.surface_mut(s).unwrap().feed = Feed::Display {
        output: a,
        region: WHOLE,
    };
    let p = show.add_projector();
    let pr = show.projector_mut(p).unwrap();
    let d = pr.distance_for(4.0);
    pr.placement.position = [0.0, 0.0, d];
    pr.lens.aspect = Some(16.0 / 9.0);
    (show, a, s, p)
}

pub(crate) fn still_of(show: &Show) -> Scene {
    still_scene(show, &show.stage3d.scene(show, None))
}

fn at(map: &CpuMap, x: u32, y: u32) -> (Option<u8>, [f32; 2]) {
    let i = (y * map.size.0 + x) as usize;
    (unpack_how(map.how[i]).feed, unpack_place(map.place[i]))
}

fn close(a: [f32; 2], b: [f32; 2], within: f32) -> bool {
    (a[0] - b[0]).abs() <= within && (a[1] - b[1]).abs() <= within
}

#[test]
fn a_projector_square_on_to_a_screen_it_fills_sees_its_picture_pixel_for_pixel() {
    let (show, a, _, p) = flat_show();
    let size = (64, 36);
    let map = reference(
        &still_of(&show),
        show.projector(p).unwrap(),
        size,
        &feeds_for(&show, p),
    );
    assert_eq!(map.feeds, vec![a]);
    assert_eq!(map.size, size);
    for y in 0..size.1 {
        for x in 0..size.0 {
            let (feed, place) = at(&map, x, y);
            assert_eq!(feed, Some(0), "({x}, {y})");
            let want = [(x as f32 + 0.5) / 64.0, (y as f32 + 0.5) / 36.0];
            assert!(close(place, want, 1e-4), "({x}, {y}): {place:?} {want:?}");
        }
    }
}

#[test]
fn a_feeds_region_is_where_its_places_are() {
    let (mut show, a, s, p) = flat_show();
    show.stage3d.surface_mut(s).unwrap().feed = Feed::Display {
        output: a,
        region: [0.5, 0.25, 0.5, 0.5],
    };
    let map = reference(
        &still_of(&show),
        show.projector(p).unwrap(),
        (16, 9),
        &feeds_for(&show, p),
    );
    let (_, place) = at(&map, 0, 0);
    assert!(close(place, [0.5 + 0.5 / 32.0, 0.25 + 0.5 / 18.0], 1e-4));
    let (_, place) = at(&map, 15, 8);
    assert!(close(place, [1.0 - 0.5 / 32.0, 0.75 - 0.5 / 18.0], 1e-4));
}

#[test]
fn a_projector_twice_as_far_away_sees_the_screen_in_the_middle_of_its_picture() {
    let (mut show, _, _, p) = flat_show();
    let pr = show.projector_mut(p).unwrap();
    pr.placement.position[2] *= 2.0;
    let map = reference(
        &still_of(&show),
        show.projector(p).unwrap(),
        (32, 18),
        &feeds_for(&show, p),
    );
    // The outer quarter all round is black; the middle half is the screen.
    assert_eq!(at(&map, 0, 0).0, None);
    assert_eq!(at(&map, 7, 5).0, None);
    assert_eq!(at(&map, 8, 3).0, None);
    assert_eq!(at(&map, 31, 17).0, None);
    let (feed, place) = at(&map, 8, 5);
    assert_eq!(feed, Some(0));
    assert!(close(place, [0.5 / 16.0, 1.0 / 9.0], 1e-4), "{place:?}");
}

#[test]
fn a_front_screen_seen_from_behind_shows_nothing_and_a_rear_screen_shows_its_picture_mirrored() {
    let (mut show, _, s, p) = flat_show();
    let pr = show.projector_mut(p).unwrap();
    pr.placement.position[2] = -pr.placement.position[2];
    pr.placement.yaw = 180.0;
    let map = reference(
        &still_of(&show),
        show.projector(p).unwrap(),
        (16, 9),
        &feeds_for(&show, p),
    );
    assert!((0..16 * 9).all(|i| unpack_how(map.how[i]).feed.is_none()));
    show.stage3d.surface_mut(s).unwrap().rear = true;
    let map = reference(
        &still_of(&show),
        show.projector(p).unwrap(),
        (16, 9),
        &feeds_for(&show, p),
    );
    // Seen from behind, the picture's left is the projector's right.
    let (feed, place) = at(&map, 0, 0);
    assert_eq!(feed, Some(0));
    assert!(
        close(place, [1.0 - 0.5 / 16.0, 0.5 / 9.0], 1e-4),
        "{place:?}"
    );
}

/// A box, lit (as the venue's parts are), `half` metres each way from
/// `middle`.
pub(crate) fn box_part(middle: [f32; 3], half: [f32; 3]) -> Part {
    let mut positions = Vec::new();
    for i in 0..8 {
        positions.push([
            middle[0] + if i & 1 == 0 { -half[0] } else { half[0] },
            middle[1] + if i & 2 == 0 { -half[1] } else { half[1] },
            middle[2] + if i & 4 == 0 { -half[2] } else { half[2] },
        ]);
    }
    let faces: [[u32; 4]; 6] = [
        [0, 1, 3, 2],
        [4, 6, 7, 5],
        [0, 4, 5, 1],
        [2, 3, 7, 6],
        [0, 2, 6, 4],
        [1, 5, 7, 3],
    ];
    let indices = faces
        .iter()
        .flat_map(|f| [f[0], f[1], f[2], f[0], f[2], f[3]])
        .collect();
    let mesh = crate::render3d::Mesh {
        normals: vec![[0.0, 1.0, 0.0]; 8],
        uvs: vec![[0.0, 0.0]; 8],
        positions,
        indices,
    };
    Part {
        mesh_key: mesh.content_key(),
        mesh: Arc::new(mesh),
        transform: crate::stage3d::scene::IDENTITY,
        paint: Paint::Colour([0.5, 0.5, 0.5, 1.0]),
        lit: true,
        catch: Catch::default(),
    }
}

#[test]
fn a_box_in_the_way_shows_nothing_and_a_screen_on_a_wall_shows_its_picture() {
    let (show, _, _, p) = flat_show();
    let mut scene = still_of(&show);
    // A box in front of the screen's middle, half a metre out.
    scene
        .parts
        .insert(0, box_part([0.0, 0.0, 0.5], [0.5, 0.5, 0.25]));
    let map = reference(
        &scene,
        show.projector(p).unwrap(),
        (32, 18),
        &feeds_for(&show, p),
    );
    assert_eq!(at(&map, 16, 9).0, None, "the box hides the middle");
    assert_eq!(at(&map, 0, 0).0, Some(0), "not the corner");
    // A wall the screen is hung on, its face where the screen is: the
    // screen wins.
    let mut scene = still_of(&show);
    scene
        .parts
        .insert(0, box_part([0.0, 0.0, -0.5], [5.0, 5.0, 0.5]));
    let map = reference(
        &scene,
        show.projector(p).unwrap(),
        (32, 18),
        &feeds_for(&show, p),
    );
    assert!((0..32 * 18).all(|i| unpack_how(map.how[i]).feed == Some(0)));
}

#[test]
fn an_unfed_screen_is_in_the_way_and_shows_nothing() {
    let (mut show, _, _, p) = flat_show();
    let front = show.add_surface(Some(SurfaceShape::Flat {
        width: 1.0,
        height: 1.0,
    }));
    show.stage3d.surface_mut(front).unwrap().placement.position = [0.0, 0.0, 1.0];
    let map = reference(
        &still_of(&show),
        show.projector(p).unwrap(),
        (32, 18),
        &feeds_for(&show, p),
    );
    assert_eq!(at(&map, 16, 9).0, None);
    assert_eq!(at(&map, 0, 0).0, Some(0));
}

#[test]
fn a_map_names_the_canvases_the_model_says_it_maps() {
    let (mut show, a, _, p) = flat_show();
    let b = show.add_display();
    let far = show.add_surface(None);
    let sf = show.stage3d.surface_mut(far).unwrap();
    sf.placement.position = [100.0, 0.0, 0.0];
    sf.feed = Feed::Display {
        output: b,
        region: WHOLE,
    };
    assert_eq!(feeds_for(&show, p), vec![a]);
    assert_eq!(feeds_for(&show, p), show.mapped_feeds(p).used);
    // Turned to look at it instead.
    show.projector_mut(p).unwrap().placement.position[0] = 100.0;
    assert_eq!(feeds_for(&show, p), vec![b]);
}

#[test]
fn an_led_screen_is_only_in_the_way_whatever_it_shows() {
    use crate::stage3d::SurfaceKind;
    let (mut show, a, s, p) = flat_show();
    // A small LED screen in front of the screen's middle, showing the same
    // canvas: no light lands on it, so the projector shows nothing there.
    let led = show.add_surface(Some(SurfaceShape::Flat {
        width: 1.0,
        height: 1.0,
    }));
    let l = show.stage3d.surface_mut(led).unwrap();
    l.placement.position = [0.0, 0.0, 0.5];
    l.feed = Feed::Display {
        output: a,
        region: WHOLE,
    };
    l.kind = Some(SurfaceKind::Led);
    let still = still_of(&show);
    assert!(still.parts.iter().any(|q| q.catch.led));
    let map = reference(
        &still,
        show.projector(p).unwrap(),
        (32, 18),
        &feeds_for(&show, p),
    );
    assert_eq!(map.feeds, vec![a]);
    assert_eq!(at(&map, 16, 9).0, None, "the LED screen is in the way");
    assert_eq!(at(&map, 0, 0).0, Some(0), "the projection screen round it");
    // Chosen as LED, the big screen too shows nothing: no feed at all.
    show.stage3d.surface_mut(s).unwrap().kind = Some(SurfaceKind::Led);
    assert!(feeds_for(&show, p).is_empty());
    let map = reference(
        &still_of(&show),
        show.projector(p).unwrap(),
        (32, 18),
        &feeds_for(&show, p),
    );
    assert!((0..32 * 18).all(|i| unpack_how(map.how[i]).feed.is_none()));
}

#[test]
fn a_map_is_made_of_the_still_screens_and_the_venue_never_the_projectors() {
    let (mut show, _, _, p) = flat_show();
    let scene = show.stage3d.scene(&show, None);
    assert_eq!(scene.parts.len(), 2, "the screen and the projector's body");
    let still = still_scene(&show, &scene);
    assert_eq!(still.parts.len(), 1);
    assert!(!still.parts[0].lit);
    // Moving the projector makes a new scene, with the same still key.
    let key = still_key(&still);
    show.projector_mut(p).unwrap().placement.position[0] = 1.0;
    let moved = still_scene(&show, &show.stage3d.scene(&show, None));
    assert_eq!(still_key(&moved), key);
    // Moving the screen, or feeding it another region, doesn't.
    let s = show.stage3d.surfaces[0].id;
    show.stage3d.surface_mut(s).unwrap().placement.position[1] = 0.5;
    assert_ne!(still_key(&still_of(&show)), key);
}

#[test]
fn a_maps_key_has_the_lens_and_the_size_and_never_the_fine_tune_or_the_line_up() {
    let (mut show, _, _, p) = flat_show();
    let still = still_key(&still_of(&show));
    let key = |show: &Show, size| {
        let pr = show.projector(p).unwrap();
        map_key(still, pr, pr.aspect(size), size, false, &[])
    };
    let k = key(&show, (1920, 1080));
    assert_ne!(key(&show, (1280, 720)), k);
    let pr = show.projector_mut(p).unwrap();
    pr.fine = Some(crate::mesh::Mesh::grid(2, 2));
    pr.line_up = Some(Default::default());
    assert_eq!(key(&show, (1920, 1080)), k);
    show.projector_mut(p).unwrap().lens.shift = [0.0, 0.1];
    assert_ne!(key(&show, (1920, 1080)), k);
    let pr = show.projector(p).unwrap();
    assert_ne!(map_key(still, pr, 1.5, (1920, 1080), false, &[]), k);
    assert_ne!(
        map_key(still, pr, pr.aspect((1920, 1080)), (1920, 1080), true, &[]),
        k
    );
    // Another canvas named, another map.
    assert_ne!(
        map_key(
            still,
            pr,
            pr.aspect((1920, 1080)),
            (1920, 1080),
            false,
            &[OutputId(7)]
        ),
        k
    );
    // Anything else of the projector (its display) is in it.
    let mut other = pr.clone();
    other.display = Some(OutputId(9));
    assert_ne!(
        map_key(
            still,
            &other,
            other.aspect((1920, 1080)),
            (1920, 1080),
            false,
            &[]
        ),
        k
    );
}

#[test]
fn a_tiles_matrix_is_its_part_of_the_lens() {
    let (show, _, _, p) = flat_show();
    let pr = show.projector(p).unwrap();
    let aspect = 16.0 / 9.0;
    let size = (64, 36);
    // A point on the screen: where it is in the whole picture and in a tile.
    let point = [0.7, -0.3, 0.0];
    let whole = maths::transform4(&lens_matrix(pr, aspect), [0.7, -0.3, 0.0, 1.0]);
    let tile = [32, 18, 16, 9];
    let part = maths::transform4(
        &tile_matrix(pr, aspect, size, tile),
        [point[0], point[1], point[2], 1.0],
    );
    let px = |c: [f32; 4], w: f32, h: f32| {
        [(c[0] / c[3] * 0.5 + 0.5) * w, (0.5 - c[1] / c[3] * 0.5) * h]
    };
    let in_whole = px(whole, 64.0, 36.0);
    let in_tile = px(part, 16.0, 9.0);
    assert!(close(
        in_whole,
        [in_tile[0] + 32.0, in_tile[1] + 18.0],
        1e-3
    ));
    // The same depth.
    assert!((whole[2] / whole[3] - part[2] / part[3]).abs() < 1e-6);
}

/// `flat_show` with a second, smaller screen half a metre in front of the
/// first, fed by the same display, that a screen cue moves: movable.
pub(crate) fn movable_show() -> (Show, OutputId, SurfaceId, SurfaceId, ProjectorId) {
    let (mut show, a, s, p) = flat_show();
    let m = show.add_surface(None);
    let screen = show.stage3d.surface_mut(m).unwrap();
    screen.feed = Feed::Display {
        output: a,
        region: WHOLE,
    };
    screen.shape = SurfaceShape::Flat {
        width: 1.6,
        height: 0.9,
    };
    screen.placement.position = [0.8, 0.2, 0.5];
    let mut cue = show.make_cue(crate::show::CueType::Screen, None, 0);
    if let crate::show::CueKind::Screen(c) = &mut cue.kind {
        c.screen = Some(m);
        c.to.x = Some(-0.8);
    }
    show.cues.push(cue);
    assert!(show.movable(m) && !show.movable(s));
    (show, a, s, m, p)
}

#[test]
fn a_movable_screen_is_left_out_of_the_map_and_the_depth_is_the_nearest_still_thing() {
    let (show, a, _, m, p) = movable_show();
    let pr = show.projector(p).unwrap();
    let size = (32, 18);
    let aspect = pr.aspect(size);
    // Its canvas is still the map's (drawn live through it).
    assert_eq!(feeds_for(&show, p), vec![a]);
    let still = still_of(&show);
    let whole = show.stage3d.scene(&show, None);
    let bodies = show.stage3d.projectors.iter().filter(|p| p.visible).count();
    assert_eq!(
        still.parts.len() + 1 + bodies,
        whole.parts.len(),
        "the movable screen left out"
    );
    // The map is the first screen's alone, as if the second weren't there.
    let mut alone = show.clone();
    alone.stage3d.surfaces.retain(|x| x.id != m);
    alone.cues.clear();
    let want = reference(&still_of(&alone), alone.projector(p).unwrap(), size, &[a]);
    let map = reference_with_depth(&still, pr, size, aspect, &[a]);
    assert_eq!(map.place, want.place);
    assert_eq!(map.how, want.how);
    assert!(want.depth.is_none() && map.depth_range == DEPTH_RANGE);
    // Depth: everywhere the screen, at its distance along each pixel's ray
    // (the lens is square on to it).
    let depth = map.depth.as_ref().expect("depth worked out");
    let d = pr.placement.position[2];
    for y in 0..size.1 {
        for x in 0..size.0 {
            let ray = pr.ray((x as f32 + 0.5) / 32.0, (y as f32 + 0.5) / 18.0, aspect);
            let want = d / -ray.dir[2];
            let got = crate::mapped::unpack_depth(depth[(y * 32 + x) as usize], DEPTH_RANGE);
            assert!((got - want).abs() < 2e-3, "({x}, {y}): {got} not {want}");
        }
    }
    // A box of the room in front of the middle: there the depth is the
    // box's front, nearer than the screen; elsewhere still the screen.
    let mut scene = still.clone();
    scene
        .parts
        .insert(0, box_part([0.0, 0.0, 0.5], [0.5, 0.5, 0.25]));
    let map = reference_with_depth(&scene, pr, size, aspect, &[a]);
    let depth = map.depth.as_ref().unwrap();
    let at_middle = crate::mapped::unpack_depth(depth[(9 * 32 + 16) as usize], DEPTH_RANGE);
    let ray = pr.ray(16.5 / 32.0, 9.5 / 18.0, aspect);
    let front = (d - 0.75) / -ray.dir[2];
    assert!((at_middle - front).abs() < 2e-3, "{at_middle} not {front}");
    let corner = crate::mapped::unpack_depth(depth[0], DEPTH_RANGE);
    assert!(corner > d && corner < d * 1.2, "{corner}");
    // Nothing at all: the far end; and nothing the lens can see packs so.
    let empty = Scene {
        parts: Vec::new(),
        throws: Vec::new(),
    };
    let map = reference_with_depth(&empty, pr, (4, 4), aspect, &[a]);
    assert!(
        map.depth
            .unwrap()
            .iter()
            .all(|px| *px == [255, 255, 255, 255])
    );
    let far = crate::stage3d::light::THROW_FAR * 2.0;
    assert_ne!(
        crate::mapped::pack_depth(far, DEPTH_RANGE),
        [255, 255, 255, 255]
    );
}

#[test]
fn making_a_screen_movable_changes_the_still_key_and_where_it_goes_does_not() {
    let (show, _, _, _, p) = movable_show();
    let mut plain = show.clone();
    plain.cues.clear();
    let k = |s: &Show| still_key(&still_of(s));
    assert_ne!(k(&show), k(&plain), "a screen cue added: a new still scene");
    let mut further = show.clone();
    if let crate::show::CueKind::Screen(c) = &mut further.cues[0].kind {
        c.to.x = Some(3.0);
    }
    assert_eq!(k(&show), k(&further));
    let pr = show.projector(p).unwrap();
    let key = |depth| map_key(k(&show), pr, 16.0 / 9.0, (64, 36), depth, &[]);
    assert_ne!(key(true), key(false));
}

#[test]
fn a_moved_screens_parts_are_put_where_it_is_and_the_rest_stay() {
    let (show, _, s, m, _) = movable_show();
    let scene = show.stage3d.scene(&show, None);
    let pieces = screen_pieces(&show, &scene);
    assert_eq!(pieces.len(), 2, "two flat screens, a piece each");
    assert!(moved_parts(&pieces, &[]).is_empty());
    let mut there = show.stage3d.surface(m).unwrap().placement;
    there.position[0] = -0.8;
    there.yaw = 30.0;
    let moved = moved_parts(&pieces, &[(m, there)]);
    assert_eq!(moved.len(), 1);
    let after = with_moved(&scene, &moved);
    // The same as a scene made with the screen there.
    let mut at = show.clone();
    at.stage3d.surface_mut(m).unwrap().placement = there;
    let want = at.stage3d.scene(&at, None);
    for (i, (a, b)) in after.parts.iter().zip(&want.parts).enumerate() {
        for c in 0..4 {
            for r in 0..4 {
                assert!(
                    (a.transform[c][r] - b.transform[c][r]).abs() < 1e-5,
                    "part {i}"
                );
            }
        }
    }
    let _ = s;
}
