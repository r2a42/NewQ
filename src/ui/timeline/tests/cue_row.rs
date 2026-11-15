//! The Cues track zoomed out: each chip's name in the room up to the next,
//! cut short or left out, the hovered or picked one whole on top, and cues
//! close together or at one time each still picked with the mouse.

use super::tracks::{LabelNote, MIN_LABEL_W};
use super::*;

/// A pause cue called `name` at `at` on the fixture's timeline.
fn pause(f: &mut Fixture, name: &str, at: f64) -> CueId {
    let mut p = f.show.make_cue(CueType::Control, None, f.show.cues.len());
    p.parent = Some(f.group);
    p.pre_wait = at;
    p.name = name.into();
    if let CueKind::Control(c) = &mut p.kind {
        c.action = ControlAction::TimelinePause;
    }
    let id = p.id;
    f.show.cues.push(p);
    id
}

/// The fixture made 60 s long, with pauses: two 0.1 s apart (one chip when
/// zoomed out), one just after, two at the same time, and some spread out.
struct Row {
    f: Fixture,
    near: [CueId; 2],
    after: CueId,
    same: [CueId; 2],
    spread: Vec<CueId>,
}

fn row() -> Row {
    let mut f = super::follow::long_fixture();
    let near = [
        pause(&mut f, "Blackout before the storm", 10.0),
        pause(&mut f, "Thunder sound check", 10.1),
    ];
    let after = pause(&mut f, "Lightning flash on the cyc", 10.6);
    let same = [
        pause(&mut f, "Actors take places", 20.0),
        pause(&mut f, "House lights to half", 20.0),
    ];
    let spread = [25.0, 31.0, 35.0, 42.0, 47.0, 52.0]
        .iter()
        .enumerate()
        .map(|(n, t)| {
            pause(
                &mut f,
                &format!("Scene {} change with a long name", n + 2),
                *t,
            )
        })
        .collect();
    Row {
        f,
        near,
        after,
        same,
        spread,
    }
}

fn labels(m: &Mouse) -> Vec<LabelNote> {
    m.ctx
        .data(|d| d.get_temp(egui::Id::new(("timeline_cue_labels", Host::Pane))))
        .unwrap_or_default()
}

fn chips(m: &Mouse) -> Vec<(Vec<CueId>, Rect)> {
    m.ctx
        .data(|d| d.get_temp(egui::Id::new(("timeline_cue_chips", Host::Pane))))
        .unwrap_or_default()
}

fn chip_of(m: &Mouse, id: CueId) -> Rect {
    chips(m)
        .into_iter()
        .find(|(ids, _)| ids.contains(&id))
        .expect("a chip")
        .1
}

fn name_of(show: &Show, id: CueId) -> String {
    show.cue(id).unwrap().name.clone()
}

/// No two names cross, none reaches the next chip, and none is under the least.
fn tidy(m: &Mouse) {
    let labels = labels(m);
    let chips = chips(m);
    for (i, a) in labels.iter().enumerate() {
        if a.whole {
            // A whole name on top: the names it would cover give way.
            for b in labels.iter().filter(|b| !b.whole) {
                assert!(
                    !a.rect.expand2(vec2(3.0, 1.0)).intersects(b.rect),
                    "{:?} shows under {:?}",
                    b.text,
                    a.text
                );
            }
            continue;
        }
        assert!(a.rect.width() >= MIN_LABEL_W - 0.5 || !a.cut);
        for b in labels.iter().skip(i + 1).filter(|b| !b.whole) {
            assert!(
                !a.rect.intersects(b.rect),
                "{:?} and {:?} cross",
                a.text,
                b.text
            );
        }
        let at = chips.iter().find(|(ids, _)| ids.contains(&a.id)).unwrap().1;
        for (_, r) in chips.iter().filter(|(_, r)| r.left() > at.left() + 0.01) {
            assert!(
                a.rect.right() <= r.left(),
                "{:?} runs over the next chip",
                a.text
            );
        }
        // And no chip covers another.
        for (_, r) in chips.iter().filter(|(_, r)| *r != at) {
            assert!(r.left() >= at.right() - 0.01 || r.right() <= at.left() + 0.01);
        }
    }
}

