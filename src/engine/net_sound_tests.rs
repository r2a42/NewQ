//! Two engines ticked by one shared clock, each with a sound card the test
//! drives (`AudioOut::with_manual_device`), playing a sound cue and a film
//! with sound from real media: the first network test on two computers saw a
//! steady 118 ms "apart" while a sound played (0.24.1). The follower's card
//! here starts its sound 120 ms late, as a slower decoder or sound device
//! does.

use super::super::*;
use crate::media::audio::{AudioOut, ManualMix, SOUND_SLACK};
use crate::net::measure::{apart_between, sound_apart_between};
use crate::show::{CueType, Show};
use std::time::{Duration, Instant};

const RATE: f64 = 48_000.0;

struct Side {
    e: Engine,
    card: ManualMix,
    /// The card's callback size, and when it began calling (shared time).
    buffer: usize,
    began: Option<f64>,
}

impl Side {
    fn new(ff: &crate::media::ffmpeg::Ffmpeg, buffer: usize) -> Side {
        let (audio, card) = AudioOut::with_manual_device();
        let mut e = Engine::for_tests();
        e.ff = Some(ff.clone());
        e.audio = audio;
        Side {
            e,
            card,
            buffer,
            began: None,
        }
    }

    /// Its card's callbacks due by shared time `now`.
    fn play_to(&mut self, now: f64) {
        let Some(b) = self.began else { return };
        let due = ((now - b).max(0.0) * RATE) as u64;
        while self.card.frames + self.buffer as u64 <= due {
            self.card.mix(self.buffer);
        }
    }

    fn cue(&self, id: CueId) -> &ActiveCue {
        self.e
            .active
            .iter()
            .find(|a| a.cue_id == id)
            .expect("the cue plays")
    }
}

fn cue(show: &mut Show, ty: CueType, file: &std::path::Path) -> CueId {
    let c = show.make_cue(ty, Some(file.to_path_buf()), show.cues.len());
    let id = c.id;
    show.cues.push(c);
    id
}

