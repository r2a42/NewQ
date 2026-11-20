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
use std::collections::HashMap;
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
            },
            None if playing => Reading {
                late_frames,
                late_permille,
                stalls,
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

/// Steps the previews down fast and up slowly.
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
    /// Smooth at full rate this long: the wait is back to `PATIENCE`.
    pub const FORGIVE_AFTER: Duration = Duration::from_secs(300);

    pub fn level(&self) -> Level {
        self.level
    }

    /// Take a reading made at `now`; the level the previews are to draw at.
    pub fn observe(&mut self, now: Instant, r: Reading) -> Level {
        if self.patience.is_zero() {
            self.patience = Self::PATIENCE;
        }
        let since = |t: Option<Instant>| t.map(|t| now.saturating_duration_since(t));
        if r.pressed() {
            self.calm_since = None;
            let may = since(self.changed).is_none_or(|d| d >= Self::STEP_DOWN_EVERY);
            if may && self.level != Level::Paused {
                if since(self.stepped_up).is_some_and(|d| d < Self::RELAPSE_WITHIN) {
                    self.patience = (self.patience * 2).min(Self::PATIENCE_MOST);
                    self.stepped_up = None;
                }
                self.level = self.level.down();
                self.changed = Some(now);
            }
        } else if r.calm() {
            let calm_since = *self.calm_since.get_or_insert(now);
            let calm = now.saturating_duration_since(calm_since);
            let settled = since(self.changed).is_none_or(|d| d >= self.patience);
            if self.level != Level::Full && calm >= self.patience && settled {
                self.level = self.level.up();
                self.changed = Some(now);
                self.stepped_up = Some(now);
                self.calm_since = Some(now);
            } else if self.level == Level::Full && calm >= Self::FORGIVE_AFTER {
                self.patience = Self::PATIENCE;
            }
        } else {
            // Neither: hold where it is, and the calm starts again.
            self.calm_since = None;
        }
        self.level
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
    /// The Preview pane's single output.
    Single,
    /// A multiview tile, by its place.
    Tile(u8),
    /// The timeline workspace's monitor.
    Workspace,
    /// Any other kind (a 3D view), by its name and number. (None yet.)
    #[allow(dead_code)]
    Other(&'static str, u32),
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

    /// The level the governor has set (before the computer's cap).
    pub fn governed(&self) -> Level {
        self.governor.level()
    }

    /// For tests: be at `level`.
    #[cfg(test)]
    pub fn force(&mut self, level: Level) {
        self.governor.level = level;
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
        notice(self.governed(), cap)
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

/// What the Preview pane's header says while the previews draw less.
pub fn notice(governed: Level, cap: Cap) -> Option<String> {
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
    };
    const ONE_LATE: Reading = Reading {
        late_frames: 1,
        late_permille: 0,
        stalls: 0,
    };
    const SMOOTH: Reading = Reading {
        late_frames: 0,
        late_permille: 0,
        stalls: 0,
    };
    const SOME: Reading = Reading {
        late_frames: 0,
        late_permille: 15,
        stalls: 0,
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
        assert_eq!(run(&mut g, &mut t, 30.0, LATE).last(), Some(&Level::Paused));
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
                stalls: 1
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

    #[test]
    fn each_preview_asks_and_draws_at_its_own_pace() {
        let mut p = PreviewGovernor::default();
        let t = Instant::now();
        let ask = |p: &PreviewGovernor, id, ms: u64| {
            p.ask(id, t + Duration::from_millis(ms), Cap::Automatic)
        };
        // Full: every frame, full size.
        assert_eq!(ask(&p, PreviewId::Single, 0), PreviewBudget::FULL);
        assert!(PreviewBudget::FULL.is_full());
        // 15 a second: a picture when due, per preview.
        p.force(Level::Fps15);
        let tile = PreviewId::Tile(2);
        let three_d = PreviewId::Other("3d", 0);
        assert!(ask(&p, tile, 0).draw);
        assert!(!ask(&p, tile, 30).draw, "not yet");
        assert!(ask(&p, three_d, 30).draw, "another preview's own time");
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
        assert_eq!(notice(Level::Full, Cap::Automatic), None);
        assert_eq!(
            notice(Level::Fps15, Cap::Automatic).as_deref(),
            Some("Preview at 15 fps to keep the outputs smooth")
        );
        assert_eq!(
            notice(Level::Half, Cap::Automatic).as_deref(),
            Some("Preview at 5 fps, half size, to keep the outputs smooth")
        );
        assert_eq!(
            notice(Level::Paused, Cap::Automatic).as_deref(),
            Some("Previews paused to protect the outputs")
        );
        assert_eq!(
            notice(Level::Full, Cap::Fps30).as_deref(),
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