#[test]
fn zoomed_out_the_names_fit_between_the_chips_or_are_left_out() {
    let Row {
        mut f, near, after, ..
    } = row();
    let mut m = Mouse::new();
    // Fit: about 13 points a second, 0.1 s apart is one chip.
    m.frame(&mut f.show, &mut f.st, vec![]);
    tidy(&m);
    let all = labels(&m);
    assert!(!all.is_empty());
    let chip = chips(&m)
        .into_iter()
        .find(|(ids, _)| ids.contains(&near[0]))
        .unwrap();
    assert_eq!(chip.0, near.to_vec(), "the two near ones are one chip");
    // The 0.5 s to the next chip leaves no room for a name.
    assert!(all.iter().all(|l| !near.contains(&l.id)), "{all:?}");
    assert!(all.iter().any(|l| l.cut), "{all:?}");
    // Its tooltip lists both (drawn on hover; the chip is still there).
    assert!(chip_of(&m, after).left() > chip.1.right() - 0.01);
    // Zoomed in, they part and their names come back.
    f.st.timeline.pps = 200.0;
    f.st.timeline.scroll.x = 9.0 * 200.0;
    m.frame(&mut f.show, &mut f.st, vec![]);
    tidy(&m);
    let all = labels(&m);
    assert!(chip_of(&m, near[0]) != chip_of(&m, near[1]));
    assert!(all.iter().any(|l| l.id == near[1]), "{all:?}");
}

#[test]
fn the_hovered_and_the_picked_names_are_drawn_whole() {
    let Row { mut f, spread, .. } = row();
    let mut m = Mouse::new();
    m.frame(&mut f.show, &mut f.st, vec![]);
    let at = chip_of(&m, spread[1]).center();
    m.frame(&mut f.show, &mut f.st, vec![Event::PointerMoved(at)]);
    m.frame(&mut f.show, &mut f.st, vec![]);
    let l = labels(&m);
    tidy(&m);
    let hovered = l.iter().find(|l| l.id == spread[1]).expect("drawn");
    assert!(hovered.whole && !hovered.cut && hovered.text == name_of(&f.show, spread[1]));
    // Picked: the same, with the pointer elsewhere.
    f.st.selected = Some(spread[3]);
    m.frame(
        &mut f.show,
        &mut f.st,
        vec![Event::PointerMoved(pos2(5.0, 5.0))],
    );
    m.frame(&mut f.show, &mut f.st, vec![]);
    let l = labels(&m);
    let picked = l.iter().find(|l| l.id == spread[3]).expect("drawn");
    assert!(picked.whole && !picked.cut && picked.text == name_of(&f.show, spread[3]));
    assert!(!l.iter().any(|l| l.id == spread[1] && l.whole));
    tidy(&m);
}

#[test]
fn cues_close_together_or_at_one_time_are_each_picked_with_the_mouse() {
    let Row {
        mut f,
        near,
        after,
        same,
        ..
    } = row();
    let mut m = Mouse::new();
    m.frame(&mut f.show, &mut f.st, vec![]);
    let pick = |m: &mut Mouse, f: &mut Fixture, r: Rect| -> Vec<CueId> {
        m.click(&mut f.show, &mut f.st, r.center(), PointerButton::Primary)
            .into_iter()
            .filter_map(|a| match a {
                Action::Select(id) => Some(id),
                _ => None,
            })
            .collect()
    };
    // The chip just after the near pair is its own.
    let r = chip_of(&m, after);
    assert_eq!(pick(&mut m, &mut f, r), vec![after]);
    // One chip for two: a click picks the first, the next click the second,
    // then the first again.
    for ids in [near, same] {
        f.st.selected = None;
        let r = chip_of(&m, ids[0]);
        assert_eq!(pick(&mut m, &mut f, r), vec![ids[0]]);
        f.st.selected = Some(ids[0]);
        assert_eq!(pick(&mut m, &mut f, r), vec![ids[1]]);
        f.st.selected = Some(ids[1]);
        assert_eq!(pick(&mut m, &mut f, r), vec![ids[0]]);
    }
    // Zoomed in, the near pair are two chips, each its own click.
    f.st.selected = None;
    f.st.timeline.pps = 200.0;
    f.st.timeline.scroll.x = 9.0 * 200.0;
    m.frame(&mut f.show, &mut f.st, vec![]);
    for id in near {
        let r = chip_of(&m, id);
        assert_eq!(pick(&mut m, &mut f, r), vec![id]);
    }
    // At one time, they stay one chip however near you zoom.
    f.st.timeline.scroll.x = 19.0 * 200.0;
    m.frame(&mut f.show, &mut f.st, vec![]);
    assert_eq!(
        chips(&m)
            .iter()
            .find(|(ids, _)| ids.contains(&same[0]))
            .unwrap()
            .0,
        same.to_vec()
    );
}

