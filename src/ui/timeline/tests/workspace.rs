//! Layout B, the workspace, driven through the whole window (`ui::draw`, both
//! interfaces) at 1280×760, and the layout setting that chooses it.

use super::*;
use crate::prefs::Prefs;
use crate::timeline::edit;

/// The window at the size the workspace has to be comfortable at.
const SIZE: egui::Vec2 = egui::vec2(1280.0, 760.0);

/// The whole window in one interface, with the timeline editor opening as `layout`.
fn window(f: &mut Fixture, modern: bool, layout: TimelineLayout) -> Mouse {
    let mut m = Mouse::new();
    m.draw = Draw::Full;
    m.size = Some(SIZE);
    f.st.prefs.new_layout = modern;
    f.st.prefs.timeline_layout = layout;
    m.frame(&mut f.show, &mut f.st, vec![]);
    m
}

/// The editors the last frame drew, by host.
fn editors(m: &Mouse) -> Vec<Host> {
    m.ctx
        .data(|d| d.get_temp(egui::Id::new("timeline_editors")))
        .unwrap_or_default()
}

fn rects_in(m: &Mouse, host: Host) -> TimelineRects {
    m.ctx
        .data(|d| d.get_temp::<TimelineRects>(egui::Id::new(("timeline_rects", host))))
        .unwrap_or_else(|| panic!("no editor drawn in {host:?}"))
}

fn noted<T: Clone + Send + Sync + 'static>(m: &Mouse, name: &str) -> Option<T> {
    m.ctx.data(|d| d.get_temp::<T>(egui::Id::new(name)))
}

