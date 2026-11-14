//! The editor driven with the mouse, as the cue list's tests do (`ListMouse`):
//! where things are comes from `TimelineRects`, and what it asks for is the
//! `TimelineAction`s it gives back. The edits and the engine's runs are other
//! packages' work; nothing here depends on what they do.

use super::control::RUN_OUT_CHOICES;
use super::*;
use crate::show::{Cue, CueKind, Keyframe, RunOut, TimelineSettings};
use crate::timeline::{Marker, snap};
use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect, pos2, vec2};

fn button(pos: Pos2, pressed: bool, button: PointerButton) -> Event {
    Event::PointerButton {
        pos,
        button,
        pressed,
        modifiers: Default::default(),
    }
}

/// What a frame draws.
#[derive(Clone, Copy, PartialEq)]
enum Draw {
    /// The Timeline pane's body.
    Pane,
    /// The cue inspector.
    Inspector,
    /// The pane in a window of its own.
    Popout,
    /// The new interface.
    Modern,
}

/// Draws the editor (or what holds it) frame by frame with made-up input.
struct Mouse {
    ctx: egui::Context,
    e: Engine,
    t: f64,
    mods: Modifiers,
    sent: Modifiers,
    draw: Draw,
}

impl Mouse {
    fn new() -> Mouse {
        Mouse {
            ctx: egui::Context::default(),
            e: Engine::for_tests(),
            t: 1.0,
            mods: Modifiers::default(),
            sent: Modifiers::default(),
            draw: Draw::Pane,
        }
    }

    /// One frame; the actions it asked for (not made).
    fn frame(&mut self, show: &mut Show, st: &mut UiState, mut events: Vec<Event>) -> Vec<Action> {
        self.t += 1.0 / 60.0;
        if self.mods != self.sent {
            events.insert(0, Event::ModifiersChanged(self.mods));
            self.sent = self.mods;
        }
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 520.0))),
            time: Some(self.t),
            events,
            ..Default::default()
        };
        let mut actions = Vec::new();
        let e = &mut self.e;
        let draw = self.draw;
        self.ctx
            .data_mut(|d| d.remove::<Vec<&'static str>>(egui::Id::new("timeline_rows")));
        let mut out = self.ctx.run_ui(input, |ui| match draw {
            Draw::Pane => {
                egui::CentralPanel::default().show(ui, |ui| pane(ui, show, e, st, &mut actions));
            }
            Draw::Inspector => {
                egui::CentralPanel::default().show(ui, |ui| {
                    super::super::inspector(ui, show, e, st, &mut actions)
                });
            }
            Draw::Popout => super::super::draw_popout(ui, Pane::Timeline, show, e, st),
            Draw::Modern => super::super::modern::draw(ui, show, e, st, &mut actions),
        });
        out.textures_delta.clear();
        actions
    }

    /// A frame whose actions are made, as the app makes them.
    fn frame_apply(&mut self, show: &mut Show, st: &mut UiState, events: Vec<Event>) {
        for a in self.frame(show, st, events) {
            super::super::apply(a, show, &mut self.e, st);
        }
    }

    fn rects(&self) -> Option<TimelineRects> {
        self.ctx
            .data(|d| d.get_temp::<TimelineRects>(egui::Id::new(("timeline_rects", Host::Pane))))
    }

    fn clip(&mut self, show: &mut Show, st: &mut UiState, id: CueId) -> Rect {
        self.frame(show, st, vec![]);
        let r = self.rects().expect("the editor was drawn");
        r.clips
            .iter()
            .find(|c| c.0 == id)
            .expect("the clip is drawn")
            .1
    }

    fn button_at(&self, name: &str) -> Rect {
        let all: Vec<(&'static str, Rect)> = self
            .ctx
            .data(|d| d.get_temp(egui::Id::new(("timeline_buttons", Host::Pane))))
            .unwrap_or_default();
        all.iter()
            .find(|b| b.0 == name)
            .unwrap_or_else(|| panic!("no {name} button"))
            .1
    }

    fn rows(&self) -> Vec<&'static str> {
        self.ctx
            .data(|d| d.get_temp(egui::Id::new("timeline_rows")))
            .unwrap_or_default()
    }

    /// Press at `a`, move to `b` in steps and let go there; every action asked
    /// for, by the frame it came in (the last is the release).
    fn drag(&mut self, show: &mut Show, st: &mut UiState, a: Pos2, b: Pos2) -> Vec<Vec<Action>> {
        let mut frames = vec![
            self.frame(show, st, vec![Event::PointerMoved(a)]),
            self.frame(show, st, vec![button(a, true, PointerButton::Primary)]),
        ];
        for i in 1..=8 {
            let p = a + (b - a) * (i as f32 / 8.0);
            frames.push(self.frame(show, st, vec![Event::PointerMoved(p)]));
        }
        frames.push(self.frame(
            show,
            st,
            vec![
                Event::PointerMoved(b),
                button(b, false, PointerButton::Primary),
            ],
        ));
        frames
    }

    fn click(
        &mut self,
        show: &mut Show,
        st: &mut UiState,
        at: Pos2,
        b: PointerButton,
    ) -> Vec<Action> {
        let mut out = self.frame(show, st, vec![Event::PointerMoved(at)]);
        out.extend(self.frame(show, st, vec![button(at, true, b)]));
        out.extend(self.frame(show, st, vec![button(at, false, b)]));
        out
    }
}

