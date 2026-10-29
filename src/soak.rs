//! The overnight soak show: `newq --make-soak-show OUT.newq --media DIR [--short]`
//! writes a show that plays itself round and round, using everything that
//! can leak or drift (video decoders starting and stopping, looped and
//! speed-changed clips, effects, fades, live text, a slide show, sound),
//! without anyone pressing GO. Run it with `NEWQ_STATS` and `NEWQ_AUTOSTART`
//! (see stats.rs, and the README's soak test section); `scripts/make_soak_media.sh`
//! makes the files it needs.
//!
//! Every cue of the main list but the last follows the one before it; the
//! last starts the first again.

use crate::show::*;
use std::path::{Path, PathBuf};

/// The files of the media folder the show plays.
pub const FILES: [&str; 8] = [
    "h264_1080.mp4",
    "hap_1080.mov",
    "loop_720.mp4",
    "music.wav",
    "slide1.png",
    "slide2.png",
    "slide3.png",
    "ambience.wav",
];

/// Builds the show. `short` makes a cycle of about a minute instead of about
/// five, for trying it out before leaving it overnight.
pub fn build(media: &Path, short: bool) -> Show {
    let k = if short { 0.2 } else { 1.0 };
    let f = |name: &str| -> PathBuf { media.join(name) };
    let mut show = Show {
        name: if short { "Soak (short)" } else { "Soak" }.into(),
        ..Show::default()
    };

    // One cue, appended; `tweak` sets what the type's defaults don't.
    fn add(
        show: &mut Show,
        ty: CueType,
        file: Option<PathBuf>,
        name: &str,
        parent: Option<CueId>,
        tweak: impl FnOnce(&mut Cue),
    ) -> CueId {
        let mut c = show.make_cue(ty, file, show.cues.len());
        c.name = name.into();
        c.parent = parent;
        tweak(&mut c);
        let id = c.id;
        show.cues.push(c);
        id
    }
    fn playing(c: &mut Cue) -> &mut MediaCue {
        c.kind.media_mut().expect("a cue that plays something")
    }
    let follow = |c: &mut Cue| c.continue_mode = ContinueMode::AutoFollow;

    // 1. The workhorse: a 1080p H.264 clip with sound, fading in and out.
    let first = add(
        &mut show,
        CueType::Video,
        Some(f(FILES[0])),
        "H.264 1080p with sound",
        None,
        |c| {
            follow(c);
            let m = playing(c);
            m.end = Some(30.0 * k);
            m.fade_in = 1.0;
            m.fade_out = 1.0;
            m.transition = Transition::Crossfade;
        },
    );

    // 2. Hap, straight to the graphics card.
    add(
        &mut show,
        CueType::Video,
        Some(f(FILES[1])),
        "Hap 1080p",
        None,
        |c| {
            follow(c);
            let m = playing(c);
            m.end = Some(10.0 * k);
            m.fade_in = 0.5;
            m.fade_out = 0.5;
        },
    );

    // 3. Layers at once: two looping clips (one with its speed and position
    //    moving, one blurred), music through an echo, a live clock, and a fade
    //    cue that moves the first layer part way through.
    let layers = add(&mut show, CueType::Group, None, "Layers", None, |c| {
        follow(c);
        if let CueKind::Group(g) = &mut c.kind {
            g.mode = GroupMode::Timeline;
        }
    });
    let total = 40.0 * k;
    let passes = ((total / 6.0).ceil() as u32).max(1);
    let moving = add(
        &mut show,
        CueType::Video,
        Some(f(FILES[2])),
        "Loop, speed and position moving",
        Some(layers),
        |c| {
            let m = playing(c);
            m.transition = Transition::FadeIn;
            m.loop_media = true;
            m.loop_count = passes;
            m.layer = 1;
            m.opacity = 0.8;
            m.transform.scale_x = 0.5;
            m.transform.scale_y = 0.5;
            m.transform.x = -0.3;
            m.rate.set_key(0.0, 0.8);
            m.rate.set_key(total / 2.0, 1.25);
            m.rate.set_key(total, 0.8);
            let mut x = Animated::new(-0.3);
            x.set_key(0.0, -0.3);
            x.set_key(total / 2.0, 0.3);
            x.set_key(total, -0.3);
            m.motion.x = Some(x);
        },
    );
    add(
        &mut show,
        CueType::Video,
        Some(f(FILES[2])),
        "Loop, blurred and turning",
        Some(layers),
        |c| {
            let m = playing(c);
            m.transition = Transition::FadeIn;
            m.loop_media = true;
            m.loop_count = passes;
            m.layer = 2;
            m.opacity = 0.7;
            m.transform.scale_x = 0.4;
            m.transform.scale_y = 0.4;
            m.transform.x = 0.35;
            let mut blur = Effect::new(EffectKind::Blur);
            blur.p[0].set_key(0.0, 0.0);
            blur.p[0].set_key(total / 2.0, 12.0);
            blur.p[0].set_key(total, 0.0);
            m.effects.push(blur);
            let mut turn = Animated::new(0.0);
            turn.set_key(0.0, 0.0);
            turn.set_key(total, 360.0);
            m.motion.rotation = Some(turn);
        },
    );
    add(
        &mut show,
        CueType::Audio,
        Some(f(FILES[3])),
        "Music through an echo",
        Some(layers),
        |c| {
            let m = playing(c);
            m.end = Some(total);
            m.fade_in = 1.0;
            m.fade_out = 2.0;
            m.effects.push(Effect::new(EffectKind::Echo));
        },
    );
    add(
        &mut show,
        CueType::Clock,
        None,
        "Live clock",
        Some(layers),
        |c| {
            // A text cue crossfades by default, which would fade the layers away.
            playing(c).transition = Transition::FadeIn;
            playing(c).end = Some(total);
            playing(c).layer = 3;
        },
    );
    add(
        &mut show,
        CueType::Fade,
        None,
        "Move the first loop",
        Some(layers),
        |c| {
            c.pre_wait = total / 2.0;
            if let CueKind::Fade(fc) = &mut c.kind {
                fc.target = Some(moving);
                fc.duration = 5.0 * k.max(0.4);
                fc.curve = Curve::SCurve;
                fc.opacity = Some(0.4);
                fc.volume_db = None;
                fc.stop_target = false;
                fc.geometry.y = Some(0.3);
                fc.geometry.scale_x = Some(0.7);
                fc.geometry.scale_y = Some(0.7);
            }
        },
    );

    // 4. A slide show of three pictures crossfading.
    let slides = add(&mut show, CueType::Group, None, "Slides", None, |c| {
        follow(c);
        if let CueKind::Group(g) = &mut c.kind {
            g.slides = Some(SlideShow {
                fade: 1.0,
                duration: (4.0 * k).max(1.0),
            });
        }
    });
    show.add_slides(slides, vec![f(FILES[4]), f(FILES[5]), f(FILES[6])]);

    // 5. Things that send and wait while a sound plays under them.
    add(
        &mut show,
        CueType::Send,
        None,
        "OSC to nowhere",
        None,
        |c| {
            c.continue_mode = ContinueMode::AutoContinue;
            if let CueKind::Send(s) = &mut c.kind {
                s.target = SendTarget::Osc(OscOut {
                    host: "127.0.0.1".into(),
                    port: 9,
                    address: "/soak/cycle".into(),
                    args: "1".into(),
                });
            }
        },
    );
    add(
        &mut show,
        CueType::Audio,
        Some(f(FILES[7])),
        "Ambience, faded out",
        None,
        |c| {
            c.continue_mode = ContinueMode::AutoContinue;
            let m = playing(c);
            m.fade_in = 1.0;
        },
    );
    add(&mut show, CueType::Wait, None, "Wait", None, |c| {
        c.continue_mode = ContinueMode::AutoFollow;
        if let CueKind::Wait(w) = &mut c.kind {
            w.duration = (8.0 * k).max(3.0);
        }
    });

    // 6. Back to the start.
    add(
        &mut show,
        CueType::Control,
        None,
        "Again from the top",
        None,
        |c| {
            if let CueKind::Control(ct) = &mut c.kind {
                ct.action = ControlAction::Start;
                ct.target = Some(first);
            }
        },
    );
    show.relay_slides();
    show
}