#[test]
fn a_sound_that_starts_late_on_one_computer_catches_up_and_the_pictures_never_differ() {
    let Some((ff, tone)) = crate::media::test_media("tone_5s.wav") else {
        return;
    };
    let Some((_, film)) = crate::media::test_media("testsrc_10s.mp4") else {
        return;
    };
    let mut show = Show::default();
    let sound = cue(&mut show, CueType::Audio, &tone);
    let video = cue(&mut show, CueType::Video, &film);
    // The primary's card: 512-frame buffers, as the Mac's were; the
    // follower's: 480, and it starts 120 ms after the cues do.
    let mut p = Side::new(&ff, 512);
    let mut f = Side::new(&ff, 480);
    p.began = Some(0.0);
    let t0 = Instant::now();
    let shared = || t0.elapsed().as_secs_f64();
    for s in [&mut p, &mut f] {
        s.e.tick_shared(&show, 0.0);
        s.e.load_ahead(&show, sound);
        s.e.load_ahead(&show, video);
    }
    // Got ready (a command's lead allows for it); then both fire at one
    // shared time.
    while !(p.e.prepared.len() >= 2 && f.e.prepared.len() >= 2) {
        assert!(shared() < 30.0, "the cues were never got ready");
        std::thread::sleep(Duration::from_millis(10));
        let now = shared();
        for s in [&mut p, &mut f] {
            s.play_to(now);
            s.e.tick_shared(&show, now);
        }
    }
    let fire_at = shared() + 0.3;
    let late_by = 0.12;
    let mut fired = false;
    let mut checked = 0;
    let mut doubts = net::NetCorrection::default();
    loop {
        std::thread::sleep(Duration::from_millis(4));
        let now = shared();
        if !fired && now >= fire_at {
            for s in [&mut p, &mut f] {
                s.e.tick_shared(&show, fire_at);
                s.e.fire(&show, sound);
                s.e.fire(&show, video);
            }
            fired = true;
        }
        if fired && f.began.is_none() && now >= fire_at + late_by {
            f.began = Some(now);
        }
        for s in [&mut p, &mut f] {
            s.play_to(now);
            s.e.tick_shared(&show, now);
        }
        if !fired || now < fire_at + 1.5 {
            assert!(now < fire_at + 30.0, "the cues never started");
            continue;
        }
        // Both have played a second and more.
        let (ps, fs) = (p.e.net_state(now), f.e.net_state(now));
        let (apart, compared, missing) = apart_between(&ps, &fs);
        assert_eq!(
            (compared, missing),
            (2, 0),
            "{ps:?} {fs:?} {:?} {:?}",
            p.e.take_problems(),
            f.e.take_problems()
        );
        assert!(
            apart.unwrap() < 1.0,
            "the cues' times are the shared clock's on both: {apart:?} ms apart"
        );
        // The pictures the two players are asked for are the same, and are
        // the clock's time since the cue began, not either sound card's.
        let pic = |s: &Side| match &s.cue(video).kind {
            ActiveKind::Video(v) => v.picture_at,
            _ => panic!("a film"),
        };
        let since = now - fire_at;
        assert!((pic(&p) - pic(&f)).abs() < 1e-6, "{} {}", pic(&p), pic(&f));
        assert!((pic(&f) - since).abs() < 0.02, "{} vs {since}", pic(&f));
        // The follower's sound was 120 ms behind and skipped ahead: what its
        // card was sent is 120 ms more than it has had time to play (without
        // that skip, the old report put the sound cue 120 ms "apart"). The
        // primary's never skipped.
        let reached = |s: &Side, id: CueId| match &s.cue(id).kind {
            ActiveKind::Audio(vo) => vo.reached(),
            ActiveKind::Video(v) => v.voice.as_ref().expect("its sound").reached(),
            _ => panic!("a sound"),
        };
        let skipped = reached(&f, sound) - f.card.frames as f64 / RATE;
        // (It skips each time it is more than `SOUND_SLACK` behind, so up to
        // that is left when its card begins. How late the card really began
        // is measured: a busy machine's loop wakes later than asked.)
        let late = f.began.expect("its card began") - fire_at;
        assert!(late >= late_by, "{late}");
        assert!(
            skipped > late - SOUND_SLACK - 0.005 && skipped < late + 0.02,
            "the follower's sound skipped {skipped:.3} s, its card {late:.3} s late"
        );
        let p_skipped = reached(&p, sound) - (p.card.frames as f64 / RATE - fire_at);
        assert!(
            p_skipped.abs() < 0.02,
            "the primary's skipped {p_skipped:.3}"
        );
        // So the sounds are as close as a sound can be kept, film and cue.
        let sounds = sound_apart_between(&ps, &fs).expect("sounds on both");
        assert!(
            sounds < (SOUND_SLACK + 0.011) * 1000.0,
            "sounds {sounds:.1} ms apart"
        );
        for id in [sound, video] {
            let s = f.cue(id);
            let behind = (now - s.anchor.unwrap()) - (now - s.sound_zero.unwrap());
            assert!(behind.abs() < SOUND_SLACK + 0.011, "{behind}");
        }
        // And nothing for the follower to put right.
        let fixed = f.e.net_correct(&show, &ps, now, &mut doubts);
        assert!(
            fixed.is_empty() && doubts.is_empty(),
            "{fixed:?} {doubts:?}"
        );
        checked += 1;
        if checked == 3 {
            break;
        }
    }
}

