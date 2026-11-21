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
    /// A timeline test: what the session's driver does in a lap.
    pub timeline: Option<super::timeline::Script>,
    /// The 3D stage part: what it plays on (`build_stage`).
    pub stage: Option<super::stage::Setup>,
}

struct Builder<'a> {
    show: Show,
    res: &'a HashMap<&'static str, Resolved>,
    audio: Option<AudioOutputId>,
    notes: Vec<String>,
    used: Vec<&'static str>,
    /// The cue list new cues go in.
    list: u32,
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
        c.list = self.list;
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

/// A monitor the user chose to show the test on.
#[derive(Clone, Debug, PartialEq)]
pub struct DisplayChoice {
    /// What the choice is remembered by (see `Prefs::test_displays`).
    pub key: String,
    /// What a display assigned to it keeps to find it again.
    pub reference: crate::show::DisplayRef,
    /// How many look-alike monitors there are (see `OutputConfig::twins`).
    pub twins: u32,
    /// What to call it on screen.
    pub label: String,
    /// The computer of the show network it is on (None: this one).
    pub computer: Option<crate::show::NodeId>,
}

/// Where the test's sound goes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SoundChoice {
    /// The name of the audio device to play on; None leaves the show's own audio
    /// outputs (and, with none, the system's default device).
    pub device: Option<String>,
    /// Play with the sound off: everything is decoded, mixed and put through its
    /// effects as usual, and the device is sent silence.
    pub muted: bool,
}

impl SoundChoice {
    /// For the system report and the log.
    pub fn describe(&self) -> String {
        let on = match &self.device {
            Some(d) => format!("on {d}"),
            None => {
                "on the audio outputs of your show (the system's default device if it has none)"
                    .into()
            }
        };
        if self.muted {
            format!("muted on purpose (the mixing still runs); it would play {on}")
        } else {
            format!("plays {on}")
        }
    }
}

/// Give the show the audio device that was chosen, in place of its own outputs.
pub(super) fn use_sound(show: &mut Show, sound: &SoundChoice) {
    let Some(device) = &sound.device else {
        return;
    };
    show.audio_outputs.clear();
    let id = show.add_audio_output();
    if let Some(o) = show.audio_outputs.iter_mut().find(|o| o.id == id) {
        o.device = Some(device.clone());
    }
}

/// Give the show the displays that were chosen, in place of its own: the first
/// shows the picture, the others show the whole of the first's picture too
/// (a display that is "part of" another with the whole area is a mirror of it).
/// With none chosen the show keeps the displays it has. Done before the cues
/// are made, since a new cue is sent to the show's first display. The 3D
/// stage is kept, pointed at the test's displays (`carry_stage`); what was
/// done to it is said in the notes returned, for the report.
pub(super) fn use_displays(show: &mut Show, displays: &[DisplayChoice]) -> Vec<String> {
    use_displays_as(show, displays, true)
}

/// `use_displays`; with `mirror` false each display after the first has a
/// picture of its own (the timeline tests show clips "also on" it).
pub(super) fn use_displays_as(
    show: &mut Show,
    displays: &[DisplayChoice],
    mirror: bool,
) -> Vec<String> {
    let Some(first) = displays.first() else {
        return unshow_stage(show).notes();
    };
    let old: Vec<(OutputId, Option<OutputId>)> =
        show.outputs.iter().map(|o| (o.id, o.within)).collect();
    show.outputs.clear();
    // The user's multiview names displays that are gone: the test's are shown in turn.
    show.multiview = None;
    let main = show.add_display();
    let assign = |show: &mut Show, id: crate::show::OutputId, d: &DisplayChoice| {
        if let Some(o) = show.outputs.iter_mut().find(|o| o.id == id) {
            o.monitor = Some(d.reference.clone());
            o.twins = d.twins;
            o.computer = d.computer;
            if d.computer.is_some() {
                o.name = d.label.clone();
            }
        }
    };
    assign(show, main, first);
    let mut new = vec![main];
    for d in &displays[1..] {
        let id = show.add_display();
        new.push(id);
        assign(show, id, d);
        if !mirror {
            continue;
        }
        if let Some(o) = show.outputs.iter_mut().find(|o| o.id == id) {
            o.part_of = Some(crate::show::PartOf {
                display: main,
                area: [0.0, 0.0, 1.0, 1.0],
            });
        }
    }
    let carried = carry_stage(show, &old, &new);
    // Anything still naming a display that has gone shows nothing.
    show.forget_missing_feeds();
    carried.notes()
}

/// What the 3D stage was pointed at for a test, said in its report.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Carried {
    /// Screens that showed one of the user's displays and show a test display now.
    screens: usize,
    /// Projectors that were one of the user's displays and are a test display now.
    projectors: usize,
    /// Projectors left with none: another came to the same test display first.
    left_out: usize,
    /// With no monitor ticked: the displays that showed the stage and show
    /// the test's pictures now.
    unstaged: Vec<String>,
}

impl Carried {
    fn notes(&self) -> Vec<String> {
        let count = |n: usize, one: &str| match n {
            1 => format!("1 {one}"),
            n => format!("{n} {one}s"),
        };
        let mut v = Vec::new();
        if self.screens + self.projectors > 0 {
            v.push(format!(
                "Your 3D stage is kept: {} and {} show the test's displays in place of yours, \
                 in order. A screen that showed part of a picture shows the same part, which \
                 can look cropped on a monitor of another shape",
                count(self.screens, "screen"),
                count(self.projectors, "projector"),
            ));
        }
        if self.left_out > 0 {
            v.push(format!(
                "{} of your 3D stage throw no picture in the test: fewer displays are tested \
                 than your show has, and a display is one projector at most",
                count(self.left_out, "projector"),
            ));
        }
        if !self.unstaged.is_empty() {
            let names: Vec<&str> = self.unstaged.iter().map(String::as_str).collect();
            let one = names.len() == 1;
            v.push(format!(
                "{} {} your 3D stage; in the test {} the test's pictures instead, and so do \
                 the screens in the 3D stage that show {}",
                super::choice::and_list(&names),
                if one { "shows" } else { "show" },
                if one { "it shows" } else { "they show" },
                if one { "it" } else { "them" },
            ));
        }
        v
    }
}

/// Point the 3D stage's screens and projectors at the test's displays `new`
/// (in order) in place of the user's `old` (each display's id and, for a sub
/// display, the display it is in; in the show's order): the user's displays
/// in order take the test's in order, any past the last the first, and a sub
/// display its display's. A screen keeps the region of the picture it
/// showed. A display is one projector at most: the first projector to come
/// to one keeps it, a later one is left with none. Pure.
pub(super) fn carry_stage(
    show: &mut Show,
    old: &[(OutputId, Option<OutputId>)],
    new: &[OutputId],
) -> Carried {
    let tops: Vec<OutputId> = old
        .iter()
        .filter(|(_, within)| within.is_none())
        .map(|(id, _)| *id)
        .collect();
    let target = |id: OutputId| -> Option<OutputId> {
        let top = match old.iter().find(|(o, _)| *o == id)? {
            (_, Some(display)) => *display,
            (o, None) => *o,
        };
        let i = tops.iter().position(|t| *t == top)?;
        new.get(i).or(new.first()).copied()
    };
    let mut carried = Carried::default();
    for s in &mut show.stage3d.surfaces {
        if let crate::stage3d::Feed::Display { output, .. } = &mut s.feed {
            match target(*output) {
                Some(t) => {
                    *output = t;
                    carried.screens += 1;
                }
                None => s.feed = crate::stage3d::Feed::None,
            }
        }
    }
    let mut taken = Vec::new();
    for p in &mut show.stage3d.projectors {
        let Some(o) = p.display else { continue };
        p.display = target(o).filter(|t| !taken.contains(t));
        match p.display {
            Some(t) => {
                taken.push(t);
                carried.projectors += 1;
            }
            None => carried.left_out += 1,
        }
    }
    carried
}

/// With no monitor ticked the test's pictures go to the show's own displays
/// (the first, and those a timeline's clips are also on): one that shows the
/// 3D stage would show only the stage (and the screens it feeds would be
/// grey), so for the test none does.
fn unshow_stage(show: &mut Show) -> Carried {
    let mut carried = Carried::default();
    let staged: Vec<OutputId> = show
        .outputs
        .iter()
        .filter(|o| o.source.is_some())
        .map(|o| o.id)
        .collect();
    for id in staged {
        // Named only where it took effect (a sub display's never does).
        let shown = show.shows_stage(id) && show.whole_picture(id) == id;
        if let Some(o) = show.outputs.iter_mut().find(|o| o.id == id) {
            o.source = None;
            if shown {
                carried.unstaged.push(o.name.clone());
            }
        }
    }
    carried
}

