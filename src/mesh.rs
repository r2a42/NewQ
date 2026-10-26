//! A mesh warp: a grid of control points that shapes the picture of a display
//! more finely than four corners can (a curved screen, a cylinder, a wall with
//! a corner in it).
//!
//! Each control point sits at a place in the picture (`xs[i]`, `ys[j]`, as
//! fractions of the canvas) and is moved to a place on the display (the point
//! itself, as fractions of the display). Between points the shape is a smooth
//! cubic Hermite spline, which passes through every point and leaves a mesh
//! that hasn't been moved exactly as it was. The shader in `gpu.rs` does the
//! same sums per vertex; a test renders it and compares.

use serde::{Deserialize, Serialize};

/// The most lines either way, which is what the shader has room for.
pub const MAX_LINES: usize = 9;
/// The fewest: the edges.
pub const MIN_LINES: usize = 2;
/// How far past the edge of a cell's neighbour a lookup is extended, as a
/// step for working out the slope there.
const EDGE_STEP: f32 = 0.01;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Mesh {
    /// Where the columns lie across the picture: increasing, from 0 to 1.
    pub xs: Vec<f32>,
    /// Where the rows lie down the picture.
    pub ys: Vec<f32>,
    /// Where each point is on the display, row by row (`cols × rows` of them).
    pub points: Vec<[f32; 2]>,
}

/// One spline segment through `b` and `c` at knots `kb` and `kc`, evaluated at
/// `u`. `a` and `d` (at `ka` and `kd`) are the neighbours that set the slopes;
/// at an edge they repeat `b` or `c`, which makes the slope one-sided.
#[allow(clippy::too_many_arguments)]
fn hermite(
    ka: f32,
    kb: f32,
    kc: f32,
    kd: f32,
    a: [f32; 2],
    b: [f32; 2],
    c: [f32; 2],
    d: [f32; 2],
    u: f32,
) -> [f32; 2] {
    let h = kc - kb;
    let t = (u - kb) / h;
    let (t2, t3) = (t * t, t * t * t);
    let (h00, h10, h01, h11) = (
        2.0 * t3 - 3.0 * t2 + 1.0,
        t3 - 2.0 * t2 + t,
        -2.0 * t3 + 3.0 * t2,
        t3 - t2,
    );
    let mut out = [0.0; 2];
    for k in 0..2 {
        let mb = (c[k] - a[k]) / (kc - ka);
        let mc = (d[k] - b[k]) / (kd - kb);
        out[k] = h00 * b[k] + h10 * h * mb + h01 * c[k] + h11 * h * mc;
    }
    out
}

impl Mesh {
    /// An even grid that changes nothing.
    pub fn grid(cols: usize, rows: usize) -> Mesh {
        let cols = cols.clamp(MIN_LINES, MAX_LINES);
        let rows = rows.clamp(MIN_LINES, MAX_LINES);
        let line = |n: usize| -> Vec<f32> { (0..n).map(|i| i as f32 / (n - 1) as f32).collect() };
        let (xs, ys) = (line(cols), line(rows));
        let points = ys
            .iter()
            .flat_map(|&y| xs.iter().map(move |&x| [x, y]))
            .collect();
        Mesh { xs, ys, points }
    }

    pub fn cols(&self) -> usize {
        self.xs.len()
    }

    pub fn rows(&self) -> usize {
        self.ys.len()
    }

    pub fn point(&self, col: usize, row: usize) -> [f32; 2] {
        self.points[row * self.cols() + col]
    }

    pub fn point_mut(&mut self, col: usize, row: usize) -> &mut [f32; 2] {
        let cols = self.cols();
        &mut self.points[row * cols + col]
    }

