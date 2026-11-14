//! The timeline run: its clock and ordered starts, the pause cues and what they
//! freeze, GO, and clips whose end the clock keeps. Wait and text cues need no
//! media; the rest use the test clips (skipped without them, unless
//! `NEWQ_REQUIRE_MEDIA` is set) and sound goes to the fake device.

use super::*;
use crate::engine::CueStatus;
use crate::media::audio::AudioOut;
use crate::show::{
    ContinueMode, CueType, Curve, GroupMode, TEST_PATTERN_DEVICE, TimelineSettings, Transition,
};
use std::path::PathBuf;
use std::time::{Duration, Instant};

// ----- Building shows ----------------------------------------------------------

/// A timeline group at the end of the show.
fn group(show: &mut Show) -> CueId {
    let mut g = show.make_cue(CueType::Group, None, show.cues.len());
    if let CueKind::Group(gc) = &mut g.kind {
        gc.mode = GroupMode::Timeline;
    }
    let id = g.id;
    show.cues.push(g);
    id
}

/// `c` as a clip of the group `g` (the last group pushed) at `at` seconds.
fn child(show: &mut Show, g: CueId, mut c: Cue, at: f64) -> CueId {
    c.parent = Some(g);
    c.pre_wait = at;
    let id = c.id;
    show.cues.push(c);
    id
}

/// `c` at the end of the show (a list's cue, or one of no timeline).
fn top(show: &mut Show, mut c: Cue, list: u32, at: f64) -> CueId {
    c.list = list;
    c.pre_wait = at;
    let id = c.id;
    show.cues.push(c);
    show.normalize_lists();
    id
}

fn wait(show: &mut Show, secs: f64) -> Cue {
    let mut c = show.make_cue(CueType::Wait, None, show.cues.len());
    if let CueKind::Wait(w) = &mut c.kind {
        w.duration = secs;
    }
    c
}

/// A fade cue of `target` to `opacity` over `secs`.
fn fade(show: &mut Show, target: CueId, opacity: Option<f32>, secs: f64) -> Cue {
    let mut f = show.make_cue(CueType::Fade, None, show.cues.len());
    if let CueKind::Fade(c) = &mut f.kind {
        c.target = Some(target);
        c.duration = secs;
        c.curve = Curve::Linear;
        c.opacity = opacity;
        c.volume_db = None;
        c.stop_target = false;
    }
    f
}

fn control(show: &mut Show, c: ControlCue) -> Cue {
    let mut cue = show.make_cue(CueType::Control, None, show.cues.len());
    cue.kind = CueKind::Control(c);
    cue
}

/// A timeline pause on the cue's own timeline.
fn pause(show: &mut Show, freeze: PauseFreeze) -> Cue {
    control(
        show,
        ControlCue {
            action: ControlAction::TimelinePause,
            freeze,
            ..Default::default()
        },
    )
}

fn aimed(show: &mut Show, action: ControlAction, target: CueId) -> Cue {
    control(
        show,
        ControlCue {
            action,
            target: Some(target),
            ..Default::default()
        },
    )
}

/// A text cue held for `end` seconds (None: until stopped), cut in, no fades.
fn text(show: &mut Show, end: Option<f64>) -> Cue {
    let mut c = show.make_cue(CueType::Text, None, show.cues.len());
    if let CueKind::Text(t) = &mut c.kind {
        t.text = "Hello".into();
        let m = &mut t.media;
        m.transition = Transition::Cut;
        m.fade_in = 0.0;
        m.fade_out = 0.0;
        m.end = end;
    }
    c
}

/// A file cue (video or audio) playing `0..end` of `file`, cut in, no fades.
fn file(show: &mut Show, ty: CueType, file: PathBuf, end: f64) -> Cue {
    let mut c = show.make_cue(ty, Some(file), show.cues.len());
    let m = c.kind.media_mut().unwrap();
    m.transition = Transition::Cut;
    m.fade_in = 0.0;
    m.fade_out = 0.0;
    m.start = 0.0;
    m.end = Some(end);
    m.curve = Curve::Linear;
    c
}

fn media_of(show: &mut Show, id: CueId) -> &mut MediaCue {
    show.cue_mut(id).unwrap().kind.media_mut().unwrap()
}

/// A list that is a timeline.
fn timeline_list(show: &mut Show) -> u32 {
    let l = show.add_list("Timeline");
    show.lists.last_mut().unwrap().timeline = Some(TimelineSettings::default());
    l
}

// ----- Looking at the engine ---------------------------------------------------

fn key(g: CueId) -> TimelineKey {
    TimelineKey::Group(g)
}

fn view(e: &mut Engine, show: &Show, key: TimelineKey) -> RunView {
    e.timeline(show, key)
}

fn paused_at(e: &mut Engine, show: &Show, k: TimelineKey) -> Option<f64> {
    let v = view(e, show, k);
    matches!(v.state, RunState::Paused { .. }).then_some(v.pos)
}

fn active(e: &Engine, id: CueId) -> Option<&ActiveCue> {
    e.active.iter().rev().find(|a| a.cue_id == id)
}

fn playing(e: &Engine, id: CueId) -> bool {
    matches!(e.status(id), CueStatus::Playing { .. })
}

fn waiting(e: &Engine, id: CueId) -> bool {
    e.pending.iter().any(|p| p.cue_id == id)
}

fn elapsed(e: &Engine, id: CueId) -> f64 {
    active(e, id).map_or(-1.0, |a| a.elapsed)
}

fn opacity(e: &Engine, id: CueId) -> Option<f32> {
    active(e, id).map(|a| a.opacity.value)
}

