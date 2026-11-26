//! Starting guesses for the refinement: a flat set's homography, the 3x4
//! matrix of a set that isn't flat (normalised DLT, split by RQ), and a
//! fixed ring of places round the points, each turned to face them (Horn).
//! Every one works on the lens's own frame (looking down −Z), never a
//! textbook camera's +Z, so no guess comes out mirrored.

// Index loops over the small fixed matrices read like the sums they are.
#![allow(clippy::needless_range_loop)]

use super::LensFit;
use super::linalg::{self, M3, V3};
use super::model::{self, Pair, State};

/// Hartley's normalisation for 2D points: the matrix (row-major 3x3) that
/// moves their middle to 0 and their mean distance to √2.
fn normalise2(points: &[[f64; 2]]) -> M3 {
    let n = points.len().max(1) as f64;
    let c = points
        .iter()
        .fold([0.0; 2], |a, p| [a[0] + p[0] / n, a[1] + p[1] / n]);
    let d = points
        .iter()
        .map(|p| (p[0] - c[0]).hypot(p[1] - c[1]))
        .sum::<f64>()
        / n;
    let s = if d > 1e-300 {
        std::f64::consts::SQRT_2 / d
    } else {
        1.0
    };
    [[s, 0.0, -s * c[0]], [0.0, s, -s * c[1]], [0.0, 0.0, 1.0]]
}