    /// Whether the numbers hang together: sizes in range, lines increasing from
    /// 0 to 1, as many points as the grid has, all finite. A show file that
    /// says otherwise is not trusted.
    pub fn is_valid(&self) -> bool {
        let line_ok = |l: &[f32]| {
            (MIN_LINES..=MAX_LINES).contains(&l.len())
                && l[0] == 0.0
                && l[l.len() - 1] == 1.0
                && l.windows(2).all(|w| w[1] > w[0])
        };
        line_ok(&self.xs)
            && line_ok(&self.ys)
            && self.points.len() == self.xs.len() * self.ys.len()
            && self.points.iter().flatten().all(|v| v.is_finite())
    }

    /// Where picture position (`u`, `v`) in 0..1 lands on the display.
    fn eval_inside(&self, u: f32, v: f32) -> [f32; 2] {
        let (cols, rows) = (self.cols() as i32, self.rows() as i32);
        let find = |knots: &[f32], at: f32| {
            (1..knots.len() as i32 - 1)
                .rfind(|&k| at >= knots[k as usize])
                .unwrap_or(0)
        };
        let (i, j) = (find(&self.xs, u), find(&self.ys, v));
        let kx = |k: i32| self.xs[k.clamp(0, cols - 1) as usize];
        let ky = |k: i32| self.ys[k.clamp(0, rows - 1) as usize];
        let node = |c: i32, r: i32| {
            self.point(c.clamp(0, cols - 1) as usize, r.clamp(0, rows - 1) as usize)
        };
        let mut q = [[0.0; 2]; 4];
        for (m, slot) in q.iter_mut().enumerate() {
            let r = j - 1 + m as i32;
            *slot = hermite(
                kx(i - 1),
                kx(i),
                kx(i + 1),
                kx(i + 2),
                node(i - 1, r),
                node(i, r),
                node(i + 1, r),
                node(i + 2, r),
                u,
            );
        }
        hermite(
            ky(j - 1),
            ky(j),
            ky(j + 1),
            ky(j + 2),
            q[0],
            q[1],
            q[2],
            q[3],
            v,
        )
    }

    /// Where picture position (`u`, `v`) lands on the display. Past the edge
    /// of the picture the shape carries on in a straight line from the edge.
    pub fn eval(&self, u: f32, v: f32) -> [f32; 2] {
        let (uc, vc) = (u.clamp(0.0, 1.0), v.clamp(0.0, 1.0));
        let mut p = self.eval_inside(uc, vc);
        // How fast the display position changes per unit of picture position
        // at an edge, from two points a step apart.
        let slope =
            |lo: [f32; 2], hi: [f32; 2]| [(hi[0] - lo[0]) / EDGE_STEP, (hi[1] - lo[1]) / EDGE_STEP];
        if u != uc {
            let s = if u > 1.0 {
                slope(
                    self.eval_inside(1.0 - EDGE_STEP, vc),
                    self.eval_inside(1.0, vc),
                )
            } else {
                slope(self.eval_inside(0.0, vc), self.eval_inside(EDGE_STEP, vc))
            };
            p = [p[0] + (u - uc) * s[0], p[1] + (u - uc) * s[1]];
        }
        if v != vc {
            let s = if v > 1.0 {
                slope(
                    self.eval_inside(uc, 1.0 - EDGE_STEP),
                    self.eval_inside(uc, 1.0),
                )
            } else {
                slope(self.eval_inside(uc, 0.0), self.eval_inside(uc, EDGE_STEP))
            };
            p = [p[0] + (v - vc) * s[0], p[1] + (v - vc) * s[1]];
        }
        p
    }

    /// Put a new column where the picture is `at` of the way across (0..1,
    /// not on an existing line), on the shape as it is now, so nothing moves.
    /// Returns the new column's index, or None if there is no room or it is
    /// too close to a line that is there.
    pub fn insert_column(&mut self, at: f32) -> Option<usize> {
        let k = Self::slot(&self.xs, at)?;
        let new_points: Vec<[f32; 2]> = self.ys.iter().map(|&y| self.eval(at, y)).collect();
        let cols = self.cols();
        let mut points = Vec::with_capacity(self.points.len() + self.rows());
        for (j, row) in new_points.iter().enumerate() {
            points.extend_from_slice(&self.points[j * cols..j * cols + k]);
            points.push(*row);
            points.extend_from_slice(&self.points[j * cols + k..(j + 1) * cols]);
        }
        self.xs.insert(k, at);
        self.points = points;
        Some(k)
    }

