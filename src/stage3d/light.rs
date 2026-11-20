//! How bright a projector's picture is where it lands (pure). Deliberately
//! simple, and the read-outs say "about":
//!
//! - A projector gives `lumens` at full white, spread evenly over its picture.
//! - On a wall square to the lens axis, `z` metres along it, that is
//!   `E0 = lumens / (w1 * h1 * z²)` lux (`w1`, `h1` the picture's size one
//!   metre out): the inverse square law.
//! - On a real surface with normal `n`, for `l` the direction from the
//!   surface to the lens and `a` the axis:
//!   `E = E0 * max(n·l, 0) / max(a·(-l), ε)`. The first cosine is the angle
//!   the light comes in at; the second undoes the square wall's own slant off
//!   the axis (the same pencil of light covers `dA` on the square wall and
//!   `dA cos θa / cos θi` on the surface).
//! - A screen's brightness to the eye is `L = E * gain / π` nits (candela
//!   per square metre); gain 1 is a matt white screen.
//! - A rear-projection screen, or a venue's wall (drawn from both sides), is
//!   lit from either side (`Facing::Both`); a front screen lit from behind
//!   shows nothing.
//! - Light from several projectors adds.
//!
//! A worked check, in the tests: 10,000 lumens on a 16:9 picture 6 m wide
//! (20.25 m²) is 494 lux, and 157 nits on a matt white screen.
//!
//! Left out, and not pretended: lenses are dimmer at the edges, a screen's
//! gain falls off to the side, projectors differ in colour, black isn't
//! black, light bounces round the room, and quoted lumens are optimistic.

use super::geometry;
use super::projector::{Projector, ProjectorId};
use crate::render3d::{self, Part, Ray, Scene, maths};
use crate::stage3d::scene::Throw;

/// The most projectors one view lights (the shader's loop, the throws'
/// picture array). Past these, the view says so in words.
pub const MAX_THROWS: usize = 16;

/// Where a throw's depth starts and ends (metres from the lens): its
/// shadow map's range.
pub const THROW_NEAR: f32 = 0.1;
pub const THROW_FAR: f32 = 2000.0;

/// The Brightness view's steps (nits): five colours, under the first,
/// between each two, over the last.
pub const BRIGHTNESS_STEPS: [f32; 4] = [50.0, 100.0, 200.0, 400.0];
/// The Pixel size view's steps (millimetres per pixel).
pub const PIXEL_STEPS_MM: [f32; 4] = [1.0, 2.0, 5.0, 10.0];
/// The five colours of those views, from the first band to the last (display
/// values).
pub const SCALE: [[f32; 3]; 5] = [
    [0.15, 0.25, 0.85],
    [0.1, 0.7, 0.85],
    [0.2, 0.8, 0.25],
    [0.95, 0.85, 0.15],
    [0.95, 0.25, 0.15],
];
/// The Overlap view's colours: lit by one projector, two, three or more.
pub const OVERLAP: [[f32; 3]; 3] = [[0.2, 0.75, 0.3], [0.95, 0.85, 0.15], [0.95, 0.25, 0.15]];

/// How much of the room's usual light stays on when a view's light is on,
/// so the room can still be made out round the projected light.
pub const ROOM_LIGHT: f32 = 0.03;

/// Which band of `steps` `value` is in (0 under the first, `steps.len()`
/// at or over the last).
pub fn band(value: f32, steps: &[f32]) -> usize {
    if !value.is_finite() {
        return 0;
    }
    steps.iter().take_while(|s| value >= **s).count()
}

/// The Overlap view's colour for a point lit by `count` projectors (None:
/// none).
pub fn overlap_colour(count: usize) -> Option<[f32; 3]> {
    match count {
        0 => None,
        n => Some(OVERLAP[(n - 1).min(OVERLAP.len() - 1)]),
    }
}