/// Owner's decision 9: across a show network a test leaves the venue model
/// file out (only the 3D stage part takes it), so the other computers don't
/// fetch it, and its files, before the test can start. A sample venue is no
/// file: kept. The note for the report, when it was left out.
pub fn leave_venue_out(show: &mut Show) -> Option<String> {
    crate::relink::venue_model(show)?;
    show.stage3d.venue = None;
    Some(
        "Your 3D stage's venue model is left out of a test across the show network, so the \
         other computers don't have to fetch it first: the 3D views show the screens without \
         the room (only the 3D stage part takes the model with it)"
            .into(),
    )
}

/// Across a show network: every picture shown on the test's first display
/// is shown on each of the other displays too (`MediaCue::also_on`), in place
/// of the mirrors `use_displays` makes. Each display is its own window on its
/// own computer, each drawing the same picture, decoded once per computer.
/// Done once the test show is built (its displays from `use_displays_as`
/// without mirrors).
pub fn spread_pictures(show: &mut Show) {
    let pictures: Vec<crate::show::OutputId> = show
        .outputs
        .iter()
        .filter(|o| o.enabled && !o.is_sub_display() && o.part_of.is_none())
        .map(|o| o.id)
        .collect();
    let Some(&first) = pictures.first() else {
        return;
    };
    for c in &mut show.cues {
        let Some(m) = c.kind.media_mut() else {
            continue;
        };
        if m.output.is_none_or(|o| o == first) {
            m.also_on = pictures[1..].to_vec();
        }
    }
}

/// The show for the decoding comparison: `tiles` copies of one clip playing
/// at once, each in its own part of the picture (a video wall), looping.
pub fn build_decode(
    original: &Show,
    clip: &std::path::Path,
    tiles: usize,
    displays: &[DisplayChoice],
) -> Built {
    let mut show = original.clone();
    show.cues.clear();
    show.lists.clear();
    show.main_list_name = None;
    let notes = use_displays(&mut show, displays);
    show.name = "Test Mode: decoding".into();
    let res = HashMap::new();
    let mut b = Builder {
        show,
        res: &res,
        audio: None,
        notes,
        used: Vec::new(),
        list: 0,
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
        timeline: None,
        stage: None,
    }
}

/// The test show for `level`, built from `original`'s settings.
#[cfg(test)]
pub fn build(
    original: &Show,
    level: Level,
    res: &HashMap<&'static str, Resolved>,
    displays: &[DisplayChoice],
) -> Built {
    build_with(
        original,
        level,
        res,
        displays,
        &SoundChoice::default(),
        false,
    )
}

/// `build`, with the sound going where `sound` says.
#[cfg(test)]
pub fn build_with(
    original: &Show,
    level: Level,
    res: &HashMap<&'static str, Resolved>,
    displays: &[DisplayChoice],
    sound: &SoundChoice,
    heavy: bool,
) -> Built {
    build_with_as(original, level, res, displays, sound, heavy, true)
}

