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
use crate::render3d::{self, Part, Ray, Scene};

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
