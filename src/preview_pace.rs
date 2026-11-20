//! Every preview gives way before the outputs suffer, and only then. The
//! previews come second to the outputs (the projectors), but they still matter:
//! they are drawn as well as they can be, and give way only while the outputs
//! really miss pictures people would see, or the main loop that feeds them
//! stalls. Then they draw less: fewer pictures a second (60, 30, 15, 5), then
//! smaller (half, a quarter of their size), then not at all (paused, the last
//! picture kept); once the outputs are smooth again they step back up, one step
//! every couple of seconds. The outputs' own drawing never reads any of this.
//!
//! What counts is a **seen miss**: an output frame that was late (a refresh
//! missed) *and* carried a new picture (`render::shown_totals`: a layer's
//! texture written again, or a layer faded, moved, coloured; the 3D stage and
//! clocks are written again as they change). A missed refresh while every layer
//! holds the same picture shows nothing different, so it doesn't count: a 25 fps
//! film on a 100 Hz display holds each picture for four refreshes. Misses are
//! judged as a share of the new pictures over `Governor::WINDOW` (3 s), against
//! the computer's `Strictness` (Relaxed 5 %, Strict 1 %, Off never), and the
//! second after a cue starts or a jump lands (`Governor::GRACE`) isn't counted:
//! pausing the previews can't spare the outputs that.
//!
//! One `PreviewGovernor` per app (`UiState::preview_governor`), whatever kind of
//! preview: the Preview pane's single preview and multiview tiles, the timeline
//! workspace's monitor, the same in pop-out windows, and any kind added later
//! (a 3D view). **Every preview asks it** (`UiState::preview_budget`, keyed by
//! a `PreviewId`) each frame it would draw, and draws as the `PreviewBudget`
//! says; one drawn by the compositor hands it to `render::Pace::of`, which keeps
//! the last picture between times.
//!
//! Pure: `Governor::observe` takes what was measured and the time, and says
//! the `Level`; `Limits` are the per-computer ceilings (`Prefs::preview_cap`,
//! `Prefs::preview_size`), `Strictness` how much the outputs may miss
//! (`Prefs::preview_strictness`).

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// How the previews are drawn: each level draws less than the one before.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    /// Every frame the main window draws, full size.
    #[default]
    Full,
    /// 60 a second, full size.
    Fps60,
    /// 30 a second, full size.
    Fps30,
    /// 15 a second, full size.
    Fps15,
    /// 5 a second, full size.
    Fps5,
    /// 5 a second, at half size.
    Half,
    /// 5 a second, at a quarter of the size.
    Quarter,
    /// Not drawn: the last picture stays.
    Paused,
}

impl Level {
    pub const ALL: [Level; 8] = [
        Level::Full,
        Level::Fps60,
        Level::Fps30,
        Level::Fps15,
        Level::Fps5,
        Level::Half,
        Level::Quarter,
        Level::Paused,
    ];

    /// The previews' pictures a second (None: as often as the window draws;
    /// Some(0): none).
    pub fn fps(self) -> Option<u32> {
        match self {
            Level::Full => None,
            Level::Fps60 => Some(60),
            Level::Fps30 => Some(30),
            Level::Fps15 => Some(15),
            Level::Fps5 | Level::Half | Level::Quarter => Some(5),
            Level::Paused => Some(0),
        }
    }

    /// The size a preview is drawn at, against the size it is shown at.
    pub fn scale(self) -> f32 {
        match self {
            Level::Half => 0.5,
            Level::Quarter | Level::Paused => 0.25,
            _ => 1.0,
        }
    }

    /// The least time between two of a preview's pictures (zero at full rate).
    pub fn every(self) -> Duration {
        match self.fps() {
            None => Duration::ZERO,
            // Paused: a day (never `MAX`, which an `Instant` can't be added to).
            Some(0) => Duration::from_secs(86_400),
            Some(f) => Duration::from_secs_f64(1.0 / f64::from(f)),
        }
    }

    fn down(self) -> Level {
        Level::ALL[(self as usize + 1).min(Level::ALL.len() - 1)]
    }

    fn up(self) -> Level {
        Level::ALL[(self as usize).saturating_sub(1)]
    }
}

/// A setting a later NewQ may have more choices for: one this NewQ doesn't
/// know reads as the default, and the rest of the file is kept.
fn lenient<'de, D, T>(d: D, all: &[T]) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Serialize + Copy + Default,
{
    let v = serde_json::Value::deserialize(d)?;
    Ok(all
        .iter()
        .copied()
        .find(|c| serde_json::to_value(c).ok().as_ref() == Some(&v))
        .unwrap_or_default())
}

/// This computer's ceiling on the previews' rate (Settings ▸ General).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub enum Cap {
    /// Full rate unless the outputs need the room. (A limit from a later NewQ
    /// that this one doesn't know reads as this.)
    #[default]
    Automatic,
    Fps60,
    Fps30,
    Fps15,
    Fps5,
}

impl<'de> Deserialize<'de> for Cap {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Cap, D::Error> {
        lenient(d, &Cap::ALL)
    }
}

impl Cap {
    pub const ALL: [Cap; 5] = [
        Cap::Automatic,
        Cap::Fps60,
        Cap::Fps30,
        Cap::Fps15,
        Cap::Fps5,
    ];

    pub fn is_automatic(&self) -> bool {
        *self == Cap::Automatic
    }

    /// The level the previews never go above.
    pub fn level(self) -> Level {
        match self {
            Cap::Automatic => Level::Full,
            Cap::Fps60 => Level::Fps60,
            Cap::Fps30 => Level::Fps30,
            Cap::Fps15 => Level::Fps15,
            Cap::Fps5 => Level::Fps5,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Cap::Automatic => "Automatic",
            Cap::Fps60 => "60 fps at most",
            Cap::Fps30 => "30 fps at most",
            Cap::Fps15 => "15 fps at most",
            Cap::Fps5 => "5 fps at most",
        }
    }

    /// What the choice does, in one sentence.
    pub fn words(self) -> &'static str {
        match self {
            Cap::Automatic => "As often as the window draws, unless the displays need the room.",
            _ => "Never more pictures a second than this, whatever the displays are doing.",
        }
    }
}

/// This computer's ceiling on the size the previews are drawn at (Settings ▸
/// General): every preview, the 3D view too. The governor never goes above it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub enum SizeCap {
    /// As big as they are shown. (One from a later NewQ reads as this.)
    #[default]
    Full,
    Half,
    Quarter,
}

impl<'de> Deserialize<'de> for SizeCap {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<SizeCap, D::Error> {
        lenient(d, &SizeCap::ALL)
    }
}

impl SizeCap {
    pub const ALL: [SizeCap; 3] = [SizeCap::Full, SizeCap::Half, SizeCap::Quarter];

    pub fn is_full(&self) -> bool {
        *self == SizeCap::Full
    }

    /// The largest share of the size shown that a preview is drawn at.
    pub fn scale(self) -> f32 {
        match self {
            SizeCap::Full => 1.0,
            SizeCap::Half => 0.5,
            SizeCap::Quarter => 0.25,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SizeCap::Full => "Full",
            SizeCap::Half => "Half",
            SizeCap::Quarter => "Quarter",
        }
    }

    /// What the choice does, in one sentence.
    pub fn words(self) -> &'static str {
        match self {
            SizeCap::Full => "Previews are drawn as sharp as the screen shows them.",
            SizeCap::Half => {
                "Previews are drawn at half size and scaled up: a little soft, less work."
            }
            SizeCap::Quarter => {
                "Previews are drawn at a quarter of the size: blurry, the least work."
            }
        }
    }
}

/// This computer's ceilings, together (`Prefs::preview_limits`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Limits {
    pub rate: Cap,
    pub size: SizeCap,
}

/// How much the outputs may miss before the previews give way (Settings ▸
/// General ▸ Previews, per computer).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub enum Strictness {
    /// About one new picture in twenty late. (One from a later NewQ reads as this.)
    #[default]
    Relaxed,
    /// About one in a hundred.
    Strict,
    /// The previews never give way.
    Off,
}

impl<'de> Deserialize<'de> for Strictness {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Strictness, D::Error> {
        lenient(d, &Strictness::ALL)
    }
}

