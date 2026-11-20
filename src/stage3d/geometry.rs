//! Surfaces' geometry, pure: the meshes of flat, curved and faceted screens,
//! and the small matrix sums the 3D stage needs.
//!
//! A surface's own frame: X to the right, Y up, and +Z out of its front,
//! towards the audience (the side its picture is seen from). Its origin is
//! the middle of the screen. Matrices are column-major (`m[col][row]`), as in
//! `scene`.

// The Preview pane's 3D mode and the editing UI use these (integration).
#![allow(dead_code)]

use super::scene::{IDENTITY, Mesh};
use super::{Facet, Placement, SurfaceShape};

pub type Mat4 = [[f32; 4]; 4];

/// The most facets a surface is drawn with: a preset or a hand-edited file
/// asking for more is cut to this many.
pub const MAX_FACETS: usize = 4096;
/// A preset's columns, rows or sides are at most this many.
pub const MAX_PRESET_COUNT: u32 = 64;
/// The smallest size anything is given, in metres (a millimetre): a zero or
/// negative size in a file is drawn this small, never as nothing or inside out.
pub const MIN_SIZE: f32 = 0.001;
/// The largest, in metres.
pub const MAX_SIZE: f32 = 10_000.0;
/// A curved surface is cut into about one segment per this many degrees.
pub const DEGREES_PER_SEGMENT: f32 = 2.0;
pub const MAX_SEGMENTS: usize = 180;

/// One piece of a surface: a mesh in its own frame, where it is within the
/// surface, and the part of the surface's picture its 0..1 UVs cover
/// (`[x, y, w, h]`).
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub mesh: Mesh,
    pub transform: Mat4,
    pub region: [f32; 4],
}

/// A size from a file or a field: finite, at least `MIN_SIZE`, at most
/// `MAX_SIZE`. (Not `clamp`, which panics on NaN bounds; NaN here is
/// `MIN_SIZE`.)
pub fn size(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(MIN_SIZE, MAX_SIZE)
    } else if v == f32::INFINITY {
        MAX_SIZE
    } else {
        MIN_SIZE
    }
}

fn finite_or(v: f32, or: f32) -> f32 {
    if v.is_finite() { v } else { or }
}

/// A region kept within the picture, with some area.
pub fn region(r: [f32; 4]) -> [f32; 4] {
    let [x, y, w, h] = r.map(|v| finite_or(v, 0.0));
    let x = x.clamp(0.0, 1.0);
    let y = y.clamp(0.0, 1.0);
    let w = w.max(0.0).min(1.0 - x);
    let h = h.max(0.0).min(1.0 - y);
    if w <= 0.0 || h <= 0.0 {
        [0.0, 0.0, 1.0, 1.0]
    } else {
        [x, y, w, h]
    }
}

/// `inner` (a region of a piece) within `outer` (a region of the picture).
pub fn compose_region(outer: [f32; 4], inner: [f32; 4]) -> [f32; 4] {
    [
        outer[0] + inner[0] * outer[2],
        outer[1] + inner[1] * outer[3],
        inner[2] * outer[2],
        inner[3] * outer[3],
    ]
}

/// The surface's pieces: one for a flat or curved surface, one per facet
/// (at most `MAX_FACETS`) for a faceted one. Never panics, whatever the
/// numbers.
pub fn surface_mesh(shape: &SurfaceShape) -> Vec<Piece> {
    match shape {
        SurfaceShape::Flat { width, height } => vec![Piece {
            mesh: quad(size(*width), size(*height)),
            transform: IDENTITY,
            region: [0.0, 0.0, 1.0, 1.0],
        }],
        SurfaceShape::Curved {
            width,
            radius,
            height,
            convex,
        } => vec![Piece {
            mesh: arc(size(*width), size(*radius), size(*height), *convex),
            transform: IDENTITY,
            region: [0.0, 0.0, 1.0, 1.0],
        }],
        SurfaceShape::Faceted { facets } => facets
            .iter()
            .take(MAX_FACETS)
            .map(|f| Piece {
                mesh: quad(size(f.width), size(f.height)),
                transform: f.placement.matrix(),
                region: region(f.uv),
            })
            .collect(),
    }
}

