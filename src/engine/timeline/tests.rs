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
    // Stopped at once; what it played fades out over the default three seconds.
    ticks(&mut e, &show, 0.5, 7);
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

#[test]
fn go_plan_says_what_go_does_to_a_timeline_group_that_waits_and_go_does_just_that() {
    use crate::engine::{GoPlan, TimelineGo};
    let mut show = Show::default();
    let g = group(&mut show);
    let c = text(&mut show, None);
    child(&mut show, g, c, 0.0);
    let p = pause(&mut show, PauseFreeze::Nothing);
    child(&mut show, g, p, 2.0);
    let mut c = text(&mut show, None);
    c.kind.media_mut().unwrap().layer = 1;
    let b = child(&mut show, g, c, 3.0);
    let after = wait(&mut show, 5.0);
    let after = top(&mut show, after, 0, 0.0);
    let mut e = Engine::for_tests();
    e.playhead = Some(g);
    // Nothing waits: GO fires the cue at the playhead.
    assert_eq!(e.go_plan(&show), GoPlan::Cue(Some(g)));
    assert_eq!(e.go_plan(&show).describe(&show), None);
    e.go(&show);
    e.tick(&show, 1.0);
    assert_eq!(
        e.go_plan(&show),
        GoPlan::Cue(Some(after)),
        "it runs: GO fires on"
    );
    // At its pause: GO carries the timeline on, and says what it brings in next.
    e.tick(&show, 1.5);
    assert_eq!(paused_at(&mut e, &show, key(g)), Some(2.0));
    let plan = e.go_plan(&show);
    assert_eq!(
        plan,
        GoPlan::Timeline {
            key: key(g),
            does: TimelineGo::Resume,
            then: Some(b),
        }
    );
    let (what, then) = plan.describe(&show).unwrap();
    assert_eq!(what, format!("Continue {}", model::name(&show, key(g))));
    let b_cue = show.cue(b).unwrap();
    assert_eq!(
        then.as_deref(),
        Some(format!("then bring in {} · {}", b_cue.number, b_cue.name).as_str())
    );
    // The editor's toolbar says the same of the run.
    assert_eq!(view(&mut e, &show, key(g)).on_go, Some(b));
    e.go(&show);
    assert_eq!(state(&mut e, &show, key(g)), RunState::Running);
    assert!(
        !waiting(&e, after) && active(&e, after).is_none(),
        "GO fired nothing else"
    );
    assert_eq!(e.go_plan(&show), GoPlan::Cue(Some(after)));
    // Paused by hand and parked by a seek: GO plays from there.
    e.timeline_pause(&show, key(g), PauseFreeze::Everything, &[], false, false);
    assert_eq!(
        e.go_plan(&show),
        GoPlan::Cue(Some(after)),
        "a hand pause isn't GO's"
    );
    e.timeline_seek(&show, key(g), 1.0, true);
    assert!(view(&mut e, &show, key(g)).parked);
    let plan = e.go_plan(&show);
    assert!(
        matches!(
            plan,
            GoPlan::Timeline {
                does: TimelineGo::Play,
                ..
            }
        ),
        "{plan:?}"
    );
    assert!(plan.describe(&show).unwrap().0.starts_with("Play "));
    e.go(&show);
    assert!(!view(&mut e, &show, key(g)).parked);
    assert!(matches!(
        state(&mut e, &show, key(g)),
        RunState::Cueing | RunState::Running
    ));
    assert!(active(&e, after).is_none(), "GO fired nothing else");
}