impl Strictness {
    pub const ALL: [Strictness; 3] = [Strictness::Relaxed, Strictness::Strict, Strictness::Off];

    pub fn is_relaxed(&self) -> bool {
        *self == Strictness::Relaxed
    }

    /// The share of an output's new pictures that may be late (over
    /// `Governor::WINDOW`) before the previews give way; None: never.
    pub fn share(self) -> Option<f64> {
        match self {
            Strictness::Relaxed => Some(0.05),
            Strictness::Strict => Some(0.01),
            Strictness::Off => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Strictness::Relaxed => "Relaxed",
            Strictness::Strict => "Strict",
            Strictness::Off => "Off",
        }
    }

    /// What the choice does, in one sentence.
    pub fn words(self) -> &'static str {
        match self {
            Strictness::Relaxed => {
                "The previews give way once about 5% of the displays' new pictures come late."
            }
            Strictness::Strict => {
                "The previews give way once about 1% of the displays' new pictures come late."
            }
            Strictness::Off => {
                "The previews never give way: on a busy computer the displays may stutter."
            }
        }
    }
}

/// The output windows the counts are kept for (`render`'s slots).
pub const WINDOWS: usize = 8;

/// One output window's pictures since the last look (`render::shown_totals`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Shown {
    /// Frames that put a new picture on the display.
    pub pictures: u32,
    /// Of those, the late ones (a refresh missed): seen misses.
    pub late: u32,
}

impl Shown {
    /// The counts since `before` (counters that wrap).
    pub fn since(self, before: Shown) -> Shown {
        Shown {
            pictures: self.pictures.wrapping_sub(before.pictures),
            late: self.late.wrapping_sub(before.late),
        }
    }
}

/// What was measured since the last look.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Reading {
    /// Each output window's new pictures and seen misses since the last reading.
    pub shown: [Shown; WINDOWS],
    /// Main-loop stalls (over 100 ms) since the last reading.
    pub stalls: u32,
    /// A cue started or a jump landed since the last reading (`Engine::jolts`),
    /// or a picture layer was put up new: what the outputs miss for the
    /// next `Governor::GRACE` isn't counted.
    pub jolted: bool,
    /// Made up (`NEWQ_PREVIEW_PRESSURE`), in thousandths of the new pictures
    /// late: never learned as the outputs' own trouble, so the previews stay
    /// given way for as long as it lasts (whatever the strictness, Off too).
    pub forced: Option<u32>,
}

impl Reading {
    /// What the governor is told on a pass, but only while something plays (a
    /// stall while a show loads says nothing about now). `forced`
    /// (`NEWQ_PREVIEW_PRESSURE`) stands for the late share whether anything
    /// plays or not.
    pub fn of(
        forced: Option<u32>,
        playing: bool,
        shown: [Shown; WINDOWS],
        stalls: u32,
        jolted: bool,
    ) -> Reading {
        match forced {
            Some(f) => Reading {
                stalls: if playing { stalls } else { 0 },
                forced: Some(f),
                ..Reading::default()
            },
            None if playing => Reading {
                shown,
                stalls,
                jolted,
                forced: None,
            },
            None => Reading::default(),
        }
    }

    /// Of every window, the new pictures and the seen misses (for the log).
    pub fn total(&self) -> Shown {
        self.shown.iter().fold(Shown::default(), |a, s| Shown {
            pictures: a.pictures.saturating_add(s.pictures),
            late: a.late.saturating_add(s.late),
        })
    }
}

/// Steps the previews down when the outputs really miss pictures and back up
/// soon after, and learns when giving way doesn't help.
///
/// Trouble is seen misses over `Strictness::share` of an output's new pictures
/// in the last `WINDOW` (at least `MIN_SEEN` of them), or a main-loop stall.
/// It steps the previews down a level, at most once a `WINDOW` while it goes on
/// (a stall: at most once a `STEP_DOWN_EVERY`); after `PATIENCE` without it they
/// step back up a level. Some trouble isn't theirs: a system compositor's
/// hiccup, a lid change, a screen on a basic display driver misses pictures
/// whatever the previews do. So once the previews have stood at a level for
/// `MEASURE` (in practice paused, since trouble anywhere else steps them lower)
/// and the outputs still have trouble there at about the rate they had on the
/// way down, the previews were not the cause: they go straight back up (to
/// `ceiling`) and that rate is remembered as the outputs' own (`Floor`). From
/// then on, at that level or above, trouble at up to about that rate is let
/// be; clearly more (`surge`) steps them down, as before. Should the trouble
/// surge within `MEASURE` of going back up, the previews were the cause after
/// all: down again, and the next return waits twice as long and stops a level
/// lower. The remembered rate fades (`FLOOR_HALF_LIFE`), so the question is
/// asked again now and then. Nothing here holds the previews down for minutes:
/// every wait is capped well under one.
#[derive(Clone, Debug, Default)]
pub struct Governor {
    level: Level,
    /// How much the outputs may miss (Off: the previews never give way).
    strictness: Strictness,
    /// When the level last changed.
    changed: Option<Instant>,
    /// Since when no reading has been trouble.
    calm_since: Option<Instant>,
    /// How long it must be calm before a step up; doubled each time a step up
    /// is followed soon by trouble (so it doesn't flap), back to the start
    /// after a long smooth stretch at full rate.
    patience: Duration,
    /// When it last stepped up.
    stepped_up: Option<Instant>,
    /// Each window's new pictures and seen misses in the last `WINDOW` at this
    /// level (since it was reached), by when.
    samples: VecDeque<(Instant, [Shown; WINDOWS])>,
    /// Until when misses aren't counted: a cue started or a jump landed.
    grace_until: Option<Instant>,
    /// Trouble (see `observe`) seen at this level since it was reached.
    here: u64,
    /// The highest level the previews go back to when giving way didn't
    /// help (Full, until going back up was followed by more trouble).
    ceiling: Level,
    /// Since when the level has been under the ceiling, and the trouble seen
    /// since (the way down's rate, against which the level reached is judged).
    below_since: Option<Instant>,
    below: u64,
    /// How long a level is stood at before it is judged (`MEASURE`, doubled
    /// by each return that failed).
    measure: Duration,
    /// The outputs' own trouble, learned: let be at or above its level.
    floor: Option<Floor>,
    /// How long a floor under full rate holds before the level above is
    /// tried (`RAISE_FIRST`, doubled by each try that failed).
    raise_wait: Duration,
    /// Trouble in the last `RECENT`, by when (for the surge test).
    recent: VecDeque<(Instant, u32)>,
    /// What it decided, for the log, taken once.
    news: Option<News>,
    /// When it last found the previews weren't the cause.
    not_theirs: Option<Instant>,
}

/// The outputs' own trouble, learned: the previews gave way and it went on.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Floor {
    /// Trouble at up to the rate is let be at this level and above.
    level: Level,
    /// Trouble a second (it fades from `learned`), and what the trial found
    /// (never let be more than that).
    rate: f64,
    found: f64,
    /// When it was learned or last seen to go on (the rate fades from then).
    learned: Instant,
    /// The trouble let be since `window` began: while it goes on at the rate,
    /// the rate is kept; once it stops, the rate fades.
    seen: u64,
    window: Instant,
    /// When the previews last went up on its say-so (to the floor's level),
    /// or it settled.
    since: Instant,
    /// Gone up to try the level above a floor that held: if that fails, back
    /// to this one.
    fallback: Option<Level>,
    /// Still within `measure` of going up: a surge now means the previews
    /// were the cause after all.
    trying: bool,
}

impl Floor {
    fn rate_now(&self, now: Instant) -> f64 {
        let age = now.saturating_duration_since(self.learned).as_secs_f64();
        self.rate * 0.5f64.powf(age / Governor::FLOOR_HALF_LIFE.as_secs_f64())
    }
}

/// What the governor decided, beyond a step: for the log.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum News {
    /// The outputs went on having trouble while the previews gave way
    /// (`per_minute` at `at`, `before` a minute on the way there): not the
    /// previews' doing; back up to `back_to`.
    NotTheirs {
        at: Level,
        per_minute: f64,
        before: f64,
        back_to: Level,
    },
    /// Back at `level`, the trouble rose: the previews' doing after all.
    TheirsAfterAll { level: Level },
    /// The trouble rose clearly above what the outputs have by themselves.
    Surge { level: Level },
}

