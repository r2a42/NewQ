//! The 3D stage part's leg with a mapped projector (mapping plan, round 2):
//! does a mapped display cost the displays on this computer?
//!
//! The part's Short lap is shown on a canvas too (`also_on`: a display with
//! no monitor that the screens show, as a mapped show has it), from the
//! first leg on, so every leg draws the same pictures. In this leg the
//! last ticked monitor's display becomes a mapped projector's
//! (`Projector::mapped`): its window shows the screens as that projector
//! sees them, through a lens map baked on the stage thread, the canvas's
//! pictures drawn every frame as output work, never governed. It is
//! judged against the baseline by the same line as the other 3D legs
//! (`stage::judge`); here are what it is made of (`setup`) and its own
//! findings (`findings`): a card that won't make a lens map, or a leg in
//! which none was made.
//!
//! Left out, in words, across a show network (turning a projector mapped
//! mid-test is a show edit, which would send the show to every computer
//! again), with no monitor ticked (a mapped picture is drawn for a window),
//! and when the projector can't be mapped or reaches no screen.

use super::analysis::{Finding, Severity};
use super::stage::{Leg, LegResult};
use crate::show::{OutputId, Show};
use crate::stage3d::projector::LensPreset;
use crate::stage3d::{Projector, ProjectorId};

/// The canvas's name in the test's show.
pub const CANVAS_NAME: &str = "Canvas (test)";
/// A projector added for the leg (none was on the display): its name.
pub const PROJECTOR_NAME: &str = "Mapped (test)";

/// The leg's projector, as it is in its leg and out of it.
#[derive(Clone, Debug, PartialEq)]
pub struct MappedLeg {
    pub projector: ProjectorId,
    /// The display it is mapped onto (the last ticked monitor's), and its
    /// name.
    pub display: OutputId,
    pub display_name: String,
    /// The canvas the lap is shown on, which the screens show.
    pub canvas: OutputId,
    pub before: Projector,
    pub during: Projector,
}

impl MappedLeg {
    /// Put the projector as the leg wants it (`on`: in its leg) in `show`.
    /// Whether anything changed.
    pub fn apply(&self, show: &mut Show, on: bool) -> bool {
        let want = if on { &self.during } else { &self.before };
        match show.projector_mut(self.projector) {
            Some(p) if p != want => {
                *p = want.clone();
                true
            }
            _ => false,
        }
    }
}

/// Make the mapped leg ready in the part's `show` (its cues made, its
/// displays the test's): the canvas the lap is also on, the screens fed
/// from it, the projector on the last ticked monitor's display (one added,
/// hidden but in its leg, if none is). None, with the reason in `notes`,
/// when there is to be no such leg. `mirror` is false across a show
/// network.
pub fn setup(show: &mut Show, mirror: bool, notes: &mut Vec<String>) -> Option<MappedLeg> {
    if !mirror {
        notes.push(
            "Across a show network the 3D stage part has no leg with a mapped projector: turning a projector mapped during the test would send the show to every computer again"
                .into(),
        );
        return None;
    }
    let pictures: Vec<OutputId> = show
        .outputs
        .iter()
        .filter(|o| o.enabled && !o.is_sub_display() && o.part_of.is_none())
        .map(|o| o.id)
        .collect();
    let local: Vec<OutputId> = show
        .outputs
        .iter()
        .filter(|o| o.computer.is_none() && o.monitor.is_some() && !o.is_sub_display())
        .map(|o| o.id)
        .collect();
    let (Some(&first), Some(&display)) = (pictures.first(), local.last()) else {
        notes.push(
            "With no monitor ticked the 3D stage part has no leg with a mapped projector: a mapped picture is drawn for a display's window"
                .into(),
        );
        return None;
    };
    // The lap on a canvas too.
    let canvas = show.add_canvas();
    if let Some(o) = show.outputs.iter_mut().find(|o| o.id == canvas) {
        o.name = CANVAS_NAME.into();
    }
    for c in &mut show.cues {
        let Some(m) = c.kind.media_mut() else {
            continue;
        };
        if m.output.is_none_or(|o| o == first) && !m.also_on.contains(&canvas) {
            m.also_on.push(canvas);
        }
    }
    // The screens that showed one of the test's displays show the canvas
    // (the same picture), so none shows the display to be mapped.
    for s in &mut show.stage3d.surfaces {
        if let crate::stage3d::Feed::Display { output, .. } = &mut s.feed
            && *output != canvas
        {
            *output = canvas;
        }
    }
    let id = match show
        .stage3d
        .projectors
        .iter()
        .find(|p| p.display == Some(display))
    {
        Some(p) => p.id,
        None => {
            let screen = show.screens_showing(canvas).first().map(|s| (*s).clone());
            let id = show.add_projector();
            let p = show.projector_mut(id)?;
            p.name = PROJECTOR_NAME.into();
            p.visible = false;
            p.lens = LensPreset::StandardZoom.lens(None);
            p.lumens = 10_000.0;
            p.display = Some(display);
            if let Some(s) = screen
                && let Some(place) = p.fit_to(&s, 16.0 / 9.0)
            {
                p.placement = place;
            }
            id
        }
    };
    let before = show.projector(id)?.clone();
    let mut during = before.clone();
    during.mapped = true;
    during.visible = true;
    let display_name = show
        .output(display)
        .map_or_else(String::new, |o| o.name.clone());
    let leg = MappedLeg {
        projector: id,
        display,
        display_name,
        canvas,
        before,
        during,
    };
    // Tried on a copy: it must be mappable and see a screen.
    let mut probe = show.clone();
    leg.apply(&mut probe, true);
    let name = &leg.display_name;
    if probe.mapped_refused(id).is_some() || probe.mapped_feeds(id).used.is_empty() {
        notes.push(format!(
            "The 3D stage part has no leg with a mapped projector: the projector on {name} reaches no screen it could be mapped onto"
        ));
        return None;
    }
    notes.push(format!(
        "In the 3D stage part the lap is also shown on a canvas the screens show; in its last leg {name} is a mapped projector's display, showing the screens as {} sees them",
        leg.during.name
    ));
    Some(leg)
}

