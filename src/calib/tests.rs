//! The line-up solver on made-up projectors: points are projected through
//! a known projector (in f64, by the solver's own `project`, which the first
//! tests pin to `Projector::in_picture` and `ray`) and the solve must find
//! it again.
//!
//! Tolerances: exact data is recovered to 1e-6 m and 1e-6 rad (and 1e-6 of
//! the throw, aspect and shift), well above f64 rounding and far below what
//! matters. With noise the bound is four standard deviations from the
//! solver's own covariance at the truth (`model::spreads`, a pixel per
//! point), times the noise: the error a Gaussian noise of that size gives,
//! not a number tuned until green.

use super::linalg::{self, M3, V3};
use super::model::{self, State};
use super::*;
use crate::stage3d::projector::{Lens, LensPreset, ProjectorId};

const PIXELS: (u32, u32) = (1920, 1080);
const HD: f64 = 1920.0 / 1080.0;

fn px(p: (u32, u32)) -> (f64, f64) {
    (p.0 as f64, p.1 as f64)
}

/// A made-up projector.
#[derive(Clone, Copy, Debug)]
struct Rig {
    pose: Pose,
    lens: LensFit,
    pixels: (u32, u32),
}

impl Rig {
    fn new(position: V3, yaw: f64, pitch: f64, roll: f64, throw: f64, shift: [f64; 2]) -> Rig {
        Rig {
            pose: Pose {
                position,
                yaw,
                pitch,
                roll,
            },
            lens: LensFit {
                throw,
                shift,
                aspect: HD,
            },
            pixels: PIXELS,
        }
    }

    fn state(&self) -> State {
        State {
            r: self.pose.turn(),
            c: self.pose.position,
            lens: self.lens,
        }
    }

    /// The room point seen at picture point `(u, v)` (0..1), `depth` metres
    /// in front of the lens along its axis.
    fn spot(&self, u: f64, v: f64, depth: f64) -> V3 {
        let (w, h) = px(self.pixels);
        let b = model::bearing(&self.lens, (w, h), [u * w, v * h]);
        let p = linalg::scale(b, depth);
        linalg::add(self.pose.position, linalg::mul_v(&self.pose.turn(), p))
    }

    /// Where the picture point `(u, v)` lands on the plane through `o` with
    /// normal `n`.
    fn on_plane(&self, u: f64, v: f64, o: V3, n: V3) -> V3 {
        let (w, h) = px(self.pixels);
        let b = model::bearing(&self.lens, (w, h), [u * w, v * h]);
        let d = linalg::mul_v(&self.pose.turn(), b);
        let c = self.pose.position;
        let t = linalg::dot(linalg::sub(o, c), n) / linalg::dot(d, n);
        assert!(t > 0.0, "the plane is behind the lens");
        linalg::add(c, linalg::scale(d, t))
    }

    fn points(&self, world: &[V3]) -> Vec<LinePoint> {
        let s = self.state();
        world
            .iter()
            .map(|x| LinePoint::new(*x, model::project(&s, px(self.pixels), *x).unwrap()))
            .collect()
    }

    fn problem(&self, points: Vec<LinePoint>, entered: LensFit, free: Freedom) -> Problem {
        Problem {
            points,
            pixels: self.pixels,
            lens: entered,
            start: None,
            free,
        }
    }
}

/// Picture points spread over the picture, and how far each is (a share of
/// the rig's distance): not on one plane.
const UV: [(f64, f64, f64); 12] = [
    (0.1, 0.1, 1.0),
    (0.9, 0.15, 1.25),
    (0.85, 0.9, 0.8),
    (0.12, 0.85, 1.1),
    (0.5, 0.5, 0.9),
    (0.3, 0.7, 1.3),
    (0.7, 0.3, 0.75),
    (0.5, 0.1, 1.15),
    (0.2, 0.45, 0.85),
    (0.8, 0.6, 1.2),
    (0.45, 0.9, 0.95),
    (0.65, 0.75, 1.05),
];

fn deep_points(rig: &Rig, n: usize, distance: f64) -> Vec<V3> {
    UV.iter()
        .take(n)
        .map(|&(u, v, k)| rig.spot(u, v, distance * k))
        .collect()
}

/// A projector in the audience, a little up and to the side, aimed at the
/// stage (towards -Z) and tipped down.
fn audience_rig(throw: f64, shift: [f64; 2]) -> Rig {
    Rig::new([1.5, 4.0, 14.0], 8.0, -9.0, 1.5, throw, shift)
}

fn assert_recovers(sol: &Solution, rig: &Rig, tol_m: f64, tol_rad: f64) {
    let dp = linalg::norm(linalg::sub(sol.pose.position, rig.pose.position));
    let da = linalg::angle_between(&sol.pose.turn(), &rig.pose.turn());
    assert!(dp <= tol_m, "position {dp} m off: {sol:?} for {rig:?}");
    assert!(da <= tol_rad, "turn {da} rad off: {sol:?} for {rig:?}");
}

fn assert_lens(sol: &Solution, truth: &LensFit, tol: f64) {
    let l = &sol.lens;
    assert!(
        (l.throw - truth.throw).abs() <= tol * truth.throw,
        "throw {} for {}",
        l.throw,
        truth.throw
    );
    assert!(
        (l.shift[0] - truth.shift[0]).abs() <= tol && (l.shift[1] - truth.shift[1]).abs() <= tol,
        "shift {:?} for {:?}",
        l.shift,
        truth.shift
    );
    assert!(
        (l.aspect - truth.aspect).abs() <= tol * truth.aspect,
        "aspect {} for {}",
        l.aspect,
        truth.aspect
    );
}

/// splitmix64: the same numbers every run, no crate.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `lo..hi`.
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Standard normal (Box-Muller).
    fn normal(&mut self) -> f64 {
        let a = self.range(1e-12, 1.0);
        let b = self.range(0.0, 1.0);
        (-2.0 * a.ln()).sqrt() * (std::f64::consts::TAU * b).cos()
    }
}

fn projector_of(rig: &Rig) -> Projector {
    let mut p = Projector::new(ProjectorId(1), "P".into());
    p.placement = rig.pose.placement();
    p.lens.throw = rig.lens.throw as f32;
    p.lens.shift = rig.lens.shift.map(|v| v as f32);
    let own = rig.pixels.0 as f64 / rig.pixels.1 as f64;
    if (rig.lens.aspect - own).abs() > 1e-9 {
        p.lens.aspect = Some(rig.lens.aspect as f32);
    }
    p.resolution = Some(rig.pixels);
    p
}

