//! The ruler's marker flags zoomed out: markers at one time (or nearer than a
//! few points) are one flag with a count, as cues are one chip on the Cues
//! track (the same `tracks::chips`), each still picked and dragged with the
//! mouse, the hovered and picked drawn whole on top.

use super::*;

/// The flags the last frame drew: their markers, where, and the text.
fn flags(m: &Mouse) -> Vec<(Vec<u64>, Rect, String)> {
    m.ctx
        .data(|d| d.get_temp(egui::Id::new(("timeline_marker_flags", Host::Pane))))
        .unwrap_or_default()
}

fn whole(m: &Mouse) -> Vec<String> {
    m.ctx
        .data(|d| d.get_temp(egui::Id::new(("timeline_marker_whole", Host::Pane))))
        .unwrap_or_default()
}

fn flag_of(m: &Mouse, id: u64) -> (Vec<u64>, Rect, String) {
    flags(m)
        .into_iter()
        .find(|f| f.0.contains(&id))
        .expect("a flag")
}

/// The fixture's marker "Chorus" (1) at 5 s, with "Bridge" (2) at the same
/// time, "Coda" (3) 0.05 s after (apart only when zoomed in) and "Outro" (4)
/// at 9 s.
fn markers_fixture() -> Fixture {
    let mut f = fixture();
    let key = TimelineKey::Group(f.group);
    let s = model::settings_mut(&mut f.show, key).unwrap();
    for (id, time, name) in [(2, 5.0, "Bridge"), (3, 5.05, "Coda"), (4, 9.0, "Outro")] {
        s.markers.push(Marker {
            id,
            time,
            name: name.into(),
            color: None,
        });
    }
    f
}

#[test]
fn markers_at_one_time_are_one_flag_with_a_count_and_part_when_zoomed_in() {
    let mut f = markers_fixture();
    let mut m = Mouse::new();
    m.frame(&mut f.show, &mut f.st, vec![]);
    let (ids, rect, text) = flag_of(&m, 1);
    assert_eq!(ids, vec![1, 2, 3], "one flag for the three");
    assert!(text.starts_with("×3 "), "{text}");
    let r = m.rects().unwrap();
    for id in [1, 2, 3] {
        assert_eq!(r.markers.iter().find(|x| x.0 == id).unwrap().1, rect);
    }
    assert_eq!(flag_of(&m, 4).0, vec![4], "the one apart is its own");
    assert!(!flag_of(&m, 4).2.starts_with('×'));
    // Zoomed right in: Coda parts from the two at one time; they never do.
    f.st.timeline.pps = 400.0;
    f.st.timeline.scroll.x = 4.9 * 400.0;
    m.frame(&mut f.show, &mut f.st, vec![]);
    assert_eq!(flag_of(&m, 1).0, vec![1, 2]);
    assert_eq!(flag_of(&m, 3).0, vec![3]);
    assert!(flag_of(&m, 1).2.starts_with("×2 "));
}

#[test]
fn a_click_on_a_flag_of_several_picks_each_in_turn_and_a_drag_moves_the_one_shown() {
    let mut f = markers_fixture();
    let key = TimelineKey::Group(f.group);
    let mut m = Mouse::new();
    m.frame(&mut f.show, &mut f.st, vec![]);
    let rect = flag_of(&m, 1).1;
    let picked = |f: &Fixture| f.st.timeline.marker_pick.map(|(_, id)| id);
    let mut seen = Vec::new();
    for _ in 0..4 {
        // (Far enough apart not to be a double-click, which renames.)
        m.t += 1.0;
        m.click(
            &mut f.show,
            &mut f.st,
            rect.center(),
            PointerButton::Primary,
        );
        seen.push(picked(&f).expect("a marker picked"));
    }
    assert_eq!(seen, vec![1, 2, 3, 1], "each in turn, then round again");
    // The picked one is what the flag shows, whole on top.
    m.t += 1.0;
    m.click(
        &mut f.show,
        &mut f.st,
        rect.center(),
        PointerButton::Primary,
    );
    assert_eq!(picked(&f), Some(2));
    m.frame(
        &mut f.show,
        &mut f.st,
        vec![Event::PointerMoved(pos2(2.0, 2.0))],
    );
    assert_eq!(flag_of(&m, 2).2, "×3 Bridge");
    assert_eq!(whole(&m), vec!["×3 Bridge".to_string()], "picked: whole");
    // A drag takes the one shown.
    let pps = m.rects().unwrap().pps;
    let frames = m.drag(
        &mut f.show,
        &mut f.st,
        rect.center(),
        rect.center() + vec2(pps * 2.0, 0.0),
    );
    match timeline_actions(frames.last().unwrap()).as_slice() {
        [TimelineAction::MoveMarker { key: k, id: 2, to }] => {
            assert_eq!(*k, key);
            assert!((to - 7.0).abs() < 0.15, "{to}");
        }
        other => panic!("wanted Bridge moved, got {other:?}"),
    }
    // A press elsewhere lets go of the pick.
    let r = m.rects().unwrap();
    let empty = pos2(r.x0 + 12.0 * r.pps, r.tracks.last().unwrap().1.center().y);
    m.click(&mut f.show, &mut f.st, empty, PointerButton::Primary);
    assert_eq!(picked(&f), None);
}

#[test]
fn the_hovered_flag_is_drawn_whole_on_top() {
    let mut f = markers_fixture();
    let mut m = Mouse::new();
    m.frame(&mut f.show, &mut f.st, vec![]);
    assert!(whole(&m).is_empty());
    let rect = flag_of(&m, 4).1;
    m.frame(
        &mut f.show,
        &mut f.st,
        vec![Event::PointerMoved(rect.center())],
    );
    m.frame(&mut f.show, &mut f.st, vec![]);
    assert_eq!(whole(&m), vec!["Outro".to_string()]);
}

#[test]
fn the_markers_are_grouped_by_the_cues_tracks_rule() {
    let items = [(7u64, 100.0, true), (8, 102.0, true), (9, 200.0, true)];
    let plan = tracks::chips(&items, 320.0);
    let get = |n| plan.iter().find(|(i, _)| *i == n).unwrap().1.clone();
    assert!(matches!(get(7), tracks::Chip::Lead { members, .. } if members == vec![7, 8]));
    assert_eq!(get(8), tracks::Chip::Under);
    assert!(matches!(get(9), tracks::Chip::Lead { members, .. } if members == vec![9]));
}
