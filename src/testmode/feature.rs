//! The full check: a show that uses every kind of cue, played one part at a
//! time, with each part watched to see that it did what it should (started,
//! had a picture or took its sound, faded, ended), not only that nothing fell
//! over.
//!
//! Its media is made by FFmpeg into the run's own folder (`make_media`), cut
//! from the catalog's film and pictures when there are some (`Sources`), else
//! plain. `build` makes the show and its
//! parts from a copy of the user's show with the cues taken out; `Runner`
//! plays the parts through an `Engine`, noting what it sees every few
//! milliseconds (`Obs`), and `judge` turns what was seen into a result for
//! each check. `judge` is a plain function of the notes, tested without
//! playing anything. The same runner is driven by Test Mode's window (through
//! `session`) and by `newq --self-test` with no window at all (`selftest`).
//!
//! Nothing here reaches outside the computer: the only network use is one OSC
//! message to a socket of its own on 127.0.0.1, the timecode cue has no MIDI
//! output, no send cue runs a program or talks to a projector, live sound
//! input (which would open a microphone) is listed as not run, and the
//! show's own streams and OSC feedback are switched off in the copy.

use crate::engine::{ActiveKind, Engine, LayerSource};
use crate::media::ffmpeg::{Ffmpeg, output_until};
use crate::show::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

// ----- The media ---------------------------------------------------------------

/// The files the check plays, made in the run's folder.
#[derive(Clone, Debug)]
pub struct Media {
    /// Four seconds of moving picture with a tone.
    pub clip: PathBuf,
    /// Two seconds of the same as Hap, if this FFmpeg can make one.
    pub hap: Option<PathBuf>,
    /// Five seconds of stereo tone.
    pub tone: PathBuf,
    /// Three plain pictures.
    pub stills: Vec<PathBuf>,
    /// What could not be made, and why.
    pub notes: Vec<String>,
}

/// How long one file may take to make before FFmpeg is given up on.
const MAKE_LIMIT: Duration = Duration::from_secs(90);

fn run_ffmpeg(ff: &Ffmpeg, args: &[&str], out: &Path, cancel: &AtomicBool) -> Result<(), String> {
    let mut cmd = ff.command();
    cmd.args(["-y", "-hide_banner", "-loglevel", "error"])
        .args(args)
        .arg(out);
    let result = output_until(&mut cmd, cancel, Some(Instant::now() + MAKE_LIMIT))
        .map_err(|e| format!("FFmpeg could not be run: {e}"))?;
    if result.status.success() && out.is_file() {
        return Ok(());
    }
    let _ = std::fs::remove_file(out);
    let said = String::from_utf8_lossy(&result.stderr);
    Err(said
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("FFmpeg did not make the file")
        .trim()
        .to_string())
}

/// Real pictures to make the check's media from, when there are some: the
/// same free films and NASA pictures the longer tests use. Without them the
/// check makes plain ones.
#[derive(Clone, Debug, Default)]
pub struct Sources {
    /// A film with sound (the Sintel trailer).
    pub film: Option<PathBuf>,
    /// Pictures for the stills (three are used; fewer means plain ones).
    pub stills: Vec<PathBuf>,
}

/// The catalog's ids of what `Sources` is made from.
pub const SOURCE_FILM: &str = "sintel_trailer";
pub const SOURCE_STILLS: [&str; 3] = ["apollo11", "bluemarble", "earthrise"];

/// Make the media in `dir`. An error is something the check can't run
/// without (the clip, the tone, a picture); the Hap clip is only noted.
pub fn make_media(
    ff: &Ffmpeg,
    dir: &Path,
    cancel: &AtomicBool,
    from: &Sources,
) -> Result<Media, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut notes = Vec::new();
    let clip = dir.join("clip.mp4");
    let h264 = crate::soak::h264_args(|n| ff.has_encoder(n));
    // Four seconds of the film with its own sound, at 1280x720.
    let from_film = from.film.as_ref().and_then(|film| {
        let film = film.to_string_lossy().into_owned();
        let mut args: Vec<&str> = vec![
            "-ss",
            "20",
            "-i",
            &film,
            "-t",
            "4",
            "-map",
            "0:v:0",
            "-map",
            "0:a:0",
            "-vf",
            "scale=1280:720:force_original_aspect_ratio=decrease,pad=1280:720:(ow-iw)/2:(oh-ih)/2,setsar=1,fps=30",
            "-ac",
            "2",
            "-ar",
            "48000",
        ];
        args.extend(h264.iter().map(String::as_str));
        args.extend(["-g", "15", "-c:a", "aac"]);
        match run_ffmpeg(ff, &args, &clip, cancel) {
            Ok(()) => Some(()),
            Err(e) => {
                notes.push(format!("The film could not be cut ({e}): a test pattern is used"));
                None
            }
        }
    });
    if from_film.is_none() {
        let mut args: Vec<&str> = vec![
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=640x360:rate=30",
            "-f",
            "lavfi",
            "-i",
            "aevalsrc=0.25*sin(2*PI*440*t)|0.25*sin(2*PI*660*t):s=48000",
            "-t",
            "4",
        ];
        args.extend(h264.iter().map(String::as_str));
        args.extend(["-g", "15", "-c:a", "aac", "-shortest"]);
        run_ffmpeg(ff, &args, &clip, cancel).map_err(|e| format!("the test clip: {e}"))?;
    }

    let tone = dir.join("tone.wav");
    run_ffmpeg(
        ff,
        &[
            "-f",
            "lavfi",
            "-i",
            "aevalsrc=0.25*sin(2*PI*330*t)|0.25*sin(2*PI*495*t):s=48000:d=5",
            "-c:a",
            "pcm_s16le",
        ],
        &tone,
        cancel,
    )
    .map_err(|e| format!("the test tone: {e}"))?;

    let mut stills = Vec::new();
    if from.stills.len() >= 3 {
        for (n, picture) in from.stills.iter().take(3).enumerate() {
            let path = dir.join(format!("still_{}.png", n + 1));
            let picture = picture.to_string_lossy().into_owned();
            match run_ffmpeg(
                ff,
                &[
                    "-i",
                    &picture,
                    "-vf",
                    "scale=1280:720:force_original_aspect_ratio=decrease,pad=1280:720:(ow-iw)/2:(oh-ih)/2,setsar=1",
                    "-frames:v",
                    "1",
                ],
                &path,
                cancel,
            ) {
                Ok(()) => stills.push(path),
                Err(e) => {
                    notes.push(format!(
                        "A picture could not be used ({e}): plain ones are made"
                    ));
                    stills.clear();
                    break;
                }
            }
        }
    }
    for (n, colour) in ["0x993333", "0x339966", "0x334499"]
        .iter()
        .enumerate()
        .skip(stills.len())
    {
        let path = dir.join(format!("still_{}.png", n + 1));
        let source = format!("color=c={colour}:s=640x360:d=1");
        run_ffmpeg(
            ff,
            &["-f", "lavfi", "-i", &source, "-frames:v", "1"],
            &path,
            cancel,
        )
        .map_err(|e| format!("a test picture: {e}"))?;
        stills.push(path);
    }

    let clip_text = clip.to_string_lossy().into_owned();
    let hap = if ff.has_encoder("hap") {
        let path = dir.join("clip_hap.mov");
        match run_ffmpeg(
            ff,
            &["-i", &clip_text, "-t", "2", "-an", "-c:v", "hap"],
            &path,
            cancel,
        ) {
            Ok(()) => Some(path),
            Err(e) => {
                notes.push(format!("The Hap clip could not be made ({e})"));
                None
            }
        }
    } else {
        None
    };
    Ok(Media {
        clip,
        hap,
        tone,
        stills,
        notes,
    })
}

// ----- The parts of the check ------------------------------------------------------

/// Something the runner does at a moment of a part.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    /// Fire the cue, as a click on its Fire button would.
    Fire(CueId),
    /// Put the playhead on the cue and press GO.
    GoAt(CueId),
    /// Press GO.
    Go,
    /// Begin recording the first display into the run's folder (the window's run only).
    RecordOn,
    /// Stop recording.
    RecordOff,
    /// Start a stream to a server NewQ runs itself, on this computer only.
    ServeOn,
    ServeOff,
}

/// Something that must be so in a part. Times are seconds into the part.
#[derive(Clone, Debug, PartialEq)]
pub enum Check {
    /// It was playing (its first picture or sound there) by `by`.
    Started { cue: CueId, by: f64 },
    /// Its picture was among the layers handed to its display, at least this opaque.
    Picture { cue: CueId, opacity: f32 },
    /// Its picture kept changing: at least this many different frames.
    Frames { cue: CueId, at_least: u64 },
    /// The mixer took at least this many seconds of its sound.
    Sound { cue: CueId, secs: f64 },
    /// It was over by `by`.
    EndedBy { cue: CueId, by: f64 },
    /// It was not over before `before` (it was not cut short).
    NotBefore { cue: CueId, before: f64 },
    /// It was playing at `at`.
    PlayingAt { cue: CueId, at: f64 },
    /// It was not playing at `at`.
    IdleAt { cue: CueId, at: f64 },
    /// Its opacity at `at`.
    OpacityAt { cue: CueId, at: f64, want: f32 },
    /// Its volume in dB at `at`.
    VolumeAt { cue: CueId, at: f64, want: f32 },
    /// Where its picture is across the display at `at`.
    XAt { cue: CueId, at: f64, want: f32 },
    /// Its own time did not move between `from` and `to` (it was held).
    Held { cue: CueId, from: f64, to: f64 },
    /// Its own time moved on between `from` and `to`.
    Moving { cue: CueId, from: f64, to: f64 },
    /// It went round at least this many times.
    Passes { cue: CueId, at_least: u32 },
    /// How long the engine says it runs, in seconds.
    Length { cue: CueId, want: f64 },
    /// `b` started while `a` was still showing (a crossfade between them).
    Overlap { a: CueId, b: CueId },
    /// `b` started after `a` did.
    After { a: CueId, b: CueId },
    /// All of them were showing at once, bottom first in this order.
    Together { cues: Vec<CueId> },
    /// The playhead was on it at `at`.
    PlayheadAt { cue: CueId, at: f64 },
    /// Whether it was armed at `at`.
    ArmedAt { cue: CueId, at: f64, armed: bool },
    /// It had been got ready ahead of time at some moment before `by`.
    ReadyBy { cue: CueId, by: f64 },
    /// An OSC message to this address arrived on the loopback socket.
    Osc { address: String },
    /// A recording was written (looked at once the part is over).
    Recorded,
    /// The streaming server ran and the stream to it stayed up.
    Served,
}

/// One part of the check: what it tries, what it does and what must be so.
#[derive(Clone, Debug)]
pub struct Part {
    /// The kind of cue it is about, as the report groups them.
    pub kind: &'static str,
    /// What it tries, in a few words.
    pub title: &'static str,
    /// What is done, and when (seconds into the part).
    pub acts: Vec<(f64, Act)>,
    /// How long it is given.
    pub secs: f64,
    pub checks: Vec<Check>,
    /// Problems the engine may report in this part without it being a fault
    /// of the part (each a piece of the message), and what to say of them.
    pub expected: Vec<&'static str>,
    /// Why it is not run, if it isn't.
    pub skip: Option<String>,
    /// It needs the app's windows and graphics (a recording, a stream).
    pub needs_app: bool,
    /// It is all sound: with no sound device there is nothing of it to run.
    pub sound_only: bool,
    /// The cue whose first picture or sound the part's clock starts at.
    pub lead: Option<CueId>,
    /// It is not ended early before this, even with nothing playing.
    pub at_least: f64,
}

impl Part {
    fn new(kind: &'static str, title: &'static str, secs: f64) -> Part {
        Part {
            kind,
            title,
            acts: Vec::new(),
            secs,
            checks: Vec::new(),
            expected: Vec::new(),
            skip: None,
            needs_app: false,
            sound_only: false,
            lead: None,
            at_least: 0.0,
        }
    }
}

/// The check's show and its parts.
pub struct Plan {
    pub show: Show,
    pub parts: Vec<Part>,
    /// Where the send cue's message is waited for.
    pub osc: Option<std::net::UdpSocket>,
    /// How long it should take, in seconds.
    pub secs: f64,
}

struct Builder {
    show: Show,
    audio: Option<AudioOutputId>,
}

impl Builder {
    fn cue(
        &mut self,
        ty: CueType,
        file: Option<&Path>,
        name: &str,
        parent: Option<CueId>,
        tweak: impl FnOnce(&mut Cue),
    ) -> CueId {
        let mut c = self
            .show
            .make_cue(ty, file.map(Path::to_path_buf), self.show.cues.len());
        c.name = name.into();
        c.parent = parent;
        if let Some(m) = c.kind.media_mut() {
            m.audio_output = self.audio;
            // Each part starts from nothing; a fade in is asked for where wanted.
            m.transition = Transition::Cut;
            m.fade_in = 0.0;
            m.fade_out = 0.0;
        }
        tweak(&mut c);
        let id = c.id;
        self.show.cues.push(c);
        id
    }
}

