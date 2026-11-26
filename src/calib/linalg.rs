//! The small f64 sums the solver needs, hand-written (no linear algebra
//! crate; the owner's choice): 3-vectors, 3x3 matrices (row-major here,
//! `m[row][col]`, unlike `stage3d::geometry`'s column-major `Mat4`), turns,
//! a symmetric eigen-solve (cyclic Jacobi), Cholesky, the nearest rotation
//! (Horn's quaternion method) and an RQ split. Every loop runs in a fixed
//! order, so the same numbers always give the same answer.

// Index loops over small fixed matrices read like the sums they are.
#![allow(clippy::needless_range_loop)]

pub type V3 = [f64; 3];
pub type M3 = [[f64; 3]; 3];

pub const IDENTITY: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn scale(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

pub fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn norm(a: V3) -> f64 {
    dot(a, a).sqrt()
}

/// `a` one long, or None when it has no length (or isn't a number).
pub fn unit(a: V3) -> Option<V3> {
    let n = norm(a);
    (n.is_finite() && n > 1e-300).then(|| scale(a, 1.0 / n))
}

pub fn mul_v(m: &M3, v: V3) -> V3 {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}

/// `mᵀ v`.
pub fn mul_tv(m: &M3, v: V3) -> V3 {
    let mut out = [0.0; 3];
    for r in 0..3 {
        for c in 0..3 {
            out[c] += m[r][c] * v[r];
        }
    }
    out
}

pub fn mul(a: &M3, b: &M3) -> M3 {
    let mut m = [[0.0; 3]; 3];
    for r in 0..3 {
        for c in 0..3 {
            m[r][c] = a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c];
        }
    }
    m
}

pub fn transpose(a: &M3) -> M3 {
    let mut m = [[0.0; 3]; 3];
    for r in 0..3 {
        for c in 0..3 {
            m[r][c] = a[c][r];
        }
    }
    m
}

pub fn det(m: &M3) -> f64 {
    dot(m[0], cross(m[1], m[2]))
}

/// The inverse, or None when it has none worth having.
pub fn inverse(m: &M3) -> Option<M3> {
    let d = det(m);
    if !(d.is_finite() && d.abs() > 1e-300) {
        return None;
    }
    // Rows of the inverse's transpose are the cross products of rows.
    let c0 = cross(m[1], m[2]);
    let c1 = cross(m[2], m[0]);
    let c2 = cross(m[0], m[1]);
    let mut inv = [[0.0; 3]; 3];
    for r in 0..3 {
        inv[r] = [c0[r] / d, c1[r] / d, c2[r] / d];
    }
    Some(inv)
}

/// About +Y by `a` radians: +Z turns towards +X (as `geometry::rotation_y`).
pub fn rot_y(a: f64) -> M3 {
    let (s, c) = a.sin_cos();
    [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
}

/// About +X: +Y turns towards +Z.
pub fn rot_x(a: f64) -> M3 {
    let (s, c) = a.sin_cos();
    [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]]
}

/// About +Z: +X turns towards +Y.
pub fn rot_z(a: f64) -> M3 {
    let (s, c) = a.sin_cos();
    [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]
}

/// `Placement`'s turn, `Ry(yaw) * Rx(pitch) * Rz(roll)`, from degrees: the
/// lens's frame to the world (its columns are the picture's right, its up
/// and the lens's back, +Z).
pub fn from_euler(yaw: f64, pitch: f64, roll: f64) -> M3 {
    mul(
        &rot_y(yaw.to_radians()),
        &mul(&rot_x(pitch.to_radians()), &rot_z(roll.to_radians())),
    )
}

/// Yaw, pitch and roll (degrees) that give `r` back by `from_euler`. Pitch
/// is within ±90. Looking straight up or down (where only yaw plus roll is
/// defined) roll is 0 and the turn is all yaw. Yaw is found first and pitch
/// and roll from what is left, so the three always rebuild `r`.
pub fn to_euler(r: &M3) -> (f64, f64, f64) {
    let yaw = if r[0][2].hypot(r[2][2]) > 1e-9 {
        r[0][2].atan2(r[2][2])
    } else {
        (-r[2][0]).atan2(r[0][0])
    };
    let n = mul(&rot_y(-yaw), r); // Rx(pitch) * Rz(roll)
    let pitch = (-n[1][2]).atan2(n[2][2]);
    let roll = (-n[0][1]).atan2(n[0][0]);
    (yaw.to_degrees(), pitch.to_degrees(), roll.to_degrees())
}

/// The turn by rotation vector `w` (radians, about its direction).
pub fn exp(w: V3) -> M3 {
    let th = norm(w);
    let k = [[0.0, -w[2], w[1]], [w[2], 0.0, -w[0]], [-w[1], w[0], 0.0]];
    let (a, b) = if th < 1e-8 {
        (1.0 - th * th / 6.0, 0.5 - th * th / 24.0)
    } else {
        (th.sin() / th, (1.0 - th.cos()) / (th * th))
    };
    let k2 = mul(&k, &k);
    let mut m = IDENTITY;
    for r in 0..3 {
        for c in 0..3 {
            m[r][c] += a * k[r][c] + b * k2[r][c];
        }
    }
    m
}

