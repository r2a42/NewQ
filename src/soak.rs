//! The overnight soak show: `newq --soak [--short] [--dir DIR]` makes the media
//! it needs (with the FFmpeg NewQ finds, so the same on every platform), writes
//! the show, runs NewQ on it with the stats log and an automatic GO, and when
//! NewQ is closed writes a report. `newq --make-soak-show OUT.newq --media DIR
//! [--short]` writes just the show. The show plays itself round and round, using everything that
//! can leak or drift (video decoders starting and stopping, looped and
//! speed-changed clips, effects, fades, live text, a slide show, sound),
//! without anyone pressing GO. Run it with `NEWQ_STATS` and `NEWQ_AUTOSTART`
//! (see stats.rs, and the README's soak test section).
//!
//! Every cue of the main list but the last follows the one before it; the
//! last starts the first again.

use crate::show::*;
use std::path::{Path, PathBuf};
use std::process::Command;

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
        "Video 1080p with sound",
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

    // 2. Hap, straight to the graphics card (left out where FFmpeg can't make the clip).
    if media.join(FILES[1]).exists() {
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
    }

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

/// The arguments that encode H.264, or the nearest thing every FFmpeg can do:
/// the LGPL builds NewQ bundles (Windows and Mac) have no x264, and the system's
/// own encoders depend on the machine, so those
/// get MPEG-4 clips instead.
pub(crate) fn h264_args(has: impl Fn(&str) -> bool) -> Vec<String> {
    let (encoder, options): (&str, &[&str]) = if has("libx264") {
        ("libx264", &["-preset", "veryfast"])
    } else {
        ("mpeg4", &["-q:v", "3"])
    };
    ["-c:v", encoder]
        .iter()
        .chain(options)
        .chain(&["-pix_fmt", "yuv420p"])
        .map(|s| s.to_string())
        .collect()
}

/// Make the files of `FILES` in `dir` that aren't there yet, with `ffmpeg`.
pub fn make_media(ff: &crate::media::ffmpeg::Ffmpeg, dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let music = "aevalsrc=0.3*sin(2*PI*(220+40*sin(2*PI*t/7))*t)|0.3*sin(2*PI*(330+30*sin(2*PI*t/5))*t):s=48000:d=60";
    let h264 = h264_args(|name| ff.has_encoder(name));
    let jobs: Vec<(&str, Vec<String>)> = {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let mut jobs = vec![
            (
                FILES[0],
                v(&[
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc2=size=1920x1080:rate=30",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:sample_rate=48000",
                    "-t",
                    "30",
                    "-c:a",
                    "aac",
                    "-shortest",
                ])
                .into_iter()
                .chain(h264.clone())
                .collect(),
            ),
            (
                FILES[1],
                v(&[
                    "-f",
                    "lavfi",
                    "-i",
                    "smptehdbars=size=1920x1080:rate=30",
                    "-t",
                    "10",
                    "-c:v",
                    "hap",
                    "-format",
                    "hap",
                ]),
            ),
            (
                FILES[2],
                v(&[
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc2=size=1280x720:rate=30",
                    "-t",
                    "6",
                ])
                .into_iter()
                .chain(h264.clone())
                .collect(),
            ),
            (
                FILES[3],
                v(&["-f", "lavfi", "-i", music, "-c:a", "pcm_s16le"]),
            ),
            (
                FILES[7],
                v(&[
                    "-f",
                    "lavfi",
                    "-i",
                    "anoisesrc=color=pink:amplitude=0.05:sample_rate=48000:duration=10",
                    "-ac",
                    "2",
                    "-c:a",
                    "pcm_s16le",
                ]),
            ),
        ];
        for (n, name) in FILES[4..7].iter().enumerate() {
            jobs.push((
                name,
                v(&[
                    "-f",
                    "lavfi",
                    "-i",
                    &format!(
                        "gradients=size=1920x1080:seed={}:duration=1:speed=0.00001",
                        n + 1
                    ),
                    "-frames:v",
                    "1",
                ]),
            ));
        }
        jobs
    };
    for (name, args) in jobs {
        let out = dir.join(name);
        if out.exists() {
            continue;
        }
        // Some FFmpeg builds (Homebrew's) can't make Hap: the show goes without it.
        if name == FILES[1] && !ff.has_encoder("hap") {
            eprintln!("This FFmpeg has no Hap encoder: the soak show will have no Hap cue.");
            continue;
        }
        // Written under another name and renamed, so a stopped run leaves no half file.
        let tmp = dir.join(format!("partial_{name}"));
        let result = Command::new(&ff.ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(&args)
            .arg(&tmp)
            .output()
            .map_err(|e| format!("can't run {}: {e}", ff.ffmpeg.display()))?;
        if !result.status.success() {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!(
                "making {name} failed: {}",
                String::from_utf8_lossy(&result.stderr).trim()
            ));
        }
        std::fs::rename(&tmp, &out).map_err(|e| format!("{name}: {e}"))?;
    }
    Ok(())
}