impl News {
    /// A line for the log.
    pub fn words(&self) -> String {
        match self {
            News::NotTheirs {
                at,
                per_minute,
                before,
                back_to,
            } => format!(
                "previews: the outputs' seen misses and stalls went on while the previews were at {at:?} \
                 ({per_minute:.1} a minute; {before:.1} on the way down): not the previews' doing; \
                 back to {back_to:?}, letting about that rate be"
            ),
            News::TheirsAfterAll { level } => format!(
                "previews: back at {level:?} the outputs' seen misses rose: the previews' doing after all; \
                 giving way again (the next return waits longer, a level lower)"
            ),
            News::Surge { level } => format!(
                "previews: at {level:?} the outputs' seen misses rose clearly above what they have by themselves: giving way again"
            ),
        }
    }
}

impl Governor {
    /// The share of new pictures is judged over this long.
    pub const WINDOW: Duration = Duration::from_secs(3);
    /// Seen misses in `WINDOW` needed before the share counts at all (one
    /// miss in a few pictures is chance, not trouble).
    pub const MIN_SEEN: u32 = 2;
    /// The share is of at least this many pictures (a window that has seen
    /// only a few new pictures yet is judged as if it had seen this many).
    pub const LEAST_PICTURES: u32 = 30;
    /// After a cue starts or a jump lands, misses aren't counted for this
    /// long: starting it costs what it costs, and pausing the previews can't
    /// spare the outputs that.
    pub const GRACE: Duration = Duration::from_secs(1);
    /// A stall, or trouble right after a step up, steps down no sooner than
    /// this after the last change. (Other trouble: once a `WINDOW`.)
    pub const STEP_DOWN_EVERY: Duration = Duration::from_secs(1);
    /// How long smooth before the first step up.
    pub const PATIENCE: Duration = Duration::from_secs(2);
    /// The most it waits before a step up.
    pub const PATIENCE_MOST: Duration = Duration::from_secs(20);
    /// Trouble this soon after a step up makes the next wait longer.
    pub const RELAPSE_WITHIN: Duration = Duration::from_secs(20);
    /// Smooth at full rate this long: the waits are back to the start.
    pub const FORGIVE_AFTER: Duration = Duration::from_secs(60);
    /// How long a level is stood at, trouble still coming, before it is judged
    /// whether giving way helped (doubled by each failed return, up to
    /// `MEASURE_MOST`).
    pub const MEASURE: Duration = Duration::from_secs(6);
    pub const MEASURE_MOST: Duration = Duration::from_secs(30);
    /// Giving way helped when the trouble at the level reached is under this
    /// share of the way down's (by counts, `SLACK` allowed for chance).
    pub const HELPED_UNDER: f64 = 0.5;
    /// The surge test looks at the trouble in the last this long...
    pub const RECENT: Duration = Duration::from_secs(10);
    /// ... against this many times the learned rate's, plus `SLACK`.
    pub const SURGE_FACTOR: f64 = 2.0;
    /// Trouble allowed to chance in either test.
    pub const SLACK: f64 = 3.0;
    /// The learned rate halves in this long, so the question is asked again.
    pub const FLOOR_HALF_LIFE: Duration = Duration::from_secs(300);
    /// The trouble let be is looked at over this long, to keep the rate
    /// while it goes on.
    pub const FLOOR_WINDOW: Duration = Duration::from_secs(120);
    /// A learned floor under full rate that held this long tries the level
    /// above (doubled by each try that failed, up to `RAISE_MOST`).
    pub const RAISE_FIRST: Duration = Duration::from_secs(20);
    pub const RAISE_MOST: Duration = Duration::from_secs(60);
    /// Under this many a second, the learned rate is forgotten: the first
    /// trouble steps the previews down again.
    pub const FORGET_UNDER: f64 = 0.02;

    pub fn level(&self) -> Level {
        self.level
    }

    /// How much the outputs may miss from now on. Off puts the previews back
    /// at full rate at once and forgets what was learned.
    pub fn set_strictness(&mut self, s: Strictness) {
        if s != self.strictness {
            *self = Governor {
                strictness: s,
                ..Governor::default()
            };
        }
    }

    /// What it last decided beyond a step, once (for the log).
    pub fn take_news(&mut self) -> Option<News> {
        self.news.take()
    }

    /// When it last found the outputs' trouble wasn't the previews' doing.
    pub fn not_theirs(&self) -> Option<Instant> {
        self.not_theirs
    }

    /// The worst output window's seen misses and new pictures over the last
    /// `WINDOW` at this level (for the log and the tests).
    pub fn worst(&self) -> Shown {
        let mut worst = Shown::default();
        let mut worst_share = -1.0;
        for w in 0..WINDOWS {
            let s = self.window_sum(w);
            let share = f64::from(s.late) / f64::from(s.pictures.max(Self::LEAST_PICTURES));
            if share > worst_share && (s.pictures > 0 || s.late > 0) {
                worst_share = share;
                worst = s;
            }
        }
        worst
    }

    fn window_sum(&self, w: usize) -> Shown {
        self.samples
            .iter()
            .fold(Shown::default(), |a, (_, s)| Shown {
                pictures: a.pictures.saturating_add(s[w].pictures),
                late: a.late.saturating_add(s[w].late),
            })
    }

    /// Whether some output window's seen misses over the last `WINDOW` are
    /// more than `share` of its new pictures.
    fn missing(&self, share: f64) -> bool {
        (0..WINDOWS).any(|w| {
            let s = self.window_sum(w);
            s.late >= Self::MIN_SEEN
                && f64::from(s.late) > share * f64::from(s.pictures.max(Self::LEAST_PICTURES))
        })
    }

