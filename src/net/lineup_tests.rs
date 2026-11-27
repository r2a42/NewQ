//! `net::lineup`, pure, on a clock the test owns (an `Instant` taken once and
//! moved on by hand).

use super::*;
use crate::net::proto::{self, Msg};

const D: OutputId = OutputId(7);
const MONITOR: (u32, u32) = (1920, 1080);

fn mine(id: OutputId) -> Option<(u32, u32)> {
    (id == D).then_some(MONITOR)
}

fn overlay(session: u64, seq: u64, x: f32) -> Overlay {
    Overlay {
        session,
        seq,
        display: D,
        pixels: MONITOR,
        marks: vec![
            Mark {
                n: 1,
                at: [x, 100.0],
                on: true,
            },
            Mark {
                n: 2,
                at: [1800.0, 900.0],
                on: false,
            },
        ],
        active: Some(0),
        outline: None,
        under: Under::Black,
    }
}

fn show(o: Overlay) -> LineUpMsg {
    LineUpMsg::Show(Box::new(o))
}

const P: Source = Source::Primary(NodeId(1));

#[test]
fn crosses_are_held_for_the_lease_and_go_by_themselves() {
    let t0 = Instant::now();
    let mut s = Shown::default();
    let seen = s.take(P, show(overlay(9, 1, 10.0)), mine, t0);
    assert_eq!(
        seen,
        Some(LineUpMsg::Seen {
            session: 9,
            seq: 1,
            pixels: MONITOR
        })
    );
    assert_eq!(s.overlay().unwrap().marks[0].at, [10.0, 100.0]);
    assert_eq!(s.source(), Some(P));
    // Within the lease it stays; a newer `Show` moves it and renews it.
    assert!(!s.tick(t0 + Duration::from_millis(4_900)));
    s.take(
        P,
        show(overlay(9, 2, 20.0)),
        mine,
        t0 + Duration::from_secs(4),
    );
    assert!(!s.tick(t0 + Duration::from_millis(8_900)));
    assert_eq!(s.overlay().unwrap().marks[0].at[0], 20.0);
    // Five seconds after the last: gone.
    assert!(s.tick(t0 + Duration::from_millis(9_100)));
    assert!(s.overlay().is_none() && s.stamp().is_none());
    // A clock that seems to go back never panics nor drops it.
    s.take(
        P,
        show(overlay(9, 3, 1.0)),
        mine,
        t0 + Duration::from_secs(20),
    );
    assert!(!s.tick(t0));
}

#[test]
fn a_late_message_another_display_and_end_are_judged() {
    let t0 = Instant::now();
    let mut s = Shown::default();
    s.take(P, show(overlay(9, 5, 50.0)), mine, t0);
    // An older count of the same line-up: passed by, nothing sent back.
    assert_eq!(s.take(P, show(overlay(9, 4, 40.0)), mine, t0), None);
    assert_eq!(s.overlay().unwrap().seq, 5);
    // Not a display of this computer's: passed by.
    let mut other = overlay(9, 6, 60.0);
    other.display = OutputId(99);
    assert_eq!(s.take(P, show(other), mine, t0), None);
    assert_eq!(s.overlay().unwrap().seq, 5);
    // `End` of another line-up, or from elsewhere: nothing.
    s.take(P, LineUpMsg::End { session: 8 }, mine, t0);
    s.take(Source::Here, LineUpMsg::End { session: 9 }, mine, t0);
    assert!(s.overlay().is_some());
    // A new line-up replaces it, whatever its count.
    s.take(P, show(overlay(10, 1, 70.0)), mine, t0);
    assert_eq!(s.overlay().unwrap().session, 10);
    // Its `End`: gone at once.
    s.take(P, LineUpMsg::End { session: 10 }, mine, t0);
    assert!(s.overlay().is_none());
    // A `Seen` or a later kind does nothing here.
    assert_eq!(s.take(P, LineUpMsg::Unknown, mine, t0), None);
}

