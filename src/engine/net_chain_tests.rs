//! What follows a cue, ticking by a shared clock, starts where that cue ended
//! by the clock (`ended_at`, `chain_backdate`), but never further back than a
//! frame or two's lateness (`ENDED_LATE_MAX`) and never before the cue it
//! crossfades over began.

use super::super::*;
use crate::show::{ContinueMode, CueType, GroupMode, Transition};
use std::time::{Duration, Instant};

const T0: f64 = 1000.0;

fn wait(show: &mut Show, len: f64, parent: Option<CueId>) -> CueId {
    let mut c = show.make_cue(CueType::Wait, None, show.cues.len());
    if let CueKind::Wait(w) = &mut c.kind {
        w.duration = len;
    }
    c.parent = parent;
    if parent.is_none() {
        c.continue_mode = ContinueMode::AutoFollow;
    }
    let id = c.id;
    show.cues.push(c);
    id
}

fn anchor(e: &Engine, id: CueId) -> f64 {
    e.net_state(0.0)
        .cues
        .iter()
        .find(|c| c.cue == id)
        .map(|c| c.anchor)
        .expect("it plays")
}

/// Ticks at 100 Hz from `from` to `to` (shared time).
fn ticks(e: &mut Engine, show: &Show, from: f64, to: f64) {
    let mut t = from;
    while t < to - 1e-9 {
        t = (t + 0.01).min(to);
        e.tick_shared(show, t);
    }
}

/// A frame late, what follows is placed where the cue ended; a stall's worth
/// late, it starts at the tick, as it always did (not a quarter second into
/// itself on every computer).
#[test]
fn what_follows_a_cue_seen_to_end_after_a_stall_starts_at_that_tick() {
    let mut show = Show::default();
    let a = wait(&mut show, 1.0, None);
    let b = wait(&mut show, 1.003, None);
    let c = wait(&mut show, 1000.0, None);
    let mut e = Engine::for_tests();
    e.tick_shared(&show, T0);
    e.fire(&show, a);
    // Ends between two ticks: b begins where it ended.
    ticks(&mut e, &show, T0, T0 + 1.5);
    assert!(
        (anchor(&e, b) - (T0 + 1.0)).abs() < 1e-9,
        "{}",
        anchor(&e, b)
    );
    // b's end (at 2.003) is seen 0.6 s late, in one long tick.
    ticks(&mut e, &show, T0 + 1.5, T0 + 2.0);
    e.tick_shared(&show, T0 + 2.6);
    // (Anchored at the next tick, from the one it began at.)
    e.tick_shared(&show, T0 + 2.61);
    let at = anchor(&e, c);
    assert!((at - (T0 + 2.6)).abs() < 1e-9, "{at}");
}

/// A group whose last child was stopped a while after its other child ended:
/// what follows the group starts when the group ends, not back at that
/// child's end.
#[test]
fn what_follows_a_group_whose_last_child_was_stopped_starts_then() {
    let mut show = Show::default();
    let mut g = show.make_cue(CueType::Group, None, show.cues.len());
    if let CueKind::Group(gc) = &mut g.kind {
        gc.mode = GroupMode::Playlist;
    }
    g.continue_mode = ContinueMode::AutoFollow;
    let group = g.id;
    show.cues.push(g);
    wait(&mut show, 1.0, Some(group));
    let second = wait(&mut show, 5.0, Some(group));
    let after = wait(&mut show, 1000.0, None);
    let mut e = Engine::for_tests();
    e.tick_shared(&show, T0);
    e.fire(&show, group);
    ticks(&mut e, &show, T0, T0 + 1.6);
    // The second child began where the first ended.
    assert!((anchor(&e, second) - (T0 + 1.0)).abs() < 1e-9);
    e.stop_cue(second, 0.0);
    ticks(&mut e, &show, T0 + 1.6, T0 + 2.0);
    let at = anchor(&e, after);
    assert!(at > T0 + 1.59 && at < T0 + 1.65, "{}", at - T0);
}

