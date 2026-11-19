//! The engine's side of computers playing together (`crate::net::play`).
//!
//! **The gate.** In a show network every operator input that changes playback
//! must happen at the same shared time on every computer. Rather than find and
//! change every caller (the transport, the cue list's menu, hotkeys, OSC, MIDI,
//! the web remote, clock triggers, timecode, Test Mode), the engine's own entry
//! points catch the call: on a primary (`NetMode::Primary`) a call from outside
//! the engine is noted as a `Command` and not done (`net_take` hands them to the
//! app, which times them and does them through `net_apply`); on a follower it is
//! dropped (its controls are locked; what plays there is the primary's). What
//! the engine does to itself while it ticks (a control cue, a group's children,
//! an auto-follow) is never caught: `depth` counts the engine's own calls, and
//! every computer does those itself. Alone (`NetMode::Off`) nothing is caught.
//!
//! **The heartbeat** (`net_state`) and **corrections** (`net_correct`): what
//! plays, compared by cue (not by instance, which differ between computers) and
//! by timeline.

use super::{ActiveKind, Engine, StartAt};
use crate::net::play::{Command, CueNow, PlayState, RunNow, RunWord};
use crate::show::{CueId, CueKind, Show};
use crate::timeline::model;
use crate::timeline::{PauseFreeze, TimelineKey};
use std::collections::{HashMap, HashSet};

/// What this engine is in a show network.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NetMode {
    /// Alone: everything as it always was.
    #[default]
    Off,
    /// Runs the show for other computers: inputs are caught as commands.
    Primary,
    /// Follows a primary: inputs are dropped, and nothing is sent out (send
    /// cues, timecode), nor a camera it hasn't got looked for.
    Follower,
}

/// What had been started before a call (`Engine::net_mark`).
pub(super) struct StartMark {
    instance: u64,
    pending: usize,
    runs: Vec<(TimelineKey, super::RunState, f64, Option<f64>)>,
}

#[derive(Default)]
pub struct NetGate {
    pub mode: NetMode,
    /// The engine's own calls under way (a tick, a command being done).
    depth: u32,
    taken: Vec<Command>,
}

/// Two playing pictures are a frame apart past this (seconds; a 60 Hz frame).
pub const FRAME: f64 = 1.0 / 60.0;
/// A cue started (or let go) this close to a heartbeat is not judged by it: the
/// two computers may be either side of that moment.
const SETTLING: f64 = 0.25;
/// Off by more than this, a playing file is jumped (new decoders) rather than
/// re-anchored (its decoders run to catch up).
const JUMP_PAST: f64 = 0.5;
/// A timeline's clock off by less than this is moved; by more, it is sought.
const RUN_NUDGE: f64 = 0.25;
/// A cue off from the primary's by no more than this is left alone (R21: a
/// heartbeat never moves a picture for jitter; the commands placed it).
pub const LEAVE_UNDER: f64 = 1.5 * FRAME;
/// A cue is put right only once it has been off at this many heartbeats in a
/// row (about a second)...
const STEADY_BEATS: u32 = 4;
/// ...by amounts within this of each other (a steady difference, not jitter).
const STEADY_SPREAD: f64 = FRAME;

/// A difference seen at one heartbeat, acted on when seen again at the next.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Doubt {
    Missing(CueId, usize),
    Extra(u64),
    Off(u64),
    Run(TimelineKey),
    Paused,
}