/// What the leg says besides the comparison with the baseline (pure): a
/// card that refused the lens map, or a leg in which none was made.
pub fn findings(results: &[LegResult]) -> Vec<Finding> {
    let mut out = Vec::new();
    let legs: Vec<&LegResult> = results
        .iter()
        .filter(|r| r.leg == Some(Leg::Mapped))
        .collect();
    if legs.is_empty() {
        return out;
    }
    let refused: u64 = legs.iter().map(|r| r.maps.1).sum();
    let baked: u64 = legs.iter().map(|r| r.maps.0).sum();
    if refused > 0 {
        out.push(Finding {
            severity: Severity::Problem,
            text: "This graphics card would not make a mapped projector's lens map: a mapped projector's display stays black on this computer.".into(),
        });
    } else if baked == 0 {
        out.push(Finding {
            severity: Severity::Note,
            text: "No lens map was made in the leg with a mapped projector, so that leg says nothing about mapping here.".into(),
        });
    }
    out
}

/// The Problem sentence for a mapped leg that missed more than the
/// baseline by the line.
pub fn costs_words(share: &str, base: &str) -> String {
    format!(
        "With a mapped projector showing the lap the displays missed {share} of their pictures; with previews off, {base}. On this computer a mapped projector costs the show: its picture is drawn for every frame and never gives way. Try a smaller canvas, or fewer canvases a projector."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview_pace::{Shown, WINDOWS};

    fn leg(l: Leg, maps: (u64, u64)) -> LegResult {
        let mut shown = [Shown::default(); WINDOWS];
        shown[0] = Shown {
            pictures: 2400,
            late: 2,
        };
        LegResult {
            leg: Some(l),
            round: 1,
            secs: 41.0,
            shown,
            maps,
            ..Default::default()
        }
    }

    #[test]
    fn a_refused_lens_map_is_a_problem_and_none_made_is_said() {
        let base = leg(Leg::Baseline, (0, 0));
        assert!(findings(&[base.clone(), leg(Leg::Mapped, (1, 0))]).is_empty());
        let f = findings(&[base.clone(), leg(Leg::Mapped, (0, 1))]);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Problem);
        assert!(f[0].text.contains("stays black"));
        let f = findings(&[base.clone(), leg(Leg::Mapped, (0, 0))]);
        assert_eq!(f[0].severity, Severity::Note);
        // No mapped leg: nothing.
        assert!(findings(&[base]).is_empty());
    }

    /// The lap is on the canvas too, on one computer: every picture the
    /// lap sends to the first display is also on the canvas (so the mapped
    /// leg doesn't map a black canvas).
    #[test]
    fn the_lap_is_shown_on_the_canvas_too() {
        use crate::testmode::plan::{DisplayChoice, SoundChoice, build_stage};
        let picks: Vec<DisplayChoice> = (1..=2)
            .map(|i| DisplayChoice {
                computer: None,
                key: format!("M{i}"),
                reference: crate::show::DisplayRef {
                    name: format!("M{i}"),
                    index: i,
                    hardware: None,
                },
                twins: 1,
                label: format!("M{i}"),
            })
            .collect();
        let b = build_stage(
            &Show::default(),
            &std::collections::HashMap::new(),
            &picks,
            &SoundChoice::default(),
            true,
        );
        let m = b
            .stage
            .as_ref()
            .and_then(|s| s.mapped.clone())
            .expect("a mapped leg");
        let first = b.show.outputs[0].id;
        // Every picture the lap sends to the first display goes to the
        // canvas too.
        let sent: Vec<_> = b
            .show
            .cues
            .iter()
            .filter_map(|c| c.kind.media().map(|x| (c, x)))
            .filter(|(_, x)| x.output.is_none_or(|o| o == first))
            .collect();
        assert!(!sent.is_empty());
        assert!(sent.iter().all(|(_, x)| x.also_on.contains(&m.canvas)));
        // (The engine plays a cue on its `also_on` displays as on its own:
        // its tests; the lap's media aren't here without the downloads.)
        assert!(b.show.is_canvas(m.canvas));
    }
}