/// A chain of real media, each cue auto-following the last: a film with
/// sound, a sound, a timeline group of a film and a sound, a film without
/// sound, a sound with a pre-wait, then a long wait. The primary
/// ticks at 60 Hz and the follower at 165 (the 0.24.3 test's Mac and Windows
/// laptop); both do GO at one shared time, as a command makes them, and
/// every cue after it is started by each engine itself as the one before
/// ends. Each must begin at the same shared time on both (where the one
/// before ended by the clock), not at the frame each noticed the end in:
/// those differed by up to a 60 Hz frame at each link and added up (issue
/// 23 of the 0.24.3 test: cues 26 ms off, each "put right").
#[test]
fn a_chain_of_films_and_sounds_starts_each_cue_together_at_any_frame_rate() {
    use crate::show::{ContinueMode, CueKind, GroupMode};
    let Some((ff, tone)) = crate::media::test_media("tone_5s.wav") else {
        return;
    };
    let Some((_, film)) = crate::media::test_media("testsrc_10s.mp4") else {
        return;
    };
    let Some((_, smpte)) = crate::media::test_media("smpte_8s.mp4") else {
        return;
    };
    let mut show = Show::default();
    let mut ids = Vec::new();
    // (type, file, end, pre-wait, in the timeline group)
    let mut timeline = None;
    for (ty, file, end, pre_wait, clip) in [
        (CueType::Video, &film, 1.507, 0.0, false),
        (CueType::Audio, &tone, 1.213, 0.0, false),
        (CueType::Video, &smpte, 1.109, 0.0, true),
        (CueType::Audio, &tone, 0.917, 0.419, true),
        (CueType::Video, &smpte, 1.371, 0.0, false),
        (CueType::Audio, &tone, 0.917, 0.253, false),
    ] {
        if clip && timeline.is_none() {
            let mut g = show.make_cue(CueType::Group, None, show.cues.len());
            if let CueKind::Group(gc) = &mut g.kind {
                gc.mode = GroupMode::Timeline;
            }
            g.continue_mode = ContinueMode::AutoFollow;
            ids.push(g.id);
            timeline = Some(g.id);
            show.cues.push(g);
        }
        let id = cue(&mut show, ty, file);
        let c = show.cues.last_mut().expect("a cue");
        c.kind.media_mut().expect("media").end = Some(end);
        c.pre_wait = pre_wait;
        if clip {
            c.parent = timeline;
        } else {
            c.continue_mode = ContinueMode::AutoFollow;
            ids.push(id);
        }
    }
    let mut w = show.make_cue(CueType::Wait, None, show.cues.len());
    if let CueKind::Wait(x) = &mut w.kind {
        x.duration = 1000.0;
    }
    ids.push(w.id);
    show.cues.push(w);
    let mut p = Side::new(&ff, 512);
    let mut f = Side::new(&ff, 480);
    let t0 = Instant::now();
    let shared = || t0.elapsed().as_secs_f64();
    p.began = Some(0.0);
    f.began = Some(0.0);
    for s in [&mut p, &mut f] {
        s.e.tick_shared(&show, 0.0);
        s.e.load_ahead(&show, ids[0]);
    }
    while p.e.prepared.is_empty() || f.e.prepared.is_empty() {
        assert!(shared() < 30.0, "the first cue was never got ready");
        std::thread::sleep(Duration::from_millis(10));
        let now = shared();
        for s in [&mut p, &mut f] {
            s.play_to(now);
            s.e.tick_shared(&show, now);
        }
    }
    let fire_at = shared() + 0.2;
    // (side, its frame rate, its next tick, whether it has done the GO)
    let mut sides = [
        (p, 60.0, fire_at, false),
        (f, 165.0, fire_at + 0.0013, false),
    ];
    // Each cue's anchor on each side, as first seen (the timeline group's
    // clock's after the cues').
    let mut anchors: Vec<[Option<f64>; 2]> = vec![[None, None]; ids.len() + 1];
    while anchors.iter().any(|a| a.iter().any(Option::is_none)) {
        std::thread::sleep(Duration::from_millis(1));
        let now = shared();
        assert!(now < fire_at + 60.0, "the chain never reached its end");
        for (k, (s, hz, next, went)) in sides.iter_mut().enumerate() {
            if now < *next {
                continue;
            }
            // Its own rate, and a late wake-up as a real loop has.
            *next = (*next + 1.0 / *hz).max(now);
            s.play_to(now);
            if !*went {
                s.e.tick_shared(&show, fire_at);
                s.e.fire(&show, ids[0]);
                *went = true;
            }
            s.e.tick_shared(&show, now);
            let state = s.e.net_state(now);
            for (i, id) in ids.iter().enumerate() {
                if anchors[i][k].is_none()
                    && let Some(c) = state.cues.iter().find(|c| c.cue == *id)
                {
                    anchors[i][k] = Some(c.anchor);
                }
            }
            let key = crate::timeline::TimelineKey::Group(timeline.expect("a timeline"));
            if anchors[ids.len()][k].is_none()
                && let Some(r) = state.runs.iter().find(|r| r.key == key)
            {
                anchors[ids.len()][k] = r.anchor;
            }
        }
    }
    let mut worst: f64 = 0.0;
    for (i, a) in anchors.iter().enumerate() {
        let (pa, fa) = (a[0].unwrap_or(0.0), a[1].unwrap_or(0.0));
        eprintln!(
            "cue {i}: primary {:.4} s, follower {:.4} s, {:+.1} ms apart",
            pa - fire_at,
            fa - fire_at,
            (fa - pa) * 1000.0
        );
        worst = worst.max((fa - pa).abs());
    }
    let [(p, ..), (f, ..)] = &mut sides;
    assert!(
        worst < 0.002,
        "{:.1} ms apart; {:?} {:?}",
        worst * 1000.0,
        p.e.take_problems(),
        f.e.take_problems()
    );
}

