//! The three test shows. Each is built from the user's own show (a copy with
//! its cues taken out), so a test plays through the same displays, audio
//! outputs and settings the real show uses, and from the media that
//! `assets` made ready.
//!
//! A lap is a run of top-level cues that each hand over to the next, so the
//! show plays itself: a title, clips at three frame rates, a slide show of
//! large pictures, layers moving over one another, a 5.1 trailer mixed down
//! with a crosspoint fade, a stereo trailer through sound effects, and (the
//! long test) a burst of quick cues and more codecs. The Medium and Long
//! laps end by starting the first cue again, for as long as the test runs;
//! the Short test is a single lap.

use super::Level;
use super::assets::{Kind, Resolved, find};
use crate::show::*;
use std::collections::HashMap;

pub struct Built {
    pub show: Show,
    /// The first cue of a lap: when it starts, a lap has begun.
    pub first: CueId,
    /// Whether the laps go on (the last cue starts the first again).
    pub repeats: bool,
    /// How long a lap should take if nothing is late, in seconds.
    pub lap_secs: f64,
    /// Cues in the show.
    pub cues: usize,
    /// What was left out and why (an asset that could not be got).
    pub notes: Vec<String>,
    /// The ids of the media the show asks for, found or not.
    pub used: Vec<&'static str>,
}

struct Builder<'a> {
    show: Show,
    res: &'a HashMap<&'static str, Resolved>,
    audio: Option<AudioOutputId>,
    notes: Vec<String>,
    used: Vec<&'static str>,
}

fn follow(c: &mut Cue) {
    c.continue_mode = ContinueMode::AutoFollow;
}

fn media(c: &mut Cue) -> &mut MediaCue {
    c.kind.media_mut().expect("a cue that plays something")
}

impl<'a> Builder<'a> {
    fn cue(
        &mut self,
        ty: CueType,
        file: Option<std::path::PathBuf>,
        name: &str,
        parent: Option<CueId>,
        tweak: impl FnOnce(&mut Cue),
    ) -> CueId {
        let mut c = self.show.make_cue(ty, file, self.show.cues.len());
        c.name = name.into();
        c.parent = parent;
        if let Some(m) = c.kind.media_mut()
            && matches!(ty, CueType::Video | CueType::Audio)
        {
            m.audio_output = self.audio;
        }
        tweak(&mut c);
        let id = c.id;
        self.show.cues.push(c);
        id
    }

    /// A playable file of the fetch, or None (with a note) when it is missing.
    fn have(&mut self, id: &str, kind: Kind) -> Option<Resolved> {
        let a = find(id).expect("an asset of the catalog");
        debug_assert_eq!(a.kind, kind);
        if !self.used.contains(&a.id) {
            self.used.push(a.id);
        }
        match self.res.get(id) {
            Some(r) => Some(r.clone()),
            None => {
                self.notes.push(format!(
                    "{} was not available, so the test goes without it",
                    a.title
                ));
                None
            }
        }
    }

    fn video(
        &mut self,
        id: &str,
        name: &str,
        parent: Option<CueId>,
        tweak: impl FnOnce(&mut Cue, &Resolved),
    ) -> Option<CueId> {
        let r = self.have(id, Kind::Video)?;
        Some(
            self.cue(CueType::Video, Some(r.path.clone()), name, parent, |c| {
                tweak(c, &r)
            }),
        )
    }

    fn audio(
        &mut self,
        id: &str,
        name: &str,
        parent: Option<CueId>,
        tweak: impl FnOnce(&mut Cue, &Resolved),
    ) -> Option<CueId> {
        let r = self.have(id, Kind::Audio)?;
        Some(
            self.cue(CueType::Audio, Some(r.path.clone()), name, parent, |c| {
                tweak(c, &r)
            }),
        )
    }

    fn group(&mut self, name: &str, mode: GroupMode, chain: bool) -> CueId {
        self.cue(CueType::Group, None, name, None, |c| {
            if chain {
                follow(c);
            }
            if let CueKind::Group(g) = &mut c.kind {
                g.mode = mode;
            }
        })
    }

