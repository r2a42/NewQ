//! Where the 3D stage's screens are (mapping plan 4.2): run-time state, as a
//! playing cue's opacity is, never written into `Show::stage3d` (that would
//! make the scene and every lens map again, and resend the show on a show
//! network). A screen cue moves a screen; where it is outlives the cue.

use super::Engine;
use crate::net::play::Command;
use crate::show::{Cue, ScreenCue, Show};
use crate::stage3d::{Placement, SurfaceId};
use std::collections::BTreeMap;

/// How long a screen cue takes (a time that isn't one, or less than 0, is a
/// jump).
pub fn length(s: &ScreenCue) -> f64 {
    if s.duration.is_finite() {
        s.duration.max(0.0)
    } else {
        0.0
    }
}

/// Every moved screen: where it is now. A screen not here is at home (its
/// placement in the show).
#[derive(Default)]
pub struct Screens {
    at: BTreeMap<SurfaceId, Placement>,
}

impl Engine {
    /// Where screen `id` is now, if a cue (or *Put at*) moved it; None: at
    /// home, its placement in the show.
    pub fn screen_at(&self, id: SurfaceId) -> Option<Placement> {
        self.screens.at.get(&id).copied()
    }

    /// Every moved screen and where it is now, by id: what the outputs, the
    /// 3D views and the light draw them at (`StageLive::send`).
    pub fn moved_screens(&self) -> Vec<(SurfaceId, Placement)> {
        self.screens.at.iter().map(|(k, v)| (*k, *v)).collect()
    }

    /// *Put at*: screen `screen` is at `at` at once (a jump, no ramp). An
    /// input from outside the engine: on a show network's primary it is a
    /// command, done at the same shared time everywhere.
    pub fn put_screen(&mut self, show: &Show, screen: SurfaceId, at: Placement) {
        if self.gated(|_| Command::PutScreen { screen, at }) {
            return;
        }
        let _ = show;
        self.screens.at.insert(screen, at);
    }

    /// Screen cue `cue` (`s`, playing as `instance`) has started: its move
    /// begins from where the screen is.
    pub(super) fn screen_cue(&mut self, show: &Show, cue: &Cue, s: &ScreenCue, instance: u64) {
        let _ = (cue, instance);
        let Some(id) = s.screen else {
            return;
        };
        let from = self
            .screen_at(id)
            .or_else(|| show.stage3d.surface(id).map(|x| x.placement));
        if let Some(from) = from {
            self.screens.at.insert(id, s.to.apply(&from));
        }
    }

    /// Each tick: a screen the show no longer has, or that no cue moves any
    /// more, is at home (a lens map has it there).
    pub(super) fn screens_tick(&mut self, show: &Show) {
        if self.screens.at.is_empty() {
            return;
        }
        let movable = show.movable_screens();
        self.screens
            .at
            .retain(|id, _| movable.contains(id) && show.stage3d.surface(*id).is_some());
    }

    /// A show was opened: every screen is at home.
    pub(super) fn screens_home(&mut self) {
        self.screens = Screens::default();
    }
}
