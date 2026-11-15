//! The timeline list's tab, driven through the whole window (`ui::draw`, both
//! interfaces) with made-up mouse input, and its GO, read-outs, clips and refusals.

use super::*;
use crate::show::{ControlAction, ControlCue, CueKind, OutputId, WaitCue};
use egui::{DroppedFileHandle, Event, PointerButton, Pos2, RawInput, Rect, pos2, vec2};

/// A file dropped on the window, as the windowing layer hands it over.
#[derive(Debug)]
struct Dropped(std::path::PathBuf);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

/// The whole window, frame by frame; actions are made as the app makes them.
struct Win {
    ctx: egui::Context,
    e: Engine,
    t: f64,
    size: egui::Vec2,
}

impl Win {
    fn new() -> Win {
        Win {
            ctx: egui::Context::default(),
            e: Engine::for_tests(),
            t: 1.0,
            size: vec2(1300.0, 900.0),
        }
    }

    fn frame_with(
        &mut self,
        show: &mut Show,
        st: &mut UiState,
        events: Vec<Event>,
        dropped: Vec<DroppedFileHandle>,
    ) {
        self.t += 1.0 / 60.0;
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
            time: Some(self.t),
            events,
            dropped_files: dropped,
            ..Default::default()
        };
        let e = &mut self.e;
        self.ctx.data_mut(|d| {
            d.remove::<Vec<(CueId, Rect)>>(egui::Id::new("cue_rows"));
            d.remove::<Vec<(OutputId, Rect)>>(egui::Id::new("also_on_boxes"));
            d.remove::<Vec<(&'static str, Rect)>>(egui::Id::new("list_tab_buttons"));
            d.remove::<Rect>(egui::Id::new("list_tab_convert"));
            d.remove::<bool>(egui::Id::new("timeline_in_tab"));
            d.remove::<TimelineRects>(egui::Id::new(("timeline_rects", Host::ListTab)));
            d.remove::<TimelineRects>(egui::Id::new(("timeline_rects", Host::Pane)));
        });
        let mut out = self
            .ctx
            .run_ui(input, |ui| super::super::super::draw(ui, show, e, st));
        out.textures_delta.clear();
    }

    fn frame(&mut self, show: &mut Show, st: &mut UiState) {
        self.frame_with(show, st, vec![], vec![]);
    }

    fn click_with(&mut self, show: &mut Show, st: &mut UiState, at: Pos2, b: PointerButton) {
        let press = |pressed| Event::PointerButton {
            pos: at,
            button: b,
            pressed,
            modifiers: Default::default(),
        };
        self.frame_with(show, st, vec![Event::PointerMoved(at)], vec![]);
        self.frame_with(show, st, vec![press(true)], vec![]);
        self.frame_with(show, st, vec![press(false)], vec![]);
        self.frame(show, st);
    }

    fn click(&mut self, show: &mut Show, st: &mut UiState, at: Pos2) {
        self.click_with(show, st, at, PointerButton::Primary);
    }

    fn get<T: Clone + Send + Sync + 'static>(&self, name: &str) -> Option<T> {
        self.ctx.data(|d| d.get_temp::<T>(egui::Id::new(name)))
    }

    fn rects(&self, host: Host) -> Option<TimelineRects> {
        self.ctx
            .data(|d| d.get_temp::<TimelineRects>(egui::Id::new(("timeline_rects", host))))
    }

    fn tab_button(&self, name: &str) -> Option<Rect> {
        self.get::<Vec<(&'static str, Rect)>>("list_tab_buttons")
            .unwrap_or_default()
            .into_iter()
            .find(|b| b.0 == name)
            .map(|b| b.1)
    }

    fn list_tab(&self, list: u32) -> Rect {
        self.get::<Vec<(u32, Rect)>>("list_tabs")
            .unwrap_or_default()
            .into_iter()
            .find(|t| t.0 == list)
            .expect("the list's tab is drawn")
            .1
    }

    fn said(&self, what: &str) -> bool {
        self.e.messages.iter().any(|(_, m)| m.contains(what))
    }
}

fn wait(show: &mut Show, name: &str, list: u32, at: f64, secs: f64) -> CueId {
    let mut c = show.make_cue(CueType::Wait, None, show.cues.len());
    c.name = name.into();
    c.list = list;
    c.pre_wait = at;
    c.kind = CueKind::Wait(WaitCue { duration: secs });
    let id = c.id;
    show.cues.push(c);
    id
}

/// A main list with one cue, and "Timeline 1": a 3 s wait "Bed" from 0, a
/// pause cue at 1 s and a 1 s wait "Late" at 2 s.
struct Fixture {
    show: Show,
    st: UiState,
    list: u32,
    main: CueId,
    bed: CueId,
}

fn fixture() -> Fixture {
    let mut show = Show::default();
    let main = wait(&mut show, "Main cue", 0, 0.0, 5.0);
    let list = edit::new_timeline_list(&mut show, "Timeline 1");
    let bed = wait(&mut show, "Bed", list, 0.0, 3.0);
    let mut p = show.make_cue(CueType::Control, None, show.cues.len());
    p.name = "Hold".into();
    p.list = list;
    p.pre_wait = 1.0;
    p.kind = CueKind::Control(ControlCue {
        action: ControlAction::TimelinePause,
        ..Default::default()
    });
    show.cues.push(p);
    wait(&mut show, "Late", list, 2.0, 1.0);
    Fixture {
        show,
        st: UiState::default(),
        list,
        main,
        bed,
    }
}

fn act(a: Action, f: &mut Fixture, w: &mut Win) {
    super::super::super::apply(a, &mut f.show, &mut w.e, &mut f.st);
}

fn readout(w: &Win) -> Option<(String, Option<String>)> {
    w.get::<(String, Option<String>)>("transport_readout")
}