/// A cue that a sound and a film with sound follow is seen to end 100 ms
/// past its length (a stall, or a file that ran on) on both computers, one
/// at 60 Hz and one at 165: what follows starts at the tick on each, from its
/// beginning, as 0.24.2 did. Placed back where the cue ended, the sounds
/// would skip all but `SOUND_SLACK` of those 100 ms and the film's first
/// frames would never be shown.
#[test]
fn what_follows_a_cue_seen_100_ms_past_its_end_starts_from_its_beginning() {
    use crate::show::{ContinueMode, CueKind};
    let Some((ff, tone)) = crate::media::test_media("tone_5s.wav") else {
        return;
    };
    let Some((_, film)) = crate::media::test_media("testsrc_10s.mp4") else {
        return;
    };
    let mut show = Show::default();
    let mut w = show.make_cue(CueType::Wait, None, show.cues.len());
    if let CueKind::Wait(x) = &mut w.kind {
        x.duration = 0.6;
    }
    w.continue_mode = ContinueMode::AutoFollow;
    let first = w.id;
    show.cues.push(w);
    let video = cue(&mut show, CueType::Video, &film);
    show.cues.last_mut().expect("a cue").continue_mode = ContinueMode::AutoContinue;
    let sound = cue(&mut show, CueType::Audio, &tone);
    let mut p = Side::new(&ff, 512);
    let mut f = Side::new(&ff, 480);
    let t0 = Instant::now();
    let shared = || t0.elapsed().as_secs_f64();
    p.began = Some(0.0);
    f.began = Some(0.0);
    for s in [&mut p, &mut f] {
        s.e.tick_shared(&show, 0.0);
        s.e.load_ahead(&show, video);
        s.e.load_ahead(&show, sound);
    }
    while !(p.e.prepared.len() >= 2 && f.e.prepared.len() >= 2) {
        assert!(shared() < 30.0, "the cues were never got ready");
        std::thread::sleep(Duration::from_millis(10));
        let now = shared();
        for s in [&mut p, &mut f] {
            s.play_to(now);
            s.e.tick_shared(&show, now);
        }
    }
    let fire_at = shared() + 0.2;
    let end = fire_at + 0.6;
    let seen = end + 0.1;
    let mut sides = [(p, 60.0, fire_at, false), (f, 165.0, fire_at, false)];
    // The tick at which each side saw the end.
    let mut saw: [Option<f64>; 2] = [None, None];
    loop {
        std::thread::sleep(Duration::from_millis(1));
        let now = shared();
        assert!(now < fire_at + 30.0, "what follows never started");
        if now >= seen + 0.8 {
            break;
        }
        for (k, (s, hz, next, went)) in sides.iter_mut().enumerate() {
            if now < *next {
                continue;
            }
            *next = (*next + 1.0 / *hz).max(now);
            // Nothing between a frame before its end and 100 ms past it.
            if now > end - 0.005 && now < seen {
                *next = seen;
                continue;
            }
            s.play_to(now);
            if !*went {
                s.e.tick_shared(&show, fire_at);
                s.e.fire(&show, first);
                *went = true;
            }
            s.e.tick_shared(&show, now);
            if saw[k].is_none() && s.e.active.iter().any(|a| a.cue_id == video) {
                saw[k] = Some(now);
            }
        }
    }
    let now = shared();
    for (k, (s, ..)) in sides.iter_mut().enumerate() {
        let saw = saw[k].expect("what follows began");
        assert!(saw >= seen, "{}", saw - seen);
        let state = s.e.net_state(now);
        for id in [video, sound] {
            let at = state
                .cues
                .iter()
                .find(|c| c.cue == id)
                .map(|c| c.anchor)
                .expect("it plays");
            assert!(
                at >= saw - 0.02,
                "side {k}: placed {:.0} ms before the tick that saw the end",
                (saw - at) * 1000.0
            );
            // Its sound has played from its beginning: no more than the card
            // has had time to play since it began.
            let reached = match &s.cue(id).kind {
                ActiveKind::Audio(vo) => vo.reached(),
                ActiveKind::Video(v) => v.voice.as_ref().expect("its sound").reached(),
                _ => panic!("a sound"),
            };
            let skipped = reached - (s.card.frames as f64 / RATE - at);
            assert!(
                skipped.abs() < 0.02,
                "side {k}: the sound skipped {:.0} ms; {:?}",
                skipped * 1000.0,
                s.e.take_problems()
            );
        }
        let pic = match &s.cue(video).kind {
            ActiveKind::Video(v) => v.picture_at,
            _ => panic!("a film"),
        };
        assert!(pic < now - saw + 0.02, "the film is at {pic:.3} s");
    }
}

