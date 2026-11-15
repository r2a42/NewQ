//! A moving playhead is kept in view (`tracks::follow`): the rule on its own,
//! then the editor with a run going.

use super::*;
use crate::prefs::{Prefs, TimelineFollow};

fn follow(scroll: f32, w: f32, content: f32, at: f32, moving: bool, held: bool) -> f32 {
    super::tracks::follow(scroll, w, content, at, moving, held, TimelineFollow::Page)
}

#[test]
fn the_rule_pages_past_the_edge_or_out_of_view_and_only_while_moving() {
    // 1000 points of tracks over 4000 of timeline.
    let f = |scroll, at, moving, held| follow(scroll, 1000.0, 4000.0, at, moving, held);
    // In view, short of 90 %: nothing.
    assert_eq!(f(0.0, 500.0, true, false), 0.0);
    assert_eq!(f(0.0, 900.0, true, false), 0.0);
    // Past it: the page turns, the playhead 10 % in.
    assert_eq!(f(0.0, 901.0, true, false), 801.0);
    // Out of view on either side (a jump, a loop going back, the user away).
    assert_eq!(f(2000.0, 300.0, true, false), 200.0);
    assert_eq!(f(0.0, 2500.0, true, false), 2400.0);
    // Kept within the ends.
    assert_eq!(f(2000.0, 50.0, true, false), 0.0);
    assert_eq!(f(0.0, 3990.0, true, false), 3000.0);
    // ...and steady there, though the playhead is past 90 %.
    assert_eq!(f(3000.0, 3990.0, true, false), 3000.0);
    // Still, or a drag held: never.
    assert_eq!(f(2000.0, 300.0, false, false), 2000.0);
    assert_eq!(f(2000.0, 300.0, true, true), 2000.0);
    // All of it fits (Fit): nothing to scroll.
    assert_eq!(follow(0.0, 1000.0, 1000.0, 700.0, true, false), 0.0);
    assert_eq!(follow(0.0, 1000.0, 1000.0, 950.0, true, false), 0.0);
}

/// The fixture with its pictures taken out (they have no files, and a run
/// stops at one) and a 58 s wait in their place: a timeline 60 s long.
pub(super) fn long_fixture() -> Fixture {
    let mut f = fixture();
    f.show.cues.retain(|c| ![f.a, f.b, f.c].contains(&c.id));
    let mut wait = f.show.make_cue(CueType::Wait, None, f.show.cues.len());
    wait.name = "Long".into();
    wait.pre_wait = 2.0;
    wait.parent = Some(f.group);
    if let CueKind::Wait(wc) = &mut wait.kind {
        wc.duration = 58.0;
    }
    f.a = wait.id;
    f.show.cues.push(wait);
    f
}

/// A timeline group 60 s long, zoomed in to 40 points a second
/// (about 20 s in view), with its run going.
fn running() -> (Fixture, Mouse, TimelineKey) {
    running_with(long_fixture())
}

fn running_with(mut f: Fixture) -> (Fixture, Mouse, TimelineKey) {
    f.st.timeline.pps = 40.0;
    let key = TimelineKey::Group(f.group);
    let mut m = Mouse::new();
    m.frame(&mut f.show, &mut f.st, vec![]);
    m.e.timeline_run(&f.show, key);
    m.e.tick(&f.show, 0.05);
    (f, m, key)
}

/// Run on `secs` in steps of a quarter, a frame after each.
fn run_on(f: &mut Fixture, m: &mut Mouse, secs: f64) {
    for _ in 0..(secs * 4.0).round() as usize {
        m.e.tick(&f.show, 0.25);
        m.frame(&mut f.show, &mut f.st, vec![]);
    }
}

fn lane_w(m: &Mouse) -> f32 {
    m.rects().unwrap().ruler.width()
}

/// Where in the tracks' width the playhead is drawn (0 left, 1 right).
fn share(m: &Mouse) -> Option<f32> {
    let r = m.rects().unwrap();
    r.playhead_x.map(|x| (x - r.ruler.left()) / r.ruler.width())
}

