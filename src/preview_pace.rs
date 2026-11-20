//! Every preview gives way before the outputs suffer. While an output window
//! (a projector) shows the first sign of trouble (a late frame, a frame held up
//! presenting) or the main loop that feeds them stalls, the previews draw less:
//! fewer pictures a second (60, 30, 15, 5), then smaller (half, a quarter of
//! their size), then not at all (paused, the last picture kept); when the
//! outputs have been smooth for a while they step back up, one step at a time.
//! The outputs' own drawing never reads any of this.
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
//! the `Level`; `Cap` is the per-computer ceiling (`Prefs::preview_cap`).

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// How the previews are drawn: each level draws less than the one before.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    /// Every frame the main window draws (60 a second while playing), full size.
    #[default]
    Full,
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
    pub const ALL: [Level; 7] = [
        Level::Full,
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

/// This computer's ceiling on the previews' rate (Settings ▸ General).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub enum Cap {
    /// Full rate unless the outputs need the room. (A limit from a later NewQ
    /// that this one doesn't know reads as this.)
    #[default]
    Automatic,
    Fps30,
    Fps15,
    Fps5,
}

impl<'de> Deserialize<'de> for Cap {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Cap, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        Ok(Cap::ALL
            .into_iter()
            .find(|c| serde_json::to_value(c).ok().as_ref() == Some(&v))
            .unwrap_or_default())
    }
}

impl Cap {
    pub const ALL: [Cap; 4] = [Cap::Automatic, Cap::Fps30, Cap::Fps15, Cap::Fps5];

    pub fn is_automatic(&self) -> bool {
        *self == Cap::Automatic
    }

    /// The level the previews never go above.
    pub fn level(self) -> Level {
        match self {
            Cap::Automatic => Level::Full,
            Cap::Fps30 => Level::Fps30,
            Cap::Fps15 => Level::Fps15,
            Cap::Fps5 => Level::Fps5,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Cap::Automatic => "Automatic",
            Cap::Fps30 => "30 fps at most",
            Cap::Fps15 => "15 fps at most",
            Cap::Fps5 => "5 fps at most",
        }
    }
}

/// What was measured since the last look.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Reading {
    /// Output frames that were late (took clearly longer than a refresh, the
    /// wait to present included) since the last reading (`render::late_frames_total`).
    pub late_frames: u32,
    /// The worst output window drawing now: its late frames over its last
    /// second's frames, in thousandths (`render::late_frames_drawing`).
    pub late_permille: u32,
    /// Main-loop stalls (over 100 ms) since the last reading.
    pub stalls: u32,
    /// Made up (`NEWQ_PREVIEW_PRESSURE`): never learned as the outputs' own
    /// trouble, so the previews stay given way for as long as it lasts.
    pub forced: bool,
}

impl Reading {
    /// What the governor is told on a pass, but only while something plays (a
    /// share left from the last cue's end, or a stall while a show loads, says
    /// nothing about now). `forced` (`NEWQ_PREVIEW_PRESSURE`) stands for the
    /// late share whether anything plays or not.
    pub fn of(
        forced: Option<u32>,
        playing: bool,
        late_frames: u32,
        late_permille: u32,
        stalls: u32,
    ) -> Reading {
        match forced {
            Some(f) => Reading {
                late_frames: 0,
                late_permille: f,
                stalls: if playing { stalls } else { 0 },
                forced: true,
            },
            None if playing => Reading {
                late_frames,
                late_permille,
                stalls,
                forced: false,
            },
            None => Reading::default(),
        }
    }

    /// The first sign of trouble: one late output frame, a late share, or the
    /// main loop held up. The previews give way at once.
    fn pressed(self) -> bool {
        self.late_frames > 0 || self.late_permille >= Governor::PRESSED_AT || self.stalls > 0
    }

    /// Clearly smooth: no late frame, no stall. Between the two (a share
    /// under `PRESSED_AT` but over `CALM_AT`), nothing changes (hysteresis).
    fn calm(self) -> bool {
        self.late_frames == 0 && self.late_permille <= Governor::CALM_AT && self.stalls == 0
    }
}