#[test]
fn numbers_that_cant_be_drawn_are_left_out() {
    let t0 = Instant::now();
    let mut s = Shown::default();
    let mut o = overlay(1, 1, f32::NAN);
    o.active = Some(0);
    o.marks.extend((0..500).map(|i| Mark {
        n: i,
        at: [1.0, 1.0],
        on: true,
    }));
    o.outline = Some(vec![
        vec![[0.0, 0.0], [f32::INFINITY, 1.0]],
        vec![[0.0, 0.0]; 40_000],
    ]);
    s.take(P, show(o), mine, t0);
    let held = s.overlay().unwrap();
    assert_eq!(held.marks.len(), MAX_MARKS);
    assert!(held.marks.iter().all(|m| m.at[0].is_finite()));
    let (outline, _) = s.outline().unwrap();
    assert_eq!(outline.len(), 1, "the piece left with one point is dropped");
    assert_eq!(outline[0].len(), MAX_OUTLINE_POINTS);
    // A picture with no size can't be placed in: nothing is shown.
    let mut none = Shown::default();
    let mut z = overlay(2, 1, 1.0);
    z.pixels = (0, 1080);
    assert_eq!(none.take(P, show(z), mine, t0), None);
    let mut huge = overlay(2, 2, 1.0);
    huge.pixels = (u32::MAX, 1);
    assert_eq!(none.take(P, show(huge), mine, t0), None);
    assert!(none.overlay().is_none());
    // An active mark that isn't there is none.
    let mut a = overlay(3, 1, 1.0);
    a.active = Some(40);
    none.take(P, show(a), mine, t0);
    assert_eq!(none.overlay().unwrap().active, None);
}

#[test]
fn the_outline_is_kept_through_a_session_and_counted() {
    let t0 = Instant::now();
    let mut s = Shown::default();
    let mut o = overlay(1, 1, 1.0);
    o.outline = Some(vec![vec![[0.0, 0.0], [10.0, 10.0]]]);
    s.take(P, show(o), mine, t0);
    let (_, rev) = s.outline().unwrap();
    // A `Show` without one keeps the last.
    s.take(P, show(overlay(1, 2, 2.0)), mine, t0);
    assert_eq!(s.outline().unwrap().1, rev);
    assert_eq!(s.stamp(), Some((1, 2, rev)));
    // A new line-up without one has none.
    s.take(P, show(overlay(2, 1, 2.0)), mine, t0);
    assert!(s.outline().is_none());
    assert!(s.stamp().unwrap().2 > rev, "counted, so the picture goes");
}

#[test]
fn crosses_from_a_primary_not_followed_now_go_and_this_computers_stay() {
    let t0 = Instant::now();
    let mut s = Shown::default();
    s.take(P, show(overlay(1, 1, 1.0)), mine, t0);
    assert!(!s.keep_only(Some(NodeId(1))));
    assert!(s.keep_only(Some(NodeId(2))), "another primary");
    s.take(P, show(overlay(1, 2, 1.0)), mine, t0);
    assert!(s.keep_only(None), "left the network");
    s.take(Source::Here, show(overlay(3, 1, 1.0)), mine, t0);
    assert!(!s.keep_only(None), "its own line-up is kept");
    assert!(s.overlay().is_some());
    assert!(s.clear());
    assert!(!s.clear());
}

#[test]
fn the_trust_rule() {
    let m = show(overlay(1, 1, 1.0));
    let end = LineUpMsg::End { session: 1 };
    let seen = LineUpMsg::Seen {
        session: 1,
        seq: 1,
        pixels: MONITOR,
    };
    // Only from the primary this computer follows.
    assert_eq!(route(&m, true, false), Route::Shown);
    assert_eq!(route(&end, true, false), Route::Shown);
    assert_eq!(route(&m, false, false), Route::Pass);
    assert_eq!(route(&end, false, true), Route::Pass);
    // A primary never takes crosses (it has no primary of its own).
    assert_eq!(route(&m, false, true), Route::Pass);
    // `Seen` only on a primary.
    assert_eq!(route(&seen, false, true), Route::Sender);
    assert_eq!(route(&seen, true, false), Route::Pass);
    assert_eq!(route(&LineUpMsg::Unknown, true, true), Route::Pass);
}