fn instance(e: &Engine, id: CueId) -> u64 {
    active(e, id).map_or(0, |a| a.instance)
}

/// Tick `n` times by `dt` of show time, letting the decoders' threads run a
/// moment between ticks.
fn ticks(e: &mut Engine, show: &Show, dt: f64, n: usize) {
    for _ in 0..n {
        std::thread::sleep(Duration::from_millis(2));
        e.tick(show, dt);
    }
}

/// Tick by `dt` (None: by the real time that passes) until `cond`, for at most
/// `secs` of real time.
fn until(
    e: &mut Engine,
    show: &Show,
    dt: Option<f64>,
    what: &str,
    secs: u64,
    mut cond: impl FnMut(&Engine) -> bool,
) {
    let t0 = Instant::now();
    let mut last = Instant::now();
    while !cond(e) {
        assert!(
            t0.elapsed() < Duration::from_secs(secs),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(4));
        let now = Instant::now();
        e.tick(
            show,
            dt.unwrap_or_else(|| now.duration_since(last).as_secs_f64()),
        );
        last = now;
    }
}

/// An engine with FFmpeg (no sound device) and the path of a test file.
fn with_media(name: &str) -> Option<(Engine, PathBuf)> {
    let (ff, path) = crate::media::test_media(name)?;
    let mut e = Engine::for_tests();
    e.ff = Some(ff);
    Some((e, path))
}

// ----- The clock and its order -------------------------------------------------

#[test]
fn an_untouched_timeline_is_stopped_at_its_start_with_its_length() {
    let mut show = Show::default();
    let g = group(&mut show);
    let w = wait(&mut show, 3.0);
    child(&mut show, g, w, 2.0);
    let mut e = Engine::for_tests();
    let v = view(&mut e, &show, key(g));
    assert_eq!(v.state, RunState::Stopped);
    assert_eq!(
        (v.pos, v.length, v.next_pause, v.on_go),
        (0.0, Some(5.0), None, None)
    );
    assert!(e.timelines().is_empty() && !e.is_active());
    // Pausing or stopping what doesn't run does nothing.
    e.timeline_pause(&show, key(g), PauseFreeze::Everything, &[], true, true);
    e.timeline_stop(&show, key(g), 0.0);
    assert!(e.timelines().is_empty() && !e.is_active());
    assert!(e.timeline_detached(key(g)).is_empty() && e.timeline_waiting(key(g)).is_empty());
    // Run fires the group.
    e.timeline_run(&show, key(g));
    assert_eq!(view(&mut e, &show, key(g)).state, RunState::Running);
    assert_eq!(e.timelines(), vec![key(g)]);
}

#[test]
fn a_pause_cue_stops_the_clock_before_a_clip_just_after_it_in_one_long_tick() {
    let mut show = Show::default();
    let g = group(&mut show);
    let p = pause(&mut show, PauseFreeze::Nothing);
    child(&mut show, g, p, 2.0);
    let w = wait(&mut show, 5.0);
    let clip = child(&mut show, g, w, 2.001);
    let mut e = Engine::for_tests();
    e.fire(&show, g);
    e.tick(&show, 5.0);
    assert_eq!(paused_at(&mut e, &show, key(g)), Some(2.0));
    assert!(waiting(&e, clip) && !playing(&e, clip));
    // The time to it is from where the clock stands.
    assert!(
        matches!(e.status(clip), CueStatus::PreWait { remaining, .. }
        if (remaining - 0.001).abs() < 1e-9)
    );
    // Nothing moves while it waits, and the group doesn't end.
    e.tick(&show, 30.0);
    assert_eq!(paused_at(&mut e, &show, key(g)), Some(2.0));
    assert!(playing(&e, g) && waiting(&e, clip));
}

#[test]
fn a_pause_cue_at_a_clips_start_holds_it_until_go_and_then_it_starts_at_once() {
    let mut show = Show::default();
    let g = group(&mut show);
    // The clip comes first in the show; at the same time the pause still goes first.
    let w = wait(&mut show, 5.0);
    let clip = child(&mut show, g, w, 2.0);
    let p = pause(&mut show, PauseFreeze::Nothing);
    child(&mut show, g, p, 2.0);
    let mut e = Engine::for_tests();
    e.playhead = Some(g);
    e.go(&show);
    e.tick(&show, 3.0);
    assert_eq!(paused_at(&mut e, &show, key(g)), Some(2.0));
    assert!(!playing(&e, clip));
    let v = view(&mut e, &show, key(g));
    assert_eq!(v.on_go, Some(clip), "GO brings it in");
    assert_eq!(v.next_pause, None, "the pause is behind");
    e.go(&show);
    assert!(playing(&e, clip), "started by GO, without a tick");
    assert_eq!(view(&mut e, &show, key(g)).state, RunState::Running);
    // The group then ends with it, on its clock.
    e.tick(&show, 5.1);
    e.tick(&show, 0.01);
    assert!(!playing(&e, g) && !e.is_active());
    let v = view(&mut e, &show, key(g));
    assert_eq!(v.state, RunState::Stopped);
    assert!(v.pos > 7.0, "{}", v.pos);
}

#[test]
fn at_the_same_time_control_cues_go_before_fades_and_fades_before_clips() {
    let mut show = Show::default();
    let g = group(&mut show);
    let w = wait(&mut show, 5.0);
    let clip = child(&mut show, g, w, 1.0);
    let f = fade(&mut show, clip, None, 2.0);
    let fade = child(&mut show, g, f, 1.0);
    let mut e = Engine::for_tests();
    e.fire(&show, g);
    e.tick(&show, 1.5);
    assert!(playing(&e, clip) && playing(&e, fade));
    assert!(
        instance(&e, fade) < instance(&e, clip),
        "the fade first, though the clip is first in the show"
    );
}

