//! The screen cue ("Move a screen", mapping plan 4.2): takes a 3D stage screen
//! from where it is to a place over a time. Where screens are is the engine's
//! (`Engine::screen_at`), never written into `Show::stage3d`: moving one
//! changes no scene key and sends no show over a network.
//!
//! A new kind, not a field on the fade cue: an older NewQ reads a kind it
//! doesn't know as `CueKind::Unknown`, keeps it and never plays it
//! (`show::compat`), where a new field on a fade cue would be ignored and the
//! fade played as something else.

use super::{CueId, CueKind, Curve, Show};
use crate::stage3d::{Placement, SurfaceId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// What a screen cue does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScreenCue {
    /// The screen it moves (None: none chosen; it does nothing).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen: Option<SurfaceId>,
    /// Where to; what isn't given is left where it is.
    #[serde(skip_serializing_if = "ScreenTarget::is_empty")]
    pub to: ScreenTarget,
    /// Seconds; 0 is a jump.
    #[serde(skip_serializing_if = "is_zero")]
    pub duration: f64,
    #[serde(skip_serializing_if = "is_s_curve")]
    pub curve: Curve,
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

fn is_s_curve(c: &Curve) -> bool {
    *c == Curve::SCurve
}

impl Default for ScreenCue {
    fn default() -> ScreenCue {
        ScreenCue {
            screen: None,
            to: ScreenTarget::default(),
            duration: 0.0,
            curve: Curve::SCurve,
        }
    }
}

/// The place a screen cue takes a screen to, number by number: each of them
/// optional, and one not given is left as it is. Metres and degrees, in the
/// stage's frame, as `Placement` has them. A turn keeps counting (720 is
/// twice round), so a revolve is one number.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScreenTarget {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub z: Option<f32>,
    /// Turn (about the up axis), as `Placement::yaw`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yaw: Option<f32>,
    /// Tilt, as `Placement::pitch`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pitch: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roll: Option<f32>,
}

/// The six numbers of a placement, in `ScreenTarget`'s order: x, y, z, turn,
/// tilt, roll.
pub const AXES: usize = 6;

/// The words for each of the six numbers, as the inspector shows them.
pub const AXIS_WORDS: [&str; AXES] = [
    "Across (X)",
    "Up (Y)",
    "Towards the audience (Z)",
    "Turn",
    "Tilt",
    "Roll",
];

/// A placement's six numbers (`AXES`' order).
pub fn axes_of(p: &Placement) -> [f32; AXES] {
    [
        p.position[0],
        p.position[1],
        p.position[2],
        p.yaw,
        p.pitch,
        p.roll,
    ]
}

/// The placement of six numbers (`AXES`' order).
pub fn placement_of(v: [f32; AXES]) -> Placement {
    Placement {
        position: [v[0], v[1], v[2]],
        yaw: v[3],
        pitch: v[4],
        roll: v[5],
    }
}

impl ScreenTarget {
    pub fn is_empty(&self) -> bool {
        *self == ScreenTarget::default()
    }

    /// Every number given: exactly `p`.
    pub fn whole(p: &Placement) -> ScreenTarget {
        ScreenTarget::from_axes(axes_of(p).map(Some))
    }

    /// Its numbers in `AXES`' order.
    pub fn axes(&self) -> [Option<f32>; AXES] {
        [self.x, self.y, self.z, self.yaw, self.pitch, self.roll]
    }

    pub fn from_axes(v: [Option<f32>; AXES]) -> ScreenTarget {
        ScreenTarget {
            x: v[0],
            y: v[1],
            z: v[2],
            yaw: v[3],
            pitch: v[4],
            roll: v[5],
        }
    }

    /// Number `i` (`AXES`' order).
    pub fn axis_mut(&mut self, i: usize) -> Option<&mut Option<f32>> {
        match i {
            0 => Some(&mut self.x),
            1 => Some(&mut self.y),
            2 => Some(&mut self.z),
            3 => Some(&mut self.yaw),
            4 => Some(&mut self.pitch),
            5 => Some(&mut self.roll),
            _ => None,
        }
    }

    /// Where a screen at `from` is once this is done: the numbers given (a
    /// number that isn't one, from a hand-edited file, is left), the rest as
    /// they were.
    pub fn apply(&self, from: &Placement) -> Placement {
        let mut v = axes_of(from);
        for (i, to) in self.axes().into_iter().enumerate() {
            if let Some(t) = to.filter(|t| t.is_finite()) {
                v[i] = t;
            }
        }
        placement_of(v)
    }
}