    fn text(&mut self, text: &str, secs: f64, parent: Option<CueId>, chain: bool) -> CueId {
        let t = text.to_string();
        self.cue(
            CueType::Text,
            None,
            &format!("Text: {text}"),
            parent,
            move |c| {
                if chain {
                    follow(c);
                }
                if let CueKind::Text(tc) = &mut c.kind {
                    tc.text = t;
                    tc.valign = TextVAlign::Middle;
                    tc.size = 0.09;
                    tc.media.end = Some(secs);
                    tc.media.transition = Transition::FadeIn;
                }
            },
        )
    }
}

fn blur_ramp(radius: f32, secs: f64) -> Effect {
    let mut e = Effect::new(EffectKind::Blur);
    e.p[0].set_key(0.0, 0.0);
    e.p[0].set_key(secs / 2.0, radius);
    e.p[0].set_key(secs, 0.0);
    e
}

fn track(from: f32, to: f32, secs: f64) -> Animated {
    let mut a = Animated::new(from);
    a.set_key(0.0, from);
    a.set_key(secs / 2.0, to);
    a.set_key(secs, from);
    a
}

/// The 5.1 downmix of the trailer's sound to a stereo pair: front left and
/// right straight through, centre and surrounds into both at lower levels.
fn downmix_5_1() -> Vec<Crosspoint> {
    let x = |from: u8, to: u16, db: f32| Crosspoint { from, to, db };
    vec![
        x(0, 0, 0.0),
        x(1, 1, 0.0),
        x(2, 0, -3.0),
        x(2, 1, -3.0),
        x(3, 0, -9.0),
        x(3, 1, -9.0),
        x(4, 0, -3.0),
        x(5, 1, -3.0),
    ]
}

/// The show for the decoding comparison: `tiles` copies of one clip playing
/// at once, each in its own part of the picture (a video wall), looping.
pub fn build_decode(original: &Show, clip: &std::path::Path, tiles: usize) -> Built {
    let mut show = original.clone();
    show.cues.clear();
    show.lists.clear();
    show.main_list_name = None;
    show.name = "Test Mode: decoding".into();
    let res = HashMap::new();
    let mut b = Builder {
        show,
        res: &res,
        audio: None,
        notes: Vec::new(),
        used: Vec::new(),
    };
    // Each tile starts the next straight away (a group would work out its own
    // length from its children, which the loops make meaningless here).
    let mut first = None;
    // A grid as square as it can be: 2x2 for four, 3x3 for eight.
    let cols = (tiles as f32).sqrt().ceil().max(1.0) as usize;
    let rows = tiles.div_ceil(cols);
    for i in 0..tiles {
        let (col, row) = ((i % cols) as f32, (i / cols) as f32);
        let id = b.cue(
            CueType::Video,
            Some(clip.to_path_buf()),
            &format!("4K tile {}", i + 1),
            None,
            |c| {
                if i + 1 < tiles {
                    c.continue_mode = ContinueMode::AutoContinue;
                }
                let m = media(c);
                // A plain cut would take the place of the tile before it.
                m.transition = Transition::FadeIn;
                m.loop_media = true;
                // Two minutes of passes: longer than a trial.
                m.loop_count = 12;
                m.layer = i as i32;
                m.transform.scale_x = 1.0 / cols as f32;
                m.transform.scale_y = 1.0 / rows as f32;
                m.transform.x = (col + 0.5) / cols as f32 - 0.5;
                m.transform.y = (row + 0.5) / rows as f32 - 0.5;
            },
        );
        first.get_or_insert(id);
    }
    let cues = b.show.cues.len();
    Built {
        show: b.show,
        first: first.expect("at least one tile"),
        repeats: true,
        lap_secs: 0.0,
        cues,
        notes: b.notes,
        used: b.used,
    }
}