// ---- the projection is NewQ's ----

#[test]
fn the_projection_is_the_projectors_own() {
    let rigs = [
        audience_rig(1.6, [0.0, 0.0]),
        audience_rig(0.5, [0.12, 0.4]),
        // Rear projection: behind a screen at z = -8, looking towards +Z.
        Rig::new([0.3, 2.5, -14.0], 180.0, 2.0, 0.0, 1.2, [0.0, -0.2]),
        // Straight down from the grid.
        Rig::new([2.0, 9.0, -3.0], 30.0, -90.0, 0.0, 0.8, [0.05, 0.0]),
        // Rolled on its side, anamorphic.
        Rig {
            lens: LensFit {
                throw: 2.4,
                shift: [-0.3, 0.1],
                aspect: 2.39,
            },
            ..Rig::new([-4.0, 3.0, 10.0], -20.0, 5.0, 90.0, 2.4, [0.0; 2])
        },
    ];
    let mut rng = Rng(7);
    for rig in rigs {
        let proj = projector_of(&rig);
        // The solver's state made from the show's (f32) numbers, as a caller
        // would make it from a projector.
        let s = State {
            r: Pose::of(&proj.placement).turn(),
            c: Pose::of(&proj.placement).position,
            lens: LensFit::of(&proj, rig.pixels),
        };
        assert!((s.lens.aspect - rig.lens.aspect).abs() < 1e-6);
        let aspect = proj.aspect(rig.pixels);
        let (w, h) = px(rig.pixels);
        for _ in 0..50 {
            let (u, v, d) = (
                rng.range(-0.2, 1.2),
                rng.range(-0.2, 1.2),
                rng.range(2.0, 30.0),
            );
            let x = rig.spot(u, v, d);
            let ours = model::project(&s, (w, h), x).unwrap();
            let x32 = x.map(|v| v as f32);
            let ([pu, pv], depth) = proj.in_picture(x32, aspect).unwrap();
            // f32 sums 30 m out: about 1e-6 of the picture.
            let tol = 5e-6 * w;
            assert!(
                (ours[0] - pu as f64 * w).abs() < tol && (ours[1] - pv as f64 * h).abs() < tol,
                "{ours:?} vs {:?} for {rig:?}",
                [pu as f64 * w, pv as f64 * h]
            );
            assert!((depth as f64 - d).abs() < 1e-4 * d);
            // And the projector's ray through (u, v) passes through the
            // point the solver puts there.
            let ray = proj.ray(u as f32, v as f32, aspect);
            let back = linalg::add(
                ray.origin.map(|v| v as f64),
                linalg::scale(ray.dir.map(|v| v as f64), linalg::norm(linalg::sub(x, s.c))),
            );
            assert!(
                linalg::norm(linalg::sub(back, x)) < 1e-5 * d,
                "ray misses by {}",
                linalg::norm(linalg::sub(back, x))
            );
        }
        // Behind the lens is nothing.
        let behind = linalg::sub(s.c, linalg::mul_v(&s.r, [0.0, 0.0, -1.0]));
        assert!(model::project(&s, (w, h), behind).is_none());
    }
}

// ---- the sums ----

#[test]
fn turns_go_to_yaw_pitch_roll_and_back() {
    let mut rng = Rng(3);
    let mut cases = vec![
        (0.0, 0.0, 0.0),
        (180.0, 0.0, 0.0),
        (30.0, -90.0, 0.0),
        (30.0, 90.0, 0.0),
        (-45.0, -90.0, 25.0),
        (10.0, -89.9999, 5.0),
        (0.0, 0.0, 179.0),
    ];
    for _ in 0..200 {
        cases.push((
            rng.range(-180.0, 180.0),
            rng.range(-90.0, 90.0),
            rng.range(-180.0, 180.0),
        ));
    }
    for (y, p, r) in cases {
        let m = linalg::from_euler(y, p, r);
        let (y2, p2, r2) = linalg::to_euler(&m);
        assert!((-90.0..=90.0).contains(&p2));
        let back = linalg::from_euler(y2, p2, r2);
        let a = linalg::angle_between(&m, &back);
        assert!(a < 1e-12, "{y} {p} {r} -> {y2} {p2} {r2}: {a} rad");
    }
    // Straight down, the turn is all yaw.
    let (_, p, r) = linalg::to_euler(&linalg::from_euler(-45.0, -90.0, 25.0));
    assert!((p + 90.0).abs() < 1e-9 && r.abs() < 1e-9);
}

#[test]
fn the_turn_is_placements_own() {
    // `from_euler` is `Placement::matrix`'s turn (column-major there).
    let pl = Placement {
        position: [1.0, 2.0, 3.0],
        yaw: 33.0,
        pitch: -21.0,
        roll: 12.0,
    };
    let m = pl.matrix();
    let r = Pose::of(&pl).turn();
    for row in 0..3 {
        for col in 0..3 {
            assert!((m[col][row] as f64 - r[row][col]).abs() < 1e-6);
        }
    }
}

#[test]
fn the_nearest_rotation_and_the_eigen_solve() {
    let r = linalg::from_euler(40.0, -30.0, 70.0);
    let noisy: M3 = r.map(|row| row.map(|v| v * 2.5));
    assert!(linalg::angle_between(&linalg::nearest_rotation(&noisy), &r) < 1e-12);
    // A mirror's nearest rotation is still a rotation.
    let mirror = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0]];
    assert!(linalg::det(&linalg::nearest_rotation(&mirror)) > 0.0);
    // Eigenvalues smallest first, vectors that are theirs.
    let a = [4.0, 1.0, 0.5, 1.0, 3.0, 0.2, 0.5, 0.2, 1.0];
    let (values, vectors) = linalg::eigen_symmetric(&a, 3);
    assert!(values[0] <= values[1] && values[1] <= values[2]);
    for (l, v) in values.iter().zip(&vectors) {
        for row in 0..3 {
            let av: f64 = (0..3).map(|k| a[row * 3 + k] * v[k]).sum();
            assert!((av - l * v[row]).abs() < 1e-12);
        }
    }
    // RQ gives the matrix back.
    let m = [[3.0, 0.4, 1.0], [0.2, 2.0, 0.7], [0.1, 0.3, 1.5]];
    let (k, q) = linalg::rq(&m).unwrap();
    let back = linalg::mul(&k, &q);
    for row in 0..3 {
        for col in 0..3 {
            assert!((back[row][col] - m[row][col]).abs() < 1e-12);
        }
    }
    assert!(k[1][0] == 0.0 && k[2][0] == 0.0 && k[2][1] == 0.0);
}