/// Steps the previews down fast and up slowly, and learns when giving way
/// doesn't help.
///
/// The first sign of trouble steps them down at once, a step a second while it
/// goes on. Some trouble isn't theirs: a system compositor's hiccup, a lid
/// change, a screen on a basic display driver drops a frame every few seconds
/// whatever the previews do. So once the previews have stood at a level for
/// `MEASURE` (in practice paused, since trouble anywhere else steps them lower)
/// and the outputs still have trouble there at about the rate they had on the
/// way down, the previews were not the cause: they go straight back up (to
/// `ceiling`) and that rate is remembered as the outputs' own (`Floor`). From
/// then on, at that level or above, trouble at up to about that rate is let
/// be; clearly more (`surge`) steps them down at once, as before. Should the
/// trouble surge within `MEASURE` of going back up, the previews were the
/// cause after all: down again, and the next return waits twice as long and
/// stops a level lower. The remembered rate fades (`FLOOR_HALF_LIFE`), so the
/// question is asked again now and then.
#[derive(Clone, Debug, Default)]
pub struct Governor {
    level: Level,
    /// When the level last changed.
    changed: Option<Instant>,
    /// Since when every reading has been calm.
    calm_since: Option<Instant>,
    /// How long it must be calm before a step up; doubled each time a step up
    /// is followed soon by trouble (so it doesn't flap), back to the start
    /// after a long smooth stretch at full rate.
    patience: Duration,
    /// When it last stepped up.
    stepped_up: Option<Instant>,
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
    /// tried (`FORGIVE_AFTER`, doubled by each try that failed).
    raise_wait: Duration,
    /// Trouble in the last `RECENT`, by when (for the surge test).
    recent: VecDeque<(Instant, u32)>,
    /// When a late share alone last counted as trouble.
    share_counted: Option<Instant>,
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
                "previews: the outputs' late frames and stalls went on while the previews were at {at:?} \
                 ({per_minute:.1} a minute; {before:.1} on the way down): not the previews' doing; \
                 back to {back_to:?}, letting about that rate be"
            ),
            News::TheirsAfterAll { level } => format!(
                "previews: back at {level:?} the outputs' trouble rose: the previews' doing after all; \
                 giving way again (the next return waits longer, a level lower)"
            ),
            News::Surge { level } => format!(
                "previews: at {level:?} the outputs' trouble rose clearly above what they have by themselves: giving way again"
            ),
        }
    }
}

impl Governor {
    /// A late share (thousandths) that steps the previews down, even with no
    /// single late frame seen since the last look.
    pub const PRESSED_AT: u32 = 20;
    /// And at or under which the outputs count as smooth.
    pub const CALM_AT: u32 = 10;
    /// No faster than this between steps down: the last step must have had
    /// time to tell.
    pub const STEP_DOWN_EVERY: Duration = Duration::from_secs(1);
    /// How long smooth before the first step up.
    pub const PATIENCE: Duration = Duration::from_secs(10);
    /// The most it waits before a step up.
    pub const PATIENCE_MOST: Duration = Duration::from_secs(160);
    /// Trouble this soon after a step up makes the next wait longer.
    pub const RELAPSE_WITHIN: Duration = Duration::from_secs(30);
    /// Smooth at full rate this long: the wait is back to `PATIENCE`. And a
    /// learned floor under full rate that held this long tries the level above.
    pub const FORGIVE_AFTER: Duration = Duration::from_secs(300);
    /// How long a level is stood at, trouble still coming, before it is judged
    /// whether giving way helped (doubled by each failed return, up to
    /// `PATIENCE_MOST`).
    pub const MEASURE: Duration = Duration::from_secs(15);
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
    /// The longest a floor under full rate holds before the level above is tried.
    pub const RAISE_MOST: Duration = Duration::from_secs(2400);
    /// Under this many a second, the learned rate is forgotten: the first
    /// sign of trouble steps the previews down again.
    pub const FORGET_UNDER: f64 = 0.02;