/// The angle (radians) of the turn from `a` to `b`.
pub fn angle_between(a: &M3, b: &M3) -> f64 {
    let d = mul(&transpose(a), b);
    // The skew part's size is sin, the trace gives cos: atan2 of the two is
    // accurate at small angles, where acos of the trace isn't.
    let s = norm([d[2][1] - d[1][2], d[0][2] - d[2][0], d[1][0] - d[0][1]]) / 2.0;
    let c = (d[0][0] + d[1][1] + d[2][2] - 1.0) / 2.0;
    s.atan2(c)
}

/// Eigenvalues (smallest first) and their eigenvectors (`vectors[i]`, one
/// long) of the symmetric `n x n` matrix `a` (row-major), by cyclic Jacobi.
pub fn eigen_symmetric(a: &[f64], n: usize) -> (Vec<f64>, Vec<Vec<f64>>) {
    let mut m = a.to_vec();
    let mut v = vec![0.0; n * n];
    for i in 0..n {
        v[i * n + i] = 1.0;
    }
    let scale: f64 = m.iter().map(|x| x * x).sum::<f64>().sqrt();
    for _sweep in 0..100 {
        let off: f64 = (0..n)
            .flat_map(|p| (0..n).filter(move |&q| q != p).map(move |q| (p, q)))
            .map(|(p, q)| m[p * n + q] * m[p * n + q])
            .sum::<f64>()
            .sqrt();
        if !(off.is_finite() && off > 1e-15 * scale.max(1e-300)) {
            break;
        }
        for p in 0..n {
            for q in p + 1..n {
                let apq = m[p * n + q];
                if apq == 0.0 {
                    continue;
                }
                let app = m[p * n + p];
                let aqq = m[q * n + q];
                let theta = (aqq - app) / (2.0 * apq);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let t = if theta == 0.0 { 1.0 } else { t };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for k in 0..n {
                    let akp = m[k * n + p];
                    let akq = m[k * n + q];
                    m[k * n + p] = c * akp - s * akq;
                    m[k * n + q] = s * akp + c * akq;
                }
                for k in 0..n {
                    let apk = m[p * n + k];
                    let aqk = m[q * n + k];
                    m[p * n + k] = c * apk - s * aqk;
                    m[q * n + k] = s * apk + c * aqk;
                }
                for k in 0..n {
                    let vkp = v[k * n + p];
                    let vkq = v[k * n + q];
                    v[k * n + p] = c * vkp - s * vkq;
                    v[k * n + q] = s * vkp + c * vkq;
                }
            }
        }
    }
    let mut order: Vec<usize> = (0..n).collect();
    // A stable sort on a total order: ties keep their column order.
    order.sort_by(|&i, &j| m[i * n + i].total_cmp(&m[j * n + j]));
    let values = order.iter().map(|&i| m[i * n + i]).collect();
    let vectors = order
        .iter()
        .map(|&i| (0..n).map(|k| v[k * n + i]).collect())
        .collect();
    (values, vectors)
}

/// The lower-triangular `l` with `l lᵀ = a` (`n x n`, row-major), None when
/// `a` isn't positive definite.
pub fn cholesky(a: &[f64], n: usize) -> Option<Vec<f64>> {
    let mut l = vec![0.0; n * n];
    for i in 0..n {
        for j in 0..=i {
            let mut s = a[i * n + j];
            for k in 0..j {
                s -= l[i * n + k] * l[j * n + k];
            }
            if i == j {
                if !(s.is_finite() && s > 0.0) {
                    return None;
                }
                l[i * n + i] = s.sqrt();
            } else {
                l[i * n + j] = s / l[j * n + j];
            }
        }
    }
    Some(l)
}

/// `x` with `l lᵀ x = b`.
pub fn cholesky_solve(l: &[f64], n: usize, b: &[f64]) -> Vec<f64> {
    let mut y = vec![0.0; n];
    for i in 0..n {
        let mut s = b[i];
        for k in 0..i {
            s -= l[i * n + k] * y[k];
        }
        y[i] = s / l[i * n + i];
    }
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let mut s = y[i];
        for k in i + 1..n {
            s -= l[k * n + i] * x[k];
        }
        x[i] = s / l[i * n + i];
    }
    x
}

/// The diagonal of `a⁻¹` (`a` symmetric positive definite), None when it
/// isn't.
pub fn inverse_diagonal(a: &[f64], n: usize) -> Option<Vec<f64>> {
    let l = cholesky(a, n)?;
    Some(
        (0..n)
            .map(|i| {
                let mut e = vec![0.0; n];
                e[i] = 1.0;
                cholesky_solve(&l, n, &e)[i]
            })
            .collect(),
    )
}