    /// Like `insert_column`, for a row `at` of the way down.
    pub fn insert_row(&mut self, at: f32) -> Option<usize> {
        let k = Self::slot(&self.ys, at)?;
        let row: Vec<[f32; 2]> = self.xs.iter().map(|&x| self.eval(x, at)).collect();
        let cols = self.cols();
        let tail = self.points.split_off(k * cols);
        self.points.extend(row);
        self.points.extend(tail);
        self.ys.insert(k, at);
        Some(k)
    }

    /// Where `at` would go among `lines`, if there is room and it isn't on
    /// top of one.
    fn slot(lines: &[f32], at: f32) -> Option<usize> {
        const MIN_GAP: f32 = 0.02;
        if lines.len() >= MAX_LINES || at <= 0.0 || at >= 1.0 {
            return None;
        }
        if lines.iter().any(|l| (l - at).abs() < MIN_GAP) {
            return None;
        }
        Some(lines.iter().position(|&l| l > at).unwrap_or(lines.len()))
    }

    /// Whether a column could be put `at` of the way across.
    pub fn can_insert_column(&self, at: f32) -> bool {
        Self::slot(&self.xs, at).is_some()
    }

    /// Whether a row could be put `at` of the way down.
    pub fn can_insert_row(&self, at: f32) -> bool {
        Self::slot(&self.ys, at).is_some()
    }

    /// Take out column `col`, if it isn't an edge and the rest still make a grid.
    pub fn remove_column(&mut self, col: usize) -> bool {
        let cols = self.cols();
        if cols <= MIN_LINES || col == 0 || col >= cols - 1 {
            return false;
        }
        let rows = self.rows();
        self.points = (0..rows)
            .flat_map(|j| (0..cols).filter(move |&i| i != col).map(move |i| (i, j)))
            .map(|(i, j)| self.points[j * cols + i])
            .collect();
        self.xs.remove(col);
        true
    }

    pub fn remove_row(&mut self, row: usize) -> bool {
        let (cols, rows) = (self.cols(), self.rows());
        if rows <= MIN_LINES || row == 0 || row >= rows - 1 {
            return false;
        }
        self.points.drain(row * cols..(row + 1) * cols);
        self.ys.remove(row);
        true
    }

    /// The same shape on an even grid of `cols` by `rows`.
    pub fn resampled(&self, cols: usize, rows: usize) -> Mesh {
        let mut out = Mesh::grid(cols, rows);
        let (c, r) = (out.cols(), out.rows());
        for j in 0..r {
            for i in 0..c {
                *out.point_mut(i, j) = self.eval(out.xs[i], out.ys[j]);
            }
        }
        out
    }

    /// A mesh of `cols` by `rows` that makes the same shape as four corners
    /// pinned by `pin`, a function from picture to display position.
    pub fn from_pin(cols: usize, rows: usize, pin: impl Fn(f32, f32) -> [f32; 2]) -> Mesh {
        let mut out = Mesh::grid(cols, rows);
        for j in 0..out.rows() {
            for i in 0..out.cols() {
                *out.point_mut(i, j) = pin(out.xs[i], out.ys[j]);
            }
        }
        out
    }

    /// The four corners: top left, top right, bottom right, bottom left.
    pub fn corners(&self) -> [[f32; 2]; 4] {
        let (c, r) = (self.cols() - 1, self.rows() - 1);
        [
            self.point(0, 0),
            self.point(c, 0),
            self.point(c, r),
            self.point(0, r),
        ]
    }