// ---- known lens: four points or more ----

#[test]
fn four_points_and_the_lens_find_the_projector() {
    for preset in LensPreset::ALL {
        let (a, b) = preset.range();
        let throw = ((a + b) / 2.0) as f64;
        let rig = audience_rig(throw, [0.0, 0.0]);
        // Far enough that the picture is a stage's size.
        let distance = 4.0 * throw;
        let world = deep_points(&rig, 4, distance);
        let problem = rig.problem(rig.points(&world), rig.lens, Freedom::default());
        let sol = solve(&problem).unwrap();
        assert_eq!(sol.rung, Rung::Pose, "{preset:?}");
        assert_recovers(&sol, &rig, 1e-6, 1e-6);
        assert!(sol.max_px < 1e-6);
        let words = sol.words();
        assert!(
            words.starts_with(&format!(
                "Lined up to within half a pixel. Using the lens you entered (throw {throw:.2})."
            )),
            "{words}"
        );
        // Four points can't free the lens, nor show a bad one: both said.
        assert!(
            words.contains("With 6 points or more (there are 4)"),
            "{words}"
        );
        assert!(sol.notes.contains(&Note::CantTellWhich));
    }
}

#[test]
fn four_points_on_one_screen_find_the_projector() {
    let rig = audience_rig(1.6, [0.0, 0.2]);
    // The screen at z = 0, facing the audience.
    let world: Vec<V3> = [(0.1, 0.1), (0.92, 0.12), (0.88, 0.9), (0.15, 0.8)]
        .iter()
        .map(|&(u, v)| rig.on_plane(u, v, [0.0, 0.0, 0.0], [0.0, 0.0, 1.0]))
        .collect();
    let problem = rig.problem(rig.points(&world), rig.lens, Freedom::default());
    let sol = solve(&problem).unwrap();
    assert_eq!(sol.rung, Rung::Pose);
    assert_recovers(&sol, &rig, 1e-6, 1e-6);
    // Four points can't free the lens, and the words say how many would.
    assert!(sol.notes.contains(&Note::Flat), "{:?}", sol.notes);
}

#[test]
fn a_starting_place_is_used_and_does_not_change_the_answer() {
    let rig = audience_rig(1.6, [0.0, 0.0]);
    let world = deep_points(&rig, 5, 8.0);
    let mut problem = rig.problem(rig.points(&world), rig.lens, Freedom::default());
    let without = solve(&problem).unwrap();
    // Roughly where it was hung: half a metre and a few degrees out.
    problem.start = Some(Pose {
        position: linalg::add(rig.pose.position, [0.4, -0.3, 0.2]),
        yaw: rig.pose.yaw + 4.0,
        pitch: rig.pose.pitch - 3.0,
        roll: rig.pose.roll + 2.0,
    });
    let with = solve(&problem).unwrap();
    assert_recovers(&without, &rig, 1e-6, 1e-6);
    assert_recovers(&with, &rig, 1e-6, 1e-6);
}

// ---- unknown lens ----

#[test]
fn six_points_find_the_throw_for_every_lens() {
    for preset in LensPreset::ALL {
        let (a, b) = preset.range();
        let throw = ((a + b) / 2.0) as f64;
        let rig = audience_rig(throw, [0.0, 0.0]);
        let world = deep_points(&rig, 6, 4.0 * throw);
        // Entered as the standard zoom's middle: wrong for all but one.
        let entered = LensFit {
            throw: 1.6,
            ..rig.lens
        };
        let problem = rig.problem(rig.points(&world), entered, Freedom::default());
        let sol = solve(&problem).unwrap();
        assert_eq!(sol.rung, Rung::Throw, "{preset:?}: {:?}", sol.notes);
        assert_recovers(&sol, &rig, 1e-6, 1e-6);
        assert_lens(&sol, &rig.lens, 1e-6);
        assert!(
            sol.notes.contains(&Note::TooFewFor {
                part: LensPart::Shift,
                have: 6
            }),
            "{:?}",
            sol.notes
        );
        assert!(sol.words().contains("Worked out the throw from the points"));
    }
}

#[test]
fn eight_points_find_a_shifted_lens() {
    for (throw, shift) in [(1.6, [0.0, 0.35]), (0.5, [0.15, -0.2]), (1.2, [-0.1, 0.5])] {
        let rig = audience_rig(throw, shift);
        let world = deep_points(&rig, 8, 4.0 * throw);
        let entered = LensFit {
            throw: 1.5,
            shift: [0.0, 0.0],
            aspect: HD,
        };
        let problem = rig.problem(rig.points(&world), entered, Freedom::default());
        let sol = solve(&problem).unwrap();
        assert_eq!(sol.rung, Rung::Shift, "{:?}", sol.notes);
        assert_recovers(&sol, &rig, 1e-6, 1e-6);
        assert_lens(&sol, &rig.lens, 1e-6);
        // No aspect unless asked for, and nothing to say about it.
        assert!(sol.notes.is_empty(), "{:?}", sol.notes);
    }
}

#[test]
fn a_long_lens_with_eight_points_keeps_the_shift_as_entered_and_says_so() {
    // A long lens sees the points nearly flat-on: a shift and a small turn
    // look alike, and eight points pin the shift to only about 5% of the
    // picture (the solver's own sd for a pixel's error). Under the 2% it
    // needs, so the shift stays as entered and the words say why.
    let rig = audience_rig(3.0, [-0.1, 0.5]);
    let world = deep_points(&rig, 8, 12.0);
    let entered = LensFit {
        throw: 2.5,
        shift: [-0.1, 0.5],
        aspect: HD,
    };
    let sol = solve(&rig.problem(rig.points(&world), entered, Freedom::default())).unwrap();
    assert_eq!(sol.rung, Rung::Throw);
    assert_eq!(
        sol.notes,
        vec![Note::Unsure {
            part: LensPart::Shift
        }]
    );
    // With the shift entered right, the rest is exact.
    assert_recovers(&sol, &rig, 1e-6, 1e-6);
    assert_lens(&sol, &rig.lens, 1e-6);
    assert!(
        sol.words()
            .contains("don't pin the lens shift down well enough")
    );
}

