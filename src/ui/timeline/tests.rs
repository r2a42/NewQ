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
    /// The whole window (`ui::draw`: its actions are made, and the undo
    /// history kept, as in the app).
    Full,
}

/// Draws the editor (or what holds it) frame by frame with made-up input.
struct Mouse {
    ctx: egui::Context,
    e: Engine,
    t: f64,
    mods: Modifiers,
    sent: Modifiers,
    draw: Draw,
    /// The window's size, if not the one its `draw` has.
    size: Option<egui::Vec2>,
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
            size: None,
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
            screen_rect: Some(Rect::from_min_size(
                Pos2::ZERO,
                self.size.unwrap_or(if self.draw == Draw::Full {
                    vec2(1300.0, 900.0)
                } else {
                    vec2(1000.0, 520.0)
                }),
            )),
            time: Some(self.t),
            events,
            ..Default::default()
        };
        let mut actions = Vec::new();
        let e = &mut self.e;
        let draw = self.draw;
        self.ctx.data_mut(|d| {
            d.remove::<Vec<&'static str>>(egui::Id::new("timeline_rows"));
            d.remove::<Vec<Host>>(egui::Id::new("timeline_editors"));
            d.remove::<Vec<(&'static str, Rect)>>(egui::Id::new("workspace_buttons"));
            d.remove::<Vec<(String, Rect)>>(egui::Id::new("bin_items"));
            d.remove::<Vec<(super::super::modern::View, Rect)>>(egui::Id::new("modern_views"));
            d.remove::<Vec<(bool, Rect)>>(egui::Id::new("settings_timeline_layout"));
            d.remove::<Vec<(bool, Rect)>>(egui::Id::new("settings_timeline_follow"));
            d.remove::<Vec<(CueId, Rect)>>(egui::Id::new("cue_rows"));
            d.remove::<bool>(egui::Id::new("timeline_pointer"));
            d.remove::<Vec<(crate::show::ControlAction, Rect, Rect)>>(egui::Id::new(
                "control_action_rows",
            ));
        });
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
            Draw::Full => super::super::draw(ui, show, e, st),
        });
        out.textures_delta.clear();
        actions
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
fn a_panic_lets_go_of_a_drag_and_a_renaming() {
    let mut f = fixture();
    f.st.timeline.drag = Some((
        Host::Pane,
        Drag::Playhead {
            pos: Some(3.0),
            mark: Default::default(),
        },
    ));
    f.st.timeline.renaming = Some((Host::Pane, 1, "Cho".into()));
    f.st.timeline.pps = 40.0;
    f.st.after_panic();
    assert!(f.st.timeline.drag.is_none() && f.st.timeline.renaming.is_none());
    assert_eq!(f.st.timeline.pps, 40.0, "the view stays");
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
    // (A drag over a clip that can't move is a rubber band: it only picks.)
    let edits = |frames: &[Vec<Action>]| {
        frames
            .iter()
            .flat_map(|f| timeline_actions(f))
            .filter(|a| a.edits_the_show())
            .count()
    };
    assert_eq!(edits(&frames), 0);
    let grip = pos2(a.right() - 1.0, a.center().y);
    let frames = m.drag(&mut f.show, &mut f.st, grip, grip + vec2(50.0, 0.0));
    assert_eq!(edits(&frames), 0);
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
fn a_loop_pause_inspector_asks_about_the_loop_not_what_it_holds() {
    let mut f = fixture();
    let mut cue: Cue = f.show.make_cue(CueType::Control, None, f.show.cues.len());
    cue.parent = Some(f.group);
    cue.pre_wait = 3.0;
    if let CueKind::Control(c) = &mut cue.kind {
        c.action = ControlAction::TimelinePause;
        c.freeze = PauseFreeze::Clips;
        c.loop_from = Some(crate::show::TimelinePoint::Time(1.0));
        assert!(c.description().contains("again and again"));
        assert_ne!(c.description(), c.action.description());
    }
    f.st.selected = Some(cue.id);
    f.show.cues.push(cue);
    let mut m = Mouse::new();
    m.draw = Draw::Inspector;
    m.frame(&mut f.show, &mut f.st, vec![]);
    let rows = m.rows();
    for row in ["timeline", "loop", "loop_from", "leave_at_once"] {
        assert!(rows.contains(&row), "{row} missing: {rows:?}");
    }
    for row in ["freeze", "clips", "freeze_live"] {
        assert!(!rows.contains(&row), "{row} shown for a loop: {rows:?}");
    }
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

// ----- End to end: the editor, the show, the undo history and the engine -----

/// The fixture with its pictures showing `color_red.png`, the engine able to
/// play them; None (the test skips) without FFmpeg or the file.
fn with_pictures() -> Option<(Fixture, Mouse)> {
    let (ff, img) = crate::media::test_media("color_red.png")?;
    let mut f = fixture();
    for id in [f.a, f.b, f.c] {
        let i = f.show.cue_index(id).unwrap();
        f.show.cues[i].kind.media_mut().unwrap().file = img.clone();
    }
    let mut m = Mouse::new();
    m.e.ff = Some(ff);
    Some((f, m))
}

fn playing(e: &Engine, id: CueId) -> bool {
    matches!(e.status(id), crate::engine::CueStatus::Playing { .. })
}

/// Tick the engine in real time until `cond`, at most `secs`.
fn until(
    m: &mut Mouse,
    show: &Show,
    what: &str,
    secs: u64,
    mut cond: impl FnMut(&mut Engine) -> bool,
) {
    let t0 = std::time::Instant::now();
    while !cond(&mut m.e) {
        assert!(
            t0.elapsed().as_secs() < secs,
            "timed out waiting for {what}"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
        m.e.tick(show, 0.005);
    }
}

#[test]
fn a_clip_dragged_in_the_window_moves_and_one_undo_puts_it_back() {
    let mut f = fixture();
    let mut m = Mouse::new();
    m.draw = Draw::Full;
    super::super::apply(
        Action::Timeline(TimelineAction::Open(None)),
        &mut f.show,
        &mut m.e,
        &mut f.st,
    );
    let before = f.show.cue(f.b).unwrap().pre_wait;
    let b = m.clip(&mut f.show, &mut f.st, f.b);
    let pps = m.rects().unwrap().pps;
    m.drag(
        &mut f.show,
        &mut f.st,
        b.center(),
        b.center() + vec2(1.5 * pps, 0.0),
    );
    // A frame or two for the history to see it.
    m.frame(&mut f.show, &mut f.st, vec![]);
    let moved = f.show.cue(f.b).unwrap().pre_wait;
    assert!((moved - before - 1.5).abs() < 0.3, "{before} -> {moved}");
    super::super::apply(Action::Undo, &mut f.show, &mut m.e, &mut f.st);
    assert_eq!(f.show.cue(f.b).unwrap().pre_wait, before);
}

#[test]
fn letting_go_on_the_ruler_parks_the_timeline_with_its_picture_up() {
    let Some((mut f, mut m)) = with_pictures() else {
        return;
    };
    let key = TimelineKey::Group(f.group);
    let out = f.show.outputs[0].id;
    m.frame(&mut f.show, &mut f.st, vec![]);
    let r = m.rects().unwrap();
    let a = pos2(r.x0 + 1.5 * r.pps, r.ruler.center().y);
    let b = pos2(r.x0 + 3.0 * r.pps, r.ruler.center().y);
    for frame in m.drag(&mut f.show, &mut f.st, a, b) {
        for a in frame {
            super::super::apply(a, &mut f.show, &mut m.e, &mut f.st);
        }
    }
    let v = m.e.timeline(&f.show, key);
    assert!(
        matches!(
            v.state,
            RunState::Paused {
                waits_for_go: true,
                ..
            }
        ),
        "{:?}",
        v.state
    );
    assert!((v.pos - 3.0).abs() < 0.15, "{}", v.pos);
    assert!(super::toolbar::state_text(&v, v.pos).starts_with("Parked"));
    // A (0-4 s) and C (2-5 s) are under the playhead.
    let show = f.show.clone();
    until(&mut m, &show, "the pictures", 20, |e| {
        e.layers(out).len() == 2
    });
}

#[test]
fn run_runs_the_timeline_and_a_pause_before_a_clip_brings_it_in_on_go() {
    let Some((mut f, mut m)) = with_pictures() else {
        return;
    };
    let key = TimelineKey::Group(f.group);
    // The clip menu's pause before B comes in (6 s).
    super::super::apply(
        Action::Timeline(TimelineAction::AddPauseBefore {
            key,
            clip: f.b,
            what: PauseBefore::ClipIn,
        }),
        &mut f.show,
        &mut m.e,
        &mut f.st,
    );
    // The toolbar's Run.
    m.frame(&mut f.show, &mut f.st, vec![]);
    let run = m.button_at("run").center();
    for a in m.click(&mut f.show, &mut f.st, run, PointerButton::Primary) {
        super::super::apply(a, &mut f.show, &mut m.e, &mut f.st);
    }
    assert_eq!(m.e.timeline(&f.show, key).state, RunState::Running);
    // The clock runs to the pause and stops there; B is not in.
    let show = f.show.clone();
    for _ in 0..70 {
        m.e.tick(&show, 0.1);
    }
    let v = m.e.timeline(&show, key);
    assert!(
        matches!(
            v.state,
            RunState::Paused {
                waits_for_go: true,
                ..
            }
        ),
        "{:?}",
        v.state
    );
    assert!((v.pos - 6.0).abs() < 1e-6, "{}", v.pos);
    assert_eq!(v.on_go, Some(f.b));
    // A free-run pause, not a seek.
    assert_eq!(
        super::toolbar::state_text(&v, v.pos),
        "Paused · content runs on"
    );
    assert!(!playing(&m.e, f.b));
    super::super::apply(Action::Go, &mut f.show, &mut m.e, &mut f.st);
    let show = f.show.clone();
    until(&mut m, &show, "B", 20, |e| playing(e, f.b));
    assert_eq!(m.e.timeline(&show, key).state, RunState::Running);
}

#[test]
fn a_tap_on_the_ruler_pressed_and_let_go_in_one_frame_seeks_once() {
    let mut f = fixture();
    let key = TimelineKey::Group(f.group);
    let mut m = Mouse::new();
    m.frame(&mut f.show, &mut f.st, vec![]);
    let r = m.rects().unwrap();
    let at = pos2(r.x0 + 3.3 * r.pps, r.ruler.center().y);
    let mut all = m.frame(&mut f.show, &mut f.st, vec![Event::PointerMoved(at)]);
    all.extend(m.frame(
        &mut f.show,
        &mut f.st,
        vec![
            button(at, true, PointerButton::Primary),
            button(at, false, PointerButton::Primary),
        ],
    ));
    all.extend(m.frame(&mut f.show, &mut f.st, vec![]));
    match timeline_actions(&all).as_slice() {
        [TimelineAction::Seek { key: k, pos }] => {
            assert_eq!(*k, key);
            assert!((pos - 3.3).abs() < 0.15, "{pos}");
        }
        other => panic!("wanted one Seek, got {other:?}"),
    }
}

#[test]
fn a_click_at_the_very_start_of_the_ruler_parks_on_the_first_frame() {
    let Some((mut f, mut m)) = with_pictures() else {
        return;
    };
    let key = TimelineKey::Group(f.group);
    let out = f.show.outputs[0].id;
    m.frame(&mut f.show, &mut f.st, vec![]);
    let r = m.rects().unwrap();
    // At 0 (a point in): what is asked for is 0, or a hair past it.
    let at = pos2(r.x0 + 0.5, r.ruler.center().y);
    for a in m.click(&mut f.show, &mut f.st, at, PointerButton::Primary) {
        if let Action::Timeline(TimelineAction::Seek { pos, .. }) = &a {
            assert!(*pos < 0.05, "{pos}");
        }
        super::super::apply(a, &mut f.show, &mut m.e, &mut f.st);
    }
    let v = m.e.timeline(&f.show, key);
    assert!(matches!(v.state, RunState::Paused { .. }), "{:?}", v.state);
    assert!(v.pos > 0.0 && v.pos < 0.05, "{}", v.pos);
    // A (from 0) is up.
    let show = f.show.clone();
    until(&mut m, &show, "the first picture", 20, |e| {
        e.layers(out).len() == 1
    });
}

#[test]
fn the_toolbar_buttons_stay_put_as_the_run_state_text_changes() {
    let mut f = fixture();
    let mut m = Mouse::new();
    let key = TimelineKey::Group(f.group);
    super::super::apply(
        Action::Timeline(TimelineAction::AddPauseBefore {
            key,
            clip: f.b,
            what: PauseBefore::ClipIn,
        }),
        &mut f.show,
        &mut m.e,
        &mut f.st,
    );
    let names = ["run", "pause", "stop", "marker"];
    let at = |m: &mut Mouse, f: &mut Fixture| {
        m.frame(&mut f.show, &mut f.st, vec![]);
        names.map(|n| m.button_at(n))
    };
    let stopped = at(&mut m, &mut f);
    m.e.timeline_run(&f.show, key);
    m.e.tick(&f.show, 0.5);
    assert_eq!(m.e.timeline(&f.show, key).state, RunState::Running);
    assert_eq!(at(&mut m, &mut f), stopped, "running · next pause in …");
    m.e.tick(&f.show, 6.0);
    assert!(matches!(
        m.e.timeline(&f.show, key).state,
        RunState::Paused { .. }
    ));
    assert_eq!(at(&mut m, &mut f), stopped, "paused, with what GO does");
}

// ----- The tools: split, delete and ripple, adding, tracks, bands, fades -----

/// A key pressed and let go, with `modifiers` held.
fn key_press(key: egui::Key, modifiers: Modifiers) -> Vec<Event> {
    [true, false]
        .into_iter()
        .map(|pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        })
        .collect()
}

/// The whole window with the Timeline pane open on the fixture's group.
fn in_window(f: &mut Fixture) -> Mouse {
    let mut m = Mouse::new();
    m.draw = Draw::Full;
    super::super::apply(
        Action::Timeline(TimelineAction::Open(None)),
        &mut f.show,
        &mut m.e,
        &mut f.st,
    );
    m.frame(&mut f.show, &mut f.st, vec![]);
    m
}

/// Pick a clip with a click (made as the app makes it).
fn pick(m: &mut Mouse, f: &mut Fixture, id: CueId) {
    let r = m.clip(&mut f.show, &mut f.st, id);
    let actions = m.click(&mut f.show, &mut f.st, r.center(), PointerButton::Primary);
    for a in actions {
        super::super::apply(a, &mut f.show, &mut m.e, &mut f.st);
    }
    m.frame(&mut f.show, &mut f.st, vec![]);
}

fn starts(show: &Show, ids: &[CueId]) -> Vec<f64> {
    ids.iter()
        .map(|id| show.cue(*id).unwrap().pre_wait)
        .collect()
}

#[test]
fn ctrl_k_splits_the_picked_clip_at_the_playhead_and_one_undo_joins_it_again() {
    let mut f = fixture();
    let key = TimelineKey::Group(f.group);
    // A hotkey K elsewhere in the show must not fire.
    let mut hot = f.show.make_cue(CueType::Wait, None, f.show.cues.len());
    hot.hotkey = Some("K".into());
    let hot_id = hot.id;
    f.show.cues.push(hot);
    let mut m = in_window(&mut f);
    let a = f.a;
    pick(&mut m, &mut f, a);
    assert_eq!(f.st.selected, Some(f.a));
    super::super::apply(
        Action::Timeline(TimelineAction::Seek { key, pos: 2.5 }),
        &mut f.show,
        &mut m.e,
        &mut f.st,
    );
    let before = model::members(&f.show, key).len();
    let a = m.clip(&mut f.show, &mut f.st, f.a);
    m.frame(
        &mut f.show,
        &mut f.st,
        vec![Event::PointerMoved(a.center())],
    );
    m.mods = Modifiers::COMMAND;
    m.frame(
        &mut f.show,
        &mut f.st,
        key_press(egui::Key::K, Modifiers::COMMAND),
    );
    m.mods = Modifiers::NONE;
    m.frame(&mut f.show, &mut f.st, vec![]);
    assert_eq!(
        model::members(&f.show, key).len(),
        before + 1,
        "split in two"
    );
    assert_eq!(
        f.show.cue(f.a).unwrap().kind.media().unwrap().end,
        Some(2.5),
        "the first half ends at the playhead"
    );
    assert!(!playing(&m.e, hot_id), "the hotkey K didn't fire");
    super::super::apply(Action::Undo, &mut f.show, &mut m.e, &mut f.st);
    assert_eq!(model::members(&f.show, key).len(), before);
    assert_eq!(
        f.show.cue(f.a).unwrap().kind.media().unwrap().end,
        Some(4.0)
    );
}

#[test]
fn split_comes_from_the_toolbar_and_the_clip_menu_with_the_sound_warning() {
    let mut f = fixture();
    let key = TimelineKey::Group(f.group);
    let mut m = Mouse::new();
    m.e.timeline_seek(&f.show, key, 7.0, false);
    // No clip picked (the group is): the toolbar splits what is under the
    // playhead (B).
    m.frame(&mut f.show, &mut f.st, vec![]);
    let at = m.button_at("split").center();
    let got = timeline_actions(&m.click(&mut f.show, &mut f.st, at, PointerButton::Primary));
    assert_eq!(
        got,
        vec![TimelineAction::Split {
            key,
            ids: vec![f.b],
            at: 7.0
        }]
    );
    // The clip menu: where it was right-clicked.
    let b = m.clip(&mut f.show, &mut f.st, f.b);
    let spot = pos2(b.left() + b.width() * 0.75, b.center().y);
    m.click(&mut f.show, &mut f.st, spot, PointerButton::Secondary);
    m.frame(&mut f.show, &mut f.st, vec![]);
    let item = m.button_at("split_here").center();
    let got = timeline_actions(&m.click(&mut f.show, &mut f.st, item, PointerButton::Primary));
    match got.as_slice() {
        [TimelineAction::Split { ids, at, .. }] => {
            assert_eq!(ids, &vec![f.b]);
            assert!((at - 9.0).abs() < 0.2, "{at}");
        }
        other => panic!("wanted one Split, got {other:?}"),
    }
    // A sound split says it may click at the join.
    let i = f.show.cue_index(f.b).unwrap();
    let mut sound = crate::show::MediaCue::new("x.wav".into(), None);
    sound.end = Some(4.0);
    f.show.cues[i].kind = CueKind::Audio(sound);
    let mut e = Engine::for_tests();
    super::super::apply(
        Action::Timeline(TimelineAction::Split {
            key,
            ids: vec![f.b],
            at: 7.0,
        }),
        &mut f.show,
        &mut e,
        &mut f.st,
    );
    let said: Vec<String> = e.messages.iter().map(|m| m.1.clone()).collect();
    let said = said.join("\n");
    assert!(said.contains("may click"), "{said}");
}

#[test]
fn shift_delete_ripples_the_picked_clip_and_plain_delete_leaves_the_gap() {
    for (shift, toggle, ripples) in [
        (true, false, true),
        (false, false, false),
        (false, true, true),
    ] {
        let mut f = fixture();
        let key = TimelineKey::Group(f.group);
        f.st.timeline.ripple = toggle;
        let mut m = in_window(&mut f);
        let a = f.a;
        pick(&mut m, &mut f, a);
        let a = m.clip(&mut f.show, &mut f.st, f.a);
        m.frame(
            &mut f.show,
            &mut f.st,
            vec![Event::PointerMoved(a.center())],
        );
        let mods = if shift {
            Modifiers::SHIFT
        } else {
            Modifiers::NONE
        };
        m.mods = mods;
        m.frame(&mut f.show, &mut f.st, key_press(egui::Key::Delete, mods));
        m.mods = Modifiers::NONE;
        m.frame(&mut f.show, &mut f.st, vec![]);
        assert!(f.show.cue(f.a).is_none(), "A is gone");
        assert_eq!(model::key_of(&f.show, f.b), Some(key));
        // B (6 s) was on A's track: rippled up by A's 4 s, or left.
        let want = if ripples { 2.0 } else { 6.0 };
        assert_eq!(starts(&f.show, &[f.b, f.c]), vec![want, 2.0]);
        super::super::apply(Action::Undo, &mut f.show, &mut m.e, &mut f.st);
        assert!(f.show.cue(f.a).is_some());
        assert_eq!(starts(&f.show, &[f.b]), vec![6.0]);
    }
}

#[test]
fn delete_of_a_selection_that_is_not_all_clips_is_the_cue_lists() {
    let mut f = fixture();
    let mut m = in_window(&mut f);
    // The group itself picked: Delete removes the group, as the list does.
    f.st.selected = Some(f.group);
    m.frame(&mut f.show, &mut f.st, vec![]);
    let r = m.rects().unwrap();
    m.frame(
        &mut f.show,
        &mut f.st,
        vec![Event::PointerMoved(r.tracks[0].1.center())],
    );
    m.frame(
        &mut f.show,
        &mut f.st,
        key_press(egui::Key::Delete, Modifiers::NONE),
    );
    assert!(f.show.cue(f.group).is_none());
}

#[test]
fn the_clip_menu_deletes_and_ripple_deletes_the_clip() {
    let mut f = fixture();
    let key = TimelineKey::Group(f.group);
    let mut m = Mouse::new();
    for (item, ripple) in [("delete", false), ("ripple_delete", true)] {
        let a = m.clip(&mut f.show, &mut f.st, f.a);
        m.click(&mut f.show, &mut f.st, a.center(), PointerButton::Secondary);
        m.frame(&mut f.show, &mut f.st, vec![]);
        let at = m.button_at(item).center();
        let got = timeline_actions(&m.click(&mut f.show, &mut f.st, at, PointerButton::Primary));
        assert_eq!(
            got,
            vec![TimelineAction::Delete {
                key,
                ids: vec![f.a],
                ripple
            }]
        );
    }
}

#[test]
fn a_rubber_band_over_empty_track_picks_the_clips_it_touches() {
    let mut f = fixture();
    let mut m = Mouse::new();
    m.frame(&mut f.show, &mut f.st, vec![]);
    let r = m.rects().unwrap();
    // From the empty end of the top track (after C) down and back over C and A.
    let top = r.tracks[0].1;
    let from = pos2(r.x0 + 5.5 * r.pps, top.top() + 2.0);
    let to = pos2(r.x0 + 1.0 * r.pps, r.tracks[1].1.center().y);
    let frames = m.drag(&mut f.show, &mut f.st, from, to);
    let got = timeline_actions(frames.last().unwrap());
    match got.as_slice() {
        [TimelineAction::SelectClips { ids, add: false }] => {
            let mut ids = ids.clone();
            ids.sort();
            let mut want = vec![f.a, f.c];
            want.sort();
            assert_eq!(ids, want);
        }
        other => panic!("wanted one SelectClips, got {other:?}"),
    }
    for a in frames.into_iter().flatten() {
        super::super::apply(a, &mut f.show, &mut m.e, &mut f.st);
    }
    assert_eq!(f.st.selection(&f.show), vec![f.a, f.c]);
}

#[test]
fn an_empty_spot_of_a_track_offers_new_clips_there_and_the_tracks() {
    let mut f = fixture();
    let key = TimelineKey::Group(f.group);
    let mut m = Mouse::new();
    m.frame(&mut f.show, &mut f.st, vec![]);
    let r = m.rects().unwrap();
    let (track, row) = r.tracks[0];
    let spot = pos2(r.x0 + 7.0 * r.pps, row.center().y);
    m.click(&mut f.show, &mut f.st, spot, PointerButton::Secondary);
    m.frame(&mut f.show, &mut f.st, vec![]);
    let at = m.button_at("Text").center();
    let got = timeline_actions(&m.click(&mut f.show, &mut f.st, at, PointerButton::Primary));
    match got.as_slice() {
        [
            TimelineAction::AddClip {
                key: k,
                ty: CueType::Text,
                file: None,
                track: t,
                at,
            },
        ] => {
            assert_eq!((*k, *t), (key, track));
            assert!((at - 7.0).abs() < 0.1, "{at}");
        }
        other => panic!("wanted one AddClip, got {other:?}"),
    }
    // Made: a text on that track at that time, picked.
    for a in got {
        super::super::apply(Action::Timeline(a), &mut f.show, &mut m.e, &mut f.st);
    }
    let new = f.st.selected.unwrap();
    assert_eq!(model::key_of(&f.show, new), Some(key));
    assert_eq!(model::track_of(f.show.cue(new).unwrap()), track);
    // The top track can't go up.
    m.frame(&mut f.show, &mut f.st, vec![]);
    let spot = pos2(r.x0 + 1.0 * r.pps, row.center().y);
    m.click(&mut f.show, &mut f.st, spot, PointerButton::Secondary);
    m.frame(&mut f.show, &mut f.st, vec![]);
    let up = m.button_at("track_up");
    let got =
        timeline_actions(&m.click(&mut f.show, &mut f.st, up.center(), PointerButton::Primary));
    assert!(got.is_empty(), "the top track can't go up: {got:?}");
}

#[test]
fn tracks_are_added_removed_from_their_header_and_moved() {
    let mut f = fixture();
    let key = TimelineKey::Group(f.group);
    let mut m = Mouse::new();
    super::super::apply(
        Action::Timeline(TimelineAction::AddTrack { key, picture: true }),
        &mut f.show,
        &mut m.e,
        &mut f.st,
    );
    m.frame(&mut f.show, &mut f.st, vec![]);
    let r = m.rects().unwrap();
    assert_eq!(r.tracks.len(), 4);
    let (empty, row) = r.tracks[0];
    let head = pos2(row.left() - 40.0, row.center().y);
    m.click(&mut f.show, &mut f.st, head, PointerButton::Secondary);
    m.frame(&mut f.show, &mut f.st, vec![]);
    let at = m.button_at("remove_track").center();
    let got = timeline_actions(&m.click(&mut f.show, &mut f.st, at, PointerButton::Primary));
    assert_eq!(got, vec![TimelineAction::RemoveTrack { key, track: empty }]);
    // A track with clips can't go.
    let (busy, row) = r.tracks[1];
    let head = pos2(row.left() - 40.0, row.center().y);
    m.click(&mut f.show, &mut f.st, head, PointerButton::Secondary);
    m.frame(&mut f.show, &mut f.st, vec![]);
    let at = m.button_at("remove_track").center();
    let got = timeline_actions(&m.click(&mut f.show, &mut f.st, at, PointerButton::Primary));
    assert!(got.is_empty(), "{busy:?}: {got:?}");
    // Layer 0's track moves above layer 1's: their clips swap layers.
    let output = f.show.outputs.first().map(|o| o.id);
    super::super::apply(
        Action::Timeline(TimelineAction::MoveTrack {
            key,
            track: TrackId::Picture { output, layer: 0 },
            up: true,
        }),
        &mut f.show,
        &mut m.e,
        &mut f.st,
    );
    let layer = |id| f.show.cue(id).unwrap().kind.media().unwrap().layer;
    assert_eq!((layer(f.a), layer(f.c)), (1, 0));
}

#[test]
fn the_cue_tracks_plus_adds_a_pause_at_the_playhead() {
    let mut f = fixture();
    let key = TimelineKey::Group(f.group);
    let mut m = Mouse::new();
    m.e.timeline_seek(&f.show, key, 3.0, false);
    m.frame(&mut f.show, &mut f.st, vec![]);
    let plus = m.button_at("cue_plus").center();
    m.click(&mut f.show, &mut f.st, plus, PointerButton::Primary);
    m.frame(&mut f.show, &mut f.st, vec![]);
    let at = m.button_at("add_pause").center();
    let got = timeline_actions(&m.click(&mut f.show, &mut f.st, at, PointerButton::Primary));
    assert_eq!(
        got,
        vec![TimelineAction::AddControl {
            key,
            action: ControlAction::TimelinePause,
            at: 3.0
        }]
    );
}

#[test]
fn a_fade_handle_dragged_sets_the_fade_and_one_undo_takes_it_back() {
    let mut f = fixture();
    let mut m = in_window(&mut f);
    // The handles show on the clip under the pointer (or a picked one) only.
    assert!(m.rects().unwrap().fades.iter().all(|g| g.0 != f.b));
    let b = f.b;
    hover(&mut m, &mut f, b);
    m.frame(&mut f.show, &mut f.st, vec![]);
    let r = m.rects().unwrap();
    let grip = r
        .fades
        .iter()
        .find(|g| g.0 == f.b && g.1 == Edge::Left)
        .expect("B's fade-in handle")
        .2;
    m.drag(
        &mut f.show,
        &mut f.st,
        grip.center(),
        grip.center() + vec2(1.5 * r.pps, 0.0),
    );
    m.frame(&mut f.show, &mut f.st, vec![]);
    let fade = f.show.cue(f.b).unwrap().kind.media().unwrap().fade_in;
    assert!((fade - 1.5).abs() < 0.3, "{fade}");
    super::super::apply(Action::Undo, &mut f.show, &mut m.e, &mut f.st);
    assert_eq!(f.show.cue(f.b).unwrap().kind.media().unwrap().fade_in, 0.0);
}

#[test]
fn trimming_a_clip_of_the_selection_trims_the_others_as_far() {
    let mut f = fixture();
    let mut m = Mouse::new();
    f.st.select_many(vec![f.a, f.c], f.a);
    let a = m.clip(&mut f.show, &mut f.st, f.a);
    let pps = m.rects().unwrap().pps;
    let grip = pos2(a.right() - 1.0, a.center().y + 6.0);
    let frames = m.drag(&mut f.show, &mut f.st, grip, grip + vec2(0.5 * pps, 0.0));
    let got = timeline_actions(frames.last().unwrap());
    match got.as_slice() {
        [
            TimelineAction::Trim {
                id: x,
                edge: Edge::Right,
                to: ta,
            },
            TimelineAction::Trim {
                id: y,
                edge: Edge::Right,
                to: tc,
            },
        ] => {
            assert_eq!((*x, *y), (f.a, f.c));
            // A ended at 4, C at 5: both moved as far.
            assert!(((ta - 4.0) - (tc - 5.0)).abs() < 1e-9, "{ta} {tc}");
            assert!(*ta > 4.0);
        }
        other => panic!("wanted two Trims, got {other:?}"),
    }
}

// ----- The speed and opacity lanes -----

fn clip_len(e: &mut Engine, show: &Show, key: TimelineKey, id: CueId) -> Option<f64> {
    model::clips(show, key, &mut |c| e.cue_duration(show, c))
        .into_iter()
        .find(|c| c.id == id)
        .and_then(|c| c.len)
}

/// The fixture with A made a video of the 10 s test clip, its length known.
fn with_video() -> Option<(Fixture, Mouse)> {
    let (ff, file) = crate::media::test_media("testsrc_10s.mp4")?;
    let mut f = fixture();
    let i = f.show.cue_index(f.a).unwrap();
    let output = f.show.outputs.first().map(|o| o.id);
    f.show.cues[i].kind = CueKind::Video(crate::show::MediaCue::new(file, output));
    let mut m = Mouse::new();
    m.e.ff = Some(ff);
    let key = TimelineKey::Group(f.group);
    let t0 = std::time::Instant::now();
    while clip_len(&mut m.e, &f.show, key, f.a).is_none() {
        assert!(t0.elapsed().as_secs() < 30, "the clip's length never came");
        m.e.tick(&f.show, 0.0);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    Some((f, m))
}

fn lane_rect(m: &Mouse, id: CueId, lane: Lane) -> Rect {
    m.rects()
        .unwrap()
        .lanes
        .iter()
        .find(|l| l.0 == id && l.1 == lane)
        .expect("the lane is open")
        .2
}

fn lane_keys(m: &Mouse, id: CueId, lane: Lane) -> Vec<Pos2> {
    m.rects()
        .unwrap()
        .keys
        .iter()
        .filter(|k| k.0 == id && k.1 == lane)
        .map(|k| k.2)
        .collect()
}

/// Make what was asked for, and draw a frame.
fn made(m: &mut Mouse, f: &mut Fixture, actions: Vec<Action>) {
    for a in actions {
        super::super::apply(a, &mut f.show, &mut m.e, &mut f.st);
    }
    m.frame(&mut f.show, &mut f.st, vec![]);
}

#[test]
fn the_clip_menu_opens_a_clips_lanes_and_the_header_closes_them() {
    let mut f = fixture();
    let mut m = Mouse::new();
    let c = m.clip(&mut f.show, &mut f.st, f.c);
    m.click(&mut f.show, &mut f.st, c.center(), PointerButton::Secondary);
    m.frame(&mut f.show, &mut f.st, vec![]);
    // An image has an opacity lane and no speed lane.
    let buttons: Vec<(&'static str, Rect)> = m
        .ctx
        .data(|d| d.get_temp(egui::Id::new(("timeline_buttons", Host::Pane))))
        .unwrap_or_default();
    assert!(!buttons.iter().any(|b| b.0 == "speed_lane"));
    let item = m.button_at("opacity_lane").center();
    let got = m.click(&mut f.show, &mut f.st, item, PointerButton::Primary);
    assert_eq!(
        timeline_actions(&got),
        vec![TimelineAction::ShowLane {
            id: f.c,
            lane: Lane::Opacity,
            on: true
        }]
    );
    made(&mut m, &mut f, got);
    let lane = lane_rect(&m, f.c, Lane::Opacity);
    // Under C's track; the tracks below make room for it.
    let r = m.rects().unwrap();
    assert!((lane.top() - r.tracks[0].1.bottom()).abs() < 0.5);
    assert!(r.tracks[1].1.top() >= lane.bottom() - 0.5);
    let close = pos2(lane.left() - 11.0, lane.top() + 11.0);
    let got = m.click(&mut f.show, &mut f.st, close, PointerButton::Primary);
    assert_eq!(
        timeline_actions(&got),
        vec![TimelineAction::ShowLane {
            id: f.c,
            lane: Lane::Opacity,
            on: false
        }]
    );
    made(&mut m, &mut f, got);
    assert!(m.rects().unwrap().lanes.is_empty());
}

#[test]
fn the_speed_lane_adds_moves_and_removes_keys_and_the_clip_follows_the_speed() {
    let Some((mut f, mut m)) = with_video() else {
        return;
    };
    let key = TimelineKey::Group(f.group);
    let a = f.a;
    made(
        &mut m,
        &mut f,
        vec![Action::Timeline(TimelineAction::ShowLane {
            id: a,
            lane: Lane::Speed,
            on: true,
        })],
    );
    let full = m.clip(&mut f.show, &mut f.st, f.a).width();
    let lane = lane_rect(&m, f.a, Lane::Speed);
    let r = m.rects().unwrap();
    // A double-click at 5 s at normal speed (the middle): keys at 0 and 5.
    let at = pos2(r.x0 + 5.0 * r.pps, lane.center().y);
    let mut got = m.click(&mut f.show, &mut f.st, at, PointerButton::Primary);
    got.extend(m.click(&mut f.show, &mut f.st, at, PointerButton::Primary));
    let asked = timeline_actions(&got);
    match asked.as_slice() {
        [TimelineAction::SetRate { id, rate }] => {
            assert_eq!(*id, f.a);
            let ks: Vec<(f64, f32)> = rate.keys.iter().map(|k| (k.t, k.v)).collect();
            assert_eq!(ks.len(), 2, "{ks:?}");
            assert_eq!(ks[0], (0.0, 1.0));
            assert!((ks[1].0 - 5.0).abs() < 0.1 && ks[1].1 == 1.0, "{ks:?}");
        }
        other => panic!("wanted one SetRate, got {other:?}"),
    }
    made(&mut m, &mut f, got);
    assert!((m.clip(&mut f.show, &mut f.st, f.a).width() - full).abs() < 1.0);
    // Drag the key at 5 s up: faster from there, so the clip is shorter, and
    // already while it is dragged.
    let k = lane_keys(&m, f.a, Lane::Speed)[1];
    let up = pos2(k.x, lane.top() + 8.0);
    let mut frames = vec![
        m.frame(&mut f.show, &mut f.st, vec![Event::PointerMoved(k)]),
        m.frame(
            &mut f.show,
            &mut f.st,
            vec![button(k, true, PointerButton::Primary)],
        ),
    ];
    for i in 1..=6 {
        let p = k + (up - k) * (i as f32 / 6.0);
        frames.push(m.frame(&mut f.show, &mut f.st, vec![Event::PointerMoved(p)]));
    }
    let dragging = m
        .rects()
        .unwrap()
        .clips
        .iter()
        .find(|c| c.0 == f.a)
        .unwrap()
        .1;
    assert!(
        dragging.width() < full - 20.0,
        "{} vs {full}",
        dragging.width()
    );
    assert!(frames.iter().all(|f| timeline_actions(f).is_empty()));
    let last = m.frame(
        &mut f.show,
        &mut f.st,
        vec![button(up, false, PointerButton::Primary)],
    );
    made(&mut m, &mut f, last);
    let rate = f.show.cue(f.a).unwrap().kind.media().unwrap().rate.clone();
    assert!(rate.keys[1].v > 2.5, "{rate:?}");
    let short = m.clip(&mut f.show, &mut f.st, f.a).width();
    assert!(short < full - 20.0);
    assert!(clip_len(&mut m.e, &f.show, key, f.a).unwrap() < 9.0);
    // Right-click each key away: the last going leaves a steady speed.
    for _ in 0..2 {
        let k = lane_keys(&m, f.a, Lane::Speed)[0];
        let got = m.click(&mut f.show, &mut f.st, k, PointerButton::Secondary);
        made(&mut m, &mut f, got);
    }
    let rate = f.show.cue(f.a).unwrap().kind.media().unwrap().rate.clone();
    assert!(!rate.is_animated(), "{rate:?}");
    assert!(lane_keys(&m, f.a, Lane::Speed).is_empty());
}

#[test]
fn the_opacity_lane_moves_the_end_of_the_lead_in_with_the_rising_key() {
    let mut f = fixture();
    let key = TimelineKey::Group(f.group);
    let i = f.show.cue_index(f.c).unwrap();
    let k = |t, v| Keyframe {
        t,
        v,
        curve: Default::default(),
    };
    f.show.cues[i].kind.media_mut().unwrap().motion.opacity = Some(Animated {
        value: 1.0,
        keys: vec![k(0.0, 0.0), k(1.5, 0.0), k(2.5, 1.0)],
    });
    let mut m = Mouse::new();
    let c = f.c;
    made(
        &mut m,
        &mut f,
        vec![Action::Timeline(TimelineAction::ShowLane {
            id: c,
            lane: Lane::Opacity,
            on: true,
        })],
    );
    let pps = m.rects().unwrap().pps;
    let before = m.clip(&mut f.show, &mut f.st, f.c).width();
    assert!((before - 4.5 * pps).abs() < 0.5);
    // The key the rise starts from, 0.5 s later.
    let keys = lane_keys(&m, f.c, Lane::Opacity);
    assert_eq!(keys.len(), 3);
    let from = keys[1];
    let to = from + vec2(0.5 * pps, 0.0);
    let frames = m.drag(&mut f.show, &mut f.st, from, to);
    let asked = timeline_actions(frames.last().unwrap());
    match asked.as_slice() {
        [TimelineAction::SetOpacity { id, opacity }] => {
            assert_eq!(*id, f.c);
            assert!((opacity.keys[1].t - 2.0).abs() < 0.05, "{opacity:?}");
            assert_eq!(opacity.keys[1].v, 0.0);
        }
        other => panic!("wanted one SetOpacity, got {other:?}"),
    }
    made(&mut m, &mut f, frames.into_iter().flatten().collect());
    let c = f.show.cue(f.c).unwrap();
    match model::lead_in(&f.show, key, c) {
        model::LeadIn::At(v) => assert!((v - 2.0).abs() < 0.05, "{v}"),
        other => panic!("{other:?}"),
    }
    let after = m.clip(&mut f.show, &mut f.st, f.c).width();
    assert!((after - 5.0 * pps).abs() < 1.0, "{after} vs {}", 5.0 * pps);
}

// ----- A clip on several displays -----

#[test]
fn a_clip_also_on_another_display_has_a_ghost_there_that_picks_it_and_doesnt_drag() {
    let mut f = fixture();
    let d2 = f.show.add_display();
    let i = f.show.cue_index(f.a).unwrap();
    f.show.cues[i].kind.media_mut().unwrap().also_on = vec![d2];
    let mut m = Mouse::new();
    m.frame(&mut f.show, &mut f.st, vec![]);
    let r = m.rects().unwrap();
    let ghost_track = TrackId::Picture {
        output: Some(d2),
        layer: 0,
    };
    let row = r
        .tracks
        .iter()
        .find(|t| t.0 == ghost_track)
        .expect("its track")
        .1;
    let ghost = r.ghosts.iter().find(|g| g.0 == f.a).expect("the ghost").1;
    assert!(row.contains(ghost.center()));
    let home = r.clips.iter().find(|c| c.0 == f.a).unwrap().1;
    assert!((ghost.left() - home.left()).abs() < 0.5);
    assert!((ghost.width() - home.width()).abs() < 0.5);
    // A click picks the clip; a drag moves nothing.
    let got = m.click(
        &mut f.show,
        &mut f.st,
        ghost.center(),
        PointerButton::Primary,
    );
    assert!(
        got.iter()
            .any(|a| matches!(a, Action::Select(id) if *id == f.a))
    );
    let frames = m.drag(
        &mut f.show,
        &mut f.st,
        ghost.center(),
        ghost.center() + vec2(60.0, 0.0),
    );
    assert!(
        frames
            .iter()
            .flat_map(|f| timeline_actions(f))
            .all(|a| !matches!(a, TimelineAction::MoveClips { .. }))
    );
}

// ----- What comes from outside: dropped files, a chosen file -----

/// A file let go over the window.
#[derive(Debug)]
struct Dropped(std::path::PathBuf);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &std::path::Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
    }
}

#[test]
fn a_file_dropped_on_a_track_becomes_a_clip_there() {
    let mut f = fixture();
    let key = TimelineKey::Group(f.group);
    let mut m = Mouse::new();
    m.frame(&mut f.show, &mut f.st, vec![]);
    let r = m.rects().unwrap();
    let (track, row) = r.tracks[1];
    let at = pos2(r.x0 + 4.5 * r.pps, row.center().y);
    m.frame(&mut f.show, &mut f.st, vec![Event::PointerMoved(at)]);
    m.t += 1.0 / 60.0;
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 520.0))),
        time: Some(m.t),
        dropped_files: vec![
            std::sync::Arc::new(Dropped("still.png".into())),
            std::sync::Arc::new(Dropped("song.wav".into())),
        ],
        ..Default::default()
    };
    let mut actions = vec![Action::AddFiles(vec![
        "still.png".into(),
        "song.wav".into(),
    ])];
    let (show, st, e) = (&mut f.show, &mut f.st, &mut m.e);
    let mut out = m.ctx.run_ui(input, |ui| {
        egui::CentralPanel::default().show(ui, |ui| pane(ui, show, e, st, &mut actions));
    });
    out.textures_delta.clear();
    assert!(!actions.iter().any(|a| matches!(a, Action::AddFiles(_))));
    let got = timeline_actions(&actions);
    match got.as_slice() {
        [
            TimelineAction::AddClip {
                key: k1,
                ty: CueType::Image,
                track: t1,
                at: a1,
                ..
            },
            TimelineAction::AddClip {
                ty: CueType::Audio,
                track: TrackId::Sound { .. },
                ..
            },
        ] => {
            assert_eq!((*k1, *t1), (key, track));
            assert!((a1 - 4.5).abs() < 0.1);
        }
        other => panic!("wanted two AddClips, got {other:?}"),
    }
}

