//! The lens map worked out on the CPU (mapping plan 4.1), pure: for each
//! pixel of a mapped projector's picture, the ray through the lens
//! (`Projector::ray`), the nearest still screen it meets in the scene
//! (`render3d::pick`, the venue in the way), and that spot's place in its
//! feed's picture, packed as `mapped::pack_place`/`pack_how` do. The GPU
//! bake (`render3d::bake`) is tested against it.
//!
//! What the bake and the reference share, so they can't disagree:
//! - `still_scene`: what a map is made of. The scene the views draw without
//!   the projectors' bodies and without any screen that can move
//!   (`Show::movable`; those are drawn live, plan 4.2).
//! - `feeds_for`: which canvases a projector's map names (its `feeds`),
//!   exactly `Show::mapped_feeds(p).used`, what the projector's row on the
//!   page says it maps: in the screens' order, at most `MAX_FEEDS`. A fed
//!   screen showing another canvas is in the way like the venue, and shows
//!   nothing; so is an LED screen (`Catch::led`, `Show::kind_of`), whatever
//!   it shows: no projector's light lands on it.
//! - Which wins where two meet: a fed screen wins over anything else (the
//!   venue, an unfed screen) up to `SCREEN_SLACK` of its distance behind it,
//!   so a screen hung on a wall of the venue model shows its picture.
//! - A front screen (`Catch::front_only`) met from behind shows nothing
//!   (the light lands on its back), and is still in the way.
//! - The place is taken at the middle of each pixel: the bake draws at twice
//!   the size and its resolve averages the four samples' places, which is
//!   the middle of the pixel when all four agree.
//! - The key (`map_key`): the still parts (`still_key`), the projector
//!   (`Projector::without_tuning`: never its `fine` or `line_up`), its
//!   feeds, the picture's shape and size. Nothing a cue changes.

use super::projector::Projector;
use crate::mapped::{CpuMap, How, MAX_FEEDS};
use crate::render3d::maths::{self, Mat4, Vec3};
use crate::render3d::{Paint, Part, Ray, Scene};
use crate::show::{OutputId, Show};
use crate::stage3d::scene::Fnv;

/// How far behind something else (a share of its distance from the lens) a
/// fed screen still wins: a screen on a wall shows its picture.
pub const SCREEN_SLACK: f32 = 0.001;

/// What a lens map is made of: `scene` (made by `Stage3d::scene` for `show`)
/// without the projectors' bodies and without the screens that can move. An
/// LED screen (`Catch::led`) is kept only as something in the way: its
/// picture is never a feed.
pub fn still_scene(show: &Show, scene: &Scene) -> Scene {
    let stage = &show.stage3d;
    let bodies_from = stage.projectors_from(scene);
    let venue_parts = scene.parts.iter().take_while(|p| p.lit).count();
    // Which surface each screen part is (as `Stage3d::part_surface`, worked
    // out once for every part).
    let mut owner: Vec<Option<super::SurfaceId>> = vec![None; scene.parts.len()];
    let mut i = venue_parts;
    for s in stage.surfaces.iter().filter(|s| s.visible) {
        let n = super::geometry::surface_mesh(&s.shape).len();
        for o in owner.iter_mut().skip(i).take(n) {
            *o = Some(s.id);
        }
        i += n;
    }
    Scene {
        parts: scene
            .parts
            .iter()
            .enumerate()
            .filter(|(i, _)| *i < bodies_from)
            .filter(|(i, _)| owner[*i].is_none_or(|s| !show.movable(s)))
            .map(|(_, p)| {
                let mut p = p.clone();
                if p.catch.led && matches!(p.paint, Paint::Output { .. }) {
                    p.paint = Paint::Colour(crate::stage3d::UNFED_GREY);
                }
                p
            })
            .collect(),
        throws: Vec::new(),
    }
}

