//! The few vector and matrix sums the renderer needs (pure). Matrices are
//! column-major `[[f32; 4]; 4]`: `m[column][row]`, as WGSL's `mat4x4` reads them.

pub type Vec3 = [f32; 3];
pub type Mat4 = [[f32; 4]; 4];

pub const IDENTITY: Mat4 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

pub fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn scale(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

pub fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn length(a: Vec3) -> f32 {
    dot(a, a).sqrt()
}

/// `a` made one long, or None when it has no direction (or isn't a number).
pub fn normalize(a: Vec3) -> Option<Vec3> {
    let l = length(a);
    (l.is_finite() && l > 1e-12).then(|| scale(a, 1.0 / l))
}

pub fn finite(a: Vec3) -> bool {
    a.iter().all(|v| v.is_finite())
}

/// `a * b`: `b` is applied first.
pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut m = [[0.0; 4]; 4];
    for (c, col) in m.iter_mut().enumerate() {
        for (r, v) in col.iter_mut().enumerate() {
            *v = (0..4).map(|k| a[k][r] * b[c][k]).sum();
        }
    }
    m
}

/// `m * (p, 1)`, divided by w.
pub fn transform_point(m: &Mat4, p: Vec3) -> Vec3 {
    let v = transform4(m, [p[0], p[1], p[2], 1.0]);
    if v[3] != 0.0 && v[3] != 1.0 {
        [v[0] / v[3], v[1] / v[3], v[2] / v[3]]
    } else {
        [v[0], v[1], v[2]]
    }
}

pub fn transform4(m: &Mat4, v: [f32; 4]) -> [f32; 4] {
    let mut o = [0.0; 4];
    for (r, out) in o.iter_mut().enumerate() {
        *out = (0..4).map(|c| m[c][r] * v[c]).sum();
    }
    o
}

/// The matrix normals are carried by (the cofactors of the upper 3×3: the
/// inverse transpose times the determinant; the shader makes them one long
/// again, and turns them towards the eye, so the scale and sign don't matter).
pub fn normal_matrix(m: &Mat4) -> [Vec3; 3] {
    let c = |i: usize| [m[i][0], m[i][1], m[i][2]];
    let (a, b, d) = (c(0), c(1), c(2));
    // Columns of the cofactor matrix: cross products of the other two columns.
    [cross(b, d), cross(d, a), cross(a, b)]
}

/// A right-handed view matrix: the eye at `eye` looking at `target`.
/// None when the two are the same place or `up` is along the view.
pub fn look_at(eye: Vec3, target: Vec3, up: Vec3) -> Option<Mat4> {
    let f = normalize(sub(target, eye))?;
    let s = normalize(cross(f, up))?;
    let u = cross(s, f);
    Some([
        [s[0], u[0], -f[0], 0.0],
        [s[1], u[1], -f[1], 0.0],
        [s[2], u[2], -f[2], 0.0],
        [-dot(s, eye), -dot(u, eye), dot(f, eye), 1.0],
    ])
}

/// A perspective projection with reversed depth: `near` goes to depth 1 and
/// `far` to 0 (wgpu's 0..1 range; with a Depth32Float buffer this keeps a
/// venue's far walls as sharp as the near ones). Clear depth to 0 and pass
/// fragments whose depth is greater.
pub fn perspective_reversed(fov_y: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let f = 1.0 / (fov_y * 0.5).tan();
    let mut m = [[0.0; 4]; 4];
    m[0][0] = f / aspect;
    m[1][1] = f;
    m[2][2] = near / (far - near);
    m[2][3] = -1.0;
    m[3][2] = near * far / (far - near);
    m
}

/// `perspective_reversed` for a frustum that needn't be even about the axis
/// (a shifted lens): its edges one metre ahead, reversed depth as there.
pub fn frustum_reversed(f: &super::camera::Frustum, near: f32, far: f32) -> Mat4 {
    let w = f.right - f.left;
    let h = f.top - f.bottom;
    let (w, h) = (
        if w.abs() > 1e-9 { w } else { 1e-9 },
        if h.abs() > 1e-9 { h } else { 1e-9 },
    );
    let mut m = [[0.0; 4]; 4];
    m[0][0] = 2.0 / w;
    m[1][1] = 2.0 / h;
    // Where the axis is in the picture: x and y are moved in proportion to depth.
    m[2][0] = (f.right + f.left) / w;
    m[2][1] = (f.top + f.bottom) / h;
    m[2][2] = near / (far - near);
    m[2][3] = -1.0;
    m[3][2] = near * far / (far - near);
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-4)
    }

    #[test]
    fn multiplying_applies_the_right_hand_matrix_first() {
        let mut t = IDENTITY;
        t[3] = [1.0, 2.0, 3.0, 1.0];
        let mut s = IDENTITY;
        s[0][0] = 2.0;
        // Scale then move.
        assert!(close(
            transform_point(&mul(&t, &s), [1.0, 1.0, 1.0]),
            [3.0, 3.0, 4.0]
        ));
        assert_eq!(mul(&IDENTITY, &t), t);
    }

    #[test]
    fn the_view_puts_the_target_straight_ahead_on_minus_z() {
        let v = look_at([0.0, 2.0, 5.0], [0.0, 2.0, 0.0], [0.0, 1.0, 0.0]).unwrap();
        assert!(close(
            transform_point(&v, [0.0, 2.0, 0.0]),
            [0.0, 0.0, -5.0]
        ));
        // Right in the world is right in view (+x), up is up.
        assert!(close(
            transform_point(&v, [1.0, 3.0, 0.0]),
            [1.0, 1.0, -5.0]
        ));
        assert!(look_at([0.0; 3], [0.0; 3], [0.0, 1.0, 0.0]).is_none());
        assert!(look_at([0.0, 5.0, 0.0], [0.0; 3], [0.0, 1.0, 0.0]).is_none());
    }

    #[test]
    fn reversed_depth_puts_near_at_one_and_far_at_zero() {
        let p = perspective_reversed(60f32.to_radians(), 1.5, 0.1, 100.0);
        let depth = |z: f32| {
            let v = transform4(&p, [0.0, 0.0, -z, 1.0]);
            v[2] / v[3]
        };
        assert!((depth(0.1) - 1.0).abs() < 1e-5);
        assert!(depth(100.0).abs() < 1e-5);
        assert!(depth(1.0) > depth(10.0));
        // The edge of the field of view is the edge of the picture.
        let h = (30f32).to_radians().tan() * 10.0;
        let v = transform4(&p, [h * 1.5, h, -10.0, 1.0]);
        assert!((v[0] / v[3] - 1.0).abs() < 1e-4 && (v[1] / v[3] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn normals_follow_a_stretch_as_the_inverse_transpose() {
        // Stretch x by 2: a surface sloping at 45° in x/y gets flatter, so its
        // normal leans further towards y.
        let mut m = IDENTITY;
        m[0][0] = 2.0;
        let n = normal_matrix(&m);
        let v = [1.0, 1.0, 0.0];
        let out = [
            n[0][0] * v[0] + n[1][0] * v[1] + n[2][0] * v[2],
            n[0][1] * v[0] + n[1][1] * v[1] + n[2][1] * v[2],
            n[0][2] * v[0] + n[1][2] * v[1] + n[2][2] * v[2],
        ];
        let out = normalize(out).unwrap();
        // The surface's direction (1,-1,0) stretched is (2,-1,0); the normal is
        // at right angles to it.
        assert!(dot(out, [2.0, -1.0, 0.0]).abs() < 1e-5);
    }
}
