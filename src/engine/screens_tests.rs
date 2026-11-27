//! Screens that move on a cue, on clocks the tests own (`tick_shared`, or
//! `tick` by a set `dt`): the ramp, where the screen is after the cue, *Stop
//! all*, *Put at*, a timeline's clock, a show opened. The network's side is in
//! `net/play_tests.rs`.

use super::*;
use crate::show::{
    ControlAction, ControlCue, CueId, CueKind, CueType, Curve, GroupMode, PauseFreeze,
};

/// A show with one screen at home (0, 1, -2) and a cue moving it to x 4,
/// turn 90 over 4 s along `curve`.
pub(crate) fn moving_show(curve: Curve) -> (Show, SurfaceId, CueId) {
    let mut show = Show::default();
    let id = show.add_surface(None);
    show.stage3d.surface_mut(id).unwrap().placement.position = [0.0, 1.0, -2.0];
    let c = screen_cue(
        &mut show,
        id,
        [Some(4.0), None, None, Some(90.0), None, None],
        4.0,
        curve,
    );
    (show, id, c)
}

/// A screen cue at the end of the show.
pub(crate) fn screen_cue(
    show: &mut Show,
    screen: SurfaceId,
    to: [Option<f32>; AXES],
    secs: f64,
    curve: Curve,
) -> CueId {
    let mut c = show.make_cue(CueType::Screen, None, show.cues.len());
    if let CueKind::Screen(s) = &mut c.kind {
        s.screen = Some(screen);
        s.to = crate::show::ScreenTarget::from_axes(to);
        s.duration = secs;
        s.curve = curve;
    }
    let id = c.id;
    show.cues.push(c);
    id
}

fn x_of(e: &Engine, id: SurfaceId) -> f32 {
    e.screen_at(id).expect("moved").position[0]
}

fn near(a: f32, b: f32, what: &str) {
    assert!((a - b).abs() < 1e-4, "{what}: {a} against {b}");
}

#[test]
fn a_screen_cue_ramps_its_numbers_by_the_shared_clock_and_stays_there() {
    for curve in [Curve::Linear, Curve::SCurve] {
        let (show, id, cue) = moving_show(curve);
        let mut e = Engine::for_tests();
        e.tick_shared(&show, 10.0);
        assert_eq!(e.screen_at(id), None, "at home until a cue moves it");
        e.fire(&show, cue);
        // Uneven ticks: the place is the ramp's at the clock's time, not a
        // sum of them.
        let mut t = 10.0;
        for dt in [0.013, 0.5, 0.0, 0.25, 0.977, 0.03, 1.2, 0.4, 0.33, 0.7] {
            t += dt;
            e.tick_shared(&show, t);
            let k = curve.apply((t - 10.0) / 4.0) as f32;
            near(x_of(&e, id), 4.0 * k, &format!("{curve:?} at {t}"));
            let p = e.screen_at(id).unwrap();
            near(p.yaw, 90.0 * k, "the turn with it");
            assert_eq!(p.position[1..], [1.0, -2.0], "what isn't given is left");
        }
        // Past its end, and long after the cue: where it was told.
        e.tick_shared(&show, 30.0);
        assert!(e.active.is_empty(), "the cue is over");
        near(x_of(&e, id), 4.0, "kept after the cue");
        assert_eq!(e.moved_screens().len(), 1);
        assert!(!e.screens_moving());
    }
}

#[test]
fn stop_all_and_panic_neither_send_a_screen_back_nor_stop_its_move() {
    let (show, id, cue) = moving_show(Curve::Linear);
    for panic in [false, true] {
        let mut e = Engine::for_tests();
        e.tick_shared(&show, 0.0);
        e.fire(&show, cue);
        e.tick_shared(&show, 1.0);
        near(x_of(&e, id), 1.0, "a quarter of the way");
        if panic {
            e.panic(&show);
        } else {
            e.stop_all(0.0);
        }
        e.tick_shared(&show, 2.0);
        assert!(e.active.is_empty(), "the cue is let go");
        near(x_of(&e, id), 2.0, "the scenery goes on");
        e.tick_shared(&show, 9.0);
        near(x_of(&e, id), 4.0, "to where it was told");
    }
    // Ticking alone (no shared clock): the same, by its ticks.
    let mut e = Engine::for_tests();
    e.fire(&show, cue);
    for _ in 0..4 {
        e.tick(&show, 0.25);
    }
    e.stop_all(0.0);
    for _ in 0..4 {
        e.tick(&show, 0.25);
    }
    near(x_of(&e, id), 2.0, "half way after 2 s of ticks");
    for _ in 0..20 {
        e.tick(&show, 0.25);
    }
    near(x_of(&e, id), 4.0, "there");
}