    /// Put every point back where its lines cross, which changes nothing.
    pub fn flatten(&mut self) {
        let cols = self.cols();
        for (k, p) in self.points.iter_mut().enumerate() {
            *p = [self.xs[k % cols], self.ys[k / cols]];
        }
    }

    /// Whether every point is where its lines cross.
    pub fn is_flat(&self) -> bool {
        let mut flat = self.clone();
        flat.flatten();
        flat == *self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: [f32; 2], b: [f32; 2]) {
        assert!(
            (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4,
            "{a:?} != {b:?}"
        );
    }

    #[test]
    fn an_untouched_mesh_changes_nothing_however_it_is_laid_out() {
        let mut m = Mesh::grid(4, 3);
        for (u, v) in [
            (0.0, 0.0),
            (0.3, 0.8),
            (1.0, 1.0),
            (0.5, 0.25),
            (0.999, 0.01),
        ] {
            near(m.eval(u, v), [u, v]);
        }
        // Lines at odd places don't bend a straight picture either.
        m.insert_column(0.1).unwrap();
        m.insert_row(0.9).unwrap();
        for (u, v) in [(0.05, 0.5), (0.2, 0.95), (0.77, 0.33)] {
            near(m.eval(u, v), [u, v]);
        }
        assert!(m.is_valid());
    }

    #[test]
    fn it_passes_through_every_point() {
        let mut m = Mesh::grid(4, 4);
        *m.point_mut(1, 1) = [0.2, 0.4];
        *m.point_mut(2, 1) = [0.7, 0.2];
        *m.point_mut(3, 3) = [0.9, 1.1];
        for j in 0..4 {
            for i in 0..4 {
                near(m.eval(m.xs[i], m.ys[j]), m.point(i, j));
            }
        }
    }

    #[test]
    fn four_corners_match_the_corner_pin() {
        // A mesh of just the corners is a bilinear patch: the middle of the
        // picture is the average of the corners, on a straight edge too.
        let mut m = Mesh::grid(2, 2);
        *m.point_mut(0, 0) = [0.1, 0.0];
        *m.point_mut(1, 0) = [0.9, 0.0];
        near(m.eval(0.5, 0.0), [0.5, 0.0]);
        near(m.eval(0.5, 1.0), [0.5, 1.0]);
        near(m.eval(0.0, 0.5), [0.05, 0.5]);
    }

    #[test]
    fn a_moved_point_pulls_its_neighbourhood_smoothly() {
        let mut m = Mesh::grid(3, 3);
        *m.point_mut(1, 1) = [0.6, 0.5];
        near(m.eval(0.5, 0.5), [0.6, 0.5]);
        // Between the pulled point and a fixed one, it is in between.
        let x = m.eval(0.25, 0.5)[0];
        assert!(x > 0.25 && x < 0.6, "{x}");
        // The edges, which didn't move, stay put.
        near(m.eval(0.0, 0.5), [0.0, 0.5]);
        near(m.eval(1.0, 0.5), [1.0, 0.5]);
    }

    #[test]
    fn past_the_edge_it_carries_on_straight() {
        let m = Mesh::grid(3, 3);
        near(m.eval(-0.25, 0.5), [-0.25, 0.5]);
        near(m.eval(1.5, 1.2), [1.5, 1.2]);
        // On a stretched mesh the slope at the edge continues.
        let s = Mesh::from_pin(3, 3, |u, v| [u * 2.0, v]);
        near(s.eval(1.5, 0.5), [3.0, 0.5]);
        near(s.eval(-0.5, 0.5), [-1.0, 0.5]);
    }

    #[test]
    fn inserting_a_line_keeps_the_shape() {
        let mut m = Mesh::grid(3, 3);
        *m.point_mut(1, 1) = [0.6, 0.45];
        *m.point_mut(2, 0) = [1.0, 0.1];
        let before: Vec<[f32; 2]> = [(0.1, 0.1), (0.4, 0.7), (0.8, 0.3), (0.55, 0.5)]
            .iter()
            .map(|&(u, v)| m.eval(u, v))
            .collect();
        let c = m.insert_column(0.3).unwrap();
        let r = m.insert_row(0.7).unwrap();
        assert_eq!((m.cols(), m.rows(), c, r), (4, 4, 1, 2));
        assert!(m.is_valid());
        for (k, (u, v)) in [(0.1, 0.1), (0.4, 0.7), (0.8, 0.3), (0.55, 0.5)]
            .iter()
            .enumerate()
        {
            // A new knot changes the slopes at its neighbours, so a strongly
            // bent mesh drifts a little (here under 1% of the screen); a
            // gentle one hardly at all.
            let now = m.eval(*u, *v);
            assert!(
                (now[0] - before[k][0]).abs() < 0.01 && (now[1] - before[k][1]).abs() < 0.01,
                "{now:?} {:?}",
                before[k]
            );
        }
        // The new line is at the place asked for.
        assert_eq!((m.xs[1], m.ys[2]), (0.3, 0.7));
        // Too close to a line, or on the edge, or no room: refused.
        assert_eq!(m.insert_column(0.31), None);
        assert_eq!(m.insert_column(0.0), None);
        assert_eq!(m.insert_row(1.0), None);
        let mut full = Mesh::grid(MAX_LINES, 2);
        assert_eq!(full.insert_column(0.01), None);
    }

    #[test]
    fn lines_come_out_and_edges_stay() {
        let mut m = Mesh::grid(4, 4);
        *m.point_mut(2, 2) = [0.7, 0.7];
        assert!(m.remove_column(1) && m.remove_row(1));
        assert_eq!((m.cols(), m.rows()), (3, 3));
        assert!(m.is_valid());
        assert_eq!(m.point(1, 1), [0.7, 0.7]);
        assert!(!m.remove_column(0) && !m.remove_column(2) && !m.remove_row(0));
        let mut small = Mesh::grid(2, 2);
        assert!(!small.remove_column(1));
    }

    #[test]
    fn resampling_keeps_the_look() {
        let mut m = Mesh::grid(3, 3);
        *m.point_mut(1, 1) = [0.55, 0.45];
        let r = m.resampled(5, 4);
        assert_eq!((r.cols(), r.rows()), (5, 4));
        for (u, v) in [(0.2, 0.3), (0.5, 0.5), (0.9, 0.8)] {
            let (a, b) = (m.eval(u, v), r.eval(u, v));
            assert!(
                (a[0] - b[0]).abs() < 0.02 && (a[1] - b[1]).abs() < 0.02,
                "{a:?} {b:?}"
            );
        }
    }

    #[test]
    fn a_bad_mesh_is_not_valid() {
        let mut m = Mesh::grid(3, 3);
        assert!(m.is_valid());
        m.points.pop();
        assert!(!m.is_valid());
        let mut m = Mesh::grid(3, 3);
        m.xs[1] = 1.5;
        assert!(!m.is_valid());
        let mut m = Mesh::grid(3, 3);
        m.points[0] = [f32::NAN, 0.0];
        assert!(!m.is_valid());
        let mut m = Mesh::grid(3, 3);
        m.xs = vec![0.0, 0.5, 0.5, 1.0];
        m.points = vec![[0.0, 0.0]; 12];
        assert!(!m.is_valid());
    }

    #[test]
    fn corners_and_flatness() {
        let m = Mesh::grid(3, 3);
        assert!(m.is_flat());
        assert_eq!(
            m.corners(),
            [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]
        );
        let mut m = m;
        *m.point_mut(1, 1) = [0.5, 0.6];
        assert!(!m.is_flat());
        m.flatten();
        assert!(m.is_flat());
        // Flattening keeps lines that were added at odd places.
        m.insert_column(0.1).unwrap();
        *m.point_mut(1, 1) = [0.3, 0.3];
        m.flatten();
        assert_eq!((m.xs[1], m.point(1, 1)[0]), (0.1, 0.1));
    }
}