/// The playback settings of the cue being made (in a `tweak`, which is left
/// at once if the cue is of a kind that has none: never so for the cues below).
macro_rules! media {
    ($c:expr) => {
        match $c.kind.media_mut() {
            Some(m) => m,
            None => return,
        }
    };
}

/// How long a part's first cue is given to get going (FFmpeg has to start)
/// before the part goes on without it.
const START: f64 = 5.0;

/// The show for the check and its parts, from `original`'s settings (its
/// displays and audio outputs, unless others were chosen) and `media`.
/// `hosting` says whether a streaming server may be run and can be.
///
/// The times in a part count from the moment its first cue (`Part::lead`) is
/// playing, so a slow start of FFmpeg moves the whole part and spoils none of
/// its checks.
pub fn build(original: &Show, media_files: &Media, hosting: Result<(), String>) -> Plan {
    let mut show = original.clone();
    show.cues.clear();
    show.lists.clear();
    show.main_list_name = None;
    show.name = "Test Mode: full check".into();
    // Layers kept apart, as every new show has them: a show from before that
    // setting replaces everything on a display at each cue, and the parts
    // below that play things together would then be testing that instead.
    show.own_layers = true;
    // Nothing of the show's own leaves the computer while the check runs.
    for o in &mut show.outputs {
        o.stream = None;
    }
    show.remote.osc_feedback = String::new();
    if show.outputs.is_empty() {
        show.add_display();
    }
    let audio = show.audio_outputs.first().map(|a| a.id);
    let mut b = Builder { show, audio };
    let mut parts: Vec<Part> = Vec::new();
    let m = media_files;
    let (clip, tone) = (m.clip.as_path(), m.tone.as_path());
    let still = |n: usize| m.stills[n % m.stills.len().max(1)].as_path();

    // ----- Video -----
    {
        let v = b.cue(
            CueType::Video,
            Some(clip),
            "Video: a clip with sound",
            None,
            |c| {
                let m = media!(c);
                m.transition = Transition::FadeIn;
                m.fade_in = 0.5;
            },
        );
        let mut p = Part::new("Video", "plays a clip with its sound, start to end", 5.0);
        p.lead = Some(v);
        p.acts.push((0.0, Act::Fire(v)));
        p.checks = vec![
            Check::Picture {
                cue: v,
                opacity: 0.95,
            },
            Check::Frames {
                cue: v,
                at_least: 40,
            },
            Check::Sound { cue: v, secs: 2.0 },
            Check::NotBefore {
                cue: v,
                before: 3.0,
            },
            Check::EndedBy { cue: v, by: 4.6 },
        ];
        parts.push(p);
    }
    {
        // One second of the clip, three times round.
        let v = b.cue(
            CueType::Video,
            Some(clip),
            "Video: a loop of three",
            None,
            |c| {
                let m = media!(c);
                m.end = Some(1.0);
                m.loop_media = true;
                m.loop_count = 3;
            },
        );
        let mut p = Part::new("Video", "loops a set number of times and ends", 4.2);
        p.lead = Some(v);
        p.acts.push((0.0, Act::Fire(v)));
        p.checks = vec![
            Check::Passes {
                cue: v,
                at_least: 3,
            },
            Check::NotBefore {
                cue: v,
                before: 2.2,
            },
            Check::EndedBy { cue: v, by: 3.8 },
        ];
        parts.push(p);
    }
    {
        // A loop with no end, let go by a control cue.
        let v = b.cue(
            CueType::Video,
            Some(clip),
            "Video: a loop without end",
            None,
            |c| {
                let m = media!(c);
                m.end = Some(1.0);
                m.loop_media = true;
                m.loop_count = 0;
            },
        );
        let d = b.cue(
            CueType::Control,
            None,
            "Control: let the loop go",
            None,
            |c| {
                if let CueKind::Control(k) = &mut c.kind {
                    k.action = ControlAction::Devamp;
                    k.target = Some(v);
                }
            },
        );
        let mut p = Part::new(
            "Control",
            "lets a loop that would go round for ever finish and end (devamp)",
            8.0,
        );
        p.lead = Some(v);
        p.acts = vec![(0.0, Act::Fire(v)), (2.4, Act::Fire(d))];
        p.checks = vec![
            Check::PlayingAt { cue: v, at: 2.3 },
            Check::EndedBy { cue: v, by: 7.5 },
        ];
        parts.push(p);
    }
    {
        let v = b.cue(
            CueType::Video,
            Some(clip),
            "Video: at double speed",
            None,
            |c| {
                media!(c).rate = Animated::new(2.0);
            },
        );
        let mut p = Part::new("Video", "plays at twice the speed", 3.4);
        p.lead = Some(v);
        p.acts.push((0.0, Act::Fire(v)));
        p.checks = vec![
            Check::Length { cue: v, want: 2.0 },
            Check::Frames {
                cue: v,
                at_least: 20,
            },
            Check::EndedBy { cue: v, by: 3.0 },
        ];
        parts.push(p);
    }
    {
        let v = b.cue(
            CueType::Video,
            Some(clip),
            "Video: picture effects",
            None,
            |c| {
                let m = media!(c);
                m.end = Some(2.0);
                let mut blur = Effect::new(EffectKind::Blur);
                blur.p[0].set_key(0.0, 0.0);
                blur.p[0].set_key(1.0, 12.0);
                blur.p[0].set_key(2.0, 0.0);
                m.effects.push(blur);
                for kind in [
                    EffectKind::Vignette,
                    EffectKind::Sharpen,
                    EffectKind::Pixelate,
                    EffectKind::Negative,
                ] {
                    m.effects.push(Effect::new(kind));
                }
            },
        );
        let mut p = Part::new(
            "Effects",
            "plays a clip through every picture effect, one of them keyframed",
            3.2,
        );
        p.lead = Some(v);
        p.acts.push((0.0, Act::Fire(v)));
        p.checks = vec![
            Check::Picture {
                cue: v,
                opacity: 0.95,
            },
            Check::Frames {
                cue: v,
                at_least: 20,
            },
            Check::EndedBy { cue: v, by: 2.8 },
        ];
        parts.push(p);
    }
    {
        let mut p = Part::new("Video", "plays a Hap clip", 3.2);
        match &m.hap {
            Some(hap) => {
                let v = b.cue(CueType::Video, Some(hap), "Video: Hap", None, |_| {});
                p.lead = Some(v);
                p.acts.push((0.0, Act::Fire(v)));
                p.checks = vec![
                    Check::Picture {
                        cue: v,
                        opacity: 0.95,
                    },
                    Check::Frames {
                        cue: v,
                        at_least: 20,
                    },
                    Check::EndedBy { cue: v, by: 2.8 },
                ];
            }
            None => {
                p.skip = Some(
                    "the FFmpeg in use has no Hap encoder to make a test clip with (playing Hap needs none)"
                        .into(),
                );
            }
        }
        parts.push(p);
    }

    // ----- Pictures, text, shapes -----
    {
        let i = b.cue(
            CueType::Image,
            Some(still(0)),
            "Image: held two seconds",
            None,
            |c| {
                let m = media!(c);
                m.end = Some(2.0);
                m.transition = Transition::FadeIn;
                m.fade_in = 0.5;
            },
        );
        let mut p = Part::new("Image", "fades a picture in, holds it and ends", 3.4);
        p.lead = Some(i);
        p.acts.push((0.0, Act::Fire(i)));
        p.checks = vec![
            Check::Picture {
                cue: i,
                opacity: 0.95,
            },
            Check::NotBefore {
                cue: i,
                before: 1.5,
            },
            Check::EndedBy { cue: i, by: 3.0 },
        ];
        parts.push(p);
    }
    {
        let t = b.cue(CueType::Text, None, "Text: a title", None, |c| {
            if let CueKind::Text(tc) = &mut c.kind {
                tc.text = "NewQ full check".into();
                tc.size = 0.1;
                tc.media.end = Some(2.0);
            }
        });
        let mut p = Part::new("Text", "draws a title, holds it and ends", 3.4);
        p.lead = Some(t);
        p.acts.push((0.0, Act::Fire(t)));
        p.checks = vec![
            Check::Picture {
                cue: t,
                opacity: 0.95,
            },
            Check::NotBefore {
                cue: t,
                before: 1.5,
            },
            Check::EndedBy { cue: t, by: 3.0 },
        ];
        parts.push(p);
    }
    {
        let clock = b.cue(CueType::Clock, None, "Text: the time of day", None, |c| {
            if let CueKind::Text(tc) = &mut c.kind {
                tc.source = TextSource::Clock {
                    hour24: true,
                    seconds: true,
                };
                tc.media.end = Some(3.2);
                tc.media.transform.y = -0.2;
            }
        });
        let timer = b.cue(
            CueType::Timer,
            None,
            "Text: a timer counting up",
            None,
            |c| {
                if let CueKind::Text(tc) = &mut c.kind {
                    tc.source = TextSource::Timer {
                        total: 10.0,
                        up: true,
                        warn: 0.0,
                    };
                    tc.media.end = Some(3.2);
                    tc.media.layer = 1;
                    tc.media.transform.y = 0.2;
                }
            },
        );
        let mut p = Part::new(
            "Live text",
            "shows a clock and a timer and draws them again as the seconds pass",
            4.6,
        );
        p.lead = Some(clock);
        p.acts = vec![(0.0, Act::Fire(clock)), (0.0, Act::Fire(timer))];
        p.checks = vec![
            Check::Started {
                cue: timer,
                by: 1.0,
            },
            Check::Frames {
                cue: clock,
                at_least: 2,
            },
            Check::Frames {
                cue: timer,
                at_least: 2,
            },
            Check::EndedBy {
                cue: clock,
                by: 4.2,
            },
            Check::EndedBy {
                cue: timer,
                by: 4.2,
            },
        ];
        parts.push(p);
    }
    {
        let s = b.cue(CueType::Shape, None, "Shape: a star", None, |c| {
            if let CueKind::Shape(sc) = &mut c.kind {
                sc.shape = ShapeKind::Star;
                sc.color = [240, 200, 60, 255];
                sc.media.end = Some(2.0);
            }
        });
        let mut p = Part::new("Shape", "draws a shape, holds it and ends", 3.4);
        p.lead = Some(s);
        p.acts.push((0.0, Act::Fire(s)));
        p.checks = vec![
            Check::Picture {
                cue: s,
                opacity: 0.95,
            },
            Check::NotBefore {
                cue: s,
                before: 1.5,
            },
            Check::EndedBy { cue: s, by: 3.0 },
        ];
        parts.push(p);
    }
    {
        let cam = b.cue(
            CueType::Camera,
            None,
            "Camera: the test pattern",
            None,
            |c| {
                if let CueKind::Camera(cc) = &mut c.kind {
                    cc.device = TEST_PATTERN_DEVICE.into();
                    cc.media.end = Some(3.0);
                }
            },
        );
        let mut p = Part::new(
            "Camera",
            "shows live picture from the built-in test pattern device and ends when its time is up",
            4.4,
        );
        p.lead = Some(cam);
        p.acts.push((0.0, Act::Fire(cam)));
        p.checks = vec![
            Check::Picture {
                cue: cam,
                opacity: 0.95,
            },
            Check::Frames {
                cue: cam,
                at_least: 20,
            },
            Check::EndedBy { cue: cam, by: 4.0 },
        ];
        parts.push(p);
    }

    // ----- Sound -----
    {
        let a = b.cue(
            CueType::Audio,
            Some(tone),
            "Audio: a tone, routed",
            None,
            |c| {
                let m = media!(c);
                m.volume_db = -6.0;
                m.matrix = vec![
                    Crosspoint {
                        from: 0,
                        to: 0,
                        db: 0.0,
                    },
                    Crosspoint {
                        from: 1,
                        to: 1,
                        db: -3.0,
                    },
                ];
            },
        );
        let down = b.cue(CueType::Fade, None, "Fade: the tone down", None, |c| {
            if let CueKind::Fade(f) = &mut c.kind {
                f.target = Some(a);
                f.duration = 1.0;
                f.volume_db = Some(-30.0);
                f.opacity = None;
                f.stop_target = false;
                f.matrix = vec![Crosspoint {
                    from: 1,
                    to: 1,
                    db: -20.0,
                }];
            }
        });
        let stop = b.cue(
            CueType::Stop,
            None,
            "Stop: the tone, over half a second",
            None,
            |c| {
                if let CueKind::Stop(s) = &mut c.kind {
                    s.target = Some(a);
                    s.fade = 0.5;
                    s.use_own_fade = false;
                }
            },
        );
        let mut p = Part::new(
            "Audio",
            "plays a tone through a routing matrix, fades it and a crosspoint down with a fade cue, and stops it with a stop cue",
            4.6,
        );
        p.sound_only = true;
        p.lead = Some(a);
        p.acts = vec![
            (0.0, Act::Fire(a)),
            (1.0, Act::Fire(down)),
            (3.0, Act::Fire(stop)),
        ];
        p.checks = vec![
            Check::Sound { cue: a, secs: 1.0 },
            Check::VolumeAt {
                cue: a,
                at: 0.8,
                want: -6.0,
            },
            Check::VolumeAt {
                cue: a,
                at: 2.7,
                want: -30.0,
            },
            Check::PlayingAt { cue: a, at: 2.9 },
            Check::IdleAt { cue: a, at: 4.2 },
        ];
        parts.push(p);
    }
    {
        let a = b.cue(
            CueType::Audio,
            Some(tone),
            "Audio: sound effects",
            None,
            |c| {
                let m = media!(c);
                m.end = Some(2.5);
                m.fade_in = 0.3;
                m.fade_out = 0.3;
                for kind in [
                    EffectKind::Eq,
                    EffectKind::Filter,
                    EffectKind::Compressor,
                    EffectKind::Echo,
                    EffectKind::Reverb,
                ] {
                    m.effects.push(Effect::new(kind));
                }
            },
        );
        let mut p = Part::new(
            "Effects",
            "plays a tone through every sound effect, fading in and out by itself",
            4.0,
        );
        p.sound_only = true;
        p.lead = Some(a);
        p.acts.push((0.0, Act::Fire(a)));
        p.checks = vec![
            Check::Sound { cue: a, secs: 1.5 },
            Check::EndedBy { cue: a, by: 3.6 },
        ];
        parts.push(p);
    }
    {
        let mut p = Part::new("Live audio", "plays sound from an input", 0.0);
        p.skip = Some(
            "not run: it would open a microphone or line input, and a test touches nothing outside NewQ"
                .into(),
        );
        parts.push(p);
    }

    // ----- Slide shows and groups -----
    {
        let g = b.cue(CueType::Group, None, "Slides: by themselves", None, |c| {
            if let CueKind::Group(g) = &mut c.kind {
                g.slides = Some(SlideShow {
                    fade: 0.3,
                    duration: 0.8,
                    ..SlideShow::default()
                });
            }
        });
        b.show.add_slides(g, m.stills.clone());
        let kids: Vec<CueId> = b.show.children(g).iter().map(|c| c.id).collect();
        let mut p = Part::new(
            "Slide show",
            "shows three slides one after another by itself, each fading over the last",
            5.4,
        );
        p.lead = kids.first().copied();
        p.acts.push((0.0, Act::Fire(g)));
        for k in &kids {
            p.checks.push(Check::Started { cue: *k, by: 4.0 });
            p.checks.push(Check::Picture {
                cue: *k,
                opacity: 0.95,
            });
        }
        for pair in kids.windows(2) {
            p.checks.push(Check::After {
                a: pair[0],
                b: pair[1],
            });
            p.checks.push(Check::Overlap {
                a: pair[0],
                b: pair[1],
            });
        }
        p.checks.push(Check::EndedBy { cue: g, by: 5.0 });
        parts.push(p);
    }
    {
        let g = b.cue(CueType::Group, None, "Slides: on GO", None, |c| {
            if let CueKind::Group(g) = &mut c.kind {
                g.slides = Some(SlideShow {
                    fade: 0.3,
                    duration: 1.0,
                    advance: Advance::OnGo,
                });
            }
        });
        b.show.add_slides(g, m.stills.clone());
        let kids: Vec<CueId> = b.show.children(g).iter().map(|c| c.id).collect();
        let mut p = Part::new(
            "Slide show",
            "steps through three slides, one at each GO, and stays on each until then",
            5.0,
        );
        p.lead = kids.first().copied();
        p.acts = vec![(0.0, Act::GoAt(g)), (1.5, Act::Go), (3.0, Act::Go)];
        if let [one, two, three] = kids[..] {
            p.checks = vec![
                Check::PlayingAt { cue: one, at: 1.3 },
                Check::IdleAt { cue: two, at: 1.3 },
                Check::PlayingAt { cue: two, at: 2.8 },
                Check::IdleAt {
                    cue: three,
                    at: 2.8,
                },
                Check::PlayingAt {
                    cue: three,
                    at: 4.6,
                },
                Check::Overlap { a: one, b: two },
            ];
        }
        parts.push(p);
    }
    {
        let g = b.cue(
            CueType::Group,
            None,
            "Group: in order, crossfading",
            None,
            |c| {
                if let CueKind::Group(g) = &mut c.kind {
                    g.mode = GroupMode::Playlist;
                }
            },
        );
        let kids: Vec<CueId> = (0..3)
            .map(|n| {
                b.cue(
                    CueType::Image,
                    Some(still(n)),
                    &format!("Picture {} of the group", n + 1),
                    Some(g),
                    |c| {
                        let m = media!(c);
                        m.end = Some(1.2);
                        if n > 0 {
                            m.transition = Transition::Crossfade;
                            m.fade_in = 0.5;
                        }
                    },
                )
            })
            .collect();
        let mut p = Part::new(
            "Group",
            "plays three pictures in order, each crossfading over the one before",
            5.0,
        );
        p.lead = kids.first().copied();
        p.acts.push((0.0, Act::Fire(g)));
        for k in &kids {
            p.checks.push(Check::Started { cue: *k, by: 3.6 });
        }
        for pair in kids.windows(2) {
            p.checks.push(Check::After {
                a: pair[0],
                b: pair[1],
            });
            p.checks.push(Check::Overlap {
                a: pair[0],
                b: pair[1],
            });
        }
        p.checks.push(Check::EndedBy { cue: g, by: 4.6 });
        parts.push(p);
    }
    {
        let g = b.cue(
            CueType::Group,
            None,
            "Group: all together, in layers",
            None,
            |c| {
                if let CueKind::Group(g) = &mut c.kind {
                    g.mode = GroupMode::Timeline;
                }
            },
        );
        let back = b.cue(
            CueType::Video,
            Some(clip),
            "Layer 0: the clip",
            Some(g),
            |c| {
                let m = media!(c);
                m.end = Some(3.0);
                m.layer = 0;
            },
        );
        let mid = b.cue(CueType::Shape, None, "Layer 1: a circle", Some(g), |c| {
            if let CueKind::Shape(sc) = &mut c.kind {
                sc.shape = ShapeKind::Circle;
                sc.media.end = Some(6.0);
                sc.media.layer = 1;
                sc.media.opacity = 0.7;
            }
        });
        let top = b.cue(CueType::Text, None, "Layer 2: words", Some(g), |c| {
            if let CueKind::Text(tc) = &mut c.kind {
                tc.text = "three layers".into();
                tc.media.end = Some(6.0);
                tc.media.layer = 2;
            }
        });
        let mut p = Part::new(
            "Group",
            "starts a clip, a shape and a text together on three layers of one display",
            3.6,
        );
        p.lead = Some(back);
        p.acts.push((0.0, Act::Fire(g)));
        p.checks = vec![
            Check::Started { cue: mid, by: 1.0 },
            Check::Started { cue: top, by: 1.0 },
            Check::Together {
                cues: vec![back, mid, top],
            },
            Check::EndedBy { cue: back, by: 3.5 },
        ];
        parts.push(p);
    }

    // ----- Transitions -----
    {
        let a = b.cue(
            CueType::Image,
            Some(still(0)),
            "Under: a picture",
            None,
            |c| {
                media!(c).end = Some(20.0);
            },
        );
        let x = b.cue(
            CueType::Image,
            Some(still(1)),
            "Crossfade over it",
            None,
            |c| {
                let m = media!(c);
                m.end = Some(20.0);
                m.transition = Transition::Crossfade;
                m.fade_in = 0.6;
            },
        );
        let dip = b.cue(
            CueType::Image,
            Some(still(2)),
            "Dip to black into this",
            None,
            |c| {
                let m = media!(c);
                m.end = Some(20.0);
                m.transition = Transition::DipToBlack;
                m.fade_in = 1.0;
            },
        );
        let cut = b.cue(CueType::Image, Some(still(0)), "Cut to this", None, |c| {
            media!(c).end = Some(20.0);
        });
        let mut p = Part::new(
            "Transitions",
            "crossfades, dips to black and cuts from one picture to the next, each taking the place of the last",
            6.5,
        );
        p.lead = Some(a);
        p.acts = vec![
            (0.0, Act::Fire(a)),
            (1.0, Act::Fire(x)),
            (2.5, Act::Fire(dip)),
            (5.0, Act::Fire(cut)),
        ];
        p.checks = vec![
            Check::PlayingAt { cue: a, at: 0.9 },
            Check::Overlap { a, b: x },
            Check::IdleAt { cue: a, at: 2.3 },
            Check::PlayingAt { cue: x, at: 2.3 },
            Check::IdleAt { cue: x, at: 4.5 },
            Check::PlayingAt { cue: dip, at: 4.5 },
            Check::IdleAt { cue: dip, at: 5.9 },
            Check::PlayingAt { cue: cut, at: 5.9 },
        ];
        parts.push(p);
    }

    // ----- Fade cues -----
    {
        let i = b.cue(
            CueType::Image,
            Some(still(1)),
            "Picture to be faded",
            None,
            |c| {
                media!(c).end = Some(20.0);
            },
        );
        let dim = b.cue(CueType::Fade, None, "Fade: dim and move it", None, |c| {
            if let CueKind::Fade(f) = &mut c.kind {
                f.target = Some(i);
                f.duration = 1.0;
                f.opacity = Some(0.3);
                f.volume_db = None;
                f.stop_target = false;
                f.geometry.x = Some(0.25);
                f.geometry.scale_x = Some(0.5);
                f.geometry.scale_y = Some(0.5);
            }
        });
        let out = b.cue(CueType::Fade, None, "Fade: out, and stop it", None, |c| {
            if let CueKind::Fade(f) = &mut c.kind {
                f.target = Some(i);
                f.duration = 0.6;
                f.opacity = Some(0.0);
                f.volume_db = None;
                f.stop_target = true;
            }
        });
        let mut p = Part::new(
            "Fade",
            "fades a picture's opacity while moving and shrinking it, then fades it out and stops it",
            4.8,
        );
        p.lead = Some(i);
        p.acts = vec![
            (0.0, Act::Fire(i)),
            (1.0, Act::Fire(dim)),
            (3.0, Act::Fire(out)),
        ];
        p.checks = vec![
            Check::OpacityAt {
                cue: i,
                at: 0.9,
                want: 1.0,
            },
            Check::OpacityAt {
                cue: i,
                at: 2.6,
                want: 0.3,
            },
            Check::XAt {
                cue: i,
                at: 2.6,
                want: 0.25,
            },
            Check::IdleAt { cue: i, at: 4.4 },
        ];
        parts.push(p);
    }

    // ----- Control cues -----
    {
        let t = b.cue(
            CueType::Video,
            Some(clip),
            "Target: a clip going round",
            None,
            |c| {
                let m = media!(c);
                m.loop_media = true;
                m.loop_count = 0;
            },
        );
        let u = b.cue(
            CueType::Image,
            Some(still(2)),
            "Target: a picture",
            None,
            |c| {
                let m = media!(c);
                m.end = Some(20.0);
                m.layer = 1;
                m.transform.scale_x = 0.4;
                m.transform.scale_y = 0.4;
            },
        );
        let control = |b: &mut Builder, name: &str, action: ControlAction, target: CueId| {
            b.cue(CueType::Control, None, name, None, |c| {
                if let CueKind::Control(k) = &mut c.kind {
                    k.action = action;
                    k.target = Some(target);
                }
            })
        };
        let memo = control(&mut b, "Control: a memo", ControlAction::Memo, t);
        let load = control(&mut b, "Control: load the picture", ControlAction::Load, u);
        let pause = control(&mut b, "Control: pause the clip", ControlAction::Pause, t);
        let resume = control(&mut b, "Control: resume the clip", ControlAction::Resume, t);
        let start = control(
            &mut b,
            "Control: start the picture",
            ControlAction::Start,
            u,
        );
        let goto = control(
            &mut b,
            "Control: playhead to the picture",
            ControlAction::GoTo,
            u,
        );
        let disarm = control(
            &mut b,
            "Control: disarm the picture",
            ControlAction::Disarm,
            u,
        );
        let arm = control(&mut b, "Control: arm the picture", ControlAction::Arm, u);
        let reset = control(&mut b, "Control: reset the clip", ControlAction::Reset, t);
        let mut p = Part::new(
            "Control",
            "loads, starts, pauses, resumes and resets cues, moves the playhead, and disarms and arms a cue",
            7.5,
        );
        p.lead = Some(t);
        p.acts = vec![
            (0.0, Act::Fire(t)),
            (0.0, Act::Fire(memo)),
            (0.2, Act::Fire(load)),
            (1.0, Act::Fire(pause)),
            (2.0, Act::Fire(resume)),
            (3.0, Act::Fire(start)),
            (3.5, Act::Fire(goto)),
            (4.0, Act::Fire(disarm)),
            (5.0, Act::Fire(arm)),
            (6.0, Act::Fire(reset)),
        ];
        p.checks = vec![
            Check::ReadyBy { cue: u, by: 2.9 },
            Check::Held {
                cue: t,
                from: 1.3,
                to: 1.9,
            },
            Check::Moving {
                cue: t,
                from: 2.3,
                to: 2.9,
            },
            Check::IdleAt { cue: u, at: 2.9 },
            Check::PlayingAt { cue: u, at: 3.4 },
            Check::PlayheadAt { cue: u, at: 3.9 },
            Check::ArmedAt {
                cue: u,
                at: 4.8,
                armed: false,
            },
            Check::ArmedAt {
                cue: u,
                at: 5.8,
                armed: true,
            },
            Check::PlayingAt { cue: t, at: 5.9 },
            Check::IdleAt { cue: t, at: 7.0 },
        ];
        parts.push(p);
    }

    // ----- GO, continues, wait and stop -----
    {
        let i = b.cue(
            CueType::Image,
            Some(still(0)),
            "GO: a picture, continuing",
            None,
            |c| {
                media!(c).end = Some(20.0);
                c.continue_mode = ContinueMode::AutoContinue;
            },
        );
        let w = b.cue(
            CueType::Wait,
            None,
            "Wait: one second, then follow",
            None,
            |c| {
                if let CueKind::Wait(w) = &mut c.kind {
                    w.duration = 1.0;
                }
                c.continue_mode = ContinueMode::AutoFollow;
            },
        );
        b.cue(CueType::Stop, None, "Stop: everything", None, |c| {
            if let CueKind::Stop(s) = &mut c.kind {
                s.target = None;
                s.fade = 0.3;
                s.use_own_fade = false;
            }
            c.pre_wait = 0.5;
        });
        let mut p = Part::new(
            "GO, wait and stop",
            "GO fires a picture, which continues into a wait, which is followed after a pre-wait by a stop of everything",
            3.4,
        );
        p.lead = Some(i);
        p.acts.push((0.0, Act::GoAt(i)));
        p.checks = vec![
            Check::PlayingAt { cue: w, at: 0.5 },
            Check::EndedBy { cue: w, by: 1.4 },
            // The wait is a second and the stop's pre-wait half of one.
            Check::PlayingAt { cue: i, at: 1.3 },
            Check::IdleAt { cue: i, at: 2.8 },
        ];
        parts.push(p);
    }

    // ----- Send and timecode -----
    let osc = std::net::UdpSocket::bind(("127.0.0.1", 0))
        .and_then(|s| s.set_nonblocking(true).map(|()| s))
        .ok();
    {
        let mut p = Part::new(
            "Send",
            "sends an OSC message to this computer itself and hears it arrive",
            2.0,
        );
        p.at_least = 1.0;
        match osc.as_ref().and_then(|s| s.local_addr().ok()) {
            Some(addr) => {
                let address = "/newq/fullcheck";
                let send = b.cue(
                    CueType::Send,
                    None,
                    "Send: OSC to this computer",
                    None,
                    |c| {
                        if let CueKind::Send(s) = &mut c.kind {
                            s.target = SendTarget::Osc(OscOut {
                                host: "127.0.0.1".into(),
                                port: addr.port(),
                                address: address.into(),
                                args: "7".into(),
                            });
                        }
                    },
                );
                p.acts.push((0.1, Act::Fire(send)));
                p.checks.push(Check::Osc {
                    address: address.into(),
                });
            }
            None => p.skip = Some("no socket could be opened on 127.0.0.1 to listen on".into()),
        }
        parts.push(p);
    }
    {
        let tc = b.cue(
            CueType::Timecode,
            None,
            "Timecode: two seconds",
            None,
            |c| {
                if let CueKind::Timecode(t) = &mut c.kind {
                    t.port = None;
                    t.duration = 2.0;
                }
            },
        );
        let mut p = Part::new(
            "Timecode",
            "runs a timecode cue for its two seconds (with no MIDI output: a test sends nothing to devices)",
            3.2,
        );
        p.lead = Some(tc);
        p.acts.push((0.0, Act::Fire(tc)));
        p.checks = vec![
            Check::PlayingAt { cue: tc, at: 1.0 },
            Check::EndedBy { cue: tc, by: 2.8 },
        ];
        p.expected = vec!["no output chosen for the timecode"];
        parts.push(p);
    }

    // ----- Send out -----
    {
        let v = b.cue(
            CueType::Video,
            Some(clip),
            "Recorded: the clip, going round",
            None,
            |c| {
                let m = media!(c);
                m.loop_media = true;
                m.loop_count = 0;
            },
        );
        let mut p = Part::new(
            "Send out",
            "records the first display's picture to a file in the test's folder",
            8.0,
        );
        p.needs_app = true;
        p.lead = Some(v);
        p.acts = vec![
            (0.0, Act::Fire(v)),
            (0.0, Act::RecordOn),
            (7.0, Act::RecordOff),
        ];
        p.checks = vec![Check::Recorded];
        parts.push(p);
    }
    {
        let v = b.cue(
            CueType::Video,
            Some(clip),
            "Streamed: the clip, going round",
            None,
            |c| {
                let m = media!(c);
                m.loop_media = true;
                m.loop_count = 0;
            },
        );
        let mut p = Part::new(
            "Send out",
            "runs the streaming server on this computer only and streams the first display to it",
            14.0,
        );
        p.needs_app = true;
        p.lead = Some(v);
        p.acts = vec![
            (0.0, Act::Fire(v)),
            (0.0, Act::ServeOn),
            (13.0, Act::ServeOff),
        ];
        p.checks = vec![Check::Served];
        // The stream may try before the server is up, and try again.
        p.expected = vec!["Trying again in"];
        if let Err(why) = hosting {
            p.skip = Some(why);
        }
        parts.push(p);
    }

    // ----- Everything at once -----
    {
        let g = b.cue(
            CueType::Group,
            None,
            "Load: five things at once",
            None,
            |c| {
                if let CueKind::Group(g) = &mut c.kind {
                    g.mode = GroupMode::Timeline;
                }
            },
        );
        let mut kids = Vec::new();
        for n in 0..4 {
            kids.push(b.cue(
                CueType::Video,
                Some(clip),
                &format!("Load: clip {}", n + 1),
                Some(g),
                |c| {
                    let m = media!(c);
                    m.loop_media = true;
                    m.loop_count = 2;
                    m.layer = n;
                    m.volume_db = -24.0;
                    m.transform.scale_x = 0.5;
                    m.transform.scale_y = 0.5;
                    m.transform.x = if n % 2 == 0 { -0.25 } else { 0.25 };
                    m.transform.y = if n < 2 { -0.25 } else { 0.25 };
                    if n == 3 {
                        m.effects.push(Effect::new(EffectKind::Blur));
                    }
                },
            ));
        }
        kids.push(
            b.cue(CueType::Text, None, "Load: words on top", Some(g), |c| {
                if let CueKind::Text(tc) = &mut c.kind {
                    tc.text = "everything at once".into();
                    tc.media.end = Some(6.0);
                    tc.media.layer = 5;
                }
            }),
        );
        let mut p = Part::new(
            "Load",
            "plays four clips with their sound and a text all at once, twice round",
            10.0,
        );
        p.lead = kids.first().copied();
        p.acts.push((0.0, Act::Fire(g)));
        for k in kids.iter().skip(1) {
            p.checks.push(Check::Started { cue: *k, by: 2.5 });
        }
        p.checks.push(Check::Together {
            cues: kids[..4].to_vec(),
        });
        p.checks.push(Check::EndedBy { cue: g, by: 9.6 });
        parts.push(p);
    }

    b.show.relay_slides();
    let secs = parts
        .iter()
        .filter(|p| p.skip.is_none())
        .map(|p| p.secs + GAP + 0.5)
        .sum();
    Plan {
        show: b.show,
        parts,
        osc,
        secs,
    }
}