#[test]
fn a_cue_moves_only_its_own_numbers_and_starts_from_where_the_screen_is() {
    let (mut show, id, cue) = moving_show(Curve::Linear);
    // A second cue lifts it (y) over 2 s, fired a second into the first.
    let lift = screen_cue(
        &mut show,
        id,
        [None, Some(3.0), None, None, None, None],
        2.0,
        Curve::Linear,
    );
    // A third sends x back to 0 over 1 s, from wherever it is.
    let back = screen_cue(
        &mut show,
        id,
        [Some(0.0), None, None, None, None, None],
        1.0,
        Curve::Linear,
    );
    let mut e = Engine::for_tests();
    e.tick_shared(&show, 0.0);
    e.fire(&show, cue);
    e.tick_shared(&show, 1.0);
    e.fire(&show, lift);
    e.tick_shared(&show, 2.0);
    let p = e.screen_at(id).unwrap();
    near(p.position[0], 2.0, "x goes on by its own cue");
    near(p.position[1], 2.0, "y half way up");
    e.fire(&show, back);
    e.tick_shared(&show, 2.5);
    near(x_of(&e, id), 1.0, "x from 2 towards 0");
    e.tick_shared(&show, 10.0);
    let p = e.screen_at(id).unwrap();
    assert_eq!((p.position, p.yaw), ([0.0, 3.0, -2.0], 90.0));
}

#[test]
fn put_at_jumps_and_the_next_cue_starts_from_there() {
    let (show, id, cue) = moving_show(Curve::Linear);
    let mut e = Engine::for_tests();
    e.tick_shared(&show, 0.0);
    let there = crate::stage3d::Placement {
        position: [-3.0, 1.0, -2.0],
        yaw: 0.0,
        pitch: 0.0,
        roll: 10.0,
    };
    e.put_screen(&show, id, there);
    assert_eq!(e.screen_at(id), Some(there), "at once");
    e.fire(&show, cue);
    e.tick_shared(&show, 2.0);
    let p = e.screen_at(id).unwrap();
    near(p.position[0], 0.5, "half way from -3 to 4");
    assert_eq!(p.roll, 10.0);
    // A screen the show doesn't have, or a place that isn't one: nothing.
    e.put_screen(&show, SurfaceId(999), there);
    assert_eq!(e.screen_at(SurfaceId(999)), None);
    let mut bad = there;
    bad.yaw = f32::NAN;
    e.put_screen(&show, id, bad);
    assert!(e.screen_at(id).unwrap().yaw.is_finite());
}

#[test]
fn a_show_opened_or_a_screen_no_cue_moves_is_at_home() {
    let (mut show, id, cue) = moving_show(Curve::Linear);
    let mut e = Engine::for_tests();
    e.fire(&show, cue);
    e.tick(&show, 5.0);
    near(x_of(&e, id), 4.0, "moved");
    e.show_opened();
    assert_eq!(e.screen_at(id), None, "a show opened: every screen at home");
    e.fire(&show, cue);
    e.tick(&show, 5.0);
    assert!(e.screen_at(id).is_some());
    // The cue deleted: the screen can't move, and a lens map has it at home.
    show.cues.clear();
    e.tick(&show, 0.1);
    assert_eq!(e.screen_at(id), None);
}

#[test]
fn a_screen_cue_with_no_screen_or_no_time_does_no_harm() {
    let (mut show, id, _) = moving_show(Curve::Linear);
    let jump = screen_cue(
        &mut show,
        id,
        [Some(7.0), None, None, None, None, None],
        0.0,
        Curve::Linear,
    );
    let mut c = show.make_cue(CueType::Screen, None, show.cues.len());
    if let CueKind::Screen(s) = &mut c.kind {
        s.screen = None;
        s.duration = f64::NAN;
    }
    let none = c.id;
    show.cues.push(c);
    let mut e = Engine::for_tests();
    e.fire(&show, none);
    e.tick(&show, 0.1);
    assert!(e.moved_screens().is_empty());
    e.fire(&show, jump);
    e.tick(&show, 0.0);
    near(x_of(&e, id), 7.0, "0 s is a jump");
}

#[test]
fn a_paused_show_holds_a_screen_cue_that_plays_and_its_move() {
    let (show, id, cue) = moving_show(Curve::Linear);
    let mut e = Engine::for_tests();
    e.tick_shared(&show, 0.0);
    e.fire(&show, cue);
    e.tick_shared(&show, 1.0);
    e.set_paused(true);
    e.tick_shared(&show, 3.0);
    near(x_of(&e, id), 1.0, "held while paused");
    e.set_paused(false);
    e.tick_shared(&show, 4.0);
    near(x_of(&e, id), 2.0, "on from where it was");
}