#[test]
fn an_anamorphic_lens_is_found_when_asked_for() {
    let mut rig = audience_rig(1.2, [0.05, 0.2]);
    rig.lens.aspect = 2.39;
    let world = deep_points(&rig, 12, 6.0);
    let entered = LensFit {
        throw: 1.5,
        shift: [0.0; 2],
        aspect: HD,
    };
    let free = Freedom {
        aspect: true,
        ..Freedom::default()
    };
    let sol = solve(&rig.problem(rig.points(&world), entered, free)).unwrap();
    assert_eq!(sol.rung, Rung::Aspect, "{:?}", sol.notes);
    assert_recovers(&sol, &rig, 1e-6, 1e-6);
    assert_lens(&sol, &rig.lens, 1e-6);
    assert!(sol.words().contains("picture's shape (2.39:1)"));
}

#[test]
fn a_lens_kept_as_entered_stays_as_entered() {
    let rig = audience_rig(1.6, [0.0, 0.0]);
    let world = deep_points(&rig, 10, 8.0);
    let free = Freedom {
        throw: false,
        shift: false,
        aspect: false,
    };
    let sol = solve(&rig.problem(rig.points(&world), rig.lens, free)).unwrap();
    assert_eq!(sol.rung, Rung::Pose);
    assert_eq!(sol.lens, rig.lens);
    assert!(sol.notes.is_empty());
}

#[test]
fn points_on_one_screen_keep_the_lens_as_entered() {
    let rig = audience_rig(1.6, [0.0, 0.0]);
    let world: Vec<V3> = UV
        .iter()
        .take(9)
        .map(|&(u, v, _)| rig.on_plane(u, v, [0.0, 0.0, 0.0], [0.0, 0.0, 1.0]))
        .collect();
    let sol = solve(&rig.problem(rig.points(&world), rig.lens, Freedom::default())).unwrap();
    assert_eq!(sol.rung, Rung::Pose);
    assert_eq!(sol.notes, vec![Note::Flat]);
    assert_recovers(&sol, &rig, 1e-6, 1e-6);
    assert!(sol.words().contains("all on one flat surface"));
}

// ---- other ways to hang a projector ----

#[test]
fn rear_projection_solves() {
    // Behind a screen at z = -8 (its front towards the audience at +Z),
    // looking towards +Z through it.
    let rig = Rig::new([0.3, 2.6, -15.0], 180.0, 1.5, 0.0, 1.2, [0.0, -0.1]);
    let world: Vec<V3> = [
        (0.08, 0.1),
        (0.9, 0.08),
        (0.93, 0.92),
        (0.1, 0.88),
        (0.5, 0.45),
    ]
    .iter()
    .map(|&(u, v)| rig.on_plane(u, v, [0.0, 2.5, -8.0], [0.0, 0.0, 1.0]))
    .collect();
    let sol = solve(&rig.problem(rig.points(&world), rig.lens, Freedom::default())).unwrap();
    assert_recovers(&sol, &rig, 1e-6, 1e-6);
    // And with points off the screen too (its frame, the floor behind),
    // the whole lens.
    let world = deep_points(&rig, 9, 7.0);
    let entered = LensFit {
        throw: 1.5,
        shift: [0.0; 2],
        aspect: HD,
    };
    let sol = solve(&rig.problem(rig.points(&world), entered, Freedom::default())).unwrap();
    assert_eq!(sol.rung, Rung::Shift);
    assert_recovers(&sol, &rig, 1e-6, 1e-6);
    assert_lens(&sol, &rig.lens, 1e-6);
}

#[test]
fn a_projector_looking_straight_down_solves() {
    for (yaw, pitch, roll) in [(30.0, -90.0, 0.0), (-120.0, -89.99, 12.0), (0.0, 90.0, 0.0)] {
        let rig = Rig::new([2.0, 9.0, -3.0], yaw, pitch, roll, 0.8, [0.05, 0.1]);
        // On the floor (or the ceiling, looking up).
        let floor = if pitch < 0.0 { 0.0 } else { 14.0 };
        let flat: Vec<V3> = UV
            .iter()
            .take(5)
            .map(|&(u, v, _)| rig.on_plane(u, v, [0.0, floor, 0.0], [0.0, 1.0, 0.0]))
            .collect();
        let sol = solve(&rig.problem(rig.points(&flat), rig.lens, Freedom::default())).unwrap();
        assert_recovers(&sol, &rig, 1e-6, 1e-6);
        if pitch == -90.0 {
            assert!(sol.pose.roll.abs() < 1e-6, "{:?}", sol.pose);
        }
        // Boxes on the floor too: the lens as well.
        let deep = deep_points(&rig, 8, 7.0);
        let entered = LensFit {
            throw: 1.5,
            shift: [0.0; 2],
            aspect: HD,
        };
        let sol = solve(&rig.problem(rig.points(&deep), entered, Freedom::default())).unwrap();
        assert_eq!(sol.rung, Rung::Shift);
        assert_recovers(&sol, &rig, 1e-6, 1e-6);
        assert_lens(&sol, &rig.lens, 1e-6);
    }
}

// ---- noise ----

/// `points` with `sigma` pixels of Gaussian noise (the same draws for the
/// same seed, so two sigmas give errors in proportion).
fn noisy(points: &[LinePoint], sigma: f64, seed: u64) -> Vec<LinePoint> {
    let mut rng = Rng(seed);
    points
        .iter()
        .map(|p| {
            let mut p = p.clone();
            p.pixel[0] += sigma * rng.normal();
            p.pixel[1] += sigma * rng.normal();
            p
        })
        .collect()
}

#[test]
fn pixel_noise_moves_the_answer_by_what_the_points_allow() {
    let rig = audience_rig(1.6, [0.0, 0.25]);
    let world = deep_points(&rig, 12, 9.0);
    let exact = rig.points(&world);
    let pairs: Vec<model::Pair> = exact.iter().map(|p| (p.world, p.pixel)).collect();
    let (_, s, _) = linalg::spread(&world);
    let size = s[2].sqrt();
    let sd = model::spreads(&rig.state(), Rung::Shift, px(PIXELS), &pairs, size).unwrap();
    let sd_turn = (sd[0] * sd[0] + sd[1] * sd[1] + sd[2] * sd[2]).sqrt();
    let sd_place = (sd[3] * sd[3] + sd[4] * sd[4] + sd[5] * sd[5]).sqrt();
    let entered = LensFit {
        throw: 1.5,
        shift: [0.0; 2],
        aspect: HD,
    };
    let mut errors = Vec::new();
    for sigma in [0.5, 2.0] {
        let sol =
            solve(&rig.problem(noisy(&exact, sigma, 11), entered, Freedom::default())).unwrap();
        assert_eq!(sol.rung, Rung::Shift, "{sigma}: {:?}", sol.notes);
        assert_recovers(&sol, &rig, 4.0 * sigma * sd_place, 4.0 * sigma * sd_turn);
        assert!((sol.lens.throw - rig.lens.throw).abs() <= 4.0 * sigma * sd[6]);
        assert!((sol.lens.shift[0] - rig.lens.shift[0]).abs() <= 4.0 * sigma * sd[7]);
        assert!((sol.lens.shift[1] - rig.lens.shift[1]).abs() <= 4.0 * sigma * sd[8]);
        // The fit is about the noise (each point's error is the length of
        // two noisy numbers, less what the nine solved numbers soak up).
        assert!(
            sol.rms_px > 0.5 * sigma && sol.rms_px < 2.0 * sigma,
            "{sigma}: {}",
            sol.rms_px
        );
        assert!(sol.worst.is_none(), "{sigma}: {:?}", sol.worst);
        errors.push(linalg::norm(linalg::sub(
            sol.pose.position,
            rig.pose.position,
        )));
    }
    // The same draws four times as big move it about four times as far.
    let ratio = errors[1] / errors[0];
    assert!((3.5..4.5).contains(&ratio), "{ratio}");
}