#[test]
fn a_timeline_group_still_starts_together_ends_with_its_last_child_and_follows() {
    let mut show = Show::default();
    let g = group(&mut show);
    show.cues[0].continue_mode = ContinueMode::AutoFollow;
    let a = wait(&mut show, 1.0);
    let a = child(&mut show, g, a, 0.0);
    let b = wait(&mut show, 1.0);
    let b = child(&mut show, g, b, 0.5);
    let after = wait(&mut show, 1.0);
    let after = top(&mut show, after, 0, 0.0);
    let mut e = Engine::for_tests();
    e.playhead = Some(g);
    e.go(&show);
    assert!(playing(&e, a) && waiting(&e, b) && !playing(&e, after));
    // The group's time is its clock's.
    e.tick(&show, 0.6);
    assert!(playing(&e, b));
    assert!(
        matches!(e.status(g), CueStatus::Playing { elapsed, .. } if (elapsed - 0.6).abs() < 1e-9)
    );
    e.tick(&show, 1.0);
    e.tick(&show, 0.01);
    assert!(!playing(&e, g) && playing(&e, after));
}

// ----- What a pause freezes ----------------------------------------------------

#[test]
fn a_free_run_pause_stops_only_the_clock_and_the_others_freeze_what_they_say() {
    for freeze in [
        PauseFreeze::Nothing,
        PauseFreeze::Everything,
        PauseFreeze::Clips,
    ] {
        let mut show = Show::default();
        let g = group(&mut show);
        let w = wait(&mut show, 30.0);
        let a = child(&mut show, g, w, 0.0);
        let w = wait(&mut show, 30.0);
        let b = child(&mut show, g, w, 0.0);
        let mut p = pause(&mut show, freeze);
        if let CueKind::Control(c) = &mut p.kind {
            c.clips = vec![a];
        }
        child(&mut show, g, p, 1.0);
        let w = wait(&mut show, 30.0);
        let c = child(&mut show, g, w, 2.0);
        let mut e = Engine::for_tests();
        e.fire(&show, g);
        ticks(&mut e, &show, 0.5, 2);
        assert_eq!(paused_at(&mut e, &show, key(g)), Some(1.0), "{freeze:?}");
        let (a0, b0) = (elapsed(&e, a), elapsed(&e, b));
        ticks(&mut e, &show, 0.5, 4);
        let moved = |x: f64, x0: f64| x > x0 + 1.9;
        let (a1, b1) = (elapsed(&e, a), elapsed(&e, b));
        match freeze {
            PauseFreeze::Nothing => assert!(moved(a1, a0) && moved(b1, b0)),
            PauseFreeze::Everything => assert!(a1 == a0 && b1 == b0),
            PauseFreeze::Clips => assert!(a1 == a0 && moved(b1, b0)),
        }
        assert!(waiting(&e, c), "{freeze:?}: the clock is still");
        // GO: the clock runs on and what the pause froze goes on.
        e.go(&show);
        ticks(&mut e, &show, 0.5, 2);
        assert!(elapsed(&e, a) > a1 && elapsed(&e, b) > b1, "{freeze:?}");
        assert!(playing(&e, c), "{freeze:?}");
    }
}

#[test]
fn pausing_a_timeline_group_with_a_control_cue_freezes_its_waiting_children_too() {
    // (Holding the children used to leave the ones waiting counting down and starting.)
    let mut show = Show::default();
    let g = group(&mut show);
    let w = wait(&mut show, 30.0);
    let a = child(&mut show, g, w, 0.0);
    let w = wait(&mut show, 30.0);
    let b = child(&mut show, g, w, 2.0);
    let hold = aimed(&mut show, ControlAction::Pause, g);
    let hold = top(&mut show, hold, 0, 0.0);
    let go_on = aimed(&mut show, ControlAction::Resume, g);
    let go_on = top(&mut show, go_on, 0, 0.0);
    let mut e = Engine::for_tests();
    e.fire(&show, g);
    e.tick(&show, 1.0);
    e.fire(&show, hold);
    e.tick(&show, 5.0);
    assert!(waiting(&e, b) && !playing(&e, b), "the clock stopped");
    assert_eq!(elapsed(&e, a), 1.0, "and what plays holds");
    assert!(matches!(
        view(&mut e, &show, key(g)).state,
        RunState::Paused {
            freeze: PauseFreeze::Everything,
            waits_for_go: false
        }
    ));
    // GO doesn't resume a pause from outside the timeline: its Resume does.
    e.go(&show);
    assert!(paused_at(&mut e, &show, key(g)).is_some());
    e.fire(&show, go_on);
    e.tick(&show, 1.0);
    assert!(playing(&e, b) && elapsed(&e, a) == 2.0);
}