fn want(x: f32) -> Overlay {
    overlay(0, 0, x)
}

#[test]
fn the_sender_sends_a_change_at_once_and_repeats_once_a_second() {
    let t0 = Instant::now();
    let ms = |n| t0 + Duration::from_millis(n);
    let mut s = Sender::new(77, D, Target::Computer(NodeId(5)));
    let Some(LineUpMsg::Show(first)) = s.due(&want(1.0), None, 0, t0) else {
        panic!("the first is sent");
    };
    assert_eq!((first.session, first.seq, first.display), (77, 1, D));
    assert_eq!(s.due(&want(1.0), None, 0, ms(500)), None, "nothing new");
    // A nudge: at once.
    let Some(LineUpMsg::Show(nudge)) = s.due(&want(2.0), None, 0, ms(510)) else {
        panic!("a nudge is sent at once");
    };
    assert_eq!((nudge.seq, nudge.marks[0].at[0]), (2, 2.0));
    // Nothing changing: once a second.
    assert_eq!(s.due(&want(2.0), None, 0, ms(1_400)), None);
    assert!(s.due(&want(2.0), None, 0, ms(1_510)).is_some());
    assert_eq!(s.end(), LineUpMsg::End { session: 77 });
}

#[test]
fn the_outline_goes_at_most_five_times_a_second_and_with_each_repeat() {
    let t0 = Instant::now();
    let ms = |n| t0 + Duration::from_millis(n);
    let line = |x: f32| vec![vec![[0.0, 0.0], [x, x]]];
    let mut s = Sender::new(1, D, Target::Here);
    let outline_of = |m: Option<LineUpMsg>| match m {
        Some(LineUpMsg::Show(o)) => Some(o.outline.is_some()),
        _ => None,
    };
    assert_eq!(
        outline_of(s.due(&want(1.0), Some(&line(1.0)), 1, t0)),
        Some(true)
    );
    // A new outline 50 ms later waits; a nudge then goes without it.
    assert_eq!(
        outline_of(s.due(&want(1.0), Some(&line(2.0)), 2, ms(50))),
        None
    );
    assert_eq!(
        outline_of(s.due(&want(3.0), Some(&line(2.0)), 2, ms(60))),
        Some(false)
    );
    // Its time come, it goes by itself.
    assert_eq!(
        outline_of(s.due(&want(3.0), Some(&line(2.0)), 2, ms(210))),
        Some(true)
    );
    assert_eq!(
        outline_of(s.due(&want(3.0), Some(&line(2.0)), 2, ms(300))),
        None
    );
    // The once-a-second repeat is the whole state, outline too.
    assert_eq!(
        outline_of(s.due(&want(3.0), Some(&line(2.0)), 2, ms(1_300))),
        Some(true)
    );
}

#[test]
fn the_link_lost_older_and_seen_in_words() {
    let t0 = Instant::now();
    let secs = |n| t0 + Duration::from_secs(n);
    let n = NodeId(5);
    let mut s = Sender::new(3, D, Target::Computer(n));
    assert_eq!(s.link(true, t0), Link::Waiting);
    s.due(&want(1.0), None, 0, t0);
    assert_eq!(s.link(true, secs(2)), Link::Waiting);
    assert_eq!(s.link(false, secs(2)), Link::Lost);
    // Connected and nothing back: an older NewQ.
    assert_eq!(s.link(true, secs(4)), Link::Older);
    let older = link_words(Link::Older, "Stage-left", "0.27.2", MONITOR).unwrap();
    assert!(older.contains("Stage-left runs NewQ 0.27.2"), "{older}");
    let lost = link_words(Link::Lost, "Stage-left", "0.28.0", MONITOR).unwrap();
    assert!(lost.contains("Lost the link to Stage-left"), "{lost}");
    assert!(lost.contains("Your points are kept"), "{lost}");
    // A `Seen` from another computer, or for another line-up: passed by.
    let seen = |session, pixels| LineUpMsg::Seen {
        session,
        seq: 1,
        pixels,
    };
    assert!(!s.heard(Target::Computer(NodeId(6)), &seen(3, MONITOR), secs(4)));
    assert!(!s.heard(Target::Computer(n), &seen(4, MONITOR), secs(4)));
    assert!(!s.heard(Target::Here, &seen(3, MONITOR), secs(4)));
    // From it, for this one: seen, on a monitor of that size.
    assert!(s.heard(Target::Computer(n), &seen(3, (1280, 720)), secs(4)));
    assert_eq!(
        s.link(true, secs(9)),
        Link::Seen {
            pixels: (1280, 720)
        }
    );
    let size = link_words(
        Link::Seen {
            pixels: (1280, 720),
        },
        "Stage-left",
        "",
        MONITOR,
    )
    .unwrap();
    assert!(
        size.contains("1280 × 720") && size.contains("1920 × 1080"),
        "{size}"
    );
    assert_eq!(
        link_words(Link::Seen { pixels: MONITOR }, "x", "", MONITOR),
        None
    );
    // A count never sent is no `Seen`.
    assert!(!s.heard(
        Target::Computer(n),
        &LineUpMsg::Seen {
            session: 3,
            seq: 99,
            pixels: MONITOR
        },
        secs(9)
    ));
}