/// The differences seen at the last heartbeat (`Engine::net_correct`), and
/// what has been put right.
#[derive(Default, Debug)]
pub struct NetCorrection {
    seen: HashSet<Doubt>,
    /// A playing instance off from the primary's: the heartbeats in a row it
    /// has been, and the least and most it was off by (seconds).
    steady: HashMap<u64, (u32, f64, f64)>,
    /// Instances put right already: never again (R21: one correction, then
    /// it is said, not jumped at again).
    fixed: HashSet<u64>,
    /// Instances said to still differ after their one correction.
    said: HashSet<u64>,
    /// Cues started here part way because the primary played them and this
    /// computer didn't: the primary's instance (cue, nth live) and its anchor
    /// then. One that is gone again (it failed to start, or ended at once) is
    /// never started again for the same instance there: only for a new one
    /// (another anchor, or the cue let go there in between).
    started: HashMap<(CueId, usize), f64>,
    /// Those said to have gone again.
    gave_up: HashSet<(CueId, usize)>,
    /// The primary was paused at the last heartbeat: its anchors slide while
    /// it is (by the time it stands), so they are followed, not compared.
    was_paused: bool,
}

/// The primary's anchor of one instance of a cue, as heartbeats carry it: the
/// same instance within this (seconds; a seek there moves it by more).
const SAME_ANCHOR: f64 = LEAVE_UNDER;

impl NetCorrection {
    pub fn is_empty(&self) -> bool {
        self.seen.is_empty()
    }
}

impl Engine {
    /// Put the engine in a show network as `mode` (or alone). Leaving one
    /// gives up the shared clock, so `tick` alone is as it always was.
    pub fn net_set_mode(&mut self, mode: NetMode) {
        if self.net.mode == mode {
            return;
        }
        log::info!("show network: the engine is now {mode:?}");
        self.net.mode = mode;
        self.net.taken.clear();
        if mode == NetMode::Off {
            self.net_leave_shared();
        }
    }

    /// Between passes of the main loop no call of the engine's own is under
    /// way (a panic in one leaves the count up: this puts it back).
    pub fn net_settle(&mut self) {
        self.net.depth = 0;
    }