/// A sound set to play twice, ticking by a shared clock: what follows it
/// starts after the second pass, where the clock takes it past its whole
/// length (`over_by_clock`), not after the first.
#[test]
fn what_follows_a_sound_played_twice_starts_after_both_passes() {
    use crate::show::{ContinueMode, CueKind};
    let Some((ff, tone)) = crate::media::test_media("tone_5s.wav") else {
        return;
    };
    let mut show = Show::default();
    let sound = cue(&mut show, CueType::Audio, &tone);
    {
        let c = show.cues.last_mut().expect("a cue");
        let m = c.kind.media_mut().expect("media");
        m.end = Some(0.4);
        m.loop_media = true;
        m.loop_count = 2;
        c.continue_mode = ContinueMode::AutoFollow;
    }
    let mut w = show.make_cue(CueType::Wait, None, show.cues.len());
    if let CueKind::Wait(x) = &mut w.kind {
        x.duration = 1000.0;
    }
    let after = w.id;
    show.cues.push(w);
    let mut s = Side::new(&ff, 512);
    let t0 = Instant::now();
    let shared = || t0.elapsed().as_secs_f64();
    s.began = Some(0.0);
    s.e.tick_shared(&show, 0.0);
    s.e.load_ahead(&show, sound);
    while s.e.prepared.is_empty() {
        assert!(shared() < 30.0, "never got ready");
        std::thread::sleep(Duration::from_millis(10));
        let now = shared();
        s.play_to(now);
        s.e.tick_shared(&show, now);
    }
    let fire_at = shared() + 0.1;
    let mut fired = false;
    let at = loop {
        std::thread::sleep(Duration::from_millis(4));
        let now = shared();
        assert!(now < fire_at + 20.0, "what follows never started");
        s.play_to(now);
        if !fired && now >= fire_at {
            s.e.tick_shared(&show, fire_at);
            s.e.fire(&show, sound);
            fired = true;
        }
        s.e.tick_shared(&show, now);
        let state = s.e.net_state(now);
        if let Some(c) = state.cues.iter().find(|c| c.cue == after) {
            break c.anchor;
        }
    };
    assert!(
        (at - fire_at - 0.8).abs() < 0.03,
        "what follows began {:.3} s after the sound",
        at - fire_at
    );
}