#[test]
fn a_run_going_past_the_edge_turns_the_page_and_the_playhead_never_leaves() {
    let (mut f, mut m, key) = running();
    assert_eq!(m.e.timeline(&f.show, key).state, RunState::Running);
    let w = lane_w(&m);
    let mut pages = 0;
    let mut last = f.st.timeline.scroll.x;
    for _ in 0..(40 * 4) {
        m.e.tick(&f.show, 0.25);
        m.frame(&mut f.show, &mut f.st, vec![]);
        let at = share(&m).expect("the playhead is in view while it runs");
        assert!(at <= 0.9 + 1.0 / w + 0.02, "past the edge: {at}");
        if f.st.timeline.scroll.x > last + 1.0 {
            pages += 1;
            // Turned so it is a tenth in.
            assert!((at - 0.1).abs() < 0.03, "{at}");
        }
        last = f.st.timeline.scroll.x;
    }
    assert!(pages >= 2, "the view paged {pages} times");
}

#[test]
fn a_loop_going_back_brings_the_playhead_into_view() {
    let mut f = long_fixture();
    // A loop pause at 40 s playing again from 3 s.
    let mut p = f.show.make_cue(CueType::Control, None, f.show.cues.len());
    p.parent = Some(f.group);
    p.pre_wait = 40.0;
    if let CueKind::Control(c) = &mut p.kind {
        c.action = ControlAction::TimelinePause;
        c.loop_from = Some(TimelinePoint::Time(3.0));
    }
    f.show.cues.push(p);
    let (mut f, mut m, key) = running_with(f);
    let mut back = false;
    for _ in 0..(45 * 4) {
        let was = m.e.timeline(&f.show, key).pos;
        m.e.tick(&f.show, 0.25);
        m.frame(&mut f.show, &mut f.st, vec![]);
        let now = m.e.timeline(&f.show, key).pos;
        back |= now < was - 1.0;
        assert!(share(&m).is_some(), "out of view at {now}");
    }
    assert!(back, "the loop went back");
    assert!(m.e.timeline(&f.show, key).looping.is_some());
}

#[test]
fn scrolled_away_while_running_comes_back_and_while_paused_stays() {
    let (mut f, mut m, key) = running();
    run_on(&mut f, &mut m, 3.0);
    // The user scrolls far ahead.
    f.st.timeline.scroll.x = 1500.0;
    m.frame(&mut f.show, &mut f.st, vec![]);
    assert!(share(&m).is_some(), "back to the playhead");
    // In view after a scroll of their own: left where they put it.
    f.st.timeline.scroll.x = 40.0;
    m.frame(&mut f.show, &mut f.st, vec![]);
    assert_eq!(f.st.timeline.scroll.x, 40.0);
    // Paused, it may be scrolled off screen and stays off.
    m.e.timeline_pause(&f.show, key, PauseFreeze::Everything, &[], false, false);
    m.e.tick(&f.show, 0.05);
    f.st.timeline.scroll.x = 1500.0;
    for _ in 0..5 {
        m.e.tick(&f.show, 0.25);
        m.frame(&mut f.show, &mut f.st, vec![]);
    }
    assert_eq!(f.st.timeline.scroll.x, 1500.0);
    assert!(share(&m).is_none());
    // Stopped, the same.
    m.e.timeline_stop(&f.show, key, 0.0);
    m.frame(&mut f.show, &mut f.st, vec![]);
    assert_eq!(f.st.timeline.scroll.x, 1500.0);
}

#[test]
fn the_scroll_bar_held_is_not_fought_and_on_release_the_view_comes_back() {
    let (mut f, mut m, _key) = running();
    run_on(&mut f, &mut m, 2.0);
    let thumb = m.rects().unwrap().hscroll.expect("a scroll bar");
    let a = thumb.center();
    let b = a + vec2(500.0, 0.0);
    m.frame(&mut f.show, &mut f.st, vec![Event::PointerMoved(a)]);
    m.frame(
        &mut f.show,
        &mut f.st,
        vec![button(a, true, PointerButton::Primary)],
    );
    for i in 1..=8 {
        m.e.tick(&f.show, 0.1);
        let p = a + (b - a) * (i as f32 / 8.0);
        m.frame(&mut f.show, &mut f.st, vec![Event::PointerMoved(p)]);
    }
    // Held, far ahead of the playhead: the view is the user's.
    for _ in 0..4 {
        m.e.tick(&f.show, 0.1);
        m.frame(&mut f.show, &mut f.st, vec![]);
    }
    assert!(
        f.st.timeline.scroll.x > 1000.0,
        "{}",
        f.st.timeline.scroll.x
    );
    assert!(share(&m).is_none());
    m.frame(
        &mut f.show,
        &mut f.st,
        vec![button(b, false, PointerButton::Primary)],
    );
    m.frame(&mut f.show, &mut f.st, vec![]);
    assert!(share(&m).is_some(), "followed again on release");
}