#[test]
fn a_file_chosen_in_the_dialog_comes_back_as_a_new_clip() {
    let mut f = fixture();
    let key = TimelineKey::Group(f.group);
    let (tx, rx) = std::sync::mpsc::channel();
    f.st.timeline.pick = Some(rx);
    let track = TrackId::Cues;
    tx.send((key, CueType::Video, track, 2.0, Some("clip.mp4".into())))
        .unwrap();
    let mut m = Mouse::new();
    let got = timeline_actions(&m.frame(&mut f.show, &mut f.st, vec![]));
    assert_eq!(
        got,
        vec![TimelineAction::AddClip {
            key,
            ty: CueType::Video,
            file: Some("clip.mp4".into()),
            track,
            at: 2.0
        }]
    );
    assert!(f.st.timeline.pick.is_none());
}

#[test]
fn show_mode_splits_nothing_from_the_editor() {
    let mut f = fixture();
    f.st.show_mode = true;
    let mut m = Mouse::new();
    let a = m.clip(&mut f.show, &mut f.st, f.a);
    m.frame(
        &mut f.show,
        &mut f.st,
        vec![Event::PointerMoved(a.center())],
    );
    m.mods = Modifiers::COMMAND;
    let got = m.frame(
        &mut f.show,
        &mut f.st,
        key_press(egui::Key::K, Modifiers::COMMAND),
    );
    m.mods = Modifiers::NONE;
    assert!(timeline_actions(&got).is_empty());
    assert!(
        TimelineAction::Split {
            key: TimelineKey::Group(f.group),
            ids: vec![],
            at: 0.0
        }
        .edits_the_show()
    );
}

