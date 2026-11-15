//! A cue on several displays: one instance on each, replaced on one display
//! alone, over when it is on none. Text cues need no media; the timeline's
//! cases (a pause holding a crossfade on one display, a seek) use them too.

use super::super::*;
use crate::show::{ControlCue, CueType, GroupMode, PauseFreeze, Transition};
use std::time::{Duration, Instant};

/// A show with two displays.
fn two_displays() -> (Show, OutputId, OutputId) {
    let mut show = Show::default();
    let d1 = show.outputs[0].id;
    let d2 = show.add_display();
    (show, d1, d2)
}

/// A text cue on `home` (and `also`), on `layer`, coming in by `transition`
/// over `fade_in` (linear), held until stopped. Not yet in the show.
fn text_on(
    show: &mut Show,
    home: OutputId,
    also: &[OutputId],
    layer: i32,
    transition: Transition,
    fade_in: f64,
) -> Cue {
    let mut c = show.make_cue(CueType::Text, None, show.cues.len());
    if let CueKind::Text(t) = &mut c.kind {
        t.text = "Hello".into();
        let m = &mut t.media;
        m.output = Some(home);
        m.also_on = also.to_vec();
        m.layer = layer;
        m.transition = transition;
        m.fade_in = fade_in;
        m.fade_out = 0.0;
        m.end = None;
        m.curve = Curve::Linear;
    }
    c
}

/// `text_on`, pushed at the end of the show.
fn text(
    show: &mut Show,
    home: OutputId,
    also: &[OutputId],
    layer: i32,
    transition: Transition,
    fade_in: f64,
) -> CueId {
    let c = text_on(show, home, also, layer, transition, fade_in);
    let id = c.id;
    show.cues.push(c);
    id
}