fn timeline_actions(actions: &[Action]) -> Vec<TimelineAction> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Timeline(t) => Some(t.clone()),
            _ => None,
        })
        .collect()
}

/// A picture of `end` seconds on `layer`, starting at `at`.
fn picture(show: &mut Show, name: &str, at: f64, end: f64, layer: i32, group: CueId) -> CueId {
    let mut c = show.make_cue(CueType::Image, None, show.cues.len());
    c.name = name.into();
    c.pre_wait = at;
    c.parent = Some(group);
    if let Some(m) = c.kind.media_mut() {
        m.end = Some(end);
        m.layer = layer;
        m.fade_in = 0.0;
        m.fade_out = 0.0;
    }
    let id = c.id;
    show.cues.push(c);
    id
}

/// A timeline group "T" with pictures A (0–4 s) and B (6–10 s) on layer 0, C
/// (2–5 s) on layer 1, a 2 s wait W at 1 s on the cue track, and a marker at
/// 5 s. The group is selected.
struct Fixture {
    show: Show,
    st: UiState,
    group: CueId,
    a: CueId,
    b: CueId,
    c: CueId,
    w: CueId,
}

fn fixture() -> Fixture {
    let mut show = Show::default();
    let mut g = show.make_cue(CueType::Group, None, 0);
    g.name = "T".into();
    if let CueKind::Group(gc) = &mut g.kind {
        gc.timeline = Some(TimelineSettings {
            markers: vec![Marker {
                id: 1,
                time: 5.0,
                name: "Chorus".into(),
                color: None,
            }],
            ..TimelineSettings::for_editor()
        });
    }
    let group = g.id;
    show.cues.push(g);
    let a = picture(&mut show, "A", 0.0, 4.0, 0, group);
    let b = picture(&mut show, "B", 6.0, 4.0, 0, group);
    let c = picture(&mut show, "C", 2.0, 3.0, 1, group);
    let mut wait = show.make_cue(CueType::Wait, None, show.cues.len());
    wait.name = "W".into();
    wait.pre_wait = 1.0;
    wait.parent = Some(group);
    if let CueKind::Wait(wc) = &mut wait.kind {
        wc.duration = 2.0;
    }
    let w = wait.id;
    show.cues.push(wait);
    let st = UiState {
        selected: Some(group),
        ..UiState::default()
    };
    Fixture {
        show,
        st,
        group,
        a,
        b,
        c,
        w,
    }
}

fn clips_of(show: &Show, key: TimelineKey) -> Vec<model::Clip> {
    let mut e = Engine::for_tests();
    model::clips(show, key, &mut |c| e.cue_duration(show, c))
}

/// Where a clip dragged from `start` by `dx` points lands, as the editor
/// works it out: snapped (by whatever the snapping does) or to a hundredth.
fn expected_start(
    show: &Show,
    key: TimelineKey,
    id: CueId,
    dx: f32,
    pps: f32,
    snapping: bool,
) -> f64 {
    let clips = clips_of(show, key);
    let clip = clips.iter().find(|c| c.id == id).unwrap();
    let raw = clip.start + f64::from(dx / pps);
    if snapping {
        let markers = &model::settings(show, key).unwrap().markers;
        let pts = snap::points(&clips, markers, Some(0.0), &[id]);
        if let (s, Some(_)) = snap::snap_span(raw, clip.len, &pts, f64::from(8.0 / pps)) {
            return s;
        }
    }
    (raw * 100.0).round() / 100.0
}

#[test]
fn edits_are_refused_in_show_mode_and_the_transport_is_not() {
    let key = TimelineKey::List(1);
    assert!(TimelineAction::AddMarker { key, at: 1.0 }.edits_the_show());
    assert!(TimelineAction::NewTimelineGroup.edits_the_show());
    assert!(!TimelineAction::Run(key).edits_the_show());
    assert!(!TimelineAction::Seek { key, pos: 2.0 }.edits_the_show());
    assert!(!TimelineAction::SetLayout(TimelineLayout::Workspace).edits_the_show());
}