/// One path: the primary's display on itself. The sender's messages go
/// straight into this computer's `Shown` and its `Seen` straight back.
#[test]
fn alone_the_sender_and_the_crosses_are_one_loop() {
    let t0 = Instant::now();
    let mut s = Sender::new(4, D, Target::Here);
    let mut shown = Shown::default();
    let pass = |x: f32, at: Instant, s: &mut Sender, shown: &mut Shown| {
        if let Some(m) = s.due(&want(x), None, 0, at)
            && let Some(seen) = shown.take(Source::Here, m, mine, at)
        {
            assert!(s.heard(Target::Here, &seen, at));
        }
        shown.tick(at);
    };
    pass(5.0, t0, &mut s, &mut shown);
    assert_eq!(s.link(true, t0), Link::Seen { pixels: MONITOR });
    pass(6.0, t0 + Duration::from_millis(16), &mut s, &mut shown);
    assert_eq!(shown.overlay().unwrap().marks[0].at[0], 6.0);
    // The repeat keeps the lease: a minute of nothing changing, still up.
    for k in 1..=60 {
        pass(6.0, t0 + Duration::from_secs(k), &mut s, &mut shown);
    }
    assert!(shown.overlay().is_some());
    // *Done*: `End`, gone.
    shown.take(Source::Here, s.end(), mine, t0 + Duration::from_secs(61));
    assert!(shown.overlay().is_none());
}

#[test]
fn on_the_wire_a_later_newqs_kinds_and_missing_fields_are_read() {
    let m = Msg::LineUp(Box::new(show(overlay(1, 2, 3.0))));
    let bytes = serde_json::to_vec(&m).unwrap();
    let json = String::from_utf8(bytes.clone()).unwrap();
    assert!(
        json.contains(r#""t":"line_up""#) && json.contains(r#""k":"show""#),
        "{json}"
    );
    assert_eq!(proto::decode(&bytes).unwrap(), m);
    // A later kind inside it, a later field, a later `under`; fields left out.
    let read = |j: &str| proto::decode(j.as_bytes()).unwrap();
    assert_eq!(
        read(r#"{"t":"line_up","k":"something_new","x":1}"#),
        Msg::LineUp(Box::new(LineUpMsg::Unknown))
    );
    let Msg::LineUp(m) = read(r#"{"t":"line_up","k":"show","under":"glowing","colour":3}"#) else {
        panic!("read as a line-up");
    };
    let LineUpMsg::Show(o) = *m else {
        panic!("a show")
    };
    assert_eq!((o.under, o.seq, o.marks.len()), (Under::Unknown, 0, 0));
    assert_eq!(
        read(r#"{"t":"line_up","k":"seen"}"#),
        Msg::LineUp(Box::new(LineUpMsg::Seen {
            session: 0,
            seq: 0,
            pixels: (0, 0)
        }))
    );
    assert_eq!(read(r#"{"t":"something_newer"}"#), Msg::Unknown);
}