/// A workspace button (`back`, `as_pane`).
fn ws_button(m: &Mouse, name: &str) -> Rect {
    noted::<Vec<(&'static str, Rect)>>(m, "workspace_buttons")
        .unwrap_or_default()
        .into_iter()
        .find(|b| b.0 == name)
        .unwrap_or_else(|| panic!("no {name} button in the workspace"))
        .1
}

fn bin_item(m: &Mouse, name: &str) -> Rect {
    noted::<Vec<(String, Rect)>>(m, "bin_items")
        .unwrap_or_default()
        .into_iter()
        .find(|b| b.0 == name)
        .unwrap_or_else(|| panic!("no {name} in the bin"))
        .1
}

fn toolbar_button(m: &Mouse, host: Host, name: &str) -> Rect {
    m.ctx
        .data(|d| {
            d.get_temp::<Vec<(&'static str, Rect)>>(egui::Id::new(("timeline_buttons", host)))
        })
        .unwrap_or_default()
        .into_iter()
        .find(|b| b.0 == name)
        .unwrap_or_else(|| panic!("no {name} in the toolbar"))
        .1
}

/// The cue rows the cue list drew (none: it isn't showing).
fn cue_rows(m: &Mouse) -> Vec<(CueId, Rect)> {
    noted(m, "cue_rows").unwrap_or_default()
}

/// Click and let the window make what it asked for; then frames for what
/// opened to settle (a panel shown the first time is laid out twice in its
/// first frame).
fn press(m: &mut Mouse, f: &mut Fixture, at: Pos2) {
    m.click(&mut f.show, &mut f.st, at, PointerButton::Primary);
    settle(m, f);
}

fn settle(m: &mut Mouse, f: &mut Fixture) {
    m.frame(&mut f.show, &mut f.st, vec![]);
    m.frame(&mut f.show, &mut f.st, vec![]);
}

/// Double-click a cue's row in the list.
fn open_by_row(m: &mut Mouse, f: &mut Fixture, id: CueId) {
    m.t += 1.0;
    let at = cue_rows(m)
        .into_iter()
        .find(|r| r.0 == id)
        .expect("the row is drawn")
        .1
        .center();
    m.frame(&mut f.show, &mut f.st, vec![Event::PointerMoved(at)]);
    for _ in 0..2 {
        m.frame(
            &mut f.show,
            &mut f.st,
            vec![button(at, true, PointerButton::Primary)],
        );
        m.frame(
            &mut f.show,
            &mut f.st,
            vec![button(at, false, PointerButton::Primary)],
        );
    }
    settle(m, f);
}

/// Where a time is on a track of the editor in `host`.
fn spot(m: &Mouse, host: Host, track: TrackId, t: f64) -> Pos2 {
    let r = rects_in(m, host);
    let row = r
        .tracks
        .iter()
        .find(|(id, _)| *id == track)
        .expect("the track is drawn")
        .1;
    pos2(r.x0 + t as f32 * r.pps, row.center().y)
}

fn picture_track(show: &Show) -> TrackId {
    TrackId::Picture {
        output: show.outputs.first().map(|o| o.id),
        layer: 0,
    }
}

#[test]
fn each_layout_opens_from_the_cue_list_in_each_interface_once_and_back_returns() {
    for modern in [false, true] {
        for layout in [TimelineLayout::Pane, TimelineLayout::Workspace] {
            let mut f = fixture();
            let mut m = window(&mut f, modern, layout);
            let what = format!("modern {modern}, {layout:?}");
            assert!(editors(&m).is_empty(), "{what}: nothing open yet");
            let g = f.group;
            open_by_row(&mut m, &mut f, g);
            let host = match layout {
                TimelineLayout::Pane => Host::Pane,
                TimelineLayout::Workspace => Host::Workspace,
            };
            assert_eq!(editors(&m), vec![host], "{what}: one editor, in its place");
            assert_eq!(rects_in(&m, host).clips.len(), 4, "{what}");
            if layout == TimelineLayout::Pane {
                // The cue list is still there beside it.
                assert!(!cue_rows(&m).is_empty(), "{what}");
                continue;
            }
            // The workspace took the cue list's place (the panes', or the
            // Editor view's: the new interface is on its Timeline view).
            assert!(cue_rows(&m).is_empty(), "{what}: the cue list is put away");
            assert_eq!(
                f.st.modern_view == super::super::super::modern::View::Timeline,
                modern,
                "{what}"
            );
            assert!(!f.st.layout.is_open(Pane::Timeline), "{what}: no pane too");
            let back = ws_button(&m, "back");
            press(&mut m, &mut f, back.center());
            assert!(editors(&m).is_empty(), "{what}: Back puts it away");
            assert!(!f.st.timeline.open, "{what}");
            assert!(!cue_rows(&m).is_empty(), "{what}: the cue list is back");
            // Window ▸ Timeline (the same action) opens it again where it was.
            super::super::super::apply(
                Action::Timeline(TimelineAction::Open(None)),
                &mut f.show,
                &mut m.e,
                &mut f.st,
            );
            settle(&mut m, &mut f);
            assert_eq!(editors(&m), vec![Host::Workspace], "{what}: reopened");
            assert_eq!(rects_in(&m, Host::Workspace).clips.len(), 4, "{what}");
        }
    }
}

#[test]
fn the_setting_moves_an_open_editor_and_is_kept_for_this_computer() {
    let dir = std::env::temp_dir().join(format!("newq_ws_prefs_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut f = fixture();
    f.st.layout_dir = Some(dir.clone());
    let mut m = window(&mut f, false, TimelineLayout::Pane);
    super::super::super::apply(
        Action::Timeline(TimelineAction::Open(None)),
        &mut f.show,
        &mut m.e,
        &mut f.st,
    );
    settle(&mut m, &mut f);
    assert_eq!(editors(&m), vec![Host::Pane]);
    // The pane's toolbar has the other layout.
    let to_ws = toolbar_button(&m, Host::Pane, "layout");
    press(&mut m, &mut f, to_ws.center());
    assert_eq!(editors(&m), vec![Host::Workspace]);
    assert!(!f.st.layout.is_open(Pane::Timeline), "the pane went");
    assert_eq!(
        Prefs::load(&dir).timeline_layout,
        TimelineLayout::Workspace,
        "kept for this computer"
    );
    // And the workspace has the way back to a pane.
    let as_pane = ws_button(&m, "as_pane");
    press(&mut m, &mut f, as_pane.center());
    assert_eq!(editors(&m), vec![Host::Pane]);
    assert!(f.st.layout.is_docked(Pane::Timeline));
    assert_eq!(Prefs::load(&dir).timeline_layout, TimelineLayout::Pane);
    // At its default it isn't written.
    let text = std::fs::read_to_string(dir.join("prefs.json")).unwrap();
    assert!(!text.contains("timeline_layout"), "{text}");

    // The new interface: the settings page has it too, and an open editor
    // moves from the Editor view's panel to the Timeline view.
    super::super::super::apply(Action::SetNewLayout(true), &mut f.show, &mut m.e, &mut f.st);
    settle(&mut m, &mut f);
    assert_eq!(editors(&m), vec![Host::Pane], "the Editor view's panel");
    super::super::super::apply(
        Action::OpenSettings(super::super::super::SettingsPage::Interface),
        &mut f.show,
        &mut m.e,
        &mut f.st,
    );
    m.frame(&mut f.show, &mut f.st, vec![]);
    let radios: Vec<(bool, Rect)> =
        noted(&m, "settings_timeline_layout").expect("the Interface page has the choice");
    let ws = radios.iter().find(|r| r.0).unwrap().1;
    press(&mut m, &mut f, ws.center());
    assert_eq!(f.st.prefs.timeline_layout, TimelineLayout::Workspace);
    assert!(
        !f.st.settings_open(),
        "the workspace takes the view's place"
    );
    assert_eq!(
        f.st.modern_view,
        super::super::super::modern::View::Timeline
    );
    assert_eq!(editors(&m), vec![Host::Workspace]);
    assert_eq!(Prefs::load(&dir).timeline_layout, TimelineLayout::Workspace);
    // The strip has the Timeline view while the layout is the workspace;
    // leaving it puts the editor away, and its tab opens it again.
    let views: Vec<(super::super::super::modern::View, Rect)> = noted(&m, "modern_views").unwrap();
    let tab = |v| views.iter().find(|x| x.0 == v).unwrap().1;
    let (editor_tab, timeline_tab) = (
        tab(super::super::super::modern::View::Editor),
        tab(super::super::super::modern::View::Timeline),
    );
    press(&mut m, &mut f, editor_tab.center());
    assert!(editors(&m).is_empty() && !f.st.timeline.open);
    press(&mut m, &mut f, timeline_tab.center());
    assert_eq!(editors(&m), vec![Host::Workspace]);
    // Back in the classic interface it stays open, in the panes' place.
    super::super::super::apply(
        Action::SetNewLayout(false),
        &mut f.show,
        &mut m.e,
        &mut f.st,
    );
    settle(&mut m, &mut f);
    assert_eq!(editors(&m), vec![Host::Workspace]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_bin_item_dragged_onto_a_track_becomes_a_clip_there_in_one_undo_step() {
    for modern in [false, true] {
        let mut f = fixture();
        let mut m = window(&mut f, modern, TimelineLayout::Workspace);
        let g = f.group;
        open_by_row(&mut m, &mut f, g);
        let key = TimelineKey::Group(f.group);
        let before = f.show.cues.len();
        let text = bin_item(&m, "Text");
        let to = spot(&m, Host::Workspace, picture_track(&f.show), 3.0);
        m.drag(&mut f.show, &mut f.st, text.center(), to);
        m.frame(&mut f.show, &mut f.st, vec![]);
        assert_eq!(f.show.cues.len(), before + 1, "modern {modern}");
        let made = f.show.cue(f.st.selected.unwrap()).unwrap().clone();
        assert!(matches!(made.kind, CueKind::Text(_)));
        assert_eq!(made.parent, Some(f.group), "on this timeline");
        assert!((made.pre_wait - 3.0).abs() < 0.15, "{}", made.pre_wait);
        let clips = clips_of(&f.show, key);
        let clip = clips.iter().find(|c| c.id == made.id).unwrap();
        assert_eq!(clip.track, picture_track(&f.show), "on the track let go on");
        // Held for a length to trim, as in a timeline list's tab, not for ever.
        assert_eq!(made.kind.media().unwrap().end, Some(edit::STILL_LEN));
        assert_eq!(clip.len, Some(edit::STILL_LEN));
        // One step back takes it off again.
        super::super::super::apply(Action::Undo, &mut f.show, &mut m.e, &mut f.st);
        assert_eq!(f.show.cues.len(), before);
        m.frame(&mut f.show, &mut f.st, vec![]);
        // A pause let go on the cue track is a pause cue at that time.
        let pause = bin_item(&m, "Pause");
        let to = spot(&m, Host::Workspace, TrackId::Cues, 7.0);
        m.drag(&mut f.show, &mut f.st, pause.center(), to);
        m.frame(&mut f.show, &mut f.st, vec![]);
        let made = f.show.cue(f.st.selected.unwrap()).unwrap();
        assert!(
            matches!(&made.kind, CueKind::Control(c) if c.action == ControlAction::TimelinePause),
            "{:?}",
            made.kind
        );
        assert!((made.pre_wait - 7.0).abs() < 0.15, "{}", made.pre_wait);
        // A click on a new item puts it at the playhead.
        let n = f.show.cues.len();
        let shape = bin_item(&m, "Shape");
        press(&mut m, &mut f, shape.center());
        assert_eq!(f.show.cues.len(), n + 1);
        let shape = f.show.cue(f.st.selected.unwrap()).unwrap();
        assert!(matches!(shape.kind, CueKind::Shape(_)));
        assert_eq!(shape.kind.media().unwrap().end, Some(edit::STILL_LEN));
    }
}

#[test]
fn a_clip_dragged_in_the_workspace_moves_in_one_undo_step_and_the_bin_picks_clips() {
    let mut f = fixture();
    let mut m = window(&mut f, false, TimelineLayout::Workspace);
    let g = f.group;
    open_by_row(&mut m, &mut f, g);
    let before = f.show.cue(f.b).unwrap().pre_wait;
    let r = rects_in(&m, Host::Workspace);
    let b = r.clips.iter().find(|c| c.0 == f.b).unwrap().1;
    m.drag(
        &mut f.show,
        &mut f.st,
        b.center(),
        b.center() + vec2(1.5 * r.pps, 0.0),
    );
    m.frame(&mut f.show, &mut f.st, vec![]);
    let moved = f.show.cue(f.b).unwrap().pre_wait;
    assert!((moved - before - 1.5).abs() < 0.3, "{before} -> {moved}");
    super::super::super::apply(Action::Undo, &mut f.show, &mut m.e, &mut f.st);
    assert_eq!(f.show.cue(f.b).unwrap().pre_wait, before);
    // The bin lists the clips; a click picks one, and the inspector shows its
    // timeline rows first.
    m.frame(&mut f.show, &mut f.st, vec![]);
    let number = f.show.cue(f.c).unwrap().number.clone();
    let shelf = bin_item(&m, "shelf CLIPS").center();
    press(&mut m, &mut f, shelf);
    let row = bin_item(&m, &format!("clip {number}"));
    press(&mut m, &mut f, row.center());
    assert_eq!(f.st.selected, Some(f.c));
    assert!(m.rows().contains(&"run_out"), "{:?}", m.rows());
}

#[test]
fn show_mode_refuses_edits_in_the_workspace_and_back_still_works() {
    let mut f = fixture();
    let mut m = window(&mut f, false, TimelineLayout::Workspace);
    let g = f.group;
    open_by_row(&mut m, &mut f, g);
    f.st.show_mode = true;
    m.frame(&mut f.show, &mut f.st, vec![]);
    let was = serde_json::to_string(&f.show).unwrap();
    let r = rects_in(&m, Host::Workspace);
    let b = r.clips.iter().find(|c| c.0 == f.b).unwrap().1;
    m.drag(
        &mut f.show,
        &mut f.st,
        b.center(),
        b.center() + vec2(1.5 * r.pps, 0.0),
    );
    let text = bin_item(&m, "Text");
    let to = spot(&m, Host::Workspace, picture_track(&f.show), 3.0);
    m.drag(&mut f.show, &mut f.st, text.center(), to);
    let shape = bin_item(&m, "Shape");
    press(&mut m, &mut f, shape.center());
    assert_eq!(
        serde_json::to_string(&f.show).unwrap(),
        was,
        "nothing changed"
    );
    // The transport still runs it (show mode is for running the show).
    let run = toolbar_button(&m, Host::Workspace, "run");
    press(&mut m, &mut f, run.center());
    assert_ne!(
        m.e.timeline(&f.show, TimelineKey::Group(f.group)).state,
        RunState::Stopped
    );
    let back = ws_button(&m, "back");
    press(&mut m, &mut f, back.center());
    assert!(!f.st.timeline.open);
}

#[test]
fn a_timeline_pane_or_its_pop_out_points_to_the_workspace_and_draws_no_second_editor() {
    let mut f = fixture();
    // A pane left in the panes (say from an earlier layout) and the
    // workspace open: the workspace is drawn, once.
    f.st.layout.open(Pane::Timeline);
    let mut m = window(&mut f, false, TimelineLayout::Workspace);
    let g = f.group;
    open_by_row(&mut m, &mut f, g);
    assert_eq!(editors(&m), vec![Host::Workspace]);
    // In a window of its own the pane only points to it.
    f.st.layout.pop_out(Pane::Timeline);
    let mut p = Mouse::new();
    p.draw = Draw::Popout;
    p.frame(&mut f.show, &mut f.st, vec![]);
    p.frame(&mut f.show, &mut f.st, vec![]);
    assert!(editors(&p).is_empty());
    assert_eq!(noted::<bool>(&p, "timeline_pointer"), Some(true));
    // The workspace in the main window is unchanged.
    settle(&mut m, &mut f);
    assert_eq!(editors(&m), vec![Host::Workspace]);
}

#[test]
fn a_timeline_lists_tab_opens_the_workspace_and_back_shows_its_rows() {
    let mut f = fixture();
    let list = edit::new_timeline_list(&mut f.show, "Act 2");
    let key = TimelineKey::List(list);
    let track = picture_track(&f.show);
    let clip = edit::add_clip(&mut f.show, key, CueType::Text, None, track, 1.0).unwrap();
    for modern in [false, true] {
        let mut m = window(&mut f, modern, TimelineLayout::Workspace);
        let tab: Vec<(u32, Rect)> = noted(&m, "list_tabs").unwrap();
        let tab = tab.iter().find(|t| t.0 == list).unwrap().1;
        press(&mut m, &mut f, tab.center());
        assert_eq!(m.e.active_list, list);
        assert_eq!(editors(&m), vec![Host::Workspace], "modern {modern}");
        assert_eq!(rects_in(&m, Host::Workspace).clips[0].0, clip);
        let back = ws_button(&m, "back");
        press(&mut m, &mut f, back.center());
        assert!(editors(&m).is_empty(), "modern {modern}");
        // The tab shows the list's rows, and its Tracks button opens the
        // workspace again.
        assert!(cue_rows(&m).iter().any(|r| r.0 == clip), "modern {modern}");
        let tracks: Vec<(&'static str, Rect)> = noted(&m, "list_tab_buttons").unwrap();
        let tracks = tracks.iter().find(|b| b.0 == "tracks").unwrap().1;
        press(&mut m, &mut f, tracks.center());
        assert_eq!(editors(&m), vec![Host::Workspace], "modern {modern}");
        // Leave it, and the list, for the other interface.
        let back = ws_button(&m, "back");
        press(&mut m, &mut f, back.center());
        super::super::super::apply(Action::SelectList(0), &mut f.show, &mut m.e, &mut f.st);
    }
    // As a pane, the tab shows the tracks itself (as before).
    let mut m = window(&mut f, false, TimelineLayout::Pane);
    super::super::super::apply(Action::SelectList(list), &mut f.show, &mut m.e, &mut f.st);
    settle(&mut m, &mut f);
    assert_eq!(editors(&m), vec![Host::ListTab]);
}

#[test]
fn the_workspace_is_comfortable_at_1280_by_760_in_both_interfaces() {
    for modern in [false, true] {
        let mut f = fixture();
        // A display to show.
        let out = f.show.outputs.first().unwrap().id;
        f.st.previews.push(super::super::super::PreviewOutput {
            id: out,
            name: "Display 1".into(),
            layers: Vec::new(),
            warp: crate::gpu::IDENTITY,
            mesh: None,
            slice: [0.0, 0.0, 1.0, 1.0],
            canvas: (1920, 1080),
            picture: (1920, 1080),
        });
        // Seven clips, four files: more than the bin shows at once.
        for (i, name) in ["D", "E", "F"].into_iter().enumerate() {
            let id = super::picture(&mut f.show, name, 10.0 + i as f64, 1.0, 2, f.group);
            if let Some(m) = f.show.cues.iter_mut().find(|c| c.id == id)
                && let Some(m) = m.kind.media_mut()
            {
                m.file = format!("{name}.png").into();
            }
        }
        for id in [f.a, f.b] {
            if let Some(m) = f.show.cues.iter_mut().find(|c| c.id == id)
                && let Some(m) = m.kind.media_mut()
            {
                m.file = "AB.png".into();
            }
        }
        let mut m = window(&mut f, modern, TimelineLayout::Workspace);
        let g = f.group;
        open_by_row(&mut m, &mut f, g);
        // Pick a clip: its inspector.
        let r = rects_in(&m, Host::Workspace);
        let a = r.clips.iter().find(|c| c.0 == f.a).unwrap().1;
        press(&mut m, &mut f, a.center());
        assert_eq!(f.st.selected, Some(f.a));
        let r = rects_in(&m, Host::Workspace);
        let what = format!("modern {modern}");
        // The tracks across the window, every track in sight.
        assert!(r.ruler.width() > 1000.0, "{what}: {:?}", r.ruler);
        let bottom = r.tracks.iter().map(|t| t.1.bottom()).fold(0.0, f32::max);
        assert!(
            bottom < SIZE.y - 30.0,
            "{what}: the last track ends at {bottom}"
        );
        // A picture worth looking at.
        let monitor: Rect = noted(&m, "ws_monitor").expect("the monitor is drawn");
        assert!(
            monitor.width() >= 280.0 && monitor.height() >= 150.0,
            "{what}: {monitor:?}"
        );
        // The display's own shape inside it, never stretched to the panel's.
        let picture: Rect = noted(&m, "ws_picture").expect("the picture is drawn");
        assert!(
            (picture.width() / picture.height() - 16.0 / 9.0).abs() < 0.01
                && monitor.expand(0.5).contains_rect(picture),
            "{what}: {picture:?} in {monitor:?}"
        );
        // It asked the previews' governor, as every preview does.
        assert!(
            f.st.preview_governor
                .asked
                .borrow()
                .contains(&crate::preview_pace::PreviewId::Workspace),
            "{what}"
        );
        // The clip's timeline rows are at the top of the inspector, in sight.
        let rows: Rect = noted(&m, "ws_rows").unwrap();
        assert!(m.rows().contains(&"run_out"), "{what}");
        assert!(
            rows.height() > 20.0 && rows.bottom() < r.ruler.top(),
            "{what}: {rows:?}"
        );
        // The bin under the monitor: each shelf useful without scrolling.
        let bin: Rect = noted(&m, "ws_bin").expect("the bin is drawn");
        assert!(bin.top() >= monitor.bottom(), "{what}: {bin:?} {monitor:?}");
        let new = noted::<Vec<(String, Rect)>>(&m, "bin_items")
            .unwrap_or_default()
            .into_iter()
            .filter(|(n, _)| !n.starts_with("shelf "))
            .collect::<Vec<_>>();
        assert_eq!(new.len(), 10, "{what}: {new:?}");
        for (name, r) in &new {
            assert!(
                r.bottom() <= bin.bottom() + 0.5,
                "{what}: {name} is out of sight in {bin:?}"
            );
        }
        let at = bin_item(&m, "shelf FILES").center();
        press(&mut m, &mut f, at);
        let files = noted::<Vec<(String, Rect)>>(&m, "bin_items")
            .unwrap_or_default()
            .into_iter()
            .filter(|(n, _)| n.ends_with(".png"))
            .collect::<Vec<_>>();
        assert_eq!(files.len(), 4, "{what}: {files:?}");
        assert!(
            files.iter().all(|(_, r)| r.bottom() <= bin.bottom() + 0.5),
            "{what}: {files:?} in {bin:?}"
        );
        let at = bin_item(&m, "shelf CLIPS").center();
        press(&mut m, &mut f, at);
        let clips = noted::<Vec<(String, Rect)>>(&m, "bin_items")
            .unwrap_or_default()
            .into_iter()
            .filter(|(n, _)| n.starts_with("clip "))
            .collect::<Vec<_>>();
        assert_eq!(clips.len(), 7, "{what}: {clips:?}");
        let seen = clips
            .iter()
            .filter(|(_, r)| r.bottom() <= bin.bottom() + 0.5)
            .count();
        assert!(seen >= 5, "{what}: {seen} of 7 clips in sight in {bin:?}");
        // The shelf picked stays picked.
        settle(&mut m, &mut f);
        assert!(bin_item(&m, &clips[0].0).height() > 0.0, "{what}");
    }
}

#[test]
fn the_layout_action_is_not_an_edit() {
    let mut f = fixture();
    let mut e = Engine::for_tests();
    f.st.show_mode = true;
    let a = Action::Timeline(TimelineAction::SetLayout(TimelineLayout::Workspace));
    super::super::super::apply(a, &mut f.show, &mut e, &mut f.st);
    assert_eq!(f.st.prefs.timeline_layout, TimelineLayout::Workspace);
}

#[test]
fn the_group_inspector_offers_to_open_the_editor_only_where_it_is_not_open_on_it() {
    for modern in [false, true] {
        let what = format!("modern {modern}");
        let mut f = fixture();
        let mut m = window(&mut f, modern, TimelineLayout::Workspace);
        let g = f.group;
        // Not open: the group's inspector offers it.
        super::super::super::apply(Action::Select(g), &mut f.show, &mut m.e, &mut f.st);
        settle(&mut m, &mut f);
        assert_eq!(noted::<bool>(&m, "open_in_editor"), Some(true), "{what}");
        open_by_row(&mut m, &mut f, g);
        assert_eq!(editors(&m), vec![Host::Workspace], "{what}");
        assert_eq!(f.st.selected, Some(g), "{what}");
        // Open on it: the inspector in the workspace says where its tracks are.
        m.ctx
            .data_mut(|d| d.remove::<bool>(egui::Id::new("open_in_editor")));
        settle(&mut m, &mut f);
        assert_eq!(noted::<bool>(&m, "open_in_editor"), Some(false), "{what}");
        // Back to the cue list: offered again.
        let back = ws_button(&m, "back").center();
        press(&mut m, &mut f, back);
        assert!(editors(&m).is_empty(), "{what}");
        settle(&mut m, &mut f);
        assert_eq!(noted::<bool>(&m, "open_in_editor"), Some(true), "{what}");
    }
    // The new interface's Editor view with its timeline panel on the group: the same.
    let mut f = fixture();
    let mut m = window(&mut f, true, TimelineLayout::Pane);
    let g = f.group;
    open_by_row(&mut m, &mut f, g);
    assert_eq!(editors(&m), vec![Host::Pane]);
    m.ctx
        .data_mut(|d| d.remove::<bool>(egui::Id::new("open_in_editor")));
    settle(&mut m, &mut f);
    assert_eq!(noted::<bool>(&m, "open_in_editor"), Some(false));
    super::super::super::apply(
        Action::Timeline(TimelineAction::Close),
        &mut f.show,
        &mut m.e,
        &mut f.st,
    );
    settle(&mut m, &mut f);
    assert_eq!(noted::<bool>(&m, "open_in_editor"), Some(true));
}

/// A remote command arrives between frames (as `app.rs` hands them to
/// `ui::run_remote`) while the workspace is open on its timeline and a clip is
/// being dragged: the remote's seek and run happen once, the drag lands once
/// on release as one undo step, and neither undoes the other.
#[test]
fn a_remote_command_during_a_drag_in_the_workspace_is_made_once_and_the_drag_lands_once() {
    use crate::remote::{Command, Place, TimelineDo};
    for modern in [false, true] {
        let what = format!("modern {modern}");
        let mut f = fixture();
        let mut m = window(&mut f, modern, TimelineLayout::Workspace);
        let g = f.group;
        open_by_row(&mut m, &mut f, g);
        let key = TimelineKey::Group(g);
        let name = format!("#{}", g.0);
        let remote = |cmd: Command, m: &mut Mouse, f: &mut Fixture| {
            super::super::super::run_remote(cmd, &mut f.show, &mut m.e, &mut f.st);
        };
        let before = f.show.cue(f.b).unwrap().pre_wait;
        let r = rects_in(&m, Host::Workspace);
        let from = r.clips.iter().find(|c| c.0 == f.b).unwrap().1.center();
        let to = from + vec2(1.5 * r.pps, 0.0);
        m.frame(&mut f.show, &mut f.st, vec![Event::PointerMoved(from)]);
        m.frame(
            &mut f.show,
            &mut f.st,
            vec![button(from, true, PointerButton::Primary)],
        );
        for i in 1..=4 {
            let p = from + (to - from) * (i as f32 / 8.0);
            m.frame(&mut f.show, &mut f.st, vec![Event::PointerMoved(p)]);
        }
        assert!(f.st.timeline.drag.is_some(), "{what}: dragging");
        // Mid-drag: a remote parks the timeline at 3 s.
        remote(
            Command::Timeline {
                timeline: name.clone(),
                does: TimelineDo::Jump(Place::Time(3.0)),
            },
            &mut m,
            &mut f,
        );
        let parked = m.e.timeline(&f.show, key);
        assert!((parked.pos - 3.0).abs() < 1e-6, "{what}: {}", parked.pos);
        for i in 5..=7 {
            let p = from + (to - from) * (i as f32 / 8.0);
            m.frame(&mut f.show, &mut f.st, vec![Event::PointerMoved(p)]);
        }
        assert!(f.st.timeline.drag.is_some(), "{what}: still dragging");
        assert_eq!(
            f.show.cue(f.b).unwrap().pre_wait,
            before,
            "{what}: nothing moved before the release"
        );
        // Then runs it, still mid-drag.
        remote(
            Command::Timeline {
                timeline: name.clone(),
                does: TimelineDo::Run(None),
            },
            &mut m,
            &mut f,
        );
        assert!(
            matches!(
                m.e.timeline(&f.show, key).state,
                crate::engine::RunState::Running | crate::engine::RunState::Cueing
            ),
            "{what}: {:?}",
            m.e.timeline(&f.show, key).state
        );
        m.frame(
            &mut f.show,
            &mut f.st,
            vec![
                Event::PointerMoved(to),
                button(to, false, PointerButton::Primary),
            ],
        );
        settle(&mut m, &mut f);
        assert!(f.st.timeline.drag.is_none(), "{what}");
        let moved = f.show.cue(f.b).unwrap().pre_wait;
        assert!(
            (moved - before - 1.5).abs() < 0.3,
            "{what}: moved once: {before} -> {moved}"
        );
        // More frames move it no further.
        settle(&mut m, &mut f);
        assert_eq!(f.show.cue(f.b).unwrap().pre_wait, moved, "{what}");
        // One undo step takes the move back, and leaves the run running.
        super::super::super::apply(Action::Undo, &mut f.show, &mut m.e, &mut f.st);
        assert_eq!(f.show.cue(f.b).unwrap().pre_wait, before, "{what}");
        assert_ne!(
            m.e.timeline(&f.show, key).state,
            crate::engine::RunState::Stopped,
            "{what}: the undo doesn't stop the run"
        );
        // A second undo has nothing of the drag's or the remote's to take.
        super::super::super::apply(Action::Undo, &mut f.show, &mut m.e, &mut f.st);
        assert_eq!(f.show.cue(f.b).unwrap().pre_wait, before, "{what}");
    }
}

/// The playhead is being dragged on the ruler when something else moves the
/// run (a remote's jump, run, GO or pause, between frames as `app.rs` hands
/// them over): the drag gives way. Its ghost playhead goes at once, and its
/// release asks for nothing, so the remote's place and state stand. With
/// nothing in between, the release parks the run where it was let go.
#[test]
fn a_remote_command_during_a_playhead_drag_cancels_the_drag() {
    use crate::engine::RunState;
    use crate::remote::{Command, Place, TimelineDo};
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum Meanwhile {
        Nothing,
        Jump,
        Run,
        Go,
        Pause,
    }
    for modern in [false, true] {
        for meanwhile in [
            Meanwhile::Nothing,
            Meanwhile::Jump,
            Meanwhile::Run,
            Meanwhile::Go,
            Meanwhile::Pause,
        ] {
            let what = format!("modern {modern}, {meanwhile:?}");
            let mut f = fixture();
            let mut m = window(&mut f, modern, TimelineLayout::Workspace);
            let g = f.group;
            open_by_row(&mut m, &mut f, g);
            let key = TimelineKey::Group(g);
            let name = format!("#{}", g.0);
            let remote = |does: TimelineDo, m: &mut Mouse, f: &mut Fixture| {
                let cmd = Command::Timeline {
                    timeline: name.clone(),
                    does,
                };
                super::super::super::run_remote(cmd, &mut f.show, &mut m.e, &mut f.st);
            };
            if meanwhile == Meanwhile::Pause {
                // Running before the drag begins.
                remote(TimelineDo::Run(None), &mut m, &mut f);
                settle(&mut m, &mut f);
            }
            let r = rects_in(&m, Host::Workspace);
            let at = |t: f64| pos2(r.x0 + t as f32 * r.pps, r.ruler.center().y);
            let (from, to) = (at(1.0), at(2.5));
            m.frame(&mut f.show, &mut f.st, vec![Event::PointerMoved(from)]);
            m.frame(
                &mut f.show,
                &mut f.st,
                vec![button(from, true, PointerButton::Primary)],
            );
            for i in 1..=4 {
                let p = from + (to - from) * (i as f32 / 8.0);
                m.frame(&mut f.show, &mut f.st, vec![Event::PointerMoved(p)]);
            }
            assert!(
                matches!(
                    f.st.timeline.drag,
                    Some((_, Drag::Playhead { pos: Some(_), .. }))
                ),
                "{what}: dragging the playhead"
            );
            match meanwhile {
                Meanwhile::Nothing => {}
                Meanwhile::Jump => remote(TimelineDo::Jump(Place::Time(6.0)), &mut m, &mut f),
                Meanwhile::Run => remote(TimelineDo::Run(None), &mut m, &mut f),
                Meanwhile::Go => remote(TimelineDo::Go, &mut m, &mut f),
                Meanwhile::Pause => remote(TimelineDo::Pause, &mut m, &mut f),
            }
            let after = m.e.timeline(&f.show, key);
            for i in 5..=8 {
                let p = from + (to - from) * (i as f32 / 8.0);
                m.frame(&mut f.show, &mut f.st, vec![Event::PointerMoved(p)]);
            }
            // (What a frame draws is where the drag was as it began.)
            m.frame(&mut f.show, &mut f.st, vec![]);
            let r = rects_in(&m, Host::Workspace);
            let ghost = r.playhead_x.expect("a playhead drawn");
            if meanwhile == Meanwhile::Nothing {
                assert!(
                    (ghost - to.x).abs() < 2.0,
                    "{what}: the ghost follows: {ghost} {}",
                    to.x
                );
            } else {
                assert!(
                    matches!(
                        f.st.timeline.drag,
                        Some((_, Drag::Playhead { pos: None, .. }))
                    ),
                    "{what}: the drag gave way"
                );
                let run_x = r.x0 + m.e.timeline(&f.show, key).pos as f32 * r.pps;
                assert!(
                    (ghost - run_x).abs() < 2.0,
                    "{what}: the ghost is gone ({ghost} vs the run's {run_x})"
                );
            }
            m.frame(
                &mut f.show,
                &mut f.st,
                vec![
                    Event::PointerMoved(to),
                    button(to, false, PointerButton::Primary),
                ],
            );
            settle(&mut m, &mut f);
            assert!(f.st.timeline.drag.is_none(), "{what}");
            // And a press held on after the cancel doesn't start another drag
            // (checked above by `pos: None` lasting to the release).
            let v = m.e.timeline(&f.show, key);
            match meanwhile {
                Meanwhile::Nothing => {
                    assert!(v.parked && (v.pos - 2.5).abs() < 0.1, "{what}: {v:?}")
                }
                Meanwhile::Jump => {
                    assert!(v.parked && (v.pos - 6.0).abs() < 1e-6, "{what}: {v:?}")
                }
                Meanwhile::Run | Meanwhile::Go => {
                    assert!(
                        matches!(v.state, RunState::Running | RunState::Cueing) && v.pos < 1.0,
                        "{what}: still running from the top: {v:?}"
                    )
                }
                Meanwhile::Pause => {
                    assert!(
                        matches!(v.state, RunState::Paused { .. })
                            && (v.pos - after.pos).abs() < 1e-6,
                        "{what}: paused where the remote paused it: {v:?}"
                    )
                }
            }
        }
    }
}

/// A control cue's "Do" menu open at 1280×760 in both interfaces: every
/// action, the timeline ones after the line included, is on the screen and
/// seen in the menu, with no scrolling.
#[test]
fn every_action_of_the_do_menu_is_in_sight_without_scrolling() {
    use crate::show::ControlAction;
    for modern in [false, true] {
        let what = format!("modern {modern}");
        let mut f = fixture();
        let mut cue = f.show.make_cue(CueType::Control, None, f.show.cues.len());
        cue.name = "Do it".into();
        let id = cue.id;
        f.show.cues.push(cue);
        f.st.selected = Some(id);
        let mut m = window(&mut f, modern, TimelineLayout::Pane);
        // The app's own sizes (its rows are taller than egui's).
        super::super::super::setup_style(&m.ctx);
        settle(&mut m, &mut f);
        let combo: Rect = noted(&m, "control_action_combo")
            .unwrap_or_else(|| panic!("{what}: the Do menu's button is drawn"));
        press(&mut m, &mut f, combo.center());
        // A popup is placed by its size, which it has only after a frame or two.
        settle(&mut m, &mut f);
        let rows: Vec<(ControlAction, Rect, Rect)> =
            noted(&m, "control_action_rows").unwrap_or_default();
        let screen = Rect::from_min_size(Pos2::ZERO, SIZE);
        let shown: Vec<_> = rows.iter().map(|r| r.0).collect();
        assert_eq!(shown, ControlAction::ALL.to_vec(), "{what}: all listed");
        for (a, r, clip) in &rows {
            assert!(
                screen.contains_rect(*r) && clip.contains_rect(*r),
                "{what}: {a:?} at {r:?} is out of sight (seen: {clip:?})"
            );
        }
    }
}

/// The classic interface with the editor as a pane: a Timeline pane on the
/// group, in front of its tabs, is where its tracks are, so the inspector says
/// so; behind another tab it isn't seen, so the button stays, and brings it
/// to the front.
#[test]
fn a_timeline_pane_in_front_on_the_group_replaces_the_open_button_and_one_behind_comes_forward() {
    let mut f = fixture();
    let mut m = window(&mut f, false, TimelineLayout::Pane);
    // Tall enough for the button to be in sight above the pane, unscrolled.
    m.size = Some(egui::vec2(1280.0, 1300.0));
    let g = f.group;
    super::super::super::apply(Action::Select(g), &mut f.show, &mut m.e, &mut f.st);
    settle(&mut m, &mut f);
    assert_eq!(noted::<bool>(&m, "open_in_editor"), Some(true), "not open");
    open_by_row(&mut m, &mut f, g);
    assert!(f.st.layout.in_front(Pane::Timeline));
    assert_eq!(f.st.selected, Some(g));
    m.ctx
        .data_mut(|d| d.remove::<bool>(egui::Id::new("open_in_editor")));
    settle(&mut m, &mut f);
    assert_eq!(
        noted::<bool>(&m, "open_in_editor"),
        Some(false),
        "the pane in front shows its tracks"
    );
    // Another tab put in front of it in its node.
    f.st.layout.close(Pane::Active);
    let tl = f.st.layout.dock.find_tab(&Pane::Timeline).unwrap();
    f.st.layout
        .dock
        .set_focused_node_and_surface(tl.node_path());
    f.st.layout.dock.push_to_focused_leaf(Pane::Active);
    assert!(!f.st.layout.in_front(Pane::Timeline) && f.st.layout.is_docked(Pane::Timeline));
    settle(&mut m, &mut f);
    assert_eq!(
        noted::<bool>(&m, "open_in_editor"),
        Some(true),
        "behind a tab: offered"
    );
    let b: Rect = noted(&m, "open_in_editor_button").expect("the button");
    press(&mut m, &mut f, b.center());
    assert!(f.st.layout.in_front(Pane::Timeline), "brought to the front");
    m.ctx
        .data_mut(|d| d.remove::<bool>(egui::Id::new("open_in_editor")));
    settle(&mut m, &mut f);
    assert_eq!(noted::<bool>(&m, "open_in_editor"), Some(false));
    assert!(editors(&m).contains(&Host::Pane));
}