#[test]
fn the_show_paused_and_let_go_leaves_a_run_paused_by_its_cue_paused() {
    let mut show = Show::default();
    let g = group(&mut show);
    let w = wait(&mut show, 30.0);
    let a = child(&mut show, g, w, 0.0);
    let p = pause(&mut show, PauseFreeze::Everything);
    child(&mut show, g, p, 1.0);
    let mut e = Engine::for_tests();
    e.fire(&show, g);
    ticks(&mut e, &show, 0.5, 3);
    let held = elapsed(&e, a);
    e.set_paused(true);
    e.tick(&show, 1.0);
    e.set_paused(false);
    ticks(&mut e, &show, 0.5, 2);
    assert_eq!(paused_at(&mut e, &show, key(g)), Some(1.0));
    assert_eq!(elapsed(&e, a), held);
    assert!(active(&e, a).unwrap().hold);
    // While the show is paused the clock stands too.
    e.go(&show);
    e.set_paused(true);
    e.tick(&show, 5.0);
    assert_eq!(view(&mut e, &show, key(g)).pos, 1.0);
    // GO lets go of the show's pause, as firing a cue does.
    e.timeline_pause(&show, key(g), PauseFreeze::Nothing, &[], false, true);
    e.go(&show);
    assert!(!e.paused);
    e.tick(&show, 0.5);
    assert_eq!(view(&mut e, &show, key(g)).pos, 1.5);
}

// ----- Clips whose end the clock keeps -----------------------------------------

#[test]
fn a_clip_frozen_by_its_own_pause_ends_by_the_clock_and_go_does_not_let_it_go() {
    let mut show = Show::default();
    let g = group(&mut show);
    let t = text(&mut show, Some(3.0));
    let t = child(&mut show, g, t, 0.0);
    let freeze = aimed(&mut show, ControlAction::Pause, t);
    child(&mut show, g, freeze, 1.0);
    let p = pause(&mut show, PauseFreeze::Nothing);
    child(&mut show, g, p, 2.0);
    let mut e = Engine::for_tests();
    e.fire(&show, g);
    until(&mut e, &show, Some(0.0), "the text", 5, |e| {
        active(e, t).is_some_and(|a| a.started)
    });
    ticks(&mut e, &show, 0.25, 10);
    assert_eq!(paused_at(&mut e, &show, key(g)), Some(2.0));
    let a = active(&e, t).unwrap();
    assert!(a.hold && a.clocked && a.detached);
    assert!(a.elapsed <= 1.0 + 1e-9, "{}", a.elapsed);
    assert_eq!(e.timeline_detached(key(g)), vec![t]);
    e.go(&show);
    ticks(&mut e, &show, 0.25, 2);
    assert!(active(&e, t).unwrap().hold, "GO doesn't let it go");
    // Held at 1 s of its 3, it still goes when the clock reaches 3.
    ticks(&mut e, &show, 0.25, 3);
    assert!(active(&e, t).is_none(), "ended by the clock");
}

#[test]
fn a_clip_with_no_pause_in_it_ends_by_its_content_as_before() {
    let mut show = Show::default();
    let g = group(&mut show);
    let t = text(&mut show, Some(1.0));
    let t = child(&mut show, g, t, 0.0);
    // A pause after it: not inside it.
    let p = pause(&mut show, PauseFreeze::Nothing);
    child(&mut show, g, p, 1.5);
    let mut e = Engine::for_tests();
    e.fire(&show, g);
    until(&mut e, &show, Some(0.0), "the text", 5, |e| {
        active(e, t).is_some_and(|a| a.started)
    });
    assert!(!active(&e, t).unwrap().clocked);
    ticks(&mut e, &show, 0.25, 5);
    assert!(active(&e, t).is_none());
}

#[test]
fn a_pause_at_a_fade_cue_holds_the_fade_until_go() {
    let mut show = Show::default();
    let g = group(&mut show);
    let t = text(&mut show, None);
    let t = child(&mut show, g, t, 0.0);
    let f = fade(&mut show, t, Some(0.0), 1.0);
    child(&mut show, g, f, 2.0);
    let p = pause(&mut show, PauseFreeze::Nothing);
    child(&mut show, g, p, 2.0);
    let mut e = Engine::for_tests();
    e.fire(&show, g);
    until(&mut e, &show, Some(0.0), "the text", 5, |e| {
        active(e, t).is_some_and(|a| a.started)
    });
    ticks(&mut e, &show, 0.5, 10);
    assert_eq!(paused_at(&mut e, &show, key(g)), Some(2.0));
    assert_eq!(opacity(&e, t), Some(1.0), "no fade while paused");
    e.go(&show);
    ticks(&mut e, &show, 0.25, 2);
    let o = opacity(&e, t).unwrap();
    assert!((0.3..0.7).contains(&o), "the fade runs from GO: {o}");
}

// ----- Lists, GO and the transport ---------------------------------------------

#[test]
fn go_resumes_a_group_waiting_at_its_own_pause_before_firing_the_next_cue() {
    let mut show = Show::default();
    let other = show.add_list("Other");
    let g = group(&mut show);
    let p = pause(&mut show, PauseFreeze::Nothing);
    child(&mut show, g, p, 1.0);
    let w = wait(&mut show, 5.0);
    let clip = child(&mut show, g, w, 1.0);
    let w = wait(&mut show, 5.0);
    let next = top(&mut show, w, 0, 0.0);
    let w = wait(&mut show, 5.0);
    let elsewhere = top(&mut show, w, other, 0.0);
    let mut e = Engine::for_tests();
    e.playhead = Some(g);
    e.go(&show);
    assert_eq!(e.playhead, Some(next));
    e.tick(&show, 2.0);
    assert_eq!(paused_at(&mut e, &show, key(g)), Some(1.0));
    // Another list's GO fires its own cue and leaves the group waiting.
    e.go_list(&show, other);
    assert!(playing(&e, elsewhere));
    assert!(paused_at(&mut e, &show, key(g)).is_some() && !playing(&e, clip));
    // This list's GO resumes the group first, then fires the next cue.
    e.go(&show);
    assert!(playing(&e, clip) && !playing(&e, next));
    e.go(&show);
    assert!(playing(&e, next));
}