/// Seconds between parts, for the last one's FFmpegs and fades to be gone.
const GAP: f64 = 0.4;

// ----- What is seen ----------------------------------------------------------------

impl Default for CueObs {
    fn default() -> CueObs {
        CueObs {
            cue: CueId(0),
            instance: 0,
            started: false,
            stopping: false,
            elapsed: 0.0,
            duration: None,
            opacity: 0.0,
            volume: 0.0,
            layer: None,
            frame: 0,
            sound: None,
            pass: None,
        }
    }
}

/// A cue as it was at one moment.
#[derive(Clone, Debug, PartialEq)]
pub struct CueObs {
    pub cue: CueId,
    pub instance: u64,
    pub started: bool,
    pub stopping: bool,
    /// Its own time, in seconds.
    pub elapsed: f64,
    pub duration: Option<f64>,
    pub opacity: f32,
    pub volume: f32,
    /// It was among its display's layers with a picture to show, at this
    /// opacity, this far across, and this far up the pile (0 = bottom).
    pub layer: Option<(f32, f32, usize)>,
    /// Which frame of a film, or which drawing of a picture, it was showing.
    pub frame: u64,
    /// Seconds of its sound the mixer has taken.
    pub sound: Option<f64>,
    pub pass: Option<u32>,
}

/// Everything noted at one moment of a part.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Obs {
    pub t: f64,
    pub cues: Vec<CueObs>,
    pub playhead: Option<CueId>,
    /// The cues that were disarmed.
    pub disarmed: Vec<CueId>,
    /// The cues that were got ready ahead of time.
    pub ready: Vec<CueId>,
}

