//! The projection the solver fits, the same as `Projector::in_picture`'s in
//! f64, and the Levenberg-Marquardt refinement over it.

use super::linalg::{self, M3, V3};
use super::{LensFit, Pose, Rung};
use crate::stage3d::Placement;
use crate::stage3d::projector::{Lens, sane_aspect};

/// How near the lens a point may be and still count as in front (metres;
/// `Projector::in_picture`'s own limit).
pub const IN_FRONT: f64 = 1e-6;

/// A projector being fitted: its turn (the lens's frame to the world, as
/// `Placement::matrix`), where the lens is, and the lens.
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub r: M3,
    pub c: V3,
    pub lens: LensFit,
}

/// One pair: a point in the room and where it is in the picture (pixels).
pub type Pair = (V3, [f64; 2]);

/// Where `x` is in the picture, in pixels (`pixels` = its width and height),
/// or None when it is behind the lens. As `Projector::in_picture`, times the
/// picture's size: `u = x·throw − sx + ½`, `v = sy + ½ − y·throw·aspect`, with
/// `(x, y)` the point's lens-frame position over its depth.
pub fn project(s: &State, pixels: (f64, f64), x: V3) -> Option<[f64; 2]> {
    let p = linalg::mul_tv(&s.r, linalg::sub(x, s.c));
    let depth = -p[2];
    if !(depth.is_finite() && depth > IN_FRONT) {
        return None;
    }
    let (x, y) = (p[0] / depth, p[1] / depth);
    let l = &s.lens;
    let u = x * l.throw - l.shift[0] + 0.5;
    let v = l.shift[1] + 0.5 - y * l.throw * l.aspect;
    Some([u * pixels.0, v * pixels.1])
}

/// The state NewQ draws `placement` and `lens` with, for a picture of
/// `pixels` (the lens's own aspect, else the pixels').
pub fn state_of(placement: &Placement, lens: &Lens, pixels: (u32, u32)) -> State {
    let pose = Pose::of(placement);
    let own = pixels.0.max(1) as f32 / pixels.1.max(1) as f32;
    let aspect = sane_aspect(lens.aspect().unwrap_or(own)) as f64;
    State {
        r: pose.turn(),
        c: pose.position,
        lens: LensFit {
            throw: lens.throw() as f64,
            shift: lens.shift().map(|v| v as f64),
            aspect,
        }
        .sane(),
    }
}

/// The lens-frame direction (not one long) through picture point `px`:
/// `project`'s inverse, `(x, y, −1)`.
pub fn bearing(lens: &LensFit, pixels: (f64, f64), px: [f64; 2]) -> V3 {
    let u = px[0] / pixels.0;
    let v = px[1] / pixels.1;
    let x = (u - 0.5 + lens.shift[0]) / lens.throw;
    let y = (lens.shift[1] + 0.5 - v) / (lens.throw * lens.aspect);
    [x, y, -1.0]
}

/// Every pair's error (picture minus where it was put), x then y, in pixels;
/// None when a point is behind the lens.
pub fn residuals(s: &State, pixels: (f64, f64), pairs: &[Pair]) -> Option<Vec<f64>> {
    let mut r = Vec::with_capacity(pairs.len() * 2);
    for (x, px) in pairs {
        let p = project(s, pixels, *x)?;
        r.push(p[0] - px[0]);
        r.push(p[1] - px[1]);
    }
    r.iter().all(|v| v.is_finite()).then_some(r)
}

pub fn cost(s: &State, pixels: (f64, f64), pairs: &[Pair]) -> Option<f64> {
    residuals(s, pixels, pairs).map(|r| r.iter().map(|v| v * v).sum())
}

/// How many numbers a rung solves.
pub fn unknowns(rung: &Rung) -> usize {
    match rung {
        Rung::Pose | Rung::Unknown(_) => 6,
        Rung::Throw => 7,
        Rung::Shift => 9,
        Rung::Aspect => 10,
    }
}

/// `s` moved by `d`: a small turn in the lens's own frame, then the
/// position, then the lens numbers the rung frees (throw, shift x, shift y,
/// aspect), kept within a lens's bounds.
pub fn step(s: &State, d: &[f64]) -> State {
    let r = linalg::mul(&s.r, &linalg::exp([d[0], d[1], d[2]]));
    let c = linalg::add(s.c, [d[3], d[4], d[5]]);
    let mut lens = s.lens;
    if d.len() > 6 {
        lens.throw += d[6];
    }
    if d.len() > 8 {
        lens.shift[0] += d[7];
        lens.shift[1] += d[8];
    }
    if d.len() > 9 {
        lens.aspect += d[9];
    }
    State {
        r,
        c,
        lens: lens.sane(),
    }
}

/// The step `h` each number's derivative is taken over (central
/// differences): `size` is how big the set of points is (metres).
fn steps(s: &State, n: usize, size: f64) -> Vec<f64> {
    let mut h = vec![1e-6, 1e-6, 1e-6];
    h.extend([1e-6 * size.max(1e-3); 3]);
    h.push(1e-6 * s.lens.throw);
    h.extend([1e-6; 2]);
    h.push(1e-6 * s.lens.aspect);
    h.truncate(n);
    h
}