#[test]
fn a_timeline_list_runs_on_go_ignores_go_while_running_resumes_and_stops_at_its_end() {
    let mut show = Show::default();
    let l = timeline_list(&mut show);
    let mut w = wait(&mut show, 1.0);
    // A continue on a timeline's clip does nothing: the clock times them.
    w.continue_mode = ContinueMode::AutoContinue;
    let a = top(&mut show, w, l, 0.0);
    let p = pause(&mut show, PauseFreeze::Nothing);
    top(&mut show, p, l, 2.0);
    let w = wait(&mut show, 1.0);
    let b = top(&mut show, w, l, 2.0);
    let k = TimelineKey::List(l);
    let mut e = Engine::for_tests();
    e.switch_list(&show, l);
    e.go(&show);
    assert!(
        playing(&e, a) && waiting(&e, b),
        "a continue doesn't start the next"
    );
    assert!(e.is_active());
    e.tick(&show, 0.5);
    e.go(&show);
    assert_eq!(
        view(&mut e, &show, k).pos,
        0.5,
        "GO while it runs does nothing"
    );
    assert!(
        e.messages
            .iter()
            .any(|(_, m)| m.contains("timeline is running"))
    );
    e.tick(&show, 2.0);
    assert_eq!(paused_at(&mut e, &show, k), Some(2.0));
    e.go(&show);
    assert!(playing(&e, b));
    e.tick(&show, 1.5);
    e.tick(&show, 0.1);
    let v = view(&mut e, &show, k);
    assert_eq!(v.state, RunState::Stopped, "played to its end");
    assert!(!e.is_active());
    e.go(&show);
    assert!(!e.is_active(), "GO at the end does nothing");
    assert!(e.messages.iter().any(|(_, m)| m.contains("at its end")));
    // A clip fired on its own starts at once.
    e.fire(&show, b);
    assert!(playing(&e, b));
}

#[test]
fn stopping_all_keeps_a_list_timelines_place_and_reset_puts_it_back_to_the_start() {
    let mut show = Show::default();
    let l = timeline_list(&mut show);
    let w = wait(&mut show, 30.0);
    top(&mut show, w, l, 0.0);
    let w = wait(&mut show, 30.0);
    let later = top(&mut show, w, l, 5.0);
    let k = TimelineKey::List(l);
    let mut e = Engine::for_tests();
    e.switch_list(&show, l);
    e.go(&show);
    e.tick(&show, 3.0);
    e.stop_all(0.0);
    let v = view(&mut e, &show, k);
    assert_eq!((v.state, v.pos), (RunState::Stopped, 3.0));
    assert!(!e.is_active() && e.timelines().is_empty());
    // GO runs on from there (what was already under the playhead isn't started again).
    e.go(&show);
    assert!(!e.active.iter().any(|a| a.cue_id != later));
    e.tick(&show, 2.0);
    assert!(playing(&e, later));
    e.reset_all(&show);
    let v = view(&mut e, &show, k);
    assert_eq!((v.state, v.pos), (RunState::Stopped, 0.0));
}

#[test]
fn another_timeline_is_run_paused_and_stopped_by_its_key() {
    let mut show = Show::default();
    let l = timeline_list(&mut show);
    let w = wait(&mut show, 30.0);
    let a = top(&mut show, w, l, 0.0);
    let w = wait(&mut show, 30.0);
    let b = top(&mut show, w, l, 2.0);
    let k = TimelineKey::List(l);
    let by_key = |show: &mut Show, action| {
        control(
            show,
            ControlCue {
                action,
                timeline: Some(k),
                ..Default::default()
            },
        )
    };
    let c = by_key(&mut show, ControlAction::TimelineRun);
    let run = top(&mut show, c, 0, 0.0);
    let c = by_key(&mut show, ControlAction::TimelinePause);
    let hold = top(&mut show, c, 0, 0.0);
    let c = by_key(&mut show, ControlAction::TimelineStop);
    let stop = top(&mut show, c, 0, 0.0);
    let mut e = Engine::for_tests();
    e.fire(&show, run);
    assert!(playing(&e, a));
    e.tick(&show, 1.0);
    e.fire(&show, hold);
    assert!(matches!(
        view(&mut e, &show, k).state,
        RunState::Paused {
            waits_for_go: false,
            ..
        }
    ));
    e.tick(&show, 5.0);
    assert!(waiting(&e, b));
    // The main list's GO is not that timeline's.
    e.go(&show);
    assert!(paused_at(&mut e, &show, k).is_some());
    e.fire(&show, run);
    e.tick(&show, 1.5);
    assert!(playing(&e, b));
    e.fire(&show, stop);
    let v = view(&mut e, &show, k);
    assert_eq!(v.state, RunState::Stopped);
    assert!(!playing(&e, a) && !playing(&e, b) && !e.is_active());
    // A timeline action from a cue on no timeline, naming none, says so.
    let c = control(
        &mut show,
        ControlCue {
            action: ControlAction::TimelinePause,
            ..Default::default()
        },
    );
    let lost = top(&mut show, c, 0, 0.0);
    e.fire(&show, lost);
    assert!(
        e.messages
            .iter()
            .any(|(_, m)| m.contains("not on a timeline"))
    );
}