#[test]
fn the_editor_follows_a_selected_timeline_group_or_its_child_then_stays() {
    let mut show = Show::default();
    let g = show.make_cue(CueType::Group, None, 0);
    let gid = g.id;
    show.cues.push(g);
    let mut child = show.make_cue(CueType::Video, None, 1);
    child.parent = Some(gid);
    let cid = child.id;
    show.cues.push(child);
    let other = show.make_cue(CueType::Wait, None, 2);
    let oid = other.id;
    show.cues.push(other);
    let engine = Engine::for_tests();
    let mut st = UiState::default();
    assert_eq!(target(&show, &engine, &st), None);
    st.selected = Some(gid);
    assert_eq!(target(&show, &engine, &st), Some(TimelineKey::Group(gid)));
    st.selected = Some(cid);
    assert_eq!(target(&show, &engine, &st), Some(TimelineKey::Group(gid)));
    // Once shown, it stays when a cue on no timeline is picked.
    st.timeline.pinned = Some(TimelineKey::Group(gid));
    st.selected = Some(oid);
    assert_eq!(target(&show, &engine, &st), Some(TimelineKey::Group(gid)));
    // Not once the group is a playlist.
    if let CueKind::Group(g) = &mut show.cues[0].kind {
        g.mode = crate::show::GroupMode::Playlist;
    }
    assert_eq!(target(&show, &engine, &st), None);
}

#[test]
fn the_pane_draws_a_group_timeline_and_the_empty_state_for_none() {
    let mut f = fixture();
    let mut m = Mouse::new();
    let none = &mut UiState::default();
    m.frame(&mut f.show, none, vec![]);
    assert!(m.rects().is_none());
    assert!(
        m.ctx
            .data(|d| d.get_temp::<bool>(egui::Id::new("timeline_empty")))
            .unwrap_or(false)
    );

    let mut m = Mouse::new();
    m.frame(&mut f.show, &mut f.st, vec![]);
    let r = m.rects().expect("the editor was drawn");
    assert_eq!(r.clips.len(), 4);
    // Tracks: layer 1 above layer 0, then the cue track.
    let tracks: Vec<TrackId> = r.tracks.iter().map(|t| t.0).collect();
    let output = f.show.outputs.first().map(|o| o.id);
    assert_eq!(
        tracks,
        vec![
            TrackId::Picture { output, layer: 1 },
            TrackId::Picture { output, layer: 0 },
            TrackId::Cues
        ]
    );
    let at = |id| r.clips.iter().find(|c| c.0 == id).unwrap().1;
    let row = |t: TrackId| r.tracks.iter().find(|x| x.0 == t).unwrap().1;
    // Each clip is in its track's row, from its start for its length.
    for (id, track, start, len) in [
        (f.a, TrackId::Picture { output, layer: 0 }, 0.0, 4.0),
        (f.b, TrackId::Picture { output, layer: 0 }, 6.0, 4.0),
        (f.c, TrackId::Picture { output, layer: 1 }, 2.0, 3.0),
        (f.w, TrackId::Cues, 1.0, 2.0),
    ] {
        let c = at(id);
        assert!(row(track).contains(c.center()), "{id:?}");
        assert!((c.left() - (r.x0 + start * r.pps)).abs() < 0.5, "{id:?}");
        assert!((c.width() - len * r.pps).abs() < 0.5, "{id:?}");
    }
    // Fitted: the whole timeline shows, past its last clip.
    assert!(r.x0 + 10.0 * r.pps < r.ruler.right());
    assert_eq!(r.markers.len(), 1);
    assert!((r.markers[0].1.left() - (r.x0 + 5.0 * r.pps)).abs() < 0.5);
    // Stopped at the start (the engine's run is not built yet).
    assert_eq!(r.playhead_x, Some(r.x0));
}

#[test]
fn dragging_a_clip_asks_for_one_move_on_release_and_alt_turns_snapping_off() {
    for alt in [false, true] {
        let mut f = fixture();
        let key = TimelineKey::Group(f.group);
        let mut m = Mouse::new();
        let a = m.clip(&mut f.show, &mut f.st, f.a);
        let pps = m.rects().unwrap().pps;
        m.mods = if alt { Modifiers::ALT } else { Modifiers::NONE };
        let dx = 113.0;
        let frames = m.drag(
            &mut f.show,
            &mut f.st,
            a.center(),
            a.center() + vec2(dx, 0.0),
        );
        let (last, before) = frames.split_last().unwrap();
        // Nothing while it moves; the drag shows in the editor only.
        assert!(
            before.iter().all(|f| timeline_actions(f).is_empty()),
            "an action before the release"
        );
        let want = expected_start(&f.show, key, f.a, dx, pps, !alt);
        match timeline_actions(last).as_slice() {
            [
                TimelineAction::MoveClips {
                    key: k,
                    ids,
                    dt,
                    to,
                },
            ] => {
                assert_eq!((*k, ids.clone(), *to), (key, vec![f.a], None));
                assert!((dt - want).abs() < 1e-9, "dt {dt}, wanted {want}");
            }
            other => panic!("wanted one MoveClips, got {other:?}"),
        }
        // Alt: no snapping at all, to a hundredth of a second.
        if alt {
            let raw = f64::from(dx / pps);
            assert!((want - (raw * 100.0).round() / 100.0).abs() < 1e-12);
        }
        m.mods = Modifiers::NONE;
    }
}

