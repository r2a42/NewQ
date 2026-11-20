//! The camera's sums (pure): view and projection, an orbiting camera moved by
//! the mouse, and framing a scene's bounds.

use super::maths::{self, Mat4, Vec3};
use super::scene::{Camera, Scene};

/// Field of view used when a camera's is not a sensible number.
pub const DEFAULT_FOV: f32 = 50.0;
const MIN_FOV: f32 = 1.0;
const MAX_FOV: f32 = 160.0;
/// Pitch is kept off the poles, where the view's "up" would be lost.
pub const MAX_PITCH: f32 = 89.0 * std::f32::consts::PI / 180.0;
pub const MIN_DISTANCE: f32 = 0.05;
pub const MAX_DISTANCE: f32 = 100_000.0;
/// Radians turned per pixel dragged.
pub const ORBIT_PER_PIXEL: f32 = 0.006;
/// How much one notch of the wheel moves in (`dolly(1.0)` is 15 % closer).
pub const DOLLY_STEP: f32 = 0.85;

impl Camera {
    /// The camera with every number usable: a field of view of 1° to 160°,
    /// a positive near plane, a far plane beyond it, an eye apart from the
    /// target and an up not along the view. Nothing here panics on NaN.
    pub fn sane(&self) -> Camera {
        let fov = if self.fov_y_deg.is_finite() {
            self.fov_y_deg.clamp(MIN_FOV, MAX_FOV)
        } else {
            DEFAULT_FOV
        };
        let near = if self.near.is_finite() && self.near > 0.0 {
            self.near
        } else {
            0.05
        };
        let far = if self.far.is_finite() && self.far > near * 1.001 {
            self.far
        } else {
            (near * 100_000.0).max(1000.0)
        };
        let target = if maths::finite(self.target) {
            self.target
        } else {
            [0.0; 3]
        };
        let mut eye = if maths::finite(self.eye) {
            self.eye
        } else {
            maths::add(target, [0.0, 1.7, 10.0])
        };
        if maths::length(maths::sub(target, eye)) < 1e-6 {
            eye = maths::add(target, [0.0, 0.0, 1.0]);
        }
        let forward = maths::normalize(maths::sub(target, eye)).unwrap_or([0.0, 0.0, -1.0]);
        let up = maths::normalize(self.up)
            .filter(|u| maths::length(maths::cross(forward, *u)) > 1e-4)
            .unwrap_or_else(|| {
                // Along the view (looking straight down, say): any up across it.
                if forward[1].abs() < 0.99 {
                    [0.0, 1.0, 0.0]
                } else {
                    [0.0, 0.0, -forward[1].signum()]
                }
            });
        Camera {
            eye,
            target,
            up,
            fov_y_deg: fov,
            near,
            far,
        }
    }

    /// World to view (right-handed: the camera looks down -Z).
    pub fn view(&self) -> Mat4 {
        let c = self.sane();
        maths::look_at(c.eye, c.target, c.up).unwrap_or(maths::IDENTITY)
    }

    /// View to clip, for a picture `aspect` wide per one high. Depth is
    /// reversed (near 1, far 0): see `maths::perspective_reversed`.
    pub fn projection(&self, aspect: f32) -> Mat4 {
        let c = self.sane();
        let aspect = if aspect.is_finite() && aspect > 1e-4 {
            aspect
        } else {
            1.0
        };
        maths::perspective_reversed(c.fov_y_deg.to_radians(), aspect, c.near, c.far)
    }