/// Projector `p`'s light as the renderer wants it: its picture `aspect`
/// wide per one high, `pixels` wide and high, in picture array layer
/// `layer` (`picture` false: it throws plain white). The matrix is made
/// straight from the lens's frustum (not through a `Camera`, which keeps a
/// view's angle within limits), so it agrees with `Projector::in_picture`.
pub fn throw_of(
    p: &Projector,
    aspect: f32,
    pixels: (u32, u32),
    layer: u32,
    picture: bool,
) -> Throw {
    let eye = p.position();
    let axis = p.axis();
    let view = maths::look_at(eye, maths::add(eye, axis), p.up()).unwrap_or(maths::IDENTITY);
    let proj = maths::frustum_reversed(&p.frustum(aspect), THROW_NEAR, THROW_FAR);
    let (w1, _) = p.at_one_metre(aspect);
    let lux = lux_at_one_metre(p, aspect);
    Throw {
        view_proj: maths::mul(&proj, &view),
        eye,
        axis,
        lux_at_one_metre: if lux.is_finite() { lux as f32 } else { 0.0 },
        gamma: p.lens.gamma(),
        layer,
        picture,
        pixel_at_one_metre: w1 / pixels.0.max(1) as f32,
        on: p.visible,
    }
}

/// Where a world point lands in a throw's picture (`(u, v)`, 0..1 inside,
/// (0, 0) its top-left) and its depth along the lens's axis; None behind
/// it. The shader's sum, on the processor.
pub fn throw_uv(t: &Throw, point: [f32; 3]) -> Option<([f32; 2], f32)> {
    let c = maths::transform4(&t.view_proj, [point[0], point[1], point[2], 1.0]);
    if !(c[3].is_finite() && c[3] > 1e-6) {
        return None;
    }
    let (x, y) = (c[0] / c[3], c[1] / c[3]);
    let depth = maths::dot(maths::sub(point, t.eye), t.axis);
    Some(([x * 0.5 + 0.5, 0.5 - y * 0.5], depth))
}

/// The light (lux) a throw puts on `point` (normal `normal`; light counted
/// on the front only when `front_only`) at full white, with nothing in the
/// way: what the shader adds for it before its picture and its shadow.
/// The same model as `illuminance`.
pub fn throw_lux(t: &Throw, point: [f32; 3], normal: [f32; 3], front_only: bool) -> f32 {
    if !t.on {
        return 0.0;
    }
    let Some(([u, v], depth)) = throw_uv(t, point) else {
        return 0.0;
    };
    if !((0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v)) || depth <= 1e-6 {
        return 0.0;
    }
    let l = maths::sub(t.eye, point);
    let dist = maths::length(l);
    let (Some(l), Some(n)) = (maths::normalize(l), maths::normalize(normal)) else {
        return 0.0;
    };
    let facing = maths::dot(n, l);
    let incidence = if front_only { facing } else { facing.abs() };
    if incidence <= 0.0 {
        return 0.0;
    }
    let off_axis = (depth / dist).max(1e-3);
    let e = t.lux_at_one_metre / (depth * depth) * incidence / off_axis;
    if e.is_finite() { e } else { 0.0 }
}

/// The white point a view's Picture mode takes by itself (nits shown as
/// full white): the brightest of `nits` (each projector's on the screen
/// it is aimed at), never under 1. A room with none: 100.
pub fn auto_white(nits: &[f32]) -> f32 {
    let most = nits
        .iter()
        .copied()
        .filter(|v| v.is_finite() && *v > 0.0)
        .fold(0.0f32, f32::max);
    if most > 0.0 { most.max(1.0) } else { 100.0 }
}

/// Which side of a surface light counts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Facing {
    /// Only its front (a front-projection screen, whose normal is its front).
    Front,
    /// Either side (a rear-projection screen; a venue's walls, whose normals
    /// may point either way).
    Both,
}

/// The light a projector puts on a wall one metre away, square to the lens
/// (lux): `lumens / (w1 * h1)`.
pub fn lux_at_one_metre(p: &Projector, aspect: f32) -> f64 {
    let (w, h) = p.at_one_metre(aspect);
    f64::from(p.lumens()) / (f64::from(w) * f64::from(h))
}