#[test]
fn a_clip_of_the_selection_drags_the_selection_and_to_another_track() {
    let mut f = fixture();
    let key = TimelineKey::Group(f.group);
    let mut m = Mouse::new();
    // Pick A, then Ctrl-click B.
    let a = m.clip(&mut f.show, &mut f.st, f.a);
    for x in m.click(&mut f.show, &mut f.st, a.center(), PointerButton::Primary) {
        super::super::apply(x, &mut f.show, &mut m.e, &mut f.st);
    }
    assert_eq!(f.st.selected, Some(f.a));
    let b = m.clip(&mut f.show, &mut f.st, f.b);
    m.mods = Modifiers::COMMAND;
    for x in m.click(&mut f.show, &mut f.st, b.center(), PointerButton::Primary) {
        super::super::apply(x, &mut f.show, &mut m.e, &mut f.st);
    }
    m.mods = Modifiers::NONE;
    assert_eq!(f.st.selection(&f.show), vec![f.a, f.b]);
    // Drag B up onto layer 1's track: both move, B is the anchor.
    let r = m.rects().unwrap();
    let up = r.tracks[0].1.center().y;
    let b = m.clip(&mut f.show, &mut f.st, f.b);
    let frames = m.drag(
        &mut f.show,
        &mut f.st,
        b.center(),
        pos2(b.center().x + 40.0, up),
    );
    let output = f.show.outputs.first().map(|o| o.id);
    match timeline_actions(frames.last().unwrap()).as_slice() {
        [
            TimelineAction::MoveClips {
                key: k, ids, to, ..
            },
        ] => {
            assert_eq!(*k, key);
            assert_eq!(ids, &vec![f.b, f.a]);
            assert_eq!(*to, Some(TrackId::Picture { output, layer: 1 }));
        }
        other => panic!("wanted one MoveClips, got {other:?}"),
    }
}

#[test]
fn clicking_a_clip_selects_it_and_the_inspector_follows() {
    let mut f = fixture();
    let mut m = Mouse::new();
    let c = m.clip(&mut f.show, &mut f.st, f.c);
    let actions = m.click(&mut f.show, &mut f.st, c.center(), PointerButton::Primary);
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::Select(id) if *id == f.c))
    );
    for a in actions {
        super::super::apply(a, &mut f.show, &mut m.e, &mut f.st);
    }
    assert_eq!(f.st.selected, Some(f.c));
    // The editor still shows the group (C is on it).
    m.frame(&mut f.show, &mut f.st, vec![]);
    assert_eq!(m.rects().unwrap().clips.len(), 4);
    // The inspector is C's.
    m.draw = Draw::Inspector;
    m.frame(&mut f.show, &mut f.st, vec![]);
    assert!(m.rows().contains(&"run_out"), "{:?}", m.rows());
}

#[test]
fn dragging_an_edge_asks_for_a_trim() {
    let mut f = fixture();
    let mut m = Mouse::new();
    let a = m.clip(&mut f.show, &mut f.st, f.a);
    let pps = m.rects().unwrap().pps;
    let grip = pos2(a.right() - 1.0, a.center().y);
    let frames = m.drag(&mut f.show, &mut f.st, grip, grip + vec2(60.0, 0.0));
    assert!(
        frames[..frames.len() - 1]
            .iter()
            .all(|f| timeline_actions(f).is_empty())
    );
    match timeline_actions(frames.last().unwrap()).as_slice() {
        [TimelineAction::Trim { id, edge, to }] => {
            assert_eq!((*id, *edge), (f.a, Edge::Right));
            // Near where it was let go (snapped or to a hundredth).
            let let_go = f64::from((grip.x + 60.0 - m.rects().unwrap().x0) / pps);
            assert!((to - let_go).abs() < 0.2, "{to} vs {let_go}");
        }
        other => panic!("wanted one Trim, got {other:?}"),
    }
    let c = m.clip(&mut f.show, &mut f.st, f.c);
    let grip = pos2(c.left() + 1.0, c.center().y);
    let frames = m.drag(&mut f.show, &mut f.st, grip, grip - vec2(40.0, 0.0));
    assert!(matches!(
        timeline_actions(frames.last().unwrap()).as_slice(),
        [TimelineAction::Trim { id, edge: Edge::Left, to }] if *id == f.c && *to < 2.0
    ));
}