    /// The inputs caught since the last call (a primary's).
    pub fn net_take(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.net.taken)
    }

    /// Whether a call from outside is caught: noted (primary) or dropped
    /// (follower). Called first thing in each entry point.
    pub(super) fn gated(&mut self, cmd: impl FnOnce(&Engine) -> Command) -> bool {
        if self.net.depth > 0 {
            return false;
        }
        match self.net.mode {
            NetMode::Off => false,
            NetMode::Primary => {
                let c = cmd(self);
                self.net.taken.push(c);
                true
            }
            NetMode::Follower => {
                log::debug!("show network: an input on a follower is left to the primary");
                true
            }
        }
    }

    pub(super) fn enter(&mut self) {
        self.net.depth += 1;
    }

    pub(super) fn leave(&mut self) {
        self.net.depth = self.net.depth.saturating_sub(1);
    }

    /// A follower: send cues and timecode stay on the primary.
    pub(super) fn sends_here(&self) -> bool {
        self.net.mode != NetMode::Follower
    }

    /// A follower plays a camera cue only if the device is on this computer
    /// (an NDI source is looked for when it starts); otherwise nothing, quietly.
    pub(super) fn camera_missing(&mut self, cue: &crate::show::Cue) -> bool {
        let CueKind::Camera(c) = &cue.kind else {
            return false;
        };
        if self.net.mode != NetMode::Follower
            || c.device.starts_with("ndi:")
            || c.device == super::TEST_PATTERN_DEVICE
        {
            return false;
        }
        let here = self.cameras().contains(&c.device);
        if !here {
            log::debug!(
                "show network: camera {} isn't on this computer; cue {} shows nothing here",
                c.device,
                cue.number
            );
        }
        !here
    }

    /// The `nth` (by start) playing instance of `cue`, and back.
    pub(super) fn nth_of(&self, instance: u64) -> Option<(CueId, usize)> {
        let a = self.active.iter().find(|a| a.instance == instance)?;
        let nth = self
            .active
            .iter()
            .filter(|b| b.cue_id == a.cue_id && b.instance < instance)
            .count();
        Some((a.cue_id, nth))
    }

    fn instance_of(&self, cue: CueId, nth: usize) -> Option<u64> {
        let mut v: Vec<u64> = self
            .active
            .iter()
            .filter(|a| a.cue_id == cue)
            .map(|a| a.instance)
            .collect();
        v.sort_unstable();
        v.get(nth).copied()
    }

    /// The shared time of the last tick, while ticking by one.
    pub fn net_shared_time(&self) -> Option<f64> {
        self.sync.map(|s| s.now)
    }

    /// The shared clock was put back (or on) by `by` seconds (a follower's
    /// clock that was far off, `SharedNow::jumps`): every shared time
    /// the engine holds moves with it, so what plays carries on exactly where
    /// it is, neither jumping nor standing still.
    pub fn net_shift_shared(&mut self, by: f64) {
        if !by.is_finite() {
            return;
        }
        if let Some(s) = &mut self.sync {
            s.now += by;
        }
        for a in &mut self.active {
            if let Some(x) = &mut a.anchor {
                *x += by;
            }
            if let Some(x) = &mut a.sound_zero {
                *x += by;
            }
        }
        // Where the cues just ended did, for what follows them.
        for t in self.net_ended.values_mut() {
            *t += by;
        }
        self.runs.net_shift(by);
    }

    /// Back to ticking by `dt` alone: anchors are let go, so times are sums of
    /// ticks again from where they are.
    pub fn net_leave_shared(&mut self) {
        self.sync = None;
        self.net_ended.clear();
        for a in &mut self.active {
            a.anchor = None;
        }
        self.runs.net_unanchor();
    }

    /// Do a command (at the shared time of the last tick). `late`: it was due
    /// that long ago, so what it starts is placed as if it had been done then.
    pub fn net_apply(&mut self, show: &Show, cmd: &Command, late: f64) {
        self.enter();
        let mark = self.net_mark();
        self.apply_one(show, cmd);
        if late > 0.0
            && let Some(now) = self.net_shared_time()
        {
            self.net_backdate(&mark, now - late);
        }
        self.leave();
    }

    /// What has been started so far, so that what a call starts after it can
    /// be placed earlier (`net_backdate`).
    pub(super) fn net_mark(&self) -> StartMark {
        StartMark {
            instance: self.next_instance,
            pending: self.pending.len(),
            runs: self.runs.net_runs(),
        }
    }

    /// Whether what was started since `mark` has a sound that would begin
    /// now, were it placed `late` back: a cue playing with a voice, or one
    /// still to start whose pre-wait that would use up.
    pub(super) fn net_started_sound(&self, show: &Show, mark: &StartMark, late: f64) -> bool {
        self.active
            .iter()
            .filter(|a| a.instance >= mark.instance)
            .any(|a| a.voice().is_some())
            || self.pending.iter().skip(mark.pending).any(|p| {
                p.remaining <= late
                    && show
                        .cue(p.cue_id)
                        .is_some_and(|c| matches!(c.kind, CueKind::Audio(_) | CueKind::Video(_)))
            })
    }

    /// Ticking by a shared clock: what was started since `mark` (cues, the
    /// pre-waits of cues to come, timelines set going) began at shared time
    /// `due`, before this tick's: a late command, or the end of the cue it
    /// follows, which every computer reaches at the same shared time whatever
    /// frame it noticed it in. Nothing when `due` isn't before this tick.
    pub(super) fn net_backdate(&mut self, mark: &StartMark, due: f64) {
        let Some(now) = self.net_shared_time() else {
            return;
        };
        let late = now - due;
        if late.is_nan() || late <= 0.0 {
            return;
        }
        for a in self
            .active
            .iter_mut()
            .filter(|a| a.instance >= mark.instance)
        {
            if a.run.is_none() {
                a.anchor = Some(due - a.elapsed);
            }
        }
        for p in self.pending.iter_mut().skip(mark.pending) {
            if p.run.is_none() && p.reading.is_none() {
                p.remaining -= late;
            }
        }
        // A run set going by it began its clock that long ago.
        for (key, state, pos, _) in self.runs.net_runs() {
            let was = mark.runs.iter().find(|r| r.0 == key);
            let went = matches!(state, super::RunState::Running | super::RunState::Cueing)
                && was.is_none_or(|w| {
                    !matches!(w.1, super::RunState::Running | super::RunState::Cueing)
                        || (w.2 - pos).abs() > 1e-9
                });
            if went {
                self.runs.net_place(key, due - pos, None);
            }
        }
    }

    fn apply_one(&mut self, show: &Show, cmd: &Command) {
        let set_list = |e: &mut Engine, list: u32, playhead: Option<CueId>| {
            if e.active_list != list {
                e.switch_list(show, list);
            }
            e.playhead = playhead;
        };
        match cmd {
            Command::Go { list, playhead } => {
                set_list(self, *list, *playhead);
                self.go(show);
            }
            Command::GoPlayhead { list, playhead } => {
                set_list(self, *list, *playhead);
                self.go_playhead(show);
            }
            Command::GoList { list } => self.go_list(show, *list),
            Command::Fire { cue } => self.fire(show, *cue),
            Command::StopCue { cue, fade } => self.stop_cue(*cue, *fade),
            Command::FadeOutCue { cue } => self.fade_out_cue(show, *cue),
            Command::StopAll { fade } => self.stop_all(*fade),
            Command::Panic => self.panic(show),
            Command::ResetAll => self.reset_all(show),
            Command::SetPaused { paused } => self.set_paused(*paused),
            Command::StopInstance { cue, nth, fade } => {
                if let Some(i) = self.instance_of(*cue, *nth) {
                    self.stop_instance(i, *fade);
                }
            }
            Command::FadeOutInstance { cue, nth } => {
                if let Some(i) = self.instance_of(*cue, *nth) {
                    self.fade_out_instance(show, i);
                }
            }
            Command::Seek { cue, nth, to } => {
                if let Some(i) = self.instance_of(*cue, *nth)
                    && let Err(e) = self.seek(show, i, *to)
                {
                    log::debug!("show network: a seek couldn't be done here: {e}");
                }
            }
            Command::Control {
                action,
                target,
                from,
            } => self.control_target(show, *action, *target, from),
            Command::TimelineRun { key } => self.timeline_run(show, *key),
            Command::TimelineSeek { key, pos, picture } => {
                self.timeline_seek(show, *key, *pos, *picture)
            }
            Command::TimelineRunFrom { key, pos } => self.timeline_run_from(show, *key, *pos),
            Command::TimelinePause {
                key,
                freeze,
                clips,
                live,
                waits_for_go,
            } => self.timeline_pause(show, *key, *freeze, clips, *live, *waits_for_go),
            Command::TimelineStop { key, fade } => self.timeline_stop(show, *key, *fade),
            Command::TimelineGoOne { key } => self.timeline_go_one(show, *key),
            Command::TimelineContinue { key } => self.timeline_continue(show, *key),
            Command::TimelineLeave { key } => self.timeline_leave(show, *key),
            Command::Unknown => {
                log::debug!("show network: a command from a later NewQ, not known here")
            }
        }
    }

    /// What this engine plays at shared time `now` (the heartbeat; `epoch`,
    /// `seq` and `show` are the caller's).
    pub fn net_state(&self, now: f64) -> PlayState {
        let mut cues: Vec<CueNow> = self
            .active
            .iter()
            .filter(|a| a.listed())
            .map(|a| CueNow {
                cue: a.cue_id,
                anchor: a.anchor.unwrap_or(now - a.elapsed),
                stopping: a.stopping,
                sound: a.sound_zero,
            })
            .collect();
        cues.sort_by(|a, b| a.cue.0.cmp(&b.cue.0).then(a.anchor.total_cmp(&b.anchor)));
        let mut playheads: Vec<(u32, Option<CueId>)> =
            self.list_playheads.iter().map(|(l, p)| (*l, *p)).collect();
        playheads.sort_by_key(|p| p.0);
        let runs = self
            .runs
            .net_runs()
            .into_iter()
            .map(|(key, state, pos, anchor)| RunNow {
                key,
                state: word(state),
                pos: match (state, anchor) {
                    (super::RunState::Running, Some(a)) => (now - a).max(pos),
                    _ => pos,
                },
                anchor,
            })
            .collect();
        PlayState {
            time: now,
            paused: self.paused,
            list: self.active_list,
            playhead: self.playhead,
            playheads,
            cues,
            runs,
            started: self.started_since_show(),
            ..Default::default()
        }
    }

    /// Compare what plays here with the primary's heartbeat `s`, at shared time
    /// `now`, and put right what has differed at this heartbeat and the last
    /// (`doubts` keeps them between). Returns what was put right, in words.
    pub fn net_correct(
        &mut self,
        show: &Show,
        s: &PlayState,
        now: f64,
        doubts: &mut NetCorrection,
    ) -> Vec<String> {
        self.enter();
        let mut seen: HashSet<Doubt> = HashSet::new();
        let mut done = Vec::new();
        // The playheads are the primary's (no side effects: nothing plays).
        if self.active_list != s.list && show.has_list(s.list) {
            self.active_list = s.list;
        }
        self.playhead = s.playhead.filter(|id| show.cue(*id).is_some());
        self.list_playheads = s
            .playheads
            .iter()
            .filter(|(l, _)| show.has_list(*l) && *l != self.active_list)
            .map(|(l, p)| (*l, p.filter(|id| show.cue(*id).is_some())))
            .collect();
        if self.paused != s.paused {
            if doubts.seen.contains(&Doubt::Paused) {
                self.set_paused(s.paused);
                done.push(if s.paused { "paused" } else { "resumed" }.to_string());
            } else {
                seen.insert(Doubt::Paused);
            }
        }
        // Cues, by cue: the primary's instances against ours, oldest first.
        let mut theirs: HashMap<CueId, Vec<&CueNow>> = HashMap::new();
        for c in &s.cues {
            theirs.entry(c.cue).or_default().push(c);
        }
        let mut ours: HashMap<CueId, Vec<(u64, f64, bool)>> = HashMap::new();
        for a in self.active.iter().filter(|a| a.listed()) {
            ours.entry(a.cue_id).or_default().push((
                a.instance,
                a.anchor.unwrap_or(now - a.elapsed),
                a.stopping,
            ));
        }
        for v in ours.values_mut() {
            v.sort_by(|a, b| a.1.total_cmp(&b.1));
        }
        let timeline_groups: HashSet<CueId> = s
            .runs
            .iter()
            .filter_map(|r| match r.key {
                TimelineKey::Group(id) => Some(id),
                _ => None,
            })
            .collect();
        let mut start: Vec<(CueId, usize, f64)> = Vec::new();
        let mut stop: Vec<u64> = Vec::new();
        let mut shift: Vec<(u64, f64)> = Vec::new();
        for (cue, list) in &theirs {
            let here = ours.get(cue).cloned().unwrap_or_default();
            let live: Vec<&&CueNow> = list.iter().filter(|c| !c.stopping).collect();
            let here_live: Vec<&(u64, f64, bool)> = here.iter().filter(|h| !h.2).collect();
            for (n, c) in live.iter().enumerate() {
                match here_live.get(n) {
                    None => {
                        // Not one this engine starts by itself part way (a
                        // timeline's clip is its run's to start, never a cue
                        // of its own).
                        if s.time - c.anchor < SETTLING
                            || !restartable(show, *cue)
                            || timeline_groups.contains(cue)
                            || model::key_of(show, *cue).is_some()
                        {
                            continue;
                        }
                        // Started here for this instance already and gone
                        // again: never started over and over (a sound whose
                        // output isn't here, a file that won't open).
                        let sliding = s.paused || doubts.was_paused;
                        if doubts
                            .started
                            .get(&(*cue, n))
                            .is_some_and(|a| sliding || (a - c.anchor).abs() <= SAME_ANCHOR)
                        {
                            let waiting = self.pending.iter().any(|p| p.cue_id == *cue);
                            if !waiting && doubts.gave_up.insert((*cue, n)) {
                                log::info!(
                                    "show network: {} was started here to match the primary and is gone again; it isn't started again until the primary starts it anew",
                                    name(show, *cue)
                                );
                            }
                            continue;
                        }
                        let d = Doubt::Missing(*cue, n);
                        if doubts.seen.contains(&d) {
                            start.push((*cue, n, c.anchor));
                        } else {
                            seen.insert(d);
                        }
                    }
                    Some((instance, anchor, _)) => {
                        let off = anchor - c.anchor;
                        if s.time - c.anchor < SETTLING {
                            continue;
                        }
                        if off.abs() <= LEAVE_UNDER {
                            continue;
                        }
                        seen.insert(Doubt::Off(*instance));
                        // Put right only a difference that is real: steady
                        // over several heartbeats, and only once.
                        let st = doubts.steady.entry(*instance).or_insert((0, off, off));
                        st.0 += 1;
                        st.1 = st.1.min(off);
                        st.2 = st.2.max(off);
                        if st.2 - st.1 > STEADY_SPREAD {
                            *st = (1, off, off);
                        }
                        if st.0 < STEADY_BEATS {
                            continue;
                        }
                        if doubts.fixed.contains(instance) {
                            if doubts.said.insert(*instance) {
                                log::info!(
                                    "show network: {} still differs from the primary's by {:+.0} ms after being put right once; it is left as it is",
                                    name(show, *cue),
                                    off * 1000.0
                                );
                            }
                            continue;
                        }
                        shift.push((*instance, c.anchor));
                    }
                }
            }
        }
        for (cue, list) in &ours {
            let wanted = theirs
                .get(cue)
                .map_or(0, |v| v.iter().filter(|c| !c.stopping).count());
            for (instance, anchor, stopping) in list.iter().filter(|h| !h.2).skip(wanted) {
                if *stopping || s.time - anchor < SETTLING {
                    continue;
                }
                let d = Doubt::Extra(*instance);
                if doubts.seen.contains(&d) {
                    stop.push(*instance);
                } else {
                    seen.insert(d);
                }
            }
        }
        for (cue, n, anchor) in start {
            let from = (now - anchor).max(0.0);
            let before = self.next_instance;
            self.start_with(
                show,
                cue,
                None,
                StartAt {
                    from,
                    ..StartAt::default()
                },
            );
            let mut began = false;
            for a in self.active.iter_mut().filter(|a| a.instance >= before) {
                a.anchor = Some(anchor);
                began = true;
            }
            // (Or waiting for its file to be read.)
            began |= self.pending.iter().any(|p| p.cue_id == cue);
            doubts.started.insert((cue, n), anchor);
            if began {
                done.push(format!("{} started {from:.2} s in", name(show, cue)));
            } else {
                log::info!(
                    "show network: {} couldn't be started here to match the primary",
                    name(show, cue)
                );
            }
        }
        for instance in stop {
            let id = self
                .active
                .iter()
                .find(|a| a.instance == instance)
                .map(|a| a.cue_id);
            self.stop_instance(instance, 0.1);
            if let Some(id) = id {
                done.push(format!("{} stopped", name(show, id)));
            }
        }
        for (instance, anchor) in shift {
            doubts.fixed.insert(instance);
            doubts.steady.remove(&instance);
            let Some(a) = self.active.iter_mut().find(|a| a.instance == instance) else {
                continue;
            };
            let id = a.cue_id;
            let was = a.anchor.unwrap_or(now - a.elapsed);
            let file = matches!(a.kind, ActiveKind::Video(_) | ActiveKind::Audio(_));
            // A sound re-anchored behind where it plays catches up by itself
            // (`Voice::keep_to`); one ahead goes on a little ahead.
            if (was - anchor).abs() > JUMP_PAST && file {
                let to = (now - anchor).max(0.0);
                let _ = self.seek(show, instance, to);
            } else {
                a.anchor = Some(anchor);
            }
            done.push(format!(
                "{} moved {:+.0} ms",
                name(show, id),
                (was - anchor) * 1000.0
            ));
        }
        // Timelines.
        let here: HashMap<TimelineKey, (super::RunState, f64, Option<f64>)> = self
            .runs
            .net_runs()
            .into_iter()
            .map(|(k, st, pos, anchor)| {
                let pos = match (st, anchor) {
                    (super::RunState::Running, Some(a)) => (now - a).max(pos),
                    _ => pos,
                };
                (k, (st, pos, anchor))
            })
            .collect();
        let mut keys: Vec<TimelineKey> = here.keys().copied().collect();
        keys.extend(s.runs.iter().map(|r| r.key));
        keys.sort_by_key(|k| format!("{k:?}"));
        keys.dedup();
        for key in keys {
            if !match key {
                TimelineKey::Group(id) => show.cue(id).is_some(),
                TimelineKey::List(n) => show.has_list(n),
            } {
                continue;
            }
            let theirs = s.runs.iter().find(|r| r.key == key);
            let mine = here.get(&key).copied();
            let Some(fix) = run_fix(theirs, mine, s.time, now) else {
                continue;
            };
            let d = Doubt::Run(key);
            if !doubts.seen.contains(&d) {
                seen.insert(d);
                continue;
            }
            match fix {
                RunFix::RunFrom(pos, anchor) => {
                    self.timeline_run_from(show, key, pos);
                    self.runs.net_place(key, anchor, Some(now));
                    done.push(format!("timeline run from {pos:.2} s"));
                }
                RunFix::Nudge(anchor) => {
                    self.runs.net_place(key, anchor, Some(now));
                    done.push("timeline clock moved".to_string());
                }
                RunFix::Pause(pos) => {
                    self.timeline_pause(show, key, PauseFreeze::Everything, &[], false, false);
                    if self
                        .runs
                        .net_runs()
                        .iter()
                        .any(|r| r.0 == key && (r.2 - pos).abs() > FRAME)
                    {
                        self.timeline_seek(show, key, pos, true);
                    }
                    done.push(format!("timeline paused at {pos:.2} s"));
                }
                RunFix::Park(pos) => {
                    self.timeline_seek(show, key, pos, true);
                    done.push(format!("timeline put at {pos:.2} s"));
                }
                RunFix::Stop => {
                    self.timeline_stop(show, key, 0.2);
                    done.push("timeline stopped".to_string());
                }
            }
        }
        // What no longer differs starts counting again; what has ended is
        // forgotten.
        doubts.steady.retain(|i, _| seen.contains(&Doubt::Off(*i)));
        let playing: HashSet<u64> = self.active.iter().map(|a| a.instance).collect();
        doubts.fixed.retain(|i| playing.contains(i));
        doubts.said.retain(|i| playing.contains(i));
        // A start is remembered while the primary still plays that instance
        // (followed while its anchors slide in a pause).
        let sliding = s.paused || doubts.was_paused;
        doubts.started.retain(|(cue, n), a| {
            let Some(c) = theirs
                .get(cue)
                .and_then(|l| l.iter().filter(|c| !c.stopping).nth(*n))
            else {
                return false;
            };
            if sliding {
                *a = c.anchor;
            }
            (*a - c.anchor).abs() <= SAME_ANCHOR
        });
        let started = &doubts.started;
        doubts.gave_up.retain(|k| started.contains_key(k));
        doubts.was_paused = s.paused;
        doubts.seen = seen;
        self.leave();
        done
    }
}