/// A `width` x `height` rectangle centred on the origin, facing +Z, UVs
/// (0, 0) top-left.
pub fn quad(width: f32, height: f32) -> Mesh {
    let (x, y) = (width / 2.0, height / 2.0);
    Mesh {
        // Top-left, bottom-left, bottom-right, top-right.
        positions: vec![[-x, y, 0.0], [-x, -y, 0.0], [x, -y, 0.0], [x, y, 0.0]],
        normals: vec![[0.0, 0.0, 1.0]; 4],
        uvs: vec![[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]],
        indices: vec![1, 2, 3, 1, 3, 0],
    }
}

/// The angle (radians) a curved surface of this width (along the curve) and
/// radius spans, at most a whole turn.
pub fn arc_angle(width: f32, radius: f32) -> f32 {
    (size(width) / size(radius)).min(std::f32::consts::TAU)
}

/// How many segments a curve of this angle is cut into.
pub fn segments_for(angle: f32) -> usize {
    // (Less a little, so 60 degrees is 30 segments, not 31 by rounding.)
    let n = (angle.to_degrees() / DEGREES_PER_SEGMENT - 1e-3).ceil();
    if n.is_finite() {
        (n as usize).clamp(2, MAX_SEGMENTS)
    } else {
        2
    }
}

/// Part of a cylinder (axis vertical) `width` long around its curve. Its
/// middle is at the origin, facing +Z. Concave (`convex` false) curves its
/// sides towards the audience, as a wall wrapped around them; convex curves
/// them away, as a pillar seen from outside. u runs left to right along the
/// curve.
pub fn arc(width: f32, radius: f32, height: f32, convex: bool) -> Mesh {
    let angle = arc_angle(width, radius);
    let segments = segments_for(angle);
    let y = height / 2.0;
    let mut mesh = Mesh::default();
    for i in 0..=segments {
        let t = i as f32 / segments as f32;
        let phi = (t - 0.5) * angle;
        let (s, c) = phi.sin_cos();
        // Concave: the axis is in front of the screen, at +Z radius; convex:
        // behind it.
        let (x, z, normal) = if convex {
            (radius * s, radius * c - radius, [s, 0.0, c])
        } else {
            (radius * s, radius - radius * c, [-s, 0.0, c])
        };
        mesh.positions.push([x, y, z]);
        mesh.positions.push([x, -y, z]);
        mesh.normals.push(normal);
        mesh.normals.push(normal);
        mesh.uvs.push([t, 0.0]);
        mesh.uvs.push([t, 1.0]);
    }
    for i in 0..segments as u32 {
        let (tl, bl, tr, br) = (2 * i, 2 * i + 1, 2 * i + 2, 2 * i + 3);
        mesh.indices.extend_from_slice(&[bl, br, tr, bl, tr, tl]);
    }
    mesh
}

/// A box of screens hung over an arena: `sides` screens (3 to
/// `MAX_PRESET_COUNT`), each `width` x `height`, around a vertical axis,
/// facing outwards; side 0 faces +Z, the others go round to the right as seen
/// from above (anticlockwise). `bottom` adds a screen facing down, under a
/// four-sided box only (`width` square). With `share` each side shows its
/// share of the picture, left to right, else each shows the whole of it.
pub fn jumbotron(sides: u32, width: f32, height: f32, bottom: bool, share: bool) -> Vec<Facet> {
    let n = sides.clamp(3, MAX_PRESET_COUNT);
    let (w, h) = (size(width), size(height));
    // From the axis to the middle of a side.
    let apothem = w / (2.0 * (std::f32::consts::PI / n as f32).tan());
    let mut facets: Vec<Facet> = (0..n)
        .map(|i| {
            let yaw = 360.0 * i as f32 / n as f32;
            let (s, c) = yaw.to_radians().sin_cos();
            Facet {
                width: w,
                height: h,
                placement: Placement {
                    position: [apothem * s, 0.0, apothem * c],
                    yaw,
                    ..Placement::default()
                },
                uv: if share {
                    [i as f32 / n as f32, 0.0, 1.0 / n as f32, 1.0]
                } else {
                    [0.0, 0.0, 1.0, 1.0]
                },
            }
        })
        .collect();
    if bottom && n == 4 {
        facets.push(Facet {
            width: w,
            height: w,
            placement: Placement {
                position: [0.0, -h / 2.0, 0.0],
                // +Z turned to face down.
                pitch: 90.0,
                ..Placement::default()
            },
            uv: [0.0, 0.0, 1.0, 1.0],
        });
    }
    facets
}