/// The residuals and their Jacobian (`2·pairs × n`, row-major) at `s`.
pub fn jacobian(
    s: &State,
    n: usize,
    pixels: (f64, f64),
    pairs: &[Pair],
    size: f64,
) -> Option<(Vec<f64>, Vec<f64>)> {
    let r0 = residuals(s, pixels, pairs)?;
    let m = r0.len();
    let mut j = vec![0.0; m * n];
    let h = steps(s, n, size);
    for k in 0..n {
        let mut d = vec![0.0; n];
        d[k] = h[k];
        let plus = step(s, &d);
        d[k] = -h[k];
        let minus = step(s, &d);
        // A bound may have stopped one side: divide by the real distance.
        let dist = match k {
            6 => plus.lens.throw - minus.lens.throw,
            7 => plus.lens.shift[0] - minus.lens.shift[0],
            8 => plus.lens.shift[1] - minus.lens.shift[1],
            9 => plus.lens.aspect - minus.lens.aspect,
            _ => 2.0 * h[k],
        };
        if dist.abs() < h[k] * 0.5 {
            return None;
        }
        let rp = residuals(&plus, pixels, pairs)?;
        let rm = residuals(&minus, pixels, pairs)?;
        for i in 0..m {
            j[i * n + k] = (rp[i] - rm[i]) / dist;
        }
    }
    Some((r0, j))
}

/// `jᵀj` (`n x n`) and `jᵀr`.
pub fn normal(j: &[f64], r: &[f64], n: usize) -> (Vec<f64>, Vec<f64>) {
    let m = r.len();
    let mut a = vec![0.0; n * n];
    let mut g = vec![0.0; n];
    for i in 0..m {
        let row = &j[i * n..(i + 1) * n];
        for p in 0..n {
            g[p] += row[p] * r[i];
            for q in 0..n {
                a[p * n + q] += row[p] * row[q];
            }
        }
    }
    (a, g)
}

/// The most rounds a refinement takes (no clock: the same rounds always).
/// Exact data converges in well under this; noisy data in a few dozen.
pub const MAX_ROUNDS: usize = 100;
/// The rounds a starting guess (or a leave-one-out from the answer) gets.
pub const START_ROUNDS: usize = 30;
/// The rounds a leave-one-out gets, from the answer (a point less moves it
/// a little; LM is back within rounding in a few).
pub const LEAVE_OUT_ROUNDS: usize = 10;

/// Levenberg-Marquardt from `s` over the numbers `rung` frees, on the error
/// in pixels. A step that would put a point behind the lens is refused like
/// one that makes the fit worse. None when `s` itself has a point behind.
/// The flag is whether it stopped at `rounds` still moving.
pub fn refine(
    s: &State,
    rung: &Rung,
    pixels: (f64, f64),
    pairs: &[Pair],
    size: f64,
    rounds: usize,
) -> Option<(State, f64, bool)> {
    let n = unknowns(rung);
    let mut s = s.clone();
    let mut cost_now = cost(&s, pixels, pairs)?;
    let mut lambda = 1e-3;
    let mut capped = true;

    for _ in 0..rounds {
        if cost_now < 1e-26 {
            capped = false;
            break;
        }
        let Some((r, j)) = jacobian(&s, n, pixels, pairs, size) else {
            capped = false;
            break;
        };
        let (a, g) = normal(&j, &r, n);
        let mut moved = false;
        let mut last_step = f64::INFINITY;
        while lambda < 1e16 {
            let mut damped = a.clone();
            for i in 0..n {
                damped[i * n + i] += lambda * a[i * n + i].max(1e-12);
            }
            let Some(l) = linalg::cholesky(&damped, n) else {
                lambda *= 10.0;
                continue;
            };
            let d: Vec<f64> = linalg::cholesky_solve(&l, n, &g)
                .into_iter()
                .map(|v| -v)
                .collect();
            let next = step(&s, &d);
            match cost(&next, pixels, pairs) {
                Some(c) if c < cost_now => {
                    // In units of the step each derivative was taken over.
                    let h = steps(&s, n, size);
                    last_step = d
                        .iter()
                        .zip(&h)
                        .map(|(v, h)| (v / h).abs())
                        .fold(0.0, f64::max);
                    let gain = cost_now - c;
                    s = next;
                    cost_now = c;
                    lambda = (lambda / 10.0).max(1e-12);
                    moved = true;
                    if gain <= cost_now * 1e-14 {
                        last_step = 0.0;
                    }
                    break;
                }
                _ => lambda *= 10.0,
            }
        }
        if !moved || last_step < 1e-6 {
            capped = false;
            break;
        }
    }
    s.r = linalg::nearest_rotation(&s.r);
    let c = cost(&s, pixels, pairs)?;
    Some((s, c, capped))
}

/// The standard deviation of each number the rung frees, if each point's
/// place in the picture were a pixel out (`σ² (jᵀj)⁻¹`); None when the
/// points don't fix them at all.
pub fn spreads(
    s: &State,
    rung: &Rung,
    pixels: (f64, f64),
    pairs: &[Pair],
    size: f64,
) -> Option<Vec<f64>> {
    let n = unknowns(rung);
    let (r, j) = jacobian(s, n, pixels, pairs, size)?;
    let (a, _) = normal(&j, &r, n);
    // Scale the columns first so metres, radians and throw mix fairly.
    let d: Vec<f64> = (0..n).map(|i| a[i * n + i].sqrt().max(1e-300)).collect();
    let mut scaled = a.clone();
    for p in 0..n {
        for q in 0..n {
            scaled[p * n + q] /= d[p] * d[q];
        }
    }
    let inv = linalg::inverse_diagonal(&scaled, n)?;
    Some(
        inv.iter()
            .zip(&d)
            .map(|(v, d)| v.max(0.0).sqrt() / d)
            .collect(),
    )
}