    /// World to clip.
    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        maths::mul(&self.projection(aspect), &self.view())
    }

    /// Where a world point is in a picture of `size` (pixels from its
    /// top-left), or None when it is behind the camera.
    pub fn project(&self, point: Vec3, size: (u32, u32)) -> Option<[f32; 2]> {
        let (w, h) = (size.0.max(1) as f32, size.1.max(1) as f32);
        let v = maths::transform4(
            &self.view_projection(w / h),
            [point[0], point[1], point[2], 1.0],
        );
        (v[3] > 1e-6).then(|| [(v[0] / v[3] + 1.0) * 0.5 * w, (1.0 - v[1] / v[3]) * 0.5 * h])
    }

    /// The ray from the eye through `pixel` (from the top-left of a picture
    /// of `size`): its start and a direction one long.
    pub fn ray(&self, pixel: [f32; 2], size: (u32, u32)) -> Ray {
        let c = self.sane();
        let (w, h) = (size.0.max(1) as f32, size.1.max(1) as f32);
        let x = pixel[0] / w * 2.0 - 1.0;
        let y = 1.0 - pixel[1] / h * 2.0;
        let f = maths::normalize(maths::sub(c.target, c.eye)).unwrap_or([0.0, 0.0, -1.0]);
        let s = maths::normalize(maths::cross(f, c.up)).unwrap_or([1.0, 0.0, 0.0]);
        let u = maths::cross(s, f);
        let t = (c.fov_y_deg.to_radians() * 0.5).tan();
        let dir = maths::add(
            f,
            maths::add(maths::scale(s, x * t * w / h), maths::scale(u, y * t)),
        );
        Ray {
            origin: c.eye,
            dir: maths::normalize(dir).unwrap_or(f),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    /// One long.
    pub dir: Vec3,
}

/// An axis-aligned box in the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min: Vec3,
    pub max: Vec3,
}

impl Bounds {
    pub fn centre(&self) -> Vec3 {
        maths::scale(maths::add(self.min, self.max), 0.5)
    }

    pub fn radius(&self) -> f32 {
        maths::length(maths::sub(self.max, self.min)) * 0.5
    }

    fn take(&mut self, p: Vec3) {
        for (i, v) in p.into_iter().enumerate() {
            self.min[i] = self.min[i].min(v);
            self.max[i] = self.max[i].max(v);
        }
    }

    /// The box around `points` (only the finite ones), None if there are none.
    pub fn around(points: impl IntoIterator<Item = Vec3>) -> Option<Bounds> {
        let mut b: Option<Bounds> = None;
        for p in points.into_iter().filter(|p| maths::finite(*p)) {
            match &mut b {
                Some(b) => b.take(p),
                None => b = Some(Bounds { min: p, max: p }),
            }
        }
        b
    }
}

/// Every part's vertices that some triangle uses, in the world: what "zoom to
/// fit" frames. None for a scene with nothing in it.
pub fn scene_bounds(scene: &Scene) -> Option<Bounds> {
    Bounds::around(scene.parts.iter().flat_map(|p| {
        let n = p.mesh.positions.len();
        p.mesh
            .indices
            .iter()
            .filter(move |&&i| (i as usize) < n)
            .map(move |&i| maths::transform_point(&p.transform, p.mesh.positions[i as usize]))
    }))
}

/// A camera that turns around a point: the usual way to look at a model.
/// `yaw` is about the up (Y) axis, 0 with the eye on +Z of the target;
/// `pitch` is how far above the target the eye is (positive: looking down).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Orbit {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub target: Vec3,
    pub fov_y_deg: f32,
}

impl Default for Orbit {
    /// From the back of a hall, a little above head height.
    fn default() -> Orbit {
        Orbit {
            yaw: 0.0,
            pitch: 15f32.to_radians(),
            distance: 15.0,
            target: [0.0, 1.5, 0.0],
            fov_y_deg: DEFAULT_FOV,
        }
    }
}

impl Orbit {
    pub fn eye(&self) -> Vec3 {
        let o = self.sane();
        let (sy, cy) = o.yaw.sin_cos();
        let (sp, cp) = o.pitch.sin_cos();
        maths::add(o.target, maths::scale([cp * sy, sp, cp * cy], o.distance))
    }

    fn sane(&self) -> Orbit {
        let d = Orbit::default();
        Orbit {
            yaw: if self.yaw.is_finite() {
                self.yaw.rem_euclid(std::f32::consts::TAU)
            } else {
                d.yaw
            },
            pitch: if self.pitch.is_finite() {
                self.pitch.clamp(-MAX_PITCH, MAX_PITCH)
            } else {
                d.pitch
            },
            distance: if self.distance.is_finite() {
                self.distance.clamp(MIN_DISTANCE, MAX_DISTANCE)
            } else {
                d.distance
            },
            target: if maths::finite(self.target) {
                self.target
            } else {
                d.target
            },
            fov_y_deg: if self.fov_y_deg.is_finite() {
                self.fov_y_deg.clamp(MIN_FOV, MAX_FOV)
            } else {
                DEFAULT_FOV
            },
        }
    }

