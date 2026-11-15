//! Space and 0 through the whole window (`ui::draw`, its actions made): Space
//! plays and pauses a timeline being worked on (the editor that holds the
//! keys, else a timeline list being shown) and is GO anywhere else; 0 is GO
//! everywhere.

use super::*;
use crate::remote::fixtures;
use egui::Key;

/// The remote's timeline show (a list "Act 1": a text 0–6 s, a free-run
/// pause at 3 s; a group "5" in the main list: a text 0–4 s, a pause at 1 s
/// that holds everything), drawn whole, in show mode or not.
struct Win {
    show: Show,
    st: UiState,
    m: Mouse,
    list: TimelineKey,
    group: TimelineKey,
}

fn win(show_mode: bool) -> Win {
    let (show, list, group) = fixtures::show();
    let mut m = Mouse::new();
    m.draw = Draw::Full;
    m.e = fixtures::engine(&show);
    // GO on the main list fires the group.
    m.e.playhead = Some(group);
    let st = UiState {
        show_mode,
        ..UiState::default()
    };
    let mut w = Win {
        show,
        st,
        m,
        list: TimelineKey::List(list),
        group: TimelineKey::Group(group),
    };
    w.frame(vec![]);
    w
}

impl Win {
    fn frame(&mut self, events: Vec<Event>) {
        self.m.frame(&mut self.show, &mut self.st, events);
    }

    fn apply(&mut self, a: Action) {
        super::super::super::apply(a, &mut self.show, &mut self.m.e, &mut self.st);
    }

    /// Press and let go of `key`.
    fn key(&mut self, key: Key) {
        let ev = |pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Default::default(),
        };
        self.frame(vec![ev(true)]);
        self.frame(vec![ev(false)]);
    }

    fn state(&mut self, key: TimelineKey) -> RunState {
        self.m.e.timeline(&self.show, key).state
    }

    /// Let the engine run `secs` (texts drawn, runs cued), a frame after.
    fn run(&mut self, key: TimelineKey, secs: f64) {
        let mut left = secs;
        fixtures::run_while(&mut self.m.e, &self.show, key, 0.05, |_| {
            left -= 0.05;
            left > 0.0
        });
        self.frame(vec![]);
    }

    fn on_list(&mut self) {
        let TimelineKey::List(l) = self.list else {
            unreachable!()
        };
        self.apply(Action::SelectList(l));
        self.frame(vec![]);
    }
}

fn paused(s: RunState) -> bool {
    matches!(s, RunState::Paused { .. })
}

#[test]
fn on_a_timeline_list_space_runs_pauses_and_resumes_in_either_mode() {
    for show_mode in [false, true] {
        let mut w = win(show_mode);
        w.on_list();
        let key = w.list;
        assert_eq!(w.state(key), RunState::Stopped);
        w.key(Key::Space);
        w.run(key, 0.5);
        assert_eq!(w.state(key), RunState::Running, "Space runs it");
        // Running: GO (0) does nothing; Space pauses.
        w.key(Key::Num0);
        assert_eq!(w.state(key), RunState::Running, "0 is GO: nothing");
        w.key(Key::Space);
        assert!(paused(w.state(key)), "{:?}", w.state(key));
        let at = w.m.e.timeline(&w.show, key).pos;
        w.run(key, 0.5);
        assert_eq!(w.m.e.timeline(&w.show, key).pos, at, "the clock stood");
        w.key(Key::Space);
        w.run(key, 0.3);
        assert_eq!(w.state(key), RunState::Running, "and resumes");
        assert!(w.m.e.timeline(&w.show, key).pos > at);
    }
}

#[test]
fn on_an_ordinary_cue_list_space_and_0_are_go_even_with_a_timeline_running() {
    for show_mode in [false, true] {
        for k in [Key::Space, Key::Num0] {
            let mut w = win(show_mode);
            let group = w.group;
            // GO fires the group (a timeline) at the playhead.
            w.key(k);
            w.run(group, 0.5);
            // Its own pause at 1 s waits for GO: past it, running.
            let pos = w.m.e.timeline(&w.show, group).pos;
            assert!(pos > 0.0, "{k:?} fired the group");
            // Pressed again, it is GO on the main list: the group isn't paused.
            let before = w.state(group);
            w.key(k);
            let after = w.state(group);
            assert!(
                !(before == RunState::Running && paused(after)),
                "{k:?} paused the group"
            );
        }
    }
}

#[test]
fn an_editor_holding_the_keys_takes_space_for_its_group_and_0_stays_go() {
    let mut w = win(false);
    let group = w.group;
    w.apply(Action::Timeline(TimelineAction::Open(Some(group))));
    w.frame(vec![]);
    w.frame(vec![]);
    // Click an empty spot of its tracks: the editor holds the keys.
    let r =
        w.m.ctx
            .data(|d| d.get_temp::<TimelineRects>(egui::Id::new(("timeline_rects", Host::Pane))))
            .expect("the pane's editor");
    let spot = pos2(r.ruler.right() - 20.0, r.ruler.bottom() + 6.0);
    w.m.click(&mut w.show, &mut w.st, spot, PointerButton::Primary);
    w.frame(vec![Event::PointerMoved(pos2(5.0, 5.0))]);
    assert_eq!(w.st.timeline.focus, Some(Host::Pane));
    w.key(Key::Space);
    w.run(group, 0.4);
    assert_eq!(w.state(group), RunState::Running, "Space ran the group");
    w.key(Key::Space);
    assert!(paused(w.state(group)), "and paused it");
    w.key(Key::Space);
    w.run(group, 0.2);
    assert_eq!(w.state(group), RunState::Running, "and resumed it");
    // Clicking the cue list gives the keys back to it: Space is GO there.
    w.st.timeline.focus = None;
    w.frame(vec![Event::PointerMoved(pos2(5.0, 5.0))]);
    w.key(Key::Space);
    assert_eq!(w.state(group), RunState::Running, "GO doesn't pause it");
}