/// Note what the engine is doing now.
pub fn observe(show: &Show, engine: &Engine, t: f64) -> Obs {
    let mut cues: Vec<CueObs> = engine
        .active
        .iter()
        .filter(|a| !a.is_matte())
        .map(|a| {
            let sound = match &a.kind {
                ActiveKind::Audio(v) => Some(v.clock()),
                ActiveKind::Video(v) => v.voice.as_ref().map(|v| v.clock()),
                _ => None,
            };
            CueObs {
                cue: a.cue_id,
                instance: a.instance,
                started: a.started,
                stopping: a.stopping,
                elapsed: a.elapsed,
                duration: a.duration,
                opacity: a.opacity.value,
                volume: a.volume.value,
                layer: None,
                frame: 0,
                sound,
                pass: a.pass_number(),
            }
        })
        .collect();
    for output in &show.outputs {
        for (place, l) in engine.layers(output.id).iter().enumerate() {
            let frame = match &l.source {
                LayerSource::Video(p) => p.current().map(|_| p.serial.max(1)),
                LayerSource::Image(_, version) => Some(version + 1),
                LayerSource::Solid(_) => None,
            };
            if let (Some(frame), Some(c)) =
                (frame, cues.iter_mut().find(|c| c.instance == l.instance))
            {
                c.layer = Some((l.opacity, l.transform.x, place));
                c.frame = frame;
            }
        }
    }
    Obs {
        t,
        cues,
        playhead: engine.playhead,
        disarmed: show
            .cues
            .iter()
            .filter(|c| !c.armed)
            .map(|c| c.id)
            .collect(),
        ready: show
            .cues
            .iter()
            .filter(|c| engine.is_prepared(c.id))
            .map(|c| c.id)
            .collect(),
    }
}

/// What a check came to.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// It was so; what was seen.
    Ok(String),
    /// It was not; what was seen instead.
    Failed(String),
    /// It could not be looked at; why.
    NotChecked(String),
}

/// What the runner knows that the notes don't say.
#[derive(Clone, Debug, Default)]
pub struct Context {
    /// The OSC addresses that arrived on the loopback socket.
    pub osc: Vec<String>,
    /// Whether sound is being mixed at all.
    pub sound: bool,
    /// What the recording came to, when it has been looked at.
    pub recorded: Option<Verdict>,
    /// What the server and its stream came to.
    pub served: Option<Verdict>,
}

fn of(obs: &Obs, cue: CueId) -> Option<&CueObs> {
    // The newest instance, should a cue be playing twice.
    obs.cues
        .iter()
        .filter(|c| c.cue == cue)
        .max_by_key(|c| c.instance)
}

/// The note nearest `at`.
fn near(obs: &[Obs], at: f64) -> Option<&Obs> {
    obs.iter()
        .min_by(|a, b| (a.t - at).abs().total_cmp(&(b.t - at).abs()))
}

/// When the cue was first seen playing.
fn first_started(obs: &[Obs], cue: CueId) -> Option<f64> {
    obs.iter()
        .find(|o| of(o, cue).is_some_and(|c| c.started))
        .map(|o| o.t)
}

/// When the cue was last seen at all.
fn last_seen(obs: &[Obs], cue: CueId) -> Option<f64> {
    obs.iter().rev().find(|o| of(o, cue).is_some()).map(|o| o.t)
}

fn name(show: &Show, cue: CueId) -> String {
    show.cue(cue).map_or_else(
        || "a cue".to_string(),
        |c| format!("\u{201c}{}\u{201d}", c.name),
    )
}

/// What a check that waits for the driver says until the driver has looked.
const NOT_LOOKED_AT: &str = "it was not looked at before the test ended";