    /// The camera it is. Near and far follow the distance, so a close look
    /// and a whole arena both have depth to spare.
    pub fn camera(&self) -> Camera {
        let o = self.sane();
        Camera {
            eye: self.eye(),
            target: o.target,
            up: [0.0, 1.0, 0.0],
            fov_y_deg: o.fov_y_deg,
            near: (o.distance * 0.002).clamp(0.01, 1.0),
            far: (o.distance * 100.0).max(2000.0),
        }
    }

    /// Looking as `camera` does (its roll, if any, is dropped).
    pub fn from_camera(camera: &Camera) -> Orbit {
        let c = camera.sane();
        let d = maths::sub(c.eye, c.target);
        let distance = maths::length(d);
        Orbit {
            yaw: d[0].atan2(d[2]),
            pitch: (d[1] / distance).clamp(-1.0, 1.0).asin(),
            distance,
            target: c.target,
            fov_y_deg: c.fov_y_deg,
        }
        .sane()
    }

    /// Turn around the target by a mouse drag of `dx`, `dy` pixels: dragging
    /// right turns the model to the right, dragging down looks from higher.
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        let o = self.sane();
        self.yaw = (o.yaw - finite_or_0(dx) * ORBIT_PER_PIXEL).rem_euclid(std::f32::consts::TAU);
        self.pitch = (o.pitch + finite_or_0(dy) * ORBIT_PER_PIXEL).clamp(-MAX_PITCH, MAX_PITCH);
    }

    /// Slide the target across the view by a drag of `dx`, `dy` pixels in a
    /// view `height` pixels high, so the point under the pointer (at the
    /// target's distance) stays under it.
    pub fn pan(&mut self, dx: f32, dy: f32, height: f32) {
        let o = self.sane();
        let cam = o.camera();
        let f = maths::normalize(maths::sub(cam.target, cam.eye)).unwrap_or([0.0, 0.0, -1.0]);
        let Some(s) = maths::normalize(maths::cross(f, [0.0, 1.0, 0.0])) else {
            return;
        };
        let u = maths::cross(s, f);
        let per_pixel = 2.0 * o.distance * (o.fov_y_deg.to_radians() * 0.5).tan() / height.max(1.0);
        let moved = maths::add(
            maths::scale(s, -finite_or_0(dx) * per_pixel),
            maths::scale(u, finite_or_0(dy) * per_pixel),
        );
        self.target = maths::add(o.target, moved);
    }

    /// Move in (`steps` > 0, a wheel notch each) or out, towards the target.
    pub fn dolly(&mut self, steps: f32) {
        let o = self.sane();
        self.distance =
            (o.distance * DOLLY_STEP.powf(finite_or_0(steps))).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    /// Look at the whole of `bounds` from the present direction, in a view
    /// `aspect` wide per one high ("zoom to fit"): the sphere around the box
    /// fits in the narrower field of view, with a little room.
    pub fn frame(&mut self, bounds: &Bounds, aspect: f32) {
        let o = self.sane();
        let aspect = if aspect.is_finite() && aspect > 1e-3 {
            aspect
        } else {
            1.0
        };
        let half_y = o.fov_y_deg.to_radians() * 0.5;
        let half_x = (half_y.tan() * aspect).atan();
        let half = half_y.min(half_x);
        let r = bounds.radius().max(0.1);
        self.target = bounds.centre();
        self.distance = (r * 1.05 / half.sin()).clamp(MIN_DISTANCE, MAX_DISTANCE);
        self.yaw = o.yaw;
        self.pitch = o.pitch;
        self.fov_y_deg = o.fov_y_deg;
    }

    /// `frame` the scene; nothing changes for an empty one.
    pub fn frame_scene(&mut self, scene: &Scene, aspect: f32) -> bool {
        match scene_bounds(scene) {
            Some(b) => {
                self.frame(&b, aspect);
                true
            }
            None => false,
        }
    }
}