/// A key for a still scene: each part's mesh, where it is, what it shows
/// and how. The same parts, the same key (a scene made again because a
/// projector moved has the same key).
pub fn still_key(scene: &Scene) -> u64 {
    let mut h = Fnv::new();
    for p in &scene.parts {
        h.bytes(&p.mesh_key.to_le_bytes());
        h.bytes(&(p.mesh.positions.len() as u64).to_le_bytes());
        p.transform.iter().flatten().for_each(|v| h.f32(*v));
        match &p.paint {
            Paint::Output { output, region } => {
                h.bytes(&[1]);
                h.bytes(&output.0.to_le_bytes());
                region.iter().for_each(|v| h.f32(*v));
            }
            _ => h.bytes(&[0]),
        }
        h.bytes(&[u8::from(p.lit), u8::from(p.catch.front_only)]);
    }
    h.0
}

/// The key a map of `projector` at `size` (a picture `aspect` wide per
/// high), naming `feeds` (`feeds_for`), is made for, over a still scene of
/// key `still`. `depth`: whether the map has its depth picture (only with a
/// movable screen, round 3). The projector is hashed whole but for its
/// fine-tune and line-up (`Projector::without_tuning`).
pub fn map_key(
    still: u64,
    projector: &Projector,
    aspect: f32,
    size: (u32, u32),
    depth: bool,
    feeds: &[OutputId],
) -> u64 {
    let mut h = Fnv::new();
    h.bytes(&still.to_le_bytes());
    h.bytes(format!("{:?}", projector.without_tuning()).as_bytes());
    for f in feeds {
        h.bytes(&f.0.to_le_bytes());
    }
    h.bytes(&(feeds.len() as u64).to_le_bytes());
    h.f32(aspect);
    h.bytes(&size.0.to_le_bytes());
    h.bytes(&size.1.to_le_bytes());
    h.bytes(&[u8::from(depth)]);
    h.0
}

/// The lens's matrix (world to its clip space, reversed depth), as the light
/// throws it (`light::throw_of`).
pub fn lens_matrix(projector: &Projector, aspect: f32) -> Mat4 {
    crate::stage3d::light::throw_of(projector, aspect, (1, 1), 0, true).view_proj
}

/// The lens's matrix for tile `tile` (`[x, y, w, h]` in pixels) of a
/// picture `size` big: the part of the lens's picture the tile covers, from
/// edge to edge of the tile.
pub fn tile_matrix(projector: &Projector, aspect: f32, size: (u32, u32), tile: [u32; 4]) -> Mat4 {
    use crate::stage3d::light::{THROW_FAR, THROW_NEAR};
    let f = projector.frustum(aspect);
    let (w, h) = (size.0.max(1) as f32, size.1.max(1) as f32);
    let (u0, u1) = (tile[0] as f32 / w, (tile[0] + tile[2]) as f32 / w);
    let (v0, v1) = (tile[1] as f32 / h, (tile[1] + tile[3]) as f32 / h);
    let sub = crate::render3d::camera::Frustum {
        left: f.left + u0 * (f.right - f.left),
        right: f.left + u1 * (f.right - f.left),
        top: f.top - v0 * (f.top - f.bottom),
        bottom: f.top - v1 * (f.top - f.bottom),
    };
    let eye = projector.position();
    let view = maths::look_at(eye, maths::add(eye, projector.axis()), projector.up())
        .unwrap_or(maths::IDENTITY);
    maths::mul(&maths::frustum_reversed(&sub, THROW_NEAR, THROW_FAR), &view)
}

/// The canvases projector `id`'s map names: exactly what `Show::
/// mapped_feeds` uses (and the projector's row on the page says it maps),
/// in the screens' order, at most `MAX_FEEDS`.
pub fn feeds_for(show: &Show, id: crate::stage3d::ProjectorId) -> Vec<OutputId> {
    let mut used = show.mapped_feeds(id).used;
    used.truncate(MAX_FEEDS);
    used
}

/// Which feed part `part` shows in a map naming `feeds`, if it is a fed
/// screen (else it is only in the way).
pub fn part_feed(part: &Part, feeds: &[OutputId]) -> Option<u8> {
    match part.paint {
        Paint::Output { output, .. } if !part.lit => {
            feeds.iter().position(|f| *f == output).map(|i| i as u8)
        }
        _ => None,
    }
}

/// A part's region, as the renderer reads it (the whole picture for one
/// that isn't numbers).
pub fn part_region(part: &Part) -> [f32; 4] {
    match part.paint {
        Paint::Output { region, .. } if region.iter().all(|v| v.is_finite()) => region,
        _ => [0.0, 0.0, 1.0, 1.0],
    }
}