    /// Take a reading made at `now`; the level the previews are to draw at.
    pub fn observe(&mut self, now: Instant, r: Reading) -> Level {
        let share = match (self.strictness.share(), r.forced) {
            (Some(s), _) => s,
            // Forced pressure works whatever the setting (for trying the steps).
            (None, Some(_)) => Strictness::Relaxed.share().unwrap_or(0.05),
            (None, None) => {
                // Off: full rate, and anything forced meanwhile forgotten.
                if self.level != Level::Full {
                    *self = Governor {
                        strictness: Strictness::Off,
                        ..Governor::default()
                    };
                }
                return Level::Full;
            }
        };
        if self.patience.is_zero() {
            self.patience = Self::PATIENCE;
        }
        if self.measure.is_zero() {
            self.measure = Self::MEASURE;
        }
        if self.raise_wait.is_zero() {
            self.raise_wait = Self::RAISE_FIRST;
        }
        // Just after a cue started or a jump landed, the outputs' misses are
        // the start's, not the previews': none of it is counted.
        if r.jolted {
            self.grace_until = Some(now + Self::GRACE);
        }
        let in_grace = self.grace_until.is_some_and(|g| now <= g);
        let shown = if in_grace {
            [Shown::default(); WINDOWS]
        } else {
            r.shown
        };
        if shown.iter().any(|s| s.pictures > 0 || s.late > 0) {
            self.samples.push_back((now, shown));
        }
        while self
            .samples
            .front()
            .is_some_and(|(t, _)| now.saturating_duration_since(*t) > Self::WINDOW)
        {
            self.samples.pop_front();
        }
        // How much trouble this is: each seen miss and stall.
        let seen: u32 = shown.iter().map(|s| s.late).fold(0, u32::saturating_add);
        let weight = seen.saturating_add(r.stalls);
        self.here += u64::from(weight);
        if self.below_since.is_some() {
            self.below += u64::from(weight);
        }
        if weight > 0 {
            self.recent.push_back((now, weight));
        }
        while self
            .recent
            .front()
            .is_some_and(|(t, _)| now.saturating_duration_since(*t) > Self::RECENT)
        {
            self.recent.pop_front();
        }
        if self
            .floor
            .is_some_and(|f| f.rate_now(now) < Self::FORGET_UNDER)
        {
            self.floor = None;
        }
        // Forced pressure (for trying the steps) is never taken as the outputs'.
        let floor = self
            .floor
            .filter(|f| r.forced.is_none() && self.level <= f.level);
        if let Some(f) = &mut self.floor
            && floor.is_some()
            && !f.trying
        {
            f.seen += u64::from(weight);
        }
        let since = |t: Option<Instant>| t.map(|t| now.saturating_duration_since(t));
        let forced = r.forced.is_some_and(|f| f64::from(f) >= share * 1000.0);
        let missing = self.missing(share);
        if forced || missing || r.stalls > 0 {
            self.calm_since = None;
            // At most once a window of trouble; a stall, or trouble right
            // after a step up (the previews' doing, by the look of it), sooner.
            let relapse = self.stepped_up.is_some() && self.stepped_up == self.changed;
            let spacing = if r.stalls > 0 || relapse {
                Self::STEP_DOWN_EVERY
            } else {
                Self::WINDOW
            };
            let may = since(self.changed).is_none_or(|d| d >= spacing);
            let let_be = floor.is_some_and(|f| !self.surge(now, &f));
            if may && !let_be && self.level != Level::Paused {
                if since(self.stepped_up).is_some_and(|d| d < Self::RELAPSE_WITHIN) {
                    self.patience = (self.patience * 2).min(Self::PATIENCE_MOST);
                    self.stepped_up = None;
                }
                match floor {
                    Some(f) if f.trying => {
                        // Went up on the floor's say-so, and it got worse.
                        self.news = Some(News::TheirsAfterAll { level: self.level });
                        match f.fallback {
                            Some(back) => {
                                self.raise_wait = (self.raise_wait * 2).min(Self::RAISE_MOST);
                                self.floor = Some(Floor {
                                    level: back,
                                    since: now,
                                    seen: 0,
                                    window: now,
                                    fallback: None,
                                    trying: false,
                                    ..f
                                });
                                self.ceiling = back;
                                self.move_to(now, back);
                            }
                            None => {
                                self.measure = (self.measure * 2).min(Self::MEASURE_MOST);
                                self.floor = None;
                                let down = self.level.down();
                                self.ceiling = down;
                                self.move_to(now, down);
                            }
                        }
                    }
                    Some(f) => {
                        self.news = Some(News::Surge { level: self.level });
                        // At the floor's own level, the outputs' trouble has
                        // changed: it is learned afresh. Above it, it stands.
                        if self.level == f.level {
                            self.floor = None;
                        }
                        self.move_to(now, self.level.down());
                    }
                    None => self.move_to(now, self.level.down()),
                }
            }
        } else {
            let calm_since = *self.calm_since.get_or_insert(now);
            let calm = now.saturating_duration_since(calm_since);
            let settled = since(self.changed).is_none_or(|d| d >= self.patience);
            if self.level != Level::Full && calm >= self.patience && settled {
                self.move_to(now, self.level.up());
                self.stepped_up = Some(now);
                self.calm_since = Some(now);
            } else if self.level == Level::Full && calm >= Self::FORGIVE_AFTER {
                self.patience = Self::PATIENCE;
                self.measure = Self::MEASURE;
            }
        }
        if r.forced.is_none() {
            self.learn(now);
        }
        self.level
    }

    /// Clearly more trouble than the outputs have by themselves (`f`) in the
    /// last `RECENT` (since the previews last went up on its say-so): judged
    /// over the time looked at (at least a `WINDOW`), so a surge just after
    /// going back up shows within a few seconds, while `trying`.
    fn surge(&self, now: Instant, f: &Floor) -> bool {
        let from = match now.checked_sub(Self::RECENT) {
            Some(t) if t > f.since => t,
            _ => f.since,
        };
        let seen: u64 = self
            .recent
            .iter()
            .filter(|(t, _)| *t > from)
            .map(|(_, w)| u64::from(*w))
            .sum();
        let looked = now
            .saturating_duration_since(from)
            .max(Self::WINDOW)
            .as_secs_f64();
        seen as f64 > Self::SLACK + Self::SURGE_FACTOR * f.rate_now(now) * looked
    }

    /// Whether giving way helped, once there is enough to tell; and the
    /// floor's own course (settling, trying the level above).
    fn learn(&mut self, now: Instant) {
        if let Some(f) = &mut self.floor
            && f.trying
            && now.saturating_duration_since(f.since) >= self.measure
        {
            if f.fallback.take().is_some() {
                self.raise_wait = Self::RAISE_FIRST;
            }
            f.trying = false;
            f.since = now;
            f.seen = 0;
            f.window = now;
        }
        if let Some(f) = &mut self.floor
            && !f.trying
        {
            // While the trouble let be goes on at about the rate, the rate is
            // kept (never above what the trial found); once it stops, it fades.
            let window = now.saturating_duration_since(f.window);
            if window >= Self::FLOOR_WINDOW {
                let seen = f.seen as f64 / window.as_secs_f64();
                if seen >= f.rate_now(now) {
                    f.rate = seen.min(f.found);
                    f.learned = now;
                }
                f.seen = 0;
                f.window = now;
            }
        }
        if let Some(f) = self.floor
            && !f.trying
            && f.level != Level::Full
            && self.level == f.level
            && now.saturating_duration_since(f.since) >= self.raise_wait
        {
            // A floor under full rate that has held a while: try the level above.
            let up = f.level.up();
            self.floor = Some(Floor {
                level: up,
                since: now,
                fallback: Some(f.level),
                trying: true,
                ..f
            });
            self.ceiling = up;
            self.move_to(now, up);
            return;
        }
        let (Some(below_since), Some(changed)) = (self.below_since, self.changed) else {
            return;
        };
        let stood = now.saturating_duration_since(changed);
        if self.here == 0 || stood < self.measure {
            return;
        }
        let stood = stood.as_secs_f64();
        // The way down: from going under the ceiling to reaching this level.
        let way = changed
            .saturating_duration_since(below_since)
            .as_secs_f64()
            .max(1.0);
        let before = self.below.saturating_sub(self.here) as f64 / way;
        if (self.here as f64) + Self::SLACK < Self::HELPED_UNDER * before * stood {
            return; // It helped: clearly less trouble here than on the way down.
        }
        let here = self.here as f64 / stood;
        // The outputs' own rate: all the trouble since the previews gave way
        // (more of it to go on than this level's alone).
        let own = self.below as f64 / (way + stood);
        let back_to = self.ceiling;
        self.news = Some(News::NotTheirs {
            at: self.level,
            per_minute: here * 60.0,
            before: before * 60.0,
            back_to,
        });
        self.not_theirs = Some(now);
        self.floor = Some(Floor {
            level: back_to,
            rate: own,
            found: own,
            learned: now,
            seen: 0,
            window: now,
            since: now,
            fallback: None,
            trying: true,
        });
        self.calm_since = None;
        self.stepped_up = None;
        self.move_to(now, back_to);
    }

    /// Be at `level` from `now`: the window of pictures starts again there.
    fn move_to(&mut self, now: Instant, level: Level) {
        self.level = level;
        self.changed = Some(now);
        self.here = 0;
        self.samples.clear();
        if level == Level::Full {
            self.ceiling = Level::Full;
        }
        if level <= self.ceiling {
            self.ceiling = level;
            self.below_since = None;
            self.below = 0;
        } else if self.below_since.is_none() {
            self.below_since = Some(now);
            self.below = 0;
        }
    }
}

/// The level the previews draw at: the governor's, never above the cap.
pub fn effective(governed: Level, cap: Cap) -> Level {
    governed.max(cap.level())
}

/// The share of their size the previews are drawn at: the level's, never above
/// the computer's size limit.
pub fn effective_scale(level: Level, size: SizeCap) -> f32 {
    level.scale().min(size.scale())
}

/// Which preview is asking: each keeps its own time of its last picture.
/// A new kind of preview takes a variant of its own (or `Other`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PreviewId {
    /// A preview host's single output, by the host (a Preview pane's instance,
    /// or `dock::OPERATOR_PREVIEW`).
    Single(u8),
    /// A multiview tile, by its host and its place.
    Tile(u8, u8),
    /// The timeline workspace's monitor.
    Workspace,
    /// A preview host's 3D view (`ui::stage_view`), by the host.
    Stage(u8),
    /// The Displays page's area editor, by the monitor it shows.
    Area(usize),
    /// Any other kind, by its name and number. (None yet.)
    #[allow(dead_code)]
    Other(&'static str, u32),
}