#[test]
fn the_ruler_moves_a_ghost_playhead_and_seeks_once_on_release() {
    let mut f = fixture();
    let key = TimelineKey::Group(f.group);
    let mut m = Mouse::new();
    m.frame(&mut f.show, &mut f.st, vec![]);
    let r = m.rects().unwrap();
    // Away from the marker's flag.
    let a = pos2(r.x0 + 1.5 * r.pps, r.ruler.center().y);
    let b = pos2(r.x0 + 7.3 * r.pps, r.ruler.center().y);
    let frames = m.drag(&mut f.show, &mut f.st, a, b);
    assert!(
        frames[..frames.len() - 1]
            .iter()
            .all(|f| timeline_actions(f).is_empty())
    );
    // While it moved, the playhead drawn followed the pointer.
    match timeline_actions(frames.last().unwrap()).as_slice() {
        [TimelineAction::Seek { key: k, pos }] => {
            assert_eq!(*k, key);
            assert!((pos - 7.3).abs() < 0.15, "{pos}");
        }
        other => panic!("wanted one Seek, got {other:?}"),
    }
    // Show mode: a group's ruler is locked (parking puts picture on the outputs).
    f.st.show_mode = true;
    let frames = m.drag(&mut f.show, &mut f.st, a, b);
    assert!(frames.iter().all(|f| timeline_actions(f).is_empty()));
}

#[test]
fn ctrl_wheel_zooms_and_keeps_the_time_under_the_pointer() {
    let mut f = fixture();
    let mut m = Mouse::new();
    m.frame(&mut f.show, &mut f.st, vec![]);
    let r = m.rects().unwrap();
    let px = r.x0 + 3.0 * r.pps;
    let at = pos2(px, r.tracks[1].1.center().y);
    let t_before = (px - r.x0) / r.pps;
    m.frame(&mut f.show, &mut f.st, vec![Event::PointerMoved(at)]);
    m.mods = Modifiers::COMMAND;
    m.frame(
        &mut f.show,
        &mut f.st,
        vec![Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, 120.0),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::COMMAND,
        }],
    );
    for _ in 0..30 {
        m.frame(&mut f.show, &mut f.st, vec![]);
        let now = m.rects().unwrap();
        assert!(
            ((px - now.x0) / now.pps - t_before).abs() < 0.01,
            "the time under the pointer moved"
        );
    }
    let after = m.rects().unwrap();
    assert!(after.pps > r.pps * 1.05, "{} -> {}", r.pps, after.pps);
    assert!((f.st.timeline.pps - after.pps).abs() < 1e-3);
}

#[test]
fn the_clip_menu_adds_a_pause_before_the_clip_comes_in() {
    let mut f = fixture();
    let key = TimelineKey::Group(f.group);
    let mut m = Mouse::new();
    let b = m.clip(&mut f.show, &mut f.st, f.b);
    m.click(&mut f.show, &mut f.st, b.center(), PointerButton::Secondary);
    m.frame(&mut f.show, &mut f.st, vec![]);
    let item = m.button_at("pause_in");
    let actions = m.click(
        &mut f.show,
        &mut f.st,
        item.center(),
        PointerButton::Primary,
    );
    assert_eq!(
        timeline_actions(&actions),
        vec![TimelineAction::AddPauseBefore {
            key,
            clip: f.b,
            what: PauseBefore::ClipIn
        }]
    );
}

#[test]
fn a_clip_waiting_for_its_fade_in_draws_longer_than_its_content() {
    let mut f = fixture();
    // C's opacity stays at 0 until 1.5 s into it, then rises.
    let i = f.show.cue_index(f.c).unwrap();
    let m = f.show.cues[i].kind.media_mut().unwrap();
    m.motion.opacity = Some(Animated {
        value: 1.0,
        keys: vec![
            Keyframe {
                t: 0.0,
                v: 0.0,
                curve: Default::default(),
            },
            Keyframe {
                t: 1.5,
                v: 0.0,
                curve: Default::default(),
            },
            Keyframe {
                t: 2.5,
                v: 1.0,
                curve: Default::default(),
            },
        ],
    });
    let mut mouse = Mouse::new();
    let c = mouse.clip(&mut f.show, &mut f.st, f.c);
    let pps = mouse.rects().unwrap().pps;
    // 3 s of content after 1.5 s of waiting.
    assert!(
        (c.width() - 4.5 * pps).abs() < 0.5,
        "{} vs {}",
        c.width(),
        4.5 * pps
    );
    // Without the timeline's rule it is its content's length.
    if let CueKind::Group(g) = &mut f.show.cues[0].kind {
        g.timeline.as_mut().unwrap().start_on_fade_in = false;
    }
    let c = mouse.clip(&mut f.show, &mut f.st, f.c);
    assert!((c.width() - 3.0 * mouse.rects().unwrap().pps).abs() < 0.5);
}

