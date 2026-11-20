use super::*;
use crate::render3d::{Mesh, Paint};
use crate::stage3d::geometry;
use std::sync::Arc;

const HD: f32 = 16.0 / 9.0;

/// A projector `distance` metres in front of the origin (on +Z), looking at
/// it, giving a picture 6 m wide there.
fn six_metres_wide(distance: f32, lumens: f32) -> Projector {
    let mut p = Projector::new(ProjectorId(1), "P".into());
    p.placement.position = [0.0, 0.0, distance];
    p.lens.throw = distance / 6.0;
    p.lumens = lumens;
    p
}

#[test]
fn the_worked_check_ten_thousand_lumens_on_six_metres() {
    // 10,000 lumens over 6 m by 3.375 m (20.25 m²): 494 lux, 157 nits.
    let p = six_metres_wide(9.0, 10_000.0);
    let lux = illuminance(&p, HD, [0.0; 3], Some([0.0, 0.0, 1.0]), Facing::Front);
    assert!((lux - 10_000.0 / 20.25).abs() < 0.05, "{lux}");
    assert!((lux - 493.8).abs() < 0.1, "{lux}");
    let nits = luminance(lux, 1.0);
    assert!((nits - 157.2).abs() < 0.1, "{nits}");
    // Gain 1.5 screen: half as bright again.
    assert!((luminance(lux, 1.5) / nits - 1.5).abs() < 1e-5);
    assert_eq!(luminance(lux, f32::NAN), nits);
    // The same from any throw: it is the picture's size that counts.
    let far = six_metres_wide(30.0, 10_000.0);
    let again = illuminance(&far, HD, [0.0; 3], None, Facing::Front);
    assert!((again - lux).abs() < 0.05, "{again}");
}

#[test]
fn light_falls_off_with_the_square_of_the_distance() {
    let p = six_metres_wide(9.0, 5000.0);
    let near = illuminance(&p, HD, [0.0, 0.0, 4.5], None, Facing::Front);
    let far = illuminance(&p, HD, [0.0; 3], None, Facing::Front);
    assert!((near / far - 4.0).abs() < 1e-3, "{}", near / far);
    // Across a wall square to the lens it is even (the two cosines cancel).
    let n = Some([0.0, 0.0, 1.0]);
    let middle = illuminance(&p, HD, [0.0; 3], n, Facing::Front);
    let corner = illuminance(&p, HD, [2.9, 1.6, 0.0], n, Facing::Front);
    assert!((corner / middle - 1.0).abs() < 1e-3, "{}", corner / middle);
    // Outside the picture, behind the lens: none.
    assert_eq!(illuminance(&p, HD, [3.1, 0.0, 0.0], n, Facing::Front), 0.0);
    assert_eq!(illuminance(&p, HD, [0.0, 0.0, 20.0], n, Facing::Front), 0.0);
}

#[test]
fn a_slanting_wall_gets_the_cosine_and_a_front_screen_nothing_from_behind() {
    let p = six_metres_wide(9.0, 5000.0);
    let square = illuminance(&p, HD, [0.0; 3], Some([0.0, 0.0, 1.0]), Facing::Front);
    for deg in [30.0f32, 60.0, 80.0] {
        let n = [deg.to_radians().sin(), 0.0, deg.to_radians().cos()];
        let e = illuminance(&p, HD, [0.0; 3], Some(n), Facing::Front);
        assert!(
            (e / square - deg.to_radians().cos()).abs() < 1e-3,
            "{deg}: {}",
            e / square
        );
    }
    // Lit from behind: a front screen shows nothing, a rear one (or a wall,
    // whichever way it is wound) the same light.
    let back = Some([0.0, 0.0, -1.0]);
    assert_eq!(illuminance(&p, HD, [0.0; 3], back, Facing::Front), 0.0);
    let rear = illuminance(&p, HD, [0.0; 3], back, Facing::Both);
    assert!((rear - square).abs() < 1e-3);
}

#[test]
fn lumens_from_a_file_are_kept_sane() {
    let mut p = six_metres_wide(9.0, f32::NAN);
    let e = illuminance(&p, HD, [0.0; 3], None, Facing::Front);
    assert!(
        (e - crate::stage3d::projector::DEFAULT_LUMENS / 20.25).abs() < 0.05,
        "{e}"
    );
    p.lumens = 1e12;
    let e = illuminance(&p, HD, [0.0; 3], None, Facing::Front);
    assert!((e - crate::stage3d::projector::MAX_LUMENS / 20.25).abs() < 1.0);
    p.lumens = -3.0;
    assert_eq!(illuminance(&p, HD, [0.0; 3], None, Facing::Front), 0.0);
}

fn wall(z: f32, size: f32) -> Part {
    let mut t = crate::render3d::scene::IDENTITY;
    t[3] = [0.0, 0.0, z, 1.0];
    Part {
        mesh: Arc::new(geometry::quad(size, size)),
        mesh_key: (z * 1000.0) as u64,
        transform: t,
        paint: Paint::Colour([1.0; 4]),
        lit: true,
    }
}

#[test]
fn something_in_the_way_casts_a_shadow() {
    let p = six_metres_wide(9.0, 5000.0);
    let mut q = six_metres_wide(9.0, 5000.0);
    q.id = ProjectorId(2);
    q.placement.position = [1.0, 0.0, 9.0];
    let screen = wall(0.0, 10.0);
    let scene = Scene {
        parts: vec![screen.clone()],
        throws: Vec::new(),
    };
    let ps = [(&p, HD), (&q, HD)];
    let n = Some([0.0, 0.0, 1.0]);
    let lit = lit_by(&ps, &scene, &|_, _| true, [0.0; 3], n, Facing::Front);
    assert_eq!(lit.len(), 2, "both, on the screen itself");
    let both: f32 = lit.iter().map(|(_, l)| l).sum();
    assert!(both > 1.9 * lit[0].1, "light adds");
    // A board in front of the first lens only.
    let mut board = Part {
        mesh: Arc::new(Mesh {
            positions: vec![
                [-0.3, -0.3, 0.0],
                [0.3, -0.3, 0.0],
                [0.3, 0.3, 0.0],
                [-0.3, 0.3, 0.0],
            ],
            normals: vec![],
            uvs: vec![],
            indices: vec![0, 1, 2, 0, 2, 3],
        }),
        ..wall(4.0, 1.0)
    };
    board.mesh_key = 99;
    let scene = Scene {
        parts: vec![screen, board],
        throws: Vec::new(),
    };
    let lit = lit_by(&ps, &scene, &|_, _| true, [0.0; 3], n, Facing::Front);
    assert_eq!(
        lit.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        vec![ProjectorId(2)]
    );
    // The board left out of what blocks (a projector's own body): lit again.
    let lit = lit_by(&ps, &scene, &|i, _| i == 0, [0.0; 3], n, Facing::Front);
    assert_eq!(lit.len(), 2);
    // A hidden projector gives no light.
    let mut hidden = p.clone();
    hidden.visible = false;
    assert!(
        lit_by(
            &[(&hidden, HD)],
            &scene,
            &|i, _| i == 0,
            [0.0; 3],
            n,
            Facing::Front
        )
        .is_empty()
    );
}