#[test]
fn firing_a_timeline_group_again_cancels_what_its_earlier_run_was_waiting_to_start() {
    let mut show = Show::default();
    let g = group(&mut show);
    let w = wait(&mut show, 30.0);
    child(&mut show, g, w, 0.0);
    let w = wait(&mut show, 30.0);
    let b = child(&mut show, g, w, 5.0);
    let mut e = Engine::for_tests();
    e.fire(&show, g);
    e.tick(&show, 1.0);
    e.fire(&show, g);
    assert_eq!(e.pending.iter().filter(|p| p.cue_id == b).count(), 1);
    e.tick(&show, 4.5);
    assert!(!playing(&e, b), "the first run's would be on by now");
    e.tick(&show, 1.0);
    assert_eq!(e.active.iter().filter(|a| a.cue_id == b).count(), 1);
}

#[test]
fn a_run_gets_ready_only_the_clips_its_clock_is_nearing() {
    let Some((mut e, img)) = with_media("color_red.png") else {
        return;
    };
    let mut show = Show::default();
    let g = group(&mut show);
    let mut clips = Vec::new();
    for n in 1..=30 {
        let mut c = show.make_cue(CueType::Image, Some(img.clone()), show.cues.len());
        c.kind.media_mut().unwrap().end = Some(0.5);
        clips.push(child(&mut show, g, c, f64::from(n)));
    }
    e.fire(&show, g);
    // (The clock doesn't move: only the first ten seconds are near.)
    until(&mut e, &show, Some(0.0), "the near ones ready", 10, |e| {
        clips[..10].iter().all(|c| e.is_prepared(*c))
    });
    ticks(&mut e, &show, 0.0, 5);
    assert!(clips[10..].iter().all(|c| !e.is_prepared(*c)));
    // As the clock moves on, the next ones are got ready.
    e.tick(&show, 5.0);
    until(&mut e, &show, Some(0.0), "the next ones ready", 10, |e| {
        clips[10..15].iter().all(|c| e.is_prepared(*c))
    });
    assert!(clips[16..].iter().all(|c| !e.is_prepared(*c)));
}

// ----- With media: the stage manager's hold, and running out --------------------

#[test]
fn a_loop_runs_on_through_a_free_run_pause_and_go_crossfades_to_the_next_clip() {
    let Some((mut e, clip)) = with_media("testsrc_10s.mp4") else {
        return;
    };
    let mut show = Show::default();
    let out = show.outputs[0].id;
    let g = group(&mut show);
    // A one-second loop, a pause at the next clip's start, the next clip crossfading in.
    let mut c = file(&mut show, CueType::Video, clip.clone(), 1.0);
    c.kind.media_mut().unwrap().loop_media = true;
    let lp = child(&mut show, g, c, 0.0);
    let p = pause(&mut show, PauseFreeze::Nothing);
    child(&mut show, g, p, 2.0);
    let mut c = file(&mut show, CueType::Video, clip, 3.0);
    let m = c.kind.media_mut().unwrap();
    m.transition = Transition::Crossfade;
    m.fade_in = 1.0;
    let next = child(&mut show, g, c, 2.0);
    e.playhead = Some(g);
    e.go(&show);
    until(&mut e, &show, Some(0.0), "the loop", 10, |e| {
        active(e, lp).is_some_and(|a| a.started)
    });
    // Several loops long: the loop goes round, nothing else starts.
    until(&mut e, &show, Some(0.05), "five passes", 20, |e| {
        active(e, lp).is_some_and(|a| a.pass_number() >= Some(5))
    });
    assert_eq!(paused_at(&mut e, &show, key(g)), Some(2.0));
    assert!(!active(&e, lp).unwrap().stopping && opacity(&e, lp) == Some(1.0));
    assert!(!playing(&e, next) && waiting(&e, next));
    assert!(e.is_prepared(next), "what GO brings in is ready");
    assert_eq!(e.layers(out).len(), 1);
    // GO: the next clip comes in and the loop fades under it over the crossfade.
    e.go(&show);
    until(&mut e, &show, Some(0.02), "the next clip", 10, |e| {
        active(e, next).is_some_and(|a| a.started)
    });
    let mut faded = false;
    until(&mut e, &show, Some(0.05), "the loop gone", 10, |e| {
        if let Some(o) = opacity(e, lp) {
            faded |= o > 0.05 && o < 0.95;
        }
        active(e, lp).is_none()
    });
    assert!(faded, "crossfaded, not cut");
    assert!(active(&e, next).is_some());
}

/// A timeline group with a video or sound clip of `len` seconds from 0, its
/// own fade-out `fade_out`, and a free-run pause at `pause_at`.
fn run_out_show(
    ty: CueType,
    file_path: PathBuf,
    len: f64,
    fade_out: f64,
    pause_at: f64,
    run_out: RunOut,
    run_out_fade: Option<f64>,
) -> (Show, CueId, CueId) {
    let mut show = Show::default();
    let g = group(&mut show);
    let c = file(&mut show, ty, file_path, len);
    let clip = child(&mut show, g, c, 0.0);
    let m = media_of(&mut show, clip);
    m.fade_out = fade_out;
    m.run_out = run_out;
    m.run_out_fade = run_out_fade;
    let p = pause(&mut show, PauseFreeze::Nothing);
    child(&mut show, g, p, pause_at);
    (show, g, clip)
}

/// Run a picture clip (show time by `dt`, as it has no sound) until it is
/// gone or `secs` of show time have passed: the clip's own time when it began
/// to fade (if it did), and whether it was ever part faded.
fn watch_fade(e: &mut Engine, show: &Show, clip: CueId, dt: f64, secs: f64) -> (Option<f64>, bool) {
    let (mut began, mut part) = (None, false);
    let mut t = 0.0;
    while t < secs {
        e.tick(show, dt);
        std::thread::sleep(Duration::from_millis(2));
        t += dt;
        let Some(a) = active(e, clip) else {
            break;
        };
        if a.auto_faded && began.is_none() {
            began = Some(a.elapsed);
        }
        part |= a.opacity.value > 0.02 && a.opacity.value < 0.98;
    }
    (began, part)
}