#[test]
fn a_screen_cue_on_a_timeline_moves_with_its_clock_and_waits_at_its_pause() {
    let mut show = Show::default();
    let id = show.add_surface(None);
    let mut g = show.make_cue(CueType::Group, None, 0);
    if let CueKind::Group(gc) = &mut g.kind {
        gc.mode = GroupMode::Timeline;
    }
    let gid = g.id;
    show.cues.push(g);
    let cue = screen_cue(
        &mut show,
        id,
        [Some(4.0), None, None, None, None, None],
        4.0,
        Curve::Linear,
    );
    show.cue_mut(cue).unwrap().parent = Some(gid);
    let mut p = show.make_cue(CueType::Control, None, show.cues.len());
    p.kind = CueKind::Control(ControlCue {
        action: ControlAction::TimelinePause,
        freeze: PauseFreeze::Nothing,
        ..ControlCue::default()
    });
    p.parent = Some(gid);
    p.pre_wait = 2.0;
    show.cues.push(p);
    let mut e = Engine::for_tests();
    e.fire(&show, gid);
    for _ in 0..12 {
        e.tick(&show, 0.25);
    }
    near(x_of(&e, id), 2.0, "half way at the pause");
    for _ in 0..12 {
        e.tick(&show, 0.25);
    }
    near(x_of(&e, id), 2.0, "waits while the clock stands");
    e.go(&show);
    for _ in 0..4 {
        e.tick(&show, 0.25);
    }
    near(x_of(&e, id), 3.0, "on with the clock");
    for _ in 0..8 {
        e.tick(&show, 0.25);
    }
    near(x_of(&e, id), 4.0, "there");
}

/// R21 on a follower: a screen that differs from the primary's heartbeat is
/// left for three heartbeats, then eased to the primary's place over half a
/// second, never jumped; the first heartbeat a computer compares is taken at
/// once.
#[test]
fn a_follower_eases_a_screen_that_steadily_differs_and_takes_the_first_at_once() {
    use crate::net::play::{PlayState, ScreenNow};
    let (show, id, _) = moving_show(Curve::Linear);
    let beat = |x: f32, t: f64| PlayState {
        time: t,
        screens: vec![ScreenNow {
            screen: id,
            at: crate::stage3d::Placement {
                position: [x, 1.0, -2.0],
                ..Default::default()
            },
            moving: Vec::new(),
        }],
        ..PlayState::default()
    };
    // Just joined: the first heartbeat is taken at once.
    let mut e = Engine::for_tests();
    e.tick_shared(&show, 0.0);
    let mut doubts = crate::engine::NetCorrection::default();
    let said = e.net_correct(&show, &beat(3.0, 0.0), 0.0, &mut doubts);
    assert!(said.iter().any(|s| s.contains("put where")), "{said:?}");
    near(x_of(&e, id), 3.0, "taken at once");
    // Then the primary has it elsewhere: three heartbeats change nothing.
    let mut t = 0.0;
    for _ in 0..3 {
        t += 0.25;
        e.tick_shared(&show, t);
        assert!(
            e.net_correct(&show, &beat(1.0, t), t, &mut doubts)
                .is_empty()
        );
        near(x_of(&e, id), 3.0, "left alone");
    }
    t += 0.25;
    e.tick_shared(&show, t);
    let said = e.net_correct(&show, &beat(1.0, t), t, &mut doubts);
    assert!(said.iter().any(|s| s.contains("eased")), "{said:?}");
    near(x_of(&e, id), 3.0, "no jump at the correction");
    // Eased over half a second at 60 Hz: no frame's step is more than the
    // ease's steepest (S-curve: 1.5 times 2 m over 0.5 s) allows.
    let mut last = x_of(&e, id);
    for _ in 0..40 {
        t += 1.0 / 60.0;
        e.tick_shared(&show, t);
        let x = x_of(&e, id);
        assert!(
            (x - last).abs() <= 1.5 * 2.0 / 0.5 / 60.0 + 1e-4,
            "{last} -> {x}"
        );
        assert!(x <= last + 1e-6, "towards the primary's");
        last = x;
    }
    near(last, 1.0, "at the primary's place");
}

/// *Put at* on a show network: the primary catches it as a command (done
/// everywhere at one shared time), a follower drops it (its controls are
/// the primary's), and the command does it.
#[test]
fn put_at_is_a_command_on_a_primary_and_nothing_on_a_follower() {
    use crate::engine::NetMode;
    use crate::net::play::Command;
    let (show, id, _) = moving_show(Curve::Linear);
    let at = crate::stage3d::Placement {
        position: [5.0, 0.0, 0.0],
        ..Default::default()
    };
    let mut p = Engine::for_tests();
    p.net_set_mode(NetMode::Primary);
    p.put_screen(&show, id, at);
    assert_eq!(p.screen_at(id), None, "not done until its time");
    let cmds = p.net_take();
    assert_eq!(cmds, vec![Command::PutScreen { screen: id, at }]);
    p.tick_shared(&show, 1.0);
    p.net_apply(&show, &cmds[0], 0.0);
    assert_eq!(p.screen_at(id), Some(at));
    let mut f = Engine::for_tests();
    f.net_set_mode(NetMode::Follower);
    f.put_screen(&show, id, at);
    assert_eq!(f.screen_at(id), None);
    assert!(f.net_take().is_empty());
    // On the wire, and as an older NewQ reads one it doesn't know.
    let wire = serde_json::to_value(&cmds[0]).unwrap();
    assert_eq!(wire["do"], "put_screen");
    let later: Command =
        serde_json::from_value(serde_json::json!({"do": "put_screen_on_a_track", "x": 1})).unwrap();
    assert_eq!(later, Command::Unknown);
}