#[test]
fn a_lane_key_near_the_ends_snaps_to_unseen_full_and_normal_speed() {
    // A pixel off the bottom would end a lead-in (anything above 0 is seen).
    assert_eq!(Lane::Opacity.value(0.02), 0.0);
    assert_eq!(Lane::Opacity.value(0.98), 1.0);
    assert_eq!(Lane::Opacity.value(0.5), 0.5);
    assert_eq!(Lane::Speed.value(0.5), 1.0);
    assert_eq!(Lane::Speed.value(0.51), 1.0);
    assert_eq!(Lane::Speed.value(1.0), 4.0);
    assert_eq!(Lane::Speed.value(0.0), 0.25);
}

// ----- Copy, cut, paste and duplicate (I2) -----

/// A frame with the pointer over clip `id` (so the editor holds the keys).
fn hover(m: &mut Mouse, f: &mut Fixture, id: CueId) {
    let r = m.clip(&mut f.show, &mut f.st, id);
    m.frame(
        &mut f.show,
        &mut f.st,
        vec![Event::PointerMoved(r.center())],
    );
}

fn layer_of(show: &Show, id: CueId) -> i32 {
    show.cue(id).unwrap().kind.media().unwrap().layer
}

#[test]
fn ctrl_c_and_v_paste_the_picked_clips_at_the_playhead_on_their_tracks_in_one_undo() {
    let mut f = fixture();
    let key = TimelineKey::Group(f.group);
    let mut m = in_window(&mut f);
    let a = f.a;
    pick(&mut m, &mut f, a);
    f.st.select_many(vec![f.a, f.c], f.c);
    let id = f.a;
    hover(&mut m, &mut f, id);
    m.frame(&mut f.show, &mut f.st, vec![Event::Copy]);
    assert_eq!(f.st.clipboard.len(), 2);
    super::super::apply(
        Action::Timeline(TimelineAction::Seek { key, pos: 12.0 }),
        &mut f.show,
        &mut m.e,
        &mut f.st,
    );
    let before: Vec<CueId> = model::members(&f.show, key).iter().map(|c| c.id).collect();
    let id = f.a;
    hover(&mut m, &mut f, id);
    m.frame(&mut f.show, &mut f.st, vec![Event::Paste(String::new())]);
    m.frame(&mut f.show, &mut f.st, vec![]);
    let new: Vec<CueId> = model::members(&f.show, key)
        .iter()
        .map(|c| c.id)
        .filter(|id| !before.contains(id))
        .collect();
    assert_eq!(new.len(), 2, "both pasted into the group");
    // A (0 s, layer 0) and C (2 s, layer 1): at 12 s and 14 s, on their layers.
    let mut got: Vec<(f64, i32)> = new
        .iter()
        .map(|id| (f.show.cue(*id).unwrap().pre_wait, layer_of(&f.show, *id)))
        .collect();
    got.sort_by(|x, y| x.0.total_cmp(&y.0));
    assert!((got[0].0 - 12.0).abs() < 1e-3 && (got[1].0 - 14.0).abs() < 1e-3);
    assert_eq!((got[0].1, got[1].1), (0, 1));
    assert_eq!(f.st.selection(&f.show).len(), 2, "the copies are picked");
    super::super::apply(Action::Undo, &mut f.show, &mut m.e, &mut f.st);
    assert_eq!(model::members(&f.show, key).len(), before.len(), "one undo");
}