/// The rotation `r` that makes `Σ m[i][j] r[i][j]` largest (the nearest
/// rotation to `m`; for `m = Σ wᵢ bᵢᵀ`, the turn that best takes each `bᵢ`
/// to its `wᵢ`). Horn's method: the top eigenvector of a 4x4 is the turn's
/// quaternion. Always a proper rotation, never a mirror.
pub fn nearest_rotation(m: &M3) -> M3 {
    let n = [
        [
            m[0][0] + m[1][1] + m[2][2],
            m[2][1] - m[1][2],
            m[0][2] - m[2][0],
            m[1][0] - m[0][1],
        ],
        [
            m[2][1] - m[1][2],
            m[0][0] - m[1][1] - m[2][2],
            m[0][1] + m[1][0],
            m[0][2] + m[2][0],
        ],
        [
            m[0][2] - m[2][0],
            m[0][1] + m[1][0],
            -m[0][0] + m[1][1] - m[2][2],
            m[1][2] + m[2][1],
        ],
        [
            m[1][0] - m[0][1],
            m[0][2] + m[2][0],
            m[1][2] + m[2][1],
            -m[0][0] - m[1][1] + m[2][2],
        ],
    ];
    let flat: Vec<f64> = n.iter().flatten().copied().collect();
    let (_, vectors) = eigen_symmetric(&flat, 4);
    let q = &vectors[3];
    let (w, x, y, z) = (q[0], q[1], q[2], q[3]);
    let l = (w * w + x * x + y * y + z * z).sqrt();
    if !(l.is_finite() && l > 0.0) {
        return IDENTITY;
    }
    let (w, x, y, z) = (w / l, x / l, y / l, z / l);
    [
        [
            w * w + x * x - y * y - z * z,
            2.0 * (x * y - w * z),
            2.0 * (x * z + w * y),
        ],
        [
            2.0 * (x * y + w * z),
            w * w - x * x + y * y - z * z,
            2.0 * (y * z - w * x),
        ],
        [
            2.0 * (x * z - w * y),
            2.0 * (y * z + w * x),
            w * w - x * x - y * y + z * z,
        ],
    ]
}

/// `m = k q` with `k` upper triangular (positive diagonal) and `q`'s rows
/// one long and square to each other (`q` is a rotation when `det m > 0`).
/// None when `m` is singular.
pub fn rq(m: &M3) -> Option<(M3, M3)> {
    let k22 = norm(m[2]);
    let q2 = unit(m[2])?;
    let k12 = dot(m[1], q2);
    let r1 = sub(m[1], scale(q2, k12));
    let k11 = norm(r1);
    let q1 = unit(r1)?;
    let k02 = dot(m[0], q2);
    let k01 = dot(m[0], q1);
    let r0 = sub(sub(m[0], scale(q2, k02)), scale(q1, k01));
    let k00 = norm(r0);
    let q0 = unit(r0)?;
    if k00 < 1e-12 * k22 || k11 < 1e-12 * k22 {
        return None;
    }
    Some(([[k00, k01, k02], [0.0, k11, k12], [0.0, 0.0, k22]], [q0, q1, q2]))
}

/// The spread of a set of points: their middle, and the eigenvalues
/// (smallest first) and axes of their covariance.
pub fn spread(points: &[V3]) -> (V3, [f64; 3], [V3; 3]) {
    let n = points.len().max(1) as f64;
    let mut c = [0.0; 3];
    for p in points {
        c = add(c, *p);
    }
    c = scale(c, 1.0 / n);
    let mut cov = [0.0; 9];
    for p in points {
        let d = sub(*p, c);
        for r in 0..3 {
            for k in 0..3 {
                cov[r * 3 + k] += d[r] * d[k] / n;
            }
        }
    }
    let (values, vectors) = eigen_symmetric(&cov, 3);
    let axis = |i: usize| [vectors[i][0], vectors[i][1], vectors[i][2]];
    (
        c,
        [values[0], values[1], values[2]],
        [axis(0), axis(1), axis(2)],
    )
}

/// The same for points in a plane: their middle and the two eigenvalues
/// (smallest first) of their covariance.
pub fn spread2(points: &[[f64; 2]]) -> ([f64; 2], [f64; 2]) {
    let n = points.len().max(1) as f64;
    let mut c = [0.0; 2];
    for p in points {
        c[0] += p[0] / n;
        c[1] += p[1] / n;
    }
    let mut cov = [0.0; 4];
    for p in points {
        let d = [p[0] - c[0], p[1] - c[1]];
        for r in 0..2 {
            for k in 0..2 {
                cov[r * 2 + k] += d[r] * d[k] / n;
            }
        }
    }
    let (values, _) = eigen_symmetric(&cov, 2);
    (c, [values[0], values[1]])
}