/// Whether `check` held, from what was noted during its part.
pub fn judge(check: &Check, obs: &[Obs], ctx: &Context, show: &Show) -> Verdict {
    use Verdict::{Failed, NotChecked, Ok};
    match check {
        Check::Started { cue, by } => match first_started(obs, *cue) {
            Some(t) if t <= *by => Ok(format!("{} was playing after {t:.1} s", name(show, *cue))),
            Some(t) => Failed(format!(
                "{} took {t:.1} s to start playing (the limit is {by:.1} s)",
                name(show, *cue)
            )),
            None => Failed(format!("{} never started playing", name(show, *cue))),
        },
        Check::Picture { cue, opacity } => {
            let best = obs
                .iter()
                .filter_map(|o| of(o, *cue).and_then(|c| c.layer))
                .map(|l| l.0)
                .fold(None, |b: Option<f32>, v| Some(b.map_or(v, |b| b.max(v))));
            match best {
                Some(v) if v >= *opacity => Ok(format!(
                    "{}'s picture was handed to its display at {:.0}% opacity",
                    name(show, *cue),
                    v * 100.0
                )),
                Some(v) => Failed(format!(
                    "{}'s picture only reached {:.0}% opacity (it should reach {:.0}%)",
                    name(show, *cue),
                    v * 100.0,
                    opacity * 100.0
                )),
                None => Failed(format!(
                    "{} never had a picture among its display's layers",
                    name(show, *cue)
                )),
            }
        }
        Check::Frames { cue, at_least } => {
            let mut frames: Vec<u64> = obs
                .iter()
                .filter_map(|o| of(o, *cue))
                .filter(|c| c.layer.is_some())
                .map(|c| c.frame)
                .collect();
            frames.dedup();
            let n = frames.len() as u64;
            if n >= *at_least {
                Ok(format!(
                    "{} showed {n} different pictures",
                    name(show, *cue)
                ))
            } else {
                Failed(format!(
                    "{} showed {n} different pictures, not the {at_least} or more it should",
                    name(show, *cue)
                ))
            }
        }
        Check::Sound { cue, secs } => {
            if !ctx.sound {
                return NotChecked("there is no sound device to play on".into());
            }
            let heard = obs
                .iter()
                .filter_map(|o| of(o, *cue).and_then(|c| c.sound))
                .fold(0.0f64, f64::max);
            if heard >= *secs {
                Ok(format!(
                    "the mixer took {heard:.1} s of {}'s sound",
                    name(show, *cue)
                ))
            } else {
                Failed(format!(
                    "the mixer took only {heard:.1} s of {}'s sound (at least {secs:.1} s should play)",
                    name(show, *cue)
                ))
            }
        }
        Check::EndedBy { cue, by } => match (first_started(obs, *cue), last_seen(obs, *cue)) {
            (None, _) => Failed(format!(
                "{} never started, so it never ended",
                name(show, *cue)
            )),
            (_, Some(_)) if obs.last().is_some_and(|o| of(o, *cue).is_some()) => Failed(format!(
                "{} was still playing when its part was over",
                name(show, *cue)
            )),
            (_, Some(t)) if t <= *by => Ok(format!("{} ended after {t:.1} s", name(show, *cue))),
            (_, Some(t)) => Failed(format!(
                "{} was still playing after {t:.1} s (it should be over by {by:.1} s)",
                name(show, *cue)
            )),
            (Some(_), None) => Failed(format!("{} was lost sight of", name(show, *cue))),
        },
        Check::NotBefore { cue, before } => {
            match (first_started(obs, *cue), last_seen(obs, *cue)) {
                (Some(_), Some(t)) if t >= *before => {
                    Ok(format!("{} played for its length", name(show, *cue)))
                }
                (Some(_), Some(t)) => Failed(format!(
                    "{} was over after only {t:.1} s (it should last past {before:.1} s)",
                    name(show, *cue)
                )),
                _ => Failed(format!("{} never started", name(show, *cue))),
            }
        }
        Check::PlayingAt { cue, at } => match near(obs, *at).and_then(|o| of(o, *cue)) {
            Some(_) => Ok(format!("{} was playing at {at:.1} s", name(show, *cue))),
            None => Failed(format!("{} was not playing at {at:.1} s", name(show, *cue))),
        },
        Check::IdleAt { cue, at } => match near(obs, *at).and_then(|o| of(o, *cue)) {
            None => Ok(format!(
                "{} was no longer playing at {at:.1} s",
                name(show, *cue)
            )),
            Some(_) => Failed(format!(
                "{} was still playing at {at:.1} s",
                name(show, *cue)
            )),
        },
        Check::OpacityAt { cue, at, want } => match near(obs, *at).and_then(|o| of(o, *cue)) {
            Some(c) if (c.opacity - want).abs() <= 0.06 => Ok(format!(
                "{} was at {:.0}% opacity at {at:.1} s",
                name(show, *cue),
                c.opacity * 100.0
            )),
            Some(c) => Failed(format!(
                "{} was at {:.0}% opacity at {at:.1} s, not {:.0}%",
                name(show, *cue),
                c.opacity * 100.0,
                want * 100.0
            )),
            None => Failed(format!("{} was not playing at {at:.1} s", name(show, *cue))),
        },
        Check::VolumeAt { cue, at, want } => {
            if !ctx.sound {
                return NotChecked("there is no sound device to play on".into());
            }
            match near(obs, *at).and_then(|o| of(o, *cue)) {
                Some(c) if (c.volume - want).abs() <= 1.5 => Ok(format!(
                    "{} was at {:.0} dB at {at:.1} s",
                    name(show, *cue),
                    c.volume
                )),
                Some(c) => Failed(format!(
                    "{} was at {:.0} dB at {at:.1} s, not {want:.0} dB",
                    name(show, *cue),
                    c.volume
                )),
                None => Failed(format!("{} was not playing at {at:.1} s", name(show, *cue))),
            }
        }
        Check::XAt { cue, at, want } => {
            match near(obs, *at)
                .and_then(|o| of(o, *cue))
                .and_then(|c| c.layer)
            {
                Some((_, x, _)) if (x - want).abs() <= 0.03 => Ok(format!(
                    "{}'s picture had moved to {x:.2} across at {at:.1} s",
                    name(show, *cue)
                )),
                Some((_, x, _)) => Failed(format!(
                    "{}'s picture was at {x:.2} across at {at:.1} s, not {want:.2}",
                    name(show, *cue)
                )),
                None => Failed(format!("{} had no picture at {at:.1} s", name(show, *cue))),
            }
        }
        Check::Held { cue, from, to } | Check::Moving { cue, from, to } => {
            let at = |t: f64| near(obs, t).and_then(|o| of(o, *cue)).map(|c| c.elapsed);
            let (Some(a), Some(b)) = (at(*from), at(*to)) else {
                return Failed(format!(
                    "{} was not playing from {from:.1} to {to:.1} s",
                    name(show, *cue)
                ));
            };
            let moved = b - a;
            let span = to - from;
            match check {
                Check::Held { .. } if moved.abs() < 0.1 => Ok(format!(
                    "{} stood still while it was paused",
                    name(show, *cue)
                )),
                Check::Held { .. } => Failed(format!(
                    "{} went on {moved:.2} s while it should have been paused",
                    name(show, *cue)
                )),
                _ if moved > span * 0.5 => {
                    Ok(format!("{} went on once it was resumed", name(show, *cue)))
                }
                _ => Failed(format!(
                    "{} moved only {moved:.2} s in {span:.1} s after it was resumed",
                    name(show, *cue)
                )),
            }
        }
        Check::Passes { cue, at_least } => {
            let most = obs
                .iter()
                .filter_map(|o| of(o, *cue).and_then(|c| c.pass))
                .max();
            match most {
                Some(n) if n >= *at_least => {
                    Ok(format!("{} went round {n} times", name(show, *cue)))
                }
                Some(n) => Failed(format!(
                    "{} went round {n} times, not {at_least}",
                    name(show, *cue)
                )),
                None => Failed(format!("{} never played as a loop", name(show, *cue))),
            }
        }
        Check::Length { cue, want } => {
            let seen = obs
                .iter()
                .filter_map(|o| of(o, *cue))
                .filter(|c| c.started)
                .find_map(|c| c.duration);
            match seen {
                Some(d) if (d - want).abs() <= want * 0.15 + 0.1 => {
                    Ok(format!("{} runs {d:.1} s", name(show, *cue)))
                }
                Some(d) => Failed(format!(
                    "{} runs {d:.1} s, not the {want:.1} s it should",
                    name(show, *cue)
                )),
                None => Failed(format!("{} never said how long it runs", name(show, *cue))),
            }
        }
        Check::Overlap { a, b } => {
            let both = obs
                .iter()
                .any(|o| of(o, *a).is_some() && of(o, *b).is_some_and(|c| c.started));
            if both {
                Ok(format!(
                    "{} came in while {} was still showing",
                    name(show, *b),
                    name(show, *a)
                ))
            } else {
                Failed(format!(
                    "{} and {} were never showing together, so there was no crossfade",
                    name(show, *a),
                    name(show, *b)
                ))
            }
        }
        Check::After { a, b } => match (first_started(obs, *a), first_started(obs, *b)) {
            (Some(ta), Some(tb)) if tb > ta => {
                Ok(format!("{} came after {}", name(show, *b), name(show, *a)))
            }
            (Some(_), Some(_)) => Failed(format!(
                "{} did not come after {}",
                name(show, *b),
                name(show, *a)
            )),
            _ => Failed(format!(
                "{} and {} did not both play",
                name(show, *a),
                name(show, *b)
            )),
        },
        Check::Together { cues } => {
            let places = |o: &Obs| -> Option<Vec<usize>> {
                cues.iter()
                    .map(|c| of(o, *c).and_then(|c| c.layer).map(|l| l.2))
                    .collect()
            };
            let seen: Vec<Vec<usize>> = obs.iter().filter_map(places).collect();
            if seen.is_empty() {
                Failed("they were never all showing at once".into())
            } else if seen.iter().all(|p| p.windows(2).all(|w| w[0] < w[1])) {
                Ok(format!(
                    "all {} were showing at once, in the order of their layers",
                    cues.len()
                ))
            } else {
                Failed("they were showing at once, but not in the order of their layers".into())
            }
        }
        Check::PlayheadAt { cue, at } => match near(obs, *at).map(|o| o.playhead) {
            Some(Some(p)) if p == *cue => Ok(format!(
                "the playhead was on {} at {at:.1} s",
                name(show, *cue)
            )),
            _ => Failed(format!(
                "the playhead was not on {} at {at:.1} s",
                name(show, *cue)
            )),
        },
        Check::ArmedAt { cue, at, armed } => {
            let Some(o) = near(obs, *at) else {
                return Failed("nothing was noted".into());
            };
            let is = !o.disarmed.contains(cue);
            let word = |a: bool| if a { "armed" } else { "disarmed" };
            if is == *armed {
                Ok(format!(
                    "{} was {} at {at:.1} s",
                    name(show, *cue),
                    word(is)
                ))
            } else {
                Failed(format!(
                    "{} was {} at {at:.1} s, not {}",
                    name(show, *cue),
                    word(is),
                    word(*armed)
                ))
            }
        }
        Check::ReadyBy { cue, by } => {
            if obs.iter().any(|o| o.t <= *by && o.ready.contains(cue)) {
                Ok(format!("{} was got ready ahead of time", name(show, *cue)))
            } else {
                Failed(format!(
                    "{} was not got ready within {by:.1} s of being asked for",
                    name(show, *cue)
                ))
            }
        }
        Check::Osc { address } => {
            if ctx.osc.iter().any(|a| a == address) {
                Ok(format!("the message to {address} arrived on 127.0.0.1"))
            } else {
                Failed(format!("no message to {address} arrived on 127.0.0.1"))
            }
        }
        Check::Recorded => ctx
            .recorded
            .clone()
            .unwrap_or_else(|| NotChecked(NOT_LOOKED_AT.into())),
        Check::Served => ctx
            .served
            .clone()
            .unwrap_or_else(|| NotChecked(NOT_LOOKED_AT.into())),
    }
}

// ----- Running the parts -------------------------------------------------------------

/// How a part came out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Passed,
    Failed,
    /// Not run, or none of its checks could be looked at.
    Skipped,
}

/// A part after it has run.
#[derive(Clone, Debug)]
pub struct PartResult {
    pub kind: &'static str,
    pub title: &'static str,
    pub outcome: Outcome,
    /// Each check's verdict.
    pub checks: Vec<Verdict>,
    /// Problems the engine reported during the part that it should not have.
    pub problems: Vec<String>,
    /// Problems that were expected, as they were said.
    pub expected: Vec<String>,
    /// Why it was not run.
    pub skipped: Option<String>,
    /// The checks that wait for something after the part (a file to be closed).
    pending: Vec<Check>,
}

impl PartResult {
    #[cfg(test)]
    pub fn for_test(kind: &'static str, outcome: Outcome, checks: Vec<Verdict>) -> PartResult {
        PartResult {
            kind,
            title: "does a thing",
            outcome,
            checks,
            problems: Vec::new(),
            expected: Vec::new(),
            skipped: None,
            pending: Vec::new(),
        }
    }

    fn settle(&mut self) {
        let failed = !self.problems.is_empty()
            || self.checks.iter().any(|c| matches!(c, Verdict::Failed(_)));
        let passed = self.checks.iter().any(|c| matches!(c, Verdict::Ok(_)));
        self.outcome = if self.skipped.is_some() {
            Outcome::Skipped
        } else if failed {
            Outcome::Failed
        } else if passed {
            Outcome::Passed
        } else {
            Outcome::Skipped
        };
    }
}