#[test]
fn ctrl_d_duplicates_a_clip_just_after_it_and_ctrl_x_cuts_it_to_paste_back() {
    let mut f = fixture();
    let key = TimelineKey::Group(f.group);
    let mut m = in_window(&mut f);
    let a = f.a;
    pick(&mut m, &mut f, a);
    let id = f.a;
    hover(&mut m, &mut f, id);
    let n = model::members(&f.show, key).len();
    m.mods = Modifiers::COMMAND;
    m.frame(
        &mut f.show,
        &mut f.st,
        key_press(egui::Key::D, Modifiers::COMMAND),
    );
    m.mods = Modifiers::NONE;
    m.frame(&mut f.show, &mut f.st, vec![]);
    assert_eq!(model::members(&f.show, key).len(), n + 1);
    let copy = f.st.selected.unwrap();
    assert_ne!(copy, f.a);
    // A is 0–4 s: its copy starts at 4 s, on its layer, in the group.
    assert_eq!(starts(&f.show, &[copy]), vec![4.0]);
    assert_eq!(model::key_of(&f.show, copy), Some(key));
    super::super::apply(Action::Undo, &mut f.show, &mut m.e, &mut f.st);
    assert_eq!(model::members(&f.show, key).len(), n);
    // Cut: A goes; pasted back at the playhead (3 s) it has its id again.
    let a = f.a;
    pick(&mut m, &mut f, a);
    let id = f.a;
    hover(&mut m, &mut f, id);
    m.frame(&mut f.show, &mut f.st, vec![Event::Cut]);
    m.frame(&mut f.show, &mut f.st, vec![]);
    assert!(f.show.cue(f.a).is_none(), "A is cut");
    assert_eq!(starts(&f.show, &[f.b]), vec![6.0], "no ripple");
    super::super::apply(
        Action::Timeline(TimelineAction::Seek { key, pos: 3.0 }),
        &mut f.show,
        &mut m.e,
        &mut f.st,
    );
    let id = f.b;
    hover(&mut m, &mut f, id);
    m.frame(&mut f.show, &mut f.st, vec![Event::Paste(String::new())]);
    m.frame(&mut f.show, &mut f.st, vec![]);
    assert_eq!(starts(&f.show, &[f.a]), vec![3.0], "back with its id");
}