#[test]
fn the_plus_menu_makes_a_timeline_list_whose_tab_shows_its_tracks() {
    let mut show = Show::default();
    wait(&mut show, "Main cue", 0, 0.0, 5.0);
    show.add_list("List 2");
    let mut st = UiState::default();
    let mut w = Win::new();
    w.frame(&mut show, &mut st);
    let plus = w.get::<Rect>("list_tab_plus").expect("the + is drawn");
    w.click(&mut show, &mut st, plus.center());
    let item = w
        .get::<Rect>("list_tab_new_timeline")
        .expect("the + menu offers a timeline");
    w.click(&mut show, &mut st, item.center());
    let made = show.lists.last().unwrap().id;
    assert!(is_timeline_list(&show, made), "{:?}", show.lists);
    assert_eq!(w.e.active_list, made);
    w.frame(&mut show, &mut st);
    assert!(w.rects(Host::ListTab).is_some(), "the tab draws the tracks");
    assert!(
        w.get::<Vec<(CueId, Rect)>>("cue_rows").is_none(),
        "and no rows"
    );
    // The rows are a click away, and back.
    let rows = w.tab_button("rows").unwrap();
    w.click(&mut show, &mut st, rows.center());
    assert!(st.timeline.list_rows);
    w.frame(&mut show, &mut st);
    assert!(w.rects(Host::ListTab).is_none());
    assert!(w.get::<Vec<(CueId, Rect)>>("cue_rows").is_some());
    let tracks = w.tab_button("tracks").unwrap();
    w.click(&mut show, &mut st, tracks.center());
    w.frame(&mut show, &mut st);
    assert!(w.rects(Host::ListTab).is_some());
    // The main list's tab shows rows, as always.
    let main = w.list_tab(0);
    w.click(&mut show, &mut st, main.center());
    assert_eq!(w.e.active_list, 0);
    w.frame(&mut show, &mut st);
    assert!(w.rects(Host::ListTab).is_none());
    assert!(w.get::<Vec<(CueId, Rect)>>("cue_rows").is_some());
    // One undo takes the new timeline away again, and the main list is worked.
    let mut f = Fixture {
        show,
        st,
        list: made,
        main: CueId(0),
        bed: CueId(0),
    };
    act(Action::SelectList(made), &mut f, &mut w);
    act(Action::Undo, &mut f, &mut w);
    w.frame(&mut f.show, &mut f.st);
    assert!(!is_timeline_list(&f.show, made) || !f.show.has_list(made));
}

#[test]
fn the_new_interface_shows_the_tab_too_and_the_pane_does_not_draw_it_twice() {
    let mut f = fixture();
    let mut w = Win::new();
    f.st.prefs.new_layout = true;
    act(Action::SelectList(f.list), &mut f, &mut w);
    act(Action::Timeline(TimelineAction::Open(None)), &mut f, &mut w);
    w.frame(&mut f.show, &mut f.st);
    assert!(w.rects(Host::ListTab).is_some());
    assert!(w.rects(Host::Pane).is_none());
    assert_eq!(w.get::<bool>("timeline_in_tab"), Some(true));
    // With the rows in the tab, the timeline panel has the tracks.
    f.st.timeline.list_rows = true;
    w.frame(&mut f.show, &mut f.st);
    assert!(w.rects(Host::ListTab).is_none());
    assert!(w.rects(Host::Pane).is_some());
    assert!(w.get::<Vec<(CueId, Rect)>>("cue_rows").is_some());
}

#[test]
fn go_on_a_timeline_list_runs_it_and_the_read_outs_say_what_go_does() {
    let mut f = fixture();
    let mut w = Win::new();
    let key = TimelineKey::List(f.list);
    act(Action::SelectList(f.list), &mut f, &mut w);
    f.st.selected = Some(f.bed);
    w.frame(&mut f.show, &mut f.st);
    assert_eq!(
        readout(&w),
        Some(("Run Timeline 1".to_string(), None)),
        "stopped: GO runs it"
    );

    act(Action::Go, &mut f, &mut w);
    assert_eq!(w.e.timeline(&f.show, key).state, RunState::Running);
    assert_eq!(f.st.selected, Some(f.bed), "GO keeps the selection");
    assert!(
        !matches!(w.e.status(f.main), crate::engine::CueStatus::Playing { .. }),
        "the main list's cue is not fired"
    );
    w.frame(&mut f.show, &mut f.st);
    let (what, then) = readout(&w).unwrap();
    assert_eq!(what, "RUNNING · next pause in 0:01.0");
    assert_eq!(then.as_deref(), Some("GO does nothing until it pauses"));

    // GO while it runs does nothing.
    w.e.tick(&f.show, 0.4);
    let pos = w.e.timeline(&f.show, key).pos;
    act(Action::Go, &mut f, &mut w);
    assert_eq!(w.e.timeline(&f.show, key).state, RunState::Running);
    assert_eq!(w.e.timeline(&f.show, key).pos, pos);

    // At the pause cue GO carries it on.
    for _ in 0..4 {
        w.e.tick(&f.show, 0.3);
    }
    let v = w.e.timeline(&f.show, key);
    assert!(matches!(v.state, RunState::Paused { .. }), "{:?}", v.state);
    w.frame(&mut f.show, &mut f.st);
    let (what, _) = readout(&w).unwrap();
    assert_eq!(what, "Continue Timeline 1");
    // The new interface's header says the same.
    f.st.prefs.new_layout = true;
    w.frame(&mut f.show, &mut f.st);
    let hover = w.get::<String>("strip_readout").unwrap();
    assert!(hover.starts_with("GO: Continue Timeline 1"), "{hover}");
    f.st.prefs.new_layout = false;

    act(Action::Go, &mut f, &mut w);
    assert_eq!(w.e.timeline(&f.show, key).state, RunState::Running);
    for _ in 0..20 {
        w.e.tick(&f.show, 0.3);
    }
    let v = w.e.timeline(&f.show, key);
    assert_eq!(v.state, RunState::Stopped);
    assert!(v.pos >= 2.9, "it ran to its end: {}", v.pos);
    w.frame(&mut f.show, &mut f.st);
    let (what, _) = readout(&w).unwrap();
    assert_eq!(what, "Timeline 1 is at its end");
    // GO at the end does nothing; the playhead stays at the end.
    act(Action::Go, &mut f, &mut w);
    w.e.tick(&f.show, 0.1);
    let after = w.e.timeline(&f.show, key);
    assert_eq!(after.state, RunState::Stopped);
    assert_eq!(after.pos, v.pos);
    assert!(w.said("at its end"));
}

