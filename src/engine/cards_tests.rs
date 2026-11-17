//! A follower's engine whose files are on their way (`cards.rs`), with a
//! finder the test controls, and real media.

use super::*;
use crate::engine::LayerSource;
use crate::net::media::{Finder, Local, Transfer};
use crate::net::store::manifest::Facts;
use crate::show::{CueId, CueType, Show};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Stands in for a store: the file is on its way until `arrive`.
struct Fake {
    here: AtomicBool,
    file: PathBuf,
    facts: Option<Facts>,
    generation: AtomicU64,
    /// Times `local` was asked, and times it answered `Here`.
    asked: AtomicU64,
}

impl Fake {
    fn arrive(&self) {
        self.here.store(true, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
}

impl Finder for Fake {
    fn local(&self, _: &Path) -> Local {
        self.asked.fetch_add(1, Ordering::SeqCst);
        if self.here.load(Ordering::SeqCst) {
            Local::Here(self.file.clone())
        } else {
            Local::Transferring(Transfer {
                done: 40,
                total: 100,
                eta_secs: Some(150.0),
                known: true,
            })
        }
    }

    fn facts(&self, _: &Path) -> Option<Facts> {
        self.facts.clone()
    }

    fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }
}

fn tick_until(e: &mut Engine, show: &Show, what: &str, mut cond: impl FnMut(&Engine) -> bool) {
    let t0 = Instant::now();
    let mut last = Instant::now();
    while !cond(e) {
        assert!(
            t0.elapsed() < Duration::from_secs(60),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(5));
        let now = Instant::now();
        e.tick(show, now.duration_since(last).as_secs_f64());
        last = now;
    }
}

/// A follower's engine with a video cue whose file the finder holds back,
/// the cue fired. The show names the file by a path this computer hasn't
/// (the primary's), the finder hands out the real one once it "arrives".
fn follower(
    clip_name: &str,
    ty: CueType,
) -> Option<(Engine, Show, CueId, Arc<Fake>, crate::show::OutputId)> {
    let (ff, clip) = crate::media::test_media(clip_name)?;
    let info = ff.probe(&clip).ok()?;
    let fake = Arc::new(Fake {
        here: AtomicBool::new(false),
        file: clip,
        facts: Some(Facts::of(&info)),
        generation: AtomicU64::new(0),
        asked: AtomicU64::new(0),
    });
    let mut e = Engine::for_tests();
    e.ff = Some(ff);
    e.set_media_finder(Some(fake.clone() as Arc<dyn Finder>));
    let mut show = Show::default();
    let out = show.outputs[0].id;
    let c = show.make_cue(ty, Some(PathBuf::from("/Shows/Primary/media/clip.mp4")), 0);
    let id = c.id;
    show.cues.push(c);
    e.playhead = Some(id);
    e.go(&show);
    Some((e, show, id, fake, out))
}

#[test]
fn a_video_on_its_way_shows_a_card_keeps_its_time_and_takes_the_file_up_where_it_is() {
    let Some((mut e, show, id, fake, out)) = follower("testsrc_10s.mp4", CueType::Video) else {
        return;
    };
    tick_until(&mut e, &show, "the card", |e| {
        !e.layers(out).is_empty() && e.cards().iter().any(|(c, t)| *c == id && t.is_some())
    });
    let card = e.cards()[0].1.clone().unwrap();
    assert_eq!(card.heading, "Media transferring");
    assert_eq!(card.name, "clip.mp4");
    assert_eq!(card.detail, "40 % · about 3 min");
    let a = e.active.iter().find(|a| a.cue_id == id).unwrap();
    // The length the primary read from the file.
    assert!(
        a.duration.is_some_and(|d| (d - 10.0).abs() < 0.2),
        "{:?}",
        a.duration
    );
    assert!(matches!(e.layers(out)[0].source, LayerSource::Image(..)));
    // Its time goes on while the card is up.
    tick_until(&mut e, &show, "two seconds of the card", |e| {
        e.active.iter().any(|a| a.cue_id == id && a.elapsed >= 2.0)
    });
    assert!(e.cards().len() == 1, "still a card");
    fake.arrive();
    tick_until(&mut e, &show, "the video to take over", |e| {
        e.active
            .iter()
            .any(|a| a.cue_id == id && matches!(a.kind, ActiveKind::Video(_)) && !a.seeking())
    });
    assert!(e.cards().is_empty());
    // Where the cue is by now, not from the start of the file.
    tick_until(&mut e, &show, "a frame", |e| e.shown_frame(id).is_some());
    let a = e.active.iter().find(|a| a.cue_id == id).unwrap();
    let shown = e.shown_frame(id).unwrap();
    assert!(a.elapsed > 2.0, "{}", a.elapsed);
    assert!(
        (shown - a.elapsed).abs() < 0.6,
        "frame {shown} for time {}",
        a.elapsed
    );
    assert!(shown > 1.8, "{shown}");
    assert!(matches!(e.layers(out)[0].source, LayerSource::Video(_)));
    // And it ends when it would have.
    tick_until(&mut e, &show, "the end", |e| e.active.is_empty());
}

#[test]
fn a_file_not_found_shows_so_and_a_sound_on_its_way_is_silence_that_keeps_time() {
    struct Gone;
    impl Finder for Gone {
        fn local(&self, _: &Path) -> Local {
            Local::NotFound
        }
        fn facts(&self, _: &Path) -> Option<Facts> {
            None
        }
        fn generation(&self) -> u64 {
            0
        }
    }
    let Some((ff, _)) = crate::media::test_media("testsrc_10s.mp4") else {
        return;
    };
    let mut e = Engine::for_tests();
    e.ff = Some(ff.clone());
    e.set_media_finder(Some(Arc::new(Gone)));
    let mut show = Show::default();
    let out = show.outputs[0].id;
    let c = show.make_cue(CueType::Video, Some(PathBuf::from("C:\\Show\\lost.mov")), 0);
    let id = c.id;
    show.cues.push(c);
    e.playhead = Some(id);
    e.go(&show);
    tick_until(&mut e, &show, "the card", |e| {
        !e.layers(out).is_empty() && e.cards().iter().any(|(_, t)| t.is_some())
    });
    let card = e.cards()[0].1.clone().unwrap();
    assert_eq!(card.heading, "Media not found");
    assert_eq!(card.name, "lost.mov");
    // Black instead, when the show says so.
    show.no_transfer_cards = true;
    e.stop_all(0.0);
    tick_until(&mut e, &show, "the stop", |e| e.active.is_empty());

    // A sound on its way: no layer, its time kept, then its file taken up.
    let Some((mut e, show, id, fake, out)) = follower("tone_5s.wav", CueType::Audio) else {
        return;
    };
    tick_until(&mut e, &show, "a second of silence", |e| {
        e.active.iter().any(|a| a.cue_id == id && a.elapsed >= 1.0)
    });
    assert!(e.layers(out).is_empty());
    let a = e.active.iter().find(|a| a.cue_id == id).unwrap();
    assert!(a.duration.is_some_and(|d| (d - 5.0).abs() < 0.2));
    fake.arrive();
    // (No sound device in tests: the voice can't play, so the silence goes on
    // to the cue's end; what matters is that it ends on time.)
    let t0 = Instant::now();
    tick_until(&mut e, &show, "the end", |e| e.active.is_empty());
    assert!(t0.elapsed() < Duration::from_secs(30));
}

#[test]
fn a_file_is_never_opened_before_the_finder_says_it_is_here() {
    // The finder is asked over and over while the cue plays; nothing but its
    // `Here` answer is ever given to FFmpeg: until then the cue has no decoders.
    let Some((mut e, show, id, fake, _)) = follower("testsrc_10s.mp4", CueType::Video) else {
        return;
    };
    for _ in 0..200 {
        e.tick(&show, 0.01);
        std::thread::sleep(Duration::from_millis(2));
        assert!(
            !e.active
                .iter()
                .any(|a| matches!(a.kind, ActiveKind::Video(_)) || a.seeking()),
            "decoders before the file was here"
        );
        assert!(
            !e.prepared.iter().any(|p| p.cue_id == id),
            "got ready before the file was here"
        );
    }
    assert!(fake.asked.load(Ordering::SeqCst) > 0);
}

#[test]
fn a_file_read_before_joining_the_network_is_asked_again_through_the_store() {
    // Alone, the cue list asked for the file's length (read from the path as
    // the show names it); then this computer joins a network: the cue must
    // not wait for that old answer, it shows its card.
    let Some((ff, clip)) = crate::media::test_media("testsrc_10s.mp4") else {
        return;
    };
    let mut e = Engine::for_tests();
    e.ff = Some(ff.clone());
    let mut show = Show::default();
    let out = show.outputs[0].id;
    let c = show.make_cue(CueType::Video, Some(clip.clone()), 0);
    let id = c.id;
    show.cues.push(c);
    let _ = e.probe_state(&clip);
    let fake = Arc::new(Fake {
        here: AtomicBool::new(false),
        file: clip.clone(),
        facts: Some(Facts::of(&ff.probe(&clip).unwrap())),
        generation: AtomicU64::new(0),
        asked: AtomicU64::new(0),
    });
    e.set_media_finder(Some(fake.clone() as Arc<dyn Finder>));
    e.playhead = Some(id);
    e.go(&show);
    tick_until(&mut e, &show, "the card", |e| {
        !e.layers(out).is_empty() && e.cards().iter().any(|(c, t)| *c == id && t.is_some())
    });
    fake.arrive();
    tick_until(&mut e, &show, "the video", |e| {
        e.active
            .iter()
            .any(|a| matches!(a.kind, ActiveKind::Video(_)) && !a.seeking())
    });
}

/// A carded cue whose length isn't known (no facts from the primary) would
/// hold until stopped; on a follower the primary's heartbeat ends it once
/// the primary's own cue has ended (`engine/net.rs`).
#[test]
fn a_carded_cue_of_unknown_length_ends_when_the_primarys_does() {
    use crate::engine::{NetCorrection, NetMode};
    use crate::net::play::{Command, CueNow, PlayState};
    let Some((ff, clip)) = crate::media::test_media("testsrc_10s.mp4") else {
        return;
    };
    let fake = Arc::new(Fake {
        here: AtomicBool::new(false),
        file: clip,
        facts: None,
        generation: AtomicU64::new(0),
        asked: AtomicU64::new(0),
    });
    let mut e = Engine::for_tests();
    e.ff = Some(ff);
    e.set_media_finder(Some(fake as Arc<dyn Finder>));
    let mut show = Show::default();
    let c = show.make_cue(
        CueType::Video,
        Some(PathBuf::from("/Shows/Primary/media/clip.mp4")),
        0,
    );
    let id = c.id;
    show.cues.push(c);
    e.net_set_mode(NetMode::Follower);
    let mut now = 100.0;
    e.tick_shared(&show, now);
    // The primary's command, done here.
    e.net_apply(&show, &Command::Fire { cue: id }, 0.0);
    let t0 = Instant::now();
    while !e.cards().iter().any(|(c, t)| *c == id && t.is_some()) {
        assert!(t0.elapsed() < Duration::from_secs(60), "no card");
        std::thread::sleep(Duration::from_millis(5));
        now += 0.005;
        e.tick_shared(&show, now);
    }
    let a = e.active.iter().find(|a| a.cue_id == id).unwrap();
    assert_eq!(a.duration, None, "its length isn't known here");
    // While the primary plays it, it stays.
    let mut doubts = NetCorrection::default();
    for _ in 0..3 {
        now += 0.3;
        e.tick_shared(&show, now);
        let s = PlayState {
            time: now,
            cues: vec![CueNow {
                cue: id,
                anchor: 100.0,
                stopping: false,
                sound: None,
            }],
            ..Default::default()
        };
        let fixed = e.net_correct(&show, &s, now, &mut doubts);
        assert!(fixed.is_empty(), "{fixed:?}");
    }
    assert!(e.active.iter().any(|a| a.cue_id == id));
    // The primary's cue has ended: two heartbeats without it end it here.
    for _ in 0..2 {
        now += 0.3;
        e.tick_shared(&show, now);
        let s = PlayState {
            time: now,
            ..Default::default()
        };
        e.net_correct(&show, &s, now, &mut doubts);
    }
    let t0 = Instant::now();
    while e.active.iter().any(|a| a.cue_id == id) {
        assert!(t0.elapsed() < Duration::from_secs(60), "still playing");
        std::thread::sleep(Duration::from_millis(5));
        now += 0.005;
        e.tick_shared(&show, now);
    }
}
