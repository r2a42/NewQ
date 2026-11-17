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