#[test]
fn the_rule_narrows_chips_to_the_next_and_groups_those_nearer_than_a_chip() {
    let id = |n| CueId(n);
    let items = [
        (id(1), 100.0, true),
        (id(2), 102.0, true),
        (id(3), 106.0, true),
        (id(4), 200.0, false),
        (id(5), 300.0, true),
    ];
    let plan = tracks::chips(&items, 320.0);
    let get = |n| plan.iter().find(|(i, _)| *i == id(n)).unwrap().1.clone();
    assert_eq!(
        get(1),
        tracks::Chip::Lead {
            members: vec![id(1), id(2)],
            width: 6.0,
            limit: 102.0
        }
    );
    assert_eq!(get(2), tracks::Chip::Under);
    // Its name may reach the clip at 200.
    assert!(matches!(
        get(3),
        tracks::Chip::Lead {
            width: 9.0,
            limit: 196.0,
            ..
        }
    ));
    // The last is stopped by the row's end.
    assert!(matches!(get(5), tracks::Chip::Lead { limit: 318.0, .. }));
    assert!(plan.iter().all(|(i, _)| *i != id(4)));
}

fn chip_tips(m: &Mouse) -> Vec<(Vec<CueId>, String)> {
    m.ctx
        .data(|d| d.get_temp(egui::Id::new(("timeline_chip_tips", Host::Pane))))
        .unwrap_or_default()
}

fn tip_of(m: &Mouse, id: CueId) -> String {
    chip_tips(m)
        .into_iter()
        .find(|(ids, _)| ids.contains(&id))
        .expect("a chip")
        .1
}

#[test]
fn a_chip_of_several_cues_has_a_short_line_each_and_one_cues_chip_the_whole_story() {
    let Row {
        mut f,
        same,
        spread,
        ..
    } = row();
    // Ten more at 56 s: past eight lines, the rest are counted.
    let many: Vec<CueId> = (0..10)
        .map(|n| pause(&mut f, &format!("Cue light {n}"), 56.0))
        .collect();
    let mut m = Mouse::new();
    m.frame(&mut f.show, &mut f.st, vec![]);
    let long = match &f.show.cue(same[0]).unwrap().kind {
        CueKind::Control(c) => c.description(),
        _ => unreachable!(),
    };
    // Two at one time: a line each, number · name · time · what it does.
    let tip = tip_of(&m, same[0]);
    let lines: Vec<&str> = tip.lines().collect();
    assert!(lines[0].starts_with("2 cues here"), "{tip}");
    for id in same {
        let q = f.show.cue(id).unwrap();
        let line = format!(
            "{} · {} · {} · Pause timeline · content runs on",
            q.number,
            q.name,
            crate::ui::fmt_time(20.0)
        );
        assert!(lines.contains(&line.as_str()), "{line:?} in {tip}");
    }
    assert!(!tip.contains(long), "no long description: {tip}");
    assert_eq!(lines.len(), 1 + 1 + 2 + 1 + 1, "{tip}");
    // One cue's chip: the whole description.
    let one = tip_of(&m, spread[0]);
    assert!(one.contains(long) && one.contains("At "), "{one}");
    // Ten: seven lines and "and 3 more".
    let tip = tip_of(&m, many[0]);
    let lines: Vec<&str> = tip.lines().collect();
    assert!(lines[0].starts_with("10 cues here"), "{tip}");
    let cues = lines.iter().filter(|l| l.contains("Cue light")).count();
    assert_eq!(cues, super::tracks::TIP_LINES - 1, "{tip}");
    assert!(lines.contains(&"and 3 more"), "{tip}");
    assert!(lines.len() <= super::tracks::TIP_LINES + 4, "{tip}");
}