/// The first address of the OSC messages in a packet (bundles are not sent).
fn osc_address(packet: &[u8]) -> Option<String> {
    let end = packet.iter().position(|b| *b == 0)?;
    let text = std::str::from_utf8(&packet[..end]).ok()?;
    text.starts_with('/').then(|| text.to_string())
}

/// What the runner asks of whoever drives it, for the parts that need the app.
#[derive(Clone, Debug, PartialEq)]
pub enum Ask {
    RecordOn,
    RecordOff,
    ServeOn,
    ServeOff,
}

/// The part under way.
struct Playing {
    /// Which part.
    n: usize,
    /// When its first acts were done.
    fired: Instant,
    /// When its clock started: its lead cue was playing, or was given up on.
    clock: Option<Instant>,
    /// How many of its acts are done.
    done: usize,
    /// How long the lead took to play, or that it never did.
    lead: Option<Verdict>,
}

/// Plays the parts one after another and judges each.
pub struct Runner {
    parts: Vec<Part>,
    osc: Option<std::net::UdpSocket>,
    pub results: Vec<PartResult>,
    now: Option<Playing>,
    next: usize,
    /// When the last part ended (the next waits a moment after it).
    rested: Option<Instant>,
    obs: Vec<Obs>,
    last_note: f64,
    problems: Vec<String>,
    ctx: Context,
    /// Whether the app is there (windows, graphics).
    has_app: bool,
    /// What the driver is to do, in order (see `Ask`).
    pub asks: Vec<Ask>,
}

impl Runner {
    pub fn new(parts: Vec<Part>, osc: Option<std::net::UdpSocket>, has_app: bool) -> Runner {
        Runner {
            parts,
            osc,
            results: Vec::new(),
            now: None,
            next: 0,
            rested: None,
            obs: Vec::new(),
            last_note: -1.0,
            problems: Vec::new(),
            ctx: Context::default(),
            has_app,
            asks: Vec::new(),
        }
    }

    /// The part being played: its number (from 1), how many there are, and
    /// its kind and title.
    pub fn progress(&self) -> (usize, usize, &'static str, &'static str) {
        let n = self.now.as_ref().map_or(self.next, |p| p.n);
        let part = self.parts.get(n.min(self.parts.len().saturating_sub(1)));
        (
            (n + 1).min(self.parts.len()),
            self.parts.len(),
            part.map_or("", |p| p.kind),
            part.map_or("", |p| p.title),
        )
    }

    /// What the recording came to (looked at by the driver once its file is closed).
    pub fn set_recorded(&mut self, result: Verdict, show: &Show) {
        self.ctx.recorded = Some(result);
        self.rejudge(show);
    }

    pub fn set_served(&mut self, result: Verdict, show: &Show) {
        self.ctx.served = Some(result);
        self.rejudge(show);
    }

    /// Whether a recording or a stream is still to be looked at by the driver.
    pub fn awaits(&self, check: &Check) -> bool {
        self.results.iter().any(|r| r.pending.contains(check))
    }

    /// Judge again the checks that were waiting for the driver.
    fn rejudge(&mut self, show: &Show) {
        for r in &mut self.results {
            if r.pending.is_empty() {
                continue;
            }
            let mut still = Vec::new();
            for check in std::mem::take(&mut r.pending) {
                let looked = match check {
                    Check::Recorded => self.ctx.recorded.is_some(),
                    Check::Served => self.ctx.served.is_some(),
                    _ => true,
                };
                if !looked {
                    still.push(check);
                    continue;
                }
                let v = judge(&check, &[], &self.ctx, show);
                // In the place of the "not looked at" it left when the part ended.
                if let Some(slot) = r
                    .checks
                    .iter_mut()
                    .find(|c| matches!(c, Verdict::NotChecked(why) if why == NOT_LOOKED_AT))
                {
                    *slot = v;
                } else {
                    r.checks.push(v);
                }
            }
            r.pending = still;
            r.settle();
        }
    }

    /// End the check where it is: the part under way is judged on what was
    /// seen of it, and the rest are listed as not run.
    pub fn abandon(&mut self, show: &Show, engine: &mut Engine, why: &str) {
        if self.now.is_some() {
            self.end_part(show, engine, Some(why));
        }
        while self.next < self.parts.len() {
            let p = &self.parts[self.next];
            self.results.push(PartResult {
                kind: p.kind,
                title: p.title,
                outcome: Outcome::Skipped,
                checks: Vec::new(),
                problems: Vec::new(),
                expected: Vec::new(),
                skipped: Some(p.skip.clone().unwrap_or_else(|| why.to_string())),
                pending: Vec::new(),
            });
            self.next += 1;
        }
    }

    fn end_part(&mut self, show: &Show, engine: &mut Engine, cut_short: Option<&str>) {
        let Some(playing) = self.now.take() else {
            return;
        };
        let part = &self.parts[playing.n];
        let (expected, problems): (Vec<String>, Vec<String>) = std::mem::take(&mut self.problems)
            .into_iter()
            .partition(|said| part.expected.iter().any(|e| said.contains(e)));
        let mut pending = Vec::new();
        let mut checks: Vec<Verdict> = playing.lead.into_iter().collect();
        checks.extend(part.checks.iter().map(|c| {
            let v = judge(c, &self.obs, &self.ctx, show);
            let waits = match c {
                Check::Recorded => self.ctx.recorded.is_none(),
                Check::Served => self.ctx.served.is_none(),
                _ => false,
            };
            if waits {
                pending.push(c.clone());
            }
            v
        }));
        if let Some(why) = cut_short {
            // What had not happened yet is not a failure of the cue.
            for c in &mut checks {
                if let Verdict::Failed(what) = c {
                    *c = Verdict::NotChecked(format!("{why} before this could be seen ({what})"));
                }
            }
            pending.clear();
        }
        let mut result = PartResult {
            kind: part.kind,
            title: part.title,
            outcome: Outcome::Skipped,
            checks,
            problems,
            expected,
            skipped: None,
            pending,
        };
        result.settle();
        self.results.push(result);
        engine.stop_all(0.0);
        engine.set_paused(false);
        engine.unload_all();
        self.obs.clear();
        self.ctx.osc.clear();
        self.rested = Some(Instant::now());
    }

    fn act(&mut self, act: &Act, show: &Show, engine: &mut Engine) {
        match act {
            Act::Fire(id) => engine.fire(show, *id),
            Act::GoAt(id) => {
                engine.playhead = Some(*id);
                engine.go(show);
            }
            Act::Go => engine.go(show),
            Act::RecordOn => self.asks.push(Ask::RecordOn),
            Act::RecordOff => self.asks.push(Ask::RecordOff),
            Act::ServeOn => self.asks.push(Ask::ServeOn),
            Act::ServeOff => self.asks.push(Ask::ServeOff),
        }
    }

    /// Why the next part is not to be run, if it isn't.
    fn skip_reason(&self, part: &Part) -> Option<String> {
        part.skip
            .clone()
            .or_else(|| {
                (part.needs_app && !self.has_app).then(|| {
                    "not run without the app's windows: it needs the graphics card and a display's picture"
                        .to_string()
                })
            })
            .or_else(|| {
                (part.sound_only && !self.ctx.sound)
                    .then(|| "there is no sound device to play on".to_string())
            })
    }

    /// One look. `problems` is what the engine reported since the last look.
    /// Returns true once every part has been played.
    pub fn tick(
        &mut self,
        show: &Show,
        engine: &mut Engine,
        now: Instant,
        problems: Vec<String>,
    ) -> bool {
        self.ctx.sound = engine.audio.available();
        if let Some(sock) = &self.osc {
            let mut buf = [0u8; 2048];
            while let Ok((n, _)) = sock.recv_from(&mut buf) {
                if let Some(a) = osc_address(&buf[..n]) {
                    self.ctx.osc.push(a);
                }
            }
        }
        if self.now.is_some() {
            self.problems.extend(problems);
        }
        let Some(mut playing) = self.now.take() else {
            // Between parts: the next begins once the last one's cues are gone.
            if self.next >= self.parts.len() {
                return true;
            }
            let since = self
                .rested
                .map_or(f64::MAX, |t| now.duration_since(t).as_secs_f64());
            if since < GAP || (engine.is_active() && since < 3.0) {
                return false;
            }
            if let Some(why) = self.skip_reason(&self.parts[self.next]) {
                let part = &self.parts[self.next];
                self.results.push(PartResult {
                    kind: part.kind,
                    title: part.title,
                    outcome: Outcome::Skipped,
                    checks: Vec::new(),
                    problems: Vec::new(),
                    expected: Vec::new(),
                    skipped: Some(why),
                    pending: Vec::new(),
                });
                self.next += 1;
                return self.next >= self.parts.len();
            }
            engine.playhead = None;
            let n = self.next;
            self.next += 1;
            self.last_note = -1.0;
            // What the part begins with is done at once; the rest wait for its clock.
            let first: Vec<Act> = self.parts[n]
                .acts
                .iter()
                .take_while(|(at, _)| *at <= 0.0)
                .map(|(_, a)| a.clone())
                .collect();
            for a in &first {
                self.act(a, show, engine);
            }
            // What the part fires later is got ready now, so that it starts
            // when it is fired however long FFmpeg takes to start here (the
            // part's times are about the cues, not about that).
            for (_, act) in self.parts[n].acts.iter().skip(first.len()) {
                if let Act::Fire(id) | Act::GoAt(id) = act
                    && show.cue(*id).is_some_and(|c| c.kind.preloads())
                {
                    engine.load_ahead(show, *id);
                }
            }
            self.now = Some(Playing {
                n,
                fired: now,
                clock: None,
                done: first.len(),
                lead: None,
            });
            return false;
        };
        let part = self.parts[playing.n].clone();
        // The clock starts when the lead cue has its first picture or sound.
        let Some(clock) = playing.clock else {
            let waited = now.duration_since(playing.fired).as_secs_f64();
            let lead_name = part.lead.map(|l| name(show, l));
            let up = part
                .lead
                .is_none_or(|l| engine.active.iter().any(|a| a.cue_id == l && a.started));
            // (And what it fires later is ready, or has had as long to be.)
            let ready = part.acts.iter().all(|(at, act)| match act {
                Act::Fire(id) | Act::GoAt(id) if *at > 0.0 => {
                    !show.cue(*id).is_some_and(|c| c.kind.preloads()) || engine.is_prepared(*id)
                }
                _ => true,
            });
            if up && !ready && waited < START {
                self.now = Some(playing);
                return false;
            }
            if up {
                playing.clock = Some(now);
                playing.lead = lead_name.map(|n| {
                    Verdict::Ok(format!("{n} was playing {waited:.1} s after it was fired"))
                });
            } else if waited >= START {
                playing.clock = Some(now);
                playing.lead = lead_name.map(|n| {
                    Verdict::Failed(format!(
                        "{n} was not playing {START:.0} s after it was fired"
                    ))
                });
            }
            self.now = Some(playing);
            return false;
        };
        let t = now.duration_since(clock).as_secs_f64();
        while let Some((at, act)) = part.acts.get(playing.done) {
            if *at > t {
                break;
            }
            self.act(act, show, engine);
            playing.done += 1;
        }
        // Noted every 20 ms: often enough for a half-second fade, and a part
        // of ten seconds is five hundred notes.
        if t - self.last_note >= 0.02 {
            self.last_note = t;
            self.obs.push(observe(show, engine, t));
        }
        // Over when its time is up, or sooner once everything it does is done
        // and nothing is left playing.
        let all_done = playing.done >= part.acts.len();
        let early = all_done && t >= part.at_least.max(0.3) && !engine.is_active();
        self.now = Some(playing);
        if t >= part.secs || early {
            // As it is at the very end, whenever the last note was.
            self.obs.push(observe(show, engine, t));
            self.end_part(show, engine, None);
        }
        false
    }
}

// ----- The account of it -----------------------------------------------------------