/// The same in 3D (row-major 4x4), mean distance √3.
fn normalise3(points: &[V3]) -> [[f64; 4]; 4] {
    let n = points.len().max(1) as f64;
    let c = points
        .iter()
        .fold([0.0; 3], |a, p| linalg::add(a, linalg::scale(*p, 1.0 / n)));
    let d = points
        .iter()
        .map(|p| linalg::norm(linalg::sub(*p, c)))
        .sum::<f64>()
        / n;
    let s = if d > 1e-300 { 3f64.sqrt() / d } else { 1.0 };
    [
        [s, 0.0, 0.0, -s * c[0]],
        [0.0, s, 0.0, -s * c[1]],
        [0.0, 0.0, s, -s * c[2]],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

fn apply2(t: &M3, p: [f64; 2]) -> [f64; 2] {
    let v = linalg::mul_v(t, [p[0], p[1], 1.0]);
    [v[0] / v[2], v[1] / v[2]]
}

/// The eigenvector of `aᵀa`'s smallest eigenvalue (`a` has `cols` columns):
/// the least-squares null vector.
fn null_vector(rows: &[Vec<f64>], cols: usize) -> Vec<f64> {
    let mut ata = vec![0.0; cols * cols];
    for row in rows {
        for p in 0..cols {
            for q in 0..cols {
                ata[p * cols + q] += row[p] * row[q];
            }
        }
    }
    let (_, vectors) = linalg::eigen_symmetric(&ata, cols);
    vectors[0].clone()
}

/// A flat set's guess: the plane through the points (its two widest axes),
/// the homography from it to the lens's directions, and the turn and place
/// that make it, with the sign that puts the points in front of the lens.
pub fn homography(pairs: &[Pair], lens: &LensFit, pixels: (f64, f64)) -> Option<State> {
    let world: Vec<V3> = pairs.iter().map(|p| p.0).collect();
    let (c, _, axes) = linalg::spread(&world);
    let e1 = axes[2];
    let e2 = axes[1];
    let e3 = linalg::cross(e1, e2); // a right-handed frame on the plane
    let plane: Vec<[f64; 2]> = world
        .iter()
        .map(|x| {
            let d = linalg::sub(*x, c);
            [linalg::dot(d, e1), linalg::dot(d, e2)]
        })
        .collect();
    // (x, y, −1) is (−x, −y, 1) up to scale: the homography's target.
    let seen: Vec<[f64; 2]> = pairs
        .iter()
        .map(|p| {
            let b = model::bearing(lens, pixels, p.1);
            [-b[0], -b[1]]
        })
        .collect();
    let t1 = normalise2(&plane);
    let t2 = normalise2(&seen);
    let rows: Vec<Vec<f64>> = plane
        .iter()
        .zip(&seen)
        .flat_map(|(a, m)| {
            let a = apply2(&t1, *a);
            let m = apply2(&t2, *m);
            let a = [a[0], a[1], 1.0];
            let mut r1 = vec![0.0; 9];
            let mut r2 = vec![0.0; 9];
            for k in 0..3 {
                r1[3 + k] = -a[k];
                r1[6 + k] = m[1] * a[k];
                r2[k] = a[k];
                r2[6 + k] = -m[0] * a[k];
            }
            [r1, r2]
        })
        .collect();
    let h = null_vector(&rows, 9);
    let hn = [[h[0], h[1], h[2]], [h[3], h[4], h[5]], [h[6], h[7], h[8]]];
    let h = linalg::mul(&linalg::mul(&linalg::inverse(&t2)?, &hn), &t1);
    let col = |k: usize| [h[0][k], h[1][k], h[2][k]];
    let (h1, h2, h3) = (col(0), col(1), col(2));
    let size = (linalg::norm(h1) + linalg::norm(h2)) / 2.0;
    if !(size.is_finite() && size > 1e-300) {
        return None;
    }
    // p = s·H·(a, b, 1): choose s's sign so the points' depth (−p_z) is
    // positive for most of them.
    let in_front = plane
        .iter()
        .filter(|a| linalg::mul_v(&h, [a[0], a[1], 1.0])[2] < 0.0)
        .count();
    let s = if in_front * 2 >= plane.len() {
        1.0 / size
    } else {
        -1.0 / size
    };
    let r1 = linalg::scale(h1, s);
    let r2 = linalg::scale(h2, s);
    let r3 = linalg::cross(r1, r2);
    // Rᵀ·[e1 e2 e3] = [r1 r2 r3], so Rᵀ = [r1 r2 r3]·[e1 e2 e3]ᵀ.
    let q = [
        [r1[0], r2[0], r3[0]],
        [r1[1], r2[1], r3[1]],
        [r1[2], r2[2], r3[2]],
    ];
    let e = [
        [e1[0], e2[0], e3[0]],
        [e1[1], e2[1], e3[1]],
        [e1[2], e2[2], e3[2]],
    ];
    let rt = linalg::nearest_rotation(&linalg::mul(&q, &linalg::transpose(&e)));
    let r = linalg::transpose(&rt);
    // Rᵀ(c − C) = s·h3.
    let pos = linalg::sub(c, linalg::mul_v(&r, linalg::scale(h3, s)));
    let state = State {
        r,
        c: pos,
        lens: *lens,
    };
    state.c.iter().all(|v| v.is_finite()).then_some(state)
}

/// Six or more points that aren't flat: the 3x4 matrix taking the room to
/// the picture (normalised DLT), split into the lens, the turn and the
/// place. Its lens is a guess too, kept within a lens's bounds.
pub fn dlt(pairs: &[Pair], pixels: (f64, f64)) -> Option<State> {
    let world: Vec<V3> = pairs.iter().map(|p| p.0).collect();
    let seen: Vec<[f64; 2]> = pairs.iter().map(|p| p.1).collect();
    let t3 = normalise3(&world);
    let t2 = normalise2(&seen);
    let rows: Vec<Vec<f64>> = world
        .iter()
        .zip(&seen)
        .flat_map(|(x, m)| {
            let x = [x[0], x[1], x[2], 1.0];
            let x: Vec<f64> = (0..4)
                .map(|r| (0..4).map(|k| t3[r][k] * x[k]).sum())
                .collect();
            let m = apply2(&t2, *m);
            let mut r1 = vec![0.0; 12];
            let mut r2 = vec![0.0; 12];
            for k in 0..4 {
                r1[k] = x[k];
                r1[8 + k] = -m[0] * x[k];
                r2[4 + k] = x[k];
                r2[8 + k] = -m[1] * x[k];
            }
            [r1, r2]
        })
        .collect();
    let v = null_vector(&rows, 12);
    let pn = [
        [v[0], v[1], v[2], v[3]],
        [v[4], v[5], v[6], v[7]],
        [v[8], v[9], v[10], v[11]],
    ];
    let t2i = linalg::inverse(&t2)?;
    // P = T2⁻¹ · Pn · T3.
    let mut p = [[0.0; 4]; 3];
    for r in 0..3 {
        for c in 0..4 {
            p[r][c] = (0..3)
                .map(|i| t2i[r][i] * (0..4).map(|k| pn[i][k] * t3[k][c]).sum::<f64>())
                .sum();
        }
    }
    let mut m = [
        [p[0][0], p[0][1], p[0][2]],
        [p[1][0], p[1][1], p[1][2]],
        [p[2][0], p[2][1], p[2][2]],
    ];
    let mut p4 = [p[0][3], p[1][3], p[2][3]];
    if linalg::det(&m) < 0.0 {
        m = m.map(|row| row.map(|v| -v));
        p4 = linalg::scale(p4, -1.0);
    }
    // m = Kpos · D · Rᵀ with D = diag(1, −1, −1): the lens looks down −Z and
    // its picture's v runs down.
    let (k, q) = linalg::rq(&m)?;
    let d = [[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, -1.0]];
    let rt = linalg::nearest_rotation(&linalg::mul(&d, &q));
    let r = linalg::transpose(&rt);
    let c = linalg::scale(linalg::mul_v(&linalg::inverse(&m)?, p4), -1.0);
    let k = k.map(|row| row.map(|v| v / k[2][2]));
    let throw = k[0][0] / pixels.0;
    let lens = LensFit {
        throw,
        shift: [0.5 - k[0][2] / pixels.0, k[1][2] / pixels.1 - 0.5],
        aspect: k[1][1] / (pixels.1 * throw),
    };
    if !(lens.throw.is_finite() && lens.shift.iter().chain(&c).all(|v| v.is_finite())) {
        return None;
    }
    let state = State {
        r,
        c,
        lens: lens.sane(),
    };
    Some(state)
}

/// Directions spread evenly over a sphere (a Fibonacci lattice): the same
/// list every time.
fn sphere(n: usize) -> Vec<V3> {
    let golden = std::f64::consts::PI * (3.0 - 5f64.sqrt());
    (0..n)
        .map(|i| {
            let y = 1.0 - 2.0 * (i as f64 + 0.5) / n as f64;
            let r = (1.0 - y * y).max(0.0).sqrt();
            let a = golden * i as f64;
            [r * a.cos(), y, r * a.sin()]
        })
        .collect()
}

/// The turn that best points each point's picture direction at it from
/// `c` (Horn), with the lens as entered.
fn facing(pairs: &[Pair], lens: &LensFit, pixels: (f64, f64), c: V3) -> Option<M3> {
    let mut m = [[0.0; 3]; 3];
    for (x, px) in pairs {
        let w = linalg::unit(linalg::sub(*x, c))?;
        let b = linalg::unit(model::bearing(lens, pixels, *px))?;
        for r in 0..3 {
            for k in 0..3 {
                m[r][k] += w[r] * b[k];
            }
        }
    }
    Some(linalg::nearest_rotation(&m))
}

/// Places all round the points, as far off as their spread in the picture
/// says (and nearer and further), each turned to face them: the guesses for
/// a set no linear method suits. The `keep` best by their fit, best first.
pub fn ring(pairs: &[Pair], lens: &LensFit, pixels: (f64, f64), keep: usize) -> Vec<State> {
    let world: Vec<V3> = pairs.iter().map(|p| p.0).collect();
    let (middle, values, _) = linalg::spread(&world);
    let radius = values[2].max(0.0).sqrt().max(1e-3);
    // How wide the points are in the picture, as an angle from their middle.
    let bearings: Vec<V3> = pairs
        .iter()
        .filter_map(|p| linalg::unit(model::bearing(lens, pixels, p.1)))
        .collect();
    let Some(mean) = linalg::unit(bearings.iter().fold([0.0; 3], |a, b| linalg::add(a, *b)))
    else {
        return Vec::new();
    };
    let half = bearings
        .iter()
        .map(|b| linalg::dot(*b, mean).clamp(-1.0, 1.0).acos())
        .map(|a| a * a)
        .sum::<f64>()
        / bearings.len().max(1) as f64;
    let half = half.sqrt().max(1e-3);
    let far = (radius / half.tan()).clamp(radius * 0.1, radius * 1e4);
    let mut tried: Vec<(f64, State)> = Vec::new();
    for dir in sphere(64) {
        for k in [0.5, 1.0, 2.0] {
            let c = linalg::add(middle, linalg::scale(dir, far * k));
            let Some(r) = facing(pairs, lens, pixels, c) else {
                continue;
            };
            let s = State { r, c, lens: *lens };
            if let Some(cost) = model::cost(&s, pixels, pairs) {
                tried.push((cost, s));
            }
        }
    }
    // A stable sort: equal costs keep the list's order.
    tried.sort_by(|a, b| a.0.total_cmp(&b.0));
    tried.into_iter().take(keep).map(|(_, s)| s).collect()
}