/// `build_with`; with `mirror` false the chosen displays after the first are
/// displays of their own (across a show network, `spread_pictures`).
pub fn build_with_as(
    original: &Show,
    level: Level,
    res: &HashMap<&'static str, Resolved>,
    displays: &[DisplayChoice],
    sound: &SoundChoice,
    heavy: bool,
    mirror: bool,
) -> Built {
    let mut show = original.clone();
    show.cues.clear();
    show.lists.clear();
    show.main_list_name = None;
    let notes = use_displays_as(&mut show, displays, mirror);
    use_sound(&mut show, sound);
    show.name = format!("Test Mode: {}", level.label());
    let audio = show.audio_outputs.first().map(|a| a.id);
    let mut b = Builder {
        show,
        res,
        audio,
        notes,
        used: Vec::new(),
        list: 0,
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
                    ..SlideShow::default()
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
    if heavy {
        // More for the graphics card: whole pictures blended one over another
        // (the cost grows with the pixels covered), each with effects and a
        // changing speed. The lap is no longer.
        for (id, name, layer, opacity, kind) in [
            (
                "bbb_60",
                "Heavy: full screen 0.50, sharpened",
                5i32,
                0.5f32,
                EffectKind::Sharpen,
            ),
            (
                "jelly_30",
                "Heavy: full screen 0.40, vignette",
                6,
                0.4,
                EffectKind::Vignette,
            ),
            (
                "sintel_24",
                "Heavy: full screen 0.35, negative",
                7,
                0.35,
                EffectKind::Negative,
            ),
        ] {
            b.video(id, name, Some(d), |c, r| {
                let m = media(c);
                m.transition = Transition::FadeIn;
                m.loop_media = true;
                m.loop_count = passes(r);
                m.layer = layer;
                m.opacity = opacity;
                m.effects.push(Effect::new(kind));
                m.effects.push(blur_ramp(6.0, layers_secs));
                m.rate.set_key(0.0, 1.0);
                m.rate.set_key(layers_secs / 2.0, 1.4);
                m.rate.set_key(layers_secs, 1.0);
            });
        }
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
        timeline: None,
        stage: None,
    }
}

// ----- The 3D stage part --------------------------------------------------------------

/// The 3D stage part's show (`testmode::stage`): the Short cue list's lap
/// going round, on the ticked monitors (`mirror` as `build_with_as`), with
/// the 3D stage made ready on this copy only: the user's own if it has
/// screens, else the Proscenium sample with its usual screens showing the
/// test's first display and a projector for each test display (as far as
/// two... or the light's limit) aimed at the main screen. The display that
/// shows the stage in the last leg: with two or more monitors ticked here
/// (and `mirror`), the last one's; else a display of its own with no
/// window, off until that leg.
pub fn build_stage(
    original: &Show,
    res: &HashMap<&'static str, Resolved>,
    displays: &[DisplayChoice],
    sound: &SoundChoice,
    mirror: bool,
) -> Built {
    let mut built = build_with_as(original, Level::Short, res, displays, sound, false, mirror);
    round_again(&mut built);
    built.show.name = "Test Mode: 3D stage".into();
    let (setup, notes) = stage_setup(&mut built.show, displays, mirror);
    built.notes.extend(notes);
    built.stage = Some(setup);
    built
}

/// Make a single lap go round: the last cue hands over to one that starts
/// the first again.
fn round_again(built: &mut Built) {
    if built.repeats {
        return;
    }
    if let Some(last) = built.show.cues.last_mut() {
        last.continue_mode = ContinueMode::AutoFollow;
    }
    let mut c = built
        .show
        .make_cue(CueType::Control, None, built.show.cues.len());
    c.name = "Round again".into();
    if let CueKind::Control(ct) = &mut c.kind {
        ct.action = ControlAction::Start;
        ct.target = Some(built.first);
    }
    built.show.cues.push(c);
    built.cues = built.show.cues.len();
    built.repeats = true;
}

/// Most projectors the sample is given (one a test display).
const SAMPLE_PROJECTORS: usize = 4;

/// The 3D stage of the part's show, made ready (see `build_stage`), and
/// what to say of it.
pub(super) fn stage_setup(
    show: &mut Show,
    displays: &[DisplayChoice],
    mirror: bool,
) -> (super::stage::Setup, Vec<String>) {
    use super::stage::{Leg, Setup, StageDisplay};
    use crate::stage3d::projector::LensPreset;
    let mut notes = Vec::new();
    let sample = show.stage3d.surfaces.is_empty();
    // The test's pictures, on the displays that show their own.
    let shown: Vec<OutputId> = show
        .outputs
        .iter()
        .filter(|o| o.enabled && !o.is_sub_display())
        .map(|o| o.id)
        .collect();
    if sample {
        let kind = crate::stage3d::samples::SampleVenue::Proscenium;
        let ids = crate::ui::stage_edit::use_sample(show, kind, true);
        if show.stage3d.projectors.is_empty()
            && let Some(main) = ids.first().and_then(|s| show.stage3d.surface(*s).cloned())
        {
            let n = shown.len().min(SAMPLE_PROJECTORS);
            for (i, d) in shown.iter().take(n).enumerate() {
                let id = show.add_projector();
                let Some(p) = show.projector_mut(id) else {
                    continue;
                };
                p.lens = LensPreset::StandardZoom.lens(None);
                p.lumens = 10_000.0;
                p.display = Some(*d);
                let aspect = 16.0 / 9.0;
                if let Some(mut place) = p.fit_to(&main, aspect) {
                    // Side by side, each aimed at the screen's middle: an
                    // overlapping pair.
                    place.position[0] += (i as f32 - (n as f32 - 1.0) / 2.0) * 0.8;
                    p.placement = place;
                    p.aim_at(main.placement.position, aspect);
                }
            }
        }
    }
    let projectors = show
        .stage3d
        .projectors
        .iter()
        .filter(|p| p.visible && p.display.is_some())
        .count();
    let viewpoint = show.stage3d.viewpoints.first().map(|v| v.name.clone());
    let source = Some(crate::stage3d::StageSource { viewpoint });
    // Here, with two or more ticked: the last one shows the stage.
    let local: Vec<OutputId> = show
        .outputs
        .iter()
        .filter(|o| o.computer.is_none() && o.monitor.is_some() && !o.is_sub_display())
        .map(|o| o.id)
        .collect();
    let stage_display = if !mirror {
        // Across a show network: a display changed in the middle would send
        // the show to every computer again, mid-test. Left out, and said.
        notes.push(
            "Across a show network the 3D stage part has no leg with a display showing the stage: changing a display during the test would send the show to every computer again"
                .into(),
        );
        None
    } else if displays.len() >= 2 && local.len() >= 2 {
        let id = *local.last().expect("two or more");
        let before = show.output(id).cloned().expect("a display of the show");
        let mut during = before.clone();
        during.part_of = None;
        during.source = source;
        let monitor = displays
            .iter()
            .rev()
            .find(|d| d.computer.is_none())
            .map(|d| d.reference.index);
        if show
            .stage3d
            .projectors
            .iter()
            .any(|p| p.display == Some(id))
        {
            notes.push(format!(
                "In the 3D stage part's last leg {} shows the stage, so the projector on it throws no picture then",
                before.name
            ));
        }
        Some(StageDisplay {
            id,
            windowed: true,
            monitor,
            during,
            before,
        })
    } else {
        let id = show.add_display();
        show.outputs.iter_mut().find(|o| o.id == id).map(|o| {
            o.name = "3D stage (test)".into();
            o.monitor = None;
            o.enabled = false;
            let before = o.clone();
            let mut during = o.clone();
            during.enabled = true;
            during.source = source;
            StageDisplay {
                id,
                windowed: false,
                monitor: None,
                during,
                before,
            }
        })
    };
    let mut legs = vec![Leg::Baseline, Leg::View];
    if projectors > 0 {
        legs.push(Leg::Light);
    }
    if stage_display.is_some() {
        legs.push(Leg::Display);
    }
    if sample {
        notes.push(format!(
            "Your show has no 3D stage, so the 3D stage part used the Proscenium sample venue, with {} projector{} (on the test's copy of your show only)",
            projectors,
            if projectors == 1 { "" } else { "s" }
        ));
    }
    let screens = show.stage3d.surfaces.len();
    (
        Setup {
            sample,
            screens,
            projectors,
            stage_display,
            legs,
        },
        notes,
    )
}

// ----- The timeline tests --------------------------------------------------------------

/// The tracks of the timeline tests: films and stills, two picture-in-picture
/// tracks, more for the long test's mosaic, titles on top, and a still under
/// everything on a second display.
const FILMS: i32 = 0;
const INSET: i32 = 1;
const LANES: i32 = 2;
const TITLES: i32 = 8;
const BACKDROP: i32 = -1;

/// Seconds a pause waits for GO before the session presses it.
pub const TIMELINE_HOLD: f64 = 5.0;
/// Passes a loop pause plays before the session leaves it.
pub const TIMELINE_LOOP_PASSES: u32 = 3;

/// A timeline test being built: where its clips go and what its lap takes.
struct Tl {
    /// The group the clips are children of (None: the list's own).
    parent: Option<CueId>,
    /// The display of the second picture, if the test has one.
    second: Option<crate::show::OutputId>,
}

impl Builder<'_> {
    /// A stretch of a film on the timeline: `len` seconds from `from` into it
    /// (less if the file is shorter), filling the display.
    #[allow(clippy::too_many_arguments)]
    fn film(
        &mut self,
        tl: &Tl,
        id: &str,
        name: &str,
        at: f64,
        from: f64,
        len: f64,
        layer: i32,
        tweak: impl FnOnce(&mut MediaCue),
    ) -> Option<CueId> {
        let parent = tl.parent;
        self.video(id, name, parent, |c, r| {
            c.pre_wait = at;
            let m = media(c);
            let len = len.min(r.duration.max(1.0));
            let from = from.min((r.duration - len).max(0.0));
            m.start = from;
            m.end = Some(from + len);
            m.fit = Fit::Cover;
            m.layer = layer;
            m.transition = Transition::Cut;
            m.fade_in = 0.0;
            m.fade_out = 0.0;
            m.volume_db = -12.0;
            tweak(m);
        })
    }

    /// A picture held `len` seconds on the timeline, filling the display.
    #[allow(clippy::too_many_arguments)]
    fn still(
        &mut self,
        tl: &Tl,
        id: &str,
        name: &str,
        at: f64,
        len: f64,
        layer: i32,
        tweak: impl FnOnce(&mut MediaCue),
    ) -> Option<CueId> {
        let r = self.have(id, Kind::Image)?;
        Some(
            self.cue(CueType::Image, Some(r.path), name, tl.parent, |c| {
                c.pre_wait = at;
                let m = media(c);
                m.end = Some(len);
                m.fit = Fit::Cover;
                m.layer = layer;
                // Held through a pause that lets the content run on, until
                // the clock takes it off (a still's own time would run out).
                m.run_out = crate::timeline::RunOut::Hold;
                m.transition = Transition::Cut;
                m.fade_in = 0.0;
                m.fade_out = 0.0;
                tweak(m);
            }),
        )
    }

    /// Words for the audience: `big` centred, else a caption low down.
    fn words(&mut self, tl: &Tl, text: &str, at: f64, len: Option<f64>, big: bool) -> CueId {
        let t = text.to_string();
        self.cue(
            CueType::Text,
            None,
            &format!("Title: {text}"),
            tl.parent,
            |c| {
                c.pre_wait = at;
                if let CueKind::Text(tc) = &mut c.kind {
                    tc.text = t;
                    tc.bold = true;
                    tc.shadow = true;
                    tc.size = if big { 0.085 } else { 0.055 };
                    tc.valign = if big {
                        TextVAlign::Middle
                    } else {
                        TextVAlign::Bottom
                    };
                    tc.media.end = len;
                    tc.media.layer = TITLES;
                    tc.media.transition = Transition::FadeIn;
                    tc.media.fade_in = 1.0;
                    tc.media.fade_out = 1.0;
                }
            },
        )
    }

    /// A control cue on the timeline at `at`.
    fn on_clock(
        &mut self,
        tl: &Tl,
        name: &str,
        at: f64,
        tweak: impl FnOnce(&mut ControlCue),
    ) -> CueId {
        self.cue(CueType::Control, None, name, tl.parent, |c| {
            c.pre_wait = at;
            if let CueKind::Control(k) = &mut c.kind {
                tweak(k);
            }
        })
    }

    /// The sound of the Sintel trailer under a part of the timeline.
    fn bed(
        &mut self,
        tl: &Tl,
        name: &str,
        at: f64,
        from: f64,
        len: f64,
        tweak: impl FnOnce(&mut MediaCue),
    ) -> Option<CueId> {
        self.audio("sintel_sound", name, tl.parent, |c, r| {
            c.pre_wait = at;
            let m = media(c);
            let from = from.min((r.duration - 1.0).max(0.0));
            m.start = from;
            m.end = Some((from + len).min(r.duration.max(from + 1.0)));
            m.fade_in = 2.0;
            m.fade_out = 3.0;
            m.volume_db = -8.0;
            // Through a pause the content runs on, and may run out first.
            m.run_out = crate::timeline::RunOut::Hold;
            tweak(m);
        })
    }

    fn timeline_settings(markers: &[(u64, f64, &str)]) -> crate::timeline::TimelineSettings {
        crate::timeline::TimelineSettings {
            markers: markers
                .iter()
                .map(|&(id, time, name)| crate::timeline::Marker {
                    id,
                    time,
                    name: name.into(),
                    color: None,
                })
                .collect(),
            ..crate::timeline::TimelineSettings::for_editor()
        }
    }
}

/// A picture made small and put to one side.
fn inset(m: &mut MediaCue, scale: f32, x: f32, y: f32) {
    m.fit = Fit::Contain;
    m.transform.scale_x = scale;
    m.transform.scale_y = scale;
    m.transform.x = x;
    m.transform.y = y;
}

/// A slow push in on a still, over `secs`.
fn push_in(m: &mut MediaCue, secs: f64) {
    let mut a = Animated::new(1.0);
    a.set_key(0.0, 1.0);
    a.set_key(secs, 1.12);
    m.motion.scale_x = Some(a.clone());
    m.motion.scale_y = Some(a);
}