#[test]
fn a_video_that_runs_out_during_a_free_run_pause_ends_loops_or_holds_as_it_is_set_to() {
    let Some((mut e, path)) = with_media("testsrc_10s.mp4") else {
        return;
    };
    let start = |e: &mut Engine, show: &Show, g: CueId, clip: CueId| {
        e.stop_all(0.0);
        e.fire(show, g);
        until(e, show, Some(0.0), "the clip", 10, |e| {
            active(e, clip).is_some_and(|a| a.started)
        });
    };
    // End, by its own fade-out: a 1.5 s clip paused at 0.5 fades over its last 0.5 s.
    let (show, g, clip) = run_out_show(
        CueType::Video,
        path.clone(),
        1.5,
        0.5,
        0.5,
        RunOut::End,
        None,
    );
    start(&mut e, &show, g, clip);
    let (began, part) = watch_fade(&mut e, &show, clip, 0.05, 6.0);
    let began = began.expect("it faded");
    assert!(part && (0.95..1.15).contains(&began), "{began}");
    assert!(active(&e, clip).is_none() && paused_at(&mut e, &show, key(g)) == Some(0.5));
    // End with no fade-out of its own: the default fade, two seconds.
    let (show, g, clip) = run_out_show(
        CueType::Video,
        path.clone(),
        4.0,
        0.0,
        0.5,
        RunOut::End,
        None,
    );
    start(&mut e, &show, g, clip);
    let (began, part) = watch_fade(&mut e, &show, clip, 0.05, 8.0);
    let began = began.expect("it faded");
    assert!(part && (1.95..2.15).contains(&began), "{began}");
    assert!(active(&e, clip).is_none());
    // End with a cut.
    let (show, g, clip) = run_out_show(
        CueType::Video,
        path.clone(),
        1.5,
        0.5,
        0.5,
        RunOut::End,
        Some(0.0),
    );
    start(&mut e, &show, g, clip);
    let (_, part) = watch_fade(&mut e, &show, clip, 0.05, 6.0);
    assert!(!part && active(&e, clip).is_none(), "cut");
    // Loop: still going round, then the clock ends it after GO.
    let (show, g, clip) = run_out_show(
        CueType::Video,
        path.clone(),
        1.5,
        0.5,
        0.5,
        RunOut::Loop,
        None,
    );
    start(&mut e, &show, g, clip);
    ticks(&mut e, &show, 0.05, 80);
    let a = active(&e, clip).expect("looping");
    assert!(
        a.pass_number() > Some(2) && a.opacity.value == 1.0,
        "{:?}",
        a.pass_number()
    );
    e.go(&show);
    let (began, part) = watch_fade(&mut e, &show, clip, 0.05, 3.0);
    assert!(began.is_some() && part && active(&e, clip).is_none());
    // Hold: its last frame, at full level, until GO; then its own fade-out by the clock.
    let (show, g, clip) = run_out_show(CueType::Video, path, 1.5, 0.5, 0.5, RunOut::Hold, None);
    start(&mut e, &show, g, clip);
    until(&mut e, &show, Some(0.05), "the last frame", 10, |e| {
        active(e, clip).is_some_and(|a| a.held)
    });
    ticks(&mut e, &show, 0.05, 20);
    assert_eq!(opacity(&e, clip), Some(1.0));
    e.go(&show);
    ticks(&mut e, &show, 0.05, 15);
    let o = opacity(&e, clip).expect("fading");
    assert!(o > 0.02 && o < 0.98, "{o}");
    let (_, _) = watch_fade(&mut e, &show, clip, 0.05, 2.0);
    assert!(active(&e, clip).is_none(), "ended where the clock says");
}

#[test]
fn a_pause_at_a_clips_fade_out_holds_it_and_go_fades_it_by_its_own_fade_out() {
    let Some((mut e, path)) = with_media("testsrc_10s.mp4") else {
        return;
    };
    // Paused for longer than the content left, set to hold: no fade while paused.
    let (show, g, clip) = run_out_show(CueType::Video, path, 2.0, 1.0, 1.0, RunOut::Hold, None);
    e.fire(&show, g);
    until(&mut e, &show, Some(0.0), "the clip", 10, |e| {
        active(e, clip).is_some_and(|a| a.started)
    });
    until(&mut e, &show, Some(0.05), "its last frame", 10, |e| {
        active(e, clip).is_some_and(|a| a.held)
    });
    ticks(&mut e, &show, 0.05, 40);
    assert_eq!(paused_at(&mut e, &show, key(g)), Some(1.0));
    let a = active(&e, clip).unwrap();
    assert!(!a.auto_faded && a.opacity.value == 1.0 && a.clocked && a.detached);
    // After GO it fades over its fade-out (one second) and ends.
    e.go(&show);
    ticks(&mut e, &show, 0.05, 10);
    let o = opacity(&e, clip).expect("fading");
    assert!((0.3..0.7).contains(&o), "half way: {o}");
    ticks(&mut e, &show, 0.05, 12);
    assert!(active(&e, clip).is_none());
}