/// The engine's state for what a key's press leaves: each run's state, loop
/// and parking, the main list's playhead and what plays; and the clocks (which
/// `same` lets differ by a tick or two: texts are drawn on a thread).
fn snapshot(w: &mut Win) -> (String, [f64; 2]) {
    let (l, g) = (w.list, w.group);
    let (a, b) = (w.m.e.timeline(&w.show, l), w.m.e.timeline(&w.show, g));
    (
        format!(
            "{:?} {:?} {} {:?} {:?} {} {:?} {:?}",
            a.state,
            a.looping.map(|l| l.last_pass),
            a.parked,
            b.state,
            b.looping.map(|l| l.last_pass),
            b.parked,
            w.m.e.playhead,
            w.m.e.active.len()
        ),
        [a.pos, b.pos],
    )
}

fn same(a: &(String, [f64; 2]), b: &(String, [f64; 2])) {
    assert_eq!(a.0, b.0);
    for i in 0..2 {
        assert!((a.1[i] - b.1[i]).abs() < 0.3, "{:?} {:?}", a.1, b.1);
    }
}

#[test]
fn at_a_pause_cue_space_does_what_go_does() {
    // The group's pause holds everything; the list's lets the content run on.
    for (on_list, show_mode) in [(true, false), (true, true), (false, false)] {
        let mut out = Vec::new();
        for k in [Key::Space, Key::Num0] {
            let mut w = win(show_mode);
            let key = if on_list {
                w.on_list();
                w.list
            } else {
                w.group
            };
            w.key(Key::Num0);
            // To the pause (the group's at 1 s, the list's at 3 s).
            w.run(key, if on_list { 3.4 } else { 1.4 });
            assert!(
                matches!(
                    w.state(key),
                    RunState::Paused {
                        waits_for_go: true,
                        ..
                    }
                ),
                "{:?}",
                w.state(key)
            );
            if !on_list {
                // The group's editor holds the keys, for Space to be its.
                w.apply(Action::Timeline(TimelineAction::Open(Some(key))));
                w.frame(vec![]);
                w.st.timeline.focus = Some(Host::Pane);
                w.frame(vec![]);
            }
            w.key(k);
            w.run(key, 0.3);
            assert_eq!(w.state(key), RunState::Running, "{k:?} went on");
            out.push(snapshot(&mut w));
        }
        same(&out[0], &out[1]);
    }
}

#[test]
fn in_a_loop_space_leaves_it_as_go_does_and_the_pause_button_still_pauses() {
    let looped = || {
        let mut w = win(false);
        let TimelineKey::List(l) = w.list else {
            unreachable!()
        };
        // A loop pause at 2.5 s playing again from 1 s, in place of the
        // free-run pause.
        let i = w
            .show
            .cues
            .iter()
            .position(|c| c.list == l && matches!(&c.kind, CueKind::Control(_)))
            .unwrap();
        w.show.cues[i].pre_wait = 2.5;
        if let CueKind::Control(c) = &mut w.show.cues[i].kind {
            c.loop_from = Some(crate::timeline::TimelinePoint::Time(1.0));
        }
        w.on_list();
        w.key(Key::Num0);
        w.run(w.list, 2.8);
        assert!(w.m.e.timeline(&w.show, w.list).looping.is_some());
        w
    };
    let mut out = Vec::new();
    for k in [Key::Space, Key::Num0] {
        let mut w = looped();
        let key = w.list;
        w.key(k);
        assert_eq!(w.state(key), RunState::Running, "{k:?} didn't pause");
        let l = w.m.e.timeline(&w.show, key).looping;
        assert!(
            l.is_some_and(|l| l.last_pass),
            "{k:?} asked to leave: {l:?}"
        );
        // Again in the last pass: as GO, nothing.
        w.key(k);
        out.push(snapshot(&mut w));
    }
    same(&out[0], &out[1]);
    // The toolbar's Pause still pauses in a loop.
    let mut w = looped();
    w.apply(Action::Timeline(TimelineAction::Pause(w.list)));
    assert!(paused(w.state(w.list)));
}

#[test]
fn an_output_window_with_the_keys_follows_the_same_rules() {
    // What `app.rs::handle_output_key` calls: no editor is under the pointer.
    let space =
        |w: &mut Win| super::super::super::space_key(None, &mut w.show, &mut w.m.e, &mut w.st);
    let go = |w: &mut Win| super::super::super::go_key(&mut w.show, &mut w.m.e, &mut w.st);
    let mut w = win(true);
    w.on_list();
    let key = w.list;
    space(&mut w);
    w.run(key, 0.4);
    assert_eq!(w.state(key), RunState::Running);
    go(&mut w);
    assert_eq!(
        w.state(key),
        RunState::Running,
        "0: GO does nothing while it runs"
    );
    space(&mut w);
    assert!(paused(w.state(key)));
    // On the main list, Space is GO: the group fires.
    let mut w = win(true);
    space(&mut w);
    w.run(w.group, 0.3);
    assert!(w.m.e.timeline(&w.show, w.group).pos > 0.0);
}