/// Tick without moving time until every picture playing has been drawn.
fn drawn(e: &mut Engine, show: &Show) {
    let t0 = Instant::now();
    loop {
        e.tick(show, 0.0);
        let pending = e.pending.iter().any(|p| p.run.is_none());
        if !pending && e.active.iter().all(|a| a.started || a.is_matte()) {
            return;
        }
        assert!(t0.elapsed() < Duration::from_secs(10), "nothing was drawn");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// (cue, opacity) on the display, bottom to top.
fn on(e: &Engine, out: OutputId) -> Vec<(CueId, f32)> {
    e.layers(out)
        .iter()
        .map(|l| {
            let a = e.active.iter().find(|a| a.instance == l.instance).unwrap();
            (a.cue_id, l.opacity)
        })
        .collect()
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

fn active(e: &Engine, id: CueId) -> Option<&ActiveCue> {
    e.active.iter().find(|a| a.cue_id == id)
}

fn fire(e: &mut Engine, show: &Show, id: CueId) {
    e.fire(show, id);
    drawn(e, show);
}

#[test]
fn a_cue_also_on_another_display_is_on_both_with_one_instance() {
    let (mut show, d1, d2) = two_displays();
    let d3 = show.add_display();
    let a = text(&mut show, d1, &[d2], 0, Transition::Cut, 0.0);
    let mut e = Engine::for_tests();
    fire(&mut e, &show, a);
    let (one, two) = (e.layers(d1), e.layers(d2));
    assert_eq!((one.len(), two.len()), (1, 1));
    assert_eq!(one[0].instance, two[0].instance, "one instance on both");
    assert!(e.layers(d3).is_empty());
    assert_eq!(e.active.len(), 1);
}

#[test]
fn a_display_listed_that_the_show_lacks_or_that_is_its_home_is_ignored() {
    let (mut show, d1, d2) = two_displays();
    let a = text(
        &mut show,
        d1,
        &[OutputId(9999), d1, d2, d2],
        0,
        Transition::Cut,
        0.0,
    );
    let mut e = Engine::for_tests();
    fire(&mut e, &show, a);
    assert_eq!(active(&e, a).unwrap().showing(), vec![d1, d2]);
    assert_eq!(on(&e, d1).len(), 1);
    assert!(e.layers(OutputId(9999)).is_empty());
}

#[test]
fn a_cue_on_one_display_is_replaced_as_it_always_was() {
    // An old show's cue (no `also_on`): a cut takes it away, a crossfade fades
    // its level, as before.
    let (mut show, d1, _) = two_displays();
    let a = text(&mut show, d1, &[], 0, Transition::Cut, 0.0);
    let b = text(&mut show, d1, &[], 0, Transition::Crossfade, 2.0);
    let c = text(&mut show, d1, &[], 0, Transition::Cut, 0.0);
    let mut e = Engine::for_tests();
    fire(&mut e, &show, a);
    fire(&mut e, &show, b);
    e.tick(&show, 1.0);
    let old = active(&e, a).unwrap();
    assert!(
        old.stopping && near(old.opacity.value, 0.5),
        "its own level fades"
    );
    fire(&mut e, &show, c);
    assert_eq!(on(&e, d1), vec![(c, 1.0)]);
    assert!(active(&e, a).is_none() && active(&e, b).is_none());
}

#[test]
fn a_cut_on_one_display_replaces_the_shared_cue_there_only_and_it_runs_on() {
    let (mut show, d1, d2) = two_displays();
    let a = text(&mut show, d1, &[d2], 0, Transition::Cut, 0.0);
    let b = text(&mut show, d2, &[], 0, Transition::Cut, 0.0);
    let mut e = Engine::for_tests();
    fire(&mut e, &show, a);
    e.tick(&show, 1.0);
    fire(&mut e, &show, b);
    assert_eq!(on(&e, d1), vec![(a, 1.0)], "display 1 is untouched");
    assert_eq!(on(&e, d2), vec![(b, 1.0)]);
    let before = active(&e, a).unwrap().elapsed;
    e.tick(&show, 1.0);
    let shared = active(&e, a).unwrap();
    assert!(!shared.stopping && near(shared.opacity.value, 1.0));
    assert!(shared.elapsed > before, "it keeps running");
    assert_eq!(on(&e, d1), vec![(a, 1.0)]);
    // Replaced on its last display, it is over.
    let c = text(&mut show, d1, &[], 0, Transition::Cut, 0.0);
    fire(&mut e, &show, c);
    assert!(active(&e, a).is_none(), "on no display now");
    assert_eq!((on(&e, d1), on(&e, d2)), (vec![(c, 1.0)], vec![(b, 1.0)]));
}

#[test]
fn a_crossfade_on_one_display_fades_the_shared_cue_there_alone() {
    let (mut show, d1, d2) = two_displays();
    let a = text(&mut show, d1, &[d2], 0, Transition::Cut, 0.0);
    let b = text(&mut show, d2, &[], 0, Transition::Crossfade, 2.0);
    let mut e = Engine::for_tests();
    fire(&mut e, &show, a);
    fire(&mut e, &show, b);
    e.tick(&show, 1.0);
    let two = on(&e, d2);
    assert_eq!(two.len(), 2);
    assert!(two[0].0 == a && near(two[0].1, 0.5), "{two:?}");
    assert!(two[1].0 == b && near(two[1].1, 0.5), "{two:?}");
    assert_eq!(on(&e, d1), vec![(a, 1.0)], "full on display 1");
    let shared = active(&e, a).unwrap();
    assert!(!shared.stopping && near(shared.opacity.value, 1.0));
    e.tick(&show, 1.1);
    assert_eq!(on(&e, d2), vec![(b, 1.0)]);
    assert_eq!(on(&e, d1), vec![(a, 1.0)]);
    // A crossfade on its last display fades it out as any cue (and its sound).
    let c = text(&mut show, d1, &[], 0, Transition::Crossfade, 1.0);
    fire(&mut e, &show, c);
    e.tick(&show, 0.5);
    let shared = active(&e, a).unwrap();
    assert!(shared.stopping && near(shared.opacity.value, 0.5));
    e.tick(&show, 0.6);
    e.tick(&show, 0.0);
    assert!(active(&e, a).is_none());
}

#[test]
fn crossfades_on_both_displays_at_once_end_it_when_the_longer_is_done() {
    let (mut show, d1, d2) = two_displays();
    let a = text(&mut show, d1, &[d2], 0, Transition::Cut, 0.0);
    let b = text(&mut show, d2, &[], 0, Transition::Crossfade, 3.0);
    let c = text(&mut show, d1, &[], 0, Transition::Crossfade, 1.0);
    let mut e = Engine::for_tests();
    fire(&mut e, &show, a);
    fire(&mut e, &show, b);
    e.tick(&show, 0.5);
    fire(&mut e, &show, c);
    // Fully on no display: its sound goes with the longer fade (2.5 s left).
    let shared = active(&e, a).unwrap();
    assert!(shared.stopping);
    assert!((shared.volume.left() - 2.5).abs() < 1e-6);
    e.tick(&show, 1.5);
    assert!(on(&e, d1).iter().all(|l| l.0 != a), "done on display 1");
    let two = on(&e, d2);
    assert!(
        two.iter().any(|l| l.0 == a && near(l.1, 1.0 / 3.0)),
        "{two:?}"
    );
    e.tick(&show, 1.1);
    assert!(active(&e, a).is_none());
}

#[test]
fn a_higher_layer_on_one_display_changes_nothing_on_the_other() {
    let (mut show, d1, d2) = two_displays();
    let a = text(&mut show, d1, &[d2], 0, Transition::Cut, 0.0);
    let h = text(&mut show, d2, &[], 3, Transition::Cut, 0.0);
    let mut e = Engine::for_tests();
    fire(&mut e, &show, a);
    let before: Vec<u64> = e.layers(d1).iter().map(|l| l.instance).collect();
    fire(&mut e, &show, h);
    e.tick(&show, 1.0);
    let after: Vec<u64> = e.layers(d1).iter().map(|l| l.instance).collect();
    assert_eq!(before, after);
    assert_eq!(on(&e, d1), vec![(a, 1.0)]);
    assert_eq!(
        on(&e, d2),
        vec![(a, 1.0), (h, 1.0)],
        "covered there, not replaced"
    );
}

#[test]
fn without_own_layers_a_cue_entering_takes_everything_from_its_display_only() {
    let (mut show, d1, d2) = two_displays();
    show.own_layers = false;
    let a = text(&mut show, d1, &[d2], 0, Transition::Cut, 0.0);
    let b = text(&mut show, d2, &[], 5, Transition::Cut, 0.0);
    let c = text(&mut show, d1, &[], 9, Transition::Cut, 0.0);
    let mut e = Engine::for_tests();
    fire(&mut e, &show, a);
    fire(&mut e, &show, b);
    assert_eq!(on(&e, d2), vec![(b, 1.0)], "another layer, all the same");
    assert_eq!(on(&e, d1), vec![(a, 1.0)]);
    fire(&mut e, &show, c);
    assert!(active(&e, a).is_none());
    assert_eq!(on(&e, d1), vec![(c, 1.0)]);
}

#[test]
fn a_shared_cue_entering_replaces_on_each_of_its_displays() {
    let (mut show, d1, d2) = two_displays();
    let x = text(&mut show, d1, &[], 0, Transition::Cut, 0.0);
    let y = text(&mut show, d2, &[], 0, Transition::Cut, 0.0);
    let a = text(&mut show, d1, &[d2], 0, Transition::Cut, 0.0);
    let mut e = Engine::for_tests();
    fire(&mut e, &show, x);
    fire(&mut e, &show, y);
    fire(&mut e, &show, a);
    assert_eq!((on(&e, d1), on(&e, d2)), (vec![(a, 1.0)], vec![(a, 1.0)]));
    assert_eq!(e.active.len(), 1);
}

#[test]
fn a_fade_cue_and_a_stop_act_on_every_display() {
    let (mut show, d1, d2) = two_displays();
    let a = text(&mut show, d1, &[d2], 0, Transition::Cut, 0.0);
    let mut f = show.make_cue(CueType::Fade, None, show.cues.len());
    if let CueKind::Fade(fc) = &mut f.kind {
        fc.target = Some(a);
        fc.duration = 1.0;
        fc.curve = Curve::Linear;
        fc.opacity = Some(0.5);
        fc.volume_db = None;
        fc.stop_target = false;
    }
    let fid = f.id;
    show.cues.push(f);
    let mut e = Engine::for_tests();
    fire(&mut e, &show, a);
    e.fire(&show, fid);
    e.tick(&show, 1.1);
    assert_eq!((on(&e, d1), on(&e, d2)), (vec![(a, 0.5)], vec![(a, 0.5)]));
    // Replaced on display 2, then faded out by hand: it goes from display 1 too.
    let b = text(&mut show, d2, &[], 0, Transition::Cut, 0.0);
    fire(&mut e, &show, b);
    e.stop_cue(a, 1.0);
    e.tick(&show, 0.5);
    assert!(on(&e, d1)[0].0 == a && near(on(&e, d1)[0].1, 0.25));
    e.tick(&show, 0.6);
    assert!(on(&e, d1).is_empty() && active(&e, a).is_none());
    // A stop cue aimed at it: off both at once.
    e.fire(&show, a);
    drawn(&mut e, &show);
    assert_eq!(on(&e, d1).len(), 1);
    let mut s = show.make_cue(CueType::Stop, None, show.cues.len());
    if let CueKind::Stop(sc) = &mut s.kind {
        sc.target = Some(a);
        sc.fade = 0.0;
        sc.use_own_fade = false;
    }
    let sid = s.id;
    show.cues.push(s);
    e.fire(&show, sid);
    e.tick(&show, 0.0);
    assert!(on(&e, d1).is_empty() && !on(&e, d2).iter().any(|l| l.0 == a));
}

#[test]
fn editing_the_further_displays_while_it_plays_adds_and_removes_them_live() {
    let (mut show, d1, d2) = two_displays();
    let a = text(&mut show, d1, &[], 0, Transition::Cut, 0.0);
    let mut e = Engine::for_tests();
    fire(&mut e, &show, a);
    assert!(on(&e, d2).is_empty());
    show.cue_mut(a).unwrap().kind.media_mut().unwrap().also_on = vec![d2];
    e.tick(&show, 0.1);
    assert_eq!(on(&e, d2), vec![(a, 1.0)]);
    show.cue_mut(a)
        .unwrap()
        .kind
        .media_mut()
        .unwrap()
        .also_on
        .clear();
    e.tick(&show, 0.1);
    assert!(on(&e, d2).is_empty());
    assert_eq!(on(&e, d1), vec![(a, 1.0)], "still on its home");
}

#[test]
fn a_dip_dips_every_display_of_its_cue_and_leaves_the_rest_alone() {
    let (mut show, d1, d2) = two_displays();
    let d3 = show.add_display();
    // On display 1 and 3; a dip comes in on 1 and 2.
    let shared = text(&mut show, d1, &[d3], 0, Transition::Cut, 0.0);
    let y = text(&mut show, d2, &[], 0, Transition::Cut, 0.0);
    let dip = text(&mut show, d1, &[d2], 0, Transition::DipToBlack, 1.0);
    let mut e = Engine::for_tests();
    fire(&mut e, &show, shared);
    fire(&mut e, &show, y);
    e.fire(&show, dip);
    e.tick(&show, 0.25);
    let matte = |e: &Engine, o| {
        e.layers(o)
            .iter()
            .any(|l| matches!(l.source, LayerSource::Solid(_)))
    };
    assert!(matte(&e, d1) && matte(&e, d2) && !matte(&e, d3));
    let y_out = active(&e, y).unwrap();
    assert!(y_out.stopping, "only on the dipped displays: it goes");
    assert!(
        !active(&e, shared).unwrap().stopping,
        "on display 3 too: it stays"
    );
    // The midpoint, the reveal.
    e.tick(&show, 0.3);
    drawn(&mut e, &show);
    e.tick(&show, 0.0);
    assert!(active(&e, y).is_none());
    assert!(!on(&e, d1).iter().any(|l| l.0 == shared));
    assert_eq!(on(&e, d3), vec![(shared, 1.0)]);
    assert!(on(&e, d1).iter().any(|l| l.0 == dip) && on(&e, d2).iter().any(|l| l.0 == dip));
}

#[test]
fn the_decode_size_is_its_largest_open_display_and_further_displays_keep_a_preload() {
    let (mut show, d1, d2) = two_displays();
    let d3 = show.add_display();
    let mut c = show.make_cue(CueType::Video, Some("f.mp4".into()), 0);
    let m = c.kind.media_mut().unwrap();
    m.output = Some(d1);
    m.also_on = vec![d2, d3];
    let mut e = Engine::for_tests();
    e.output_sizes.insert(d1, (1280, 720));
    e.output_sizes.insert(d2, (3840, 2160));
    // (Display 3 has no window open: it counts for nothing.)
    let sig = e.signature(&c).unwrap();
    assert_eq!(sig.size, (3840, 2160));
    // Another list of displays of the same size decodes alike.
    let mut other = c.clone();
    other.kind.media_mut().unwrap().also_on = vec![d2];
    assert!(e.signature(&other).unwrap() == sig);
    other.kind.media_mut().unwrap().also_on.clear();
    assert_eq!(e.signature(&other).unwrap().size, (1280, 720));
    // A text is drawn at its home display's size.
    let t = text_on(&mut show, d1, &[d2], 0, Transition::Cut, 0.0);
    let CueKind::Text(tc) = &t.kind else {
        unreachable!()
    };
    let Prepared {
        kind: PreparedKind::Text(rx),
        ..
    } = e.prepare_text(&t, tc)
    else {
        panic!("a text is drawn");
    };
    let img = rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
    assert_eq!((img.width, img.height), (1280, 720));
}

// ----- On a timeline ---------------------------------------------------------------

/// A timeline group with the shared text `a` (displays 1 and 2) at 0 and `b`
/// crossfading in on display 2 alone over 2 s at 1; with a free-run pause at 2.
fn timeline(with_pause: bool) -> (Show, CueId, CueId, CueId, OutputId, OutputId) {
    let (mut show, d1, d2) = two_displays();
    let mut g = show.make_cue(CueType::Group, None, 0);
    if let CueKind::Group(gc) = &mut g.kind {
        gc.mode = GroupMode::Timeline;
    }
    let gid = g.id;
    show.cues.push(g);
    let push = |show: &mut Show, mut c: Cue, at: f64| {
        c.parent = Some(gid);
        c.pre_wait = at;
        let id = c.id;
        show.cues.push(c);
        id
    };
    let a = text_on(&mut show, d1, &[d2], 0, Transition::Cut, 0.0);
    let a = push(&mut show, a, 0.0);
    let b = text_on(&mut show, d2, &[], 0, Transition::Crossfade, 2.0);
    let b = push(&mut show, b, 1.0);
    if with_pause {
        let mut p = show.make_cue(CueType::Control, None, show.cues.len());
        p.kind = CueKind::Control(ControlCue {
            action: ControlAction::TimelinePause,
            freeze: PauseFreeze::Nothing,
            ..Default::default()
        });
        push(&mut show, p, 2.0);
    }
    (show, gid, a, b, d1, d2)
}

/// The shared cue's level on each display.
fn levels(e: &Engine, a: CueId, d1: OutputId, d2: OutputId) -> (Option<f32>, Option<f32>) {
    let level = |o| on(e, o).iter().find(|l| l.0 == a).map(|l| l.1);
    (level(d1), level(d2))
}

#[test]
fn a_timeline_pause_holds_a_crossfade_on_one_display_and_go_lets_it_finish() {
    let (show, g, a, b, d1, d2) = timeline(true);
    let mut e = Engine::for_tests();
    e.fire(&show, g);
    drawn(&mut e, &show);
    e.tick(&show, 1.0);
    drawn(&mut e, &show);
    assert!(active(&e, b).is_some());
    e.tick(&show, 1.0);
    let at_pause = levels(&e, a, d1, d2);
    assert!(
        at_pause.0 == Some(1.0) && near(at_pause.1.unwrap(), 0.5),
        "{at_pause:?}"
    );
    // Stood at the pause: the fade on display 2 waits with it.
    e.tick(&show, 5.0);
    assert_eq!(levels(&e, a, d1, d2), at_pause);
    e.go(&show);
    e.tick(&show, 0.5);
    assert!(near(levels(&e, a, d1, d2).1.unwrap(), 0.25));
    e.tick(&show, 0.6);
    assert_eq!(levels(&e, a, d1, d2), (Some(1.0), None));
    assert_eq!(on(&e, d2), vec![(b, 1.0)]);
}

#[test]
fn a_seek_gives_the_shared_cue_the_levels_playing_there_would() {
    let (show, g, a, b, d1, d2) = timeline(false);
    // Played to 2.0.
    let mut e = Engine::for_tests();
    e.fire(&show, g);
    drawn(&mut e, &show);
    e.tick(&show, 1.0);
    drawn(&mut e, &show);
    e.tick(&show, 1.0);
    let played = levels(&e, a, d1, d2);
    // Sought to.
    let mut e = Engine::for_tests();
    e.timeline_seek(&show, TimelineKey::Group(g), 2.0, true);
    drawn(&mut e, &show);
    let sought = levels(&e, a, d1, d2);
    assert_eq!(sought.0, Some(1.0));
    assert!(
        near(sought.1.unwrap(), played.1.unwrap()),
        "{sought:?} {played:?}"
    );
    let b_level = |e: &Engine| on(e, d2).iter().find(|l| l.0 == b).map(|l| l.1);
    assert!(near(b_level(&e).unwrap(), 0.5));
    // Past the crossfade: on display 1 alone.
    e.timeline_seek(&show, TimelineKey::Group(g), 3.5, true);
    drawn(&mut e, &show);
    assert_eq!(levels(&e, a, d1, d2), (Some(1.0), None));
    assert_eq!(b_level(&e), Some(1.0));
}