    pub fn level(&self) -> Level {
        self.level
    }

    /// What it last decided beyond a step, once (for the log).
    pub fn take_news(&mut self) -> Option<News> {
        self.news.take()
    }

    /// When it last found the outputs' trouble wasn't the previews' doing.
    pub fn not_theirs(&self) -> Option<Instant> {
        self.not_theirs
    }

    /// Take a reading made at `now`; the level the previews are to draw at.
    pub fn observe(&mut self, now: Instant, r: Reading) -> Level {
        if self.patience.is_zero() {
            self.patience = Self::PATIENCE;
        }
        if self.measure.is_zero() {
            self.measure = Self::MEASURE;
        }
        if self.raise_wait.is_zero() {
            self.raise_wait = Self::FORGIVE_AFTER;
        }
        // How much trouble this is: each late frame and stall, and a late
        // share alone (it lasts a second after one late frame) once a second.
        let mut weight = r.late_frames.saturating_add(r.stalls);
        if weight == 0
            && r.pressed()
            && self
                .share_counted
                .is_none_or(|t| now.saturating_duration_since(t) >= Duration::from_secs(1))
        {
            self.share_counted = Some(now);
            weight = 1;
        }
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
        let floor = self.floor.filter(|f| !r.forced && self.level <= f.level);
        if let Some(f) = &mut self.floor
            && floor.is_some()
            && !f.trying
        {
            f.seen += u64::from(weight);
        }
        let since = |t: Option<Instant>| t.map(|t| now.saturating_duration_since(t));
        if r.pressed() {
            self.calm_since = None;
            let may = since(self.changed).is_none_or(|d| d >= Self::STEP_DOWN_EVERY);
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
                                self.measure = (self.measure * 2).min(Self::PATIENCE_MOST);
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
        } else if r.calm() {
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
        } else {
            // Neither: hold where it is, and the calm starts again.
            self.calm_since = None;
        }
        if !r.forced {
            self.learn(now);
        }
        self.level
    }

    /// Clearly more trouble than the outputs have by themselves (`f`) in the
    /// last `RECENT` (since the previews last went up on its say-so).
    fn surge(&self, now: Instant, f: &Floor) -> bool {
        let seen: u64 = self
            .recent
            .iter()
            .filter(|(t, _)| *t > f.since)
            .map(|(_, w)| u64::from(*w))
            .sum();
        seen as f64
            > Self::SLACK + Self::SURGE_FACTOR * f.rate_now(now) * Self::RECENT.as_secs_f64()
    }