/// The kinds of cue, in the order of the report, with how each came out:
/// passed, failed, and not run.
pub fn by_kind(results: &[PartResult]) -> Vec<(&'static str, usize, usize, usize)> {
    let mut kinds: Vec<(&'static str, usize, usize, usize)> = Vec::new();
    for r in results {
        let at = match kinds.iter().position(|k| k.0 == r.kind) {
            Some(i) => i,
            None => {
                kinds.push((r.kind, 0, 0, 0));
                kinds.len() - 1
            }
        };
        match r.outcome {
            Outcome::Passed => kinds[at].1 += 1,
            Outcome::Failed => kinds[at].2 += 1,
            Outcome::Skipped => kinds[at].3 += 1,
        }
    }
    kinds
}

/// The part of the report that says what each kind of cue did.
pub fn account(results: &[PartResult]) -> String {
    let mut out = String::new();
    let mut line = |s: &str| {
        out.push_str(s);
        out.push('\n');
    };
    line("What each kind of cue did");
    line("-------------------------");
    for (kind, passed, failed, skipped) in by_kind(results) {
        let word = if failed > 0 {
            "FAILED"
        } else if passed > 0 {
            "ok"
        } else {
            "not run"
        };
        let mut counts = Vec::new();
        if passed > 0 {
            counts.push(format!("{passed} passed"));
        }
        if failed > 0 {
            counts.push(format!("{failed} failed"));
        }
        if skipped > 0 {
            counts.push(format!("{skipped} not run"));
        }
        line(&format!("  {word:8} {kind:18} {}", counts.join(", ")));
    }
    line("");
    line("Each part");
    line("---------");
    for r in results {
        let word = match r.outcome {
            Outcome::Passed => "ok",
            Outcome::Failed => "FAILED",
            Outcome::Skipped => "not run",
        };
        line(&format!("{word:8} {}: {}", r.kind, r.title));
        if let Some(why) = &r.skipped {
            line(&format!("           ({why})"));
        }
        for c in &r.checks {
            match c {
                Verdict::Ok(what) => line(&format!("           ok       {what}")),
                Verdict::Failed(what) => line(&format!("           FAILED   {what}")),
                Verdict::NotChecked(why) => line(&format!("           not seen {why}")),
            }
        }
        for p in &r.problems {
            line(&format!("           FAILED   NewQ reported: {p}"));
        }
        for p in &r.expected {
            line(&format!("           as expected, NewQ said: {p}"));
        }
    }
    out
}

/// Findings for the verdict: a problem for each part that failed, a note for
/// what was not run.
pub fn findings(results: &[PartResult]) -> Vec<super::analysis::Finding> {
    use super::analysis::{Finding, Severity};
    let mut out = Vec::new();
    for r in results.iter().filter(|r| r.outcome == Outcome::Failed) {
        let mut what: Vec<String> = r
            .checks
            .iter()
            .filter_map(|c| match c {
                Verdict::Failed(w) => Some(w.clone()),
                _ => None,
            })
            .collect();
        what.extend(r.problems.iter().map(|p| format!("NewQ reported: {p}")));
        out.push(Finding {
            severity: Severity::Problem,
            text: format!("{} ({}): {}.", r.kind, r.title, what.join("; ")),
        });
    }
    // What was not run, by why: one line for each reason, however many parts.
    let mut reasons: Vec<(String, Vec<&'static str>)> = Vec::new();
    for r in results.iter().filter(|r| r.outcome == Outcome::Skipped) {
        let why = r
            .skipped
            .clone()
            .unwrap_or_else(|| "nothing of it could be seen".into());
        let why = why.strip_prefix("not run: ").unwrap_or(&why).to_string();
        match reasons.iter_mut().find(|(w, _)| *w == why) {
            Some((_, kinds)) => {
                if !kinds.contains(&r.kind) {
                    kinds.push(r.kind);
                }
            }
            None => reasons.push((why, vec![r.kind])),
        }
    }
    for (why, kinds) in reasons {
        out.push(Finding {
            severity: Severity::Note,
            text: format!("Not run: {} ({why}).", kinds.join(", ")),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(t: f64, cues: Vec<CueObs>) -> Obs {
        Obs {
            t,
            cues,
            ..Default::default()
        }
    }

    fn playing(cue: u64, elapsed: f64) -> CueObs {
        CueObs {
            cue: CueId(cue),
            instance: cue,
            started: true,
            elapsed,
            opacity: 1.0,
            layer: Some((1.0, 0.0, 0)),
            frame: (elapsed * 30.0) as u64 + 1,
            sound: Some(elapsed),
            pass: Some((elapsed.floor() as u32) + 1),
            duration: Some(4.0),
            ..Default::default()
        }
    }

    /// A cue that loads for half a second, plays four and is gone.
    fn a_clip() -> Vec<Obs> {
        (0..300)
            .map(|n| {
                let t = f64::from(n) * 0.02;
                let cues = if t < 0.5 {
                    vec![CueObs {
                        cue: CueId(1),
                        instance: 1,
                        ..Default::default()
                    }]
                } else if t < 4.5 {
                    vec![playing(1, t - 0.5)]
                } else {
                    Vec::new()
                };
                note(t, cues)
            })
            .collect()
    }

    fn sound() -> Context {
        Context {
            sound: true,
            ..Default::default()
        }
    }

    fn ok(c: &Check, obs: &[Obs], ctx: &Context) -> bool {
        matches!(judge(c, obs, ctx, &Show::default()), Verdict::Ok(_))
    }

    fn failed(c: &Check, obs: &[Obs], ctx: &Context) -> bool {
        matches!(judge(c, obs, ctx, &Show::default()), Verdict::Failed(_))
    }

    #[test]
    fn a_clip_that_plays_and_ends_passes_and_one_that_does_not_fails() {
        let (obs, cue, ctx) = (a_clip(), CueId(1), sound());
        assert!(ok(&Check::Started { cue, by: 1.0 }, &obs, &ctx));
        assert!(failed(&Check::Started { cue, by: 0.2 }, &obs, &ctx), "late");
        assert!(failed(
            &Check::Started {
                cue: CueId(9),
                by: 1.0
            },
            &obs,
            &ctx
        ));
        assert!(ok(&Check::Picture { cue, opacity: 0.95 }, &obs, &ctx));
        assert!(ok(&Check::Frames { cue, at_least: 100 }, &obs, &ctx));
        assert!(failed(&Check::Frames { cue, at_least: 500 }, &obs, &ctx));
        assert!(ok(&Check::Sound { cue, secs: 3.5 }, &obs, &ctx));
        assert!(failed(&Check::Sound { cue, secs: 5.0 }, &obs, &ctx));
        assert!(ok(&Check::EndedBy { cue, by: 4.7 }, &obs, &ctx));
        assert!(
            failed(&Check::EndedBy { cue, by: 3.0 }, &obs, &ctx),
            "it ran on"
        );
        assert!(ok(&Check::NotBefore { cue, before: 4.0 }, &obs, &ctx));
        assert!(
            failed(&Check::NotBefore { cue, before: 5.0 }, &obs, &ctx),
            "cut short"
        );
        assert!(ok(&Check::PlayingAt { cue, at: 2.0 }, &obs, &ctx));
        assert!(failed(&Check::PlayingAt { cue, at: 5.0 }, &obs, &ctx));
        assert!(ok(&Check::IdleAt { cue, at: 5.0 }, &obs, &ctx));
        assert!(failed(&Check::IdleAt { cue, at: 2.0 }, &obs, &ctx));
        assert!(ok(&Check::Passes { cue, at_least: 4 }, &obs, &ctx));
        assert!(failed(&Check::Passes { cue, at_least: 6 }, &obs, &ctx));
        assert!(ok(&Check::Length { cue, want: 4.0 }, &obs, &ctx));
        assert!(failed(&Check::Length { cue, want: 2.0 }, &obs, &ctx));
        assert!(ok(
            &Check::Moving {
                cue,
                from: 1.0,
                to: 2.0
            },
            &obs,
            &ctx
        ));
        assert!(failed(
            &Check::Held {
                cue,
                from: 1.0,
                to: 2.0
            },
            &obs,
            &ctx
        ));
        // A cue still there when the part is over has not ended, whatever the limit.
        let cut: Vec<Obs> = obs.iter().filter(|o| o.t < 3.0).cloned().collect();
        assert!(failed(&Check::EndedBy { cue, by: 3.2 }, &cut, &ctx));
    }

    #[test]
    fn a_picture_that_never_reaches_the_display_or_stays_dim_fails() {
        let ctx = sound();
        let mut dim = playing(1, 1.0);
        dim.layer = Some((0.4, 0.0, 0));
        let obs = vec![note(1.0, vec![dim])];
        assert!(failed(
            &Check::Picture {
                cue: CueId(1),
                opacity: 0.95
            },
            &obs,
            &ctx
        ));
        assert!(ok(
            &Check::Picture {
                cue: CueId(1),
                opacity: 0.3
            },
            &obs,
            &ctx
        ));
        let mut unseen = playing(1, 1.0);
        unseen.layer = None;
        let obs = vec![note(1.0, vec![unseen])];
        assert!(failed(
            &Check::Picture {
                cue: CueId(1),
                opacity: 0.3
            },
            &obs,
            &ctx
        ));
        assert!(failed(
            &Check::Frames {
                cue: CueId(1),
                at_least: 1
            },
            &obs,
            &ctx
        ));
    }

    #[test]
    fn fades_are_judged_by_where_the_levels_are_at_a_moment() {
        let ctx = sound();
        let at = |t: f64, opacity: f32, volume: f32, x: f32| {
            let mut c = playing(1, t);
            c.opacity = opacity;
            c.volume = volume;
            c.layer = Some((opacity, x, 0));
            note(t, vec![c])
        };
        let obs = vec![at(0.5, 1.0, -6.0, 0.0), at(2.5, 0.3, -30.0, 0.25)];
        let cue = CueId(1);
        assert!(ok(
            &Check::OpacityAt {
                cue,
                at: 2.6,
                want: 0.3
            },
            &obs,
            &ctx
        ));
        assert!(failed(
            &Check::OpacityAt {
                cue,
                at: 0.4,
                want: 0.3
            },
            &obs,
            &ctx
        ));
        assert!(ok(
            &Check::VolumeAt {
                cue,
                at: 2.5,
                want: -30.0
            },
            &obs,
            &ctx
        ));
        assert!(failed(
            &Check::VolumeAt {
                cue,
                at: 2.5,
                want: -6.0
            },
            &obs,
            &ctx
        ));
        assert!(ok(
            &Check::XAt {
                cue,
                at: 2.5,
                want: 0.25
            },
            &obs,
            &ctx
        ));
        assert!(failed(
            &Check::XAt {
                cue,
                at: 0.5,
                want: 0.25
            },
            &obs,
            &ctx
        ));
        // With no sound device the sound's checks are not failures.
        let quiet = Context::default();
        let v = judge(
            &Check::VolumeAt {
                cue,
                at: 2.5,
                want: -6.0,
            },
            &obs,
            &quiet,
            &Show::default(),
        );
        assert!(matches!(v, Verdict::NotChecked(_)), "{v:?}");
        let v = judge(
            &Check::Sound { cue, secs: 1.0 },
            &obs,
            &quiet,
            &Show::default(),
        );
        assert!(matches!(v, Verdict::NotChecked(_)), "{v:?}");
    }

    #[test]
    fn order_overlap_and_layers_are_judged_from_who_was_there_together() {
        let ctx = sound();
        let layered = |cue: u64, place: usize| {
            let mut c = playing(cue, 1.0);
            c.layer = Some((1.0, 0.0, place));
            c
        };
        let obs = vec![
            note(0.5, vec![layered(1, 0)]),
            note(1.5, vec![layered(1, 0), layered(2, 1)]),
            note(2.5, vec![layered(2, 0), layered(3, 1)]),
            note(3.5, vec![layered(1, 0), layered(2, 1), layered(3, 2)]),
        ];
        let (a, b, c) = (CueId(1), CueId(2), CueId(3));
        assert!(ok(&Check::Overlap { a, b }, &obs, &ctx));
        assert!(ok(&Check::After { a, b }, &obs, &ctx));
        assert!(failed(&Check::After { a: b, b: a }, &obs, &ctx));
        assert!(ok(
            &Check::Together {
                cues: vec![a, b, c]
            },
            &obs,
            &ctx
        ));
        assert!(
            failed(
                &Check::Together {
                    cues: vec![c, b, a]
                },
                &obs,
                &ctx
            ),
            "wrong order"
        );
        assert!(failed(
            &Check::Together {
                cues: vec![a, CueId(8)]
            },
            &obs,
            &ctx
        ));
        assert!(failed(&Check::Overlap { a, b: CueId(8) }, &obs, &ctx));
    }

    #[test]
    fn the_playhead_arming_readiness_and_messages_are_judged() {
        let mut o = note(1.0, Vec::new());
        o.playhead = Some(CueId(4));
        o.disarmed = vec![CueId(5)];
        o.ready = vec![CueId(6)];
        let obs = vec![o];
        let ctx = Context {
            osc: vec!["/newq/fullcheck".into()],
            sound: true,
            recorded: Some(Verdict::Failed("no file was written".into())),
            served: None,
        };
        assert!(ok(
            &Check::PlayheadAt {
                cue: CueId(4),
                at: 1.0
            },
            &obs,
            &ctx
        ));
        assert!(failed(
            &Check::PlayheadAt {
                cue: CueId(5),
                at: 1.0
            },
            &obs,
            &ctx
        ));
        assert!(ok(
            &Check::ArmedAt {
                cue: CueId(5),
                at: 1.0,
                armed: false
            },
            &obs,
            &ctx
        ));
        assert!(ok(
            &Check::ArmedAt {
                cue: CueId(4),
                at: 1.0,
                armed: true
            },
            &obs,
            &ctx
        ));
        assert!(failed(
            &Check::ArmedAt {
                cue: CueId(5),
                at: 1.0,
                armed: true
            },
            &obs,
            &ctx
        ));
        assert!(ok(
            &Check::ReadyBy {
                cue: CueId(6),
                by: 2.0
            },
            &obs,
            &ctx
        ));
        assert!(failed(
            &Check::ReadyBy {
                cue: CueId(6),
                by: 0.5
            },
            &obs,
            &ctx
        ));
        assert!(ok(
            &Check::Osc {
                address: "/newq/fullcheck".into()
            },
            &obs,
            &ctx
        ));
        assert!(failed(
            &Check::Osc {
                address: "/other".into()
            },
            &obs,
            &ctx
        ));
        assert!(failed(&Check::Recorded, &obs, &ctx));
        let v = judge(&Check::Served, &obs, &ctx, &Show::default());
        assert!(matches!(v, Verdict::NotChecked(_)));
        assert_eq!(
            osc_address(b"/newq/fullcheck\0,i\0\0\0\0\0\x07").as_deref(),
            Some("/newq/fullcheck")
        );
        assert_eq!(osc_address(b"#bundle\0"), None);
        assert_eq!(osc_address(b"no end"), None);
    }

    fn media_for_plan() -> Media {
        Media {
            clip: "clip.mp4".into(),
            hap: None,
            tone: "tone.wav".into(),
            stills: vec!["a.png".into(), "b.png".into(), "c.png".into()],
            notes: Vec::new(),
        }
    }

    #[test]
    fn the_check_uses_every_kind_of_cue_and_reaches_nowhere_outside() {
        let mut original = Show::default();
        let out = original.add_display();
        // The user's show streams somewhere and sends feedback: the copy must not.
        if let Some(o) = original.outputs.iter_mut().find(|o| o.id == out) {
            o.stream = Some(StreamOut {
                enabled: true,
                kind: StreamKind::Udp {
                    host: "203.0.113.9".into(),
                    port: 5000,
                },
                ..StreamOut::default()
            });
        }
        original.remote.osc_feedback = "203.0.113.9:9000".into();
        let before = serde_json::to_string(&original).unwrap();
        let plan = build(&original, &media_for_plan(), Err("no server here".into()));
        assert_eq!(
            serde_json::to_string(&original).unwrap(),
            before,
            "the show is only read"
        );
        assert!(plan.show.outputs.iter().all(|o| o.stream.is_none()));
        assert!(plan.show.remote.osc_feedback.is_empty());
        // Every kind of cue there is, but live sound input.
        let has = |f: &dyn Fn(&CueKind) -> bool| plan.show.cues.iter().any(|c| f(&c.kind));
        assert!(has(&|k| matches!(k, CueKind::Video(_))));
        assert!(has(&|k| matches!(k, CueKind::Image(_))));
        assert!(has(&|k| matches!(k, CueKind::Audio(_))));
        assert!(has(&|k| matches!(k, CueKind::Fade(_))));
        assert!(has(&|k| matches!(k, CueKind::Stop(_))));
        assert!(has(&|k| matches!(k, CueKind::Wait(_))));
        assert!(has(&|k| matches!(k, CueKind::Group(_))));
        assert!(has(&|k| matches!(k, CueKind::Text(_))));
        assert!(has(&|k| matches!(k, CueKind::Shape(_))));
        assert!(has(&|k| matches!(k, CueKind::Camera(_))));
        assert!(has(&|k| matches!(k, CueKind::Send(_))));
        assert!(has(&|k| matches!(k, CueKind::Control(_))));
        assert!(has(&|k| matches!(k, CueKind::Timecode(_))));
        assert!(
            !has(&|k| matches!(k, CueKind::LiveAudio(_))),
            "no microphone is opened"
        );
        // Every control action is used.
        for action in ControlAction::ALL {
            assert!(
                has(&|k| matches!(k, CueKind::Control(c) if c.action == action)),
                "{action:?}"
            );
        }
        // Clock and timer text, a slide show of each kind, both kinds of group.
        assert!(has(
            &|k| matches!(k, CueKind::Text(t) if matches!(t.source, TextSource::Clock { .. }))
        ));
        assert!(has(
            &|k| matches!(k, CueKind::Text(t) if matches!(t.source, TextSource::Timer { .. }))
        ));
        assert!(has(
            &|k| matches!(k, CueKind::Group(g) if g.slides.is_some_and(|s| s.advance == Advance::OnGo))
        ));
        assert!(has(
            &|k| matches!(k, CueKind::Group(g) if g.slides.is_some_and(|s| s.advance == Advance::Automatic))
        ));
        assert!(has(
            &|k| matches!(k, CueKind::Group(g) if g.mode == GroupMode::Timeline)
        ));
        // Every effect there is, on something.
        for kind in [
            EffectKind::Blur,
            EffectKind::Sharpen,
            EffectKind::Vignette,
            EffectKind::Pixelate,
            EffectKind::Negative,
            EffectKind::Eq,
            EffectKind::Filter,
            EffectKind::Compressor,
            EffectKind::Echo,
            EffectKind::Reverb,
        ] {
            assert!(
                has(&|k| k
                    .media()
                    .is_some_and(|m| m.effects.iter().any(|e| e.kind == kind))),
                "{kind:?}"
            );
        }
        // Nothing is sent anywhere but this computer, and no device is named.
        for c in &plan.show.cues {
            match &c.kind {
                CueKind::Send(s) => match &s.target {
                    SendTarget::Osc(o) => assert_eq!(o.host, "127.0.0.1"),
                    other => panic!("a send that is not OSC to loopback: {other:?}"),
                },
                CueKind::Timecode(t) => assert_eq!(t.port, None),
                CueKind::Camera(cam) => assert_eq!(cam.device, TEST_PATTERN_DEVICE),
                _ => {}
            }
        }
        // The parts: live sound and the server are listed as not run, with why.
        let skipped: Vec<&str> = plan
            .parts
            .iter()
            .filter(|p| p.skip.is_some())
            .map(|p| p.kind)
            .collect();
        assert!(
            skipped.contains(&"Live audio") && skipped.contains(&"Send out"),
            "{skipped:?}"
        );
        assert!(plan.secs > 60.0 && plan.secs < 240.0, "{}", plan.secs);
        // Every cue a part acts on or checks is in the show.
        for p in &plan.parts {
            for (_, act) in &p.acts {
                if let Act::Fire(id) | Act::GoAt(id) = act {
                    assert!(plan.show.cue(*id).is_some(), "{}", p.title);
                }
            }
            assert!(
                p.acts.windows(2).all(|w| w[0].0 <= w[1].0),
                "{}: acts in order",
                p.title
            );
        }
        // The show it makes is one NewQ can save and open again.
        let json = serde_json::to_string(&plan.show).unwrap();
        let again: Show = serde_json::from_str(&json).unwrap();
        assert_eq!(again.cues.len(), plan.show.cues.len());
    }

    #[test]
    fn the_account_groups_the_parts_by_kind_of_cue() {
        let part = |kind, outcome, checks: Vec<Verdict>, skipped: Option<&str>| PartResult {
            kind,
            title: "does a thing",
            outcome,
            checks,
            problems: Vec::new(),
            expected: Vec::new(),
            skipped: skipped.map(str::to_string),
            pending: Vec::new(),
        };
        let results = vec![
            part(
                "Video",
                Outcome::Passed,
                vec![Verdict::Ok("it played".into())],
                None,
            ),
            part(
                "Video",
                Outcome::Failed,
                vec![Verdict::Failed("it never started".into())],
                None,
            ),
            part(
                "Audio",
                Outcome::Passed,
                vec![Verdict::Ok("it sounded".into())],
                None,
            ),
            part(
                "Live audio",
                Outcome::Skipped,
                Vec::new(),
                Some("no microphone is opened"),
            ),
        ];
        assert_eq!(
            by_kind(&results),
            vec![
                ("Video", 1, 1, 0),
                ("Audio", 1, 0, 0),
                ("Live audio", 0, 0, 1)
            ]
        );
        let text = account(&results);
        assert!(text.contains("FAILED   Video"), "{text}");
        assert!(text.contains("ok       Audio"), "{text}");
        assert!(text.contains("not run  Live audio"), "{text}");
        assert!(text.contains("FAILED   it never started"), "{text}");
        assert!(text.contains("(no microphone is opened)"), "{text}");
        let found = findings(&results);
        assert_eq!(found.len(), 2);
        assert!(
            found[0]
                .text
                .starts_with("Video (does a thing): it never started")
        );
        assert_eq!(
            found[1].text,
            "Not run: Live audio (no microphone is opened)."
        );
        // Many parts not run for one reason are one note, not a page of them.
        let stopped: Vec<PartResult> = ["Image", "Text", "Text", "Shape"]
            .into_iter()
            .map(|k| {
                part(
                    k,
                    Outcome::Skipped,
                    Vec::new(),
                    Some("the test was stopped"),
                )
            })
            .collect();
        let found = findings(&stopped);
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].text,
            "Not run: Image, Text, Shape (the test was stopped)."
        );
    }

    #[test]
    fn a_part_passes_only_if_something_was_seen_and_nothing_was_wrong() {
        let mut r = PartResult {
            kind: "Audio",
            title: "plays",
            outcome: Outcome::Skipped,
            checks: vec![Verdict::NotChecked("no device".into())],
            problems: Vec::new(),
            expected: vec!["as expected".into()],
            skipped: None,
            pending: Vec::new(),
        };
        r.settle();
        assert_eq!(r.outcome, Outcome::Skipped, "nothing could be seen");
        r.checks.push(Verdict::Ok("it started".into()));
        r.settle();
        assert_eq!(r.outcome, Outcome::Passed);
        r.problems.push("Cue 3: audio failed".into());
        r.settle();
        assert_eq!(
            r.outcome,
            Outcome::Failed,
            "the engine said something went wrong"
        );
    }

    /// The check's media cut from a real film and pictures, and plain ones in
    /// the place of a film with no sound.
    #[test]
    fn the_media_is_made_from_a_film_and_pictures_when_there_are_some() {
        let Some((ff, picture)) = crate::media::test_media("color_red.png") else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("newq_sources_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cancel = AtomicBool::new(false);
        // A 26 s "film" with sound, as the trailer is (the cut starts at 20 s).
        let film = dir.join("film.mp4");
        let h264 = crate::soak::h264_args(|n| ff.has_encoder(n));
        let mut args: Vec<&str> = vec![
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=480x200:rate=24",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=500:sample_rate=44100",
            "-t",
            "26",
        ];
        args.extend(h264.iter().map(String::as_str));
        args.extend(["-c:a", "aac", "-shortest"]);
        run_ffmpeg(&ff, &args, &film, &cancel).expect("the film is made");
        let from = Sources {
            film: Some(film.clone()),
            stills: vec![picture.clone(); 3],
        };
        let media = make_media(&ff, &dir.join("real"), &cancel, &from).expect("made");
        assert!(
            media.notes.iter().all(|n| !n.contains("film")),
            "{:?}",
            media.notes
        );
        let clip = ff.probe(&media.clip).unwrap();
        let v = clip.video.as_ref().expect("a picture");
        assert_eq!((v.width, v.height), (1280, 720));
        assert!(clip.has_audio && clip.audio_channels == 2);
        let length = clip.duration.unwrap_or(0.0);
        assert!((length - 4.0).abs() < 0.3, "{length}");
        assert_eq!(media.stills.len(), 3);
        for still in &media.stills {
            let p = ff.probe(still).unwrap();
            let v = p.video.as_ref().expect("a picture");
            assert_eq!((v.width, v.height), (1280, 720));
        }
        // A film with no sound can't stand in for a clip with sound.
        let silent = dir.join("silent.mp4");
        let mut args: Vec<&str> = vec![
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=480x200:rate=24",
            "-t",
            "26",
        ];
        args.extend(h264.iter().map(String::as_str));
        run_ffmpeg(&ff, &args, &silent, &cancel).expect("the silent film is made");
        let from = Sources {
            film: Some(silent),
            stills: vec![picture; 2],
        };
        let media = make_media(&ff, &dir.join("plain"), &cancel, &from).expect("made");
        assert!(
            media.notes.iter().any(|n| n.contains("film")),
            "{:?}",
            media.notes
        );
        let clip = ff.probe(&media.clip).unwrap();
        let v = clip.video.as_ref().expect("a picture");
        assert_eq!((v.width, v.height), (640, 360), "the test pattern");
        assert!(clip.has_audio);
        assert_eq!(media.stills.len(), 3, "plain pictures for want of three");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The whole check, played for real through an engine with the silent mixer.
    /// Ignored in `cargo test`: CI runs the same check as its own step
    /// (`newq --self-test`), and playing it twice per job doubles 90 s of timing
    /// on slow shared runners. Run it here with `--ignored`.
    #[test]
    #[ignore = "CI runs the same check as `newq --self-test`"]
    fn the_whole_check_passes_when_played_through_the_engine() {
        let Some((ff, _)) = crate::media::test_media("tone_5s.wav") else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("newq_fullcheck_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cancel = AtomicBool::new(false);
        let media = make_media(&ff, &dir, &cancel, &Sources::default()).expect("the media is made");
        // A show from before layers were kept apart: the check plays the same.
        let original = Show {
            own_layers: false,
            ..Show::default()
        };
        let (results, _) = super::super::selftest::play(&ff, &original, &media, None);
        let text = account(&results);
        let failed: Vec<&PartResult> = results
            .iter()
            .filter(|r| r.outcome == Outcome::Failed)
            .collect();
        assert!(failed.is_empty(), "{text}");
        // Everything that can run without a window did.
        let passed = results
            .iter()
            .filter(|r| r.outcome == Outcome::Passed)
            .count();
        assert!(passed >= 20, "{passed} passed\n{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