/// A picture crossfading in for longer than the one before it lasts starts
/// no earlier than that one did.
#[test]
fn a_crossfade_longer_than_the_picture_before_never_starts_before_it() {
    let mut show = Show::default();
    let mut g = show.make_cue(CueType::Group, None, show.cues.len());
    if let CueKind::Group(gc) = &mut g.kind {
        gc.mode = GroupMode::Playlist;
    }
    let group = g.id;
    show.cues.push(g);
    let mut ids = Vec::new();
    for (end, fade_in) in [(Some(0.9), 0.0), (None, 1.0)] {
        let mut c = show.make_cue(CueType::Text, None, show.cues.len());
        if let CueKind::Text(t) = &mut c.kind {
            t.text = "Hello".into();
            t.media.transition = Transition::Crossfade;
            t.media.fade_in = fade_in;
            t.media.fade_out = 0.0;
            t.media.end = end;
        }
        c.parent = Some(group);
        ids.push(c.id);
        show.cues.push(c);
    }
    let mut e = Engine::for_tests();
    let t0 = Instant::now();
    let shared = || T0 + t0.elapsed().as_secs_f64();
    e.tick_shared(&show, shared());
    e.fire(&show, group);
    let second = loop {
        assert!(t0.elapsed() < Duration::from_secs(10), "it never came in");
        std::thread::sleep(Duration::from_millis(2));
        e.tick_shared(&show, shared());
        let s = e.net_state(0.0);
        if let Some(c) = s.cues.iter().find(|c| c.cue == ids[1]) {
            break c.anchor;
        }
    };
    let first = anchor(&e, ids[0]);
    assert!(second >= first - 1e-9, "{} before", first - second);
}

/// The shared clock put back moves where the cues just ended with
/// everything else (`net_shift_shared`), so what follows them is placed by
/// the clock as it is now.
#[test]
fn putting_the_shared_clock_back_moves_where_cues_ended() {
    let mut show = Show::default();
    let a = wait(&mut show, 1.0, None);
    let mut e = Engine::for_tests();
    e.tick_shared(&show, T0);
    e.fire(&show, a);
    ticks(&mut e, &show, T0, T0 + 1.005);
    let ended = |e: &Engine| e.net_ended.get(&a).copied().expect("its end is kept");
    assert!((ended(&e) - (T0 + 1.0)).abs() < 1e-9, "{}", ended(&e) - T0);
    e.net_shift_shared(-2.0);
    assert!((ended(&e) - (T0 - 1.0)).abs() < 1e-9, "{}", ended(&e) - T0);
    assert!((e.net_shared_time().unwrap() - (T0 - 0.995)).abs() < 1e-9);
}

/// A later NewQ's kind of timeline (`TimelineKey::UNKNOWN`) in a command, a
/// heartbeat or a control cue does nothing: nothing plays, no run is made,
/// nothing is put right.
#[test]
fn a_timeline_this_newq_doesnt_know_is_left_alone() {
    use crate::net::play::{Command, PlayState, RunNow, RunWord};
    use crate::show::{ControlAction, ControlCue, PauseFreeze};
    use crate::timeline::TimelineKey;
    let (mut show, list, _) = crate::remote::fixtures::show();
    let key = TimelineKey::UNKNOWN;
    let mut e = Engine::for_tests();
    e.tick_shared(&show, T0);
    for cmd in [
        Command::TimelineRun { key },
        Command::TimelineSeek {
            key,
            pos: 2.0,
            picture: true,
        },
        Command::TimelineRunFrom { key, pos: 1.0 },
        Command::TimelinePause {
            key,
            freeze: PauseFreeze::Everything,
            clips: vec![],
            live: false,
            waits_for_go: false,
        },
        Command::TimelineGoOne { key },
        Command::TimelineContinue { key },
        Command::TimelineLeave { key },
        Command::TimelineStop { key, fade: 0.0 },
    ] {
        e.net_apply(&show, &cmd, 0.0);
    }
    let mut c = crate::remote::fixtures::control(
        &mut show,
        ControlCue {
            action: ControlAction::TimelineRun,
            timeline: Some(key),
            ..Default::default()
        },
    );
    c.list = 0;
    let cid = c.id;
    show.cues.push(c);
    e.fire(&show, cid);
    ticks(&mut e, &show, T0, T0 + 0.5);
    assert!(!e.is_active(), "nothing plays");
    assert!(e.timelines().is_empty(), "{:?}", e.timelines());
    // (A run-from or a seek of a timeline that isn't there notes where it
    // was asked to be, stopped, as for any timeline no longer in the show.)
    assert!(
        e.timeline_clock(key)
            .is_none_or(|(state, _)| state == RunState::Stopped),
        "{:?}",
        e.timeline_clock(key)
    );
    // A heartbeat with that timeline running: nothing to put right, twice.
    let s = PlayState {
        time: T0 + 0.5,
        list,
        runs: vec![RunNow {
            key,
            state: RunWord::Running,
            pos: 3.0,
            anchor: Some(T0 - 2.5),
        }],
        ..PlayState::default()
    };
    let mut doubts = NetCorrection::default();
    for _ in 0..6 {
        let fixed = e.net_correct(&show, &s, T0 + 0.5, &mut doubts);
        assert!(fixed.is_empty(), "{fixed:?}");
    }
    assert!(!e.is_active() && e.timelines().is_empty());
}