#[test]
fn go_plan_on_a_timeline_list_runs_it_does_nothing_while_it_runs_and_continues_it_at_a_pause() {
    use crate::engine::{GoPlan, TimelineGo};
    let mut show = Show::default();
    let l = timeline_list(&mut show);
    let w = wait(&mut show, 10.0);
    top(&mut show, w, l, 0.0);
    let p = pause(&mut show, PauseFreeze::Nothing);
    top(&mut show, p, l, 1.0);
    let k = TimelineKey::List(l);
    let mut e = Engine::for_tests();
    e.active_list = l;
    let does = |e: &Engine| match e.go_plan(&show) {
        GoPlan::Timeline { key, does, .. } if key == k => Some(does),
        _ => None,
    };
    assert_eq!(does(&e), Some(TimelineGo::Start));
    e.go(&show);
    assert_eq!(state(&mut e, &show, k), RunState::Running);
    assert_eq!(does(&e), Some(TimelineGo::Running));
    e.go(&show);
    assert_eq!(state(&mut e, &show, k), RunState::Running, "GO did nothing");
    e.tick(&show, 1.5);
    assert_eq!(paused_at(&mut e, &show, k), Some(1.0));
    assert_eq!(does(&e), Some(TimelineGo::Resume));
    e.go(&show);
    assert_eq!(state(&mut e, &show, k), RunState::Running);
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
    // It goes when its picture runs out, which a busy machine may not have
    // decoded yet in the few real seconds those ticks took.
    until(&mut e, &show, None, "it to end", 20, |e| {
        active(e, clip).is_none()
    });
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

// ----- Seek, parking, jumps (plan 2.6, 2.7) -------------------------------------

use crate::engine::{LayerSource, PreparedKind};
use crate::media::ProbeState;
use crate::media::ffmpeg::MediaInfo;
use crate::show::{Animated, Keyframe, Marker, TimelinePoint};

/// A timeline group made in the editor (clips at opacity 0 wait for their fade-in).
fn editor_group(show: &mut Show) -> CueId {
    let g = group(show);
    if let Some(CueKind::Group(gc)) = show.cue_mut(g).map(|c| &mut c.kind) {
        gc.timeline = Some(TimelineSettings::for_editor());
    }
    g
}

fn ramp(pairs: &[(f64, f32)]) -> Animated {
    let mut a = Animated::new(pairs.first().map_or(1.0, |p| p.1));
    a.keys = pairs
        .iter()
        .map(|&(t, v)| Keyframe {
            t,
            v,
            curve: Curve::Linear,
        })
        .collect();
    a
}

/// What FFmpeg says of a file, once it has.
fn probed(e: &mut Engine, file: &std::path::Path) -> MediaInfo {
    let t0 = Instant::now();
    loop {
        e.probe.poll();
        match e.probe.get_urgent(e.ff.as_ref(), file) {
            ProbeState::Ready(info) => return info.clone(),
            ProbeState::Failed(err) => panic!("{err}"),
            ProbeState::Pending => {}
        }
        assert!(t0.elapsed() < Duration::from_secs(20), "never probed");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A picture's bytes, made short to compare.
fn digest(data: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    data.hash(&mut h);
    h.finish()
}

/// The first frame of `id`'s video decoded from `file_pos` seconds past its in
/// point at a steady speed, by FFmpeg, as the engine would decode it.
fn frame_at(e: &mut Engine, show: &Show, id: CueId, file_pos: f64) -> u64 {
    let mut cue = show.cue(id).unwrap().clone();
    let m = cue.kind.media_mut().unwrap();
    m.start += file_pos;
    m.rate = Animated::new(1.0);
    m.loop_media = false;
    let info = probed(e, &m.file.clone());
    let p = e.prepare_from(&cue, &info, 0.0, 0.0).unwrap();
    let PreparedKind::Video { mut player, .. } = p.kind else {
        panic!("a video");
    };
    let t0 = Instant::now();
    while !player.ready() {
        assert!(t0.elapsed() < Duration::from_secs(20), "no reference frame");
        std::thread::sleep(Duration::from_millis(5));
    }
    player.update(0.0);
    digest(&player.current().unwrap().data)
}

/// The frame a clip shows.
fn shown(e: &Engine, id: CueId) -> Option<u64> {
    match active(e, id).map(|a| &a.kind) {
        Some(ActiveKind::Video(v)) => v.player.current().map(|f| digest(&f.data)),
        _ => None,
    }
}

fn started(e: &Engine, id: CueId) -> bool {
    active(e, id).is_some_and(|a| a.started)
}

fn state(e: &mut Engine, show: &Show, k: TimelineKey) -> RunState {
    view(e, show, k).state
}

fn parked(e: &mut Engine, show: &Show, k: TimelineKey) -> bool {
    matches!(
        state(e, show, k),
        RunState::Paused {
            freeze: PauseFreeze::Everything,
            waits_for_go: true
        }
    )
}

#[test]
fn seeking_a_running_timeline_cues_its_clips_part_way_in_and_lets_them_go_on_one_tick() {
    let Some((mut e, clip)) = with_media("testsrc_10s.mp4") else {
        return;
    };
    probed(&mut e, &clip);
    let mut show = Show::default();
    let g = group(&mut show);
    let c = file(&mut show, CueType::Video, clip.clone(), 9.0);
    let a = child(&mut show, g, c, 1.0);
    let mut c = file(&mut show, CueType::Video, clip, 9.0);
    c.kind.media_mut().unwrap().layer = 1;
    let b = child(&mut show, g, c, 2.0);
    e.fire(&show, g);
    ticks(&mut e, &show, 0.1, 3);
    assert_eq!(state(&mut e, &show, key(g)), RunState::Running);
    e.timeline_seek(&show, key(g), 4.5, true);
    assert_eq!(state(&mut e, &show, key(g)), RunState::Cueing);
    let v = view(&mut e, &show, key(g));
    assert_eq!(v.pos, 4.5);
    for (id, from) in [(a, 3.5), (b, 2.5)] {
        let c = active(&e, id).expect("started");
        assert!(c.hold && (c.elapsed - from).abs() < 1e-9 && (c.file_from - from).abs() < 1e-9);
    }
    // The clock stands until both have a frame; then both go on the same tick.
    let mut released = None;
    for n in 0..4000 {
        std::thread::sleep(Duration::from_millis(3));
        e.tick(&show, 0.0);
        let holds = (active(&e, a).unwrap().hold, active(&e, b).unwrap().hold);
        if state(&mut e, &show, key(g)) == RunState::Running {
            assert_eq!(holds, (false, false), "let go together");
            released = Some(n);
            break;
        }
        assert_eq!(holds, (true, true));
        assert_eq!(view(&mut e, &show, key(g)).pos, 4.5, "the clock stands");
    }
    assert!(released.is_some(), "never let go");
    assert!(started(&e, a) && started(&e, b));
    // Each shows the picture of its new place in the file, not of its start.
    let here = frame_at(&mut e, &show, a, 3.5);
    let top = frame_at(&mut e, &show, a, 0.0);
    assert!(shown(&e, a) == Some(here) && shown(&e, a) != Some(top));
    assert_eq!(shown(&e, b), Some(frame_at(&mut e, &show, b, 2.5)));
    // And the run goes on from there.
    ticks(&mut e, &show, 0.1, 5);
    assert!(view(&mut e, &show, key(g)).pos > 4.5 && elapsed(&e, a) > 3.5);
}

#[test]
fn a_sound_seeked_into_plays_from_that_place_in_its_file_to_its_end() {
    let Some((mut e, wav)) = with_media("tone_5s.wav") else {
        return;
    };
    e.audio = AudioOut::with_playing_device();
    probed(&mut e, &wav);
    let mut show = Show::default();
    let g = group(&mut show);
    let c = file(&mut show, CueType::Audio, wav, 5.0);
    let s = child(&mut show, g, c, 1.0);
    e.fire(&show, g);
    e.timeline_seek(&show, key(g), 4.5, true);
    let a = active(&e, s).unwrap();
    assert!((a.elapsed - 3.5).abs() < 1e-9 && (a.file_from - 3.5).abs() < 1e-9);
    assert_eq!(a.duration, Some(5.0), "the whole clip's length");
    // Played to its end: its time then is its length, which it would be 3.5 s
    // past had its decoder begun at the in point.
    let mut last = 0.0;
    until(&mut e, &show, None, "the sound to end", 20, |e| {
        if let Some(a) = active(e, s) {
            last = a.elapsed;
        }
        active(e, s).is_none() && e.pending.iter().all(|p| p.cue_id != s)
    });
    assert!((last - 5.0).abs() < 0.3, "ended at {last}");
}

#[test]
fn a_seek_into_a_faster_or_keyframed_speed_starts_where_the_speed_has_got_the_file_to() {
    let Some((mut e, clip)) = with_media("testsrc_10s.mp4") else {
        return;
    };
    probed(&mut e, &clip);
    let mut show = Show::default();
    let g = group(&mut show);
    let mut c = file(&mut show, CueType::Video, clip.clone(), 10.0);
    c.kind.media_mut().unwrap().rate = Animated::new(2.0);
    let fast = child(&mut show, g, c, 0.0);
    let mut c = file(&mut show, CueType::Video, clip, 10.0);
    let m = c.kind.media_mut().unwrap();
    m.layer = 1;
    m.rate = ramp(&[(0.0, 0.5), (4.0, 2.0)]);
    let keyed = child(&mut show, g, c, 1.0);
    e.timeline_seek(&show, key(g), 2.0, true);
    assert!((active(&e, fast).unwrap().file_from - 4.0).abs() < 1e-9);
    let map = show
        .cue(keyed)
        .unwrap()
        .kind
        .media()
        .unwrap()
        .speed_map()
        .unwrap();
    let want = map.media_at(1.0);
    let k = active(&e, keyed).unwrap();
    assert!(
        (k.file_from - want).abs() < 1e-9 && k.elapsed == 1.0,
        "{}",
        k.file_from
    );
    until(&mut e, &show, Some(0.0), "the frames", 20, |e| {
        started(e, fast) && started(e, keyed)
    });
    assert_eq!(shown(&e, fast), Some(frame_at(&mut e, &show, fast, 4.0)));
    assert_eq!(shown(&e, keyed), Some(frame_at(&mut e, &show, keyed, want)));
    // The keyframed one's length is where its file runs out along the map.
    let len = e
        .cue_duration(&show, show.cue(keyed).unwrap())
        .flatten()
        .unwrap();
    assert!((len - map.time_for(10.0)).abs() < 1e-6);
}

#[test]
fn a_loop_seeked_into_starts_in_the_pass_it_has_got_to() {
    let Some((mut e, clip)) = with_media("testsrc_10s.mp4") else {
        return;
    };
    probed(&mut e, &clip);
    let mut show = Show::default();
    let g = group(&mut show);
    let mut c = file(&mut show, CueType::Video, clip, 2.0);
    c.kind.media_mut().unwrap().loop_media = true;
    let lp = child(&mut show, g, c, 1.0);
    e.timeline_seek(&show, key(g), 6.3, true);
    until(&mut e, &show, Some(0.0), "the loop", 20, |e| started(e, lp));
    let a = active(&e, lp).unwrap();
    assert!((a.elapsed - 5.3).abs() < 1e-9);
    assert_eq!(a.pass_number(), Some(3));
    assert!((a.bar_position() - 1.3).abs() < 1e-6);
    assert_eq!(shown(&e, lp), Some(frame_at(&mut e, &show, lp, 1.3)));
}

#[test]
fn a_still_a_text_and_a_camera_under_the_playhead_are_up_with_their_time_counted_from_their_start()
{
    let Some((mut e, img)) = with_media("color_red.png") else {
        return;
    };
    let mut show = Show::default();
    let out = show.outputs[0].id;
    let g = group(&mut show);
    let mut c = show.make_cue(CueType::Image, Some(img), show.cues.len());
    let m = c.kind.media_mut().unwrap();
    (m.transition, m.fade_in, m.end) = (Transition::Cut, 0.0, Some(5.0));
    let still = child(&mut show, g, c, 1.0);
    let mut c = text(&mut show, None);
    c.kind.media_mut().unwrap().layer = 1;
    let words = child(&mut show, g, c, 0.0);
    let mut c = camera(&mut show);
    let m = c.kind.media_mut().unwrap();
    (m.layer, m.end) = (2, Some(10.0));
    let cam = child(&mut show, g, c, 2.0);
    // Parked at 3: all three up, held, each at its own time.
    e.timeline_seek(&show, key(g), 3.0, true);
    assert!(parked(&mut e, &show, key(g)));
    until(&mut e, &show, None, "all three up", 20, |e| {
        [still, words, cam].iter().all(|c| started(e, *c)) && shown_frames(e, cam) > 0
    });
    assert_eq!(e.layers(out).len(), 3);
    assert_eq!(
        (elapsed(&e, still), elapsed(&e, words), elapsed(&e, cam)),
        (2.0, 3.0, 1.0)
    );
    // The camera is live while parked; the rest stand still.
    let first = shown_frames(&e, cam);
    run_for(&mut e, &show, 0.5);
    assert!(shown_frames(&e, cam) > first, "live");
    assert_eq!((elapsed(&e, still), elapsed(&e, cam)), (2.0, 1.0));
    // Run: the still ends at its time (5 s after its start), the camera's run at 10.
    e.timeline_run(&show, key(g));
    ticks(&mut e, &show, 0.0, 2);
    ticks(&mut e, &show, 0.5, 7);
    assert!(active(&e, still).is_none() && active(&e, cam).is_some());
    // Past a still's time it is not started at all.
    e.timeline_seek(&show, key(g), 7.0, true);
    assert!(active(&e, still).is_none() && active(&e, words).is_some());
}

#[test]
fn seeking_past_a_pause_cue_or_onto_one_leaves_the_run_running_or_parked_as_it_was() {
    let mut show = Show::default();
    let l = timeline_list(&mut show);
    let c = text(&mut show, None);
    let t = top(&mut show, c, l, 0.0);
    let p = pause(&mut show, PauseFreeze::Nothing);
    top(&mut show, p, l, 3.0);
    let p = pause(&mut show, PauseFreeze::Nothing);
    top(&mut show, p, l, 8.0);
    let k = TimelineKey::List(l);
    let mut e = Engine::for_tests();
    e.active_list = l;
    e.go(&show);
    ticks(&mut e, &show, 0.0, 2);
    // Running: past the pause at 3, it runs on from 5.
    e.timeline_seek(&show, k, 5.0, true);
    until(&mut e, &show, Some(0.0), "running", 10, |e| {
        e.runs
            .map
            .get(&k)
            .is_some_and(|r| r.state == RunState::Running)
    });
    e.tick(&show, 1.0);
    assert!((view(&mut e, &show, k).pos - 6.0).abs() < 1e-9);
    // Exactly onto it: it counts as passed.
    e.timeline_seek(&show, k, 3.0, true);
    until(&mut e, &show, Some(0.0), "running", 10, |e| {
        e.runs
            .map
            .get(&k)
            .is_some_and(|r| r.state == RunState::Running)
    });
    e.tick(&show, 0.5);
    assert!((view(&mut e, &show, k).pos - 3.5).abs() < 1e-9);
    // Paused at the next one, a seek past it parks there; GO runs on.
    e.tick(&show, 5.0);
    assert_eq!(paused_at(&mut e, &show, k), Some(8.0));
    assert!(
        !view(&mut e, &show, k).parked,
        "a pause cue's wait is not parked"
    );
    e.timeline_seek(&show, k, 9.0, true);
    assert!(parked(&mut e, &show, k));
    assert!(view(&mut e, &show, k).parked);
    // The text is drawn on a thread of its own: under load that can take
    // longer than a few ticks, and going on from a park cues it (the clock
    // stands until it is up), so these wait for it rather than for a count of
    // ticks. Ticks of 0 keep the clock where it is meanwhile.
    until(&mut e, &show, Some(0.0), "the text drawn", 10, |e| {
        started(e, t)
    });
    assert!(active(&e, t).is_some_and(|a| a.hold), "the text held");
    assert!(parked(&mut e, &show, k));
    e.go(&show);
    until(&mut e, &show, Some(0.0), "running", 10, |e| {
        e.runs
            .map
            .get(&k)
            .is_some_and(|r| r.state == RunState::Running)
    });
    assert!((view(&mut e, &show, k).pos - 9.0).abs() < 1e-9);
    assert!(!view(&mut e, &show, k).parked);
    assert!(active(&e, t).is_some_and(|a| !a.hold));
    e.tick(&show, 1.0);
    assert!((view(&mut e, &show, k).pos - 10.0).abs() < 1e-9);
}

#[test]
fn fades_and_stops_passed_are_played_through_and_control_cues_passed_are_not_done_again() {
    let mut show = Show::default();
    let elsewhere = wait(&mut show, 30.0);
    let elsewhere = top(&mut show, elsewhere, 0, 0.0);
    let g = group(&mut show);
    let c = text(&mut show, None);
    let a = child(&mut show, g, c, 0.0);
    let mut c = text(&mut show, None);
    c.kind.media_mut().unwrap().layer = 1;
    let b = child(&mut show, g, c, 0.0);
    let f = fade(&mut show, a, Some(0.25), 2.0);
    child(&mut show, g, f, 1.0);
    let s = aimed(&mut show, ControlAction::Start, elsewhere);
    child(&mut show, g, s, 1.5);
    let d = aimed(&mut show, ControlAction::Disarm, elsewhere);
    child(&mut show, g, d, 1.5);
    let mut st = show.make_cue(CueType::Stop, None, show.cues.len());
    if let CueKind::Stop(sc) = &mut st.kind {
        sc.target = Some(b);
        sc.fade = 3.0;
    }
    child(&mut show, g, st, 2.5);
    let mut e = Engine::for_tests();
    // In the middle of the fade: on its way from 1 to 0.25, held there.
    e.timeline_seek(&show, key(g), 2.0, true);
    ticks(&mut e, &show, 0.1, 3);
    let o = opacity(&e, a).unwrap();
    assert!((o - 0.625).abs() < 1e-4, "{o}");
    assert!(active(&e, a).unwrap().faded && active(&e, b).is_some());
    // Run: it goes on over the second left (once cued: under load the texts
    // can take more than a couple of ticks, and the clock stands meanwhile).
    e.timeline_run(&show, key(g));
    until(&mut e, &show, Some(0.0), "the run to be cued", 10, |e| {
        e.runs
            .map
            .get(&key(g))
            .is_some_and(|r| r.state == RunState::Running)
    });
    ticks(&mut e, &show, 0.25, 6);
    assert!((opacity(&e, a).unwrap() - 0.25).abs() < 1e-4);
    // Past it all: the end level, the stopped one gone, the cue elsewhere not
    // started, nothing disarmed.
    e.stop_all(0.0);
    e.take_show_edits();
    e.timeline_seek(&show, key(g), 4.0, true);
    ticks(&mut e, &show, 0.1, 3);
    assert!((opacity(&e, a).unwrap() - 0.25).abs() < 1e-6);
    assert!(active(&e, b).is_none() && !playing(&e, elsewhere));
    assert!(e.take_show_edits().is_empty());
}

#[test]
fn a_clip_waiting_for_its_fade_in_is_waiting_and_ready_in_its_lead_in_and_counts_from_it_after() {
    let Some((mut e, clip)) = with_media("testsrc_10s.mp4") else {
        return;
    };
    probed(&mut e, &clip);
    let mut show = Show::default();
    let g = editor_group(&mut show);
    let mut c = file(&mut show, CueType::Video, clip, 10.0);
    c.kind.media_mut().unwrap().motion.opacity = Some(ramp(&[(0.0, 0.0), (4.0, 0.0), (5.0, 1.0)]));
    let v = child(&mut show, g, c, 1.0);
    // Inside the lead-in (1 to 5): not started, waiting, got ready.
    e.timeline_seek(&show, key(g), 3.0, true);
    assert!(active(&e, v).is_none());
    assert_eq!(e.timeline_waiting(key(g)), vec![v]);
    until(&mut e, &show, Some(0.0), "it to be ready", 20, |e| {
        e.is_prepared(v)
    });
    // In its visible part: its own time from its start, its file from the lead-in.
    e.timeline_seek(&show, key(g), 7.0, true);
    let a = active(&e, v).unwrap();
    assert!(a.elapsed == 6.0 && (a.file_from - 2.0).abs() < 1e-9 && a.lead == 4.0);
    assert!(e.timeline_waiting(key(g)).is_empty());
    until(&mut e, &show, Some(0.0), "its frame", 20, |e| started(e, v));
    assert_eq!(shown(&e, v), Some(frame_at(&mut e, &show, v, 2.0)));
    // Its keyframed opacity where it is, not its base.
    assert_eq!(opacity(&e, v), Some(1.0));
    // The group's length has the lead-in in it.
    let len = e
        .cue_duration(&show, show.cue(g).unwrap())
        .flatten()
        .unwrap();
    assert!((len - 15.0).abs() < 0.2, "{len}");
}

#[test]
fn a_clip_whose_opacity_rises_late_starts_its_content_from_its_in_point_as_it_appears() {
    let Some((mut e, clip)) = with_media("testsrc_10s.mp4") else {
        return;
    };
    probed(&mut e, &clip);
    // Made in the editor, already running when it fades in, and an old show's.
    for case in ["waits", "runs unseen", "old show"] {
        let mut show = Show::default();
        let g = if case == "old show" {
            group(&mut show)
        } else {
            editor_group(&mut show)
        };
        let mut c = file(&mut show, CueType::Video, clip.clone(), 10.0);
        let m = c.kind.media_mut().unwrap();
        m.motion.opacity = Some(ramp(&[(0.0, 0.0), (4.0, 0.0), (5.0, 1.0)]));
        m.runs_unseen = case == "runs unseen";
        let v = child(&mut show, g, c, 0.0);
        let w = wait(&mut show, 9.0);
        child(&mut show, g, w, 0.0);
        e.stop_all(0.0);
        e.fire(&show, g);
        let mut t = 0.0;
        while t < 3.9 {
            e.tick(&show, 0.1);
            t += 0.1;
            std::thread::sleep(Duration::from_millis(2));
            assert_eq!(active(&e, v).is_none(), case == "waits", "{case} at {t}");
        }
        until(&mut e, &show, Some(0.1), "it to start", 20, |e| {
            active(e, v).is_some()
        });
        // (Its first frame, with the clock still.)
        until(&mut e, &show, Some(0.0), "it to be up", 20, |e| {
            started(e, v)
        });
        let a = active(&e, v).unwrap();
        if case == "waits" {
            assert!(
                (a.elapsed - 4.0).abs() < 0.15 && a.file_from == 0.0,
                "{}",
                a.elapsed
            );
            // One of its first frames, counted from its in point: as many as it
            // has had time for since its lead-in (it may have had a tick, and a
            // busy machine may not have decoded them all yet).
            let n = ((a.elapsed - a.lead) * 30.0).round() as u32;
            let now = shown(&e, v);
            let first: Vec<Option<u64>> = (0..=n)
                .map(|i| {
                    let at = if i > 0 {
                        f64::from(i) / 30.0 - 0.001
                    } else {
                        0.0
                    };
                    Some(frame_at(&mut e, &show, v, at))
                })
                .collect();
            assert!(
                first.contains(&now),
                "not one of its first {} frames",
                n + 1
            );
            assert_ne!(now, Some(frame_at(&mut e, &show, v, 4.0)));
        } else {
            assert!(a.file_from == 0.0 && a.lead == 0.0, "{case}");
        }
    }
}

#[test]
fn a_clip_at_opacity_0_starts_when_a_fade_cue_raises_it_and_never_without_one() {
    let Some((mut e, clip)) = with_media("testsrc_10s.mp4") else {
        return;
    };
    probed(&mut e, &clip);
    let mut show = Show::default();
    let g = editor_group(&mut show);
    let mut c = file(&mut show, CueType::Video, clip.clone(), 10.0);
    c.kind.media_mut().unwrap().opacity = 0.0;
    let v = child(&mut show, g, c, 0.0);
    let f = fade(&mut show, v, Some(1.0), 1.0);
    child(&mut show, g, f, 6.0);
    e.fire(&show, g);
    for _ in 0..55 {
        e.tick(&show, 0.1);
        assert!(active(&e, v).is_none());
    }
    assert_eq!(e.timeline_waiting(key(g)), vec![v]);
    until(&mut e, &show, Some(0.1), "the fade", 20, |e| {
        active(e, v).is_some()
    });
    let a = active(&e, v).unwrap();
    // It starts at the fade, from its in point. Its file was got ready ahead, so
    // on some machines its first frame is there in the tick that starts it, and
    // it has then had that tick (seen on the Mac runner).
    let had = a.elapsed - a.lead;
    assert!(
        (a.lead - 6.0).abs() < 0.15 && (-1e-9..0.1 + 1e-9).contains(&had) && a.file_from == 0.0,
        "lead {} elapsed {} from {}",
        a.lead,
        a.elapsed,
        a.file_from
    );
    assert!(a.faded && a.opacity.ramping(), "the fade brings it up");
    until(&mut e, &show, Some(0.0), "its frame", 20, |e| started(e, v));
    // One of its first frames: as many as it has had time for.
    let a = active(&e, v).unwrap();
    let n = ((a.elapsed - a.lead) * 30.0).round() as u32;
    let now = shown(&e, v);
    let first: Vec<Option<u64>> = (0..=n)
        .map(|i| {
            let at = if i > 0 {
                f64::from(i) / 30.0 - 0.001
            } else {
                0.0
            };
            Some(frame_at(&mut e, &show, v, at))
        })
        .collect();
    assert!(
        first.contains(&now),
        "not one of its first {} frames",
        n + 1
    );
    ticks(&mut e, &show, 0.25, 6);
    assert!(opacity(&e, v).unwrap() > 0.99);
    // With no fade cue it never starts, and the group still ends.
    let mut show = Show::default();
    let g = editor_group(&mut show);
    let mut c = file(&mut show, CueType::Video, clip, 10.0);
    c.kind.media_mut().unwrap().opacity = 0.0;
    let v = child(&mut show, g, c, 0.0);
    let w = wait(&mut show, 1.0);
    child(&mut show, g, w, 0.0);
    e.stop_all(0.0);
    e.fire(&show, g);
    ticks(&mut e, &show, 0.5, 4);
    assert!(active(&e, v).is_none() && !playing(&e, g) && !e.is_active());
}

#[test]
fn a_silent_sound_on_a_timeline_starts_when_a_fade_cue_brings_it_up() {
    let Some((mut e, wav)) = with_media("tone_5s.wav") else {
        return;
    };
    e.audio = AudioOut::with_playing_device();
    probed(&mut e, &wav);
    let mut show = Show::default();
    let g = editor_group(&mut show);
    let mut c = file(&mut show, CueType::Audio, wav, 5.0);
    c.kind.media_mut().unwrap().volume_db = MIN_DB;
    let s = child(&mut show, g, c, 0.0);
    let mut f = fade(&mut show, s, None, 1.0);
    if let CueKind::Fade(fc) = &mut f.kind {
        fc.volume_db = Some(0.0);
    }
    child(&mut show, g, f, 1.0);
    e.fire(&show, g);
    ticks(&mut e, &show, 0.2, 4);
    assert!(active(&e, s).is_none() && waiting(&e, s));
    e.tick(&show, 0.3);
    let a = active(&e, s).expect("woken");
    assert!((a.lead - 1.0).abs() < 1e-9 && a.file_from == 0.0 && a.volume.ramping());
}

#[test]
fn with_a_keyframed_speed_and_a_lead_in_the_file_counts_from_the_lead_in_as_the_editor_trims() {
    let Some((mut e, clip)) = with_media("testsrc_10s.mp4") else {
        return;
    };
    let mut show = Show::default();
    let g = editor_group(&mut show);
    let mut c = file(&mut show, CueType::Video, clip, 10.0);
    let m = c.kind.media_mut().unwrap();
    m.motion.opacity = Some(ramp(&[(0.0, 0.0), (2.0, 0.0), (3.0, 1.0)]));
    m.rate = ramp(&[(0.0, 0.5), (6.0, 2.0)]);
    m.end = None;
    let v = child(&mut show, g, c, 1.0);
    let map = show
        .cue(v)
        .unwrap()
        .kind
        .media()
        .unwrap()
        .speed_map()
        .unwrap();
    probed(
        &mut e,
        &show.cue(v).unwrap().kind.media().unwrap().file.clone(),
    );
    // Clip time 4 (2 s after the lead-in): M(4) - M(2) into the file.
    e.timeline_seek(&show, key(g), 5.0, true);
    let a = active(&e, v).unwrap();
    let want = map.media_at(4.0) - map.media_at(2.0);
    assert!((a.file_from - want).abs() < 1e-9, "{} {want}", a.file_from);
    // Trimmed by the editor to end at clip time 7, its content is 5 s long.
    let was = show.clone();
    assert!(crate::timeline::edit::trim(
        &mut show,
        v,
        crate::timeline::edit::Edge::Right,
        8.0,
        &mut |c| e.cue_duration(&was, c),
    ));
    let content = e
        .cue_duration(&show, show.cue(v).unwrap())
        .flatten()
        .unwrap();
    assert!((content - 5.0).abs() < 1e-6, "{content}");
    let clips = model::clips(&show, key(g), &mut |c| e.cue_duration(&show, c));
    assert!((clips[0].len.unwrap() - 7.0).abs() < 1e-6);
}

#[test]
fn a_parked_timeline_shows_the_frame_at_its_playhead_held_and_runs_on_from_it() {
    let Some((mut e, clip)) = with_media("testsrc_10s.mp4") else {
        return;
    };
    e.audio = AudioOut::with_playing_device();
    let mut show = Show::default();
    let out = show.outputs[0].id;
    let l = timeline_list(&mut show);
    let c = file(&mut show, CueType::Video, clip, 10.0);
    let v = top(&mut show, c, l, 1.0);
    let k = TimelineKey::List(l);
    probed(
        &mut e,
        &show.cue(v).unwrap().kind.media().unwrap().file.clone(),
    );
    ticks(&mut e, &show, 0.1, 3);
    assert!(
        e.layers(out).is_empty() && e.active.is_empty(),
        "nothing until a seek"
    );
    // Without the picture: parked, nothing started.
    e.timeline_seek(&show, k, 4.0, false);
    ticks(&mut e, &show, 0.1, 3);
    assert!(parked(&mut e, &show, k) && e.active.is_empty() && e.layers(out).is_empty());
    // With it: the frame at the playhead is on the output, held.
    e.timeline_seek(&show, k, 4.0, true);
    until(&mut e, &show, None, "the frame", 20, |e| {
        !e.layers(out).is_empty()
    });
    assert_eq!(shown(&e, v), Some(frame_at(&mut e, &show, v, 3.0)));
    let held = |e: &Engine| {
        let a = active(e, v).unwrap();
        let clock = match &a.kind {
            ActiveKind::Video(s) => s.voice.as_ref().map(|vo| vo.clock()),
            _ => None,
        };
        (a.elapsed, clock)
    };
    let before = held(&e);
    assert!(before.1.is_some(), "it has its sound");
    run_for(&mut e, &show, 0.4);
    assert_eq!(held(&e), before, "still, and silent");
    let LayerSource::Video(_) = &e.layers(out)[0].source else {
        panic!("its video");
    };
    // GO: it runs on from there at once.
    e.active_list = l;
    e.go(&show);
    assert_eq!(state(&mut e, &show, k), RunState::Running);
    run_for(&mut e, &show, 0.4);
    assert!(elapsed(&e, v) > 3.1 && view(&mut e, &show, k).pos > 4.1);
}

#[test]
fn moving_a_parked_playhead_again_and_again_leaves_no_decoders_behind() {
    let Some((mut e, clip)) = with_media("testsrc_10s.mp4") else {
        return;
    };
    probed(&mut e, &clip);
    let mut show = Show::default();
    let g = group(&mut show);
    let c = file(&mut show, CueType::Video, clip.clone(), 10.0);
    let a = child(&mut show, g, c, 0.0);
    let mut c = file(&mut show, CueType::Video, clip, 10.0);
    c.kind.media_mut().unwrap().layer = 1;
    let b = child(&mut show, g, c, 0.5);
    let mut readers = Vec::new();
    for n in 0..15 {
        e.timeline_seek(&show, key(g), 1.0 + f64::from(n) * 0.37, true);
        for id in [a, b] {
            if let Some(ActiveKind::Video(v)) = active(&e, id).map(|x| &x.kind) {
                readers.push(v.player.reader_done());
            }
        }
        ticks(&mut e, &show, 0.0, 2);
    }
    assert_eq!(readers.len(), 30);
    assert_eq!(e.active.iter().filter(|x| x.run.is_some()).count(), 2);
    let (old, now) = readers.split_at(28);
    let t0 = Instant::now();
    while !old
        .iter()
        .all(|r| r.load(std::sync::atomic::Ordering::Acquire))
    {
        assert!(
            t0.elapsed() < Duration::from_secs(20),
            "old decoders left running"
        );
        std::thread::sleep(Duration::from_millis(20));
        e.tick(&show, 0.0);
    }
    assert!(
        now.iter()
            .all(|r| !r.load(std::sync::atomic::Ordering::Acquire))
    );
    // The group parked is still the one group, and stopping it ends them.
    assert_eq!(e.active.iter().filter(|x| x.cue_id == g).count(), 1);
    e.stop_cue(g, 0.0);
    assert!(e.active.is_empty());
}

#[test]
fn jump_cues_go_to_a_time_a_marker_or_a_clip_and_one_to_its_own_place_does_not_spin() {
    let mut show = Show::default();
    let l = timeline_list(&mut show);
    show.lists.last_mut().unwrap().timeline = Some(TimelineSettings {
        markers: vec![Marker {
            id: 1,
            time: 12.0,
            name: "Chorus".into(),
            color: None,
        }],
        ..TimelineSettings::default()
    });
    let c = text(&mut show, Some(2.0));
    let early = top(&mut show, c, l, 0.0);
    let c = text(&mut show, Some(3.0));
    let late = top(&mut show, c, l, 10.0);
    let jump = |show: &mut Show, at: TimelinePoint| {
        control(
            show,
            ControlCue {
                action: ControlAction::TimelineJump,
                at: Some(at),
                ..Default::default()
            },
        )
    };
    let j = jump(&mut show, TimelinePoint::Time(10.5));
    let j = top(&mut show, j, l, 1.0);
    let k = TimelineKey::List(l);
    let mut e = Engine::for_tests();
    e.active_list = l;
    e.go(&show);
    e.tick(&show, 1.5);
    assert!(active(&e, late).is_some());
    assert!((elapsed(&e, late) - 0.5).abs() < 1e-9);
    until(&mut e, &show, Some(0.0), "the text cued", 10, |e| {
        e.runs
            .map
            .get(&k)
            .is_some_and(|r| r.state == RunState::Running)
    });
    // What it played before stayed up until then (P5's smooth seek).
    assert!(active(&e, early).is_none());
    // To a marker, and to a clip's start.
    for (at, pos) in [
        (TimelinePoint::Marker(1), 12.0),
        (TimelinePoint::Cue(late), 10.0),
    ] {
        if let CueKind::Control(c) = &mut show.cue_mut(j).unwrap().kind {
            c.at = Some(at);
        }
        e.stop_all(0.0);
        e.timeline_seek(&show, k, 0.0, false);
        e.go(&show);
        e.tick(&show, 1.0);
        ticks(&mut e, &show, 0.0, 2);
        let v = view(&mut e, &show, k);
        assert!((v.pos - pos).abs() < 1e-9, "{at:?}: {}", v.pos);
    }
    // To its own time: it has been passed there, so it runs on.
    if let CueKind::Control(c) = &mut show.cue_mut(j).unwrap().kind {
        c.at = Some(TimelinePoint::Time(1.0));
    }
    e.stop_all(0.0);
    e.timeline_seek(&show, k, 0.0, false);
    e.go(&show);
    e.tick(&show, 1.0);
    until(&mut e, &show, Some(0.0), "the text cued", 10, |e| {
        e.runs
            .map
            .get(&k)
            .is_some_and(|r| r.state == RunState::Running)
    });
    e.tick(&show, 0.5);
    assert!((view(&mut e, &show, k).pos - 1.5).abs() < 1e-9);
    assert!(e.take_problems().is_empty());
    // One at 0 to 0 would go round for ever in a frame: it is stopped.
    if let CueKind::Control(c) = &mut show.cue_mut(j).unwrap().kind {
        c.at = Some(TimelinePoint::Time(0.0));
    }
    show.cue_mut(j).unwrap().pre_wait = 0.0;
    e.set_log_tap(true);
    e.stop_all(0.0);
    e.timeline_seek(&show, k, 0.0, false);
    e.go(&show);
    e.tick(&show, 0.1);
    assert!(
        e.take_problems().iter().any(|p| p.contains("jumped")),
        "it says so"
    );
}

#[test]
fn a_run_cue_with_a_place_runs_its_timeline_from_there() {
    let mut show = Show::default();
    let g = group(&mut show);
    let c = text(&mut show, Some(5.0));
    let a = child(&mut show, g, c, 2.0);
    let mut r = control(
        &mut show,
        ControlCue {
            action: ControlAction::TimelineRun,
            timeline: Some(key(g)),
            at: Some(TimelinePoint::Time(4.0)),
            ..Default::default()
        },
    );
    r.pre_wait = 0.0;
    let r = top(&mut show, r, 0, 0.0);
    let mut e = Engine::for_tests();
    e.fire(&show, r);
    assert_eq!(state(&mut e, &show, key(g)), RunState::Cueing);
    until(&mut e, &show, Some(0.0), "the text cued", 10, |e| {
        e.runs
            .map
            .get(&key(g))
            .is_some_and(|r| r.state == RunState::Running)
    });
    assert!(playing(&e, g) && (elapsed(&e, a) - 2.0).abs() < 1e-9);
    e.tick(&show, 1.0);
    assert!((view(&mut e, &show, key(g)).pos - 5.0).abs() < 1e-9);
}

// ----- Keyframes follow the timeline's clock (P2c) -------------------------------

use crate::show::{Effect, EffectKind};

/// Give `c` keys that ramp from 1.5 s to 2.5 s of its time: opacity 1 to 0.2,
/// x 0 to 0.5 and a blur of 0 to 20.
fn keyed(mut c: Cue) -> Cue {
    let m = c.kind.media_mut().unwrap();
    m.motion.opacity = Some(ramp(&[(0.0, 1.0), (1.5, 1.0), (2.5, 0.2)]));
    m.motion.x = Some(ramp(&[(0.0, 0.0), (1.5, 0.0), (2.5, 0.5)]));
    let mut blur = Effect::new(EffectKind::Blur);
    blur.p[0] = ramp(&[(0.0, 0.0), (1.5, 0.0), (2.5, 20.0)]);
    m.effects = vec![blur];
    c
}

/// What a picture clip looks like: its opacity, x and blur.
fn looks(e: &Engine, id: CueId) -> (f32, f32, f32) {
    let a = active(e, id).expect("it is playing");
    let (x, blur) = match &a.kind {
        ActiveKind::Video(v) => (v.transform.x, v.fx.blur),
        ActiveKind::Image(i) => (i.transform.x, i.fx.blur),
        _ => panic!("a picture"),
    };
    (a.opacity.value, x, blur)
}

fn near(got: (f32, f32, f32), want: (f32, f32, f32), what: &str) {
    let ok = (got.0 - want.0).abs() < 0.02
        && (got.1 - want.1).abs() < 0.01
        && (got.2 - want.2).abs() < 0.4;
    assert!(ok, "{what}: {got:?}, not {want:?}");
}

/// A timeline group with a one-second looping video from 0 with `keyed`'s
/// keys, and a free-run pause at 1 s, before the ramp.
fn looping_keyed_show(clip: PathBuf) -> (Show, CueId, CueId) {
    let mut show = Show::default();
    let g = group(&mut show);
    let mut c = keyed(file(&mut show, CueType::Video, clip, 1.0));
    c.kind.media_mut().unwrap().loop_media = true;
    let lp = child(&mut show, g, c, 0.0);
    let p = pause(&mut show, PauseFreeze::Nothing);
    child(&mut show, g, p, 1.0);
    (show, g, lp)
}

/// Play `looping_keyed_show` to its pause, let the loop run on there for 4 s,
/// GO, and play on: how the loop looked at the pause, after 4 s of it, and with
/// the clock at 1.5, 2.0 and 2.6 s after GO.
fn through_the_pause(e: &mut Engine, show: &Show, g: CueId, lp: CueId) -> [(f32, f32, f32); 5] {
    e.playhead = Some(g);
    e.go(show);
    until(e, show, Some(0.0), "the loop", 20, |e| started(e, lp));
    ticks(e, show, 0.05, 30);
    assert_eq!(paused_at(e, show, key(g)), Some(1.0));
    let at_pause = looks(e, lp);
    let (content, passes) = {
        let a = active(e, lp).unwrap();
        (a.elapsed, a.pass_number())
    };
    ticks(e, show, 0.05, 80);
    let a = active(e, lp).unwrap();
    // The content went on round its loop; its keyframes' time stood with the clock.
    assert!(a.elapsed > content + 3.9, "{} {content}", a.elapsed);
    assert!(
        a.pass_number() >= passes.map(|p| p + 3),
        "{:?}",
        a.pass_number()
    );
    assert!((a.key_time() - 1.0).abs() < 1e-9, "{}", a.key_time());
    let after_wait = looks(e, lp);
    e.go(show);
    assert_eq!(state(e, show, key(g)), RunState::Running);
    let mut at = Vec::new();
    for n in [10, 10, 12] {
        ticks(e, show, 0.05, n);
        at.push(looks(e, lp));
    }
    let pos = view(e, show, key(g)).pos;
    assert!((pos - 2.6).abs() < 1e-6, "{pos}");
    [at_pause, after_wait, at[0], at[1], at[2]]
}

#[test]
fn opacity_keys_after_a_free_run_pause_stand_while_the_loop_runs_on_and_ramp_after_go() {
    let Some((mut e, clip)) = with_media("testsrc_10s.mp4") else {
        return;
    };
    let (show, g, lp) = looping_keyed_show(clip);
    let seen = through_the_pause(&mut e, &show, g, lp);
    let opacity: Vec<f32> = seen.iter().map(|s| s.0).collect();
    assert_eq!(&opacity[..3], &[1.0, 1.0, 1.0], "nothing moves until GO");
    // From GO, the ramp over its length: half way at 2 s, done by 2.5 s.
    assert!((opacity[3] - 0.6).abs() < 0.02, "{opacity:?}");
    assert!((opacity[4] - 0.2).abs() < 1e-4, "{opacity:?}");
}

#[test]
fn geometry_and_effect_keys_after_a_free_run_pause_also_wait_for_go() {
    let Some((mut e, clip)) = with_media("testsrc_10s.mp4") else {
        return;
    };
    let (show, g, lp) = looping_keyed_show(clip);
    let seen = through_the_pause(&mut e, &show, g, lp);
    for (s, what) in seen[..3]
        .iter()
        .zip(["at the pause", "after 4 s", "at 1.5 s"])
    {
        assert!(s.1.abs() < 1e-6 && s.2.abs() < 1e-4, "{what}: {s:?}");
    }
    assert!(
        (seen[3].1 - 0.25).abs() < 0.01 && (seen[3].2 - 10.0).abs() < 0.4,
        "{seen:?}"
    );
    assert_eq!((seen[4].1, seen[4].2), (0.5, 20.0));
}

#[test]
fn speed_keys_on_a_timeline_stay_with_the_content_through_a_free_run_pause() {
    let Some((mut e, clip)) = with_media("testsrc_10s.mp4") else {
        return;
    };
    probed(&mut e, &clip);
    let mut show = Show::default();
    let g = group(&mut show);
    let mut c = file(&mut show, CueType::Video, clip, 10.0);
    c.kind.media_mut().unwrap().rate = ramp(&[(0.0, 1.0), (2.0, 2.0)]);
    let v = child(&mut show, g, c, 0.0);
    let p = pause(&mut show, PauseFreeze::Nothing);
    child(&mut show, g, p, 0.5);
    e.playhead = Some(g);
    e.go(&show);
    until(&mut e, &show, Some(0.0), "the clip", 20, |e| started(e, v));
    ticks(&mut e, &show, 0.05, 12);
    assert_eq!(paused_at(&mut e, &show, key(g)), Some(0.5));
    until(&mut e, &show, Some(0.05), "3 s of content", 30, |e| {
        elapsed(e, v) >= 3.0
    });
    let a = active(&e, v).unwrap();
    assert!((a.key_time() - 0.5).abs() < 1e-9, "{}", a.key_time());
    // The frame it shows is where its speed has got the file to by its content's
    // own time (3 s: 5 s into the file), not by the clock's (0.5 s).
    let t = a.elapsed - a.seek_base;
    let map = show
        .cue(v)
        .unwrap()
        .kind
        .media()
        .unwrap()
        .speed_map()
        .unwrap();
    let frame = (t * 30.0 + 1e-6).floor() / 30.0;
    let want = (map.media_at(frame) * 30.0).round();
    assert!(want > 120.0, "{want}");
    let near_want: Vec<Option<u64>> = [want - 1.0, want, want + 1.0]
        .iter()
        .map(|f| Some(frame_at(&mut e, &show, v, f / 30.0 - 0.001)))
        .collect();
    until(&mut e, &show, Some(0.0), "the frame", 20, |e| {
        near_want.contains(&shown(e, v))
    });
    assert!((active(&e, v).unwrap().key_time() - 0.5).abs() < 1e-9);
}

#[test]
fn keys_on_a_cue_fired_off_a_timeline_follow_its_own_time_as_before() {
    let mut show = Show::default();
    let g = group(&mut show);
    let c = keyed(text(&mut show, None));
    // A clip of a timeline fired on its own, and a cue on no timeline.
    let clip = child(&mut show, g, c, 0.0);
    let mut c = keyed(text(&mut show, None));
    c.kind.media_mut().unwrap().layer = 1;
    let lone = top(&mut show, c, 0, 0.0);
    let hold = aimed(&mut show, ControlAction::Pause, lone);
    let hold = top(&mut show, hold, 0, 0.0);
    let go_on = aimed(&mut show, ControlAction::Resume, lone);
    let go_on = top(&mut show, go_on, 0, 0.0);
    let mut e = Engine::for_tests();
    e.fire(&show, clip);
    e.fire(&show, lone);
    until(&mut e, &show, Some(0.0), "the texts", 20, |e| {
        started(e, clip) && started(e, lone)
    });
    let follows = |e: &Engine, id: CueId| {
        let a = active(e, id).unwrap();
        let t = a.elapsed as f32;
        let k = ((t - 1.5) / 1.0).clamp(0.0, 1.0);
        assert_eq!(a.key_time(), a.elapsed);
        near(
            looks(e, id),
            (1.0 - 0.8 * k, 0.5 * k, 20.0 * k),
            "by its own time",
        );
    };
    ticks(&mut e, &show, 0.1, 20);
    follows(&e, clip);
    follows(&e, lone);
    // Held on its own, its keys stand with its time, and go on with it.
    e.fire(&show, hold);
    let before = elapsed(&e, lone);
    ticks(&mut e, &show, 0.1, 3);
    assert_eq!(elapsed(&e, lone), before);
    follows(&e, lone);
    e.fire(&show, go_on);
    ticks(&mut e, &show, 0.1, 2);
    assert!(elapsed(&e, lone) > before);
    follows(&e, lone);
    follows(&e, clip);
}

/// A timeline group with a text from 0 with `keyed`'s keys, frozen by a Pause
/// of its own from 0.5 to 1 s, and a free-run pause at 1.2 s.
fn keyed_text_show() -> (Show, CueId, CueId) {
    let mut show = Show::default();
    let g = group(&mut show);
    let c = keyed(text(&mut show, None));
    let t = child(&mut show, g, c, 0.0);
    let p = aimed(&mut show, ControlAction::Pause, t);
    child(&mut show, g, p, 0.5);
    let r = aimed(&mut show, ControlAction::Resume, t);
    child(&mut show, g, r, 1.0);
    let p = pause(&mut show, PauseFreeze::Nothing);
    child(&mut show, g, p, 1.2);
    (show, g, t)
}

#[test]
fn a_seek_into_a_ramp_shows_its_middle_as_playing_there_does() {
    let (show, g, t) = keyed_text_show();
    // Parked at 2 s, the middle of the ramp.
    let mut e = Engine::for_tests();
    e.timeline_seek(&show, key(g), 2.0, true);
    assert!(parked(&mut e, &show, key(g)));
    until(&mut e, &show, Some(0.0), "the text", 20, |e| started(e, t));
    let parked_looks = looks(&e, t);
    near(parked_looks, (0.6, 0.25, 10.0), "parked at 2 s");
    // Its content is behind the clock by the half second it was frozen.
    let a = active(&e, t).unwrap();
    assert!((a.elapsed - 1.5).abs() < 1e-9 && (a.key_time() - 2.0).abs() < 1e-9);
    // It stays so while parked, and a GO plays on from there.
    ticks(&mut e, &show, 0.1, 5);
    near(looks(&e, t), parked_looks, "still parked");
    e.go(&show);
    until(&mut e, &show, Some(0.0), "running", 20, |e| {
        e.runs
            .map
            .get(&key(g))
            .is_some_and(|r| r.state == RunState::Running)
    });
    ticks(&mut e, &show, 0.05, 2);
    near(looks(&e, t), (0.52, 0.3, 12.0), "on from 2 s");

    // Played there instead, with a wait at the pause: the same.
    let mut e = Engine::for_tests();
    e.fire(&show, g);
    until(&mut e, &show, Some(0.0), "the text", 20, |e| started(e, t));
    ticks(&mut e, &show, 0.05, 30);
    assert_eq!(paused_at(&mut e, &show, key(g)), Some(1.2));
    ticks(&mut e, &show, 0.1, 30);
    near(looks(&e, t), (1.0, 0.0, 0.0), "waiting at the pause");
    e.go(&show);
    ticks(&mut e, &show, 0.05, 16);
    assert!((view(&mut e, &show, key(g)).pos - 2.0).abs() < 1e-6);
    near(looks(&e, t), parked_looks, "played to 2 s");
    let a = active(&e, t).unwrap();
    assert!((a.key_time() - 2.0).abs() < 1e-9, "{}", a.key_time());
}

// ----- The timeline Stop fades (P2d) ---------------------------------------------

/// A timeline Stop on the cue's own timeline, with `fade` (None: the default).
fn timeline_stop_cue(show: &mut Show, fade: Option<f64>) -> Cue {
    control(
        show,
        ControlCue {
            action: ControlAction::TimelineStop,
            fade,
            ..Default::default()
        },
    )
}

/// A timeline group with a text up from 0 and a timeline Stop (`fade`) at 1 s,
/// played to the Stop.
fn stopped_text(fade: Option<f64>) -> (Engine, Show, CueId, CueId, CueId) {
    let mut show = Show::default();
    let g = group(&mut show);
    let t = text(&mut show, None);
    let t = child(&mut show, g, t, 0.0);
    let s = timeline_stop_cue(&mut show, fade);
    let s = child(&mut show, g, s, 1.0);
    let mut e = Engine::for_tests();
    e.fire(&show, g);
    until(&mut e, &show, Some(0.0), "the text", 10, |e| {
        active(e, t).is_some_and(|a| a.started)
    });
    ticks(&mut e, &show, 0.5, 2);
    (e, show, g, t, s)
}

#[test]
fn a_timeline_stop_fades_the_picture_out_over_three_seconds_and_then_the_run_is_over() {
    let (mut e, show, g, t, _) = stopped_text(None);
    // Stopped at once, where it was; the picture is on its way out.
    let v = view(&mut e, &show, key(g));
    assert_eq!((v.state, v.pos), (RunState::Stopped, 1.0));
    assert!(v.fading, "the editor says it is fading out");
    assert!(active(&e, t).is_some_and(|a| a.stopping));
    ticks(&mut e, &show, 0.25, 6);
    let o = opacity(&e, t).expect("still fading at 1.5 s");
    assert!((0.2..0.8).contains(&o), "part way: {o}");
    assert_eq!(view(&mut e, &show, key(g)).pos, 1.0, "the clock stands");
    ticks(&mut e, &show, 0.25, 6);
    assert!(active(&e, t).is_none(), "gone after 3 s");
    assert!(!e.is_active(), "the group has ended with it");
    let v = view(&mut e, &show, key(g));
    assert_eq!((v.state, v.pos), (RunState::Stopped, 1.0));
    assert!(!v.fading);
}

#[test]
fn a_timeline_stop_of_0_cuts_and_a_second_stop_during_the_fade_cuts() {
    let (e, _, _, t, _) = stopped_text(Some(0.0));
    assert!(active(&e, t).is_none() && !e.is_active(), "cut");

    let (mut e, mut show, g, t, _) = stopped_text(None);
    ticks(&mut e, &show, 0.25, 2);
    assert!(opacity(&e, t).is_some_and(|o| o > 0.5 && o < 1.0));
    // Another Stop of that timeline, from the main list.
    let again = control(
        &mut show,
        ControlCue {
            action: ControlAction::TimelineStop,
            timeline: Some(key(g)),
            ..Default::default()
        },
    );
    let again = top(&mut show, again, 0, 0.0);
    e.fire(&show, again);
    assert!(active(&e, t).is_none() && !e.is_active(), "the second cuts");
    // The editor's Stop does the same: a fade, then a cut on a second press.
    let mut show = Show::default();
    let g = group(&mut show);
    let t = text(&mut show, None);
    let t = child(&mut show, g, t, 0.0);
    let mut e = Engine::for_tests();
    e.fire(&show, g);
    until(&mut e, &show, Some(0.0), "the text", 10, |e| {
        active(e, t).is_some_and(|a| a.started)
    });
    e.timeline_stop(&show, key(g), crate::show::TIMELINE_STOP_FADE);
    ticks(&mut e, &show, 0.25, 4);
    assert!(opacity(&e, t).is_some_and(|o| o > 0.0 && o < 1.0));
    e.timeline_stop(&show, key(g), crate::show::TIMELINE_STOP_FADE);
    assert!(active(&e, t).is_none() && !e.is_active());
}

#[test]
fn running_a_list_timeline_during_its_stops_fade_cues_it_again_over_the_fading_picture() {
    let mut show = Show::default();
    let l = timeline_list(&mut show);
    let t = text(&mut show, None);
    let t = top(&mut show, t, l, 0.0);
    let s = timeline_stop_cue(&mut show, None);
    top(&mut show, s, l, 1.0);
    let k = TimelineKey::List(l);
    let mut e = Engine::for_tests();
    e.switch_list(&show, l);
    e.go(&show);
    until(&mut e, &show, Some(0.0), "the text", 10, |e| {
        active(e, t).is_some_and(|a| a.started)
    });
    ticks(&mut e, &show, 0.5, 2);
    assert_eq!(view(&mut e, &show, k).state, RunState::Stopped);
    ticks(&mut e, &show, 0.25, 2);
    let old = instance(&e, t);
    // GO runs it on from where it stopped; the old picture fades on meanwhile.
    e.go(&show);
    assert!(
        e.active
            .iter()
            .any(|a| a.instance == old && a.stopping && a.opacity.value < 1.0)
    );
    until(&mut e, &show, Some(0.0), "running again", 10, |e| {
        e.runs
            .map
            .get(&k)
            .is_some_and(|r| r.state == RunState::Running)
    });
    ticks(&mut e, &show, 0.05, 2);
    let a = active(&e, t).expect("the text again");
    assert!(a.instance != old && !a.stopping && a.opacity.value == 1.0);
    assert!(
        !e.active.iter().any(|a| a.instance == old),
        "the new picture took the old one's place"
    );
    assert_eq!(view(&mut e, &show, k).pos, 1.1);
}

#[test]
fn a_timeline_stop_fades_a_sound_out_too() {
    let Some((mut e, wav)) = with_media("tone_5s.wav") else {
        return;
    };
    e.audio = AudioOut::with_playing_device();
    let mut show = Show::default();
    let g = group(&mut show);
    let c = file(&mut show, CueType::Audio, wav, 5.0);
    let clip = child(&mut show, g, c, 0.0);
    let s = timeline_stop_cue(&mut show, None);
    child(&mut show, g, s, 0.5);
    e.fire(&show, g);
    until(&mut e, &show, Some(0.0), "the sound", 10, |e| {
        active(e, clip).is_some_and(|a| a.started)
    });
    ticks(&mut e, &show, 0.25, 2);
    assert_eq!(view(&mut e, &show, key(g)).state, RunState::Stopped);
    ticks(&mut e, &show, 0.25, 6);
    let v = active(&e, clip).expect("still fading").volume.value;
    assert!(v < -1.0 && v > MIN_DB + 1.0, "part way at 1.5 s: {v} dB");
    ticks(&mut e, &show, 0.25, 6);
    assert!(active(&e, clip).is_none() && !e.is_active());
}

// ----- Fades under way wait while the clock stands (P2d, ruling L2) ---------------

/// A timeline group with a text up from 0, a fade cue at 1 s taking it to
/// opacity 0 and x 0.5 over 2 s, and a free-run pause at 2 s, half way.
fn fading_text_show() -> (Show, CueId, CueId) {
    let mut show = Show::default();
    let g = group(&mut show);
    let t = text(&mut show, None);
    let t = child(&mut show, g, t, 0.0);
    let mut f = fade(&mut show, t, Some(0.0), 2.0);
    if let CueKind::Fade(fc) = &mut f.kind {
        fc.geometry.x = Some(0.5);
    }
    child(&mut show, g, f, 1.0);
    let p = pause(&mut show, PauseFreeze::Nothing);
    child(&mut show, g, p, 2.0);
    (show, g, t)
}

/// Fire `fading_text_show`'s group and play it to its pause.
fn to_the_pause(show: &Show, g: CueId, t: CueId) -> Engine {
    let mut e = Engine::for_tests();
    e.fire(show, g);
    until(&mut e, show, Some(0.0), "the text", 10, |e| started(e, t));
    ticks(&mut e, show, 0.25, 8);
    assert_eq!(paused_at(&mut e, show, key(g)), Some(2.0));
    e
}

#[test]
fn a_fade_cue_half_done_at_a_free_run_pause_waits_there_and_finishes_after_go() {
    let (show, g, t) = fading_text_show();
    let mut e = to_the_pause(&show, g, t);
    near(looks(&e, t), (0.5, 0.25, 0.0), "half way at the pause");
    let fade_time = |e: &Engine| {
        e.active
            .iter()
            .find(|a| matches!(a.kind, ActiveKind::Fade))
            .expect("the fade cue is still going")
            .elapsed
    };
    let at_pause = fade_time(&e);
    ticks(&mut e, &show, 0.25, 12);
    near(looks(&e, t), (0.5, 0.25, 0.0), "still half way after 3 s");
    assert_eq!(fade_time(&e), at_pause, "the fade cue waits too");
    e.go(&show);
    ticks(&mut e, &show, 0.25, 2);
    let played = looks(&e, t);
    near(played, (0.25, 0.375, 0.0), "on from GO");
    ticks(&mut e, &show, 0.25, 2);
    near(looks(&e, t), (0.0, 0.5, 0.0), "done");

    // A seek to the same place on the timeline (2.5 s) shows the same.
    let mut e = Engine::for_tests();
    e.timeline_seek(&show, key(g), 2.5, true);
    until(&mut e, &show, Some(0.0), "the text", 10, |e| started(e, t));
    near(looks(&e, t), played, "seeked to 2.5 s");
}

#[test]
fn a_hand_fade_during_a_timeline_pause_runs_and_a_cue_off_the_timeline_is_unchanged() {
    let (mut show, g, t) = fading_text_show();
    // A fade cue in the main list, aimed at the clip, and a text of no timeline
    // that fades in over 2 s.
    let mut up = fade(&mut show, t, Some(1.0), 1.0);
    if let CueKind::Fade(fc) = &mut up.kind {
        fc.curve = Curve::Linear;
    }
    let up = top(&mut show, up, 0, 0.0);
    let mut lone = text(&mut show, None);
    let m = lone.kind.media_mut().unwrap();
    m.transition = Transition::FadeIn;
    m.fade_in = 2.0;
    m.curve = Curve::Linear;
    let lone = top(&mut show, lone, 0, 0.0);

    // The active panel's fade-out: it runs while the clock stands.
    let mut e = to_the_pause(&show, g, t);
    e.fade_out_instance(&show, instance(&e, t));
    ticks(&mut e, &show, 0.25, 4);
    let o = opacity(&e, t).expect("fading out");
    assert!(o > 0.0 && o < 0.4, "{o}");
    ticks(&mut e, &show, 0.25, 5);
    assert!(active(&e, t).is_none(), "gone over its fade-out");

    // A fade cue fired from the main list moves the level it names; the
    // timeline's own fade of its position still waits for GO.
    let mut e = to_the_pause(&show, g, t);
    e.fire(&show, up);
    ticks(&mut e, &show, 0.25, 2);
    near(looks(&e, t), (0.75, 0.25, 0.0), "brought up by hand");
    ticks(&mut e, &show, 0.25, 2);
    near(looks(&e, t), (1.0, 0.25, 0.0), "up");

    // A cue on no timeline fades in as ever while the timeline waits.
    e.fire(&show, lone);
    until(&mut e, &show, Some(0.0), "the other text", 10, |e| {
        started(e, lone)
    });
    ticks(&mut e, &show, 0.25, 4);
    let o = opacity(&e, lone).unwrap();
    assert!((o - 0.5).abs() < 0.02, "{o}");
    ticks(&mut e, &show, 0.25, 4);
    assert_eq!(opacity(&e, lone), Some(1.0));
    assert_eq!(paused_at(&mut e, &show, key(g)), Some(2.0));
}

#[test]
fn a_clips_fade_in_waits_at_a_free_run_pause_while_its_content_plays_on() {
    let Some((mut e, clip)) = with_media("testsrc_10s.mp4") else {
        return;
    };
    let mut show = Show::default();
    let g = group(&mut show);
    let mut c = file(&mut show, CueType::Video, clip, 8.0);
    let m = c.kind.media_mut().unwrap();
    m.transition = Transition::FadeIn;
    m.fade_in = 2.0;
    let v = child(&mut show, g, c, 0.0);
    let p = pause(&mut show, PauseFreeze::Nothing);
    child(&mut show, g, p, 1.0);
    e.fire(&show, g);
    until(&mut e, &show, Some(0.0), "the clip", 10, |e| started(e, v));
    ticks(&mut e, &show, 0.25, 4);
    assert_eq!(paused_at(&mut e, &show, key(g)), Some(1.0));
    let o = opacity(&e, v).unwrap();
    assert!((o - 0.5).abs() < 0.02, "half way in: {o}");
    ticks(&mut e, &show, 0.25, 8);
    assert_eq!(opacity(&e, v), Some(o), "it waits for GO");
    assert!(
        elapsed(&e, v) > 2.9,
        "its content plays on: {}",
        elapsed(&e, v)
    );
    e.go(&show);
    ticks(&mut e, &show, 0.25, 2);
    let o = opacity(&e, v).unwrap();
    assert!((o - 0.75).abs() < 0.02, "on from GO: {o}");
    ticks(&mut e, &show, 0.25, 2);
    assert_eq!(opacity(&e, v), Some(1.0));
}

#[test]
fn a_run_out_fade_under_way_at_a_pause_waits_on_the_last_frame_and_finishes_after_go() {
    let Some((mut e, clip)) = with_media("testsrc_10s.mp4") else {
        return;
    };
    // Three seconds of content running out over a second; a free-run pause at
    // 0.5 s lets the content get a second ahead of the clock, a second at 1.5 s
    // comes half way through its run-out fade.
    let mut show = Show::default();
    let g = group(&mut show);
    let c = file(&mut show, CueType::Video, clip, 3.0);
    let v = child(&mut show, g, c, 0.0);
    media_of(&mut show, v).run_out_fade = Some(1.0);
    let p = pause(&mut show, PauseFreeze::Nothing);
    child(&mut show, g, p, 0.5);
    let p = pause(&mut show, PauseFreeze::Nothing);
    child(&mut show, g, p, 1.5);
    e.fire(&show, g);
    until(&mut e, &show, Some(0.0), "the clip", 10, |e| started(e, v));
    ticks(&mut e, &show, 0.25, 2);
    assert_eq!(paused_at(&mut e, &show, key(g)), Some(0.5));
    ticks(&mut e, &show, 0.25, 4);
    assert!(!active(&e, v).unwrap().auto_faded);
    e.go(&show);
    ticks(&mut e, &show, 0.25, 4);
    assert_eq!(paused_at(&mut e, &show, key(g)), Some(1.5));
    let a = active(&e, v).unwrap();
    assert!(a.auto_faded, "running out");
    let o = a.opacity.value;
    assert!((0.3..0.7).contains(&o), "half way out: {o}");
    // Its content runs out meanwhile: it stays on its last frame, as faded.
    until(&mut e, &show, Some(0.05), "its last frame", 20, |e| {
        active(e, v).is_some_and(|a| a.held)
    });
    ticks(&mut e, &show, 0.25, 4);
    assert_eq!(opacity(&e, v), Some(o));
    e.go(&show);
    ticks(&mut e, &show, 0.25, 1);
    let left = opacity(&e, v).expect("fading on");
    assert!(left < o && left > 0.0, "{left}");
    ticks(&mut e, &show, 0.25, 2);
    assert!(active(&e, v).is_none(), "faded out and gone");
}

#[test]
fn a_matrix_crosspoint_fade_waits_at_a_free_run_pause_and_goes_on_after_go() {
    use crate::show::Crosspoint;
    let Some((mut e, wav)) = with_media("tone_5s.wav") else {
        return;
    };
    // The fake device, mixed in real time by a thread of its own.
    e.audio = AudioOut::with_playing_device();
    let mut show = Show::default();
    let g = group(&mut show);
    let mut c = file(&mut show, CueType::Audio, wav, 5.0);
    let xp = |db| Crosspoint { from: 0, to: 0, db };
    c.kind.media_mut().unwrap().matrix = vec![xp(0.0)];
    let s = child(&mut show, g, c, 0.0);
    // At 0.5 s, the crosspoint down to silence over 1 s; a free-run pause at 1 s.
    let mut f = fade(&mut show, s, None, 1.0);
    if let CueKind::Fade(fc) = &mut f.kind {
        fc.matrix = vec![xp(MIN_DB)];
    }
    let f = child(&mut show, g, f, 0.5);
    let p = pause(&mut show, PauseFreeze::Nothing);
    child(&mut show, g, p, 1.0);
    e.fire(&show, g);
    until(&mut e, &show, Some(0.0), "the sound", 10, |e| {
        active(e, s).is_some_and(|a| a.started)
    });
    let voice = |e: &Engine| {
        active(e, s)
            .and_then(|a| a.voice())
            .map(|v| (v.crosspoints_held(), v.crosspoint_now(0)))
    };
    // Into the fade, then to the pause at once.
    e.tick(&show, 0.6);
    // Begun at 0.5 in a tick from 0 to 0.6, its own time counts from 0.5.
    let fade_time = active(&e, f).expect("the fade is under way").elapsed;
    assert!((fade_time - 0.1).abs() < 1e-9, "{fade_time}");
    assert_eq!(
        voice(&e).map(|v| v.0),
        Some(false),
        "it moves while the clock runs"
    );
    e.tick(&show, 0.5);
    assert_eq!(paused_at(&mut e, &show, key(g)), Some(1.0));
    let (held, at_pause) = voice(&e).unwrap();
    assert!(held, "held while the clock stands");
    // Real time passes (the mixer runs on), the show's clock with it; the
    // crosspoint stays where it was.
    std::thread::sleep(Duration::from_millis(60));
    let at_pause = voice(&e).unwrap().1.min(at_pause);
    for _ in 0..12 {
        std::thread::sleep(Duration::from_millis(40));
        e.tick(&show, 0.04);
    }
    let (held, now) = voice(&e).unwrap();
    assert!(held);
    assert!(at_pause > 0.05, "not at silence yet: {at_pause}");
    assert!((now - at_pause).abs() < 1e-6, "{at_pause} -> {now}");
    assert!(active(&e, f).is_some(), "the fade cue waits too");
    // GO: it goes on, and reaches silence.
    e.go(&show);
    e.tick(&show, 0.0);
    assert_eq!(voice(&e).map(|v| v.0), Some(false), "let go");
    until(&mut e, &show, None, "the crosspoint at silence", 10, |e| {
        voice(e).is_some_and(|v| v.1 < 1e-4)
    });
}

// ----- Overlapping clips crossfade (P5, plan 2.9) ---------------------------------

use crate::show::OutputId;

/// A timeline group whose overlapping clips crossfade (or not, `on` false).
fn overlap_group(show: &mut Show, on: bool) -> CueId {
    let g = group(show);
    if let Some(CueKind::Group(gc)) = show.cue_mut(g).map(|c| &mut c.kind) {
        gc.timeline = Some(TimelineSettings {
            crossfade_overlaps: on,
            ..TimelineSettings::default()
        });
    }
    g
}

/// A text held `end` seconds with a fade-out of its own, curves straight.
fn faded_text(show: &mut Show, end: f64, fade_out: f64) -> Cue {
    let mut c = text(show, Some(end));
    let m = c.kind.media_mut().unwrap();
    m.fade_out = fade_out;
    m.curve = Curve::Linear;
    c
}

/// Each picture's opacity on `out`, by cue.
fn levels(e: &Engine, out: OutputId) -> Vec<(CueId, f32)> {
    e.layers(out)
        .iter()
        .map(|l| {
            let a = e.active.iter().find(|a| a.instance == l.instance).unwrap();
            (a.cue_id, l.opacity)
        })
        .collect()
}

/// Tick by `dt` to `to` on the run's clock, waiting with 0-ticks (as long as it
/// takes) whenever a clip of `ids` has been started but has no picture yet.
fn run_to(e: &mut Engine, show: &Show, k: TimelineKey, dt: f64, to: f64, ids: &[CueId]) {
    let t0 = Instant::now();
    while view(e, show, k).pos < to - 1e-9 {
        assert!(t0.elapsed() < Duration::from_secs(20), "never got to {to}");
        std::thread::sleep(Duration::from_millis(1));
        let drawing = ids
            .iter()
            .any(|id| active(e, *id).is_some_and(|a| !a.started));
        let left = to - view(e, show, k).pos;
        e.tick(show, if drawing { 0.0 } else { dt.min(left) });
    }
}

#[test]
fn overlapping_pictures_crossfade_over_the_overlap_and_the_first_has_no_fade_out_of_its_own() {
    for on in [true, false] {
        let mut show = Show::default();
        let out = show.outputs[0].id;
        let g = overlap_group(&mut show, on);
        // 0 to 3 s fading out over 1.5 s of its own; the next from 2 s, a cut.
        let c = faded_text(&mut show, 3.0, 1.5);
        let a = child(&mut show, g, c, 0.0);
        let c = faded_text(&mut show, 3.0, 0.0);
        let b = child(&mut show, g, c, 2.0);
        let mut e = Engine::for_tests();
        e.output_sizes.insert(out, (64, 36));
        e.fire(&show, g);
        run_to(&mut e, &show, key(g), 0.05, 1.8, &[a, b]);
        let first = opacity(&e, a).unwrap();
        if on {
            assert_eq!(
                first, 1.0,
                "no fade-out of its own before the next comes in"
            );
        } else {
            assert!(first < 0.9, "its own fade-out as before: {first}");
        }
        run_to(&mut e, &show, key(g), 0.05, 2.0, &[a, b]);
        until(&mut e, &show, Some(0.0), "the next drawn", 10, |e| {
            started(e, b)
        });
        e.tick(&show, 0.0);
        if !on {
            // A cut: the first goes as the next comes in.
            assert_eq!(levels(&e, out), vec![(b, 1.0)]);
            continue;
        }
        // Both are up through the overlap, the first going down as the next comes up.
        let mut seen = Vec::new();
        for _ in 0..9 {
            e.tick(&show, 0.1);
            let l = levels(&e, out);
            assert_eq!(l.len(), 2, "both up: {l:?}");
            let of = |id| l.iter().find(|x| x.0 == id).unwrap().1;
            let (la, lb) = (of(a), of(b));
            assert!((la + lb - 1.0).abs() < 0.02, "they cross: {l:?}");
            seen.push(la);
        }
        assert!(seen.windows(2).all(|w| w[1] < w[0]), "{seen:?}");
        assert!((seen[4] - 0.5).abs() < 0.02, "half way at 2.5 s: {seen:?}");
        e.tick(&show, 0.15);
        assert_eq!(levels(&e, out), vec![(b, 1.0)], "the first gone at its end");
    }
}

#[test]
fn an_overlap_crossfade_held_by_a_free_run_pause_runs_from_the_go() {
    let mut show = Show::default();
    let out = show.outputs[0].id;
    let g = overlap_group(&mut show, true);
    let c = faded_text(&mut show, 4.0, 0.5);
    let a = child(&mut show, g, c, 0.0);
    // The stage manager's hold: a pause at the next clip's start.
    let p = pause(&mut show, PauseFreeze::Nothing);
    child(&mut show, g, p, 2.0);
    let c = faded_text(&mut show, 3.0, 0.0);
    let b = child(&mut show, g, c, 2.0);
    let mut e = Engine::for_tests();
    e.output_sizes.insert(out, (64, 36));
    e.fire(&show, g);
    run_to(&mut e, &show, key(g), 0.05, 2.0, &[a, b]);
    assert_eq!(paused_at(&mut e, &show, key(g)), Some(2.0));
    // A while: nothing moves.
    ticks(&mut e, &show, 0.25, 4);
    assert_eq!(levels(&e, out), vec![(a, 1.0)]);
    e.go(&show);
    until(&mut e, &show, Some(0.0), "the next drawn", 10, |e| {
        started(e, b)
    });
    e.tick(&show, 0.0);
    // The overlap is 2 s (the first's slot runs to 4 s): half way after 1 s.
    e.tick(&show, 1.0);
    let l = levels(&e, out);
    assert_eq!(l.len(), 2, "{l:?}");
    assert!(
        (l[0].1 - 0.5).abs() < 0.02 && (l[1].1 - 0.5).abs() < 0.02,
        "{l:?}"
    );
    e.tick(&show, 1.05);
    assert_eq!(levels(&e, out), vec![(b, 1.0)]);
}

#[test]
fn overlapping_sounds_crossfade_and_a_seek_into_the_overlap_finds_both_part_way() {
    let Some((mut e, wav)) = with_media("tone_5s.wav") else {
        return;
    };
    e.audio = AudioOut::with_playing_device();
    let mut show = Show::default();
    let g = overlap_group(&mut show, true);
    let c = file(&mut show, CueType::Audio, wav.clone(), 3.0);
    let a = child(&mut show, g, c, 0.0);
    let c = file(&mut show, CueType::Audio, wav, 3.0);
    let b = child(&mut show, g, c, 2.0);
    for cue in [a, b] {
        media_of(&mut show, cue).curve = Curve::Linear;
    }
    // The chase's view of 2.5 s: both fading, half way across.
    e.fire(&show, g);
    until(&mut e, &show, Some(0.0), "the first sound", 10, |e| {
        started(e, a)
    });
    e.timeline_seek(&show, key(g), 2.5, true);
    until(&mut e, &show, Some(0.0), "both cued", 10, |e| {
        started(e, a) && started(e, b)
    });
    let (va, vb) = (
        active(&e, a).unwrap().volume.value,
        active(&e, b).unwrap().volume.value,
    );
    let mid = MIN_DB / 2.0;
    assert!(
        (va - mid).abs() < 1.0 && (vb - mid).abs() < 1.0,
        "{va} {vb}"
    );
    assert!(active(&e, a).unwrap().volume.ramping() && active(&e, a).unwrap().stopping);
    // Played through from the top: the same at 2.5 s.
    e.stop_all(0.0);
    e.fire(&show, g);
    until(&mut e, &show, Some(0.0), "the first sound", 10, |e| {
        started(e, a)
    });
    e.tick(&show, 2.0);
    until(&mut e, &show, Some(0.0), "the second sound", 10, |e| {
        started(e, b)
    });
    e.tick(&show, 0.5);
    let (va, vb) = (
        active(&e, a).unwrap().volume.value,
        active(&e, b).unwrap().volume.value,
    );
    assert!(
        (va - mid).abs() < 1.0 && (vb - mid).abs() < 1.0,
        "{va} {vb}"
    );
}

// ----- Gapless joins (P5) ---------------------------------------------------------

/// A timeline group with a text from 0 to 1 s and the next abutting it from
/// 1 s, by `transition` over `fade` seconds; both drawn at full size, so the
/// next takes a few ticks to have its picture.
fn joined_texts(transition: Transition, fade: f64) -> (Show, CueId, CueId, CueId) {
    let mut show = Show::default();
    let g = group(&mut show);
    let c = faded_text(&mut show, 1.0, 0.0);
    let a = child(&mut show, g, c, 0.0);
    let mut c = faded_text(&mut show, 1.0, 0.0);
    let m = c.kind.media_mut().unwrap();
    m.transition = transition;
    m.fade_in = fade;
    let b = child(&mut show, g, c, 1.0);
    (show, g, a, b)
}

#[test]
fn abutting_pictures_never_leave_the_display_empty_across_the_join() {
    for (transition, fade) in [(Transition::Cut, 0.0), (Transition::Crossfade, 0.5)] {
        let (show, g, a, b) = joined_texts(transition, fade);
        let out = show.outputs[0].id;
        let mut e = Engine::for_tests();
        e.fire(&show, g);
        until(&mut e, &show, Some(0.0), "the first drawn", 10, |e| {
            started(e, a)
        });
        let mut waited = false;
        let mut both = false;
        // No time for the next one's drawing between ticks; once it is due,
        // the clock waits for its picture (which takes a few ticks).
        let t0 = Instant::now();
        while view(&mut e, &show, key(g)).pos < 1.6 {
            assert!(t0.elapsed() < Duration::from_secs(20), "never drawn");
            let drawing = active(&e, b).is_some_and(|x| !x.started);
            if drawing {
                std::thread::sleep(Duration::from_millis(1));
            }
            e.tick(&show, if drawing { 0.0 } else { 0.01 });
            let l = levels(&e, out);
            assert!(
                !l.is_empty(),
                "{transition:?}: nothing up at {}",
                view(&mut e, &show, key(g)).pos
            );
            waited |= active(&e, b).is_some_and(|x| !x.started);
            both |= l.len() == 2 && l.iter().any(|x| x.0 == a && x.1 > 0.1 && x.1 < 0.9);
        }
        assert!(waited, "the next clip took a while to have its picture");
        assert_eq!(
            both,
            transition == Transition::Crossfade,
            "crossfaded over the first"
        );
        assert_eq!(levels(&e, out), vec![(b, 1.0)]);
    }
}

#[test]
fn a_picture_held_for_a_join_goes_when_the_next_clip_is_not_coming() {
    let (mut show, g, a, b) = joined_texts(Transition::Cut, 0.0);
    let out = show.outputs[0].id;
    let mut e = Engine::for_tests();
    e.fire(&show, g);
    until(&mut e, &show, Some(0.0), "the first drawn", 10, |e| {
        started(e, a)
    });
    e.tick(&show, 0.95);
    // Disarmed while it waits: it is cancelled and the first goes at its end.
    show.cue_mut(b).unwrap().armed = false;
    e.pending.retain(|p| p.cue_id != b);
    e.tick(&show, 0.1);
    assert!(levels(&e, out).is_empty() && active(&e, a).is_none());
}

#[test]
fn abutting_videos_never_leave_the_display_empty_across_the_join() {
    let Some((mut e, clip)) = with_media("testsrc_10s.mp4") else {
        return;
    };
    probed(&mut e, &clip);
    let mut show = Show::default();
    let out = show.outputs[0].id;
    let g = group(&mut show);
    let c = file(&mut show, CueType::Video, clip.clone(), 1.0);
    let a = child(&mut show, g, c, 0.0);
    let c = file(&mut show, CueType::Video, clip, 1.0);
    let b = child(&mut show, g, c, 1.0);
    e.fire(&show, g);
    until(&mut e, &show, Some(0.0), "the first video", 10, |e| {
        started(e, a)
    });
    let t0 = Instant::now();
    while view(&mut e, &show, key(g)).pos < 1.5 {
        assert!(t0.elapsed() < Duration::from_secs(20), "never started");
        let starting = active(&e, b).is_some_and(|x| !x.started);
        std::thread::sleep(Duration::from_millis(if starting { 2 } else { 0 }));
        e.tick(&show, if starting { 0.0 } else { 0.01 });
        assert!(!e.layers(out).is_empty(), "nothing up");
    }
    assert_eq!(levels(&e, out), vec![(b, 1.0)]);
}

// ----- Jumps got ready ahead (P5) --------------------------------------------------

/// A jump cue to `to`.
fn jump(show: &mut Show, to: TimelinePoint) -> Cue {
    control(
        show,
        ControlCue {
            action: ControlAction::TimelineJump,
            at: Some(to),
            ..Default::default()
        },
    )
}

/// Whether `id` has been got ready at clip time `from`, its first frame decoded.
fn ready_at(e: &mut Engine, id: CueId, from: f64) -> bool {
    e.prepared.iter_mut().any(|p| {
        p.cue_id == id
            && (p.shift - from).abs() < 1e-6
            && match &mut p.kind {
                PreparedKind::Video { player, .. } => player.ready(),
                PreparedKind::Audio(v) => v.ready(),
                _ => false,
            }
    })
}

#[test]
fn a_jump_the_clock_nears_has_what_it_lands_in_ready_and_its_first_frame_comes_at_once() {
    let Some((mut e, clip)) = with_media("testsrc_10s.mp4") else {
        return;
    };
    probed(&mut e, &clip);
    let mut show = Show::default();
    let g = group(&mut show);
    let c = file(&mut show, CueType::Video, clip.clone(), 3.0);
    let first = child(&mut show, g, c, 0.0);
    let mut c = file(&mut show, CueType::Video, clip, 9.0);
    c.kind.media_mut().unwrap().layer = 1;
    let late = child(&mut show, g, c, 4.0);
    let j = jump(&mut show, TimelinePoint::Time(6.0));
    child(&mut show, g, j, 2.0);
    e.fire(&show, g);
    until(&mut e, &show, Some(0.0), "the first video", 10, |e| {
        started(e, first)
    });
    // Within the horizon of the jump: the clip it lands in is got ready 2 s in.
    let t0 = Instant::now();
    while !ready_at(&mut e, late, 2.0) {
        assert!(t0.elapsed() < Duration::from_secs(20), "never got ready");
        std::thread::sleep(Duration::from_millis(5));
        e.tick(&show, 0.0);
    }
    ticks(&mut e, &show, 0.05, 45);
    let v = view(&mut e, &show, key(g));
    assert!(v.pos > 6.0 && v.pos < 6.5, "jumped: {}", v.pos);
    // It plays from where it landed, the clock running on from there, and what
    // was got ready for the jump has been used.
    assert_eq!(v.state, RunState::Running);
    let a = active(&e, late).unwrap();
    assert!(a.started && (a.file_from - 2.0).abs() < 1e-6);
    assert!(
        !e.prepared
            .iter()
            .any(|p| p.cue_id == late && !p.at_in_point())
    );
    // Again, counting the ticks the jump takes: its first frame comes within
    // two (the decoders were ready), the frame of where it landed.
    e.stop_all(0.0);
    e.fire(&show, g);
    until(&mut e, &show, Some(0.0), "the first video", 10, |e| {
        started(e, first)
    });
    let t0 = Instant::now();
    while !ready_at(&mut e, late, 2.0) {
        assert!(t0.elapsed() < Duration::from_secs(20), "never got ready");
        std::thread::sleep(Duration::from_millis(5));
        e.tick(&show, 0.0);
    }
    e.tick(&show, 2.0);
    assert_eq!(state(&mut e, &show, key(g)), RunState::Cueing);
    let mut n = 0;
    while state(&mut e, &show, key(g)) != RunState::Running {
        n += 1;
        assert!(n <= 2, "the jump took {n} ticks");
        std::thread::sleep(Duration::from_millis(3));
        e.tick(&show, 0.0);
    }
    assert!(started(&e, late));
    assert_eq!(shown(&e, late), Some(frame_at(&mut e, &show, late, 2.0)));
}

// ----- A smooth seek (P5) ----------------------------------------------------------

#[test]
fn a_seek_keeps_what_was_up_until_the_new_clips_are_cued_then_changes_all_at_once() {
    let mut show = Show::default();
    let out = show.outputs[0].id;
    let g = group(&mut show);
    let c = faded_text(&mut show, 3.0, 0.0);
    let a = child(&mut show, g, c, 0.0);
    let mut c = faded_text(&mut show, 5.0, 0.0);
    c.kind.media_mut().unwrap().layer = 1;
    let b = child(&mut show, g, c, 4.0);
    let c = faded_text(&mut show, 5.0, 0.0);
    let d = child(&mut show, g, c, 4.0);
    let mut e = Engine::for_tests();
    e.fire(&show, g);
    until(&mut e, &show, Some(0.0), "the first drawn", 10, |e| {
        started(e, a)
    });
    e.tick(&show, 1.0);
    for (to, running) in [(5.0, true), (1.0, true), (6.0, false)] {
        // Running: it cues there; paused: it parks there.
        if !running {
            e.timeline_pause(&show, key(g), PauseFreeze::Everything, &[], false, true);
        }
        e.timeline_seek(&show, key(g), to, true);
        let new: Vec<CueId> = if to > 4.0 { vec![d, b] } else { vec![a] };
        let t0 = Instant::now();
        // Drawing at full size: no time for it between the first ticks.
        let mut n = 0;
        loop {
            assert!(t0.elapsed() < Duration::from_secs(20), "never cued");
            let l: Vec<CueId> = levels(&e, out).into_iter().map(|x| x.0).collect();
            assert!(!l.is_empty(), "nothing up after the seek to {to}");
            let ready = new.iter().all(|id| started(&e, *id));
            if l.iter().any(|id| new.contains(id)) {
                // Once anything new is seen, all of it is, and only it.
                assert_eq!(l.len(), new.len(), "{to}: {l:?}");
                assert!(new.iter().all(|id| l.contains(id)), "{to}: {l:?}");
                break;
            }
            // Seen on the tick the last has come.
            assert!(!ready, "{to}: cued but not seen: {l:?}");
            n += 1;
            if n > 2 {
                std::thread::sleep(Duration::from_millis(1));
            }
            e.tick(&show, 0.0);
        }
        assert!(
            n > 0,
            "the new clips took a moment (the old stayed meanwhile)"
        );
        if running {
            until(&mut e, &show, Some(0.0), "running", 10, |e| {
                e.runs
                    .map
                    .get(&key(g))
                    .is_some_and(|r| r.state == RunState::Running)
            });
        }
    }
}

#[test]
fn a_video_alive_on_both_sides_of_a_seek_is_jumped_where_it_is() {
    let Some((mut e, clip)) = with_media("testsrc_10s.mp4") else {
        return;
    };
    probed(&mut e, &clip);
    let mut show = Show::default();
    let out = show.outputs[0].id;
    let g = group(&mut show);
    let c = file(&mut show, CueType::Video, clip, 9.0);
    let v = child(&mut show, g, c, 0.0);
    e.fire(&show, g);
    until(&mut e, &show, Some(0.0), "the video", 10, |e| started(e, v));
    ticks(&mut e, &show, 0.1, 5);
    let before = instance(&e, v);
    e.timeline_seek(&show, key(g), 5.0, true);
    assert_eq!(state(&mut e, &show, key(g)), RunState::Cueing);
    // The same cue, jumping: up all the while, its new decoders swapped in.
    assert!(active(&e, v).is_some_and(|a| a.seeking() && a.hold));
    until(&mut e, &show, Some(0.0), "running", 10, |e| {
        assert_eq!(e.layers(out).len(), 1, "up all the while");
        e.runs
            .map
            .get(&key(g))
            .is_some_and(|r| r.state == RunState::Running)
    });
    let a = active(&e, v).unwrap();
    assert_eq!(a.instance, before, "not started again");
    assert!(!a.seeking() && !a.hold && (a.file_from - 5.0).abs() < 1e-6);
    assert!((a.elapsed - 5.0).abs() < 1e-6);
    assert_eq!(shown(&e, v), Some(frame_at(&mut e, &show, v, 5.0)));
    ticks(&mut e, &show, 0.1, 3);
    assert!(elapsed(&e, v) > 5.0 && view(&mut e, &show, key(g)).pos > 5.0);
    // A seek back while it is held at a pause of the transport: the same.
    e.timeline_pause(&show, key(g), PauseFreeze::Everything, &[], false, true);
    e.timeline_seek(&show, key(g), 2.0, true);
    until(&mut e, &show, Some(0.0), "the jump", 10, |e| {
        active(e, v).is_some_and(|a| !a.seeking())
    });
    assert_eq!(instance(&e, v), before);
    assert!(parked(&mut e, &show, key(g)) && active(&e, v).unwrap().hold);
    assert_eq!(shown(&e, v), Some(frame_at(&mut e, &show, v, 2.0)));
}

// ----- Going round again when it runs out (P5, plan 2.4) ---------------------------

#[test]
fn a_clip_that_leaves_the_clock_at_run_time_set_to_loop_goes_round_from_its_in_point() {
    let Some((mut e, clip)) = with_media("testsrc_10s.mp4") else {
        return;
    };
    probed(&mut e, &clip);
    let mut show = Show::default();
    let out = show.outputs[0].id;
    let g = group(&mut show);
    // A one-second clip with no pause in it: not clocked, not played as a loop.
    let c = file(&mut show, CueType::Video, clip, 1.0);
    let v = child(&mut show, g, c, 0.0);
    media_of(&mut show, v).run_out = RunOut::Loop;
    e.fire(&show, g);
    until(&mut e, &show, Some(0.0), "the video", 10, |e| started(e, v));
    ticks(&mut e, &show, 0.05, 6);
    assert!(active(&e, v).is_some_and(|a| !a.clocked && a.pass_len.is_none()));
    // Paused from elsewhere (the editor, another list's cue), content running on.
    e.timeline_pause(&show, key(g), PauseFreeze::Nothing, &[], false, true);
    assert!(active(&e, v).is_some_and(|a| a.detached && a.clocked));
    let (mut rounds, mut last, mut ready_before) = (0, elapsed(&e, v), false);
    let mut frame = shown(&e, v);
    let t0 = Instant::now();
    while rounds < 2 {
        assert!(
            t0.elapsed() < Duration::from_secs(20),
            "it never went round twice"
        );
        ready_before |= rounds == 0 && e.prepared.iter().any(|p| p.cue_id == v && p.at_in_point());
        e.tick(&show, 0.05);
        std::thread::sleep(Duration::from_millis(3));
        assert_eq!(e.layers(out).len(), 1, "up all the while");
        let now = elapsed(&e, v);
        if now < last - 0.5 {
            rounds += 1;
            // From its in point: off its last frame, the file from its start.
            assert!(now < 0.2, "{now}");
            assert!(shown(&e, v) != frame && active(&e, v).unwrap().file_from == 0.0);
            assert!(!active(&e, v).unwrap().held);
        }
        last = now;
        frame = shown(&e, v);
    }
    assert!(ready_before, "got ready as its end neared");
    assert_eq!(paused_at(&mut e, &show, key(g)), Some(0.3));
    // GO: the clock ends it where its slot ends, 0.7 s on.
    e.go(&show);
    e.tick(&show, 0.65);
    assert!(active(&e, v).is_some());
    e.tick(&show, 0.1);
    e.tick(&show, 0.0);
    assert!(active(&e, v).is_none());
}

#[test]
fn a_clip_with_a_keyframed_speed_set_to_loop_goes_round_through_a_free_run_pause() {
    let Some((mut e, clip)) = with_media("testsrc_10s.mp4") else {
        return;
    };
    probed(&mut e, &clip);
    let (mut show, g, v) = run_out_show(CueType::Video, clip, 1.0, 0.0, 0.5, RunOut::Loop, None);
    media_of(&mut show, v).rate = ramp(&[(0.0, 1.0), (0.4, 2.0)]);
    e.fire(&show, g);
    until(&mut e, &show, Some(0.0), "the video", 10, |e| started(e, v));
    let a = active(&e, v).unwrap();
    assert!(
        a.clocked && a.speed.is_some() && a.loops_forever(),
        "played as a loop"
    );
    until(&mut e, &show, Some(0.05), "three passes", 20, |e| {
        active(e, v).is_some_and(|a| a.pass_number() >= Some(3))
    });
    assert_eq!(paused_at(&mut e, &show, key(g)), Some(0.5));
    let a = active(&e, v).unwrap();
    assert!(!a.held && !a.stopping && a.opacity.value == 1.0);
    e.go(&show);
    until(&mut e, &show, Some(0.05), "the clock to end it", 10, |e| {
        active(e, v).is_none()
    });
}

// ----- Loop this section (P5, ruling L3) --------------------------------------------

use crate::engine::GoPlan;

/// A loop pause going back to `from`, leaving at once or not.
fn loop_pause(show: &mut Show, from: TimelinePoint, at_once: bool) -> Cue {
    control(
        show,
        ControlCue {
            action: ControlAction::TimelinePause,
            loop_from: Some(from),
            leave_at_once: at_once,
            ..Default::default()
        },
    )
}

/// A group: a text all through (0 to 10 s), one inside the section (2 to 4 s),
/// a loop at 5 s back to 1 s, and the next text at 5 s.
fn looped_show(at_once: bool) -> (Show, CueId, [CueId; 4]) {
    let mut show = Show::default();
    let g = group(&mut show);
    let c = faded_text(&mut show, 10.0, 0.0);
    let bed = child(&mut show, g, c, 0.0);
    let mut c = faded_text(&mut show, 2.0, 0.0);
    c.kind.media_mut().unwrap().layer = 1;
    let inside = child(&mut show, g, c, 2.0);
    let l = loop_pause(&mut show, TimelinePoint::Time(1.0), at_once);
    let l = child(&mut show, g, l, 5.0);
    let mut c = faded_text(&mut show, 2.0, 0.0);
    c.kind.media_mut().unwrap().layer = 2;
    let after = child(&mut show, g, c, 5.0);
    (show, g, [bed, inside, l, after])
}

/// Tick by `dt` (waiting with 0-ticks while a clip is being drawn or the run
/// cues) until `cond`; the clock positions seen, in order.
fn run_while(
    e: &mut Engine,
    show: &Show,
    k: TimelineKey,
    dt: f64,
    mut cond: impl FnMut(&mut Engine) -> bool,
) -> Vec<f64> {
    let t0 = Instant::now();
    let mut seen = Vec::new();
    while cond(e) {
        assert!(t0.elapsed() < Duration::from_secs(30), "timed out");
        let busy =
            e.active.iter().any(|a| !a.started) || view(e, show, k).state == RunState::Cueing;
        if busy {
            std::thread::sleep(Duration::from_millis(1));
        }
        e.tick(show, if busy { 0.0 } else { dt });
        seen.push(view(e, show, k).pos);
    }
    seen
}

/// How many times the clock went back.
fn jumps_back(seen: &[f64]) -> usize {
    seen.windows(2).filter(|w| w[1] < w[0] - 1e-9).count()
}

/// Run until the clock has gone back `n` times and the run is running again:
/// the clock positions seen.
fn rounds(e: &mut Engine, show: &Show, k: TimelineKey, n: usize) -> Vec<f64> {
    let mut seen = vec![view(e, show, k).pos];
    run_while(e, show, k, 0.1, |e| {
        seen.push(view(e, show, k).pos);
        jumps_back(&seen) < n || view(e, show, k).state != RunState::Running
    });
    seen
}

#[test]
fn a_loop_plays_its_section_again_until_go_and_go_finishes_the_pass_and_carries_on() {
    let (show, g, [bed, inside, l, after]) = looped_show(false);
    let k = key(g);
    let out = show.outputs[0].id;
    let mut e = Engine::for_tests();
    e.output_sizes.insert(out, (64, 36));
    e.fire(&show, g);
    // Three times round: back to 1 s each time it reaches 5 s.
    let seen = rounds(&mut e, &show, k, 3);
    assert_eq!(jumps_back(&seen), 3);
    assert!(
        seen.iter().filter(|p| **p > 4.5).count() >= 3,
        "each time to the end"
    );
    assert!(seen.iter().all(|p| *p <= 5.0 + 1e-9), "never past the loop");
    assert!(active(&e, after).is_none() && waiting(&e, after));
    let v = view(&mut e, &show, k);
    assert_eq!(v.state, RunState::Running, "the clock never stopped");
    let lp = v.looping.expect("looping");
    assert!(lp.cue == l && lp.from == 1.0 && lp.to == 5.0 && !lp.at_once && !lp.last_pass);
    // GO says what it will do, and does it on the pass playing.
    let (does, then) = e.go_plan(&show).describe(&show).unwrap();
    assert!(does.starts_with("Finish this pass and continue"), "{does}");
    assert!(then.is_some());
    run_while(&mut e, &show, k, 0.1, |e| view(e, &show, k).pos < 3.0);
    assert!(active(&e, inside).is_some() && active(&e, bed).is_some());
    e.go(&show);
    assert!(view(&mut e, &show, k).looping.unwrap().last_pass);
    assert!(
        matches!(e.go_plan(&show), GoPlan::Cue(_)),
        "GO again is for the next cue"
    );
    // On to 6 s: past the cue without going back, without a stop.
    let seen = run_while(&mut e, &show, k, 0.1, |e| {
        assert_ne!(view(e, &show, k).state, RunState::Stopped);
        assert!(!matches!(view(e, &show, k).state, RunState::Paused { .. }));
        view(e, &show, k).pos < 6.0
    });
    assert_eq!(jumps_back(&seen), 0);
    let v = view(&mut e, &show, k);
    assert!(v.looping.is_none() && v.state == RunState::Running);
    assert!(active(&e, after).is_some(), "what comes after it plays");
    assert!(e.take_problems().is_empty());
}

#[test]
fn a_loop_set_to_leave_at_once_leaves_on_go_and_goes_on_from_the_cue() {
    let (show, g, [_, inside, _, after]) = looped_show(true);
    let k = key(g);
    let mut e = Engine::for_tests();
    e.output_sizes.insert(show.outputs[0].id, (64, 36));
    e.fire(&show, g);
    let seen = rounds(&mut e, &show, k, 1);
    assert!(seen.last().is_some_and(|p| *p < 2.0), "went round");
    run_while(&mut e, &show, k, 0.1, |e| view(e, &show, k).pos < 3.0);
    let (does, _) = e.go_plan(&show).describe(&show).unwrap();
    assert!(does.starts_with("Leave the loop"), "{does}");
    assert!(active(&e, inside).is_some());
    e.go(&show);
    // At once to the cue's place: it has been passed there, so it goes on.
    let v = view(&mut e, &show, k);
    assert!(v.pos == 5.0 && v.looping.is_none(), "{v:?}");
    let seen = run_while(&mut e, &show, k, 0.1, |e| view(e, &show, k).pos < 6.0);
    assert_eq!(jumps_back(&seen), 0);
    assert!(active(&e, after).is_some() && active(&e, inside).is_none());
}

#[test]
fn a_pause_inside_a_loop_still_stops_it_and_a_seek_out_of_it_ends_the_loop() {
    let (mut show, g, [bed, ..]) = looped_show(false);
    let p = pause(&mut show, PauseFreeze::Nothing);
    child(&mut show, g, p, 3.0);
    let k = key(g);
    let mut e = Engine::for_tests();
    e.output_sizes.insert(show.outputs[0].id, (64, 36));
    e.fire(&show, g);
    // Twice round, each time held at 3 s until GO.
    for round in 0..2 {
        run_while(&mut e, &show, k, 0.1, |e| {
            !matches!(view(e, &show, k).state, RunState::Paused { .. })
        });
        assert_eq!(paused_at(&mut e, &show, k), Some(3.0), "round {round}");
        assert_eq!(view(&mut e, &show, k).looping.is_some(), round > 0);
        // GO at the pause runs it on (it doesn't leave the loop).
        let (does, _) = e.go_plan(&show).describe(&show).unwrap();
        assert!(does.starts_with("Continue"), "{does}");
        e.go(&show);
        assert_eq!(state(&mut e, &show, k), RunState::Running);
        run_while(&mut e, &show, k, 0.1, |e| view(e, &show, k).pos > 2.0);
    }
    assert!(view(&mut e, &show, k).looping.is_some());
    // A seek in the section keeps it looping; one out of it ends the loop.
    e.timeline_seek(&show, k, 4.0, true);
    assert!(view(&mut e, &show, k).looping.is_some());
    e.timeline_seek(&show, k, 7.0, true);
    assert!(view(&mut e, &show, k).looping.is_none());
    let seen = run_while(&mut e, &show, k, 0.1, |e| view(e, &show, k).pos < 8.0);
    assert_eq!(jumps_back(&seen), 0);
    assert!(active(&e, bed).is_some());
}

#[test]
fn a_timeline_stop_during_a_loop_fades_it_out() {
    let (show, g, [bed, ..]) = looped_show(false);
    let k = key(g);
    let mut e = Engine::for_tests();
    e.output_sizes.insert(show.outputs[0].id, (64, 36));
    e.fire(&show, g);
    let seen = rounds(&mut e, &show, k, 1);
    assert!(seen.last().is_some_and(|p| *p < 2.0));
    e.timeline_stop(&show, k, 1.0);
    let v = view(&mut e, &show, k);
    assert!(v.state == RunState::Stopped && v.looping.is_none() && v.fading);
    e.tick(&show, 0.5);
    assert!(opacity(&e, bed).is_some_and(|o| (o - 0.5).abs() < 0.05));
    e.tick(&show, 0.6);
    assert!(active(&e, bed).is_none() && !e.is_active());
}

#[test]
fn a_loops_way_back_is_got_ready_and_comes_round_within_a_tick_or_two() {
    let Some((mut e, clip)) = with_media("testsrc_10s.mp4") else {
        return;
    };
    probed(&mut e, &clip);
    let mut show = Show::default();
    let g = group(&mut show);
    // A video from 1 s to 4 s; the section from 1.5 s to 3 s.
    let c = file(&mut show, CueType::Video, clip, 3.0);
    let v = child(&mut show, g, c, 1.0);
    let l = loop_pause(&mut show, TimelinePoint::Time(1.5), false);
    child(&mut show, g, l, 3.0);
    let k = key(g);
    e.fire(&show, g);
    until(&mut e, &show, Some(0.05), "the video", 10, |e| {
        started(e, v)
    });
    // Got ready 0.5 s in, where the loop goes back to.
    let t0 = Instant::now();
    while !ready_at(&mut e, v, 0.5) {
        assert!(t0.elapsed() < Duration::from_secs(20), "never got ready");
        std::thread::sleep(Duration::from_millis(5));
        e.tick(&show, 0.0);
    }
    for round in 0..2 {
        run_while(&mut e, &show, k, 0.05, |e| view(e, &show, k).pos < 2.9);
        e.tick(&show, 0.1);
        assert_eq!(view(&mut e, &show, k).pos, 1.5, "round {round}");
        let mut n = 0;
        while state(&mut e, &show, k) != RunState::Running {
            n += 1;
            assert!(n <= 3, "round {round}: the jump took {n} ticks");
            std::thread::sleep(Duration::from_millis(3));
            e.tick(&show, 0.0);
        }
        let a = active(&e, v).unwrap();
        assert!(
            !a.seeking() && (a.file_from - 0.5).abs() < 1e-6,
            "round {round}"
        );
        // Its elapsed time (the active panel's number) is its place on the
        // clock, as its bar is: 0.5 s in, at clock 1.5 s.
        let elapsed = a.elapsed;
        let pos = view(&mut e, &show, k).pos;
        assert!(
            (elapsed - (pos - 1.0)).abs() < 0.11,
            "round {round}: elapsed {elapsed} at clock {pos}"
        );
        // Ready again for the next time round.
        let t0 = Instant::now();
        while !ready_at(&mut e, v, 0.5) {
            assert!(
                t0.elapsed() < Duration::from_secs(20),
                "never got ready again"
            );
            std::thread::sleep(Duration::from_millis(5));
            e.tick(&show, 0.0);
        }
    }
}

/// The active panel's numbers (`elapsed`, and what is left of `duration`)
/// and its bar follow the clip's place on the clock after a loop goes back,
/// as they do before it: a clip from 0 s at clock 2.5 s says 0:02.5.
#[test]
fn after_a_loop_goes_back_a_clips_elapsed_time_is_its_place_on_the_clock() {
    let (show, g, [bed, inside, _, _]) = looped_show(false);
    let k = key(g);
    let mut e = Engine::for_tests();
    e.output_sizes.insert(show.outputs[0].id, (64, 36));
    e.fire(&show, g);
    rounds(&mut e, &show, k, 1);
    run_while(&mut e, &show, k, 0.1, |e| view(e, &show, k).pos < 2.5);
    let pos = view(&mut e, &show, k).pos;
    let start = |id| show.cue(id).unwrap().pre_wait;
    for id in [bed, inside] {
        let a = active(&e, id).unwrap();
        assert!(
            (a.elapsed - (pos - start(id))).abs() < 0.11,
            "clip at {}: elapsed {} at clock {pos}",
            start(id),
            a.elapsed
        );
        assert!((a.key_time() - (pos - start(id))).abs() < 1e-6);
        let d = a.duration.unwrap();
        assert!(d - a.elapsed > 0.0 && a.elapsed / d <= 1.0);
    }
    // Round again: still the clock's.
    rounds(&mut e, &show, k, 1);
    let pos = view(&mut e, &show, k).pos;
    let a = active(&e, bed).unwrap();
    assert!((a.elapsed - pos).abs() < 0.11, "{} at {pos}", a.elapsed);
}

#[test]
fn go_on_a_timeline_list_that_loops_finishes_the_pass() {
    let mut show = Show::default();
    let l = timeline_list(&mut show);
    let c = faded_text(&mut show, 10.0, 0.0);
    top(&mut show, c, l, 0.0);
    let lp = loop_pause(&mut show, TimelinePoint::Time(1.0), false);
    top(&mut show, lp, l, 3.0);
    let k = TimelineKey::List(l);
    let mut e = Engine::for_tests();
    e.output_sizes.insert(show.outputs[0].id, (64, 36));
    e.active_list = l;
    e.go(&show);
    rounds(&mut e, &show, k, 2);
    assert!(matches!(
        e.go_plan(&show),
        GoPlan::Timeline {
            does: TimelineGo::LeaveLoop { at_once: false },
            ..
        }
    ));
    e.go(&show);
    assert!(view(&mut e, &show, k).looping.unwrap().last_pass);
    // GO again: the timeline is running, it does nothing.
    assert!(matches!(
        e.go_plan(&show),
        GoPlan::Timeline {
            does: TimelineGo::Running,
            ..
        }
    ));
    let seen = run_while(&mut e, &show, k, 0.1, |e| view(e, &show, k).pos < 4.0);
    assert_eq!(jumps_back(&seen), 0);
    assert!(view(&mut e, &show, k).looping.is_none());
}

// ----- What GO says on a timeline list, in each state (I2) ----------------------

#[test]
fn go_plan_reads_out_a_timeline_list_in_each_state_and_does_nothing_at_its_end() {
    use crate::engine::TimelineGo;
    let mut show = Show::default();
    let l = timeline_list(&mut show);
    let c = faded_text(&mut show, 6.0, 0.0);
    let bed = top(&mut show, c, l, 0.0);
    let p = pause(&mut show, PauseFreeze::Nothing);
    top(&mut show, p, l, 1.0);
    let lp = loop_pause(&mut show, TimelinePoint::Time(2.0), false);
    top(&mut show, lp, l, 3.0);
    let k = TimelineKey::List(l);
    let mut e = Engine::for_tests();
    e.output_sizes.insert(show.outputs[0].id, (64, 36));
    e.active_list = l;
    // Stopped, never run.
    let (does, text) = said(&e, &show);
    assert_eq!(does, Some(TimelineGo::Start), "{text}");
    assert!(text.starts_with("Run Timeline"), "{text}");
    // Running.
    e.go(&show);
    e.tick(&show, 0.0);
    let (does, text) = said(&e, &show);
    assert_eq!(does, Some(TimelineGo::Running), "{text}");
    assert!(
        text.contains("is running") && text.contains("does nothing"),
        "{text}"
    );
    // At the pause.
    run_while(&mut e, &show, k, 0.1, |e| {
        !matches!(view(e, &show, k).state, RunState::Paused { .. })
    });
    let (does, text) = said(&e, &show);
    assert_eq!(does, Some(TimelineGo::Resume), "{text}");
    assert!(text.starts_with("Continue Timeline"), "{text}");
    // Looping.
    e.go(&show);
    rounds(&mut e, &show, k, 1);
    let (does, text) = said(&e, &show);
    assert_eq!(
        does,
        Some(TimelineGo::LeaveLoop { at_once: false }),
        "{text}"
    );
    assert!(text.starts_with("Finish this pass"), "{text}");
    // Parked by a seek of the stopped timeline.
    e.timeline_stop(&show, k, 0.0);
    e.timeline_seek(&show, k, 0.5, true);
    e.tick(&show, 0.0);
    let (does, text) = said(&e, &show);
    assert_eq!(does, Some(TimelineGo::Play), "{text}");
    assert!(text.contains("from its playhead"), "{text}");
    // Stopping (a Stop's fade): GO runs it again from where it stands.
    e.timeline_run(&show, k);
    run_while(&mut e, &show, k, 0.05, |e| {
        view(e, &show, k).state != RunState::Running
    });
    e.timeline_stop(&show, k, 2.0);
    assert!(view(&mut e, &show, k).fading);
    let (does, text) = said(&e, &show);
    assert_eq!(does, Some(TimelineGo::Start), "{text}");
    // Played to its end: a seek past the loop, then run out.
    e.timeline_stop(&show, k, 0.0);
    e.timeline_seek(&show, k, 3.5, true);
    e.timeline_run(&show, k);
    run_while(&mut e, &show, k, 0.1, |e| {
        view(e, &show, k).state != RunState::Stopped
    });
    assert!(active(&e, bed).is_none());
    assert!(view(&mut e, &show, k).at_end);
    let (does, text) = said(&e, &show);
    assert_eq!(does, Some(TimelineGo::AtEnd), "{text}");
    assert!(
        text.contains("is at its end") && text.contains("does nothing"),
        "{text}"
    );
    e.go(&show);
    assert_eq!(state(&mut e, &show, k), RunState::Stopped, "GO did nothing");
    // A clip added after the playhead: it can run again.
    let late = super::tests::text(&mut show, Some(1.0));
    top(&mut show, late, l, 9.0);
    assert_eq!(said(&e, &show).0, Some(TimelineGo::Start));
}

/// What GO does and the words for it (`GoPlan::describe`, the web remote's too).
fn said(e: &Engine, show: &Show) -> (Option<crate::engine::TimelineGo>, String) {
    let plan = e.go_plan(show);
    let does = match plan {
        crate::engine::GoPlan::Timeline { does, .. } => Some(does),
        _ => None,
    };
    let (what, then) = plan.describe(show).unwrap();
    (does, format!("{what} / {}", then.unwrap_or_default()))
}