    /// Whether giving way helped, once there is enough to tell; and the
    /// floor's own course (settling, trying the level above).
    fn learn(&mut self, now: Instant) {
        if let Some(f) = &mut self.floor
            && f.trying
            && now.saturating_duration_since(f.since) >= self.measure
        {
            if f.fallback.take().is_some() {
                self.raise_wait = Self::FORGIVE_AFTER;
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

    /// Be at `level` from `now`.
    fn move_to(&mut self, now: Instant, level: Level) {
        self.level = level;
        self.changed = Some(now);
        self.here = 0;
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
    /// Take a reading (the app, each pass); the governed level.
    pub fn observe(&mut self, now: Instant, r: Reading) -> Level {
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

    /// May preview `id` draw a picture at `now`, and how big? Under `cap`.
    pub fn ask(&self, id: PreviewId, now: Instant, cap: Cap) -> PreviewBudget {
        #[cfg(test)]
        self.asked.borrow_mut().push(id);
        let level = effective(self.governed(), cap);
        if level == Level::Full {
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
            scale: level.scale(),
            every,
            paused,
        }
    }

    /// What the Preview pane's header says, if anything: only while the
    /// previews draw less than they would.
    pub fn notice(&self, cap: Cap) -> Option<String> {
        let lately = self
            .governor
            .not_theirs()
            .is_some_and(|t| t.elapsed() < NOT_THEIRS_SAID);
        notice(self.governed(), cap, lately)
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

/// What the Preview pane's header says while the previews draw less, or
/// (`not_theirs`: for a while after) that they came back because the
/// outputs' late frames weren't their doing.
pub fn notice(governed: Level, cap: Cap, not_theirs: bool) -> Option<String> {
    let level = effective(governed, cap);
    let why = if governed > cap.level() {
        "to keep the outputs smooth"
    } else {
        "(this computer's setting)"
    };
    let size = match level {
        Level::Half => ", half size,",
        Level::Quarter => ", quarter size,",
        _ => "",
    };
    match level {
        _ if not_theirs && governed <= cap.level() => {
            Some("Previews back: the outputs' late frames aren't theirs".to_string())
        }
        Level::Full => None,
        Level::Paused => Some("Previews paused to protect the outputs".to_string()),
        _ => Some(format!("Preview at {} fps{size} {why}", level.fps()?)),
    }
}

/// `NEWQ_PREVIEW_PRESSURE`: for trying the preview's steps without a slow
/// computer. `1` (or `on`) is every output frame late; another number is the
/// late frames in thousandths. Unset or empty: what was measured.
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

    const LATE: Reading = Reading {
        late_frames: 3,
        late_permille: 200,
        stalls: 0,
        forced: false,
    };
    const ONE_LATE: Reading = Reading {
        late_frames: 1,
        late_permille: 0,
        stalls: 0,
        forced: false,
    };
    const SMOOTH: Reading = Reading {
        late_frames: 0,
        late_permille: 0,
        stalls: 0,
        forced: false,
    };
    const SOME: Reading = Reading {
        late_frames: 0,
        late_permille: 15,
        stalls: 0,
        forced: false,
    };

    /// Feed `r` every 100 ms for `secs`; the levels seen, in order, without
    /// repeats, the one before first.
    fn run(g: &mut Governor, t: &mut Instant, secs: f32, r: Reading) -> Vec<Level> {
        let mut seen: Vec<Level> = vec![g.level()];
        for _ in 0..(secs * 10.0).round() as u32 {
            *t += Duration::from_millis(100);
            let l = g.observe(*t, r);
            if seen.last() != Some(&l) {
                seen.push(l);
            }
        }
        seen
    }

    #[test]
    fn smooth_outputs_leave_the_previews_at_full_rate() {
        let mut g = Governor::default();
        let mut t = Instant::now();
        assert_eq!(run(&mut g, &mut t, 600.0, SMOOTH), vec![Level::Full]);
        // A small late share and no late frame: under the line, no change.
        assert_eq!(run(&mut g, &mut t, 60.0, SOME), vec![Level::Full]);
    }

    #[test]
    fn the_first_late_output_frame_steps_the_previews_down_at_once() {
        let mut g = Governor::default();
        let t = Instant::now();
        assert_eq!(g.observe(t, ONE_LATE), Level::Fps30);
        // A main-loop stall too.
        let mut g = Governor::default();
        let stall = Reading {
            stalls: 1,
            ..SMOOTH
        };
        assert_eq!(g.observe(t, stall), Level::Fps30);
    }

    #[test]
    fn rate_goes_first_then_size_then_they_pause_a_step_a_second() {
        let mut g = Governor::default();
        let mut t = Instant::now();
        let seen = run(&mut g, &mut t, 10.0, LATE);
        assert_eq!(seen, Level::ALL.to_vec(), "every step, in order");
        // The rate falls before the size does, and the size before the pause.
        let fps: Vec<Option<u32>> = Level::ALL.iter().map(|l| l.fps()).collect();
        assert_eq!(
            fps,
            vec![None, Some(30), Some(15), Some(5), Some(5), Some(5), Some(0)]
        );
        let scale: Vec<f32> = Level::ALL[..6].iter().map(|l| l.scale()).collect();
        assert_eq!(scale, vec![1.0, 1.0, 1.0, 1.0, 0.5, 0.25]);
        // One step a second, no faster.
        let mut g = Governor::default();
        let mut t = Instant::now();
        assert_eq!(
            run(&mut g, &mut t, 0.9, LATE),
            vec![Level::Full, Level::Fps30]
        );
        assert_eq!(
            run(&mut g, &mut t, 0.1, LATE),
            vec![Level::Fps30],
            "not yet"
        );
        assert_eq!(
            run(&mut g, &mut t, 0.1, LATE),
            vec![Level::Fps30, Level::Fps15]
        );
        // Down to paused; and since the outputs go on just as late with the
        // previews paused, they weren't the cause: back to full rate.
        let seen = run(&mut g, &mut t, 30.0, LATE);
        assert!(seen.contains(&Level::Paused), "{seen:?}");
        assert_eq!(seen.last(), Some(&Level::Full), "{seen:?}");
    }

    #[test]
    fn it_steps_back_up_one_level_at_a_time_after_ten_smooth_seconds() {
        let mut g = Governor::default();
        let mut t = Instant::now();
        run(&mut g, &mut t, 1.5, LATE);
        assert_eq!(g.level(), Level::Fps15);
        assert_eq!(run(&mut g, &mut t, 9.5, SMOOTH), vec![Level::Fps15]);
        assert_eq!(
            run(&mut g, &mut t, 1.0, SMOOTH),
            vec![Level::Fps15, Level::Fps30]
        );
        assert_eq!(
            run(&mut g, &mut t, 11.0, SMOOTH),
            vec![Level::Fps30, Level::Full]
        );
        // In between the lines: it holds, and the wait starts again.
        g.observe(t + Duration::from_secs(1), ONE_LATE);
        t += Duration::from_secs(1);
        assert_eq!(g.level(), Level::Fps30);
        run(&mut g, &mut t, 8.0, SMOOTH);
        assert_eq!(run(&mut g, &mut t, 5.0, SOME), vec![Level::Fps30]);
        assert_eq!(
            run(&mut g, &mut t, 8.0, SMOOTH),
            vec![Level::Fps30],
            "the calm began again"
        );
    }

    #[test]
    fn trouble_soon_after_a_step_up_makes_it_wait_longer_so_it_does_not_flap() {
        // A computer that is fine at 30 fps but not at 60: each step up is
        // followed by late frames.
        let mut g = Governor::default();
        let mut t = Instant::now();
        run(&mut g, &mut t, 0.5, LATE);
        assert_eq!(g.level(), Level::Fps30);
        let mut ups = Vec::new();
        let start = t;
        for _ in 0..2000 {
            t += Duration::from_millis(100);
            let before = g.level();
            let r = if g.level() == Level::Full {
                LATE
            } else {
                SMOOTH
            };
            if g.observe(t, r) < before {
                ups.push(t.duration_since(start).as_secs());
            }
        }
        // 200 s: with a fixed 10 s wait it would go up about 18 times.
        assert!(ups.len() <= 5, "stepped up at {ups:?}");
        let gaps: Vec<u64> = ups.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(gaps.windows(2).all(|g| g[1] >= g[0]), "{gaps:?}");
    }

    #[test]
    fn only_what_happens_while_something_plays_counts() {
        let r = Reading::of;
        assert_eq!(
            r(None, true, 2, 200, 1),
            Reading {
                late_frames: 2,
                late_permille: 200,
                stalls: 1,
                forced: false,
            }
        );
        // Nothing playing: the last cue's share and a show loading's stall are not now.
        assert_eq!(r(None, false, 2, 200, 3), Reading::default());
        // Forced: the late share, playing or not.
        assert_eq!(r(Some(1000), false, 0, 0, 2).late_permille, 1000);
        assert_eq!(r(Some(1000), false, 0, 0, 2).stalls, 0);
        // And an idle computer goes back up.
        let mut g = Governor::default();
        let mut t = Instant::now();
        run(&mut g, &mut t, 10.0, LATE);
        assert_eq!(g.level(), Level::Paused);
        run(&mut g, &mut t, 70.0, r(None, false, 5, 400, 1));
        assert_eq!(g.level(), Level::Full);
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

    /// An output that drops a frame every `period` ms whatever the previews
    /// do (a compositor's hiccup), its late share `tail` ‰ for the second
    /// after, and every third one a main-loop stall too.
    fn hiccups(period: u64, tail: u32, step: u64) -> impl FnMut(u64, Level) -> Reading {
        move |ms, _| {
            let n = ms / period;
            let into = ms % period;
            if n == 0 {
                SMOOTH
            } else if into < step {
                Reading {
                    late_frames: 1,
                    late_permille: tail,
                    stalls: u32::from(n.is_multiple_of(3)),
                    forced: false,
                }
            } else if into < 1000 {
                Reading {
                    late_permille: tail,
                    ..SMOOTH
                }
            } else {
                SMOOTH
            }
        }
    }

    /// The share of `levels` at full rate.
    fn full_share(levels: &[Level]) -> f64 {
        levels.iter().filter(|l| **l == Level::Full).count() as f64 / levels.len() as f64
    }

    #[test]
    fn late_frames_the_previews_dont_cause_dont_keep_them_down() {
        // The review's case: one late frame every 2, 5 or 9 s had the previews
        // paused after 10, 25 or 45 s, for the rest of the show.
        for period in [2_000, 5_000, 9_000] {
            for tail in [17, 33] {
                for step in [16, 100] {
                    let mut g = Governor::default();
                    let mut t = Instant::now();
                    let levels = sim(&mut g, &mut t, 1200, step, hiccups(period, tail, step));
                    let at = |s: u64| (s * 1000 / step) as usize;
                    let what =
                        format!("every {period} ms, tail {tail}‰, a reading every {step} ms");
                    // The first sign still steps them down at once...
                    assert_eq!(levels[at(period / 1000) + 1], Level::Fps30, "{what}");
                    // ... and they are back at full rate within a bounded time
                    // (paused at most `MEASURE` and a little)...
                    let back = levels[at(period / 1000)..]
                        .iter()
                        .position(|l| *l == Level::Full)
                        .map(|i| i * step as usize / 1000);
                    let bound = 6 * period / 1000 + Governor::MEASURE.as_secs() + 10;
                    assert!(
                        back.is_some_and(|s| s as u64 <= bound),
                        "{what}: back after {back:?} s"
                    );
                    let paused =
                        levels.iter().filter(|l| **l == Level::Paused).count() * step as usize;
                    assert!(paused <= 20_000, "{what}: paused {paused} ms");
                    // ... and stay up.
                    let rest = &levels[at(back.unwrap() as u64 + period / 1000)..];
                    assert!(full_share(rest) > 0.95, "{what}: {}", full_share(rest));
                    assert!(g.not_theirs().is_some(), "{what}");
                }
            }
        }
    }

    #[test]
    fn when_the_previews_are_the_cause_they_stay_down_and_go_up_carefully() {
        // Full rate and 30 a second are too much for this computer; 15 is fine.
        let world = |_: u64, l: Level| if l <= Level::Fps30 { LATE } else { SMOOTH };
        let mut g = Governor::default();
        let mut t = Instant::now();
        let levels = sim(&mut g, &mut t, 1200, 50, world);
        assert_eq!(g.take_news(), None, "nothing learned: giving way helped");
        assert!(g.not_theirs().is_none());
        // Never paused: 15 a second is enough.
        assert!(levels.iter().all(|l| *l <= Level::Fps15), "{levels:?}");
        // Mostly at 15: each try at 30 is cut short, and the tries thin out.
        let troubled = levels.iter().filter(|l| **l <= Level::Fps30).count() as f64;
        assert!(troubled / (levels.len() as f64) < 0.02, "{troubled}");
        let mut ups = Vec::new();
        for (i, w) in levels.windows(2).enumerate() {
            if w[1] < w[0] {
                ups.push(i * 50 / 1000);
            }
        }
        // The waits double to `PATIENCE_MOST`: about one try in 160 s after.
        assert!(ups.len() <= 12, "went up at {ups:?} s");
        let gaps: Vec<usize> = ups.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(
            gaps.iter().skip(3).all(|g| *g >= 150),
            "went up at {ups:?} s"
        );
        // Each time it relapses, it goes down again at once.
        let mut run_at_30 = 0;
        for l in &levels {
            if *l == Level::Fps30 {
                run_at_30 += 1;
                assert!(run_at_30 * 50 <= 1100, "stayed at 30 too long");
            } else {
                run_at_30 = 0;
            }
        }
    }

    #[test]
    fn previews_that_cause_some_of_it_settle_where_they_do_not() {
        // Full rate is too much; and the outputs hiccup every 3 s besides.
        let mut hiccup = hiccups(3_000, 33, 50);
        let world = move |ms: u64, l: Level| {
            let r = hiccup(ms, l);
            if l == Level::Full { LATE } else { r }
        };
        let mut g = Governor::default();
        let mut t = Instant::now();
        let levels = sim(&mut g, &mut t, 1200, 50, world);
        // After learning, mostly at 30 a second: neither full (its trouble)
        // nor lower (the hiccups aren't the previews').
        let last = &levels[levels.len() / 2..];
        let at_30 = last.iter().filter(|l| **l == Level::Fps30).count() as f64;
        assert!(
            at_30 / last.len() as f64 > 0.9,
            "{}",
            at_30 / last.len() as f64
        );
        assert!(full_share(last) < 0.01, "{}", full_share(last));
    }

    #[test]
    fn learned_hiccups_do_not_flap_and_new_trouble_still_steps_down_at_once() {
        // Thirty minutes of a late frame every 2 s.
        let mut g = Governor::default();
        let mut t = Instant::now();
        let levels = sim(&mut g, &mut t, 1800, 50, hiccups(2_000, 33, 50));
        let changes = levels.windows(2).filter(|w| w[0] != w[1]).count();
        assert!(changes <= 40, "{changes} changes");
        assert!(full_share(&levels) > 0.9, "{}", full_share(&levels));
        assert!(matches!(g.take_news(), Some(News::NotTheirs { .. })));
        assert_eq!(g.level(), Level::Full);
        // Then the previews become too much: down at once, and they stay down.
        let mut hiccup = hiccups(2_000, 33, 50);
        let world = move |ms: u64, l: Level| {
            let r = hiccup(ms, l);
            if l <= Level::Fps30 { LATE } else { r }
        };
        let levels = sim(&mut g, &mut t, 120, 50, world);
        let down = levels.iter().position(|l| *l > Level::Full).unwrap();
        assert!(down * 50 <= 1500, "{} ms", down * 50);
        assert!(full_share(&levels[levels.len() / 4..]) < 0.02);
    }

    #[test]
    fn once_the_outputs_own_trouble_stops_it_is_forgotten() {
        let mut g = Governor::default();
        let mut t = Instant::now();
        sim(&mut g, &mut t, 300, 50, hiccups(2_000, 33, 50));
        assert_eq!(g.level(), Level::Full);
        // While it goes on, a late frame is let be...
        g.observe(t + Duration::from_millis(50), ONE_LATE);
        assert_eq!(g.level(), Level::Full);
        // ... but once it has stopped a while, the first sign steps down again.
        sim(&mut g, &mut t, 45 * 60, 100, |_, _| SMOOTH);
        t += Duration::from_millis(100);
        assert_eq!(g.observe(t, ONE_LATE), Level::Fps30);
    }

    #[test]
    fn forced_pressure_is_never_taken_for_the_outputs_own() {
        let mut g = Governor::default();
        let mut t = Instant::now();
        let r = Reading::of(Some(1000), true, 0, 0, 0);
        assert!(r.forced);
        let levels = sim(&mut g, &mut t, 300, 50, |_, _| r);
        assert_eq!(g.level(), Level::Paused);
        assert_eq!(levels.iter().filter(|l| **l == Level::Full).count(), 0);
        assert_eq!(g.take_news(), None);
    }

    #[test]
    fn the_notice_says_briefly_when_the_previews_came_back_because_it_wasnt_them() {
        assert_eq!(
            notice(Level::Full, Cap::Automatic, true).as_deref(),
            Some("Previews back: the outputs' late frames aren't theirs")
        );
        // Not while they still give way, nor under the computer's own limit
        // when the governor holds them lower.
        assert_eq!(
            notice(Level::Fps15, Cap::Automatic, true).as_deref(),
            Some("Preview at 15 fps to keep the outputs smooth")
        );
        let mut g = PreviewGovernor::default();
        let mut t = Instant::now();
        let mut world = hiccups(2_000, 33, 50);
        for i in 0..1200 {
            t += Duration::from_millis(50);
            g.observe(t, world(i * 50 + 50, g.governed()));
        }
        assert!(matches!(g.take_news(), Some(News::NotTheirs { .. })));
        assert!(g.take_news().is_none(), "taken once");
        assert!(
            News::TheirsAfterAll { level: Level::Full }
                .words()
                .contains("after all")
        );
    }

    #[test]
    fn each_preview_asks_and_draws_at_its_own_pace() {
        let mut p = PreviewGovernor::default();
        let t = Instant::now();
        let ask = |p: &PreviewGovernor, id, ms: u64| {
            p.ask(id, t + Duration::from_millis(ms), Cap::Automatic)
        };
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
        // Smaller, then paused: nothing drawn, the last picture kept.
        p.force(Level::Quarter);
        assert_eq!(ask(&p, tile, 1000).scale, 0.25);
        p.force(Level::Paused);
        let b = ask(&p, tile, 5000);
        assert!(b.paused && !b.draw && !b.is_full());
        // The computer's cap is a ceiling.
        p.force(Level::Full);
        let b = p.ask(tile, t + Duration::from_secs(9), Cap::Fps30);
        assert!(!b.is_full() && (b.every.as_secs_f64() - 1.0 / 30.0).abs() < 1e-6);
        assert!(p.asked.borrow().contains(&three_d));
    }

    #[test]
    fn the_cap_is_a_ceiling_and_the_notice_says_why() {
        assert_eq!(effective(Level::Full, Cap::Automatic), Level::Full);
        assert_eq!(effective(Level::Full, Cap::Fps15), Level::Fps15);
        assert_eq!(effective(Level::Paused, Cap::Fps30), Level::Paused);
        assert_eq!(notice(Level::Full, Cap::Automatic, false), None);
        assert_eq!(
            notice(Level::Fps15, Cap::Automatic, false).as_deref(),
            Some("Preview at 15 fps to keep the outputs smooth")
        );
        assert_eq!(
            notice(Level::Half, Cap::Automatic, false).as_deref(),
            Some("Preview at 5 fps, half size, to keep the outputs smooth")
        );
        assert_eq!(
            notice(Level::Paused, Cap::Automatic, false).as_deref(),
            Some("Previews paused to protect the outputs")
        );
        assert_eq!(
            notice(Level::Full, Cap::Fps30, false).as_deref(),
            Some("Preview at 30 fps (this computer's setting)")
        );
        // The main window: never under what its own controls need.
        assert_eq!(window_fps(Level::Full, 60.0, 20.0), 60.0);
        assert_eq!(window_fps(Level::Fps30, 60.0, 20.0), 30.0);
        assert_eq!(window_fps(Level::Fps5, 60.0, 20.0), 20.0);
        assert_eq!(window_fps(Level::Paused, 60.0, 20.0), 20.0);
        assert_eq!(Level::Fps5.every(), Duration::from_millis(200));
        assert_eq!(Level::Full.every(), Duration::ZERO);
        // Paused waits a day, and an Instant can have that added.
        let _ = Instant::now() + Level::Paused.every();
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