#[test]
fn a_sound_that_runs_out_during_a_free_run_pause_ends_loops_or_holds_as_it_is_set_to() {
    let Some((mut e, wav)) = with_media("tone_5s.wav") else {
        return;
    };
    e.audio = AudioOut::with_playing_device();
    for run_out in [RunOut::End, RunOut::Loop, RunOut::Hold] {
        // A one-second sound from 0, fading out over 0.3 s, paused at 0.3.
        let (show, g, clip) =
            run_out_show(CueType::Audio, wav.clone(), 1.0, 0.3, 0.3, run_out, None);
        e.stop_all(0.0);
        e.fire(&show, g);
        until(&mut e, &show, None, "the sound", 10, |e| {
            active(e, clip).is_some_and(|a| a.started)
        });
        // Played in real time: well past its end.
        let t0 = Instant::now();
        let mut faded = false;
        until(&mut e, &show, None, "two seconds", 10, |e| {
            if let Some(a) = active(e, clip) {
                faded |= a.volume.value < -1.0 && a.volume.value > MIN_DB + 1.0;
            }
            t0.elapsed() > Duration::from_secs(2)
        });
        assert_eq!(paused_at(&mut e, &show, key(g)), Some(0.3));
        match run_out {
            RunOut::End => assert!(faded && active(&e, clip).is_none(), "faded and ended"),
            RunOut::Loop => {
                let a = active(&e, clip).expect("looping");
                assert!(a.pass_number() > Some(1) && a.volume.value == 0.0);
            }
            RunOut::Hold => {
                let a = active(&e, clip).expect("silent, kept");
                assert!(a.held && !a.auto_faded);
            }
        }
        if run_out != RunOut::End {
            // GO: the clock ends it 0.7 s later.
            e.go(&show);
            until(&mut e, &show, None, "the clock to end it", 10, |e| {
                active(e, clip).is_none()
            });
        }
    }
}

// ----- Live inputs -------------------------------------------------------------

fn camera(show: &mut Show) -> Cue {
    let mut c = show.make_cue(CueType::Camera, None, show.cues.len());
    if let CueKind::Camera(cam) = &mut c.kind {
        cam.device = TEST_PATTERN_DEVICE.into();
        cam.width = 320;
        cam.height = 180;
        cam.media.transition = Transition::Cut;
        cam.media.fade_in = 0.0;
        cam.media.fade_out = 0.0;
    }
    c
}

fn shown_frames(e: &Engine, id: CueId) -> u64 {
    match active(e, id).map(|a| &a.kind) {
        Some(ActiveKind::Video(v)) => v.player.serial,
        _ => 0,
    }
}

/// Real time until `secs` have passed.
fn run_for(e: &mut Engine, show: &Show, secs: f64) {
    let t0 = Instant::now();
    until(e, show, None, "time", 10, |_| {
        t0.elapsed().as_secs_f64() >= secs
    });
}

#[test]
fn a_freezing_pause_freezes_a_camera_only_when_it_says_live_inputs_freeze_too() {
    let Some((mut e, _)) = with_media("color_red.png") else {
        return;
    };
    for live in [false, true] {
        let mut show = Show::default();
        let out = show.outputs[0].id;
        let g = group(&mut show);
        let c = camera(&mut show);
        let cam = child(&mut show, g, c, 0.0);
        let mut p = pause(&mut show, PauseFreeze::Everything);
        if let CueKind::Control(c) = &mut p.kind {
            c.freeze_live = live;
        }
        child(&mut show, g, p, 0.3);
        e.stop_all(0.0);
        e.fire(&show, g);
        until(&mut e, &show, None, "the camera", 10, |e| {
            shown_frames(e, cam) > 0
        });
        run_for(&mut e, &show, 0.4);
        assert!(paused_at(&mut e, &show, key(g)).is_some());
        let first = shown_frames(&e, cam);
        run_for(&mut e, &show, 0.6);
        let then = shown_frames(&e, cam);
        if live {
            assert_eq!(then, first, "frozen");
            assert_eq!(e.layers(out).len(), 1, "its last frame stays up");
            e.go(&show);
            run_for(&mut e, &show, 0.6);
            assert!(shown_frames(&e, cam) >= then + 5, "live again at once");
        } else {
            assert!(then >= first + 5, "the camera goes on live");
        }
    }
}

#[test]
fn the_transports_pause_leaves_a_camera_live_and_a_clip_pause_on_it_freezes_it() {
    let Some((mut e, _)) = with_media("color_red.png") else {
        return;
    };
    let mut show = Show::default();
    let g = group(&mut show);
    let c = camera(&mut show);
    let cam = child(&mut show, g, c, 0.0);
    let freeze = aimed(&mut show, ControlAction::Pause, cam);
    let freeze = top(&mut show, freeze, 0, 0.0);
    let thaw = aimed(&mut show, ControlAction::Resume, cam);
    let thaw = top(&mut show, thaw, 0, 0.0);
    e.fire(&show, g);
    until(&mut e, &show, None, "the camera", 10, |e| {
        shown_frames(e, cam) > 0
    });
    // The transport's Pause: everything, but live inputs carry on.
    e.timeline_pause(&show, key(g), PauseFreeze::Everything, &[], false, false);
    let first = shown_frames(&e, cam);
    run_for(&mut e, &show, 0.6);
    assert!(shown_frames(&e, cam) >= first + 5, "still live");
    e.timeline_run(&show, key(g));
    // A Pause aimed at the camera itself freezes it.
    e.fire(&show, freeze);
    run_for(&mut e, &show, 0.1);
    let held = shown_frames(&e, cam);
    run_for(&mut e, &show, 0.6);
    assert_eq!(shown_frames(&e, cam), held);
    e.fire(&show, thaw);
    run_for(&mut e, &show, 0.6);
    assert!(shown_frames(&e, cam) >= held + 5);
}