/// What to do to a run to agree with the primary's.
#[derive(Clone, Copy, Debug, PartialEq)]
enum RunFix {
    /// Run from this place, its clock anchored here.
    RunFrom(f64, f64),
    /// Running both: its clock's anchor moved.
    Nudge(f64),
    Pause(f64),
    /// Paused both, at different places.
    Park(f64),
    Stop,
}

fn run_fix(
    theirs: Option<&RunNow>,
    mine: Option<(super::RunState, f64, Option<f64>)>,
    then: f64,
    now: f64,
) -> Option<RunFix> {
    use super::RunState as S;
    let my_state = mine.map_or(S::Stopped, |m| m.0);
    let my_pos = mine.map_or(0.0, |m| m.1);
    let Some(t) = theirs else {
        return matches!(my_state, S::Running | S::Paused { .. } | S::Cueing)
            .then_some(RunFix::Stop);
    };
    match t.state {
        // Getting ready (or a word from a later NewQ): nothing to compare yet.
        RunWord::Cueing | RunWord::Unknown => None,
        RunWord::Stopped => {
            matches!(my_state, S::Running | S::Paused { .. } | S::Cueing).then_some(RunFix::Stop)
        }
        RunWord::Running => {
            let anchor = t.anchor.unwrap_or(then - t.pos);
            let pos = (now - anchor).max(0.0);
            match my_state {
                S::Running => {
                    let off = my_pos - pos;
                    if off.abs() <= FRAME {
                        None
                    } else if off.abs() < RUN_NUDGE {
                        Some(RunFix::Nudge(anchor))
                    } else {
                        Some(RunFix::RunFrom(pos, anchor))
                    }
                }
                // Getting ready here too: it catches up by its anchor.
                S::Cueing => None,
                S::Stopped | S::Paused { .. } => Some(RunFix::RunFrom(pos, anchor)),
            }
        }
        RunWord::Paused => match my_state {
            S::Running | S::Cueing => Some(RunFix::Pause(t.pos)),
            S::Paused { .. } if (my_pos - t.pos).abs() > FRAME => Some(RunFix::Park(t.pos)),
            S::Stopped => Some(RunFix::Park(t.pos)),
            _ => None,
        },
    }
}