/// A wall of `columns` x `rows` panels, each `panel_width` x `panel_height`,
/// joined edge to edge, each column turned `angle` degrees from the one
/// before so the wall bends: a positive angle brings the sides towards the
/// audience (concave), a negative one away. The middle of the wall is at the
/// origin facing +Z. Each panel shows its share of the picture.
pub fn angled_wall(
    columns: u32,
    rows: u32,
    panel_width: f32,
    panel_height: f32,
    angle: f32,
) -> Vec<Facet> {
    let cols = columns.clamp(1, MAX_PRESET_COUNT);
    let rows = rows.clamp(1, MAX_PRESET_COUNT);
    let (w, h) = (size(panel_width), size(panel_height));
    let angle = finite_or(angle, 0.0).clamp(-90.0, 90.0);
    let middle = (cols as f32 - 1.0) / 2.0;
    // Walk the columns left to right from the left edge, then move it all so
    // the middle of the wall is at the origin.
    let mut edge = [0.0f32, 0.0];
    let mut centres = Vec::with_capacity(cols as usize);
    for k in 0..cols {
        let yaw = -(k as f32 - middle) * angle;
        let (s, c) = yaw.to_radians().sin_cos();
        // The column's right direction, in the XZ plane: Ry(yaw) * +X.
        let right = [c, -s];
        centres.push((
            [edge[0] + right[0] * w / 2.0, edge[1] + right[1] * w / 2.0],
            yaw,
        ));
        edge = [edge[0] + right[0] * w, edge[1] + right[1] * w];
    }
    let mid = if cols % 2 == 1 {
        centres[(cols / 2) as usize].0
    } else {
        // The joint between the two middle columns: the right edge of the
        // left one.
        let (c, yaw) = centres[(cols / 2 - 1) as usize];
        let (s, co) = yaw.to_radians().sin_cos();
        [c[0] + co * w / 2.0, c[1] - s * w / 2.0]
    };
    let mut facets = Vec::with_capacity((cols * rows) as usize);
    for r in 0..rows {
        let y = ((rows as f32 - 1.0) / 2.0 - r as f32) * h;
        for (k, (c, yaw)) in centres.iter().enumerate() {
            facets.push(Facet {
                width: w,
                height: h,
                placement: Placement {
                    position: [c[0] - mid[0], y, c[1] - mid[1]],
                    yaw: *yaw,
                    ..Placement::default()
                },
                uv: [
                    k as f32 / cols as f32,
                    r as f32 / rows as f32,
                    1.0 / cols as f32,
                    1.0 / rows as f32,
                ],
            });
        }
    }
    facets
}

// ----- Matrices ---------------------------------------------------------------

pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut m = [[0.0; 4]; 4];
    for (c, col) in m.iter_mut().enumerate() {
        for (r, v) in col.iter_mut().enumerate() {
            *v = (0..4).map(|k| a[k][r] * b[c][k]).sum();
        }
    }
    m
}

pub fn translation(t: [f32; 3]) -> Mat4 {
    let mut m = IDENTITY;
    m[3] = [t[0], t[1], t[2], 1.0];
    m
}

pub fn scale(s: f32) -> Mat4 {
    let mut m = IDENTITY;
    for (i, col) in m.iter_mut().enumerate().take(3) {
        col[i] = s;
    }
    m
}