/// `newq --soak [--short] [--dir DIR]`: everything, in one command.
fn soak(args: &[String]) -> Result<i32, String> {
    let mut short = false;
    let mut dir = crate::crash::log_dir().join("soak");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--short" => short = true,
            "--dir" => dir = PathBuf::from(it.next().ok_or("--dir needs a folder")?),
            other => return Err(format!("unknown option {other}")),
        }
    }
    let ff = crate::media::ffmpeg::Ffmpeg::locate().ok_or("FFmpeg not found")?;
    println!(
        "Making the media in {} (a minute or two the first time)…",
        dir.display()
    );
    make_media(&ff, &dir)?;
    let media = dir.canonicalize().map_err(|e| e.to_string())?;
    let show_path = dir.join(if short {
        "soak-short.newq"
    } else {
        "soak.newq"
    });
    build(&media, short)
        .save(&show_path)
        .map_err(|e| format!("{e:#}"))?;
    let stats = dir.join(if short {
        "stats-short.csv"
    } else {
        "stats.csv"
    });
    let _ = std::fs::remove_file(&stats);
    println!(
        "Running {}. Leave it; close the window to stop.\nLog: {}",
        show_path.display(),
        stats.display()
    );
    let status = Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .arg(&show_path)
        .env("NEWQ_STATS", &stats)
        .env("NEWQ_AUTOSTART", "5")
        .env("NEWQ_LOG_FILE", "1")
        .status()
        .map_err(|e| e.to_string())?;
    // A release on Windows has no console, so the report is also a file.
    let report = std::fs::read_to_string(&stats)
        .map_err(|e| format!("no stats were written: {e}"))
        .and_then(|t| crate::stats::report(&t));
    let text = match report {
        Ok(t) => t,
        Err(e) => format!("No report: {e}\n"),
    };
    let report_path = stats.with_extension("report.txt");
    let _ = std::fs::write(&report_path, &text);
    print!("\n{text}");
    println!("(saved in {})", report_path.display());
    Ok(status.code().unwrap_or(1))
}

/// `newq --soak`, and `newq --make-soak-show`.
pub fn run_if_asked(args: &[String]) -> Option<i32> {
    if args.first().map(String::as_str) == Some("--soak") {
        return Some(soak(&args[1..]).unwrap_or_else(|e| {
            eprintln!("soak: {e}");
            2
        }));
    }
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
            .filter(|n| *n != FILES[1] && !media.join(n).exists())
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

    #[test]
    fn the_media_is_made_with_the_ffmpeg_newq_finds_and_not_made_twice() {
        let Some(ff) = crate::media::ffmpeg::Ffmpeg::locate() else {
            assert!(
                std::env::var_os("NEWQ_REQUIRE_MEDIA").is_none(),
                "no ffmpeg"
            );
            return;
        };
        let dir = std::env::temp_dir().join(format!("newq_soak_media_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        make_media(&ff, &dir).unwrap();
        let hap = ff.has_encoder("hap");
        for n in FILES {
            if n == FILES[1] && !hap {
                assert!(!dir.join(n).exists());
                continue;
            }
            assert!(dir.join(n).metadata().unwrap().len() > 1000, "{n}");
        }
        if hap {
            let info = ff.probe(&dir.join(FILES[1])).unwrap();
            assert_eq!(info.video.unwrap().codec, "hap");
        }
        let before = dir.join(FILES[0]).metadata().unwrap().modified().unwrap();
        make_media(&ff, &dir).unwrap();
        assert_eq!(
            dir.join(FILES[0]).metadata().unwrap().modified().unwrap(),
            before
        );
        let nowhere = crate::media::ffmpeg::Ffmpeg {
            ffmpeg: PathBuf::from("/no/ffmpeg"),
            ffprobe: PathBuf::from("/no/ffprobe"),
        };
        assert!(make_media(&nowhere, &dir.join("x")).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_video_encoder_is_x264_or_else_mpeg4() {
        let enc = |has: &[&str]| h264_args(|n| has.contains(&n))[1].clone();
        assert_eq!(enc(&["libx264", "h264_videotoolbox"]), "libx264");
        assert_eq!(enc(&["h264_videotoolbox", "h264_mf"]), "mpeg4");
        assert_eq!(enc(&[]), "mpeg4");
        // x264's preset is only passed to x264.
        assert!(!h264_args(|_| false).contains(&"-preset".to_string()));
        assert!(h264_args(|_| true).contains(&"yuv420p".to_string()));
    }

    #[test]
    fn the_media_is_made_even_with_only_the_fallback_encoder() {
        // MPEG-4 is in every FFmpeg: make the clip the way an FFmpeg without x264 would.
        let Some(ff) = crate::media::ffmpeg::Ffmpeg::locate() else {
            return;
        };
        let args = h264_args(|_| false);
        let dir = std::env::temp_dir().join(format!("newq_soak_mpeg4_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("x.mp4");
        let status = ff
            .command()
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x180:rate=30",
                "-t",
                "1",
            ])
            .args(&args)
            .arg(&out)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(ff.probe(&out).unwrap().video.is_some());
        let _ = std::fs::remove_dir_all(dir);
    }
}