fn finite_or_0(v: f32) -> f32 {
    if v.is_finite() { v } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render3d::scene::{Mesh, Paint, Part};
    use std::sync::Arc;

    fn close(a: Vec3, b: Vec3, tol: f32) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < tol)
    }

    fn cam(eye: Vec3, target: Vec3) -> Camera {
        Camera {
            eye,
            target,
            up: [0.0, 1.0, 0.0],
            fov_y_deg: 60.0,
            near: 0.1,
            far: 100.0,
        }
    }

    #[test]
    fn the_target_projects_to_the_middle_and_right_is_right() {
        let c = cam([0.0, 1.0, 10.0], [0.0, 1.0, 0.0]);
        let mid = c.project([0.0, 1.0, 0.0], (200, 100)).unwrap();
        assert!((mid[0] - 100.0).abs() < 1e-3 && (mid[1] - 50.0).abs() < 1e-3);
        let right_up = c.project([1.0, 2.0, 0.0], (200, 100)).unwrap();
        assert!(right_up[0] > 100.0 && right_up[1] < 50.0);
        assert!(c.project([0.0, 1.0, 20.0], (200, 100)).is_none(), "behind");
    }

    #[test]
    fn a_ray_through_a_pixel_goes_back_through_its_projection() {
        let c = cam([3.0, 2.0, 8.0], [-1.0, 0.5, 0.0]);
        let size = (640, 360);
        for p in [[10.0, 20.0], [320.0, 180.0], [600.0, 300.0]] {
            let r = c.ray(p, size);
            let point = maths::add(r.origin, maths::scale(r.dir, 7.0));
            let back = c.project(point, size).unwrap();
            assert!(
                (back[0] - p[0]).abs() < 0.05 && (back[1] - p[1]).abs() < 0.05,
                "{p:?} -> {back:?}"
            );
        }
    }

    #[test]
    fn a_broken_camera_is_made_usable() {
        let c = Camera {
            eye: [0.0, 5.0, 0.0],
            target: [0.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0], // along the view
            fov_y_deg: f32::NAN,
            near: -1.0,
            far: 0.0,
        }
        .sane();
        assert_eq!(c.fov_y_deg, DEFAULT_FOV);
        assert!(c.near > 0.0 && c.far > c.near);
        assert!(maths::look_at(c.eye, c.target, c.up).is_some());
        let same = cam([1.0, 1.0, 1.0], [1.0, 1.0, 1.0]).sane();
        assert!(maths::look_at(same.eye, same.target, same.up).is_some());
        let m = cam([f32::NAN; 3], [0.0; 3]).view_projection(f32::NAN);
        assert!(m.iter().flatten().all(|v| v.is_finite()));
    }

    #[test]
    fn an_orbit_turns_around_its_target_at_its_distance() {
        let mut o = Orbit {
            yaw: 0.0,
            pitch: 0.0,
            distance: 10.0,
            target: [1.0, 2.0, 3.0],
            fov_y_deg: 50.0,
        };
        assert!(close(o.eye(), [1.0, 2.0, 13.0], 1e-4));
        // A quarter turn to the left by dragging: the eye goes round to -X... or +X.
        o.orbit(-(std::f32::consts::FRAC_PI_2 / ORBIT_PER_PIXEL), 0.0);
        assert!(close(o.eye(), [11.0, 2.0, 3.0], 1e-3), "{:?}", o.eye());
        // Dragging down looks from above; never over the pole.
        o.orbit(0.0, 1e6);
        assert!((o.pitch - MAX_PITCH).abs() < 1e-6);
        assert!(o.eye()[1] > 2.0 + 9.9);
        let c = o.camera();
        assert!(maths::look_at(c.eye, c.target, c.up).is_some());
        // Back from the camera.
        let again = Orbit::from_camera(&c);
        assert!(close(again.eye(), o.eye(), 1e-3));
        assert!((again.distance - 10.0).abs() < 1e-4);
    }

    #[test]
    fn panning_keeps_the_point_under_the_pointer() {
        let mut o = Orbit {
            yaw: 0.4,
            pitch: 0.3,
            distance: 12.0,
            target: [0.0, 1.0, 0.0],
            fov_y_deg: 50.0,
        };
        let size = (800u32, 500u32);
        let grabbed = o.target;
        let before = o.camera().project(grabbed, size).unwrap();
        o.pan(40.0, -25.0, size.1 as f32);
        let after = o.camera().project(grabbed, size).unwrap();
        assert!(
            (after[0] - before[0] - 40.0).abs() < 0.1,
            "{before:?} {after:?}"
        );
        assert!(
            (after[1] - before[1] + 25.0).abs() < 0.1,
            "{before:?} {after:?}"
        );
    }

    #[test]
    fn the_wheel_moves_in_and_out_within_limits() {
        let mut o = Orbit::default();
        let d = o.distance;
        o.dolly(1.0);
        assert!((o.distance - d * DOLLY_STEP).abs() < 1e-4);
        o.dolly(-1.0);
        assert!((o.distance - d).abs() < 1e-4);
        o.dolly(1e4);
        assert_eq!(o.distance, MIN_DISTANCE);
        o.dolly(f32::NAN);
        assert_eq!(o.distance, MIN_DISTANCE);
        o.dolly(-1e6);
        assert_eq!(o.distance, MAX_DISTANCE);
    }

    fn quad_at(transform: [[f32; 4]; 4]) -> Part {
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
                indices: vec![0, 1, 2, 0, 2, 3, 99], // a stray index
            }),
            mesh_key: 1,
            transform,
            paint: Paint::Colour([1.0; 4]),
            lit: true,
        }
    }

    #[test]
    fn zoom_to_fit_shows_every_corner_of_the_scene() {
        let mut far = maths::IDENTITY;
        far[3] = [20.0, 4.0, -30.0, 1.0];
        let scene = Scene {
            parts: vec![quad_at(maths::IDENTITY), quad_at(far)],
        };
        let b = scene_bounds(&scene).unwrap();
        assert!(close(b.min, [-0.5, -0.5, -30.0], 1e-5));
        assert!(close(b.max, [20.5, 4.5, 0.0], 1e-5));
        for aspect in [0.5f32, 1.0, 16.0 / 9.0, 3.0] {
            let mut o = Orbit {
                yaw: 1.0,
                pitch: 0.4,
                ..Orbit::default()
            };
            assert!(o.frame_scene(&scene, aspect));
            assert_eq!(o.target, b.centre());
            let size = ((400.0 * aspect) as u32, 400);
            let c = o.camera();
            for i in 0..8 {
                let corner = [
                    if i & 1 == 0 { b.min[0] } else { b.max[0] },
                    if i & 2 == 0 { b.min[1] } else { b.max[1] },
                    if i & 4 == 0 { b.min[2] } else { b.max[2] },
                ];
                let p = c.project(corner, size).unwrap();
                assert!(
                    (0.0..=size.0 as f32).contains(&p[0]) && (0.0..=size.1 as f32).contains(&p[1]),
                    "aspect {aspect}: corner {corner:?} at {p:?}"
                );
            }
            // ... and not from miles away: the box's sphere just fits the
            // narrower side, so its middle is in the middle and the box is
            // more than a third of that side.
            let mid = c.project(b.centre(), size).unwrap();
            assert!((mid[0] - size.0 as f32 / 2.0).abs() < 0.5);
            let xs: Vec<f32> = (0..8)
                .map(|i| {
                    let corner = [
                        if i & 1 == 0 { b.min[0] } else { b.max[0] },
                        b.min[1],
                        if i & 4 == 0 { b.min[2] } else { b.max[2] },
                    ];
                    c.project(corner, size).unwrap()[0]
                })
                .collect();
            let spread = xs.iter().cloned().fold(f32::MIN, f32::max)
                - xs.iter().cloned().fold(f32::MAX, f32::min);
            assert!(
                spread > size.0.min(size.1) as f32 / 3.0,
                "aspect {aspect}: {spread}"
            );
        }
        let mut o = Orbit::default();
        assert!(!o.frame_scene(&Scene { parts: vec![] }, 1.0));
        assert_eq!(o, Orbit::default());
    }
}