impl Show {
    /// The screens a screen cue names (anywhere in the show: every list, a
    /// timeline's clips, armed or not, so a control cue arming one at a GO
    /// changes nothing a lens map is made from). Worked out once for many
    /// questions; `movable` asks for one.
    pub fn movable_screens(&self) -> BTreeSet<SurfaceId> {
        self.cues
            .iter()
            .filter_map(|c| match &c.kind {
                CueKind::Screen(s) => s.screen,
                _ => None,
            })
            .collect()
    }

    /// The places screen `surface`'s cues take it to, in the show's order,
    /// each from where the one before left it (the first from its home):
    /// the *Put at* menu, and what a cue's place is shown from.
    pub fn screen_places(&self, surface: SurfaceId) -> Vec<(CueId, Placement)> {
        let Some(home) = self.stage3d.surface(surface).map(|s| s.placement) else {
            return Vec::new();
        };
        let mut at = home;
        let mut out = Vec::new();
        for c in &self.cues {
            if let CueKind::Screen(s) = &c.kind
                && s.screen == Some(surface)
            {
                at = s.to.apply(&at);
                out.push((c.id, at));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::show::{Cue, CueType};
    use serde_json::json;

    /// A show with two screens and a screen cue moving the first.
    fn show_with_cue() -> (Show, SurfaceId, SurfaceId, CueId) {
        let mut show = Show::default();
        let a = show.add_surface(None);
        let b = show.add_surface(None);
        let mut c = show.make_cue(CueType::Screen, None, 0);
        if let CueKind::Screen(s) = &mut c.kind {
            assert_eq!(s.screen, Some(a), "the first screen is chosen");
            s.to.x = Some(2.0);
            s.to.yaw = Some(90.0);
        }
        let id = c.id;
        show.cues.push(c);
        (show, a, b, id)
    }

    #[test]
    fn a_screen_cue_is_written_as_one_kind_with_nothing_at_its_defaults() {
        let mut show = Show::default();
        let c = show.make_cue(CueType::Screen, None, 0);
        // No screen on the stage: none chosen; a new cue takes 5 s.
        let v = serde_json::to_value(&c.kind).unwrap();
        assert_eq!(v, json!({"Screen": {"duration": 5.0}}));
        let empty = serde_json::to_value(CueKind::Screen(ScreenCue::default())).unwrap();
        assert_eq!(empty, json!({"Screen": {}}), "nothing at its defaults");
        // And read back, the same.
        let (show, ..) = show_with_cue();
        let c = &show.cues[0];
        let v = serde_json::to_value(c).unwrap();
        let back: Cue = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(back.kind.cue_type(), CueType::Screen);
        assert_eq!(serde_json::to_value(&back).unwrap(), v);
        // One key, as every cue kind is written: the shape a NewQ that
        // doesn't know the kind reads as `CueKind::Unknown` and keeps.
        assert_eq!(v["kind"].as_object().unwrap().len(), 1);
        assert_eq!(crate::show::compat::kind_name(&v["kind"]), "Screen");
    }

    #[test]
    fn a_screen_cue_whose_settings_it_cant_read_is_kept_and_never_played() {
        // As a later NewQ might write it: a curve this one doesn't know.
        let (show, a, ..) = show_with_cue();
        let mut v = serde_json::to_value(&show).unwrap();
        v["cues"][0]["kind"]["Screen"]["curve"] = json!("Bounce");
        let mut later: Show = serde_json::from_value(v.clone()).unwrap();
        let c = &later.cues[0];
        assert!(!c.kind.is_known());
        assert_eq!(
            c.kind.unknown_note().unwrap(),
            "Settings from a newer NewQ: can't play here (a Screen cue)"
        );
        assert_eq!(serde_json::to_value(&later).unwrap(), v, "kept as written");
        // It moves nothing, and the screen isn't movable here.
        let id = c.id;
        assert!(!later.movable(a));
        let mut e = crate::engine::Engine::for_tests();
        e.fire(&later, id);
        e.tick_shared(&later, 1.0);
        e.tick_shared(&later, 9.0);
        assert!(e.active.is_empty());
        assert_eq!(e.screen_at(a), None);
        assert!(e.moved_screens().is_empty());
        // A kind this NewQ doesn't know at all (as 0.27 reads this one): the
        // same, kept and never played.
        v["cues"][0]["kind"] = json!({"Hover": {"screen": a.0, "height": 2.0}});
        later = serde_json::from_value(v.clone()).unwrap();
        assert!(!later.cues[0].kind.is_known());
        assert_eq!(serde_json::to_value(&later).unwrap(), v);
    }

    #[test]
    fn a_screen_is_movable_when_a_screen_cue_names_it_armed_or_not() {
        let (mut show, a, b, id) = show_with_cue();
        assert!(show.movable(a) && !show.movable(b));
        assert_eq!(show.movable_screens(), BTreeSet::from([a]));
        // Disarmed (a control cue does that at a GO): still movable, so
        // nothing a lens map is made from changes.
        show.cue_mut(id).unwrap().armed = false;
        assert!(show.movable(a));
        // A show with screens and no screen cue: none, and nothing of the
        // kind is written.
        let mut plain = Show::default();
        let s = plain.add_surface(None);
        assert!(!plain.movable(s) && plain.movable_screens().is_empty());
        let text = serde_json::to_string(&plain).unwrap();
        assert!(!text.contains("\"Screen\""), "{text}");
    }

    #[test]
    fn the_places_a_screens_cues_take_it_follow_one_another() {
        let (mut show, a, _, first) = show_with_cue();
        show.stage3d.surface_mut(a).unwrap().placement.position = [0.0, 1.0, -3.0];
        let mut c = show.make_cue(CueType::Screen, None, 1);
        if let CueKind::Screen(s) = &mut c.kind {
            s.to.z = Some(0.0);
        }
        let second = c.id;
        show.cues.push(c);
        let places = show.screen_places(a);
        assert_eq!(places.len(), 2);
        assert_eq!(places[0].0, first);
        assert_eq!(places[0].1.position, [2.0, 1.0, -3.0]);
        assert_eq!(places[0].1.yaw, 90.0);
        assert_eq!(places[1].0, second);
        assert_eq!(
            places[1].1.position,
            [2.0, 1.0, 0.0],
            "from where the first left it"
        );
        assert_eq!(places[1].1.yaw, 90.0);
        // A number that isn't one is left.
        let t = ScreenTarget {
            x: Some(f32::NAN),
            ..ScreenTarget::default()
        };
        assert_eq!(t.apply(&places[1].1), places[1].1);
    }

    #[test]
    fn a_screen_cue_pasted_into_another_show_chooses_its_screen_again() {
        let (show, a, _, id) = show_with_cue();
        let copied = show.copy_cues(&[id]);
        let screen = |s: &Show, id: CueId| match &s.cue(id).unwrap().kind {
            CueKind::Screen(c) => c.screen,
            _ => panic!("a screen cue"),
        };
        // Into the same show: the same screen.
        let mut same = show.clone();
        let pasted = same.paste_cues(&copied, None, 0, true, false);
        assert_eq!(screen(&same, pasted[0]), Some(a));
        // Into another, whose screen of that id is another screen: none.
        let mut other = Show::default();
        other.add_surface(None);
        let pasted = other.paste_cues(&copied, None, 0, false, false);
        assert_eq!(screen(&other, pasted[0]), None);
    }

    #[test]
    fn a_heartbeat_with_moved_screens_reads_leniently_and_says_nothing_with_none() {
        use crate::net::play::PlayState;
        let plain = serde_json::to_value(PlayState::default()).unwrap();
        assert!(plain.get("screens").is_none(), "{plain}");
        let mut v = plain.clone();
        v["screens"] = json!([
            {"screen": 4, "at": {"position": [1.0, 0.0, 0.0]},
             "moving": [{"axis": 0, "from": 0.0, "to": 2.0, "anchor": 10.0,
                         "duration": 4.0, "curve": "Bounce"},
                        {"axis": 9, "from": 0.0, "to": 1.0, "anchor": 10.0, "duration": 1.0},
                        "something newer"]},
            {"screen": "not one"},
        ]);
        let s: PlayState = serde_json::from_value(v).expect("the heartbeat reads");
        assert_eq!(s.screens.len(), 1, "the entry it can't read is left out");
        let m = &s.screens[0].moving;
        assert_eq!(m.len(), 2);
        assert_eq!(
            m[0].curve,
            Curve::SCurve,
            "a later curve reads as the S-curve"
        );
        // Older (a NewQ before round 3): no field, nothing moved.
        let old: PlayState = serde_json::from_value(plain).unwrap();
        assert!(old.screens.is_empty());
    }
}