#[test]
fn a_clip_the_run_waits_on_to_fade_in_is_hatched() {
    let mut f = fixture();
    let key = TimelineKey::Group(f.group);
    // C waits 1.5 s, unseen, from its start at 2 s, then fades in.
    let i = f.show.cue_index(f.c).unwrap();
    let m = f.show.cues[i].kind.media_mut().unwrap();
    let k = |t, v| Keyframe {
        t,
        v,
        curve: Default::default(),
    };
    m.motion.opacity = Some(Animated {
        value: 1.0,
        keys: vec![k(0.0, 0.0), k(1.5, 0.0), k(2.5, 1.0)],
    });
    let mut mouse = Mouse::new();
    let waiting = |mouse: &Mouse| {
        mouse
            .ctx
            .data_mut(|d| {
                d.remove_temp::<Vec<CueId>>(egui::Id::new(("timeline_waiting", Host::Pane)))
            })
            .unwrap_or_default()
    };
    // Stopped: drawn with its lead-in, but nobody is waiting on it.
    mouse.frame(&mut f.show, &mut f.st, vec![]);
    assert!(waiting(&mouse).is_empty());
    mouse.e.timeline_run(&f.show, key);
    for _ in 0..25 {
        mouse.e.tick(&f.show, 0.1);
    }
    assert_eq!(mouse.e.timeline_waiting(key), vec![f.c]);
    waiting(&mouse);
    mouse.frame(&mut f.show, &mut f.st, vec![]);
    assert!(waiting(&mouse).contains(&f.c));
}

#[test]
fn show_mode_refuses_edit_drags_and_keeps_the_transport() {
    let mut f = fixture();
    let key = TimelineKey::Group(f.group);
    f.st.show_mode = true;
    let mut m = Mouse::new();
    let a = m.clip(&mut f.show, &mut f.st, f.a);
    let frames = m.drag(
        &mut f.show,
        &mut f.st,
        a.center(),
        a.center() + vec2(90.0, 0.0),
    );
    assert!(frames.iter().all(|f| timeline_actions(f).is_empty()));
    let grip = pos2(a.right() - 1.0, a.center().y);
    let frames = m.drag(&mut f.show, &mut f.st, grip, grip + vec2(50.0, 0.0));
    assert!(frames.iter().all(|f| timeline_actions(f).is_empty()));
    for (name, want) in [
        ("run", TimelineAction::Run(key)),
        ("pause", TimelineAction::Pause(key)),
        ("stop", TimelineAction::Stop(key)),
    ] {
        let at = m.button_at(name).center();
        let got = timeline_actions(&m.click(&mut f.show, &mut f.st, at, PointerButton::Primary));
        assert_eq!(got, vec![want]);
    }
    // And the app lets them through in show mode.
    super::super::apply(
        Action::Timeline(TimelineAction::Run(key)),
        &mut f.show,
        &mut m.e,
        &mut f.st,
    );
    assert!(!TimelineAction::Run(key).edits_the_show());
}

/// A control cue in the group, selected; its inspector is drawn.
fn control_rows(action: ControlAction, freeze: PauseFreeze) -> Vec<&'static str> {
    let mut f = fixture();
    let mut cue: Cue = f.show.make_cue(CueType::Control, None, f.show.cues.len());
    cue.parent = Some(f.group);
    if let CueKind::Control(c) = &mut cue.kind {
        c.action = action;
        c.freeze = freeze;
    }
    f.st.selected = Some(cue.id);
    f.show.cues.push(cue);
    let mut m = Mouse::new();
    m.draw = Draw::Inspector;
    m.frame(&mut f.show, &mut f.st, vec![]);
    m.rows()
}