#[test]
fn half_a_pixel_of_noise_with_four_points_is_still_close() {
    let rig = audience_rig(1.6, [0.0, 0.0]);
    let world = deep_points(&rig, 4, 8.0);
    let exact = rig.points(&world);
    let pairs: Vec<model::Pair> = exact.iter().map(|p| (p.world, p.pixel)).collect();
    let (_, s, _) = linalg::spread(&world);
    let sd = model::spreads(&rig.state(), Rung::Pose, px(PIXELS), &pairs, s[2].sqrt()).unwrap();
    let sd_turn = (sd[0] * sd[0] + sd[1] * sd[1] + sd[2] * sd[2]).sqrt();
    let sd_place = (sd[3] * sd[3] + sd[4] * sd[4] + sd[5] * sd[5]).sqrt();
    let sol = solve(&rig.problem(noisy(&exact, 0.5, 5), rig.lens, Freedom::default())).unwrap();
    assert_recovers(&sol, &rig, 4.0 * 0.5 * sd_place, 4.0 * 0.5 * sd_turn);
}

// ---- a bad point ----

#[test]
fn one_bad_point_is_named() {
    let rig = audience_rig(1.6, [0.0, 0.2]);
    let world = deep_points(&rig, 10, 9.0);
    for sigma in [0.0, 0.5] {
        let mut points = noisy(&rig.points(&world), sigma, 21);
        points[3].pixel[0] += 12.0;
        let entered = LensFit {
            throw: 1.5,
            shift: [0.0; 2],
            aspect: HD,
        };
        let sol = solve(&rig.problem(points, entered, Freedom::default())).unwrap();
        let w = sol
            .worst
            .unwrap_or_else(|| panic!("{sigma}: none named: {sol:?}"));
        assert_eq!(w.point, 3);
        // Solved without it, the others put it where it really is.
        assert!((w.off_px - 12.0).abs() < 1.0 + 4.0 * sigma, "{w:?}");
        assert!(w.rest_px <= 2.0 * sigma + 1e-6, "{w:?}");
        let words = sol.words();
        assert!(
            words.starts_with(&format!(
                "Point 4 is {:.0} px off: is it the right spot?",
                w.off_px
            )),
            "{words}"
        );
        // Switched off, the rest line up.
        let mut points = noisy(&rig.points(&world), sigma, 21);
        points[3].pixel[0] += 12.0;
        points[3].on = false;
        let sol = solve(&rig.problem(points, entered, Freedom::default())).unwrap();
        assert!(sol.worst.is_none(), "{sigma}: {:?}", sol.worst);
        assert_eq!(sol.residuals[3], None);
        assert!(sol.max_px <= 4.0 * sigma + 1e-6);
    }
}

#[test]
fn with_four_points_a_bad_one_cant_be_told() {
    let rig = audience_rig(1.6, [0.0, 0.0]);
    let world = deep_points(&rig, 4, 8.0);
    let mut points = rig.points(&world);
    points[2].pixel[1] += 15.0;
    let sol = solve(&rig.problem(points, rig.lens, Freedom::default())).unwrap();
    assert!(sol.worst.is_none());
    // The pose soaks most of it up: the fit alone wouldn't show it.
    assert!(sol.max_px < 15.0);
    assert!(sol.notes.contains(&Note::CantTellWhich), "{sol:?}");
    assert!(sol.words().contains("can't be told from the others"));
}

// ---- refused ----

#[test]
fn sets_that_cant_be_solved_are_refused_in_words() {
    let rig = audience_rig(1.6, [0.0, 0.0]);
    let world = deep_points(&rig, 6, 8.0);
    let good = rig.points(&world);
    let refuse = |points: Vec<LinePoint>| {
        solve(&rig.problem(points, rig.lens, Freedom::default())).unwrap_err()
    };

    let r = refuse(good[..3].to_vec());
    assert_eq!(r, Refusal::TooFew { have: 3 });
    assert_eq!(
        r.words(),
        "Line-up needs at least four points; there are 3."
    );
    // Switched-off points don't count.
    let mut off = good[..5].to_vec();
    off[0].on = false;
    off[4].on = false;
    assert_eq!(refuse(off), Refusal::TooFew { have: 3 });

    let mut same = good.clone();
    same[4].world = linalg::add(same[1].world, [0.0002, 0.0, 0.0]);
    let r = refuse(same);
    assert_eq!(r, Refusal::SameSpot { a: 1, b: 4 });
    assert!(r.words().starts_with("Points 2 and 5 are the same spot"));

    let mut same = good.clone();
    same[5].pixel = same[0].pixel;
    assert_eq!(refuse(same), Refusal::SamePixel { a: 0, b: 5 });

    let mut nan = good.clone();
    nan[2].world[1] = f64::NAN;
    let r = refuse(nan);
    assert_eq!(r, Refusal::NotANumber { point: 2 });
    assert!(r.words().starts_with("Point 3 "));

    // Along one batten.
    let line: Vec<V3> = (0..6).map(|i| [i as f64 - 2.5, 5.0, -2.0]).collect();
    let r = refuse(rig.points(&line));
    assert_eq!(r, Refusal::InALine);
    assert!(r.words().contains("all in one line on the model"));

    // On the floor, seen from the floor's own height: a line in the picture.
    let low = Rig::new([0.0, 0.0, 12.0], 0.0, 0.0, 0.0, 1.6, [0.0, 0.3]);
    let floor: Vec<V3> = [
        (-3.0, 0.0),
        (2.0, -1.0),
        (0.5, -4.0),
        (-1.0, -6.0),
        (3.0, -3.0),
    ]
    .iter()
    .map(|&(x, z)| [x, 0.0, z])
    .collect();
    let r = solve(&low.problem(low.points(&floor), low.lens, Freedom::default())).unwrap_err();
    assert_eq!(r, Refusal::InALineInPicture);

    // Four points on a screen, three of them along its bottom edge.
    let flat: Vec<V3> = [(0.1, 0.9), (0.5, 0.9), (0.9, 0.9), (0.5, 0.2)]
        .iter()
        .map(|&(u, v)| rig.on_plane(u, v, [0.0, 0.0, 0.0], [0.0, 0.0, 1.0]))
        .collect();
    let r = refuse(rig.points(&flat));
    assert_eq!(r, Refusal::ThreeInALine { a: 0, b: 1, c: 2 });
    assert!(r.words().starts_with("Points 1, 2 and 3 are in a line"));

    let mut p = rig.problem(good.clone(), rig.lens, Freedom::default());
    p.pixels = (0, 1080);
    assert_eq!(solve(&p).unwrap_err(), Refusal::NoPicture);

    // Every refusal has words.
    for r in [
        Refusal::TooFew { have: 1 },
        Refusal::Behind,
        Refusal::Unknown,
        Refusal::InALineInPicture,
        Refusal::NoPicture,
    ] {
        assert!(!r.words().is_empty());
    }
    assert_eq!(
        Refusal::TooFew { have: 1 }.words(),
        "Line-up needs at least four points; there is 1."
    );
}