#[test]
fn the_operator_view_shows_the_timeline_run_and_no_cue_stack() {
    let mut f = fixture();
    let mut w = Win::new();
    f.st.prefs.new_layout = true;
    f.st.modern_view = crate::ui::modern::View::Operator;
    act(Action::SelectList(f.list), &mut f, &mut w);
    w.frame(&mut f.show, &mut f.st);
    assert_eq!(
        w.get::<String>("operator_timeline").as_deref(),
        Some("Stopped")
    );
    act(Action::Go, &mut f, &mut w);
    w.frame(&mut f.show, &mut f.st);
    let state = w.get::<String>("operator_timeline").unwrap();
    assert!(state.starts_with("Running · next pause in"), "{state}");
}

#[test]
fn stop_all_stops_the_timeline_where_it_is() {
    let mut f = fixture();
    let mut w = Win::new();
    let key = TimelineKey::List(f.list);
    act(Action::SelectList(f.list), &mut f, &mut w);
    act(Action::Go, &mut f, &mut w);
    w.e.tick(&f.show, 0.5);
    act(Action::Panic, &mut f, &mut w);
    for _ in 0..10 {
        w.e.tick(&f.show, 0.5);
    }
    let v = w.e.timeline(&f.show, key);
    assert_eq!(v.state, RunState::Stopped);
    assert!((v.pos - 0.5).abs() < 0.05, "{}", v.pos);
}

#[test]
fn a_list_becomes_a_timeline_and_back_from_its_tab_but_not_one_with_groups() {
    let mut f = fixture();
    let mut w = Win::new();
    let plain = f.show.add_list("Plain");
    wait(&mut f.show, "A", plain, 0.0, 1.0);
    let grouped = f.show.add_list("Grouped");
    let mut g = f.show.make_cue(CueType::Group, None, f.show.cues.len());
    g.list = grouped;
    f.show.cues.push(g);
    w.frame(&mut f.show, &mut f.st);

    // The main list's menu has no such thing.
    let main = w.list_tab(0);
    w.click_with(
        &mut f.show,
        &mut f.st,
        main.center(),
        PointerButton::Secondary,
    );
    assert!(w.get::<Rect>("list_tab_convert").is_none());
    w.click(&mut f.show, &mut f.st, pos2(1250.0, 850.0));

    let convert = |w: &mut Win, f: &mut Fixture, list: u32| {
        let tab = w.list_tab(list);
        w.click_with(
            &mut f.show,
            &mut f.st,
            tab.center(),
            PointerButton::Secondary,
        );
        let item = w.get::<Rect>("list_tab_convert").expect("offered");
        w.click(&mut f.show, &mut f.st, item.center());
    };
    convert(&mut w, &mut f, plain);
    assert!(is_timeline_list(&f.show, plain));
    convert(&mut w, &mut f, plain);
    assert!(!is_timeline_list(&f.show, plain));
    convert(&mut w, &mut f, grouped);
    assert!(!is_timeline_list(&f.show, grouped));
    assert!(w.said("can't hold groups"));
    // The engine's word for it, too.
    act(
        Action::Timeline(TimelineAction::SetListTimeline { list: 0, on: true }),
        &mut f,
        &mut w,
    );
    assert!(!model::is_timeline(&f.show, TimelineKey::List(0)));
}

#[test]
fn a_timeline_list_takes_no_groups() {
    let mut f = fixture();
    let mut w = Win::new();
    let mut g = f.show.make_cue(CueType::Group, None, 1);
    g.list = 0;
    let gid = g.id;
    f.show.cues.insert(1, g);
    let n = f.show.cues.len();
    act(Action::MoveToList(gid, f.list), &mut f, &mut w);
    assert_eq!(f.show.cue(gid).unwrap().list, 0, "not moved in");
    assert!(w.said("can't hold groups"));
    act(Action::SelectList(f.list), &mut f, &mut w);
    act(Action::AddCue(CueType::Group), &mut f, &mut w);
    act(
        Action::Timeline(TimelineAction::NewTimelineGroup),
        &mut f,
        &mut w,
    );
    act(Action::GroupCue(f.bed), &mut f, &mut w);
    assert_eq!(f.show.cues.len(), n, "nothing added");
    assert!(f.show.cue(f.bed).unwrap().parent.is_none());
    // A plain cue is moved in, and is a clip there.
    act(Action::MoveToList(f.main, f.list), &mut f, &mut w);
    assert_eq!(f.show.cue(f.main).unwrap().list, f.list);
}