fn word(s: super::RunState) -> RunWord {
    match s {
        super::RunState::Stopped => RunWord::Stopped,
        super::RunState::Cueing => RunWord::Cueing,
        super::RunState::Running => RunWord::Running,
        super::RunState::Paused { .. } => RunWord::Paused,
    }
}

/// A cue a follower starts by itself part way in when it lacks it: something
/// that shows or sounds (not a group, a fade or a control cue, which the
/// primary's own commands and the cues' own logic see to).
fn restartable(show: &Show, id: CueId) -> bool {
    show.cue(id).is_some_and(|c| {
        matches!(
            c.kind,
            CueKind::Video(_)
                | CueKind::Image(_)
                | CueKind::Audio(_)
                | CueKind::Text(_)
                | CueKind::Shape(_)
                | CueKind::Camera(_)
                | CueKind::LiveAudio(_)
                | CueKind::Wait(_)
        )
    })
}

impl super::ActiveCue {
    /// What a heartbeat lists and corrections compare: a cue playing by
    /// itself, not one the shared clock has taken past its length
    /// (`over_by_clock`), which has ended there on every computer while a
    /// sound device plays its last buffers out (a sound kept in silence
    /// here, its output elsewhere, has none to play).
    fn listed(&self) -> bool {
        self.plays_by_itself() && !self.over_by_clock
    }
}

fn name(show: &Show, id: CueId) -> String {
    show.cue(id)
        .map(|c| format!("cue {} ({})", c.number, c.name))
        .unwrap_or_else(|| "a cue".into())
}

#[cfg(test)]
impl Engine {
    /// How many messages its send cues have sent (a test outbox records them).
    pub fn net_sent(&self) -> usize {
        self.outbox.sent().len()
    }
}

#[cfg(test)]
#[path = "net_sound_tests.rs"]
mod sound_tests;

#[cfg(test)]
#[path = "net_chain_tests.rs"]
mod chain_tests;
