//! A picture on several displays (`MediaCue::also_on`, plan P9): one cue plays
//! once (one decoder, one set of levels, effects and keyframes) and is drawn
//! on its home display and on each further one. What happens on one display
//! leaves the others alone (the owner's ruling E2): a picture entering on the
//! same layer of one of them replaces the cue there only (a cut, or a fade out
//! of that display alone for a crossfade), and the cue goes on, at full
//! opacity, on the rest. It ends when it has been replaced on its last
//! display, or by itself. A cue on one display is replaced as it always was.
//!
//! Its sound is one voice: it plays while the picture is fully on some
//! display, and fades with the last display's crossfade.

use super::{ActiveCue, ClockSteps, Param};
use crate::show::{Curve, MIN_DB, OutputId};

/// A picture's state on one of its displays.
#[derive(Clone, Debug)]
pub(super) struct Screen {
    pub output: OutputId,
    /// Leaving this display alone (a crossfade here): its level there, 1 to 0.
    pub exit: Option<Param>,
    /// Replaced on this display: no longer drawn on it.
    pub gone: bool,
}

impl Screen {
    pub fn on(output: OutputId) -> Screen {
        Screen {
            output,
            exit: None,
            gone: false,
        }
    }
}

/// A playing picture's screens kept in step with the displays its cue is on
/// now (`want`, from `MediaCue::displays`), so an edit applies live: a display
/// added shows it at once, one taken away drops it. A display it has been
/// replaced on stays gone while it is still listed.
pub(super) fn sync(screens: &mut Vec<Screen>, want: &[OutputId]) {
    if screens.len() == want.len() && screens.iter().zip(want).all(|(s, o)| s.output == *o) {
        return;
    }
    screens.retain(|s| want.contains(&s.output));
    for o in want {
        if !screens.iter().any(|s| s.output == *o) {
            screens.push(Screen::on(*o));
        }
    }
}

impl ActiveCue {
    /// Its level on the display `o` (1 fully on, less while it leaves that
    /// display alone), or None if it isn't drawn there.
    pub(super) fn level_on(&self, o: OutputId) -> Option<f32> {
        if self.screens.is_empty() {
            return (self.output() == Some(o)).then_some(1.0);
        }
        self.screens
            .iter()
            .find(|s| s.output == o && !s.gone)
            .map(|s| s.exit.as_ref().map_or(1.0, |p| p.value))
    }

    /// The displays it is drawn on now (leaving some of them or not).
    pub(super) fn showing(&self) -> Vec<OutputId> {
        if self.screens.is_empty() {
            return self.output().into_iter().collect();
        }
        self.screens
            .iter()
            .filter(|s| !s.gone)
            .map(|s| s.output)
            .collect()
    }

    /// The displays it is fully on (not gone, not leaving).
    fn full(&self) -> Vec<OutputId> {
        if self.screens.is_empty() {
            return self.output().into_iter().collect();
        }
        self.screens
            .iter()
            .filter(|s| !s.gone && s.exit.is_none())
            .map(|s| s.output)
            .collect()
    }

    /// Leaving some display on its own.
    fn leaving(&self) -> bool {
        self.screens.iter().any(|s| !s.gone && s.exit.is_some())
    }

    /// Something entering on the displays `on` cuts it from them (a cut, or a
    /// dip's reveal). False: it is on no display now and goes (as a cue on
    /// one display always did); true: it is still on some.
    pub(super) fn cut_from(&mut self, on: &[OutputId], clock: Option<(u64, f64)>) -> bool {
        let showing = self.showing();
        let hit = showing.iter().filter(|o| on.contains(o)).count();
        if hit == 0 {
            return true;
        }
        if hit == showing.len() {
            return false;
        }
        for s in &mut self.screens {
            if on.contains(&s.output) {
                s.gone = true;
                s.exit = None;
            }
        }
        self.leave_sound(Curve::Linear, clock);
        true
    }

    /// Something entering on the displays `on` crossfades over it there, over
    /// `time`: off all of its displays at once, it fades out and stops as a
    /// cue always did; off some of them, it fades out of those alone.
    pub(super) fn crossfade_from(
        &mut self,
        on: &[OutputId],
        time: f64,
        curve: Curve,
        clock: Option<(u64, f64)>,
    ) {
        let full = self.full();
        let hit: Vec<OutputId> = full.iter().copied().filter(|o| on.contains(o)).collect();
        if hit.is_empty() {
            return;
        }
        if hit.len() == full.len() && !self.leaving() {
            self.fade_out_and_stop(time, curve);
            self.opacity.follow(clock);
            self.volume.follow(clock);
            return;
        }
        for s in &mut self.screens {
            if hit.contains(&s.output) {
                let mut exit = Param::new(1.0);
                exit.ramp_to(0.0, time, curve);
                exit.follow(clock);
                // A crossfade of no time is a cut.
                s.gone = !exit.ramping();
                s.exit = exit.ramping().then_some(exit);
            }
        }
        self.leave_sound(curve, clock);
    }

    /// No longer fully on any display, only leaving some: its sound fades with
    /// the longest of those, and it goes when they are done.
    fn leave_sound(&mut self, curve: Curve, clock: Option<(u64, f64)>) {
        if !self.full().is_empty() || self.stopping {
            return;
        }
        let left = self
            .screens
            .iter()
            .filter(|s| !s.gone)
            .filter_map(|s| s.exit.as_ref().map(Param::left))
            .fold(0.0, f64::max);
        if self.hold {
            self.unfreeze(false);
        }
        self.volume.ramp_to(MIN_DB, left, curve);
        self.volume.follow(clock);
        let v = self.opacity.value;
        self.opacity.ramp_to(v, left, Curve::Linear);
        self.opacity.follow(clock);
        self.stopping = true;
    }

    /// Move its exits from the displays it is leaving alone (a timeline's waits
    /// while its clock stands, `Param::step`); one that is done is gone.
    pub(super) fn step_screens(&mut self, dt: f64, clocks: &ClockSteps) {
        for s in &mut self.screens {
            if let Some(p) = &mut s.exit {
                p.step(dt, clocks);
                if !p.ramping() {
                    s.exit = None;
                    s.gone = true;
                }
            }
        }
    }

    /// Replaced on every display it was on: it is over.
    pub(super) fn gone_everywhere(&self) -> bool {
        !self.screens.is_empty() && self.screens.iter().all(|s| s.gone)
    }
}

#[cfg(test)]
mod tests;