#[test]
fn no_input_panics() {
    let rig = audience_rig(1.6, [0.0, 0.0]);
    let world = deep_points(&rig, 8, 8.0);
    let base = rig.points(&world);
    let weird = [
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NAN,
        1e300,
        -1e300,
        0.0,
    ];
    for v in weird {
        for k in 0..5 {
            let mut points = base.clone();
            match k {
                0 => points[1].world[0] = v,
                1 => points[2].pixel[1] = v,
                2 => points.iter_mut().for_each(|p| p.pixel = [v, v]),
                3 => points.iter_mut().for_each(|p| p.world = [v, 0.0, v]),
                _ => {}
            }
            let mut lens = rig.lens;
            if k == 4 {
                lens = LensFit {
                    throw: v,
                    shift: [v, -v],
                    aspect: v,
                };
            }
            let mut p = rig.problem(points, lens, Freedom::default());
            p.start = Some(Pose {
                position: [v, 0.0, 0.0],
                yaw: v,
                pitch: v,
                roll: v,
            });
            let _ = solve(&p).map(|s| s.words()).map_err(|r| r.words());
        }
    }
    let _ = solve(&Problem::default());
}

// ---- the same every time, and carried ----

#[test]
fn the_same_problem_gives_the_same_answer() {
    let rig = audience_rig(1.6, [0.1, 0.2]);
    let world = deep_points(&rig, 10, 9.0);
    let mut points = noisy(&rig.points(&world), 1.0, 99);
    points[6].pixel[1] -= 9.0;
    let problem = rig.problem(points, LensFit::default(), Freedom::default());
    let a = solve(&problem).unwrap();
    for _ in 0..3 {
        let b = solve(&problem).unwrap();
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap()
        );
    }
    // A problem that came over the network (JSON, which may give a number
    // back a bit different in its last digit) solves to the same answer.
    let carried: Problem = serde_json::from_str(&serde_json::to_string(&problem).unwrap()).unwrap();
    let b = solve(&carried).unwrap();
    assert_eq!(b.points_key, a.points_key);
    assert_eq!(
        (b.rung, b.worst.map(|w| w.point)),
        (a.rung, a.worst.map(|w| w.point))
    );
    assert!(linalg::norm(linalg::sub(a.pose.position, b.pose.position)) < 1e-9);
}

#[test]
fn a_solution_travels_and_applies_on_a_real_projector() {
    let rig = audience_rig(1.6, [0.1, 0.3]);
    let world = deep_points(&rig, 9, 9.0);
    let points = rig.points(&world);
    let mut line_up = LineUp {
        points: points.clone(),
        pixels: PIXELS,
        ..LineUp::default()
    };
    // A projector entered roughly: a metre out, the lens as it came.
    let mut proj = Projector::new(ProjectorId(4), "Balcony".into());
    proj.placement = Placement {
        position: [2.5, 3.5, 13.0],
        yaw: 5.0,
        pitch: -5.0,
        roll: 0.0,
    };
    proj.resolution = Some(PIXELS);
    let sol = line_up.solve(&proj).unwrap().clone();
    assert!(line_up.is_current());
    // Sent as JSON, read on another computer, applied there.
    let json = serde_json::to_string(&line_up).unwrap();
    let there: LineUp = serde_json::from_str(&json).unwrap();
    assert!(there.is_current());
    let s = there.solved.as_ref().unwrap();
    assert_eq!(s.points_key, sol.points_key);
    assert!(linalg::norm(linalg::sub(s.pose.position, sol.pose.position)) < 1e-12);
    proj.placement = s.placement();
    proj.lens = s.lens_onto(&proj.lens, PIXELS);
    assert_eq!(proj.lens.aspect, None);
    let aspect = proj.aspect(PIXELS);
    // NewQ's own sums now put every point where it was placed, to the f32
    // rounding of the show's numbers (about a millionth of the picture).
    for p in &points {
        let ([u, v], _) = proj.in_picture(p.world.map(|v| v as f32), aspect).unwrap();
        let (x, y) = (u as f64 * 1920.0, v as f64 * 1080.0);
        assert!(
            (x - p.pixel[0]).hypot(y - p.pixel[1]) < 0.02,
            "{:?} vs {:?}",
            [x, y],
            p.pixel
        );
    }
    // A point nudged: the kept answer is no longer for these points.
    let mut moved = there.clone();
    moved.points[2].pixel[0] += 1.0;
    assert!(!moved.is_current());
}