impl PreviewId {
    /// The preview host it belongs to, if it is one of a host's.
    pub fn host(self) -> Option<u8> {
        match self {
            PreviewId::Single(h) | PreviewId::Tile(h, _) | PreviewId::Stage(h) => Some(h),
            PreviewId::Workspace | PreviewId::Area(_) | PreviewId::Other(..) => None,
        }
    }
}

/// What a preview may do this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreviewBudget {
    /// Draw a new picture now. False: show the last one (it isn't due yet, or
    /// the previews are paused).
    pub draw: bool,
    /// At this share of the size it is shown at.
    pub scale: f32,
    /// The least time between pictures (zero: every frame).
    pub every: Duration,
    /// Paused to protect the outputs: draw nothing new until it says otherwise
    /// (a preview with no picture yet may draw one).
    pub paused: bool,
}

impl PreviewBudget {
    /// Every frame, full size: as previews always drew.
    pub const FULL: PreviewBudget = PreviewBudget {
        draw: true,
        scale: 1.0,
        every: Duration::ZERO,
        paused: false,
    };

    pub fn is_full(&self) -> bool {
        self.every.is_zero() && self.scale >= 1.0 && !self.paused
    }
}

/// The one governor of every preview in the app.
#[derive(Default)]
pub struct PreviewGovernor {
    governor: Governor,
    /// When each preview last had a picture to draw.
    last: RefCell<HashMap<PreviewId, Instant>>,
    /// Which previews asked (for the tests that every kind does).
    #[cfg(test)]
    pub asked: RefCell<Vec<PreviewId>>,
}

impl PreviewGovernor {
    /// Take a reading (the app, each pass), under this computer's
    /// `strictness`; the governed level.
    pub fn observe(&mut self, now: Instant, r: Reading, strictness: Strictness) -> Level {
        self.governor.set_strictness(strictness);
        self.governor.observe(now, r)
    }

    /// What the governor last decided beyond a step, once (for the log).
    pub fn take_news(&mut self) -> Option<News> {
        self.governor.take_news()
    }

    /// The level the governor has set (before the computer's cap).
    pub fn governed(&self) -> Level {
        self.governor.level()
    }

    /// The worst output window's seen misses over the last `Governor::WINDOW`
    /// (for the log).
    pub fn worst(&self) -> Shown {
        self.governor.worst()
    }

    /// For tests: be at `level`.
    #[cfg(test)]
    pub fn force(&mut self, level: Level) {
        self.governor.level = level;
    }

    /// Forget the previews of the hosts not kept (Preview panes closed): a
    /// new pane given one's number starts afresh.
    pub fn retain_hosts(&self, keep: impl Fn(u8) -> bool) {
        self.last
            .borrow_mut()
            .retain(|id, _| id.host().is_none_or(&keep));
    }

    /// How many previews it keeps a time for (for the tests).
    #[cfg(test)]
    pub fn known(&self) -> usize {
        self.last.borrow().len()
    }

    /// May preview `id` draw a picture at `now`, and how big? Under `limits`.
    pub fn ask(&self, id: PreviewId, now: Instant, limits: Limits) -> PreviewBudget {
        #[cfg(test)]
        self.asked.borrow_mut().push(id);
        let level = effective(self.governed(), limits.rate);
        let scale = effective_scale(level, limits.size);
        if level == Level::Full && scale >= 1.0 {
            return PreviewBudget::FULL;
        }
        let every = level.every();
        let paused = level == Level::Paused;
        let mut last = self.last.borrow_mut();
        let due = !paused
            && last
                .get(&id)
                .is_none_or(|t| now.saturating_duration_since(*t) >= every);
        if due {
            last.insert(id, now);
        }
        PreviewBudget {
            draw: due,
            scale,
            every,
            paused,
        }
    }

    /// What the Preview pane's header says, if anything: only while the
    /// previews draw less than they would.
    pub fn notice(&self, limits: Limits) -> Option<String> {
        let lately = self
            .governor
            .not_theirs()
            .is_some_and(|t| t.elapsed() < NOT_THEIRS_SAID);
        notice(self.governed(), limits, lately)
    }
}

/// Whether an output that only the previews show gets its pictures uploaded
/// now (last done at `last`): every pass at full rate, no more often than the
/// previews draw while they give way, and only the first time while paused.
pub fn uploads_due(level: Level, last: Option<Instant>, now: Instant) -> bool {
    match (level, last) {
        (Level::Full, _) | (_, None) => true,
        (Level::Paused, Some(_)) => false,
        (l, Some(t)) => now.saturating_duration_since(t) + Duration::from_millis(2) >= l.every(),
    }
}

/// How often the main window is drawn while something plays and it shows
/// previews: `full` at full rate, else the previews' rate but never under
/// `least` (what the window needs for its own progress bars and clocks).
pub fn window_fps(level: Level, full: f64, least: f64) -> f64 {
    match level.fps() {
        None => full,
        Some(f) => f64::from(f).max(least).min(full),
    }
}

/// How long the Preview pane says the previews came back because the
/// outputs' trouble wasn't theirs.
pub const NOT_THEIRS_SAID: Duration = Duration::from_secs(20);

/// What a preview says in place of its picture while this computer's previews
/// are switched off (Settings ▸ General ▸ Previews).
pub const OFF_WORDS: &str = "Previews are off on this computer";

/// What the Preview pane's header says while the previews draw less, or
/// (`not_theirs`: for a while after) that they came back because the
/// outputs' missed pictures weren't their doing.
pub fn notice(governed: Level, limits: Limits, not_theirs: bool) -> Option<String> {
    let level = effective(governed, limits.rate);
    let scale = effective_scale(level, limits.size);
    // The governor holds them lower than the computer's own limits would.
    let governor = governed > limits.rate.level() || governed.scale() < limits.size.scale();
    let why = if governor {
        "to keep the outputs smooth"
    } else {
        "(this computer's setting)"
    };
    let size = if scale <= 0.25 {
        "quarter"
    } else if scale < 1.0 {
        "half"
    } else {
        ""
    };
    match level {
        _ if not_theirs && !governor => {
            Some("Previews back: the outputs' missed pictures aren't theirs".to_string())
        }
        Level::Paused => Some("Previews paused to protect the outputs".to_string()),
        Level::Full if size.is_empty() => None,
        Level::Full => Some(format!("Preview at {size} size {why}")),
        _ if size.is_empty() => Some(format!("Preview at {} fps {why}", level.fps()?)),
        _ => Some(format!(
            "Preview at {} fps, {size} size, {why}",
            level.fps()?
        )),
    }
}