/// What projector `projector`'s pixels see of `scene`'s still screens
/// showing `feeds` (`feeds_for`), for a picture `size` big, its shape the
/// projector's for that size (`Projector::aspect`).
pub fn reference(
    scene: &Scene,
    projector: &Projector,
    size: (u32, u32),
    feeds: &[OutputId],
) -> CpuMap {
    reference_with(scene, projector, size, projector.aspect(size), feeds)
}

/// `reference`, for a picture `aspect` wide per high whatever its size (a
/// 3D view's light draws every projector at 16:9).
pub fn reference_with(
    scene: &Scene,
    projector: &Projector,
    size: (u32, u32),
    aspect: f32,
    feeds: &[OutputId],
) -> CpuMap {
    let mut map = CpuMap::black(size);
    let feeds = &feeds[..feeds.len().min(MAX_FEEDS)];
    let fed: Vec<Option<u8>> = scene.parts.iter().map(|p| part_feed(p, feeds)).collect();
    let (w, h) = (size.0.max(1) as f32, size.1.max(1) as f32);
    for y in 0..size.1 {
        for x in 0..size.0 {
            let u = (x as f32 + 0.5) / w;
            let v = (y as f32 + 0.5) / h;
            let ray = projector.ray(u, v, aspect);
            if let Some((place, feed)) = seen_along(scene, &fed, &ray) {
                map.set(x, y, place, How::feed(feed));
            }
        }
    }
    map.feeds = feeds.to_vec();
    map
}

/// What a ray from the lens sees: the place in its feed's picture and which
/// feed, or None (nothing, the venue, an unfed screen, a screen's back).
fn seen_along(scene: &Scene, fed: &[Option<u8>], ray: &Ray) -> Option<([f32; 2], u8)> {
    let screen = crate::render3d::pick(scene, ray, &|i, _| fed[i].is_some())?;
    let other = crate::render3d::pick(scene, ray, &|i, _| fed[i].is_none());
    if other.is_some_and(|o| screen.distance / (1.0 + SCREEN_SLACK) >= o.distance) {
        return None;
    }
    let part = &scene.parts[screen.part];
    if part.catch.front_only && !screen.front {
        return None;
    }
    let uv = uv_on(part, ray)?;
    let r = part_region(part);
    Some((
        [r[0] + uv[0] * r[2], r[1] + uv[1] * r[3]],
        fed[screen.part]?,
    ))
}

/// The picture's uv where `ray` first meets `part` (the corners' uvs
/// weighed by where in the triangle it is).
fn uv_on(part: &Part, ray: &Ray) -> Option<[f32; 2]> {
    let mesh = &part.mesh;
    let world: Vec<Vec3> = mesh
        .positions
        .iter()
        .map(|p| maths::transform_point(&part.transform, *p))
        .collect();
    let uv = |i: u32| mesh.uvs.get(i as usize).copied().unwrap_or([0.0, 0.0]);
    let mut best: Option<(f32, [f32; 2])> = None;
    for t in mesh.indices.as_chunks::<3>().0 {
        let (Some(a), Some(b), Some(c)) = (
            world.get(t[0] as usize),
            world.get(t[1] as usize),
            world.get(t[2] as usize),
        ) else {
            continue;
        };
        let Some((d, s, r)) = meets(ray, *a, *b, *c) else {
            continue;
        };
        if best.is_none_or(|(bd, _)| d < bd) {
            let (ua, ub, uc) = (uv(t[0]), uv(t[1]), uv(t[2]));
            let k = 1.0 - s - r;
            best = Some((
                d,
                [
                    k * ua[0] + s * ub[0] + r * uc[0],
                    k * ua[1] + s * ub[1] + r * uc[1],
                ],
            ));
        }
    }
    best.map(|(_, uv)| uv)
}

/// Möller–Trumbore, both faces: how far along the ray, and where in the
/// triangle (the weights of `b` and `c`).
fn meets(ray: &Ray, a: Vec3, b: Vec3, c: Vec3) -> Option<(f32, f32, f32)> {
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
    (t > 1e-6 && t.is_finite()).then_some((t, u, v))
}

#[cfg(test)]
#[path = "lensmap_tests.rs"]
pub(crate) mod tests;