/// The two cosines of the model at `point` with surface `normal`: the angle
/// the light comes in at (`|n·l|`, from either side) and the angle off the
/// lens axis (`a·(-l)`). None at the lens itself or behind it.
pub fn cosines(p: &Projector, point: [f32; 3], normal: [f32; 3]) -> Option<(f32, f32)> {
    let l = geometry::sub(p.position(), point);
    let len = geometry::dot(l, l).sqrt();
    if !(len.is_finite() && len > 1e-6) {
        return None;
    }
    let l = [l[0] / len, l[1] / len, l[2] / len];
    let n = geometry::normalize(normal);
    let incidence = geometry::dot(n, l).abs();
    let off_axis = -geometry::dot(p.axis(), l);
    (incidence > 1e-6 && off_axis > 1e-6).then_some((incidence, off_axis))
}

/// The light from projector `p` (picture `aspect` wide per one high) on
/// `point` (lux), the surface there having `normal` (None: as on a wall
/// square to the lens) and lit on `facing`. 0 outside its picture, behind
/// it, or on a surface's unlit side. Nothing in the way is looked for here
/// (`lit_by` does).
pub fn illuminance(
    p: &Projector,
    aspect: f32,
    point: [f32; 3],
    normal: Option<[f32; 3]>,
    facing: Facing,
) -> f32 {
    let Some((_, depth)) = p.in_picture(point, aspect) else {
        return 0.0;
    };
    if !p.reaches(point, aspect) {
        return 0.0;
    }
    let depth = f64::from(depth);
    let square = lux_at_one_metre(p, aspect) / (depth * depth);
    let e = match normal {
        None => square,
        Some(n) => {
            if facing == Facing::Front {
                let l = geometry::sub(p.position(), point);
                if geometry::dot(geometry::normalize(n), l) <= 0.0 {
                    return 0.0;
                }
            }
            match cosines(p, point, n) {
                Some((ci, ca)) => square * f64::from(ci) / f64::from(ca.max(1e-3)),
                None => 0.0,
            }
        }
    };
    if e.is_finite() { e as f32 } else { 0.0 }
}

/// How bright a surface lit with `lux` looks (nits, candela per square
/// metre), its `gain` 1 for a matt white screen.
pub fn luminance(lux: f32, gain: f32) -> f32 {
    let g = if gain.is_finite() {
        gain.clamp(0.0, 10.0)
    } else {
        1.0
    };
    lux * g / std::f32::consts::PI
}

/// What lights `point` (the surface there with `normal`, lit on `facing`):
/// each projector whose picture reaches it with nothing in the way, and its
/// lux. `projectors` pairs each with its picture's aspect; `blocks` says
/// which of `scene`'s parts can be in the way (the venue and the screens,
/// not the projectors' own bodies). A ray test per projector: off the main
/// thread for a big venue.
pub fn lit_by(
    projectors: &[(&Projector, f32)],
    scene: &Scene,
    blocks: &dyn Fn(usize, &Part) -> bool,
    point: [f32; 3],
    normal: Option<[f32; 3]>,
    facing: Facing,
) -> Vec<(ProjectorId, f32)> {
    let mut out = Vec::new();
    for (p, aspect) in projectors {
        if !p.visible {
            continue;
        }
        let lux = illuminance(p, *aspect, point, normal, facing);
        if lux <= 0.0 {
            continue;
        }
        let from = p.position();
        let d = geometry::sub(point, from);
        let dist = geometry::dot(d, d).sqrt();
        let ray = Ray {
            origin: from,
            dir: geometry::normalize(d),
        };
        // In the way: something met more than a couple of centimetres short
        // of the point (the point's own surface is met at the point).
        let shadowed = render3d::pick(scene, &ray, blocks)
            .is_some_and(|h| h.distance < dist - (0.02 + dist * 1e-4));
        if !shadowed {
            out.push((p.id, lux));
        }
    }
    out
}

#[cfg(test)]
#[path = "light_tests.rs"]
mod tests;