#[test]
fn a_paste_with_a_cue_outside_picked_or_in_show_mode_is_not_the_editors() {
    let mut f = fixture();
    let key = TimelineKey::Group(f.group);
    let other = f.show.make_cue(CueType::Wait, None, f.show.cues.len());
    let other_id = other.id;
    f.show.cues.push(other);
    let mut m = in_window(&mut f);
    f.st.clipboard = f.show.copy_cues(&[f.a]);
    f.st.clipboard_epoch = f.st.show_epoch;
    // A cue outside the timeline picked: the cue list's paste, after it.
    f.st.selected = Some(other_id);
    let id = f.a;
    hover(&mut m, &mut f, id);
    let n = model::members(&f.show, key).len();
    m.frame(&mut f.show, &mut f.st, vec![Event::Paste(String::new())]);
    assert_eq!(model::members(&f.show, key).len(), n, "not into the group");
    // Show mode: nothing is pasted at all.
    f.st.selected = Some(f.a);
    f.st.show_mode = true;
    let cues = f.show.cues.len();
    let id = f.a;
    hover(&mut m, &mut f, id);
    m.frame(&mut f.show, &mut f.st, vec![Event::Paste(String::new())]);
    assert_eq!(f.show.cues.len(), cues);
    assert!(TimelineAction::Paste { key, at: 0.0 }.edits_the_show());
    assert!(
        TimelineAction::Duplicate {
            key,
            ids: vec![f.a]
        }
        .edits_the_show()
    );
}