#[test]
fn files_dropped_on_a_timeline_tab_become_clips_at_its_playhead() {
    let file = std::path::Path::new("test_media/color_red.png");
    if !file.exists() {
        assert!(
            std::env::var_os("NEWQ_REQUIRE_MEDIA").is_none(),
            "test_media/color_red.png is missing"
        );
        return;
    }
    let mut f = fixture();
    let mut w = Win::new();
    let key = TimelineKey::List(f.list);
    act(Action::SelectList(f.list), &mut f, &mut w);
    act(
        Action::Timeline(TimelineAction::Seek { key, pos: 0.5 }),
        &mut f,
        &mut w,
    );
    let pos = w.e.timeline(&f.show, key).pos;
    assert!((pos - 0.5).abs() < 1e-6, "{pos}");
    let before: Vec<CueId> = f.show.cues.iter().map(|c| c.id).collect();
    let drop = || -> DroppedFileHandle { std::sync::Arc::new(Dropped(file.to_path_buf())) };
    w.frame_with(&mut f.show, &mut f.st, vec![], vec![drop(), drop()]);
    let new: Vec<&crate::show::Cue> = f
        .show
        .cues
        .iter()
        .filter(|c| !before.contains(&c.id))
        .collect();
    assert_eq!(new.len(), 2);
    let (a, b) = (new[0], new[1]);
    let (a, b) = if a.pre_wait <= b.pre_wait {
        (a, b)
    } else {
        (b, a)
    };
    assert!(a.list == f.list && b.list == f.list && a.parent.is_none());
    assert!((a.pre_wait - 0.5).abs() < 1e-6, "{}", a.pre_wait);
    assert!(b.pre_wait > a.pre_wait + 1.0, "after it: {}", b.pre_wait);
    assert!(matches!(model::track_of(a), TrackId::Picture { .. }));
    assert_eq!(model::track_of(a), model::track_of(b));
    assert_eq!(f.st.selected, Some(b.id));
    // They are clips the tab draws.
    let (a, b) = (a.id, b.id);
    w.frame(&mut f.show, &mut f.st);
    let r = w.rects(Host::ListTab).unwrap();
    assert!(r.clips.iter().any(|c| c.0 == a) && r.clips.iter().any(|c| c.0 == b));
}

#[test]
fn a_picture_goes_on_a_free_track_and_over_the_others_when_all_are_taken() {
    let mut show = Show::default();
    let list = edit::new_timeline_list(&mut show, "T");
    let key = TimelineKey::List(list);
    let out = show.outputs[0].id;
    let none: &mut dyn FnMut(&crate::show::Cue) -> Option<Option<f64>> =
        &mut |c| Some(c.kind.media().and_then(|m| m.end));
    // Nothing yet: the display's layer 0.
    let clips = model::clips(&show, key, none);
    assert_eq!(
        fitting_track(&show, key, CueType::Image, 1.0, &clips),
        TrackId::Picture {
            output: Some(out),
            layer: 0
        }
    );
    let pic = edit::add_clip(
        &mut show,
        key,
        CueType::Image,
        None,
        TrackId::Picture {
            output: Some(out),
            layer: 0,
        },
        0.0,
    )
    .unwrap();
    let i = show.cue_index(pic).unwrap();
    show.cues[i].kind.media_mut().unwrap().end = Some(4.0);
    let clips = model::clips(&show, key, none);
    // Under it: a layer above. After it: the same track.
    assert_eq!(
        fitting_track(&show, key, CueType::Video, 2.0, &clips),
        TrackId::Picture {
            output: Some(out),
            layer: 1
        }
    );
    assert_eq!(
        fitting_track(&show, key, CueType::Image, 4.0, &clips),
        TrackId::Picture {
            output: Some(out),
            layer: 0
        }
    );
    assert_eq!(
        fitting_track(&show, key, CueType::Audio, 2.0, &clips),
        TrackId::Sound { output: None }
    );
    assert_eq!(
        fitting_track(&show, key, CueType::Fade, 2.0, &clips),
        TrackId::Cues
    );
}

#[test]
fn show_mode_locks_the_edits_but_not_the_active_timelines_ruler() {
    let mut f = fixture();
    let mut w = Win::new();
    let key = TimelineKey::List(f.list);
    act(Action::SelectList(f.list), &mut f, &mut w);
    act(Action::SetShowMode(true), &mut f, &mut w);
    let lists = f.show.lists.len();
    act(
        Action::Timeline(TimelineAction::NewTimelineList),
        &mut f,
        &mut w,
    );
    act(
        Action::Timeline(TimelineAction::SetListTimeline {
            list: f.list,
            on: false,
        }),
        &mut f,
        &mut w,
    );
    assert_eq!(f.show.lists.len(), lists);
    assert!(is_timeline_list(&f.show, f.list));
    // The + is greyed: its menu doesn't open.
    w.frame(&mut f.show, &mut f.st);
    let plus = w.get::<Rect>("list_tab_plus").unwrap();
    w.ctx
        .data_mut(|d| d.remove::<Rect>(egui::Id::new("list_tab_new_timeline")));
    w.click(&mut f.show, &mut f.st, plus.center());
    assert!(w.get::<Rect>("list_tab_new_timeline").is_none());
    // The active list's ruler still parks it (that is what an operator does).
    w.frame(&mut f.show, &mut f.st);
    let r = w.rects(Host::ListTab).unwrap();
    let at = pos2(r.x0 + 2.5 * r.pps, r.ruler.center().y);
    w.click(&mut f.show, &mut f.st, at);
    let v = w.e.timeline(&f.show, key);
    assert!((v.pos - 2.5).abs() < 0.1, "parked at {}", v.pos);
}

#[test]
fn the_own_layers_offer_is_shown_and_only_taken_when_clicked() {
    let mut f = fixture();
    f.show.own_layers = false;
    let mut w = Win::new();
    act(Action::SelectList(f.list), &mut f, &mut w);
    w.frame(&mut f.show, &mut f.st);
    w.frame(&mut f.show, &mut f.st);
    assert!(!f.show.own_layers, "never turned on unasked");
    let b = w.tab_button("own_layers").expect("offered");
    w.click(&mut f.show, &mut f.st, b.center());
    assert!(f.show.own_layers);
    w.frame(&mut f.show, &mut f.st);
    assert!(w.tab_button("own_layers").is_none());
}