/// `newq --make-soak-show OUT.newq --media DIR [--short]`.
pub fn run_if_asked(args: &[String]) -> Option<i32> {
    if args.first().map(String::as_str) != Some("--make-soak-show") {
        return None;
    }
    let result = (|| {
        let out = args
            .get(1)
            .ok_or("usage: --make-soak-show OUT.newq --media DIR [--short]")?;
        let mut media = None;
        let mut short = false;
        let mut it = args[2..].iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--media" => media = it.next().map(PathBuf::from),
                "--short" => short = true,
                other => return Err(format!("unknown option {other}")),
            }
        }
        let media = media.ok_or("--media DIR is needed (see scripts/make_soak_media.sh)")?;
        let media = media
            .canonicalize()
            .map_err(|e| format!("{}: {e}", media.display()))?;
        let missing: Vec<&str> = FILES
            .iter()
            .copied()
            .filter(|n| !media.join(n).exists())
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "{} lacks {}: run scripts/make_soak_media.sh",
                media.display(),
                missing.join(", ")
            ));
        }
        build(&media, short)
            .save(Path::new(out))
            .map_err(|e| format!("{e:#}"))?;
        println!("wrote {out}");
        Ok(())
    })();
    Some(match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("make-soak-show: {e}");
            2
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_soak_show_runs_itself_round_and_round() {
        for short in [true, false] {
            let show = build(Path::new("/media"), short);
            let top: Vec<&Cue> = show.cues.iter().filter(|c| c.parent.is_none()).collect();
            let (last, rest) = top.split_last().unwrap();
            // Each cue but the last hands over to the next when it starts or ends.
            for c in rest {
                assert_ne!(
                    c.continue_mode,
                    ContinueMode::DoNotContinue,
                    "{} would wait for a hand",
                    c.name
                );
            }
            // The last one starts the first again.
            let CueKind::Control(ctl) = &last.kind else {
                panic!("the last cue is {:?}", last.kind.cue_type())
            };
            assert_eq!(ctl.action, ControlAction::Start);
            assert_eq!(ctl.target, Some(top[0].id));
            // Everything it plays is in the media folder.
            for c in &show.cues {
                if let Some(m) = c.kind.media()
                    && !matches!(c.kind, CueKind::Text(_))
                {
                    assert!(m.file.starts_with("/media"), "{}", c.name);
                    let name = m.file.file_name().unwrap().to_string_lossy();
                    assert!(FILES.contains(&name.as_ref()), "{name}");
                }
            }
            // A slide show of three, timed, and the layers of the group.
            let slides = show
                .cues
                .iter()
                .find(|c| matches!(&c.kind, CueKind::Group(g) if g.slides.is_some()))
                .unwrap();
            assert_eq!(show.children(slides.id).len(), 3);
            let layers = show.cues.iter().find(|c| c.name == "Layers").unwrap();
            assert!(show.children(layers.id).len() >= 5);
            // Layers lie over each other: a crossfade or cut would remove its neighbours.
            for c in show.children(layers.id) {
                if let Some(m) = c.kind.media() {
                    assert_eq!(m.transition, Transition::FadeIn, "{}", c.name);
                }
            }
            let ids: std::collections::HashSet<_> = show.cues.iter().map(|c| c.id).collect();
            assert_eq!(ids.len(), show.cues.len());
        }
    }

    #[test]
    fn the_soak_show_is_saved_and_loads_again() {
        let dir = std::env::temp_dir().join(format!("newq_soak_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("soak.newq");
        let show = build(&dir, true);
        show.save(&path).unwrap();
        let back = Show::load(&path).unwrap();
        assert_eq!(back.cues.len(), show.cues.len());
        assert_eq!(back.name, "Soak (short)");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_tool_asks_for_what_it_needs() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(run_if_asked(&args(&["--something-else"])), None);
        assert_eq!(run_if_asked(&args(&["--make-soak-show"])), Some(2));
        assert_eq!(
            run_if_asked(&args(&[
                "--make-soak-show",
                "x.newq",
                "--media",
                "/no/such"
            ])),
            Some(2)
        );
    }
}