/// About +Y: positive turns +Z towards +X.
pub fn rotation_y(deg: f32) -> Mat4 {
    let (s, c) = deg.to_radians().sin_cos();
    [
        [c, 0.0, -s, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [s, 0.0, c, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// About +X: positive turns +Y towards +Z.
pub fn rotation_x(deg: f32) -> Mat4 {
    let (s, c) = deg.to_radians().sin_cos();
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, c, s, 0.0],
        [0.0, -s, c, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// About +Z: positive turns +X towards +Y.
pub fn rotation_z(deg: f32) -> Mat4 {
    let (s, c) = deg.to_radians().sin_cos();
    [
        [c, s, 0.0, 0.0],
        [-s, c, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

pub fn point(m: &Mat4, p: [f32; 3]) -> [f32; 3] {
    let v = |r: usize| m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r];
    [v(0), v(1), v(2)]
}

/// A direction (no translation).
pub fn direction(m: &Mat4, d: [f32; 3]) -> [f32; 3] {
    let v = |r: usize| m[0][r] * d[0] + m[1][r] * d[1] + m[2][r] * d[2];
    [v(0), v(1), v(2)]
}

/// The determinant of the 3x3 part: negative when the matrix mirrors.
pub fn det3(m: &Mat4) -> f32 {
    m[0][0] * (m[1][1] * m[2][2] - m[2][1] * m[1][2])
        - m[1][0] * (m[0][1] * m[2][2] - m[2][1] * m[0][2])
        + m[2][0] * (m[0][1] * m[1][2] - m[1][1] * m[0][2])
}

/// The matrix normals are carried by: the cofactor matrix of the 3x3 part,
/// which is its inverse transpose times its determinant. Normals are made unit
/// length after, but under a mirror (a negative determinant) they come out
/// inside out and must be turned round.
pub fn normal_matrix(m: &Mat4) -> [[f32; 3]; 3] {
    let a = |c: usize, r: usize| m[c][r];
    // Columns of the cofactor matrix: the cross products of the columns.
    let cross = |u: [f32; 3], v: [f32; 3]| {
        [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ]
    };
    let col = |c: usize| [a(c, 0), a(c, 1), a(c, 2)];
    [
        cross(col(1), col(2)),
        cross(col(2), col(0)),
        cross(col(0), col(1)),
    ]
}

pub fn apply3(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[1][0] * v[1] + m[2][0] * v[2],
        m[0][1] * v[0] + m[1][1] * v[1] + m[2][1] * v[2],
        m[0][2] * v[0] + m[1][2] * v[1] + m[2][2] * v[2],
    ]
}

pub fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 0.0 && l.is_finite() {
        [v[0] / l, v[1] / l, v[2] / l]
    } else {
        [0.0, 1.0, 0.0]
    }
}

pub fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn cross(u: [f32; 3], v: [f32; 3]) -> [f32; 3] {
    [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ]
}

pub fn dot(u: [f32; 3], v: [f32; 3]) -> f32 {
    u[0] * v[0] + u[1] * v[1] + u[2] * v[2]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    fn close3(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|i| close(a[i], b[i]))
    }

    /// Each triangle's corners go anticlockwise seen from where its normals
    /// point.
    fn faces_its_normals(m: &Mesh) {
        for t in m.indices.chunks(3) {
            let [a, b, c] = [0, 1, 2].map(|i| m.positions[t[i] as usize]);
            let face = cross(sub(b, a), sub(c, a));
            let n = m.normals[t[0] as usize];
            assert!(
                dot(face, n) > 0.0,
                "triangle {t:?} faces away from its normal"
            );
        }
    }

    fn extent(m: &Mesh) -> ([f32; 3], [f32; 3]) {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for p in &m.positions {
            for i in 0..3 {
                lo[i] = lo[i].min(p[i]);
                hi[i] = hi[i].max(p[i]);
            }
        }
        (lo, hi)
    }

    #[test]
    fn a_flat_surface_is_one_quad_of_its_size_facing_the_audience() {
        let pieces = surface_mesh(&SurfaceShape::Flat {
            width: 4.0,
            height: 2.25,
        });
        assert_eq!(pieces.len(), 1);
        let m = &pieces[0].mesh;
        assert_eq!((m.positions.len(), m.triangles()), (4, 2));
        assert_eq!(extent(m), ([-2.0, -1.125, 0.0], [2.0, 1.125, 0.0]));
        assert!(m.normals.iter().all(|n| *n == [0.0, 0.0, 1.0]));
        faces_its_normals(m);
        // The picture's top-left is at the top-left seen from the front.
        for (p, uv) in m.positions.iter().zip(&m.uvs) {
            assert_eq!(uv[0], if p[0] < 0.0 { 0.0 } else { 1.0 });
            assert_eq!(uv[1], if p[1] > 0.0 { 0.0 } else { 1.0 });
        }
        assert_eq!(pieces[0].region, [0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn a_curve_has_its_chord_sagitta_and_uvs_across_the_arc() {
        let (radius, angle_deg) = (10.0f32, 60.0f32);
        let width = radius * angle_deg.to_radians();
        for convex in [false, true] {
            let m = arc(width, radius, 3.0, convex);
            let segments = segments_for(angle_deg.to_radians());
            assert_eq!(segments, 30);
            assert_eq!(m.positions.len(), 2 * (segments + 1));
            assert_eq!(m.triangles(), 2 * segments);
            faces_its_normals(&m);
            let (lo, hi) = extent(&m);
            // The chord from end to end, 2 r sin(a / 2) = r here.
            assert!(close(hi[0] - lo[0], 10.0), "{lo:?} {hi:?}");
            assert!(close(hi[1] - lo[1], 3.0));
            // The sagitta, r (1 - cos(a / 2)): the ends towards the audience
            // when concave, away when convex; the middle at z = 0.
            let sagitta = radius * (1.0 - (angle_deg.to_radians() / 2.0).cos());
            let ends = m.positions[0][2];
            assert!(
                close(ends, if convex { -sagitta } else { sagitta }),
                "{ends}"
            );
            let middle = m.positions[segments][2]; // the top row's middle vertex
            assert!(close(middle, 0.0));
            assert!(close3(m.normals[segments], [0.0, 0.0, 1.0]));
            // u from 0 at the left end to 1 at the right; v 0 at the top.
            assert_eq!(m.uvs[0], [0.0, 0.0]);
            assert_eq!(m.uvs[1], [0.0, 1.0]);
            assert_eq!(m.uvs[2 * segments], [1.0, 0.0]);
            assert!(m.positions[0][0] < 0.0 && m.positions[2 * segments][0] > 0.0);
            // Every vertex is a radius from the axis.
            let axis_z = if convex { -radius } else { radius };
            for p in &m.positions {
                assert!(close(
                    (p[0].powi(2) + (p[2] - axis_z).powi(2)).sqrt(),
                    radius
                ));
            }
        }
    }

    #[test]
    fn a_curve_longer_than_its_circle_is_a_whole_cylinder() {
        let m = arc(1000.0, 1.0, 1.0, true);
        assert!(close(arc_angle(1000.0, 1.0), std::f32::consts::TAU));
        assert_eq!(m.positions.len(), 2 * (MAX_SEGMENTS + 1));
        // The seam closes.
        assert!(close3(m.positions[0], m.positions[2 * MAX_SEGMENTS]));
    }

    #[test]
    fn a_jumbotron_faces_outwards_all_round() {
        for (sides, bottom) in [(4, true), (6, false), (3, false)] {
            let facets = jumbotron(sides, 6.0, 3.5, bottom, false);
            assert_eq!(facets.len(), sides as usize + usize::from(bottom));
            let apothem = 3.0 / (std::f32::consts::PI / sides as f32).tan();
            for f in facets.iter().take(sides as usize) {
                let m = f.placement.matrix();
                let centre = point(&m, [0.0, 0.0, 0.0]);
                let normal = direction(&m, [0.0, 0.0, 1.0]);
                // Outwards: the normal points the way the centre lies from
                // the axis.
                let out = normalize([centre[0], 0.0, centre[2]]);
                assert!(close3(normal, out), "{normal:?} {out:?}");
                assert!(close(
                    (centre[0].powi(2) + centre[2].powi(2)).sqrt(),
                    apothem
                ));
                assert_eq!(f.uv, [0.0, 0.0, 1.0, 1.0]);
                // Neighbouring sides meet: the right edge of each is the left
                // edge of the next.
            }
            for pair in facets[..sides as usize].windows(2) {
                let right = point(&pair[0].placement.matrix(), [3.0, 0.0, 0.0]);
                let left = point(&pair[1].placement.matrix(), [-3.0, 0.0, 0.0]);
                assert!(close3(right, left), "{right:?} {left:?}");
            }
            if bottom {
                let b = facets.last().unwrap();
                let n = direction(&b.placement.matrix(), [0.0, 0.0, 1.0]);
                assert!(close3(n, [0.0, -1.0, 0.0]), "{n:?}");
                assert!(close(point(&b.placement.matrix(), [0.0; 3])[1], -1.75));
            }
        }
        // Shared, each side has its share, left to right.
        let shared = jumbotron(4, 6.0, 3.5, false, true);
        assert_eq!(shared[1].uv, [0.25, 0.0, 0.25, 1.0]);
        // A bottom only under four sides.
        assert_eq!(jumbotron(6, 1.0, 1.0, true, false).len(), 6);
    }

    #[test]
    fn an_angled_walls_columns_turn_by_its_angle_and_join() {
        let facets = angled_wall(5, 2, 0.5, 0.5, 10.0);
        assert_eq!(facets.len(), 10);
        let normals: Vec<[f32; 3]> = facets[..5]
            .iter()
            .map(|f| direction(&f.placement.matrix(), [0.0, 0.0, 1.0]))
            .collect();
        for pair in normals.windows(2) {
            let between = dot(pair[0], pair[1]).min(1.0).acos().to_degrees();
            assert!(close(between, 10.0), "{between}");
        }
        // Concave: the left column looks right (towards the middle).
        assert!(normals[0][0] > 0.0 && normals[4][0] < 0.0);
        // The middle column is at the origin, facing the audience.
        let mid = facets[2].placement.matrix();
        assert!(close3(point(&mid, [0.0; 3]), [0.0, 0.25, 0.0]));
        assert!(close3(normals[2], [0.0, 0.0, 1.0]));
        // Neighbours join edge to edge; the sides come forward.
        for pair in facets[..5].windows(2) {
            let right = point(&pair[0].placement.matrix(), [0.25, 0.0, 0.0]);
            let left = point(&pair[1].placement.matrix(), [-0.25, 0.0, 0.0]);
            assert!(close3(right, left));
        }
        assert!(point(&facets[0].placement.matrix(), [0.0; 3])[2] > 0.0);
        // Rows stacked top first; each panel its share of the picture.
        assert!(close(facets[5].placement.position[1], -0.25));
        assert_eq!(facets[7].uv, [0.4, 0.5, 0.2, 0.5]);
        // An even count is centred on the middle joint.
        let even = angled_wall(4, 1, 1.0, 1.0, 20.0);
        let joint = point(&even[1].placement.matrix(), [0.5, 0.0, 0.0]);
        assert!(close3(joint, [0.0; 3]), "{joint:?}");
        // Flat (no angle) is a plain wall.
        let flat = angled_wall(3, 1, 1.0, 1.0, 0.0);
        assert!(close3(flat[0].placement.position, [-1.0, 0.0, 0.0]));
    }

    #[test]
    fn faceted_pieces_carry_their_placement_and_region() {
        let facets = angled_wall(2, 1, 1.0, 1.0, 0.0);
        let pieces = surface_mesh(&SurfaceShape::Faceted {
            facets: facets.clone(),
        });
        assert_eq!(pieces.len(), 2);
        assert_eq!(pieces[1].region, [0.5, 0.0, 0.5, 1.0]);
        assert_eq!(pieces[1].transform, facets[1].placement.matrix());
        assert_eq!(pieces[1].mesh, quad(1.0, 1.0));
    }

    #[test]
    fn numbers_from_a_file_never_panic_or_explode() {
        let nan = f32::NAN;
        let inf = f32::INFINITY;
        for (w, h) in [(0.0, 0.0), (-3.0, 2.0), (nan, nan), (inf, -inf)] {
            let p = surface_mesh(&SurfaceShape::Flat {
                width: w,
                height: h,
            });
            assert!(p[0].mesh.positions.iter().flatten().all(|v| v.is_finite()));
        }
        for (w, r) in [
            (0.0, 0.0),
            (-1.0, -1.0),
            (nan, 1.0),
            (1.0, nan),
            (inf, inf),
            (1e9, 1e-9),
        ] {
            let p = surface_mesh(&SurfaceShape::Curved {
                width: w,
                radius: r,
                height: 1.0,
                convex: false,
            });
            let m = &p[0].mesh;
            assert!(m.positions.len() <= 2 * (MAX_SEGMENTS + 1));
            assert!(
                m.positions.iter().flatten().all(|v| v.is_finite()),
                "{w} {r}"
            );
        }
        assert_eq!(jumbotron(0, 1.0, 1.0, false, false).len(), 3);
        assert_eq!(
            jumbotron(u32::MAX, nan, nan, true, true).len(),
            MAX_PRESET_COUNT as usize
        );
        let huge = angled_wall(u32::MAX, u32::MAX, -1.0, nan, nan);
        assert_eq!(huge.len(), (MAX_PRESET_COUNT * MAX_PRESET_COUNT) as usize);
        assert!(
            huge.iter()
                .all(|f| f.placement.position.iter().all(|v| v.is_finite()))
        );
        assert_eq!(angled_wall(0, 0, 1.0, 1.0, 0.0).len(), 1);
        // A hand-edited file with too many facets is cut, its regions kept in
        // the picture.
        let mut facets = vec![
            Facet {
                width: 1.0,
                height: 1.0,
                placement: Placement::default(),
                uv: [nan, -1.0, 5.0, 0.0],
            };
            MAX_FACETS + 10
        ];
        facets[0].uv = [0.5, 0.5, 2.0, 2.0];
        let p = surface_mesh(&SurfaceShape::Faceted { facets });
        assert_eq!(p.len(), MAX_FACETS);
        assert_eq!(p[0].region, [0.5, 0.5, 0.5, 0.5]);
        assert_eq!(p[1].region, [0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn rotations_go_the_way_they_say_and_compose_yaw_pitch_roll() {
        assert!(close3(
            direction(&rotation_y(90.0), [0.0, 0.0, 1.0]),
            [1.0, 0.0, 0.0]
        ));
        assert!(close3(
            direction(&rotation_x(90.0), [0.0, 1.0, 0.0]),
            [0.0, 0.0, 1.0]
        ));
        assert!(close3(
            direction(&rotation_z(90.0), [1.0, 0.0, 0.0]),
            [0.0, 1.0, 0.0]
        ));
        let p = Placement {
            position: [1.0, 2.0, 3.0],
            yaw: 90.0,
            pitch: 90.0,
            roll: 0.0,
        };
        // Pitched up first (its front to the sky... +Y to +Z), then turned:
        // its top now points along +X.
        let m = p.matrix();
        assert!(close3(direction(&m, [0.0, 1.0, 0.0]), [1.0, 0.0, 0.0]));
        assert!(close3(point(&m, [0.0; 3]), [1.0, 2.0, 3.0]));
        assert!(close(det3(&m), 1.0));
        let n = normal_matrix(&mul(&scale(2.0), &rotation_y(30.0)));
        assert!(close3(
            normalize(apply3(&n, [0.0, 0.0, 1.0])),
            direction(&rotation_y(30.0), [0.0, 0.0, 1.0])
        ));
    }
}