#[test]
fn line_ups_read_leniently_and_write_little() {
    let p = LinePoint::new([1.0, 2.0, 3.0], [10.0, 20.0]);
    let json = serde_json::to_string(&p).unwrap();
    assert!(!json.contains("\"on\""), "{json}");
    let back: LinePoint = serde_json::from_str(&json).unwrap();
    assert_eq!(back, p);
    // A later NewQ's fields are kept and written back.
    let later: LinePoint =
        serde_json::from_str(r#"{"world":[0,0,0],"pixel":[1,1],"surface":7}"#).unwrap();
    assert!(
        serde_json::to_string(&later)
            .unwrap()
            .contains("\"surface\":7")
    );
    assert!(later.on);
    // Unknown rungs, notes and refusals don't stop a read.
    let s: Solution =
        serde_json::from_str(r#"{"rung":"Fisheye","notes":[{"note":"Wobbly"},{"note":"Flat"}]}"#)
            .unwrap();
    assert_eq!(s.rung, Rung::Unknown);
    assert_eq!(s.notes, vec![Note::Unknown, Note::Flat]);
    let r: Refusal = serde_json::from_str(r#"{"why":"Cloudy"}"#).unwrap();
    assert_eq!(r, Refusal::Unknown);
    let empty: LineUp = serde_json::from_str("{}").unwrap();
    assert_eq!(empty, LineUp::default());
    assert_eq!(
        serde_json::to_string(&LineUp::default()).unwrap(),
        r#"{"pixels":[0,0]}"#
    );
}

/// Many made-up projectors, all solved exactly: a property test with a
/// fixed seed.
#[test]
fn many_projectors_are_found_exactly() {
    let mut rng = Rng(2026);
    let mut whole = 0;
    let mut slowest = std::time::Duration::ZERO;
    for case in 0..60 {
        let preset = LensPreset::ALL[case % LensPreset::ALL.len()];
        let (a, b) = preset.range();
        let throw = rng.range(a as f64, b as f64 + 1e-9);
        let shift = [rng.range(-0.3, 0.3), rng.range(-0.5, 0.5)];
        let rig = Rig::new(
            [
                rng.range(-10.0, 10.0),
                rng.range(1.0, 12.0),
                rng.range(-20.0, 20.0),
            ],
            rng.range(-180.0, 180.0),
            rng.range(-90.0, 90.0),
            rng.range(-30.0, 30.0),
            throw,
            shift,
        );
        let n = 8 + case % 5;
        let distance = rng.range(3.0, 10.0) * throw.max(0.5);
        let world: Vec<V3> = (0..n)
            .map(|_| {
                rig.spot(
                    rng.range(0.05, 0.95),
                    rng.range(0.05, 0.95),
                    distance * rng.range(0.7, 1.3),
                )
            })
            .collect();
        let entered = LensFit {
            throw: 1.5,
            shift: [0.0; 2],
            aspect: HD,
        };
        let problem = rig.problem(rig.points(&world), entered, Freedom::default());
        let began = std::time::Instant::now();
        let answer = solve(&problem);
        slowest = slowest.max(began.elapsed());
        match answer {
            Ok(sol) if sol.rung == Rung::Shift => {
                assert_recovers(&sol, &rig, 1e-6, 1e-6);
                assert_lens(&sol, &rig.lens, 1e-6);
                assert!(!sol.capped, "case {case}");
                whole += 1;
            }
            // Random points (a long lens most of all) can say too little
            // about the shift. Then the solve must say so, and only because
            // the points truly don't pin it: the solver's sd at the truth is
            // over the bar. Never because the refinement got lost.
            Ok(sol) => {
                assert!(
                    sol.notes.iter().any(|n| matches!(n, Note::Unsure { .. })),
                    "case {case}: {sol:?}"
                );
                let pairs: Vec<model::Pair> =
                    problem.points.iter().map(|p| (p.world, p.pixel)).collect();
                let (_, sp, _) = linalg::spread(&world);
                let sd =
                    model::spreads(&rig.state(), Rung::Shift, px(PIXELS), &pairs, sp[2].sqrt())
                        .unwrap();
                let worst = (sd[6] / rig.lens.throw).max(sd[7]).max(sd[8]);
                assert!(worst > SURE * 0.999, "case {case}: sd {sd:?}, {sol:?}");
            }
            Err(r) => panic!("case {case}: {r:?}"),
        }
    }
    // Not a timing test (CI oversleeps): what a solve costs, for the report.
    eprintln!("{whole} of 60 solved whole; slowest solve {slowest:?}");
    assert!(whole >= 20, "{whole} of 60");
}

#[test]
fn project_is_what_the_solver_fits_and_what_newq_draws() {
    let rig = audience_rig(0.5, [0.15, -0.2]);
    let proj = projector_of(&rig);
    let aspect = proj.aspect(PIXELS);
    let s = model::state_of(&proj.placement, &proj.lens, PIXELS);
    let mut rng = Rng(8);
    for _ in 0..100 {
        let x = rig.spot(
            rng.range(0.0, 1.0),
            rng.range(0.0, 1.0),
            rng.range(1.0, 20.0),
        );
        let x32 = x.map(|v| v as f32);
        let ours = project(&proj.placement, &proj.lens, PIXELS, x32).unwrap();
        let fitted = model::project(&s, px(PIXELS), x32.map(|v| v as f64)).unwrap();
        assert_eq!(ours, fitted.map(|v| v as f32));
        let ([u, v], _) = proj.in_picture(x32, aspect).unwrap();
        assert!((ours[0] - u * 1920.0).abs() < 0.01 && (ours[1] - v * 1080.0).abs() < 0.01);
    }
    let behind = linalg::sub(
        rig.pose.position,
        linalg::mul_v(&rig.pose.turn(), [0.0, 0.0, -1.0]),
    );
    assert_eq!(
        project(
            &proj.placement,
            &proj.lens,
            PIXELS,
            behind.map(|v| v as f32)
        ),
        None
    );
    // No picture size still answers (as one pixel), never panics.
    let _ = project(&proj.placement, &proj.lens, (0, 0), [0.0; 3]);
}

#[test]
fn a_stored_answer_reprojects_within_a_tenth_of_a_pixel() {
    // Noisy points, so the answer isn't the truth: the show's f32 copy of
    // it must put each point where the f64 answer does.
    let rig = audience_rig(1.6, [0.1, 0.3]);
    let world = deep_points(&rig, 12, 9.0);
    let points = noisy(&rig.points(&world), 1.0, 42);
    let entered = LensFit {
        throw: 1.5,
        shift: [0.0; 2],
        aspect: HD,
    };
    let sol = solve(&rig.problem(points.clone(), entered, Freedom::default())).unwrap();
    let f64_state = State {
        r: sol.pose.turn(),
        c: sol.pose.position,
        lens: sol.lens,
    };
    // Stored in a show (JSON of the f32 types) and read back.
    let placement: Placement =
        serde_json::from_str(&serde_json::to_string(&sol.placement()).unwrap()).unwrap();
    let lens: Lens = serde_json::from_str(
        &serde_json::to_string(&sol.lens_onto(&Lens::default(), PIXELS)).unwrap(),
    )
    .unwrap();
    for p in &points {
        let a = model::project(&f64_state, px(PIXELS), p.world).unwrap();
        let b = project(&placement, &lens, PIXELS, p.world.map(|v| v as f32)).unwrap();
        let d = (a[0] - b[0] as f64).hypot(a[1] - b[1] as f64);
        assert!(d < 0.1, "{d} px");
    }
}

#[test]
fn a_mirrored_picture_is_refused_in_words() {
    // Through a mirror (or with the projector's own rear setting on), the
    // picture is flipped left to right: no placement gives that.
    let rig = audience_rig(1.6, [0.0, 0.0]);
    let world = deep_points(&rig, 8, 8.0);
    let flipped: Vec<LinePoint> = rig
        .points(&world)
        .into_iter()
        .map(|mut p| {
            p.pixel[0] = 1920.0 - p.pixel[0];
            p
        })
        .collect();
    let r = solve(&rig.problem(flipped, rig.lens, Freedom::default())).unwrap_err();
    assert_eq!(r, Refusal::Mirrored);
    assert!(r.words().contains("flipped left to right"));
    // Flipped both ways is only a turn (a projector hung upside down): fine.
    let upside: Vec<LinePoint> = rig
        .points(&world)
        .into_iter()
        .map(|mut p| {
            p.pixel = [1920.0 - p.pixel[0], 1080.0 - p.pixel[1]];
            p
        })
        .collect();
    let sol = solve(&rig.problem(upside, rig.lens, Freedom::default())).unwrap();
    assert!(sol.max_px < 1e-6, "{sol:?}");
    assert!((sol.pose.roll.abs() - 180.0).abs() < 2.0 || sol.pose.yaw.abs() > 90.0);
}

#[test]
fn a_rigid_fit_finds_the_turn_and_the_move() {
    let r = linalg::from_euler(25.0, -40.0, 70.0);
    let t = [1.5, -2.0, 3.25];
    let from: Vec<V3> = UV.iter().map(|&(u, v, k)| [u * 4.0, v * 3.0, k]).collect();
    let to: Vec<V3> = from
        .iter()
        .map(|p| linalg::add(linalg::mul_v(&r, *p), t))
        .collect();
    let (rot, tr, rms) = fit_rigid(&from, &to).unwrap();
    assert!(linalg::angle_between(&rot, &r) < 1e-12);
    assert!(linalg::norm(linalg::sub(tr, t)) < 1e-12);
    assert!(rms < 1e-12);
    // Three points are enough; a line, two points or a NaN aren't.
    assert!(fit_rigid(&from[..3], &to[..3]).is_some());
    assert!(fit_rigid(&from[..2], &to[..2]).is_none());
    let line: Vec<V3> = (0..5).map(|i| [i as f64, 0.0, 0.0]).collect();
    assert!(fit_rigid(&line, &line).is_none());
    let mut nan = to.clone();
    nan[1][2] = f64::NAN;
    assert!(fit_rigid(&from, &nan).is_none());
    assert!(fit_rigid(&from, &to[..4]).is_none());
    // A mirror image is never answered with a mirror.
    let mirrored: Vec<V3> = from.iter().map(|p| [-p[0], p[1], p[2]]).collect();
    let (rot, _, rms) = fit_rigid(&from, &mirrored).unwrap();
    assert!(linalg::det(&rot) > 0.0 && rms > 0.1);
}

#[test]
fn a_nudge_solves_from_the_last_answer() {
    // What the line-up page does at every nudge: the projector's placement
    // (the last answer) is the start, the lens is still as entered, one point
    // moved a pixel. The same answer as a solve from nothing, and the time it
    // took printed (not asserted: CI oversleeps).
    let rig = audience_rig(1.6, [0.1, 0.3]);
    let entered = LensFit {
        throw: 1.5,
        shift: [0.0; 2],
        aspect: HD,
    };
    let mut rng = Rng(77);
    for (n, sigma, bad) in [
        (12, 0.5, false),
        (12, 2.0, false),
        (36, 0.5, false),
        (36, 2.0, true),
    ] {
        let world: Vec<V3> = (0..n)
            .map(|_| {
                rig.spot(
                    rng.range(0.05, 0.95),
                    rng.range(0.05, 0.95),
                    9.0 * rng.range(0.7, 1.3),
                )
            })
            .collect();
        let mut points = noisy(&rig.points(&world), sigma, n as u64);
        if bad {
            points[7].pixel[1] += 20.0;
        }
        let mut problem = rig.problem(points, entered, Freedom::default());
        let first = solve(&problem).unwrap();
        problem.start = Some(first.pose);
        problem.points[5].pixel[0] += 1.0;
        let began = std::time::Instant::now();
        let nudged = solve(&problem).unwrap();
        let took = began.elapsed();
        problem.start = None;
        let fresh = solve(&problem).unwrap();
        eprintln!("{n} points, {sigma} px, bad point {bad}: a nudge took {took:?} (this build)");
        assert_eq!(nudged.rung, fresh.rung);
        assert_eq!(nudged.worst.map(|w| w.point), fresh.worst.map(|w| w.point));
        assert_eq!(nudged.worst.map(|w| w.point), bad.then_some(7));
        assert!(!nudged.capped);
        assert!(linalg::norm(linalg::sub(nudged.pose.position, fresh.pose.position)) < 1e-6);
        assert!((nudged.lens.throw - fresh.lens.throw).abs() < 1e-6);
    }
}

#[test]
fn a_very_wide_picture_is_drawn_and_fitted_alike() {
    // Pixels wider than a lens's own aspect may be (12.8:1): the solver
    // uses the shape NewQ draws, as `sane_aspect` keeps it.
    let pixels = (3840, 300);
    let mut rig = audience_rig(1.6, [0.0, 0.0]);
    rig.pixels = pixels;
    rig.lens.aspect = 12.8;
    let proj = projector_of(&rig);
    let s = State {
        r: rig.pose.turn(),
        c: rig.pose.position,
        lens: LensFit::of(&proj, pixels),
    };
    assert!((s.lens.aspect - 12.8).abs() < 1e-6);
    let x = rig.spot(0.3, 0.6, 8.0);
    let ours = model::project(&s, px(pixels), x).unwrap();
    let ([u, v], _) = proj
        .in_picture(x.map(|v| v as f32), proj.aspect(pixels))
        .unwrap();
    assert!((ours[0] - u as f64 * 3840.0).abs() < 0.01);
    assert!((ours[1] - v as f64 * 300.0).abs() < 0.01);
}