#[test]
fn the_control_inspector_shows_what_a_timeline_action_needs() {
    let start = control_rows(ControlAction::Start, PauseFreeze::Nothing);
    assert!(start.contains(&"cue_picker") && !start.contains(&"freeze"));
    let pause = control_rows(ControlAction::TimelinePause, PauseFreeze::Nothing);
    assert!(!pause.contains(&"cue_picker"), "{pause:?}");
    assert!(pause.contains(&"timeline") && pause.contains(&"freeze"));
    // A free-run pause holds nothing, so live inputs aren't asked about.
    assert!(!pause.contains(&"freeze_live") && !pause.contains(&"clips"));
    let all = control_rows(ControlAction::TimelinePause, PauseFreeze::Everything);
    assert!(all.contains(&"freeze_live") && !all.contains(&"clips"));
    let some = control_rows(ControlAction::TimelinePause, PauseFreeze::Clips);
    assert!(some.contains(&"freeze_live") && some.contains(&"clips"));
    let jump = control_rows(ControlAction::TimelineJump, PauseFreeze::Nothing);
    assert!(jump.contains(&"point") && !jump.contains(&"freeze"));
}

#[test]
fn a_clip_on_a_timeline_offers_end_loop_or_hold_and_the_fade_only_for_end() {
    assert_eq!(
        RUN_OUT_CHOICES.map(|c| c.0),
        [RunOut::End, RunOut::Loop, RunOut::Hold]
    );
    let mut f = fixture();
    f.st.selected = Some(f.a);
    let mut m = Mouse::new();
    m.draw = Draw::Inspector;
    m.frame(&mut f.show, &mut f.st, vec![]);
    let rows = m.rows();
    assert!(rows.contains(&"run_out") && rows.contains(&"run_out_fade"));
    assert!(rows.contains(&"runs_unseen"));
    let i = f.show.cue_index(f.a).unwrap();
    f.show.cues[i].kind.media_mut().unwrap().run_out = RunOut::Loop;
    m.frame(&mut f.show, &mut f.st, vec![]);
    assert!(m.rows().contains(&"run_out") && !m.rows().contains(&"run_out_fade"));
    // A cue on no timeline has none of it; and drawing changed nothing.
    let mut lone = f.show.make_cue(CueType::Image, None, f.show.cues.len());
    lone.parent = None;
    f.st.selected = Some(lone.id);
    f.show.cues.push(lone);
    let before = serde_json::to_string(&f.show).unwrap();
    m.frame(&mut f.show, &mut f.st, vec![]);
    assert!(!m.rows().contains(&"run_out"));
    assert_eq!(serde_json::to_string(&f.show).unwrap(), before);
    // The group's inspector has the timeline's own rules.
    f.st.selected = Some(f.group);
    m.frame(&mut f.show, &mut f.st, vec![]);
    assert!(m.rows().contains(&"settings"));
}

#[test]
fn a_clip_with_a_free_run_pause_in_it_left_to_end_is_warned_about() {
    let mut f = fixture();
    let key = TimelineKey::Group(f.group);
    let mut p = f.show.make_cue(CueType::Control, None, f.show.cues.len());
    p.parent = Some(f.group);
    p.pre_wait = 7.0;
    if let CueKind::Control(c) = &mut p.kind {
        c.action = ControlAction::TimelinePause;
    }
    f.show.cues.push(p);
    let warned = |show: &Show| -> Vec<CueId> {
        clips_of(show, key)
            .iter()
            .filter(|c| tracks::fades_if_held(show, c))
            .map(|c| c.id)
            .collect()
    };
    // B (6–10 s) has the pause in it; A and C don't.
    assert_eq!(warned(&f.show), vec![f.b]);
    let i = f.show.cue_index(f.b).unwrap();
    f.show.cues[i].kind.media_mut().unwrap().run_out = RunOut::Hold;
    assert!(warned(&f.show).is_empty());
}

#[test]
fn a_control_cue_says_what_it_does_to_its_timeline_in_the_list() {
    let mut f = fixture();
    let mut cue = f.show.make_cue(CueType::Control, None, 0);
    cue.parent = Some(f.group);
    let key = TimelineKey::Group(f.group);
    let text = |c: &crate::show::ControlCue, cue: &Cue, show: &Show| describe_control(show, cue, c);
    let mut show = f.show.clone();
    show.cues.push(cue.clone());
    let mut c = crate::show::ControlCue {
        action: ControlAction::TimelinePause,
        ..Default::default()
    };
    assert_eq!(text(&c, &cue, &show), "Pause timeline · content runs on");
    c.freeze = PauseFreeze::Everything;
    c.freeze_live = true;
    assert_eq!(
        text(&c, &cue, &show),
        "Pause timeline · holds everything and live inputs"
    );
    c.freeze = PauseFreeze::Clips;
    c.freeze_live = false;
    c.clips = vec![f.a, f.b];
    assert_eq!(text(&c, &cue, &show), "Pause timeline · holds 2 clips");
    let c = crate::show::ControlCue {
        action: ControlAction::TimelineRun,
        timeline: Some(key),
        at: Some(crate::timeline::TimelinePoint::Marker(1)),
        ..Default::default()
    };
    let name = timeline_name(&show, key);
    assert_eq!(
        text(&c, &cue, &show),
        format!("Run timeline » {name} from marker Chorus")
    );
    let c = crate::show::ControlCue {
        action: ControlAction::TimelineJump,
        ..Default::default()
    };
    assert_eq!(text(&c, &cue, &show), "Jump in timeline (nowhere chosen)");
    // Off any timeline, it says so.
    let lone = show.make_cue(CueType::Control, None, 0);
    assert_eq!(
        text(&c, &lone, &show),
        "Jump in timeline (nowhere chosen) (not on a timeline)"
    );
    // The other actions read as they always did.
    let c = crate::show::ControlCue {
        action: ControlAction::Start,
        target: Some(f.a),
        ..Default::default()
    };
    let a = show.cue(f.a).unwrap();
    assert_eq!(text(&c, &cue, &show), format!("Start » {}", cue_label(a)));
    let memo = crate::show::ControlCue {
        action: ControlAction::Memo,
        ..Default::default()
    };
    assert_eq!(text(&memo, &cue, &show), "");
}

