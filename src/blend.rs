//! Edge blending: the fade at the edge of a projector's picture, so that where
//! two projectors overlap the light of the two adds up to one even picture.
//!
//! A projector's light is its signal raised to its gamma, so to give a pixel
//! a share `L` of its light the signal is multiplied by `L^(1/gamma)`. The
//! shares across an overlap go 0 to 1 on one projector and 1 to 0 on the
//! other along a smoothstep, which add up to 1 at every point. The shader in
//! `gpu.rs` multiplies by the same number (this module is the reference it is
//! tested against).

use crate::show::EdgeBlend;

/// The share of its light a projector gives at `t` of the way through a fade
/// that goes from nothing (0) to everything (1).
pub fn share(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// What the signal is multiplied by for that share, for a projector of `gamma`.
pub fn signal(t: f32, gamma: f32) -> f32 {
    share(t).powf(1.0 / gamma.max(1.0))
}

/// The multiplier at a point `(x, y)` of the display (0 to 1 across and down).
pub fn mask(b: &EdgeBlend, x: f32, y: f32) -> f32 {
    let edge = |pos: f32, width: f32| {
        if width <= 0.0 {
            1.0
        } else {
            signal(pos / width, b.gamma)
        }
    };
    edge(x, b.left) * edge(1.0 - x, b.right) * edge(y, b.top) * edge(1.0 - y, b.bottom)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shares_of_two_overlapping_projectors_add_to_one() {
        for i in 0..=20 {
            let t = i as f32 / 20.0;
            let sum = share(t) + share(1.0 - t);
            assert!((sum - 1.0).abs() < 1e-6, "t={t}: {sum}");
        }
    }

    #[test]
    fn the_light_adds_up_once_the_projectors_gamma_is_allowed_for() {
        for gamma in [1.8f32, 2.2, 2.6] {
            for i in 0..=20 {
                let t = i as f32 / 20.0;
                // The same signal on both: each makes its share of the light.
                let light = signal(t, gamma).powf(gamma) + signal(1.0 - t, gamma).powf(gamma);
                assert!((light - 1.0).abs() < 1e-4, "gamma {gamma} t={t}: {light}");
            }
        }
    }

    #[test]
    fn it_fades_from_black_at_the_edge_to_nothing_at_the_inside() {
        let b = EdgeBlend {
            left: 0.2,
            ..EdgeBlend::default()
        };
        assert_eq!(mask(&b, 0.0, 0.5), 0.0);
        assert!(mask(&b, 0.1, 0.5) > 0.0 && mask(&b, 0.1, 0.5) < 1.0);
        assert_eq!(mask(&b, 0.2, 0.5), 1.0);
        assert_eq!(mask(&b, 0.9, 0.5), 1.0);
        // The corner of two faded edges is darker than either alone.
        let c = EdgeBlend {
            left: 0.2,
            top: 0.2,
            ..EdgeBlend::default()
        };
        assert!(mask(&c, 0.1, 0.1) < mask(&c, 0.1, 0.9));
        assert!(mask(&c, 0.1, 0.1) < mask(&c, 0.9, 0.1));
        // No blend at all changes nothing anywhere.
        let off = EdgeBlend::default();
        assert_eq!(mask(&off, 0.0, 0.0), 1.0);
    }
}