/// A show with three displays and a picture on the first, selected, drawn in
/// the new interface (its inspector below), tall enough for the whole of it.
fn three_displays() -> (Show, UiState, Win, CueId, [OutputId; 3]) {
    let mut show = Show::default();
    let d1 = show.outputs[0].id;
    let d2 = show.add_display();
    let d3 = show.add_display();
    let c = show.make_cue(CueType::Image, None, 0);
    let id = c.id;
    show.cues.push(c);
    let mut st = UiState::default();
    st.prefs.new_layout = true;
    st.selected = Some(id);
    let mut w = Win::new();
    w.size = vec2(1300.0, 2400.0);
    (show, st, w, id, [d1, d2, d3])
}

fn also_on_box(w: &Win, d: OutputId) -> Option<Rect> {
    w.get::<Vec<(OutputId, Rect)>>("also_on_boxes")
        .unwrap_or_default()
        .into_iter()
        .find(|b| b.0 == d)
        .map(|b| b.1)
}

#[test]
fn the_also_on_row_ticks_other_displays_and_the_cue_list_counts_them() {
    let (mut show, mut st, mut w, id, [d1, d2, d3]) = three_displays();
    w.frame(&mut show, &mut st);
    w.frame(&mut show, &mut st);
    assert!(also_on_box(&w, d1).is_none(), "not its own display");
    let b2 = also_on_box(&w, d2).expect("the row is drawn");
    assert!(also_on_box(&w, d3).is_some());
    w.click(&mut show, &mut st, b2.center());
    let also = |show: &Show| show.cue(id).unwrap().kind.media().unwrap().also_on.clone();
    assert_eq!(also(&show), vec![d2]);
    let text = crate::ui::cue_target_text(&show, show.cue(id).unwrap());
    assert!(text.ends_with("Display 1 +1"), "{text}");
    // Unticked again: gone, and not written.
    let b2 = also_on_box(&w, d2).unwrap();
    w.click(&mut show, &mut st, b2.center());
    assert!(also(&show).is_empty());
    let json = serde_json::to_string(show.cue(id).unwrap()).unwrap();
    assert!(!json.contains("also_on"), "{json}");
    // A cue with no picture has no such row.
    let s = show.make_cue(CueType::Audio, None, 1);
    let sid = s.id;
    show.cues.push(s);
    st.selected = Some(sid);
    w.frame(&mut show, &mut st);
    w.frame(&mut show, &mut st);
    assert!(also_on_box(&w, d2).is_none());
}

#[test]
fn the_also_on_row_is_locked_in_show_mode() {
    let (mut show, mut st, mut w, id, [_, d2, _]) = three_displays();
    st.show_mode = true;
    w.frame(&mut show, &mut st);
    w.frame(&mut show, &mut st);
    let b2 = also_on_box(&w, d2).expect("shown");
    w.click(&mut show, &mut st, b2.center());
    assert!(
        show.cue(id)
            .unwrap()
            .kind
            .media()
            .unwrap()
            .also_on
            .is_empty()
    );
}