#[test]
fn opening_the_editor_docks_the_pane_or_shows_the_new_interfaces_panel() {
    let mut f = fixture();
    let key = TimelineKey::Group(f.group);
    let mut e = Engine::for_tests();
    super::super::apply(
        Action::Timeline(TimelineAction::Open(Some(key))),
        &mut f.show,
        &mut e,
        &mut f.st,
    );
    assert!(f.st.timeline.open && f.st.layout.is_docked(Pane::Timeline));
    assert_eq!(f.st.timeline.pinned, Some(key));
    super::super::apply(
        Action::Timeline(TimelineAction::Close),
        &mut f.show,
        &mut e,
        &mut f.st,
    );
    assert!(!f.st.timeline.open && !f.st.layout.is_open(Pane::Timeline));
    // The new interface has a panel instead: the pane layout is left alone.
    f.st.prefs.new_layout = true;
    super::super::apply(
        Action::Timeline(TimelineAction::Open(None)),
        &mut f.show,
        &mut e,
        &mut f.st,
    );
    assert!(f.st.timeline.open && !f.st.layout.is_open(Pane::Timeline));
}

#[test]
fn the_pane_pops_out_and_the_new_interfaces_editor_view_has_it() {
    let mut f = fixture();
    let mut m = Mouse::new();
    m.draw = Draw::Popout;
    f.st.layout.pop_out(Pane::Timeline);
    m.frame(&mut f.show, &mut f.st, vec![]);
    assert_eq!(m.rects().expect("drawn in its own window").clips.len(), 4);

    // The Editor view: no panel until it is wanted, then the editor in it.
    let mut m = Mouse::new();
    m.draw = Draw::Modern;
    f.st.prefs.new_layout = true;
    m.frame(&mut f.show, &mut f.st, vec![]);
    assert!(m.rects().is_none());
    f.st.timeline.open = true;
    m.frame(&mut f.show, &mut f.st, vec![]);
    let r = m.rects().expect("the panel draws the editor");
    assert_eq!(r.clips.len(), 4);
    assert!(r.ruler.width() > 300.0);
}

#[test]
fn a_marker_is_dragged_and_its_menu_adds_a_pause_cue_there() {
    let mut f = fixture();
    let key = TimelineKey::Group(f.group);
    let mut m = Mouse::new();
    m.frame(&mut f.show, &mut f.st, vec![]);
    let r = m.rects().unwrap();
    let flag = r.markers[0].1;
    let frames = m.drag(
        &mut f.show,
        &mut f.st,
        flag.center(),
        flag.center() + vec2(r.pps * 2.0, 0.0),
    );
    match timeline_actions(frames.last().unwrap()).as_slice() {
        [TimelineAction::MoveMarker { key: k, id: 1, to }] => {
            assert_eq!(*k, key);
            assert!((to - 7.0).abs() < 0.15, "{to}");
        }
        other => panic!("wanted one MoveMarker, got {other:?}"),
    }
    // Nothing was made: the flag is back where the show has it.
    m.frame(&mut f.show, &mut f.st, vec![]);
    assert_eq!(m.rects().unwrap().markers[0].1, flag);
    // Its menu puts a pause cue at it.
    m.click(
        &mut f.show,
        &mut f.st,
        flag.center(),
        PointerButton::Secondary,
    );
    m.frame(&mut f.show, &mut f.st, vec![]);
    let item = m.button_at("pause_here");
    let actions = m.click(
        &mut f.show,
        &mut f.st,
        item.center(),
        PointerButton::Primary,
    );
    assert_eq!(
        timeline_actions(&actions),
        vec![TimelineAction::AddControl {
            key,
            action: ControlAction::TimelinePause,
            at: 5.0
        }]
    );
}