/// The test show for `level`, built from `original`'s settings.
pub fn build(original: &Show, level: Level, res: &HashMap<&'static str, Resolved>) -> Built {
    let mut show = original.clone();
    show.cues.clear();
    show.lists.clear();
    show.main_list_name = None;
    show.name = format!("Test Mode: {}", level.label());
    let audio = show.audio_outputs.first().map(|a| a.id);
    let mut b = Builder {
        show,
        res,
        audio,
        notes: Vec::new(),
        used: Vec::new(),
    };
    let (short, long) = (level == Level::Short, level == Level::Long);
    // Seconds of picture from each 10 s clip in the frame-rate section, and
    // how long the layered section lasts.
    let clip_secs = if short { 4.0 } else { 8.0 };
    let layers_secs = match level {
        Level::Short => 12.0,
        Level::Medium => 30.0,
        Level::Long => 45.0,
    };
    let mut lap = 0.0;

    // A. Title.
    let title_secs = if short { 3.0 } else { 4.0 };
    let first = b.text(
        &format!("NewQ Test Mode · {}", level.label()),
        title_secs,
        None,
        true,
    );
    lap += title_secs;

    // B. The same moment of three films at 60, 24 and 29.97 frames a second,
    // one after another with cross-fades; the long test adds H.265.
    let b_group = b.group("Frame rates", GroupMode::Playlist, true);
    let mut clips: Vec<(&str, &str)> = vec![
        ("bbb_60", "60 fps (Big Buck Bunny)"),
        ("sintel_24", "24 fps (Sintel)"),
        ("jelly_30", "29.97 fps (Jellyfish)"),
    ];
    if long {
        clips.push(("bbb_hevc", "H.265 (Big Buck Bunny)"));
        clips.push(("jelly_hevc", "H.265 (Jellyfish)"));
        clips.push(("sintel_hi", "25 Mbit/s H.264 (Sintel)"));
    }
    let mut played = 0;
    for (id, name) in clips {
        let secs = clip_secs;
        if b.video(id, name, Some(b_group), |c, _| {
            let m = media(c);
            m.end = Some(secs);
            m.transition = Transition::Crossfade;
            m.fade_in = 1.0;
            m.fade_out = 1.0;
        })
        .is_some()
        {
            played += 1;
        }
    }
    lap += clip_secs * f64::from(played);

    // C. Large pictures as a slide show with cross-fades.
    let pictures: &[&str] = match level {
        Level::Short => &["apollo11", "bluemarble"],
        Level::Medium => &["apollo11", "bluemarble", "earthrise", "curiosity"],
        Level::Long => &[
            "apollo11",
            "bluemarble",
            "earthrise",
            "curiosity",
            "mars",
            "andromeda",
        ],
    };
    let files: Vec<_> = pictures
        .iter()
        .filter_map(|id| b.have(id, Kind::Image).map(|r| r.path))
        .collect();
    if !files.is_empty() {
        let hold = if short { 2.0 } else { 4.0 };
        let n = files.len() as f64;
        let slides = b.cue(CueType::Group, None, "Large pictures", None, |c| {
            follow(c);
            if let CueKind::Group(g) = &mut c.kind {
                g.slides = Some(SlideShow {
                    fade: 1.0,
                    duration: hold,
                });
            }
        });
        b.show.add_slides(slides, files);
        lap += n * (hold + 1.0) + 1.0;
    }

    // D. Layers: a looping film behind, two or four smaller ones moving over
    // it, speed changing, blurs, and the trailer's sound through effects.
    let d = b.group("Layers", GroupMode::Timeline, true);
    let passes = |r: &Resolved| ((layers_secs / r.duration.max(1.0)).ceil() as u32).max(1);
    let mut moving: Option<CueId> = None;
    b.video(
        "sintel_24",
        "Layer: background, blurred",
        Some(d),
        |c, r| {
            let m = media(c);
            m.transition = Transition::FadeIn;
            m.loop_media = true;
            m.loop_count = passes(r);
            m.layer = 0;
            m.opacity = 0.7;
            m.effects.push(blur_ramp(14.0, layers_secs));
        },
    );
    if let Some(id) = b.video("bbb_60", "Layer: sliding, speed ramp", Some(d), |c, r| {
        let m = media(c);
        m.transition = Transition::FadeIn;
        m.loop_media = true;
        m.loop_count = passes(r);
        m.layer = 1;
        m.transform.scale_x = 0.45;
        m.transform.scale_y = 0.45;
        m.transform.x = -0.3;
        m.transform.y = 0.2;
        m.rate.set_key(0.0, 0.6);
        m.rate.set_key(layers_secs / 2.0, 1.8);
        m.rate.set_key(layers_secs, 0.6);
        m.motion.x = Some(track(-0.3, 0.3, layers_secs));
    }) {
        moving = Some(id);
    }
    b.video("jelly_30", "Layer: turning", Some(d), |c, r| {
        let m = media(c);
        m.transition = Transition::FadeIn;
        m.loop_media = true;
        m.loop_count = passes(r);
        m.layer = 2;
        m.transform.scale_x = 0.35;
        m.transform.scale_y = 0.35;
        m.transform.x = 0.33;
        m.transform.y = -0.25;
        m.motion.rotation = Some({
            let mut a = Animated::new(0.0);
            a.set_key(0.0, 0.0);
            a.set_key(layers_secs, 360.0);
            a
        });
    });
    if long {
        b.video("bbb_hevc", "Layer: H.265, pixelated", Some(d), |c, r| {
            let m = media(c);
            m.transition = Transition::FadeIn;
            m.loop_media = true;
            m.loop_count = passes(r);
            m.layer = 3;
            m.transform.scale_x = 0.3;
            m.transform.scale_y = 0.3;
            m.transform.x = -0.35;
            m.transform.y = -0.3;
            let mut e = Effect::new(EffectKind::Pixelate);
            e.p[0].set_key(0.0, 2.0);
            e.p[0].set_key(layers_secs / 2.0, 40.0);
            e.p[0].set_key(layers_secs, 2.0);
            m.effects.push(e);
        });
        b.video(
            "jelly_hevc",
            "Layer: H.265, negative with vignette",
            Some(d),
            |c, r| {
                let m = media(c);
                m.transition = Transition::FadeIn;
                m.loop_media = true;
                m.loop_count = passes(r);
                m.layer = 4;
                m.transform.scale_x = 0.3;
                m.transform.scale_y = 0.3;
                m.transform.x = 0.0;
                m.transform.y = 0.35;
                m.effects.push(Effect::new(EffectKind::Negative));
                m.effects.push(Effect::new(EffectKind::Vignette));
                m.effects.push(Effect::new(EffectKind::Sharpen));
            },
        );
    }
    b.audio(
        "sintel_sound",
        "Layer: sound through effects",
        Some(d),
        |c, _| {
            let m = media(c);
            m.end = Some(layers_secs);
            m.fade_in = 1.0;
            m.fade_out = 2.0;
            m.effects.push(Effect::new(EffectKind::Echo));
            if long {
                m.effects.push(Effect::new(EffectKind::Eq));
                m.effects.push(Effect::new(EffectKind::Reverb));
            }
        },
    );
    b.cue(CueType::Clock, None, "Layer: live clock", Some(d), |c| {
        let m = media(c);
        m.transition = Transition::FadeIn;
        m.end = Some(layers_secs);
        m.layer = 9;
    });
    if let Some(target) = moving {
        b.cue(
            CueType::Fade,
            None,
            "Layer: fade cue moves it",
            Some(d),
            |c| {
                c.pre_wait = layers_secs / 2.0;
                if let CueKind::Fade(f) = &mut c.kind {
                    f.target = Some(target);
                    f.duration = (layers_secs / 6.0).max(2.0);
                    f.curve = Curve::SCurve;
                    f.opacity = Some(0.5);
                    f.volume_db = None;
                    f.stop_target = false;
                    f.geometry.y = Some(-0.2);
                    f.geometry.scale_x = Some(0.6);
                    f.geometry.scale_y = Some(0.6);
                    f.geometry.rotation = Some(15.0);
                }
            },
        );
    }
    lap += layers_secs;

    // E. The trailer with 5.1 sound, mixed down to a stereo pair through a
    // matrix; a fade cue drops the centre channel part way.
    let e_group = b.group("5.1 trailer", GroupMode::Timeline, true);
    let e_secs: f64 = if short { 12.0 } else { 33.0 };
    let mut trailer = None;
    if let Some(r) = b.have("bbb_trailer", Kind::Video) {
        let multichannel = r.audio_channels >= 6;
        trailer = b.video(
            "bbb_trailer",
            "Trailer with 5.1 sound",
            Some(e_group),
            |c, r| {
                let m = media(c);
                m.transition = Transition::Crossfade;
                m.fade_in = 1.0;
                m.fade_out = 1.0;
                m.end = Some(e_secs.min(r.duration));
                if multichannel {
                    m.matrix = downmix_5_1();
                }
            },
        );
        if multichannel && let Some(t) = trailer {
            b.cue(
                CueType::Fade,
                None,
                "Trailer: centre channel down",
                Some(e_group),
                |c| {
                    c.pre_wait = e_secs / 3.0;
                    if let CueKind::Fade(f) = &mut c.kind {
                        f.target = Some(t);
                        f.duration = 3.0;
                        f.opacity = None;
                        f.volume_db = None;
                        f.stop_target = false;
                        f.matrix = vec![
                            Crosspoint {
                                from: 2,
                                to: 0,
                                db: -40.0,
                            },
                            Crosspoint {
                                from: 2,
                                to: 1,
                                db: -40.0,
                            },
                        ];
                    }
                },
            );
        }
        lap += e_secs.min(r.duration);
    }

    // F. The stereo trailer with sound effects (not in the short test).
    if !short && let Some(r) = b.have("sintel_trailer", Kind::Video) {
        let secs = r.duration.min(if long { 52.0 } else { 40.0 });
        let f_group = b.group("Stereo trailer", GroupMode::Timeline, true);
        b.video(
            "sintel_trailer",
            "Trailer with compressor and EQ",
            Some(f_group),
            |c, _| {
                let m = media(c);
                m.transition = Transition::Crossfade;
                m.fade_in = 1.0;
                m.fade_out = 1.0;
                m.end = Some(secs);
                m.effects.push(Effect::new(EffectKind::Compressor));
                m.effects.push(Effect::new(EffectKind::Eq));
                if long {
                    m.rate.set_key(0.0, 1.0);
                    m.rate.set_key(secs / 2.0, 1.4);
                    m.rate.set_key(secs, 1.0);
                }
            },
        );
        lap += secs;
    }

    // G. Quick cues, one after another: starting and stopping decoders as
    // fast as a show can ask (the long test).
    if long {
        let g = b.group("Quick cues", GroupMode::Timeline, true);
        let ids = ["bbb_60", "sintel_24", "jelly_30", "bbb_hevc", "jelly_hevc"];
        let mut n = 0.0;
        for i in 0..20 {
            let id = ids[i % ids.len()];
            let at = i as f64 * 0.8;
            if b.video(id, &format!("Quick {}", i + 1), Some(g), |c, _| {
                c.pre_wait = at;
                let m = media(c);
                m.transition = Transition::FadeIn;
                m.end = Some(1.5);
                m.fade_in = 0.2;
                m.fade_out = 0.2;
                m.transform.scale_x = 0.4;
                m.transform.scale_y = 0.4;
                m.transform.x = ((i % 5) as f32 - 2.0) * 0.18;
                m.transform.y = ((i / 5) as f32 - 1.5) * 0.2;
                m.layer = 10 + i as i32;
            })
            .is_some()
            {
                n += 1.0;
            }
        }
        if n > 0.0 {
            lap += 19.0 * 0.8 + 1.5;
        }
    }

    // H. A message sent and a short wait, then (if it goes on) round again.
    b.cue(CueType::Send, None, "Message to nowhere", None, |c| {
        c.continue_mode = ContinueMode::AutoContinue;
        if let CueKind::Send(s) = &mut c.kind {
            s.target = SendTarget::Osc(OscOut {
                host: "127.0.0.1".into(),
                port: 9,
                address: "/newq/test/lap".into(),
                args: "1".into(),
            });
        }
    });
    let wait = if short { 1.0 } else { 3.0 };
    b.cue(CueType::Wait, None, "Wait", None, |c| {
        follow(c);
        if let CueKind::Wait(w) = &mut c.kind {
            w.duration = wait;
        }
    });
    lap += wait;
    let repeats = !short;
    if repeats {
        b.cue(CueType::Control, None, "Round again", None, |c| {
            if let CueKind::Control(ct) = &mut c.kind {
                ct.action = ControlAction::Start;
                ct.target = Some(first);
            }
        });
    } else if let Some(last) = b.show.cues.last_mut() {
        // A single lap ends here.
        last.continue_mode = ContinueMode::DoNotContinue;
    }
    b.show.relay_slides();
    let _ = trailer;
    let cues = b.show.cues.len();
    Built {
        show: b.show,
        first,
        repeats,
        lap_secs: lap,
        cues,
        notes: b.notes,
        used: b.used,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testmode::assets::Source;

    #[test]
    fn the_decoding_wall_starts_all_its_tiles_at_once_and_keeps_them_playing() {
        let Some((ff, clip)) = crate::media::test_media("testsrc_10s.mp4") else {
            return;
        };
        for tiles in [4, 8] {
            let built = build_decode(&Show::default(), &clip, tiles);
            let mut show = built.show;
            assert_eq!(show.cues.len(), tiles);
            assert_eq!(show.cues[0].id, built.first);
            let mut e = crate::engine::Engine::for_tests();
            e.ff = Some(ff.clone());
            e.playhead = Some(built.first);
            e.go(&show);
            let began = std::time::Instant::now();
            let mut last = began;
            let mut all = false;
            while began.elapsed() < std::time::Duration::from_secs(5) {
                let now = std::time::Instant::now();
                e.tick(&show, now.duration_since(last).as_secs_f64());
                last = now;
                std::thread::sleep(std::time::Duration::from_millis(16));
                all |= show
                    .cues
                    .iter()
                    .all(|c| matches!(e.status(c.id), crate::engine::CueStatus::Playing { .. }));
            }
            assert!(all, "{tiles} tiles never played together");
            assert!(e.is_active(), "the wall must keep playing");
            e.stop_all(0.0);
            show.cues.clear();
        }
    }

    #[test]
    fn the_wall_is_a_grid_that_fills_the_picture_for_four_and_for_eight_tiles() {
        for (n, cols, rows) in [(4usize, 2.0f32, 2.0f32), (8, 3.0, 3.0), (1, 1.0, 1.0)] {
            let built = build_decode(&Show::default(), std::path::Path::new("x.mp4"), n);
            assert_eq!(built.show.cues.len(), n);
            for c in &built.show.cues {
                let m = match &c.kind {
                    CueKind::Video(m) => m,
                    _ => panic!("a video tile"),
                };
                assert_eq!(
                    (m.transform.scale_x, m.transform.scale_y),
                    (1.0 / cols, 1.0 / rows)
                );
                assert!(m.transform.x.abs() <= 0.5 && m.transform.y.abs() <= 0.5);
            }
            // The last tile starts nothing after it.
            assert_eq!(
                built.show.cues.last().unwrap().continue_mode,
                ContinueMode::DoNotContinue
            );
        }
    }

    #[test]
    fn each_test_downloads_exactly_the_media_its_show_asks_for() {
        for level in Level::ALL {
            let built = build(&Show::default(), level, &HashMap::new());
            let wanted: Vec<&str> = crate::testmode::assets::wanted(level)
                .iter()
                .map(|a| a.id)
                .collect();
            for id in &built.used {
                assert!(
                    wanted.contains(id),
                    "{} asks for {id}, which it does not download",
                    level.label()
                );
            }
            for id in &wanted {
                // A sound taken from a film needs the film too.
                let source_of_used = built
                    .used
                    .iter()
                    .any(|u| find(u).and_then(|a| a.from) == Some(*id));
                assert!(
                    built.used.contains(id) || source_of_used,
                    "{} downloads {id}, which its show never uses",
                    level.label()
                );
            }
        }
    }
    use crate::testmode::assets::{CATALOG, Kind};
    use std::path::PathBuf;

    fn fake(kind: Kind, id: &str) -> Resolved {
        let (duration, width, height, fps, ch) = match (kind, id) {
            (Kind::Video, "sintel_trailer") => (52.2, 1280, 720, 24.0, 2),
            (Kind::Video, "bbb_trailer") => (33.0, 853, 480, 25.0, 6),
            (Kind::Video, _) => (10.0, 1920, 1080, 30.0, 0),
            (Kind::Image, _) => (0.0, 3000, 3000, 0.0, 0),
            (Kind::Audio, _) => (52.0, 0, 0, 0.0, 2),
        };
        Resolved {
            path: PathBuf::from(format!("/m/{id}")),
            source: Source::Downloaded,
            duration,
            width,
            height,
            fps,
            audio_channels: ch,
            codec: "h264".into(),
        }
    }

    fn all() -> HashMap<&'static str, Resolved> {
        CATALOG.iter().map(|a| (a.id, fake(a.kind, a.id))).collect()
    }

    fn top(show: &Show) -> Vec<&Cue> {
        show.cues.iter().filter(|c| c.parent.is_none()).collect()
    }

    #[test]
    fn each_level_is_a_show_that_plays_itself_and_the_levels_grow() {
        let mut original = Show::default();
        original.remote.osc = true;
        let before = serde_json::to_string(&original).unwrap();
        let mut sizes = Vec::new();
        for level in Level::ALL {
            let built = build(&original, level, &all());
            let cues = top(&built.show);
            let (last, rest) = cues.split_last().unwrap();
            for c in rest {
                assert_ne!(c.continue_mode, ContinueMode::DoNotContinue, "{}", c.name);
            }
            if built.repeats {
                let CueKind::Control(ct) = &last.kind else {
                    panic!("the last cue of {level:?} is {:?}", last.kind.cue_type())
                };
                assert_eq!(
                    (ct.action, ct.target),
                    (ControlAction::Start, Some(built.first))
                );
            } else {
                assert_eq!(last.continue_mode, ContinueMode::DoNotContinue);
            }
            assert_eq!(built.repeats, level != Level::Short);
            assert_eq!(cues[0].id, built.first);
            // Every cue has its own id, groups hold their children together.
            let ids: std::collections::HashSet<_> = built.show.cues.iter().map(|c| c.id).collect();
            assert_eq!(ids.len(), built.show.cues.len());
            for g in built
                .show
                .cues
                .iter()
                .filter(|c| matches!(c.kind, CueKind::Group(_)))
            {
                let kids = built.show.children(g.id);
                assert!(!kids.is_empty(), "{} is empty", g.name);
            }
            // The user's settings come along; the user's show is not touched.
            assert!(built.show.remote.osc);
            assert!(built.notes.is_empty(), "{:?}", built.notes);
            // It saves and loads like any show.
            let dir = std::env::temp_dir().join(format!(
                "newq_plan_{}_{}",
                level.label(),
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("t.newq");
            built.show.save(&path).unwrap();
            assert_eq!(Show::load(&path).unwrap().cues.len(), built.cues);
            let _ = std::fs::remove_dir_all(dir);
            sizes.push((built.cues, built.lap_secs));
        }
        assert_eq!(serde_json::to_string(&original).unwrap(), before);
        assert!(
            sizes[0].0 < sizes[1].0 && sizes[1].0 < sizes[2].0,
            "{sizes:?}"
        );
        assert!((35.0..=100.0).contains(&sizes[0].1), "{sizes:?}");
        assert!((120.0..=300.0).contains(&sizes[1].1), "{sizes:?}");
        assert!((200.0..=420.0).contains(&sizes[2].1), "{sizes:?}");
    }

    #[test]
    fn the_test_plays_through_the_users_audio_output_and_goes_on_without_missing_media() {
        let mut original = Show::default();
        original.add_audio_output();
        let out = original.audio_outputs[0].id;
        let mut res = all();
        res.remove("bbb_60");
        res.remove("sintel_sound");
        let built = build(&original, Level::Medium, &res);
        assert!(built.notes.len() >= 2, "{:?}", built.notes);
        for c in &built.show.cues {
            if matches!(c.kind, CueKind::Video(_) | CueKind::Audio(_)) {
                assert_eq!(
                    c.kind.media().unwrap().audio_output,
                    Some(out),
                    "{}",
                    c.name
                );
            }
            if let Some(m) = c.kind.media()
                && let Some(p) = m.file.to_str()
                && p.starts_with("/m/")
            {
                assert!(
                    !p.ends_with("bbb_60") && !p.ends_with("sintel_sound"),
                    "{}",
                    c.name
                );
            }
        }
        // Nothing at all to play: a title and a wait, and it still runs round.
        let none = build(&Show::default(), Level::Short, &HashMap::new());
        assert!(none.cues >= 3 && !none.notes.is_empty());
    }

    #[test]
    fn the_trailers_sound_is_mixed_down_and_its_centre_faded() {
        let built = build(&Show::default(), Level::Medium, &all());
        let trailer = built
            .show
            .cues
            .iter()
            .find(|c| c.name == "Trailer with 5.1 sound")
            .unwrap();
        let m = trailer.kind.media().unwrap();
        assert_eq!(m.matrix.len(), 8);
        let fade = built
            .show
            .cues
            .iter()
            .find(|c| c.name == "Trailer: centre channel down")
            .unwrap();
        let CueKind::Fade(f) = &fade.kind else {
            panic!()
        };
        assert_eq!(f.target, Some(trailer.id));
        // It names crosspoints the trailer really has.
        for x in &f.matrix {
            assert!(m.matrix.iter().any(|y| (y.from, y.to) == (x.from, x.to)));
        }
    }

    /// Plays the opening seconds of each level for real, with the media of the
    /// test folder standing in for the downloads.
    #[test]
    fn the_shows_really_start_and_chain_in_the_engine() {
        let Some((ff, clip)) = crate::media::test_media("testsrc_10s.mp4") else {
            return;
        };
        let tone = clip.with_file_name("tone_5s.wav");
        let still = clip.with_file_name("color_red.png");
        if !tone.is_file() || !still.is_file() {
            return;
        }
        let mut res = HashMap::new();
        for a in CATALOG {
            let (path, kind) = match a.kind {
                Kind::Video => (&clip, Kind::Video),
                Kind::Image => (&still, Kind::Image),
                Kind::Audio => (&tone, Kind::Audio),
            };
            let mut r = crate::testmode::assets::check(&ff, path, kind).unwrap();
            r.source = Source::Downloaded;
            res.insert(a.id, r);
        }
        let built = build(&Show::default(), Level::Short, &res);
        let mut show = built.show;
        let mut e = crate::engine::Engine::for_tests();
        e.ff = Some(ff);
        e.playhead = Some(built.first);
        e.go(&show);
        let began = std::time::Instant::now();
        let mut seen_group = false;
        // The engine's clock is the real one: a loaded runner is slow, not wrong.
        let mut last = began;
        while !seen_group && began.elapsed() < std::time::Duration::from_secs(90) {
            let now = std::time::Instant::now();
            e.tick(&show, now.duration_since(last).as_secs_f64());
            last = now;
            std::thread::sleep(std::time::Duration::from_millis(16));
            seen_group |= show.cues.iter().any(|c| {
                c.name == "Frame rates"
                    && matches!(e.status(c.id), crate::engine::CueStatus::Playing { .. })
            });
            // The engine has worked its show without a failure other than a
            // missing audio device (none here).
            for m in &e.messages {
                let t = m.1.to_lowercase();
                assert!(
                    t.contains("audio") || t.contains("device") || t.contains("sound"),
                    "the engine said: {}",
                    m.1
                );
            }
        }
        assert!(seen_group, "the title never gave way to the next section");
        assert!(e.is_active());
        e.stop_all(0.0);
        show.cues.clear();
    }
}