/// The timeline test for `level`, built from `original`'s settings: a timeline
/// list that plays like a short programme (films butted and crossfaded, stills
/// under titles, a picture brought up by a fade cue, a pause GO ends, and from
/// Medium on a third track with speed and opacity keyframes, a picture "also
/// on" a second display, a freezing pause over a playing film, a split join, a
/// loop, a jump to a marker; Long adds the heavy films, a mosaic of five with
/// effects, and seeks while it runs), then (Medium and Long) a timeline group
/// in the main list. The session's driver plays it (`Built::timeline`).
pub fn build_timeline(
    original: &Show,
    level: Level,
    res: &HashMap<&'static str, Resolved>,
    displays: &[DisplayChoice],
    sound: &SoundChoice,
) -> Built {
    use crate::show::{PauseFreeze, TimelineKey, TimelinePoint};
    let mut show = original.clone();
    show.cues.clear();
    show.lists.clear();
    show.main_list_name = None;
    let notes = use_displays_as(&mut show, displays, false);
    use_sound(&mut show, sound);
    // Tracks are layers: each replaces only its own (an old show's setting
    // would make every clip replace the whole display).
    show.own_layers = true;
    let title = super::Flavour::Timeline.name(level);
    show.name = format!("Test Mode: {title}");
    let audio = show.audio_outputs.first().map(|a| a.id);
    let pictures: Vec<crate::show::OutputId> = show
        .outputs
        .iter()
        .filter(|o| !o.is_sub_display() && o.part_of.is_none())
        .map(|o| o.id)
        .collect();
    let mut b = Builder {
        show,
        res,
        audio,
        notes,
        used: Vec::new(),
        list: 0,
    };
    let (short, long) = (level == Level::Short, level == Level::Long);
    let hold = TIMELINE_HOLD;
    let mut legs = Vec::new();
    let mut lap = 0.0;

    // The group in the main list first (the main list's cues come first).
    let group = (!short).then(|| {
        let g = b.cue(CueType::Group, None, "Interval", None, |c| {
            if let CueKind::Group(g) = &mut c.kind {
                g.mode = GroupMode::Timeline;
                g.timeline = Some(Builder::timeline_settings(&[]));
            }
        });
        let tl = Tl {
            parent: Some(g),
            second: None,
        };
        b.still(
            &tl,
            "bluemarble",
            "The Blue Marble",
            0.5,
            12.0,
            FILMS,
            |m| {
                m.transition = Transition::FadeIn;
                m.fade_in = 1.5;
                push_in(m, 12.0);
            },
        );
        b.words(&tl, "Interval", 1.0, Some(5.0), true);
        b.on_clock(&tl, "Pause: wait for GO", 6.0, |k| {
            k.action = ControlAction::TimelinePause;
        });
        let mut end = 12.5;
        if long {
            b.still(&tl, "mars", "Mars", 12.5, 8.0, FILMS, |m| push_in(m, 8.0));
            b.still(
                &tl,
                "andromeda",
                "The Andromeda galaxy",
                20.5,
                8.0,
                FILMS,
                |m| push_in(m, 8.0),
            );
            end = 28.5;
        }
        b.film(&tl, "bbb_60", "Big Buck Bunny", end, 1.0, 8.0, FILMS, |m| {
            m.fade_out = 1.0;
        });
        b.words(&tl, "Part two", end + 0.5, Some(5.0), true);
        lap += end + 8.0 + hold;
        g
    });
    legs.extend(group.map(TimelineKey::Group));

    // The timeline list.
    let list = b.show.add_list("Programme");
    b.list = list;
    let key = TimelineKey::List(list);
    let tl = Tl {
        parent: None,
        second: pictures.get(1).copied(),
    };
    legs.insert(0, key);
    let mut markers: Vec<(u64, f64, &str)> = vec![(1, 37.5, "Second half")];

    // 1. A title over three films at 60, 24 and 29.97 frames a second: the
    // first two butted (one held over the join), the third crossfading over
    // the overlap; a still under a caption, and a picture a fade cue brings up.
    b.words(
        &tl,
        &format!("NewQ Test Mode · {title}"),
        0.5,
        Some(5.0),
        true,
    );
    b.film(
        &tl,
        "bbb_60",
        "Big Buck Bunny (60 fps)",
        0.5,
        0.0,
        9.0,
        FILMS,
        |m| {
            m.transition = Transition::FadeIn;
            m.fade_in = 1.5;
        },
    );
    b.film(
        &tl,
        "sintel_24",
        "Sintel (24 fps)",
        9.5,
        0.5,
        9.0,
        FILMS,
        |_| {},
    );
    b.film(
        &tl,
        "jelly_30",
        "Jellyfish (29.97 fps)",
        16.5,
        0.0,
        9.0,
        FILMS,
        |_| {},
    );
    b.still(&tl, "apollo11", "Apollo 11", 25.5, 12.0, FILMS, |m| {
        push_in(m, 12.0)
    });
    b.words(&tl, "Apollo 11 · July 1969", 26.5, Some(7.0), false);
    if let Some(earth) = b.still(
        &tl,
        "bluemarble",
        "The Earth from Apollo 17",
        28.5,
        13.0,
        INSET,
        |m| {
            inset(m, 0.36, 0.27, -0.22);
            // Unseen until the fade cue brings it up.
            m.opacity = 0.0;
        },
    ) {
        b.cue(CueType::Fade, None, "Fade: bring up the Earth", None, |c| {
            c.pre_wait = 29.5;
            if let CueKind::Fade(f) = &mut c.kind {
                f.target = Some(earth);
                f.duration = 2.0;
                f.curve = Curve::SCurve;
                f.opacity = Some(1.0);
                f.volume_db = None;
                f.stop_target = false;
            }
        });
    }
    if !short {
        // The third track: a film inset with its speed and opacity keyframed.
        b.film(
            &tl,
            "bbb_60",
            "Big Buck Bunny, slowing",
            4.5,
            2.0,
            7.0,
            LANES,
            |m| {
                inset(m, 0.3, -0.31, 0.27);
                m.rate.set_key(0.0, 1.0);
                m.rate.set_key(4.0, 0.5);
                m.rate.set_key(8.0, 1.0);
                let mut o = Animated::new(0.0);
                o.set_key(0.0, 0.0);
                o.set_key(1.5, 1.0);
                o.set_key(7.0, 1.0);
                o.set_key(8.5, 0.0);
                m.motion.opacity = Some(o);
            },
        );
    }
    // The pause: the clock stands (the pictures stay) until GO.
    b.on_clock(&tl, "Pause: wait for GO", 34.0, |k| {
        k.action = ControlAction::TimelinePause;
    });
    lap += hold;
    let bed_len = if short { 47.0 } else { 36.0 };
    b.bed(&tl, "Sound: the Sintel trailer", 0.5, 0.0, bed_len, |_| {});

    let end = if short {
        b.film(
            &tl,
            "bbb_60",
            "Big Buck Bunny, again",
            37.5,
            1.0,
            9.0,
            FILMS,
            |_| {},
        );
        b.words(&tl, "Thank you for watching", 43.5, None, true);
        48.0
    } else {
        // 2. A film with its own sound, split in two (a join), held by a pause
        // that freezes it; a still shown on the second display too.
        let trailer = |b: &mut Builder, name: &str, at: f64, from: f64| {
            b.film(&tl, "sintel_trailer", name, at, from, 8.0, FILMS, |m| {
                m.volume_db = -8.0;
            })
        };
        trailer(&mut b, "The Sintel trailer", 37.5, 15.0);
        // The second half goes on from where the first stops.
        let from2 = b
            .show
            .cues
            .last()
            .and_then(|c| c.kind.media())
            .and_then(|m| m.end)
            .unwrap_or(23.0);
        trailer(&mut b, "The Sintel trailer, second half", 45.5, from2);
        b.words(&tl, "Sintel · Blender Foundation", 38.5, Some(5.0), false);
        b.on_clock(&tl, "Pause: hold everything", 42.0, |k| {
            k.action = ControlAction::TimelinePause;
            k.freeze = PauseFreeze::Everything;
        });
        lap += hold;
        let second = tl.second;
        b.still(
            &tl,
            "curiosity",
            "Curiosity on Mars",
            53.5,
            10.0,
            FILMS,
            |m| {
                m.also_on = second.into_iter().collect();
                push_in(m, 10.0);
            },
        );
        b.words(&tl, "Curiosity on Mars", 54.5, Some(7.0), false);
        if tl.second.is_none() {
            b.notes.push(
                "Only one display was used, so the picture meant for a second display too (\"Also on\") was shown on the first alone"
                    .into(),
            );
        }
        markers.push((2, 53.5, "Mars"));

        // 3. A loop played round until GO, then a jump past a part.
        b.film(
            &tl,
            "jelly_30",
            "Jellyfish, round and round",
            63.5,
            0.0,
            9.0,
            FILMS,
            |_| {},
        );
        b.words(&tl, "The Pacific", 64.5, Some(5.0), false);
        b.on_clock(&tl, "Loop: until GO", 70.0, |k| {
            k.action = ControlAction::TimelinePause;
            k.loop_from = Some(TimelinePoint::Time(64.0));
        });
        lap += f64::from(TIMELINE_LOOP_PASSES) * 6.0;
        markers.push((3, 64.0, "Loop"));
        b.film(
            &tl,
            "bbb_60",
            "Big Buck Bunny, cut short",
            72.5,
            0.0,
            9.0,
            FILMS,
            |_| {},
        );
        b.on_clock(&tl, "Jump: past the next part", 74.0, |k| {
            k.action = ControlAction::TimelineJump;
            k.at = Some(TimelinePoint::Marker(4));
        });
        b.film(
            &tl,
            "sintel_24",
            "Sintel, jumped over",
            76.0,
            0.0,
            8.0,
            INSET,
            |m| {
                inset(m, 0.4, 0.0, 0.0);
            },
        );
        markers.push((4, 84.0, "After the jump"));
        lap -= 10.0;
        let at = if long {
            // 4. The heavy part: a 25 Mbit/s film under four insets at once
            // (two H.265), each with effects, then a large picture blurring.
            b.film(
                &tl,
                "sintel_hi",
                "Sintel, 25 Mbit/s",
                84.0,
                0.0,
                9.5,
                FILMS,
                |m| {
                    m.transition = Transition::FadeIn;
                    m.fade_in = 1.0;
                },
            );
            b.still(&tl, "mars", "Mars, blurring", 93.5, 10.5, FILMS, |m| {
                m.effects.push(blur_ramp(18.0, 10.5));
            });
            let corners = [
                (
                    "bbb_hevc",
                    "Big Buck Bunny, H.265",
                    -0.25,
                    -0.25,
                    EffectKind::Sharpen,
                ),
                (
                    "jelly_hevc",
                    "Jellyfish, H.265, negative",
                    0.25,
                    -0.25,
                    EffectKind::Negative,
                ),
                (
                    "bbb_60",
                    "Big Buck Bunny, pixelated",
                    -0.25,
                    0.25,
                    EffectKind::Pixelate,
                ),
                (
                    "jelly_30",
                    "Jellyfish, vignette",
                    0.25,
                    0.25,
                    EffectKind::Vignette,
                ),
            ];
            for (i, (id, name, x, y, fx)) in corners.into_iter().enumerate() {
                for round in 0..2 {
                    let start = 85.0 + f64::from(round) * 9.5;
                    b.film(
                        &tl,
                        id,
                        &format!("{name} ({})", round + 1),
                        start,
                        f64::from(round),
                        9.0,
                        INSET + i as i32,
                        |m| {
                            inset(m, 0.4, x, y);
                            m.transition = Transition::FadeIn;
                            m.fade_in = 0.5;
                            m.fade_out = 0.5;
                            let mut e = Effect::new(fx);
                            if fx == EffectKind::Pixelate {
                                e.p[0].set_key(0.0, 2.0);
                                e.p[0].set_key(4.5, 30.0);
                                e.p[0].set_key(9.0, 2.0);
                            }
                            m.effects.push(e);
                        },
                    );
                }
            }
            b.bed(
                &tl,
                "Sound: the Sintel trailer, through EQ and reverb",
                84.0,
                20.0,
                20.0,
                |m| {
                    m.effects.push(Effect::new(EffectKind::Eq));
                    m.effects.push(Effect::new(EffectKind::Reverb));
                },
            );
            b.words(&tl, "Mosaic", 85.0, Some(5.0), true);
            markers.push((5, 92.0, "Mosaic, middle"));
            markers.push((6, 100.0, "Mosaic, end"));
            104.0
        } else {
            b.film(
                &tl,
                "sintel_24",
                "Sintel, after the jump",
                84.0,
                0.5,
                9.0,
                FILMS,
                |m| {
                    m.transition = Transition::FadeIn;
                    m.fade_in = 1.0;
                },
            );
            b.bed(
                &tl,
                "Sound: the Sintel trailer, closing",
                84.0,
                20.0,
                10.0,
                |_| {},
            );
            93.0
        };
        b.words(&tl, "Thank you for watching", at - 4.0, None, true);
        // The second display's picture, under all of it.
        if let Some(d2) = tl.second {
            b.still(
                &tl,
                "earthrise",
                "Earthrise (second display)",
                0.5,
                at - 0.5,
                BACKDROP,
                |m| {
                    m.output = Some(d2);
                    m.transition = Transition::FadeIn;
                    m.fade_in = 2.0;
                },
            );
        } else {
            // One display: Earthrise as a picture in the picture at the end.
            b.still(&tl, "earthrise", "Earthrise", at - 5.0, 5.5, INSET, |m| {
                inset(m, 0.32, 0.29, -0.24);
                m.transition = Transition::FadeIn;
                m.fade_in = 1.0;
            });
        }
        at + 1.0
    };
    // The end: a Stop that fades the timeline out.
    b.on_clock(&tl, "Stop: fade out the timeline", end, |k| {
        k.action = ControlAction::TimelineStop;
    });
    lap += end + crate::show::TIMELINE_STOP_FADE;
    if let Some(l) = b.show.lists.iter_mut().find(|l| l.id == list) {
        l.timeline = Some(Builder::timeline_settings(&markers));
    }

    // The long test seeks while the list runs: to a marker and back, twice.
    let seeks = if long {
        vec![(20.0, 53.5), (58.0, 22.0), (90.0, 100.0), (102.0, 92.0)]
    } else {
        Vec::new()
    };
    for (from, to) in &seeks {
        lap += from - to;
    }
    let script = super::timeline::Script {
        legs,
        hold,
        loop_passes: if short { 0 } else { TIMELINE_LOOP_PASSES },
        seeks,
    };
    let first = b
        .show
        .cues
        .iter()
        .find(|c| c.list == list)
        .map(|c| c.id)
        .unwrap_or(CueId(0));
    let cues = b.show.cues.len();
    Built {
        show: b.show,
        first,
        repeats: !short,
        lap_secs: lap,
        cues,
        notes: b.notes,
        used: b.used,
        timeline: Some(script),
        stage: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testmode::assets::Source;

    #[test]
    fn a_heavy_lap_adds_blended_full_screen_layers_and_takes_no_longer() {
        let res = all();
        let light = build_with(
            &Show::default(),
            Level::Medium,
            &res,
            &[],
            &SoundChoice::default(),
            false,
        );
        let heavy = build_with(
            &Show::default(),
            Level::Medium,
            &res,
            &[],
            &SoundChoice::default(),
            true,
        );
        assert_eq!(heavy.cues, light.cues + 3);
        assert_eq!(heavy.lap_secs, light.lap_secs);
        let blended = heavy
            .show
            .cues
            .iter()
            .filter(|c| c.name.starts_with("Heavy:"))
            .count();
        assert_eq!(blended, 3);
        assert!(
            light
                .show
                .cues
                .iter()
                .all(|c| !c.name.starts_with("Heavy:"))
        );
    }

    #[test]
    fn the_chosen_audio_device_replaces_the_shows_audio_outputs() {
        let mut original = Show::default();
        original.add_audio_output();
        original.add_audio_output();
        let none = build_with(
            &original,
            Level::Short,
            &HashMap::new(),
            &[],
            &SoundChoice::default(),
            false,
        );
        assert_eq!(none.show.audio_outputs.len(), 2);
        let chosen = build_with(
            &original,
            Level::Short,
            &HashMap::new(),
            &[],
            &SoundChoice {
                device: Some("USB Interface".into()),
                muted: false,
            },
            false,
        );
        assert_eq!(chosen.show.audio_outputs.len(), 1);
        assert_eq!(
            chosen.show.audio_outputs[0].device.as_deref(),
            Some("USB Interface")
        );
        // The user's own show is not touched.
        assert_eq!(original.audio_outputs.len(), 2);
        assert!(original.audio_outputs[0].device.is_none());
    }

    #[test]
    fn the_decoding_wall_starts_all_its_tiles_at_once_and_keeps_them_playing() {
        let Some((ff, clip)) = crate::media::test_media("testsrc_10s.mp4") else {
            return;
        };
        for tiles in [4, 8] {
            let built = build_decode(&Show::default(), &clip, tiles, &[]);
            let mut show = built.show;
            assert_eq!(show.cues.len(), tiles);
            assert_eq!(show.cues[0].id, built.first);
            let mut e = crate::engine::Engine::for_tests();
            e.ff = Some(ff.clone());
            e.playhead = Some(built.first);
            e.go(&show);
            use std::time::{Duration, Instant};
            let every_tile_plays = |e: &crate::engine::Engine| {
                show.cues
                    .iter()
                    .all(|c| matches!(e.status(c.id), crate::engine::CueStatus::Playing { .. }))
            };
            // Until every tile plays: as soon as it does, however long the
            // FFmpegs take to start on a loaded machine (a fixed 5 s once
            // ran out under a parallel test run, the tiles still starting).
            let began = Instant::now();
            let mut last = began;
            let mut tick = |e: &mut crate::engine::Engine| {
                std::thread::sleep(Duration::from_millis(16));
                let now = Instant::now();
                e.tick(&show, now.duration_since(last).as_secs_f64());
                last = now;
            };
            while !every_tile_plays(&e) {
                assert!(
                    began.elapsed() < Duration::from_secs(60),
                    "{tiles} tiles never played together"
                );
                tick(&mut e);
            }
            // Then they keep playing, all of them, for a while.
            let together = Instant::now();
            while together.elapsed() < Duration::from_secs(2) {
                tick(&mut e);
                assert!(every_tile_plays(&e), "{tiles} tiles: one stopped");
            }
            assert!(e.is_active(), "the wall must keep playing");
            e.stop_all(0.0);
            show.cues.clear();
        }
    }

    #[test]
    fn the_wall_is_a_grid_that_fills_the_picture_for_four_and_for_eight_tiles() {
        for (n, cols, rows) in [(4usize, 2.0f32, 2.0f32), (8, 3.0, 3.0), (1, 1.0, 1.0)] {
            let built = build_decode(&Show::default(), std::path::Path::new("x.mp4"), n, &[]);
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

    fn choice(name: &str, i: usize) -> DisplayChoice {
        DisplayChoice {
            computer: None,
            key: name.into(),
            reference: crate::show::DisplayRef {
                name: name.into(),
                index: i,
                hardware: None,
            },
            twins: 1,
            label: name.into(),
        }
    }

    #[test]
    fn the_shows_multiview_layout_goes_with_its_displays() {
        let mut original = Show::default();
        let a = original.add_display();
        original.multiview = Some(crate::multiview::Layout::new(
            crate::multiview::Shape::Grid { cols: 2, rows: 1 },
            &[a],
        ));
        let built = build(
            &original,
            Level::Short,
            &HashMap::new(),
            &[choice("Left", 1)],
        );
        assert_eq!(built.show.multiview, None);
        // With no displays chosen the show's own are kept, and its layout.
        let built = build(&original, Level::Short, &HashMap::new(), &[]);
        assert_eq!(built.show.multiview, original.multiview);
    }

    #[test]
    fn the_chosen_displays_replace_the_shows_and_the_cues_go_to_the_first() {
        let original = Show::default();
        let a = [choice("Left", 1), choice("Right", 2)];
        for built in [
            build(&original, Level::Short, &HashMap::new(), &a),
            build_decode(&original, std::path::Path::new("x.mp4"), 4, &a),
        ] {
            let outs = &built.show.outputs;
            assert_eq!(outs.len(), 2, "the show's own display is not kept");
            assert_eq!(outs[0].monitor.as_ref().unwrap().name, "Left");
            assert!(outs[0].part_of.is_none());
            // The second shows the whole of the first's picture.
            assert_eq!(outs[1].monitor.as_ref().unwrap().name, "Right");
            let part = outs[1].part_of.unwrap();
            assert_eq!(
                (part.display, part.area),
                (outs[0].id, [0.0, 0.0, 1.0, 1.0])
            );
            // Every picture cue is sent to the first.
            let sent: Vec<_> = built
                .show
                .cues
                .iter()
                .filter_map(|c| c.kind.media())
                .filter(|m| m.output.is_some())
                .map(|m| m.output)
                .collect();
            assert!(!sent.is_empty());
            assert!(sent.iter().all(|o| *o == Some(outs[0].id)), "{sent:?}");
        }
        // The user's own show is not touched, and none chosen keeps its displays.
        assert_eq!(original.outputs.len(), 1);
        assert!(original.outputs[0].monitor.is_none());
        let kept = build(&original, Level::Short, &HashMap::new(), &[]);
        assert_eq!(kept.show.outputs.len(), 1);
        assert_eq!(kept.show.outputs[0].id, original.outputs[0].id);
    }

    #[test]
    fn each_test_downloads_exactly_the_media_its_show_asks_for() {
        for level in Level::ALL {
            let built = build(&Show::default(), level, &HashMap::new(), &[]);
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
            let built = build(&original, level, &all(), &[]);
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
        let built = build(&original, Level::Medium, &res, &[]);
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
        let none = build(&Show::default(), Level::Short, &HashMap::new(), &[]);
        assert!(none.cues >= 3 && !none.notes.is_empty());
    }

    #[test]
    fn the_trailers_sound_is_mixed_down_and_its_centre_faded() {
        let built = build(&Show::default(), Level::Medium, &all(), &[]);
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
        let built = build(&Show::default(), Level::Short, &res, &[]);
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

    // ----- The timeline tests ------------------------------------------------------

    fn timeline(level: Level, res: &HashMap<&'static str, Resolved>, d: &[DisplayChoice]) -> Built {
        build_timeline(&Show::default(), level, res, d, &SoundChoice::default())
    }

    #[test]
    fn each_timeline_test_is_a_valid_timeline_list_and_its_group_and_they_grow() {
        use crate::show::{TimelineKey, TimelinePoint};
        use crate::timeline::model;
        let original = Show::default();
        let mut laps = Vec::new();
        for level in Level::ALL {
            for displays in [vec![], vec![choice("Left", 1), choice("Right", 2)]] {
                let built = timeline(level, &all(), &displays);
                let show = &built.show;
                let what = format!("{level:?} with {} displays", displays.len());
                assert!(
                    built.notes.iter().all(|n| n.contains("Only one display")),
                    "{what}: {:?}",
                    built.notes
                );
                // The lists are as a loaded show would have them.
                let mut again = show.clone();
                again.normalize_lists();
                assert_eq!(
                    serde_json::to_string(&again).unwrap(),
                    serde_json::to_string(show).unwrap(),
                    "{what}"
                );
                // One timeline list, never the main one.
                let lists: Vec<_> = show.lists.iter().filter(|l| l.timeline.is_some()).collect();
                assert_eq!(lists.len(), 1, "{what}");
                let list = lists[0].id;
                assert_ne!(list, 0);
                let key = TimelineKey::List(list);
                assert!(model::is_timeline(show, key));
                // No group in it, and its cues are all its top-level clips.
                for c in show.cues.iter().filter(|c| c.list == list) {
                    assert!(c.parent.is_none(), "{what}: {}", c.name);
                    assert!(!matches!(c.kind, CueKind::Group(_)), "{what}: {}", c.name);
                    assert_eq!(model::key_of(show, c.id), Some(key), "{}", c.name);
                }
                // The main list holds the timeline group and its clips alone.
                let main: Vec<&Cue> = show
                    .cues
                    .iter()
                    .filter(|c| c.list == 0 && c.parent.is_none())
                    .collect();
                let script = built.timeline.as_ref().expect("a script");
                if level == Level::Short {
                    assert!(main.is_empty(), "{what}");
                    assert_eq!(script.legs, vec![key]);
                } else {
                    assert_eq!(main.len(), 1, "{what}");
                    let g = main[0].id;
                    assert!(model::is_timeline(show, TimelineKey::Group(g)));
                    assert_eq!(script.legs, vec![key, TimelineKey::Group(g)]);
                    assert!(show.children(g).len() >= 4);
                }
                assert_eq!(built.repeats, level != Level::Short);
                // Every timeline action finds its timeline and its place.
                let mut actions = Vec::new();
                for c in &show.cues {
                    match &c.kind {
                        CueKind::Control(k) if ControlAction::TIMELINE.contains(&k.action) => {
                            actions.push(k.action);
                            let on = k.timeline.or_else(|| model::key_of(show, c.id));
                            let on =
                                on.unwrap_or_else(|| panic!("{what}: {} has no timeline", c.name));
                            assert!(model::is_timeline(show, on));
                            for p in [k.at, k.loop_from].into_iter().flatten() {
                                assert!(
                                    model::resolve(show, on, p).is_some(),
                                    "{what}: {}",
                                    c.name
                                );
                            }
                        }
                        CueKind::Fade(f) => {
                            let t = f.target.and_then(|t| show.cue(t)).expect("a fade's target");
                            assert_eq!(model::key_of(show, t.id), model::key_of(show, c.id));
                        }
                        _ => {}
                    }
                    if let Some(m) = c.kind.media() {
                        for o in m.also_on.iter().chain(m.output.iter()) {
                            assert!(show.outputs.iter().any(|x| x.id == *o), "{}", c.name);
                        }
                    }
                }
                assert!(actions.contains(&ControlAction::TimelinePause), "{what}");
                assert!(actions.contains(&ControlAction::TimelineStop), "{what}");
                if level != Level::Short {
                    assert!(actions.contains(&ControlAction::TimelineJump), "{what}");
                    let looping = show
                        .cues
                        .iter()
                        .any(|c| matches!(&c.kind, CueKind::Control(k) if k.is_loop()));
                    assert!(looping, "{what}");
                }
                // A marker to jump to; the seeks stay on the timeline.
                let settings = lists[0].timeline.as_ref().unwrap();
                assert!(!settings.markers.is_empty());
                assert!(settings.crossfade_overlaps && settings.start_on_fade_in);
                for (when, to) in &script.seeks {
                    assert!(*when > 0.0 && *to > 0.0, "{what}");
                    assert!(
                        model::resolve(show, key, TimelinePoint::Time(*to)).is_some(),
                        "{what}"
                    );
                }
                assert_eq!(script.seeks.is_empty(), level != Level::Long);
                // Pictures also on the second display, when there is one.
                let also = show
                    .cues
                    .iter()
                    .filter_map(|c| c.kind.media())
                    .any(|m| !m.also_on.is_empty());
                assert_eq!(also, level != Level::Short && displays.len() == 2, "{what}");
                if displays.is_empty() {
                    // The user's own displays are kept.
                    assert_eq!(show.outputs.len(), original.outputs.len());
                    assert_eq!(show.outputs[0].id, original.outputs[0].id);
                } else {
                    // Each display its own picture: no mirror.
                    assert_eq!(show.outputs.len(), 2);
                    assert!(show.outputs.iter().all(|o| o.part_of.is_none()));
                }
                assert!(show.own_layers, "tracks are layers");
                // It saves and loads like any show.
                let dir = std::env::temp_dir()
                    .join(format!("newq_tlplan_{level:?}_{}", std::process::id()));
                std::fs::create_dir_all(&dir).unwrap();
                let path = dir.join("t.newq");
                show.save(&path).unwrap();
                assert_eq!(Show::load(&path).unwrap().cues.len(), built.cues);
                let _ = std::fs::remove_dir_all(dir);
                if displays.is_empty() {
                    laps.push((built.cues, built.lap_secs));
                }
            }
        }
        assert!(laps[0].0 < laps[1].0 && laps[1].0 < laps[2].0, "{laps:?}");
        assert!((45.0..=90.0).contains(&laps[0].1), "{laps:?}");
        assert!((100.0..=250.0).contains(&laps[1].1), "{laps:?}");
        assert!((laps[1].1..=350.0).contains(&laps[2].1), "{laps:?}");
    }

    #[test]
    fn each_timeline_test_downloads_exactly_the_media_its_show_asks_for() {
        for level in Level::ALL {
            let two = [choice("Left", 1), choice("Right", 2)];
            for d in [&two[..], &[]] {
                let built = timeline(level, &HashMap::new(), d);
                let wanted: Vec<&str> =
                    crate::testmode::assets::wanted_for(level, crate::testmode::Flavour::Timeline)
                        .iter()
                        .map(|a| a.id)
                        .collect();
                for id in &built.used {
                    assert!(
                        wanted.contains(id),
                        "{level:?} asks for {id}, which it does not download"
                    );
                }
                for id in &wanted {
                    let source_of_used = built
                        .used
                        .iter()
                        .any(|u| find(u).and_then(|a| a.from) == Some(*id));
                    assert!(
                        built.used.contains(id) || source_of_used,
                        "{level:?} downloads {id}, which its show never uses"
                    );
                }
            }
        }
    }

    #[test]
    fn a_timeline_test_goes_on_without_missing_media() {
        let mut res = all();
        res.remove("sintel_24");
        res.remove("apollo11");
        let built = timeline(Level::Medium, &res, &[]);
        assert!(
            built.notes.iter().any(|n| n.contains("Sintel")),
            "{:?}",
            built.notes
        );
        assert!(
            built
                .show
                .cues
                .iter()
                .all(|c| c.kind.media().is_none_or(|m| {
                    m.file.as_os_str().is_empty()
                        || !m.file.ends_with("sintel_24") && !m.file.ends_with("apollo11")
                }))
        );
        // Nothing at all: titles, the pause and the Stop still make a timeline.
        let none = timeline(Level::Short, &HashMap::new(), &[]);
        assert!(!none.notes.is_empty());
        assert!(none.show.cues.iter().any(
            |c| matches!(&c.kind, CueKind::Control(k) if k.action == ControlAction::TimelineStop)
        ));
    }

    // ----- The 3D stage ---------------------------------------------------------

    use crate::stage3d::scene::Paint;
    use crate::stage3d::{Feed, StageSource};

    /// The displays the 3D view paints its screens with, in order (an unfed
    /// screen is grey: not in the list). The test shows have no venue.
    fn painted(show: &Show) -> Vec<OutputId> {
        let scene = show.stage3d.scene(show, None);
        scene
            .parts
            .iter()
            .filter_map(|p| match &p.paint {
                Paint::Output { output, .. } => Some(*output),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn with_no_monitor_ticked_a_first_display_that_showed_the_stage_shows_the_test() {
        let mut original = Show::default();
        let first = original.outputs[0].id;
        original.outputs[0].source = Some(StageSource::default());
        let screen = original.add_surface(None);
        original.stage3d.surface_mut(screen).unwrap().feed = Feed::whole_picture(&original, first);
        assert!(original.shows_stage(first));
        for built in [
            build(&original, Level::Short, &HashMap::new(), &[]),
            build_timeline(
                &original,
                Level::Short,
                &HashMap::new(),
                &[],
                &SoundChoice::default(),
            ),
        ] {
            let show = &built.show;
            // The test's pictures go to the first display...
            let sent = show
                .cues
                .iter()
                .filter_map(|c| c.kind.media())
                .find_map(|m| m.output)
                .unwrap();
            assert_eq!(sent, first);
            // ...which shows them, not the stage, and the screen it fed shows them too.
            assert!(
                !show.shows_stage(sent),
                "the test's pictures are never seen"
            );
            assert_eq!(painted(show), vec![first], "the screen is grey");
            assert!(
                built.notes.iter().any(|n| n.contains("3D stage")),
                "{:?}",
                built.notes
            );
        }
        // The user's show still shows the stage.
        assert!(original.shows_stage(first));
    }

    /// A user's show with `n` displays, a screen fed by each (the whole
    /// picture, but the last a region of it) and a projector on each.
    fn staged(n: usize) -> (Show, Vec<OutputId>) {
        let mut show = Show::default();
        while show.outputs.len() < n {
            show.add_display();
        }
        let ids: Vec<OutputId> = show.outputs.iter().map(|o| o.id).collect();
        for (i, id) in ids.iter().enumerate() {
            let s = show.add_surface(None);
            let region = if i + 1 == n {
                [0.5, 0.0, 0.5, 1.0]
            } else {
                [0.0, 0.0, 1.0, 1.0]
            };
            show.stage3d.surface_mut(s).unwrap().feed = Feed::Display {
                output: *id,
                region,
            };
            let p = show.add_projector();
            show.projector_mut(p).unwrap().display = Some(*id);
        }
        (show, ids)
    }

    fn feeds(show: &Show) -> Vec<Option<OutputId>> {
        show.stage3d
            .surfaces
            .iter()
            .map(|s| s.feed.output())
            .collect()
    }

    fn throws(show: &Show) -> Vec<Option<OutputId>> {
        show.stage3d.projectors.iter().map(|p| p.display).collect()
    }

    /// A test show sanitised as a show sent to other computers is: nothing changes.
    fn stays_sane(show: &Show) {
        let mut again = show.clone();
        again.sanitize_stage();
        let json = |s: &Show| serde_json::to_string(s).unwrap();
        assert_eq!(
            json(&again),
            json(show),
            "sanitize_stage changed the test show"
        );
    }

    #[test]
    fn the_stage_follows_the_ticked_monitors_whether_there_are_more_fewer_or_as_many() {
        let picks = [choice("A", 1), choice("B", 2), choice("C", 3)];
        // As many displays as monitors: each to its own, the region kept.
        let (show, _) = staged(2);
        for mirror in [true, false] {
            let mut test = show.clone();
            let notes = use_displays_as(&mut test, &picks[..2], mirror);
            let new: Vec<OutputId> = test.outputs.iter().map(|o| o.id).collect();
            assert_eq!(feeds(&test), vec![Some(new[0]), Some(new[1])]);
            assert_eq!(throws(&test), vec![Some(new[0]), Some(new[1])]);
            assert!(matches!(
                test.stage3d.surfaces[1].feed,
                Feed::Display {
                    region: [0.5, 0.0, 0.5, 1.0],
                    ..
                }
            ));
            assert_eq!(painted(&test), new, "mirror {mirror}");
            stays_sane(&test);
            assert_eq!(notes.len(), 1, "{notes:?}");
            assert!(notes[0].contains("2 screens and 2 projectors"), "{notes:?}");
        }
        // More monitors than displays: the extra shows nothing of the stage.
        let mut test = show.clone();
        use_displays_as(&mut test, &picks, false);
        let new: Vec<OutputId> = test.outputs.iter().map(|o| o.id).collect();
        assert_eq!(feeds(&test), vec![Some(new[0]), Some(new[1])]);
        assert_eq!(throws(&test), vec![Some(new[0]), Some(new[1])]);
        // More displays than monitors: the rest go to the first; a second
        // projector there throws nothing, said.
        let (show, _) = staged(3);
        let mut test = show.clone();
        let notes = use_displays_as(&mut test, &picks[..1], true);
        let only = test.outputs[0].id;
        assert_eq!(feeds(&test), vec![Some(only); 3]);
        assert_eq!(throws(&test), vec![Some(only), None, None]);
        stays_sane(&test);
        assert!(notes[0].contains("3 screens and 1 projector "), "{notes:?}");
        assert!(notes[1].starts_with("2 projectors"), "{notes:?}");
        // The user's show is as it was.
        let own: Vec<Option<OutputId>> = show.outputs.iter().map(|o| Some(o.id)).collect();
        assert_eq!(throws(&show), own);
    }

    #[test]
    fn a_sub_display_and_a_blended_pair_carry_over_to_the_test_displays() {
        let mut show = Show::default();
        let first = show.outputs[0].id;
        let sub = show.add_sub_display(first);
        let (wide, left, right) = show.add_blended_pair(1920, 1080, 0.1);
        // A screen showing the sub display, one showing the wide picture,
        // projectors on the pair's sides; a screen showing nothing stays so.
        for o in [sub, wide] {
            let s = show.add_surface(None);
            show.stage3d.surface_mut(s).unwrap().feed = Feed::whole_picture(&show, o);
        }
        show.add_surface(None);
        for o in [left, right] {
            let p = show.add_projector();
            show.projector_mut(p).unwrap().display = Some(o);
        }
        // The user's displays in order: first, wide, left, right (the sub
        // display is in the first).
        let picks: Vec<DisplayChoice> = (0..4).map(|i| choice(&format!("M{i}"), i)).collect();
        let mut test = show.clone();
        use_displays_as(&mut test, &picks, false);
        let new: Vec<OutputId> = test.outputs.iter().map(|o| o.id).collect();
        assert_eq!(feeds(&test), vec![Some(new[0]), Some(new[1]), None]);
        assert_eq!(throws(&test), vec![Some(new[2]), Some(new[3])]);
        stays_sane(&test);
        // Two monitors: the pair's sides both come to the first, which is the
        // first projector's only.
        let mut test = show.clone();
        let notes = use_displays_as(&mut test, &picks[..2], true);
        let new: Vec<OutputId> = test.outputs.iter().map(|o| o.id).collect();
        assert_eq!(feeds(&test), vec![Some(new[0]), Some(new[1]), None]);
        assert_eq!(throws(&test), vec![Some(new[0]), None]);
        stays_sane(&test);
        assert!(
            notes.iter().any(|n| n.starts_with("1 projector of")),
            "{notes:?}"
        );
    }

    #[test]
    fn carry_stage_maps_in_order_and_leaves_a_display_to_one_projector() {
        let (mut show, old_ids) = staged(2);
        let old: Vec<(OutputId, Option<OutputId>)> = old_ids.iter().map(|id| (*id, None)).collect();
        // Two projectors on one display of the user's (a hand-edited file):
        // the first keeps it.
        show.stage3d.projectors[1].display = Some(old_ids[0]);
        let (a, b) = (OutputId(900), OutputId(901));
        let carried = carry_stage(&mut show, &old, &[a, b]);
        assert_eq!(
            carried,
            Carried {
                screens: 2,
                projectors: 1,
                left_out: 1,
                unstaged: Vec::new()
            }
        );
        assert_eq!(feeds(&show), vec![Some(a), Some(b)]);
        assert_eq!(throws(&show), vec![Some(a), None]);
        // A feed naming a display that wasn't the user's shows nothing.
        show.stage3d.surfaces[0].feed = Feed::Display {
            output: OutputId(777),
            region: [0.0, 0.0, 1.0, 1.0],
        };
        carry_stage(&mut show, &old, &[a]);
        assert_eq!(feeds(&show)[0], None);
        // A show with no stage has nothing to say.
        let mut plain = Show::default();
        let id = plain.outputs[0].id;
        assert_eq!(
            carry_stage(&mut plain, &[(id, None)], &[a]),
            Carried::default()
        );
        assert!(Carried::default().notes().is_empty());
    }

    #[test]
    fn with_no_monitor_ticked_no_display_shows_the_stage() {
        // The first display is a side of a blended pair whose picture shows
        // the stage: that picture's source goes, and so does another
        // display's (a timeline's clips are also on it).
        let mut show = Show::default();
        show.outputs.clear();
        let (wide, left, _) = show.add_blended_pair(1920, 1080, 0.1);
        let pos = show.outputs.iter().position(|o| o.id == left).unwrap();
        let l = show.outputs.remove(pos);
        show.outputs.insert(0, l);
        show.outputs
            .iter_mut()
            .find(|o| o.id == wide)
            .unwrap()
            .source = Some(StageSource::default());
        let other = show.add_display();
        show.outputs
            .iter_mut()
            .find(|o| o.id == other)
            .unwrap()
            .source = Some(StageSource::default());
        assert!(show.shows_stage(left));
        let mut test = show.clone();
        let notes = use_displays_as(&mut test, &[], true);
        assert!(!test.shows_stage(left) && !test.shows_stage(wide));
        assert!(!test.shows_stage(other), "every display shows the test");
        assert_eq!(notes.len(), 1);
        let other_name = show.output(other).unwrap().name.clone();
        assert_eq!(
            notes[0],
            format!(
                "Wide picture and {other_name} show your 3D stage; in the test they show the \
                 test's pictures instead, and so do the screens in the 3D stage that show them"
            ),
        );
        // One alone is said as one.
        let mut one = show.clone();
        one.outputs
            .iter_mut()
            .find(|o| o.id == other)
            .unwrap()
            .source = None;
        let notes = use_displays_as(&mut one, &[], true);
        assert!(
            notes[0].starts_with("Wide picture shows your 3D stage; in the test it shows"),
            "{notes:?}"
        );
        // A first display that doesn't show the stage: nothing changes.
        let mut plain = Show::default();
        let before = serde_json::to_string(&plain).unwrap();
        assert!(use_displays_as(&mut plain, &[], true).is_empty());
        assert_eq!(serde_json::to_string(&plain).unwrap(), before);
    }

    #[test]
    fn the_3d_stage_part_builds_a_sample_on_the_copy_and_keeps_a_users_stage() {
        use crate::testmode::stage::Leg;
        let picks = [choice("A", 1), choice("B", 2)];
        let original = Show::default();
        let before = serde_json::to_string(&original).unwrap();
        // No stage: the Proscenium sample with its screens, a projector a
        // display (an overlapping pair), the last monitor showing the stage
        // in the last leg.
        let b = build_stage(
            &original,
            &HashMap::new(),
            &picks,
            &SoundChoice::default(),
            true,
        );
        assert_eq!(
            serde_json::to_string(&original).unwrap(),
            before,
            "the user's show"
        );
        let setup = b.stage.as_ref().expect("a stage part");
        assert!(setup.sample && setup.screens > 0);
        assert!(
            b.show
                .stage3d
                .venue
                .as_ref()
                .is_some_and(|v| v.sample.is_some())
        );
        assert_eq!(setup.projectors, 2);
        let throws: Vec<Option<OutputId>> = b
            .show
            .stage3d
            .projectors
            .iter()
            .map(|p| p.display)
            .collect();
        let ids: Vec<OutputId> = b.show.outputs.iter().map(|o| o.id).collect();
        assert_eq!(throws, vec![Some(ids[0]), Some(ids[1])]);
        let first = b.show.stage3d.surfaces[0].feed.output();
        assert_eq!(first, Some(ids[0]), "the screens show the test's picture");
        assert_eq!(
            setup.legs,
            [Leg::Baseline, Leg::View, Leg::Light, Leg::Display]
        );
        let d = setup.stage_display.as_ref().expect("a stage display");
        assert!(d.windowed && d.id == ids[1] && d.monitor == Some(2));
        let mut show = b.show.clone();
        assert!(!show.shows_stage(d.id));
        assert!(setup.apply(&mut show, Leg::Display));
        assert!(show.shows_stage(d.id) && !show.shows_stage(ids[0]));
        assert!(show.stage_source_refused(d.id).is_none());
        assert!(setup.apply(&mut show, Leg::View) && !show.shows_stage(d.id));
        assert!(b.repeats, "the lap goes round");
        assert!(
            b.notes.iter().any(|n| n.contains("Proscenium sample")),
            "{:?}",
            b.notes
        );
        // One monitor: a display of its own, off but in its leg, no window.
        let b = build_stage(
            &original,
            &HashMap::new(),
            &picks[..1],
            &SoundChoice::default(),
            true,
        );
        let setup = b.stage.unwrap();
        assert_eq!(setup.projectors, 1);
        let d = setup.stage_display.as_ref().unwrap();
        assert!(!d.windowed && d.before.monitor.is_none() && !d.before.enabled);
        assert!(d.during.enabled && d.during.source.is_some());
        let mut show = b.show.clone();
        setup.apply(&mut show, Leg::Display);
        assert!(show.shows_stage(d.id));
        // The user's own stage is kept (and pointed at the test's displays):
        // no sample, its projector, no light leg without one throwing.
        let mut users = Show::default();
        let s = users.add_surface(None);
        let o = users.outputs[0].id;
        users.stage3d.surface_mut(s).unwrap().feed = crate::stage3d::Feed::whole_picture(&users, o);
        let b = build_stage(
            &users,
            &HashMap::new(),
            &picks[..1],
            &SoundChoice::default(),
            true,
        );
        let setup = b.stage.unwrap();
        assert!(!setup.sample && setup.screens == 1 && setup.projectors == 0);
        assert!(b.show.stage3d.venue.is_none());
        assert_eq!(setup.legs, [Leg::Baseline, Leg::View, Leg::Display]);
        assert_eq!(
            b.show.stage3d.surfaces[0].feed.output(),
            Some(b.show.outputs[0].id)
        );
        // Across a show network: no display is changed mid-test.
        let b = build_stage(
            &original,
            &HashMap::new(),
            &picks,
            &SoundChoice::default(),
            false,
        );
        let setup = b.stage.unwrap();
        assert!(setup.stage_display.is_none());
        assert_eq!(setup.legs, [Leg::Baseline, Leg::View, Leg::Light]);
    }
}