/// `NEWQ_PREVIEW_PRESSURE`: for trying the preview's steps without a slow
/// computer. `1` (or `on`) is every new picture late; another number is the
/// late pictures in thousandths. Unset or empty: what was measured. It works
/// whatever `Strictness` says, Off too.
pub fn forced_pressure(value: Option<&str>) -> Option<u32> {
    let v = value?.trim();
    if v.is_empty() {
        return None;
    }
    if matches!(v, "1" | "on" | "yes" | "true") {
        return Some(1000);
    }
    Some(v.parse::<u32>().unwrap_or(1000).min(1000))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One output window's new pictures and seen misses.
    fn one(pictures: u32, late: u32) -> Reading {
        let mut r = Reading::default();
        r.shown[0] = Shown { pictures, late };
        r
    }

    const SMOOTH: Reading = Reading {
        shown: [Shown {
            pictures: 6,
            late: 0,
        }; WINDOWS],
        stalls: 0,
        jolted: false,
        forced: None,
    };

    fn stall() -> Reading {
        Reading {
            stalls: 1,
            ..one(6, 0)
        }
    }

    fn governor(s: Strictness) -> Governor {
        let mut g = Governor::default();
        g.set_strictness(s);
        g
    }

    /// Readings every `step` ms for `secs` from `world` (the time since the
    /// start, in ms, and the level the previews are at); the level after each.
    fn sim(
        g: &mut Governor,
        t: &mut Instant,
        secs: u64,
        step: u64,
        mut world: impl FnMut(u64, Level) -> Reading,
    ) -> Vec<Level> {
        let start = *t;
        (0..secs * 1000 / step)
            .map(|_| {
                *t += Duration::from_millis(step);
                let ms = t.duration_since(start).as_millis() as u64;
                let r = world(ms, g.level());
                g.observe(*t, r)
            })
            .collect()
    }

    /// An output window showing `fps` new pictures a second, `late(ms, level)`
    /// of them late (a share, spread evenly), read every `step` ms.
    fn film(
        fps: f64,
        step: u64,
        late: impl Fn(u64, Level) -> f64,
    ) -> impl FnMut(u64, Level) -> Reading {
        let mut owed = 0.0;
        move |ms, level| {
            let pictures = ((ms as f64 * fps / 1000.0).floor()
                - (((ms - step) as f64) * fps / 1000.0).floor()) as u32;
            owed += f64::from(pictures) * late(ms, level);
            let missed = (owed.floor() as u32).min(pictures);
            owed -= f64::from(missed);
            one(pictures, missed)
        }
    }

    /// An output window showing `fps` new pictures a second whose display
    /// misses one refresh every `period` ms, on a new picture every time (the
    /// worst case), read every `step` ms.
    fn missing_every(fps: f64, period: u64, step: u64) -> impl FnMut(u64, Level) -> Reading {
        let mut pictures = film(fps, step, |_, _| 0.0);
        move |ms, level| {
            let mut r = pictures(ms, level);
            if ms % period < step && r.shown[0].pictures > 0 {
                r.shown[0].late = 1;
            }
            r
        }
    }

    fn share_at(levels: &[Level], level: Level) -> f64 {
        levels.iter().filter(|l| **l == level).count() as f64 / levels.len() as f64
    }

    #[test]
    fn a_film_on_a_fast_display_missing_a_refresh_now_and_then_leaves_the_previews_alone() {
        // The owner's Mac: 25 fps on 100 Hz, a refresh missed every 5 to 7 s
        // (every one counted as seen here, which most were not), and more
        // often than that too: none of it is worth giving way for.
        for strictness in [Strictness::Relaxed, Strictness::Strict] {
            for period in [5_000, 7_000] {
                for step in [10, 100] {
                    let mut g = governor(strictness);
                    let mut t = Instant::now();
                    let levels = sim(&mut g, &mut t, 600, step, missing_every(25.0, period, step));
                    assert!(
                        levels.iter().all(|l| *l == Level::Full),
                        "{strictness:?}, every {period} ms, read every {step} ms"
                    );
                }
            }
        }
    }

    #[test]
    fn a_film_missing_a_tenth_of_its_pictures_steps_the_previews_down() {
        // 60 fps on 60 Hz, one picture in ten late while the previews draw at
        // full rate or 60 a second; 30 a second is fine.
        let world = film(60.0, 16, |_, l| if l <= Level::Fps60 { 0.1 } else { 0.0 });
        let mut g = Governor::default();
        let mut t = Instant::now();
        let levels = sim(&mut g, &mut t, 600, 16, world);
        let down = levels.iter().position(|l| *l != Level::Full).unwrap();
        assert!(down * 16 <= 1_100, "stepped down after {} ms", down * 16);
        // Down one window later, not faster (one step a window of trouble).
        let next = levels.iter().position(|l| *l > Level::Fps60).unwrap();
        assert!(next * 16 >= 3_000 && next * 16 <= 4_200, "{} ms", next * 16);
        // Never lower than it needs, and mostly where it does.
        assert!(levels.iter().all(|l| *l <= Level::Fps30), "{levels:?}");
        let rest = &levels[levels.len() / 2..];
        assert!(
            share_at(rest, Level::Fps30) > 0.85,
            "{}",
            share_at(rest, Level::Fps30)
        );
        assert_eq!(g.take_news(), None, "nothing learned: giving way helped");
        // It tries to go back up now and then (at least every 20 s and a bit),
        // and each try is cut short within a second or so.
        let ups = rest.windows(2).filter(|w| w[1] < w[0]).count();
        assert!((10..=20).contains(&ups), "{ups} tries in 300 s");
        let mut run = 0;
        for l in &levels {
            if *l == Level::Fps60 {
                run += 1;
                assert!(run * 16 <= 3_100, "stayed at 60 too long");
            } else {
                run = 0;
            }
        }
    }

    #[test]
    fn strict_gives_way_where_relaxed_does_not() {
        // 60 fps, 3 % of the pictures late, whatever the previews do.
        let run = |s| {
            let mut g = governor(s);
            let mut t = Instant::now();
            sim(&mut g, &mut t, 30, 16, film(60.0, 16, |_, _| 0.03))
        };
        assert!(run(Strictness::Relaxed).iter().all(|l| *l == Level::Full));
        assert!(run(Strictness::Strict).iter().any(|l| *l > Level::Full));
        // And a seen miss every 2 s on a 25 fps film: two in a window.
        let run = |s| {
            let mut g = governor(s);
            let mut t = Instant::now();
            sim(&mut g, &mut t, 60, 10, missing_every(25.0, 2_000, 10))
        };
        assert!(run(Strictness::Relaxed).iter().all(|l| *l == Level::Full));
        assert!(run(Strictness::Strict).iter().any(|l| *l > Level::Full));
        // One miss alone is never enough, however few the pictures.
        let mut g = governor(Strictness::Strict);
        let t = Instant::now();
        assert_eq!(g.observe(t, one(1, 1)), Level::Full);
    }

    #[test]
    fn off_never_gives_way_but_forced_pressure_still_does() {
        let mut g = governor(Strictness::Off);
        let mut t = Instant::now();
        let levels = sim(&mut g, &mut t, 120, 50, |_, _| Reading {
            stalls: 1,
            ..one(3, 3)
        });
        assert!(levels.iter().all(|l| *l == Level::Full));
        let forced = Reading::of(Some(1000), true, [Shown::default(); WINDOWS], 0, false);
        let levels = sim(&mut g, &mut t, 60, 50, |_, _| forced);
        assert_eq!(levels.last(), Some(&Level::Paused));
        // Once the pressure goes, Off is Off again at once.
        assert_eq!(sim(&mut g, &mut t, 1, 50, |_, _| SMOOTH)[0], Level::Full);
        // Switching it off brings a governor that gave way straight back up.
        let mut g = Governor::default();
        sim(&mut g, &mut t, 20, 16, film(60.0, 16, |_, _| 0.5));
        assert!(g.level() > Level::Fps60);
        g.set_strictness(Strictness::Off);
        assert_eq!(g.level(), Level::Full);
    }

    #[test]
    fn a_main_loop_stall_steps_the_previews_down_at_once() {
        let mut g = Governor::default();
        let t = Instant::now();
        assert_eq!(g.observe(t, stall()), Level::Fps60);
        // A second within the second doesn't: one step a second at most.
        assert_eq!(
            g.observe(t + Duration::from_millis(500), stall()),
            Level::Fps60
        );
        assert_eq!(
            g.observe(t + Duration::from_millis(1000), stall()),
            Level::Fps30
        );
    }

    #[test]
    fn misses_just_after_a_go_or_a_jump_do_not_count() {
        // A GO every 15 s, and for the 0.8 s after it a third of the pictures
        // late (starting the cue), whatever the previews do.
        let world = |jolt: bool| {
            let mut pictures = film(60.0, 16, |ms, _| if ms % 15_000 < 800 { 0.33 } else { 0.0 });
            move |ms: u64, l: Level| Reading {
                jolted: jolt && ms % 15_000 <= 16,
                ..pictures(ms, l)
            }
        };
        let mut g = Governor::default();
        let mut t = Instant::now();
        let levels = sim(&mut g, &mut t, 300, 16, world(true));
        assert!(levels.iter().all(|l| *l == Level::Full));
        assert_eq!(g.take_news(), None);
        // Untold of the GOs, the same misses would step them down.
        let mut g = Governor::default();
        let levels = sim(&mut g, &mut t, 300, 16, world(false));
        assert!(levels.iter().any(|l| *l > Level::Full));
        // And once own trouble was learned, a GO's misses are no surge
        // (0.24.6's "the previews' doing after all" two seconds after a GO).
        let mut g = Governor::default();
        sim(&mut g, &mut t, 60, 16, film(60.0, 16, |_, _| 0.3));
        assert!(matches!(g.take_news(), Some(News::NotTheirs { .. })));
        assert_eq!(g.level(), Level::Full);
        let mut own = film(60.0, 16, |_, _| 0.3);
        let mut go = world(true);
        let levels = sim(&mut g, &mut t, 120, 16, |ms, l| {
            let (a, b) = (own(ms, l), go(ms, l));
            let mut r = a;
            r.shown[0].late += b.shown[0].late;
            r.jolted = b.jolted;
            r
        });
        assert!(
            share_at(&levels, Level::Full) > 0.95,
            "{}",
            share_at(&levels, Level::Full)
        );
        assert!(!matches!(g.take_news(), Some(News::TheirsAfterAll { .. })));
    }

    #[test]
    fn they_step_back_up_a_level_every_two_smooth_seconds() {
        let mut g = Governor::default();
        let mut t = Instant::now();
        // Down two levels (a stall, then a second one a second later).
        g.observe(t, stall());
        t += Duration::from_secs(1);
        g.observe(t, stall());
        assert_eq!(g.level(), Level::Fps30);
        let levels = sim(&mut g, &mut t, 5, 16, film(60.0, 16, |_, _| 0.0));
        let up = levels.iter().position(|l| *l == Level::Fps60).unwrap() * 16;
        let full = levels.iter().position(|l| *l == Level::Full).unwrap() * 16;
        assert!((1_900..=2_100).contains(&up), "{up} ms");
        assert!((3_900..=4_100).contains(&full), "{full} ms");
        // A few late pictures under the line don't hold it back.
        let mut g = Governor::default();
        g.observe(t, stall());
        let levels = sim(&mut g, &mut t, 5, 16, film(60.0, 16, |_, _| 0.02));
        assert_eq!(levels.last(), Some(&Level::Full));
    }

    #[test]
    fn trouble_soon_after_a_step_up_makes_it_wait_longer_but_never_long() {
        // A computer fine at 30 fps but not at 60 or full rate: each step up
        // to 60 is followed by misses.
        let mut g = Governor::default();
        let mut t = Instant::now();
        let world = film(60.0, 16, |_, l| if l <= Level::Fps60 { 0.2 } else { 0.0 });
        let levels = sim(&mut g, &mut t, 600, 16, world);
        let mut ups = Vec::new();
        for (i, w) in levels.windows(2).enumerate() {
            if w[1] < w[0] {
                ups.push(i * 16 / 1000);
            }
        }
        let gaps: Vec<usize> = ups.windows(2).map(|w| w[1] - w[0]).collect();
        // The waits double, to about 20 s, never more.
        assert!(gaps.windows(2).all(|g| g[1] + 1 >= g[0]), "{gaps:?}");
        assert!(gaps.iter().all(|g| *g <= 23), "{gaps:?}");
        assert!(gaps.iter().rev().take(5).all(|g| *g >= 19), "{gaps:?}");
    }

    #[test]
    fn the_ladder_has_sixty_first_then_rate_then_size_then_pause() {
        let fps: Vec<Option<u32>> = Level::ALL.iter().map(|l| l.fps()).collect();
        assert_eq!(
            fps,
            vec![
                None,
                Some(60),
                Some(30),
                Some(15),
                Some(5),
                Some(5),
                Some(5),
                Some(0)
            ]
        );
        let scale: Vec<f32> = Level::ALL[..7].iter().map(|l| l.scale()).collect();
        assert_eq!(scale, vec![1.0, 1.0, 1.0, 1.0, 1.0, 0.5, 0.25]);
        // Sustained trouble walks down every step, one a window.
        let mut g = Governor::default();
        let mut t = Instant::now();
        let mut seen = vec![g.level()];
        for l in sim(&mut g, &mut t, 22, 16, film(60.0, 16, |_, _| 0.5)) {
            if seen.last() != Some(&l) {
                seen.push(l);
            }
        }
        assert_eq!(seen, Level::ALL.to_vec());
    }

    #[test]
    fn only_what_happens_while_something_plays_counts() {
        let shown = one(60, 9).shown;
        let r = Reading::of(None, true, shown, 1, true);
        assert_eq!(
            (r.shown, r.stalls, r.jolted, r.forced),
            (shown, 1, true, None)
        );
        // Nothing playing: a show loading's stall is not now.
        assert_eq!(
            Reading::of(None, false, shown, 3, false),
            Reading::default()
        );
        // Forced: the late share, playing or not.
        assert_eq!(
            Reading::of(Some(1000), false, shown, 2, false).forced,
            Some(1000)
        );
        assert_eq!(Reading::of(Some(1000), false, shown, 2, false).stalls, 0);
        assert_eq!(
            r.total(),
            Shown {
                pictures: 60,
                late: 9
            }
        );
        assert_eq!(
            Shown {
                pictures: 3,
                late: 1
            }
            .since(Shown {
                pictures: u32::MAX,
                late: 0
            }),
            Shown {
                pictures: 4,
                late: 1
            }
        );
    }

    #[test]
    fn an_outputs_own_trouble_is_learned_and_does_not_keep_the_previews_down() {
        // A screen on a basic display driver: a third of its pictures late
        // whatever the previews do.
        let mut g = Governor::default();
        let mut t = Instant::now();
        let levels = sim(&mut g, &mut t, 600, 16, film(60.0, 16, |_, _| 0.33));
        assert!(levels.contains(&Level::Paused));
        let back = levels
            .iter()
            .skip_while(|l| **l != Level::Paused)
            .position(|l| *l == Level::Full)
            .unwrap()
            * 16;
        assert!(
            back <= Governor::MEASURE.as_millis() as usize + 1_000,
            "{back} ms"
        );
        let first_full = levels.iter().rposition(|l| *l != Level::Full).unwrap_or(0) * 16;
        assert!(first_full <= 60_000, "down until {first_full} ms");
        assert!(g.not_theirs().is_some());
        // Then the previews become too much as well: down again soon.
        let world = film(60.0, 16, |_, l| if l <= Level::Fps60 { 0.9 } else { 0.33 });
        let levels = sim(&mut g, &mut t, 120, 16, world);
        let down = levels.iter().position(|l| *l > Level::Full).unwrap() * 16;
        assert!(down <= 4_000, "{down} ms");
        assert!(share_at(&levels[levels.len() / 2..], Level::Full) < 0.05);
    }

    #[test]
    fn once_the_outputs_own_trouble_stops_it_is_forgotten() {
        let mut g = Governor::default();
        let mut t = Instant::now();
        sim(&mut g, &mut t, 120, 16, film(60.0, 16, |_, _| 0.33));
        assert_eq!(g.level(), Level::Full);
        // While it goes on, it is let be...
        let levels = sim(&mut g, &mut t, 10, 16, film(60.0, 16, |_, _| 0.33));
        assert!(levels.iter().all(|l| *l == Level::Full));
        // ... but once it has stopped a good while, trouble steps down again.
        sim(&mut g, &mut t, 60 * 60, 500, |_, _| SMOOTH);
        let levels = sim(&mut g, &mut t, 4, 16, film(60.0, 16, |_, _| 0.33));
        assert!(levels.iter().any(|l| *l > Level::Full));
    }

    #[test]
    fn forced_pressure_is_never_taken_for_the_outputs_own() {
        let mut g = Governor::default();
        let mut t = Instant::now();
        let r = Reading::of(Some(1000), true, [Shown::default(); WINDOWS], 0, false);
        let levels = sim(&mut g, &mut t, 300, 50, |_, _| r);
        assert_eq!(g.level(), Level::Paused);
        assert_eq!(levels.iter().filter(|l| **l == Level::Full).count(), 0);
        assert_eq!(g.take_news(), None);
        // Under the line: nothing (Relaxed's 5 %).
        let mut g = Governor::default();
        let r = Reading::of(Some(30), true, [Shown::default(); WINDOWS], 0, false);
        assert!(
            sim(&mut g, &mut t, 10, 50, |_, _| r)
                .iter()
                .all(|l| *l == Level::Full)
        );
    }

    #[test]
    fn the_notice_says_briefly_when_the_previews_came_back_because_it_wasnt_them() {
        let auto = Limits::default();
        assert_eq!(
            notice(Level::Full, auto, true).as_deref(),
            Some("Previews back: the outputs' missed pictures aren't theirs")
        );
        // Not while they still give way.
        assert_eq!(
            notice(Level::Fps15, auto, true).as_deref(),
            Some("Preview at 15 fps to keep the outputs smooth")
        );
        let mut g = PreviewGovernor::default();
        let mut t = Instant::now();
        let mut world = film(60.0, 16, |_, _| 0.33);
        for i in 0..4000 {
            t += Duration::from_millis(16);
            g.observe(t, world(i * 16 + 16, g.governed()), Strictness::Relaxed);
        }
        assert!(matches!(g.take_news(), Some(News::NotTheirs { .. })));
        assert!(g.take_news().is_none(), "taken once");
        let words = News::TheirsAfterAll { level: Level::Full }.words();
        assert!(words.contains("after all") && words.contains("seen misses"));
    }

    #[test]
    fn each_preview_asks_and_draws_at_its_own_pace() {
        let mut p = PreviewGovernor::default();
        let t = Instant::now();
        let auto = Limits::default();
        let ask = |p: &PreviewGovernor, id, ms: u64| p.ask(id, t + Duration::from_millis(ms), auto);
        // Full: every frame, full size.
        assert_eq!(ask(&p, PreviewId::Single(0), 0), PreviewBudget::FULL);
        assert!(PreviewBudget::FULL.is_full());
        // 15 a second: a picture when due, per preview.
        p.force(Level::Fps15);
        let tile = PreviewId::Tile(0, 2);
        let three_d = PreviewId::Stage(0);
        assert!(ask(&p, tile, 0).draw);
        assert!(!ask(&p, tile, 30).draw, "not yet");
        assert!(ask(&p, three_d, 30).draw, "another preview's own time");
        // The same tile of another pane is another preview, with its own time.
        assert!(ask(&p, PreviewId::Tile(1, 2), 30).draw);
        assert!(ask(&p, PreviewId::Single(1), 30).draw);
        assert!(!ask(&p, PreviewId::Single(1), 40).draw, "and paced too");
        // A pane closed: its previews are forgotten, the others' kept.
        let before = p.known();
        p.retain_hosts(|h| h != 1);
        assert_eq!(p.known(), before - 2);
        assert!(ask(&p, PreviewId::Single(1), 41).draw, "afresh");
        assert!(!ask(&p, tile, 41).draw, "pane 0's tile still paced");
        assert!(ask(&p, tile, 70).draw);
        assert_eq!(ask(&p, tile, 70).scale, 1.0);
        // 60 a second is paced too.
        p.force(Level::Fps60);
        assert!(!ask(&p, tile, 80).draw);
        assert!(ask(&p, tile, 90).draw);
        // Smaller, then paused: nothing drawn, the last picture kept.
        p.force(Level::Quarter);
        assert_eq!(ask(&p, tile, 1000).scale, 0.25);
        p.force(Level::Paused);
        let b = ask(&p, tile, 5000);
        assert!(b.paused && !b.draw && !b.is_full());
        // The computer's limits are ceilings: the rate...
        p.force(Level::Full);
        let rate = Limits {
            rate: Cap::Fps30,
            ..auto
        };
        let b = p.ask(tile, t + Duration::from_secs(9), rate);
        assert!(!b.is_full() && (b.every.as_secs_f64() - 1.0 / 30.0).abs() < 1e-6);
        // ... and the size, at full rate too, and never above the governor's.
        let half = Limits {
            size: SizeCap::Half,
            ..auto
        };
        let b = p.ask(tile, t + Duration::from_secs(10), half);
        assert!(b.draw && b.every.is_zero() && b.scale == 0.5 && !b.is_full());
        p.force(Level::Quarter);
        assert_eq!(p.ask(tile, t + Duration::from_secs(11), half).scale, 0.25);
        assert!(p.asked.borrow().contains(&three_d));
    }

    #[test]
    fn the_limits_are_ceilings_and_the_notice_says_why() {
        let auto = Limits::default();
        let rate = |rate| Limits { rate, ..auto };
        assert_eq!(effective(Level::Full, Cap::Automatic), Level::Full);
        assert_eq!(effective(Level::Full, Cap::Fps15), Level::Fps15);
        assert_eq!(effective(Level::Full, Cap::Fps60), Level::Fps60);
        assert_eq!(effective(Level::Paused, Cap::Fps30), Level::Paused);
        assert_eq!(effective_scale(Level::Full, SizeCap::Quarter), 0.25);
        assert_eq!(effective_scale(Level::Quarter, SizeCap::Half), 0.25);
        assert_eq!(notice(Level::Full, auto, false), None);
        assert_eq!(
            notice(Level::Fps60, auto, false).as_deref(),
            Some("Preview at 60 fps to keep the outputs smooth")
        );
        assert_eq!(
            notice(Level::Half, auto, false).as_deref(),
            Some("Preview at 5 fps, half size, to keep the outputs smooth")
        );
        assert_eq!(
            notice(Level::Paused, auto, false).as_deref(),
            Some("Previews paused to protect the outputs")
        );
        assert_eq!(
            notice(Level::Full, rate(Cap::Fps30), false).as_deref(),
            Some("Preview at 30 fps (this computer's setting)")
        );
        let half = Limits {
            size: SizeCap::Half,
            ..auto
        };
        assert_eq!(
            notice(Level::Full, half, false).as_deref(),
            Some("Preview at half size (this computer's setting)")
        );
        assert_eq!(
            notice(Level::Quarter, half, false).as_deref(),
            Some("Preview at 5 fps, quarter size, to keep the outputs smooth")
        );
        // The main window: never under what its own controls need.
        assert_eq!(window_fps(Level::Full, 60.0, 20.0), 60.0);
        assert_eq!(window_fps(Level::Fps60, 120.0, 20.0), 60.0);
        assert_eq!(window_fps(Level::Fps30, 60.0, 20.0), 30.0);
        assert_eq!(window_fps(Level::Fps5, 60.0, 20.0), 20.0);
        assert_eq!(window_fps(Level::Paused, 60.0, 20.0), 20.0);
        assert_eq!(Level::Fps5.every(), Duration::from_millis(200));
        assert_eq!(Level::Full.every(), Duration::ZERO);
        // Paused waits a day, and an Instant can have that added.
        let _ = Instant::now() + Level::Paused.every();
    }

    #[test]
    fn the_settings_read_a_later_newqs_choice_as_the_default() {
        let read = |s: &str| serde_json::from_str::<Strictness>(s).unwrap();
        assert_eq!(read("\"Strict\""), Strictness::Strict);
        assert_eq!(read("\"Off\""), Strictness::Off);
        assert_eq!(read("\"Paranoid\""), Strictness::Relaxed);
        assert_eq!(read("7"), Strictness::Relaxed);
        let size = |s: &str| serde_json::from_str::<SizeCap>(s).unwrap();
        assert_eq!(size("\"Half\""), SizeCap::Half);
        assert_eq!(size("\"Eighth\""), SizeCap::Full);
        let cap = |s: &str| serde_json::from_str::<Cap>(s).unwrap();
        assert_eq!(cap("\"Fps60\""), Cap::Fps60);
        assert_eq!(cap("\"Fps1\""), Cap::Automatic);
        // Every choice has its sentence.
        for s in Strictness::ALL {
            assert!(s.words().ends_with('.'));
        }
        assert!(Strictness::Off.words().contains("stutter"));
        for s in SizeCap::ALL {
            assert!(s.words().ends_with('.'));
        }
        for c in Cap::ALL {
            assert!(c.words().ends_with('.'));
        }
    }

    #[test]
    fn preview_only_uploads_follow_the_previews_rate_and_stop_when_paused() {
        // Last uploaded at `t`; is one due `n` ms later?
        let t = Instant::now();
        let due = |level, n| uploads_due(level, Some(t), t + Duration::from_millis(n));
        assert!(due(Level::Full, 1));
        assert!(uploads_due(Level::Paused, None, t), "the first picture");
        assert!(!due(Level::Paused, 60_000));
        assert!(!due(Level::Fps15, 30));
        assert!(due(Level::Fps15, 66));
        assert!(!due(Level::Half, 100));
        assert!(due(Level::Half, 200));
    }

    #[test]
    fn the_pressure_can_be_forced_for_trying_it() {
        assert_eq!(forced_pressure(None), None);
        assert_eq!(forced_pressure(Some("")), None);
        assert_eq!(forced_pressure(Some("1")), Some(1000));
        assert_eq!(forced_pressure(Some("30")), Some(30));
        assert_eq!(forced_pressure(Some("on")), Some(1000));
        assert_eq!(forced_pressure(Some("5000")), Some(1000));
    }
}
