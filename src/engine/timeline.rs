//! The engine's side of timelines: a run per timeline, with a clock of its own
//! (`TimelineRun`), and what GO, pause, stop and seek do to it. Hooks in
//! `engine.rs` call in here. (Stubs for now: nothing runs a timeline yet, so a
//! timeline group still plays as a "start all together" group always has.)

// Used once the run is built (the engine's run, then the editor).
#![allow(dead_code)]

use super::Engine;
use crate::show::{CueId, Show};
use crate::timeline::{PauseFreeze, TimelineKey, model};
use std::collections::HashMap;

/// Where a timeline's run is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RunState {
    /// The clock still, nothing playing.
    Stopped,
    /// Clips started held, waiting for all to be ready; the clock still.
    Cueing,
    Running,
    /// The clock still; `freeze` says what holds. `waits_for_go`: GO runs it on.
    Paused {
        freeze: PauseFreeze,
        waits_for_go: bool,
    },
}

/// A timeline's run, for the editor and the transport.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RunView {
    pub key: TimelineKey,
    pub state: RunState,
    pub pos: f64,
    pub next_pause: Option<f64>,
    pub length: Option<f64>,
    /// What GO will bring in or fade, for the read-out.
    pub on_go: Option<CueId>,
}

/// The runs of the timelines that have been touched.
#[derive(Default)]
pub struct Runs {
    views: HashMap<TimelineKey, RunView>,
}

impl Engine {
    /// A timeline's state; `Stopped` at 0 when it has never been touched.
    pub fn timeline(&mut self, show: &Show, key: TimelineKey) -> RunView {
        if let Some(v) = self.runs.views.get(&key) {
            return *v;
        }
        let clips = model::clips(show, key, &mut |c| self.cue_duration(show, c));
        RunView {
            key,
            state: RunState::Stopped,
            pos: 0.0,
            next_pause: None,
            length: model::length(&clips),
            on_go: None,
        }
    }

    /// Those not `Stopped`.
    pub fn timelines(&self) -> Vec<TimelineKey> {
        self.runs
            .views
            .values()
            .filter(|v| v.state != RunState::Stopped)
            .map(|v| v.key)
            .collect()
    }

    /// Run from where it is: Stopped/parked -> cue then run; Paused -> resume; Running -> no-op.
    pub fn timeline_run(&mut self, _show: &Show, _key: TimelineKey) {}

    /// Stop the clock. `clips` only matters for `PauseFreeze::Clips`. `live`:
    /// live inputs among the frozen clips freeze too (a camera holds its frame,
    /// live sound mutes).
    pub fn timeline_pause(
        &mut self,
        _show: &Show,
        _key: TimelineKey,
        _freeze: PauseFreeze,
        _clips: &[CueId],
        _live: bool,
        _waits_for_go: bool,
    ) {
    }

    /// Stop its clips (fade seconds; 0 = cut), keep the playhead where it is.
    pub fn timeline_stop(&mut self, _show: &Show, _key: TimelineKey, _fade: f64) {}

    /// Move the playhead. Keeps Running running; otherwise parks, with the picture
    /// (clips started held) when `show_picture`, without when not.
    pub fn timeline_seek(
        &mut self,
        _show: &Show,
        _key: TimelineKey,
        _pos: f64,
        _show_picture: bool,
    ) {
    }

    /// The clips of a run that are free-running, for the editor.
    pub fn timeline_detached(&self, _key: TimelineKey) -> Vec<CueId> {
        Vec::new()
    }

    /// The clips of a run waiting to fade in, for the editor.
    pub fn timeline_waiting(&self, _key: TimelineKey) -> Vec<CueId> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::show::CueType;

    #[test]
    fn an_untouched_timeline_is_stopped_at_its_start_with_its_length() {
        let mut show = Show::default();
        let g = show.make_cue(CueType::Group, None, 0);
        let gid = g.id;
        show.cues.push(g);
        let mut w = show.make_cue(CueType::Wait, None, 1);
        w.parent = Some(gid);
        w.pre_wait = 2.0;
        if let crate::show::CueKind::Wait(wc) = &mut w.kind {
            wc.duration = 3.0;
        }
        show.cues.push(w);
        let mut e = Engine::for_tests();
        let key = TimelineKey::Group(gid);
        let v = e.timeline(&show, key);
        assert_eq!(v.state, RunState::Stopped);
        assert_eq!(
            (v.pos, v.length, v.next_pause, v.on_go),
            (0.0, Some(5.0), None, None)
        );
        assert!(e.timelines().is_empty());
        // The stubs change nothing.
        e.timeline_run(&show, key);
        e.timeline_pause(&show, key, PauseFreeze::Everything, &[], true, true);
        e.timeline_seek(&show, key, 1.0, true);
        e.timeline_stop(&show, key, 0.0);
        assert!(e.timelines().is_empty() && !e.is_active());
        assert!(e.timeline_detached(key).is_empty() && e.timeline_waiting(key).is_empty());
    }
}
