//! Picking (pure): which part a ray from the screen meets first.

use super::camera::{Bounds, Ray};
use super::maths::{self, Vec3};
use super::scene::{Part, Scene};

/// Where a ray met a part.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    /// The part's index in `Scene::parts`.
    pub part: usize,
    /// The point met, in the world.
    pub point: Vec3,
    /// How far along the ray (metres from its start).
    pub distance: f32,
}

/// The nearest part that `ray` meets, among those `wanted` says yes to (the
/// screens, say: the venue can be left out). Both faces of a triangle count;
/// triangles with an index out of range are skipped.
pub fn pick(scene: &Scene, ray: &Ray, wanted: &dyn Fn(usize, &Part) -> bool) -> Option<Hit> {
    let mut best: Option<Hit> = None;
    let mut world = Vec::new();
    for (i, part) in scene.parts.iter().enumerate() {
        if !wanted(i, part) {
            continue;
        }
        let mesh = &part.mesh;
        world.clear();
        world.extend(
            mesh.positions
                .iter()
                .map(|p| maths::transform_point(&part.transform, *p)),
        );
        let Some(b) = Bounds::around(world.iter().copied()) else {
            continue;
        };
        let nearest = best.map_or(f32::INFINITY, |h| h.distance);
        if !meets_box(ray, &b, nearest) {
            continue;
        }
        for t in mesh.indices.as_chunks::<3>().0 {
            let (Some(a), Some(b), Some(c)) = (
                world.get(t[0] as usize),
                world.get(t[1] as usize),
                world.get(t[2] as usize),
            ) else {
                continue;
            };
            if let Some(d) = meets_triangle(ray, *a, *b, *c)
                && best.is_none_or(|h| d < h.distance)
            {
                best = Some(Hit {
                    part: i,
                    point: maths::add(ray.origin, maths::scale(ray.dir, d)),
                    distance: d,
                });
            }
        }
    }
    best
}

/// Whether the ray meets the box nearer than `within` (the slab test).
fn meets_box(ray: &Ray, b: &Bounds, within: f32) -> bool {
    let (mut lo, mut hi) = (0.0f32, within);
    for i in 0..3 {
        let d = ray.dir[i];
        if d.abs() < 1e-12 {
            if ray.origin[i] < b.min[i] || ray.origin[i] > b.max[i] {
                return false;
            }
            continue;
        }
        let (mut t0, mut t1) = (
            (b.min[i] - ray.origin[i]) / d,
            (b.max[i] - ray.origin[i]) / d,
        );
        if t0 > t1 {
            std::mem::swap(&mut t0, &mut t1);
        }
        lo = lo.max(t0);
        hi = hi.min(t1);
        if lo > hi + 1e-5 {
            return false;
        }
    }
    true
}

/// Möller–Trumbore, both faces: how far along the ray it meets the triangle.
fn meets_triangle(ray: &Ray, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let e1 = maths::sub(b, a);
    let e2 = maths::sub(c, a);
    let p = maths::cross(ray.dir, e2);
    let det = maths::dot(e1, p);
    if !det.is_finite() || det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = maths::sub(ray.origin, a);
    let u = maths::dot(s, p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = maths::cross(s, e1);
    let v = maths::dot(ray.dir, q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = maths::dot(e2, q) * inv;
    (t > 1e-6 && t.is_finite()).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render3d::scene::{Camera, Mesh, Paint};
    use std::sync::Arc;

    fn quad(at: Vec3, size: f32) -> Part {
        let mut m = maths::IDENTITY;
        m[0][0] = size;
        m[1][1] = size;
        m[3] = [at[0], at[1], at[2], 1.0];
        Part {
            mesh: Arc::new(Mesh {
                positions: vec![
                    [-0.5, -0.5, 0.0],
                    [0.5, -0.5, 0.0],
                    [0.5, 0.5, 0.0],
                    [-0.5, 0.5, 0.0],
                ],
                normals: vec![],
                uvs: vec![],
                // Clockwise seen from the camera: picked all the same.
                indices: vec![0, 2, 1, 0, 3, 2],
            }),
            mesh_key: 7,
            transform: m,
            paint: Paint::Colour([1.0; 4]),
            lit: false,
        }
    }

    fn camera() -> Camera {
        Camera {
            eye: [0.0, 0.0, 10.0],
            target: [0.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            fov_y_deg: 60.0,
            near: 0.1,
            far: 100.0,
            ..Camera::default()
        }
    }

    #[test]
    fn the_nearest_part_under_the_pointer_is_picked() {
        let scene = Scene {
            throws: Vec::new(),
            parts: vec![
                quad([0.0, 0.0, -2.0], 4.0), // behind, bigger
                quad([0.0, 0.0, 1.0], 2.0),  // in front
                quad([3.0, 0.0, 2.0], 1.0),  // off to the side
            ],
        };
        let size = (400, 300);
        let all = |_: usize, _: &Part| true;
        let hit = pick(&scene, &camera().ray([200.0, 150.0], size), &all).unwrap();
        assert_eq!(hit.part, 1);
        assert!((hit.point[2] - 1.0).abs() < 1e-4 && hit.point[0].abs() < 1e-4);
        assert!((hit.distance - 9.0).abs() < 1e-4);
        // Leaving the front one out finds the one behind it.
        let not_1 = |i: usize, _: &Part| i != 1;
        assert_eq!(
            pick(&scene, &camera().ray([200.0, 150.0], size), &not_1)
                .unwrap()
                .part,
            0
        );
        // The side one, where the camera sees it.
        let side = camera().project([3.0, 0.0, 2.0], size).unwrap();
        assert_eq!(
            pick(&scene, &camera().ray(side, size), &all).unwrap().part,
            2
        );
        // Nothing in a corner.
        assert!(pick(&scene, &camera().ray([2.0, 2.0], size), &all).is_none());
    }

    #[test]
    fn broken_meshes_and_parts_behind_the_eye_are_not_picked() {
        let mut behind = quad([0.0, 0.0, 20.0], 4.0);
        behind.mesh = Arc::new(Mesh {
            positions: vec![[0.0; 3]],
            normals: vec![],
            uvs: vec![],
            indices: vec![0, 5, 9, 1],
        });
        let scene = Scene {
            throws: Vec::new(),
            parts: vec![behind, quad([0.0, 0.0, 30.0], 4.0)],
        };
        let all = |_: usize, _: &Part| true;
        assert!(pick(&scene, &camera().ray([200.0, 150.0], (400, 300)), &all).is_none());
    }
}