#[test]
fn a_clip_drag_held_while_running_is_not_interrupted() {
    let (mut f, mut m, _key) = running();
    run_on(&mut f, &mut m, 1.0);
    let r = m.clip(&mut f.show, &mut f.st, f.a);
    let a = pos2(r.left() + 30.0, r.center().y);
    m.frame(&mut f.show, &mut f.st, vec![Event::PointerMoved(a)]);
    m.frame(
        &mut f.show,
        &mut f.st,
        vec![button(a, true, PointerButton::Primary)],
    );
    m.frame(
        &mut f.show,
        &mut f.st,
        vec![Event::PointerMoved(a + vec2(30.0, 0.0))],
    );
    assert!(matches!(f.st.timeline.drag, Some((_, Drag::Clips { .. }))));
    // The run goes past the edge while the clip is held: the view waits.
    let before = f.st.timeline.scroll.x;
    for _ in 0..(24 * 4) {
        m.e.tick(&f.show, 0.25);
        m.frame(
            &mut f.show,
            &mut f.st,
            vec![Event::PointerMoved(a + vec2(40.0, 0.0))],
        );
    }
    assert_eq!(f.st.timeline.scroll.x, before);
    assert!(matches!(f.st.timeline.drag, Some((_, Drag::Clips { .. }))));
    m.frame(
        &mut f.show,
        &mut f.st,
        vec![button(a + vec2(40.0, 0.0), false, PointerButton::Primary)],
    );
    m.frame(&mut f.show, &mut f.st, vec![]);
    assert!(share(&m).is_some(), "followed again on release");
}

#[test]
fn fit_never_moves() {
    let (mut f, mut m, _key) = running();
    f.st.timeline.pps = 0.0;
    for _ in 0..(50 * 2) {
        m.e.tick(&f.show, 0.5);
        m.frame(&mut f.show, &mut f.st, vec![]);
        assert_eq!(f.st.timeline.scroll.x, 0.0);
        assert!(share(&m).is_some());
    }
}

#[test]
fn centred_the_playhead_stays_in_the_middle_but_the_ends_hold_the_view() {
    let f = |scroll, at, moving, held| {
        super::tracks::follow(
            scroll,
            1000.0,
            4000.0,
            at,
            moving,
            held,
            TimelineFollow::Centre,
        )
    };
    // Mid-timeline: in the middle, wherever the view was (a jump re-centres,
    // a scroll of the user's is undone).
    assert_eq!(f(0.0, 1500.0, true, false), 1000.0);
    assert_eq!(f(2500.0, 1500.0, true, false), 1000.0);
    assert_eq!(f(1000.0, 1501.0, true, false), 1001.0);
    // Near the ends the view stops and the playhead leaves the middle.
    assert_eq!(f(500.0, 200.0, true, false), 0.0);
    assert_eq!(f(500.0, 3900.0, true, false), 3000.0);
    // Held, still, or all of it fits: nothing.
    assert_eq!(f(2500.0, 1500.0, true, true), 2500.0);
    assert_eq!(f(2500.0, 1500.0, false, false), 2500.0);
    assert_eq!(
        super::tracks::follow(
            0.0,
            1000.0,
            1000.0,
            700.0,
            true,
            false,
            TimelineFollow::Centre
        ),
        0.0
    );
}