#[test]
fn a_show_with_a_timeline_list_saves_and_loads_and_an_old_one_is_unchanged() {
    let f = fixture();
    let dir = std::env::temp_dir().join(format!("newq-list-tab-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.newq");
    f.show.save(&path).unwrap();
    let back = Show::load(&path).unwrap();
    assert!(is_timeline_list(&back, f.list));
    let starts = |s: &Show| -> Vec<(String, f64)> {
        model::members(s, TimelineKey::List(f.list))
            .into_iter()
            .map(|c| (c.name.clone(), c.pre_wait))
            .collect()
    };
    assert_eq!(starts(&back), starts(&f.show));
    // A show with only plain lists says nothing about timelines.
    let mut plain = Show::default();
    plain.add_list("List 2");
    wait(&mut plain, "A", 0, 0.0, 1.0);
    plain.save(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("timeline"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

// ----- Drops, pastes and moves into a timeline list (I2) -----

fn added(show: &Show, before: &[CueId]) -> Vec<crate::show::Cue> {
    show.cues
        .iter()
        .filter(|c| !before.contains(&c.id))
        .cloned()
        .collect()
}

/// A file let go with the pointer at `at` (the pointer is there a frame first).
fn drop_at(w: &mut Win, f: &mut Fixture, at: Pos2, file: &str) {
    w.frame_with(
        &mut f.show,
        &mut f.st,
        vec![Event::PointerMoved(at)],
        vec![],
    );
    let d: DroppedFileHandle = std::sync::Arc::new(Dropped(file.into()));
    w.frame_with(&mut f.show, &mut f.st, vec![], vec![d]);
}

#[test]
fn a_file_dropped_on_a_track_of_the_tab_is_one_clip_where_it_was_let_go() {
    let mut f = fixture();
    let mut w = Win::new();
    let key = TimelineKey::List(f.list);
    act(Action::SelectList(f.list), &mut f, &mut w);
    act(
        Action::Timeline(TimelineAction::Seek { key, pos: 0.5 }),
        &mut f,
        &mut w,
    );
    act(
        Action::Timeline(TimelineAction::AddTrack { key, picture: true }),
        &mut f,
        &mut w,
    );
    w.frame(&mut f.show, &mut f.st);
    let r = w.rects(Host::ListTab).unwrap();
    let (track, row) = *r
        .tracks
        .iter()
        .find(|t| matches!(t.0, TrackId::Picture { .. }))
        .expect("a picture track");
    let before: Vec<CueId> = f.show.cues.iter().map(|c| c.id).collect();
    drop_at(
        &mut w,
        &mut f,
        pos2(r.x0 + 2.0 * r.pps, row.center().y),
        "still.png",
    );
    let new = added(&f.show, &before);
    assert_eq!(new.len(), 1, "one clip, not one from each");
    assert!((new[0].pre_wait - 2.0).abs() < 0.1, "{}", new[0].pre_wait);
    assert_eq!(model::track_of(&new[0]), track);
    assert_eq!(new[0].list, f.list);
}

#[test]
fn a_file_dropped_on_the_tab_off_the_tracks_is_one_clip_at_the_playhead() {
    let mut f = fixture();
    let mut w = Win::new();
    let key = TimelineKey::List(f.list);
    act(Action::SelectList(f.list), &mut f, &mut w);
    act(
        Action::Timeline(TimelineAction::Seek { key, pos: 0.5 }),
        &mut f,
        &mut w,
    );
    w.frame(&mut f.show, &mut f.st);
    let r = w.rects(Host::ListTab).unwrap();
    let before: Vec<CueId> = f.show.cues.iter().map(|c| c.id).collect();
    // Over the ruler: not a track.
    drop_at(&mut w, &mut f, r.ruler.center(), "still.png");
    let new = added(&f.show, &before);
    assert_eq!(new.len(), 1);
    assert!((new[0].pre_wait - 0.5).abs() < 1e-6, "{}", new[0].pre_wait);
    assert_eq!(new[0].list, f.list);
}

#[test]
fn a_file_dropped_on_a_plain_cue_list_is_one_cue_in_it() {
    let mut f = fixture();
    let mut w = Win::new();
    w.frame(&mut f.show, &mut f.st);
    let rows = w.get::<Vec<(CueId, Rect)>>("cue_rows").unwrap();
    let before: Vec<CueId> = f.show.cues.iter().map(|c| c.id).collect();
    drop_at(&mut w, &mut f, rows[0].1.center(), "still.png");
    let new = added(&f.show, &before);
    assert_eq!(new.len(), 1);
    assert_eq!((new[0].list, new[0].parent), (0, None));
}

#[test]
fn cues_pasted_or_moved_into_a_timeline_list_go_to_its_playhead() {
    let mut f = fixture();
    let mut w = Win::new();
    let key = TimelineKey::List(f.list);
    // Two main-list cues 4 s apart, far from the playhead.
    let x = wait(&mut f.show, "X", 0, 20.0, 1.0);
    let y = wait(&mut f.show, "Y", 0, 24.0, 1.0);
    f.show.normalize_lists();
    f.st.select_many(vec![x, y], y);
    act(Action::Copy, &mut f, &mut w);
    act(Action::SelectList(f.list), &mut f, &mut w);
    act(
        Action::Timeline(TimelineAction::Seek { key, pos: 1.5 }),
        &mut f,
        &mut w,
    );
    f.st.selected = None;
    let before: Vec<CueId> = f.show.cues.iter().map(|c| c.id).collect();
    act(Action::Paste, &mut f, &mut w);
    let mut new = added(&f.show, &before);
    new.sort_by(|a, b| a.pre_wait.total_cmp(&b.pre_wait));
    assert_eq!(new.len(), 2);
    assert!((new[0].pre_wait - 1.5).abs() < 1e-6, "{}", new[0].pre_wait);
    assert!((new[1].pre_wait - 5.5).abs() < 1e-6, "the spacing is kept");
    assert!(new.iter().all(|c| c.list == f.list));
    // Moved in from the main list: at the playhead too.
    act(Action::SelectList(0), &mut f, &mut w);
    act(Action::MoveToList(x, f.list), &mut f, &mut w);
    let c = f.show.cue(x).unwrap();
    assert_eq!(c.list, f.list);
    assert!((c.pre_wait - 1.5).abs() < 1e-6, "{}", c.pre_wait);
    // A move into a plain list keeps its pre-wait.
    let plain = f.show.add_list("Plain");
    act(Action::MoveToList(y, plain), &mut f, &mut w);
    assert_eq!(f.show.cue(y).unwrap().pre_wait, 24.0);
}

// ----- The tab with the editor's tools, marks and displays (I2's cross tests) -----

/// A text clip on the list `list` at `at`, held `secs`, on layer `layer`.
fn text_clip(show: &mut Show, list: u32, at: f64, secs: f64, layer: i32) -> CueId {
    let mut c = show.make_cue(CueType::Text, None, show.cues.len());
    c.list = list;
    c.pre_wait = at;
    if let Some(m) = c.kind.media_mut() {
        m.end = Some(secs);
        m.layer = layer;
        m.fade_in = 0.0;
        m.fade_out = 0.0;
    }
    let id = c.id;
    show.cues.push(c);
    show.normalize_lists();
    id
}

impl Win {
    fn keys(&mut self, show: &mut Show, st: &mut UiState, key: egui::Key, mods: egui::Modifiers) {
        let ev = |pressed| Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: mods,
        };
        self.frame_with(
            show,
            st,
            vec![Event::ModifiersChanged(mods), ev(true), ev(false)],
            vec![],
        );
        self.frame_with(
            show,
            st,
            vec![Event::ModifiersChanged(egui::Modifiers::NONE)],
            vec![],
        );
    }

    fn clip_rect(&mut self, show: &mut Show, st: &mut UiState, id: CueId) -> Rect {
        self.frame(show, st);
        let r = self.rects(Host::ListTab).expect("the tab draws the tracks");
        r.clips
            .iter()
            .find(|c| c.0 == id)
            .expect("the clip is drawn")
            .1
    }

    fn button(&self, name: &str) -> Rect {
        self.get_host::<Vec<(&'static str, Rect)>>("timeline_buttons")
            .unwrap_or_default()
            .into_iter()
            .find(|b| b.0 == name)
            .unwrap_or_else(|| panic!("no {name} button"))
            .1
    }

    fn get_host<T: Clone + Send + Sync + 'static>(&self, name: &'static str) -> Option<T> {
        self.ctx
            .data(|d| d.get_temp::<T>(egui::Id::new((name, Host::ListTab))))
    }

    /// The clips' own words (badges | notes), drawn in a fresh frame.
    fn clip_text(&mut self, show: &mut Show, st: &mut UiState, id: CueId) -> String {
        self.ctx.data_mut(|d| {
            d.remove::<Vec<(CueId, String)>>(egui::Id::new(("timeline_clip_text", Host::ListTab)))
        });
        self.frame(show, st);
        self.get_host::<Vec<(CueId, String)>>("timeline_clip_text")
            .unwrap_or_default()
            .into_iter()
            .rev()
            .find(|c| c.0 == id)
            .map(|c| c.1)
            .unwrap_or_default()
    }
}

#[test]
fn the_tab_splits_ripple_deletes_adds_clips_and_opens_lanes() {
    let mut f = fixture();
    let mut w = Win::new();
    let key = TimelineKey::List(f.list);
    let a = text_clip(&mut f.show, f.list, 0.0, 4.0, 3);
    let b = text_clip(&mut f.show, f.list, 6.0, 4.0, 3);
    act(Action::SelectList(f.list), &mut f, &mut w);
    act(
        Action::Timeline(TimelineAction::Seek { key, pos: 2.0 }),
        &mut f,
        &mut w,
    );
    // Split: pick A, Ctrl+K.
    let r = w.clip_rect(&mut f.show, &mut f.st, a);
    w.click(&mut f.show, &mut f.st, r.center());
    assert_eq!(f.st.selected, Some(a));
    let n = model::members(&f.show, key).len();
    w.keys(
        &mut f.show,
        &mut f.st,
        egui::Key::K,
        egui::Modifiers::COMMAND,
    );
    assert_eq!(model::members(&f.show, key).len(), n + 1, "split in two");
    assert_eq!(f.show.cue(a).unwrap().kind.media().unwrap().end, Some(2.0));
    // Ripple delete of A's first half: B comes up by 2 s.
    let r = w.clip_rect(&mut f.show, &mut f.st, a);
    w.click(&mut f.show, &mut f.st, r.center());
    w.keys(
        &mut f.show,
        &mut f.st,
        egui::Key::Delete,
        egui::Modifiers::SHIFT,
    );
    assert!(f.show.cue(a).is_none());
    assert_eq!(f.show.cue(b).unwrap().pre_wait, 4.0, "rippled");
    // An empty spot of A's track offers a new text there.
    w.frame(&mut f.show, &mut f.st);
    let r = w.rects(Host::ListTab).unwrap();
    let track = model::track_of(f.show.cue(b).unwrap());
    let row = r.tracks.iter().find(|t| t.0 == track).unwrap().1;
    // (A's second half came up to 0–2 s, B to 4–8 s.)
    let spot = pos2(r.x0 + 3.0 * r.pps, row.center().y);
    w.click_with(&mut f.show, &mut f.st, spot, PointerButton::Secondary);
    let item = w.button("Text");
    let before: Vec<CueId> = f.show.cues.iter().map(|c| c.id).collect();
    w.click(&mut f.show, &mut f.st, item.center());
    let new = added(&f.show, &before);
    assert_eq!(new.len(), 1);
    assert_eq!((new[0].list, model::track_of(&new[0])), (f.list, track));
    assert!((new[0].pre_wait - 3.0).abs() < 0.2, "{}", new[0].pre_wait);
    // B's menu opens its opacity lane.
    let r = w.clip_rect(&mut f.show, &mut f.st, b);
    w.click_with(&mut f.show, &mut f.st, r.center(), PointerButton::Secondary);
    let item = w.button("opacity_lane");
    w.click(&mut f.show, &mut f.st, item.center());
    w.frame(&mut f.show, &mut f.st);
    let lanes = w.rects(Host::ListTab).unwrap().lanes;
    assert!(lanes.iter().any(|l| l.0 == b), "{lanes:?}");
}

#[test]
fn the_tab_hatches_a_clip_waiting_to_fade_in_and_one_free_running() {
    let mut f = fixture();
    let mut w = Win::new();
    let key = TimelineKey::List(f.list);
    // Through the free-run pause at 1 s: free-running there.
    let through = text_clip(&mut f.show, f.list, 0.0, 5.0, 1);
    // Unseen from 0.5 s until its opacity rises at 0.8 s.
    let late = text_clip(&mut f.show, f.list, 0.5, 4.0, 2);
    let i = f.show.cue_index(late).unwrap();
    let m = f.show.cues[i].kind.media_mut().unwrap();
    let mut op = crate::show::Animated::new(1.0);
    op.set_key(0.0, 0.0);
    op.set_key(0.3, 0.0);
    op.set_key(0.6, 1.0);
    m.motion.opacity = Some(op);
    act(Action::SelectList(f.list), &mut f, &mut w);
    act(Action::Go, &mut f, &mut w);
    w.e.output_sizes.insert(f.show.outputs[0].id, (64, 36));
    w.e.tick(&f.show, 0.6);
    assert!(w.e.timeline_waiting(key).contains(&late));
    let said = w.clip_text(&mut f.show, &mut f.st, late);
    assert!(said.contains("waiting to fade in"), "{said}");
    for _ in 0..6 {
        w.e.tick(&f.show, 0.2);
    }
    assert!(matches!(
        w.e.timeline(&f.show, key).state,
        RunState::Paused { .. }
    ));
    assert!(w.e.timeline_detached(key).contains(&through));
    let said = w.clip_text(&mut f.show, &mut f.st, through);
    assert!(said.contains("free-running"), "{said}");
}

#[test]
fn a_clip_also_on_a_second_display_has_a_badge_and_a_ghost_on_the_tab() {
    let mut f = fixture();
    let mut w = Win::new();
    let d2 = f.show.add_display();
    let a = text_clip(&mut f.show, f.list, 0.0, 4.0, 0);
    let i = f.show.cue_index(a).unwrap();
    f.show.cues[i].kind.media_mut().unwrap().also_on = vec![d2];
    act(Action::SelectList(f.list), &mut f, &mut w);
    let said = w.clip_text(&mut f.show, &mut f.st, a);
    assert!(said.starts_with("+1"), "{said}");
    let r = w.rects(Host::ListTab).unwrap();
    let ghost = r.ghosts.iter().find(|g| g.0 == a).expect("a ghost").1;
    let ghost_track = r
        .tracks
        .iter()
        .find(|t| t.1.contains(ghost.center()))
        .unwrap()
        .0;
    assert_eq!(
        ghost_track,
        TrackId::Picture {
            output: Some(d2),
            layer: 0
        }
    );
    // A click on the ghost picks the clip.
    f.st.selected = None;
    w.click(&mut f.show, &mut f.st, ghost.center());
    assert_eq!(f.st.selected, Some(a));
}

#[test]
fn a_loop_on_a_timeline_list_is_said_in_the_toolbar_and_the_transport() {
    let mut f = fixture();
    let mut w = Win::new();
    let key = TimelineKey::List(f.list);
    // The pause at 1 s loops the section from 0.5 s, until GO.
    let i = f
        .show
        .cues
        .iter()
        .position(|c| matches!(c.kind, CueKind::Control(_)))
        .unwrap();
    if let CueKind::Control(ctl) = &mut f.show.cues[i].kind {
        ctl.loop_from = Some(crate::show::TimelinePoint::Time(0.5));
    }
    act(Action::SelectList(f.list), &mut f, &mut w);
    act(Action::Go, &mut f, &mut w);
    w.e.tick(&f.show, 0.2);
    w.frame(&mut f.show, &mut f.st);
    let (what, then) = readout(&w).unwrap();
    assert!(what.starts_with("RUNNING · loops back in 0:00.8"), "{what}");
    assert_eq!(then.as_deref(), Some("GO does nothing until it loops"));
    let mut back = false;
    let mut last = 0.0;
    for _ in 0..30 {
        w.e.tick(&f.show, 0.1);
        let pos = w.e.timeline(&f.show, key).pos;
        back |= pos < last - 1e-9;
        last = pos;
    }
    assert!(back, "it went round");
    w.frame(&mut f.show, &mut f.st);
    let state = w.get_host::<String>("timeline_state").unwrap();
    assert!(state.starts_with("Looping"), "{state}");
    let (what, then) = readout(&w).unwrap();
    assert!(
        what.starts_with("Finish this pass and continue Timeline 1"),
        "{what}"
    );
    assert!(then.unwrap().contains("once more"));
    // GO: the last pass.
    act(Action::Go, &mut f, &mut w);
    w.frame(&mut f.show, &mut f.st);
    let state = w.get_host::<String>("timeline_state").unwrap();
    assert!(state.contains("last pass"), "{state}");
}

#[test]
fn with_the_timeline_pane_open_a_timeline_list_opens_on_its_rows_and_the_pane_has_the_tracks() {
    let mut f = fixture();
    let mut w = Win::new();
    act(Action::Timeline(TimelineAction::Open(None)), &mut f, &mut w);
    act(Action::SelectList(f.list), &mut f, &mut w);
    assert!(f.st.timeline.list_rows);
    w.frame(&mut f.show, &mut f.st);
    assert!(w.rects(Host::ListTab).is_none(), "the tab shows rows");
    assert!(w.rects(Host::Pane).is_some(), "the pane shows the tracks");
    // Tracks in the tab, if asked for: the pane then says where they are.
    let tracks = w.tab_button("tracks").unwrap();
    w.click(&mut f.show, &mut f.st, tracks.center());
    w.frame(&mut f.show, &mut f.st);
    assert!(w.rects(Host::ListTab).is_some());
    assert_eq!(w.get::<bool>("timeline_in_tab"), Some(true));
    // Without the pane, the tab opens on its tracks as before.
    act(Action::Timeline(TimelineAction::Close), &mut f, &mut w);
    act(Action::SelectList(0), &mut f, &mut w);
    f.st.timeline.list_rows = false;
    act(Action::SelectList(f.list), &mut f, &mut w);
    assert!(!f.st.timeline.list_rows);
}

#[test]
fn a_clip_over_an_earlier_one_on_its_track_is_drawn_crossfading_in() {
    let mut f = fixture();
    let mut w = Win::new();
    let a = text_clip(&mut f.show, f.list, 0.0, 4.0, 3);
    let b = text_clip(&mut f.show, f.list, 2.5, 4.0, 3);
    act(Action::SelectList(f.list), &mut f, &mut w);
    let said = w.clip_text(&mut f.show, &mut f.st, b);
    assert!(said.contains("crossfades in 1.5 s"), "{said}");
    assert!(
        !w.clip_text(&mut f.show, &mut f.st, a)
            .contains("crossfades")
    );
    // With the timeline's setting off, nothing crossfades.
    model::settings_mut(&mut f.show, TimelineKey::List(f.list))
        .unwrap()
        .crossfade_overlaps = false;
    assert!(
        !w.clip_text(&mut f.show, &mut f.st, b)
            .contains("crossfades")
    );
}