#[test]
fn a_lane_opened_under_a_track_out_of_view_is_scrolled_into_view() {
    let mut f = fixture();
    let key = TimelineKey::Group(f.group);
    let mut m = Mouse::new();
    // Many empty picture tracks above A's: A's track is at the bottom.
    for _ in 0..14 {
        made(
            &mut m,
            &mut f,
            vec![Action::Timeline(TimelineAction::AddTrack {
                key,
                picture: true,
            })],
        );
    }
    let r = m.rects().unwrap();
    let a_row = r
        .tracks
        .iter()
        .find(|t| t.0 == model::track_of(f.show.cue(f.a).unwrap()))
        .unwrap()
        .1;
    let bottom = r.tracks.iter().map(|t| t.1.bottom()).fold(0.0, f32::max);
    assert!(bottom > 520.0, "the tracks overflow the pane: {bottom}");
    let lane = Action::Timeline(TimelineAction::ShowLane {
        id: f.a,
        lane: Lane::Opacity,
        on: true,
    });
    made(&mut m, &mut f, vec![lane]);
    m.frame(&mut f.show, &mut f.st, vec![]);
    let lane = lane_rect(&m, f.a, Lane::Opacity);
    assert!(lane.bottom() <= 520.0, "the lane is in view: {lane:?}");
    let r = m.rects().unwrap();
    let now = r
        .tracks
        .iter()
        .find(|t| t.0 == model::track_of(f.show.cue(f.a).unwrap()))
        .unwrap()
        .1;
    assert!(now.top() >= r.ruler.bottom() - 0.5, "and its clip's track");
    assert!(now.top() < a_row.top(), "scrolled");
}

#[test]
fn on_a_narrow_pane_the_toolbar_takes_two_rows_and_no_button_is_cut_off() {
    let mut f = fixture();
    let mut m = Mouse::new();
    m.frame(&mut f.show, &mut f.st, vec![]);
    let names = [
        "run",
        "pause",
        "stop",
        "marker",
        "split",
        "ripple",
        "add_track",
    ];
    let at: Vec<Rect> = names.iter().map(|n| m.button_at(n)).collect();
    for (n, r) in names.iter().zip(&at) {
        assert!(r.right() <= 1000.0, "{n} is cut off: {r:?}");
    }
    // The tools are on a row of their own, under the transport.
    assert!(
        at[3].top() > at[0].bottom() - 1.0,
        "{:?} {:?}",
        at[3],
        at[0]
    );
    let r = m.rects().unwrap();
    assert!(r.ruler.top() >= at[3].bottom());
}

mod cue_row;
mod follow;
mod space;
mod workspace;