#[test]
fn centred_a_run_creeps_under_the_playhead_and_a_drag_held_waits() {
    let mut f = long_fixture();
    f.st.prefs.timeline_follow = TimelineFollow::Centre;
    let (mut f, mut m, key) = running_with(f);
    run_on(&mut f, &mut m, 15.0);
    let at = share(&m).unwrap();
    assert!((at - 0.5).abs() < 0.01, "{at}");
    // A scroll away comes straight back.
    f.st.timeline.scroll.x = 0.0;
    m.frame(&mut f.show, &mut f.st, vec![]);
    assert!((share(&m).unwrap() - 0.5).abs() < 0.01);
    // A jump back near the start: the view stops at it.
    m.e.timeline_pause(&f.show, key, PauseFreeze::Everything, &[], false, false);
    m.e.timeline_seek(&f.show, key, 1.0, true);
    m.e.timeline_run(&f.show, key);
    run_on(&mut f, &mut m, 1.0);
    assert_eq!(f.st.timeline.scroll.x, 0.0);
    assert!(share(&m).unwrap() < 0.2);
    // The scroll bar held: left alone, then centred on release.
    run_on(&mut f, &mut m, 20.0);
    let thumb = m.rects().unwrap().hscroll.unwrap();
    let a = thumb.center();
    m.frame(&mut f.show, &mut f.st, vec![Event::PointerMoved(a)]);
    m.frame(
        &mut f.show,
        &mut f.st,
        vec![button(a, true, PointerButton::Primary)],
    );
    for i in 1..=6 {
        m.e.tick(&f.show, 0.1);
        let p = a - vec2(60.0 * i as f32, 0.0);
        m.frame(&mut f.show, &mut f.st, vec![Event::PointerMoved(p)]);
    }
    assert!(
        share(&m).is_none_or(|s| s > 0.6),
        "left where it was dragged"
    );
    let p = a - vec2(360.0, 0.0);
    m.frame(
        &mut f.show,
        &mut f.st,
        vec![button(p, false, PointerButton::Primary)],
    );
    m.frame(&mut f.show, &mut f.st, vec![]);
    assert!((share(&m).unwrap() - 0.5).abs() < 0.01);
}

/// The General page's radio for following centred (`true`) or by pages.
fn follow_radio(m: &Mouse, centre: bool) -> Rect {
    let all: Vec<(bool, Rect)> = m
        .ctx
        .data(|d| d.get_temp(egui::Id::new("settings_timeline_follow")))
        .expect("the General page has the choice");
    all.iter().find(|r| r.0 == centre).unwrap().1
}

#[test]
fn the_setting_on_the_general_page_changes_an_open_editor_at_once_in_both_interfaces() {
    for modern in [true, false] {
        let dir =
            std::env::temp_dir().join(format!("newq_follow_prefs_{}_{modern}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut f = long_fixture();
        f.st.layout_dir = Some(dir.clone());
        f.st.prefs.new_layout = modern;
        f.st.timeline.pps = 40.0;
        let key = TimelineKey::Group(f.group);
        let mut m = Mouse::new();
        m.draw = Draw::Full;
        // The size the settings have to be usable at, with the editor open
        // (in the classic window a pane under the settings, taking height).
        m.size = Some(egui::vec2(1280.0, 760.0));
        let ui = |a, f: &mut Fixture, m: &mut Mouse| {
            super::super::super::apply(a, &mut f.show, &mut m.e, &mut f.st)
        };
        ui(Action::Timeline(TimelineAction::Open(None)), &mut f, &mut m);
        for page in [false, true] {
            ui(
                Action::OpenSettings(super::super::super::SettingsPage::General),
                &mut f,
                &mut m,
            );
            m.frame(&mut f.show, &mut f.st, vec![]);
            let want = if page {
                TimelineFollow::Page
            } else {
                TimelineFollow::Centre
            };
            // The radio itself, in sight without scrolling (`follow_radio`
            // gives an empty rect for one scrolled out of sight).
            let r = follow_radio(&m, !page);
            assert!(r.width() > 0.0, "modern {modern}: the radio is in sight");
            m.click(&mut f.show, &mut f.st, r.center(), PointerButton::Primary);
            assert_eq!(f.st.prefs.timeline_follow, want);
            if !page {
                assert_eq!(Prefs::load(&dir).timeline_follow, TimelineFollow::Centre);
            }
            ui(Action::CloseSettings, &mut f, &mut m);
            m.frame(&mut f.show, &mut f.st, vec![]);
            m.e.timeline_stop(&f.show, key, 0.0);
            m.e.timeline_seek(&f.show, key, 0.0, false);
            f.st.timeline.scroll.x = 0.0;
            m.e.timeline_run(&f.show, key);
            let w = lane_w(&m);
            let visible = f64::from(w / 40.0);
            // Half-way across the view: centred, the view has begun to move.
            run_on(&mut f, &mut m, visible * 0.7);
            let at = share(&m).unwrap();
            if page {
                assert!(f.st.timeline.scroll.x == 0.0 && at > 0.6, "{at}");
            } else {
                assert!((at - 0.5).abs() < 0.02, "{at}");
            }
        }
        // Kept for this computer (and at its default, not written).
        assert_eq!(Prefs::load(&dir).timeline_follow, TimelineFollow::Page);
        let text = std::fs::read_to_string(dir.join("prefs.json")).unwrap();
        assert!(!text.contains("timeline_follow"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
