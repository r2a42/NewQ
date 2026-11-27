//! Test Mode's 3D stage part (step B5 of
//! `.claude/notes/organise-and-test3d-plan.md`): does the 3D stage cost the
//! displays on this computer?
//!
//! The cue list's Short lap plays round and round on the test's displays
//! while the part goes through its legs, each the same length:
//!
//! 1. **Previews off**, the baseline: every preview is held paused
//!    (`TestMode::holds_previews`, which the app asks each pass).
//! 2. **The 3D view**: a view of the stage in the Test Mode window, turning
//!    slowly, drawn as any preview is (through the one governor).
//! 3. **Its light on**: the projectors' light in that view (left out when
//!    no projector throws a picture).
//! 4. **A display shows the stage**: the last ticked monitor's display shows
//!    it in place of the test's pictures; with one monitor (or none, or
//!    across a show network) a display with no window does, drawn for the
//!    previews only, at their pace.
//! 5. **A mapped projector** (`testmode::mapped`): the last ticked monitor's
//!    display shows the screens through a mapped projector, the lap on the
//!    canvas they show (left out across a show network or with no monitor).
//! 6. **A screen moving** (`testmode::mapped`, mapping round 3): the same,
//!    with one of the screens it maps swung to and fro, drawn live over the
//!    map.
//!
//! The governor starts each leg afresh (`PreviewGovernor::reset`), so what
//! one leg taught it doesn't decide the next. Each leg is measured from
//! `SETTLE` after it begins to its end: the display windows' new pictures and
//! seen misses (`render::shown_totals`), what the governor did (its `Tally`)
//! and what the stage drew (`stage_render::totals`). `judge` compares the 3D
//! legs with the baseline by the governor's own line (`previews::Settings::
//! line`): a 3D leg whose displays missed more than the baseline by that much
//! is a Problem (owner's decision 11); the previews giving way is worth
//! knowing, never a Problem. A leg's row is in `stage.csv`; `report_part`
//! says it all in words.

use super::analysis::{Finding, Severity};
use super::previews::{Settings, percent};
use crate::preview_pace::{Governor, Level, PreviewId, Shown, Tally, WINDOWS};
use crate::show::{OutputConfig, OutputId, Show};
use crate::stage_render::StageTotals;
use std::io::Write;

/// What the window's "What does this do?" says of the part.
pub const WHAT_IT_DOES: &str = "Plays the cue list's lap round and round, first with every preview off, then with a 3D view of the stage turning in this window, then with its projectors' light on, then with a display showing the stage, then with a mapped projector showing the screens on the last monitor, then with one of those screens moving, and says whether the 3D stage makes the displays miss pictures on this computer. With no 3D stage in your show it uses a sample venue with a projector for each of the test's displays.";

/// The Test Mode window's 3D view, as the governor knows it.
pub const VIEW: PreviewId = PreviewId::Other("test-mode-3d", 0);

/// A leg of a Short test (seconds).
pub const SHORT_LEG_SECS: f64 = 45.0;
/// A leg of a Medium or Long test: they go round for the part's time.
pub const LONG_LEG_SECS: f64 = 120.0;
/// Seconds at a leg's start that aren't measured: the governor starting
/// afresh, a display changing what it shows, the view's first pictures.
pub const SETTLE: f64 = 4.0;
/// The file the part writes, a row a leg.
pub const CSV: &str = "stage.csv";
pub const CSV_HEADER: &str = "leg,round,measured_s,new_pictures,late,late_pct,worst_window,worst_new_pictures,worst_late,worst_late_pct,previews_down,previews_up,previews_below_full_s,previews_lowest,views,stage_displays,feeds,throws,cpu_mean_ms,cpu_worst_band_ms,light_left_out,thread_stops,stage_display_fps,maps_baked,maps_refused";
/// A venue that took longer than this to load is said (ms).
pub const SLOW_VENUE_MS: u64 = 10_000;
/// The view turns this fast (degrees a second): slowly, but every picture
/// is new, so it is drawn whenever the governor lets it.
const TURN_DEG_PER_SEC: f32 = 6.0;

/// One leg.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leg {
    Baseline,
    View,
    Light,
    Display,
    /// A mapped projector's display (`testmode::mapped`).
    Mapped,
    /// The same with a screen it maps moving (`testmode::mapped`).
    Moving,
}

impl Leg {
    /// What the window and the report call it.
    pub fn name(self) -> &'static str {
        match self {
            Leg::Baseline => "previews off (the baseline)",
            Leg::View => "the 3D view, light off",
            Leg::Light => "the 3D view with its light on",
            Leg::Display => "a display showing the stage",
            Leg::Mapped => "a mapped projector's display",
            Leg::Moving => "a mapped projector's display with a screen moving",
        }
    }

    /// Its name in `stage.csv`.
    pub fn token(self) -> &'static str {
        match self {
            Leg::Baseline => "previews_off",
            Leg::View => "view",
            Leg::Light => "light",
            Leg::Display => "display",
            Leg::Mapped => "mapped",
            Leg::Moving => "moving",
        }
    }

    /// A mapped projector's display is shown (`Mapped` or `Moving`).
    pub fn mapped(self) -> bool {
        matches!(self, Leg::Mapped | Leg::Moving)
    }

    /// The view's light is on.
    pub fn light(self) -> bool {
        matches!(self, Leg::Light | Leg::Display)
    }

    /// "With the 3D view open", for the verdict's sentences.
    fn with(self) -> &'static str {
        match self {
            Leg::Baseline => "with previews off",
            Leg::View => "with the 3D view open",
            Leg::Light => "with the 3D view open and its light on",
            Leg::Display => "with a display showing the 3D stage",
            Leg::Mapped => "with a mapped projector showing the lap",
            Leg::Moving => "with a screen moving in front of a mapped projector",
        }
    }
}

/// The display that shows the stage in the last leg: its config while it
/// does, and as it is the rest of the time.
#[derive(Clone, Debug)]
pub struct StageDisplay {
    pub id: OutputId,
    /// On a ticked monitor (else it has no window: drawn for the previews only).
    pub windowed: bool,
    /// The monitor it is on (for its refresh), when windowed.
    pub monitor: Option<usize>,
    pub during: OutputConfig,
    pub before: OutputConfig,
}

/// What the part plays on, made with its show (`plan::build_stage`).
#[derive(Clone, Debug)]
pub struct Setup {
    /// The show had no screens: the Proscenium sample was used.
    pub sample: bool,
    pub screens: usize,
    /// Projectors that throw a picture (in the light legs).
    pub projectors: usize,
    pub stage_display: Option<StageDisplay>,
    /// The mapped projector of the last leg (`testmode::mapped`).
    pub mapped: Option<super::mapped::MappedLeg>,
    /// The legs, in order (Light left out with no projector to throw).
    pub legs: Vec<Leg>,
}

impl Setup {
    /// Put the stage display as `leg` wants it in `show` (the live test
    /// show). Whether anything changed.
    pub fn apply(&self, show: &mut Show, leg: Leg) -> bool {
        let mut changed = false;
        if let Some(m) = &self.mapped {
            // Put back before the stage display changes (they may be the
            // same display).
            changed |= m.apply(show, leg.mapped());
            if let Some(mv) = &m.moving {
                changed |= mv.apply(show, leg == Leg::Moving);
            }
        }
        let Some(d) = &self.stage_display else {
            return changed;
        };
        let want = if leg == Leg::Display {
            &d.during
        } else {
            &d.before
        };
        match show.outputs.iter_mut().find(|o| o.id == d.id) {
            Some(o) if !same(o, want) => {
                *o = want.clone();
                true
            }
            _ => changed,
        }
    }
}

/// The same display settings (`OutputConfig` has no `PartialEq`).
fn same(a: &OutputConfig, b: &OutputConfig) -> bool {
    serde_json::to_value(a).ok() == serde_json::to_value(b).ok()
}

/// A look at the counters.
#[derive(Clone, Debug, Default)]
struct Look {
    shown: [Shown; WINDOWS],
    tally: Tally,
    stage: StageTotals,
    at: f64,
}

impl Look {
    fn now(st: &crate::ui::UiState, at: f64) -> Look {
        Look {
            shown: crate::render::shown_totals(),
            tally: st.preview_governor.tally().clone(),
            stage: crate::stage_render::totals(),
            at,
        }
    }
}

/// What one leg measured.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LegResult {
    pub leg: Option<Leg>,
    /// Which time round (from 1).
    pub round: u32,
    /// Seconds measured.
    pub secs: f64,
    /// Each display window's new pictures and seen misses.
    pub shown: [Shown; WINDOWS],
    /// What the governor did: steps down and up, seconds below full rate,
    /// the lowest level it went to.
    pub down: u32,
    pub up: u32,
    pub below_full: f64,
    pub lowest: Level,
    /// What the 3D stage drew.
    pub stage: StageTotals,
    /// The stage display's monitor's refresh (a windowed one, its leg).
    pub refresh_hz: Option<f32>,
    /// Lens maps made and refused over the whole leg (its settling
    /// included: a mapped projector's map lands then).
    pub maps: (u64, u64),
    /// The moving leg: how many times the screen was put somewhere new.
    pub moves: u64,
}

impl LegResult {
    fn from(leg: Leg, round: u32, a: &Look, b: &Look, refresh_hz: Option<f32>) -> LegResult {
        let tally = b.tally.since(&a.tally);
        let lowest = Level::ALL
            .iter()
            .rev()
            .copied()
            .find(|l| tally.secs_at(*l) > 0.0)
            .unwrap_or(Level::Full)
            .max(
                tally
                    .steps
                    .iter()
                    .map(|s| s.to)
                    .max()
                    .unwrap_or(Level::Full),
            );
        LegResult {
            leg: Some(leg),
            round,
            secs: (b.at - a.at).max(0.0),
            shown: std::array::from_fn(|i| b.shown[i].since(a.shown[i])),
            down: tally.down,
            up: tally.up,
            below_full: (tally.secs() - tally.secs_at(Level::Full)).max(0.0),
            lowest,
            stage: b.stage.since(&a.stage),
            refresh_hz,
            maps: (0, 0),
            moves: 0,
        }
    }

    /// All the windows' pictures together.
    pub fn all(&self) -> Shown {
        self.shown.iter().fold(Shown::default(), |a, s| Shown {
            pictures: a.pictures + s.pictures,
            late: a.late + s.late,
        })
    }

    /// The share of the worst display window's new pictures that were
    /// late, when one showed enough to say (`Governor::LEAST_PICTURES`): the
    /// governor judges window by window too, so one weak monitor among
    /// several mirrors isn't lost in their sum.
    pub fn share(&self) -> Option<f64> {
        self.worst().map(|(_, _, share)| share)
    }

    /// The worst window's late pictures (none: no window showed enough).
    fn worst_late(&self) -> u32 {
        self.worst().map_or(0, |(_, s, _)| s.late)
    }

    /// Whether the 3D stage was drawn in this leg (in the display leg, for
    /// the display or the view).
    pub fn drew(&self) -> bool {
        match self.leg {
            Some(Leg::Baseline) | None => true,
            Some(Leg::Display) => self.stage.displays + self.stage.views > 0,
            Some(Leg::Mapped) => self.maps.0 > 0 && self.maps.1 == 0,
            Some(Leg::Moving) => self.maps.0 > 0 && self.maps.1 == 0 && self.moves > 0,
            Some(_) => self.stage.views > 0,
        }
    }

    /// The worst window (from 1), its counts and share.
    pub fn worst(&self) -> Option<(usize, Shown, f64)> {
        super::previews::Account::worst_of(&self.shown, Governor::LEAST_PICTURES)
    }

    /// The stage display's pictures a second in its leg.
    /// (Only one in a window or a stream: one drawn for the previews only
    /// is counted with the window's view, which can't be told apart.)
    pub fn display_fps(&self) -> Option<f64> {
        (self.leg == Some(Leg::Display) && self.secs > 0.0 && self.stage.displays > 0)
            .then(|| self.stage.displays as f64 / self.secs)
    }

    /// Two legs of a kind as one (the rounds of a Medium or Long test).
    fn add(&mut self, o: &LegResult) {
        self.leg = self.leg.or(o.leg);
        self.round = self.round.max(o.round);
        self.secs += o.secs;
        for (a, b) in self.shown.iter_mut().zip(&o.shown) {
            a.pictures += b.pictures;
            a.late += b.late;
        }
        self.down += o.down;
        self.up += o.up;
        self.below_full += o.below_full;
        self.lowest = self.lowest.max(o.lowest);
        let (s, t) = (&mut self.stage, &o.stage);
        s.jobs += t.jobs;
        s.feeds += t.feeds;
        s.throws += t.throws;
        s.views += t.views;
        s.displays += t.displays;
        s.cpu_us_views += t.cpu_us_views;
        s.cpu_us_displays += t.cpu_us_displays;
        s.cpu_us_worst = s.cpu_us_worst.max(t.cpu_us_worst);
        for (a, b) in s.frames_by_ms.iter_mut().zip(&t.frames_by_ms) {
            *a += b;
        }
        s.missing_pictures += t.missing_pictures;
        s.display_missing_pictures += t.display_missing_pictures;
        s.light_left_out += t.light_left_out;
        s.shadows_drawn += t.shadows_drawn;
        s.thread_starts += t.thread_starts;
        s.thread_stops += t.thread_stops;
        s.restarts += t.restarts;
        s.idle_releases += t.idle_releases;
        s.throws_left_out = s.throws_left_out.max(t.throws_left_out);
        s.venue_loads += t.venue_loads;
        s.venue_failures += t.venue_failures;
        s.venue_load_ms = s.venue_load_ms.max(t.venue_load_ms);
        self.refresh_hz = self.refresh_hz.or(o.refresh_hz);
        self.maps = (self.maps.0 + o.maps.0, self.maps.1 + o.maps.1);
        self.moves += o.moves;
    }
}

/// The legs of each kind, rounds added together, in the part's order.
pub fn by_leg(results: &[LegResult], legs: &[Leg]) -> Vec<LegResult> {
    legs.iter()
        .filter_map(|leg| {
            let mut of = results.iter().filter(|r| r.leg == Some(*leg));
            let mut sum = of.next()?.clone();
            for r in of {
                sum.add(r);
            }
            Some(sum)
        })
        .collect()
}

/// The part under way: which leg, since when, what was measured.
pub struct StageRun {
    pub setup: Setup,
    pub leg_secs: f64,
    /// How long it plays in all (seconds).
    pub total: f64,
    /// The leg playing (index into `setup.legs`), when it began (seconds
    /// into the test) and which time round it is.
    current: Option<(usize, f64, u32)>,
    /// The look `SETTLE` into the leg.
    look: Option<Look>,
    /// The stage's counts as the leg began (its lens maps).
    begun: Option<StageTotals>,
    /// The moving leg's screen put somewhere new, this leg.
    moves: u64,
    pub results: Vec<LegResult>,
    /// Legs played to their end.
    played: usize,
    pub finished: bool,
    /// The test show as it is now, for the window's view (the window gets no
    /// show of its own: the live one is this, the stage display as it is).
    pub show: Show,
    /// The previews were off on this computer when the part began.
    pub previews_off: bool,
    /// The venue's load as the part began (the venue is read as the test
    /// show is put in place, before the first look).
    venue: Option<(u64, u64, u64)>,
    csv: Option<std::io::BufWriter<std::fs::File>>,
}

/// What the window shows of the part.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Now {
    pub leg: Leg,
    /// Its number (from 1) of how many, and the time round.
    pub number: usize,
    pub of: usize,
    pub round: u32,
    /// Seconds into the leg and seconds left of it.
    pub into: f64,
    pub left: f64,
    /// Seconds into the part (the view turns with it).
    pub t: f64,
}

impl StageRun {
    /// The part, for `show` (the test show, as built), lasting one round of
    /// Short legs (`minutes` 0) or `minutes`, in `folder` (None: no file).
    pub fn new(
        setup: Setup,
        show: &Show,
        minutes: u32,
        previews_off: bool,
        folder: Option<&std::path::Path>,
    ) -> StageRun {
        let (leg_secs, total) = if minutes == 0 {
            (SHORT_LEG_SECS, SHORT_LEG_SECS * setup.legs.len() as f64)
        } else {
            (LONG_LEG_SECS, f64::from(minutes) * 60.0)
        };
        let csv = folder
            .and_then(|f| std::fs::File::create(f.join(CSV)).ok())
            .map(std::io::BufWriter::new)
            .and_then(|mut f| {
                writeln!(f, "{CSV_HEADER}").ok()?;
                f.flush().ok()?;
                Some(f)
            });
        StageRun {
            setup,
            leg_secs,
            total,
            current: None,
            look: None,
            begun: None,
            moves: 0,
            results: Vec::new(),
            played: 0,
            finished: false,
            show: show.clone(),
            previews_off,
            venue: None,
            csv,
        }
    }

    /// Shorter legs (the tests of the test).
    #[cfg(test)]
    pub fn shorten(&mut self, leg_secs: f64) {
        self.leg_secs = leg_secs;
        self.total = leg_secs * self.setup.legs.len() as f64;
    }

    /// The leg playing.
    pub fn leg(&self) -> Option<Leg> {
        let (i, _, _) = self.current?;
        (!self.finished).then(|| self.setup.legs[i])
    }

    /// Whether every preview is to be held off now.
    pub fn holds_previews(&self) -> bool {
        self.leg() == Some(Leg::Baseline)
    }

    /// Where it is, for the window, `t` seconds into the test.
    pub fn now(&self, t: f64) -> Option<Now> {
        let (i, began, round) = self.current?;
        if self.finished {
            return None;
        }
        Some(Now {
            leg: self.setup.legs[i],
            number: i + 1,
            of: self.setup.legs.len(),
            round,
            into: t - began,
            left: (self.leg_secs - (t - began)).max(0.0),
            t,
        })
    }

    /// The window's view now: the stage turning, its light as the leg has it.
    pub fn view_state(&self, t: f64) -> crate::ui::stage_view::StageViewState {
        // From the first seat (a sample's audience), swinging a little each
        // way; with no viewpoint, all of the stage from in front.
        let mut v = match self.show.stage3d.viewpoints.first() {
            Some(p) => crate::ui::stage_view::StageViewState::from_viewpoint(p),
            None => {
                let mut v = crate::ui::stage_view::StageViewState::default();
                v.orbit.pitch = 12f32.to_radians();
                v
            }
        };
        v.grid = false;
        v.light = self.leg().is_some_and(Leg::light) && self.setup.projectors > 0;
        v.orbit.yaw += (t as f32 * TURN_DEG_PER_SEC).to_radians().sin() * 0.35;
        v
    }

    /// One look, `t` seconds into the test: a leg begun or ended as its
    /// time comes, `show` (the live test show) changed for it. The lines for
    /// `events.log`.
    pub fn tick(&mut self, show: &mut Show, st: &mut crate::ui::UiState, t: f64) -> Vec<String> {
        let mut lines = Vec::new();
        if self.finished {
            return lines;
        }
        if self.venue.is_none() {
            let s = crate::stage_render::totals();
            self.venue = Some((s.venue_load_ms, s.venue_parts, s.venue_triangles));
        }
        let Some((i, began, round)) = self.current else {
            self.begin(0, 1, t, show, st, &mut lines);
            return lines;
        };
        if self.look.is_none() && t - began >= SETTLE.min(self.leg_secs / 2.0) {
            self.look = Some(Look::now(st, t));
        }
        if t - began < self.leg_secs {
            return lines;
        }
        // The leg is over.
        let leg = self.setup.legs[i];
        let end = Look::now(st, t);
        if let Some(start) = self.look.take() {
            let refresh = match (&self.setup.stage_display, leg) {
                (Some(d), Leg::Display) if d.windowed => d.monitor.and_then(|m| {
                    st.drawing
                        .iter()
                        .find(|r| r.index == m)
                        .and_then(|r| r.refresh_hz)
                }),
                _ => None,
            };
            let mut r = LegResult::from(leg, round, &start, &end, refresh);
            if let Some(b) = &self.begun {
                let all = end.stage.since(b);
                r.maps = (all.maps_baked, all.maps_refused);
            }
            r.moves = self.moves;
            lines.push(format!("3D stage: {} over: {}", leg.name(), leg_words(&r)));
            self.row(&r);
            self.results.push(r);
        }
        let (next, round) = if i + 1 < self.setup.legs.len() {
            (i + 1, round)
        } else {
            (0, round + 1)
        };
        // Medium and Long go round while most of a leg still fits.
        self.played += 1;
        let left = self.total - self.played as f64 * self.leg_secs;
        if left < self.leg_secs * 0.5 {
            self.finish(show, st);
            lines.push("3D stage: all its legs are done".into());
            return lines;
        }
        self.begin(next, round, t, show, st, &mut lines);
        lines
    }

    fn begin(
        &mut self,
        i: usize,
        round: u32,
        t: f64,
        show: &mut Show,
        st: &mut crate::ui::UiState,
        lines: &mut Vec<String>,
    ) {
        let leg = self.setup.legs[i];
        self.current = Some((i, t, round));
        self.look = None;
        self.moves = 0;
        self.begun = Some(crate::stage_render::totals());
        // The governor starts afresh; the hold for the baseline is set from
        // `holds_previews` (here, and by the app each pass).
        st.preview_governor.reset();
        st.preview_governor.set_held(leg == Leg::Baseline);
        if self.setup.apply(show, leg) {
            self.show = show.clone();
        }
        lines.push(format!(
            "3D stage: leg {} of {}{}: {}",
            i + 1,
            self.setup.legs.len(),
            if round > 1 {
                format!(" (round {round})")
            } else {
                String::new()
            },
            leg.name()
        ));
    }

    /// In the moving leg, `t` seconds into the test: the screen and where it
    /// is to be put now (the session puts it, `Engine::put_screen`, each
    /// look). None in any other leg.
    pub fn screen_to_put(
        &mut self,
        t: f64,
    ) -> Option<(crate::stage3d::SurfaceId, crate::stage3d::Placement)> {
        let (i, began, _) = self.current?;
        if self.finished || self.setup.legs[i] != Leg::Moving {
            return None;
        }
        let m = self.setup.mapped.as_ref()?.moving.as_ref()?;
        self.moves += 1;
        Some((m.screen, m.place_at(t - began)))
    }

    /// The part is over: the stage display put back, the hold let go.
    pub fn finish(&mut self, show: &mut Show, st: &mut crate::ui::UiState) {
        if self.finished {
            return;
        }
        self.finished = true;
        if self.setup.apply(show, Leg::Baseline) {
            self.show = show.clone();
        }
        st.preview_governor.set_held(false);
        if let Some(f) = &mut self.csv {
            let _ = f.flush();
        }
    }

    fn row(&mut self, r: &LegResult) {
        let Some(f) = &mut self.csv else { return };
        let all = r.all();
        let (wn, ws, wshare) = r.worst().unwrap_or((0, Shown::default(), 0.0));
        let pct = |s: Option<f64>| s.map_or(String::new(), |s| format!("{:.2}", s * 100.0));
        let _ = writeln!(
            f,
            "{},{},{:.1},{},{},{},{},{},{},{},{},{},{:.1},{},{},{},{},{},{},{},{},{},{},{},{}",
            r.leg.map_or("", Leg::token),
            r.round,
            r.secs,
            all.pictures,
            all.late,
            pct(r.share()),
            if wn > 0 {
                wn.to_string()
            } else {
                String::new()
            },
            ws.pictures,
            ws.late,
            if wn > 0 {
                pct(Some(wshare))
            } else {
                String::new()
            },
            r.down,
            r.up,
            r.below_full,
            super::previews::level_token(r.lowest),
            r.stage.views,
            r.stage.displays,
            r.stage.feeds,
            r.stage.throws,
            r.stage
                .mean_cpu_us()
                .map_or(String::new(), |u| format!("{:.2}", u as f64 / 1000.0)),
            r.stage
                .worst_band_ms()
                .map_or(String::new(), |m| if m == u64::MAX {
                    "over".to_string()
                } else {
                    m.to_string()
                }),
            r.stage.light_left_out,
            r.stage.thread_stops,
            r.display_fps().map_or(String::new(), |f| format!("{f:.1}")),
            r.maps.0,
            r.maps.1,
        );
        let _ = f.flush();
    }

    /// The venue's load: ms, parts, triangles (None: none was read).
    pub fn venue(&self) -> Option<(u64, u64, u64)> {
        self.venue.filter(|v| v.0 > 0 || v.1 > 0)
    }
}

/// A leg in a few words, for `events.log`: "0.1% of 2,700 new pictures late,
/// the 3D drawn 1,234 times".
pub fn leg_words(r: &LegResult) -> String {
    let a = r.all();
    let mut s = match r.share() {
        Some(share) => format!(
            "{} of {} new pictures on the displays late",
            percent(share),
            a.pictures
        ),
        None => format!(
            "{} new pictures on the displays (too few to judge)",
            a.pictures
        ),
    };
    s.push_str(&format!(
        ", the 3D stage drawn {} time{} for previews and {} for displays",
        r.stage.views,
        if r.stage.views == 1 { "" } else { "s" },
        r.stage.displays
    ));
    if r.down > 0 {
        s.push_str(&format!(
            ", the previews gave way {} (as far as {})",
            times(r.down),
            crate::stats::preview_level_words(r.lowest)
        ));
    }
    s
}

fn times(n: u32) -> String {
    match n {
        1 => "once".into(),
        2 => "twice".into(),
        n => format!("{n} times"),
    }
}

fn problem(text: String) -> Finding {
    Finding {
        severity: Severity::Problem,
        text,
    }
}

fn note(text: String) -> Finding {
    Finding {
        severity: Severity::Note,
        text,
    }
}

/// What the judge needs besides the legs.
#[derive(Clone, Copy, Debug)]
pub struct Context<'a> {
    pub setup: &'a Setup,
    pub settings: Settings,
    /// The previews were off on this computer (the 3D view is never drawn).
    pub previews_off: bool,
    /// The venue's load (ms, parts, triangles), if one was read.
    pub venue: Option<(u64, u64, u64)>,
}

/// Where the previews' settings are, in the words of a finding.
const SETTINGS_WORDS: &str = "the Previews settings for this computer";

/// The findings of the part: the 3D legs against the baseline (a Problem
/// only when the displays missed more by the governor's own line), and what
/// is worth knowing.
pub fn judge(results: &[LegResult], cx: &Context) -> Vec<Finding> {
    let mut out = Vec::new();
    let legs = by_leg(results, &cx.setup.legs);
    let (line, allowed) = cx.settings.line();
    let base = legs.iter().find(|r| r.leg == Some(Leg::Baseline));
    let threed: Vec<&LegResult> = legs
        .iter()
        .filter(|r| r.leg != Some(Leg::Baseline))
        .collect();
    if base.is_none() || threed.is_empty() {
        out.push(note(
            "The 3D stage part ended before it had a leg to compare with the baseline, so it says nothing about what 3D costs here.".into(),
        ));
        return out;
    }
    let base = base.expect("checked");
    let base_share = base.share();
    if base_share.is_none() {
        out.push(note(format!(
            "The 3D stage part couldn't compare the displays with and without 3D: no display window showed enough pictures (at least {} a leg). Tick a monitor to show the test on, and run it again.",
            Governor::LEAST_PICTURES
        )));
    }
    if let Some(s) = base_share.filter(|s| *s > line && base.worst_late() >= Governor::MIN_SEEN) {
        out.push(note(format!(
            "With every preview off the displays already missed {} of their new pictures: that much is this computer's, not the 3D stage's.",
            percent(s)
        )));
    }
    for r in &threed {
        let leg = r.leg.expect("a leg");
        if !r.drew() && leg.mapped() {
            // Said by `mapped::findings`.
            continue;
        }
        if !r.drew() {
            out.push(note(if cx.previews_off && leg != Leg::Display {
                format!(
                    "The previews are off on this computer, so the 3D view wasn't drawn in the leg with {}: turn them on in {SETTINGS_WORDS} to measure it.",
                    leg.name()
                )
            } else {
                format!(
                    "The 3D stage was not drawn in the leg with {}, so that leg says nothing.",
                    leg.name()
                )
            }));
            continue;
        }
        let (Some(b), Some(s)) = (base_share, r.share()) else {
            continue;
        };
        if s - b >= line && r.worst_late() >= Governor::MIN_SEEN && leg == Leg::Moving {
            out.push(problem(super::mapped::moving_costs_words(
                &percent(s),
                &percent(b),
            )));
        } else if s - b >= line && r.worst_late() >= Governor::MIN_SEEN && leg == Leg::Mapped {
            out.push(problem(super::mapped::costs_words(
                &percent(s),
                &percent(b),
            )));
        } else if s - b >= line && r.worst_late() >= Governor::MIN_SEEN {
            out.push(problem(format!(
                "{} the displays missed {} of their pictures; with previews off, {}. On this computer the 3D stage costs the show{}. Set the previews to Strict, or turn them off, for the performance ({SETTINGS_WORDS}).",
                capital(leg.with()),
                percent(s),
                percent(b),
                if allowed {
                    String::new()
                } else {
                    " (and the previews here are set never to give way, so they couldn't make room)"
                        .to_string()
                }
            )));
        }
    }
    // Giving way: worth knowing.
    let down: u32 = threed.iter().map(|r| r.down).sum();
    let below: f64 = threed.iter().map(|r| r.below_full).sum();
    let lowest = threed.iter().map(|r| r.lowest).max().unwrap_or(Level::Full);
    // Smooth only where it was measured.
    let measured = base_share.is_some() && threed.iter().all(|r| r.share().is_some());
    let smooth = measured && !out.iter().any(|f| f.severity == Severity::Problem);
    if allowed && (down > 0 || below >= 1.0) {
        out.push(note(format!(
            "With the 3D stage the previews gave way {}, as far as {}, for {:.0} s{}",
            times(down.max(1)),
            crate::stats::preview_level_words(lowest),
            below,
            if smooth {
                ", and the displays stayed smooth. This computer can't draw the 3D view at full rate while a show plays; the show itself is not affected."
            } else if !measured {
                " (the displays themselves couldn't be measured)."
            } else {
                "."
            }
        )));
    }
    let light = legs.iter().filter(|r| r.leg.is_some_and(Leg::light));
    if light.clone().any(|r| r.stage.light_left_out > 0) {
        out.push(note(
            "This graphics card would not draw the projectors' light; the 3D view shows the pictures without it.".into(),
        ));
    }
    if let Some(left) = light
        .map(|r| r.stage.throws_left_out)
        .max()
        .filter(|n| *n > 0)
    {
        out.push(note(format!(
            "{left} projector{} past the {} the light can show threw no picture.",
            if left == 1 { "" } else { "s" },
            crate::stage3d::light::MAX_THROWS
        )));
    }
    if let Some(r) = legs.iter().find(|r| r.leg == Some(Leg::Display))
        && let Some(fps) = r.display_fps()
        && r.stage.displays + r.stage.views > 0
    {
        let windowed = cx.setup.stage_display.as_ref().is_some_and(|d| d.windowed);
        match r.refresh_hz.filter(|_| windowed) {
            Some(hz) if fps < f64::from(hz) / 2.0 => out.push(note(format!(
                "The display that shows the stage drew {fps:.0} pictures a second on a {hz:.0} Hz monitor: a 3D stage on a display here would look jerky."
            ))),
            _ => {}
        }
    }
    out.extend(super::mapped::findings(&legs));
    let stops: u64 = legs.iter().map(|r| r.stage.thread_stops).sum();
    if stops > 0 {
        out.push(problem(format!(
            "The 3D drawing stopped and had to be started again {}. newq.log says why.",
            times(stops as u32)
        )));
    }
    if let Some((ms, _, _)) = cx.venue.filter(|v| v.0 >= SLOW_VENUE_MS) {
        out.push(note(format!(
            "The venue took {} seconds to load.",
            (ms as f64 / 1000.0).round()
        )));
    }
    out
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next().map_or(String::new(), |f| {
        f.to_uppercase().collect::<String>() + c.as_str()
    })
}

/// The one sentence of the verdict, for the report and the window.
pub fn in_short(results: &[LegResult], cx: &Context, findings: &[Finding]) -> String {
    let legs = by_leg(results, &cx.setup.legs);
    if findings.iter().any(|f| {
        f.severity == Severity::Problem
            && (f.text.contains("costs the show") || f.text.starts_with("With a mapped"))
    }) {
        return "The 3D stage costs the displays on this computer: see above.".into();
    }
    let base = legs
        .iter()
        .find(|r| r.leg == Some(Leg::Baseline))
        .and_then(LegResult::share);
    // Only legs in which the stage was drawn say anything of it.
    let worst = legs
        .iter()
        .filter(|r| r.leg != Some(Leg::Baseline) && r.drew())
        .filter_map(|r| r.share().map(|s| (s, r.leg)))
        .max_by(|a, b| a.0.total_cmp(&b.0));
    let gave_way = legs
        .iter()
        .filter(|r| r.leg != Some(Leg::Baseline))
        .any(|r| r.down > 0 || r.below_full >= 1.0);
    match (base, worst) {
        (Some(b), Some((w, leg))) => format!(
            "{} the displays missed no more pictures than without it ({} against {}). {}",
            capital(leg.map_or("with 3D", Leg::with)),
            percent(w),
            percent(b),
            if gave_way {
                "The previews gave way to keep it so."
            } else {
                "The previews drew at full rate throughout."
            }
        ),
        (Some(_), None) => "Not measured: the 3D stage was never drawn in its legs.".into(),
        _ => "Not measured: no display window showed enough pictures to compare.".into(),
    }
}

/// The report's "3D stage" part: its lines, without the heading.
pub fn report_part(results: &[LegResult], cx: &Context, findings: &[Finding]) -> Vec<String> {
    let s = cx.setup;
    let mut out = Vec::new();
    out.push(if s.sample {
        format!(
            "Your show has no 3D stage, so a sample was used: the Proscenium sample venue with its {} usual screens showing the test's displays, and {} projector{} aimed at the main screen.",
            s.screens,
            s.projectors,
            if s.projectors == 1 { "" } else { "s" }
        )
    } else {
        format!(
            "Your 3D stage was used: {} screen{} showing the test's displays, {} projector{} throwing their pictures.",
            s.screens,
            if s.screens == 1 { "" } else { "s" },
            s.projectors,
            if s.projectors == 1 { "" } else { "s" }
        )
    });
    if !s.legs.contains(&Leg::Light) {
        out.push("No projector throws a picture, so there was no leg with the light on.".into());
    }
    match &s.stage_display {
        Some(d) if d.windowed => out.push(format!(
            "In the last leg {} showed the stage in place of the test's pictures.",
            d.before.name
        )),
        Some(_) => out.push(
            "In the last leg a display with no window showed the stage (one monitor was ticked, or none): it was drawn for the previews only, at their pace.".into(),
        ),
        None => {}
    }
    if let Some(m) = &s.mapped {
        out.push(format!(
            "The lap was also shown on a canvas the screens show; in the leg with a mapped projector, {} showed the screens as {} sees them, through its lens map.",
            m.display_name,
            m.during.name
        ));
    }
    out.push(format!(
        "The 3D view in the Test Mode window turned slowly in every leg but the first; each leg the previews' governor started afresh, and its first {SETTLE:.0} s were not measured."
    ));
    if cx.previews_off {
        out.push("The previews were off on this computer, so the 3D view was never drawn.".into());
    }
    let (line, allowed) = cx.settings.line();
    out.push(format!(
        "The line: a 3D leg is a problem when its displays miss {} more of their new pictures than with previews off ({}).",
        percent(line),
        if allowed {
            "the line the previews give way at here"
        } else {
            "the previews here never give way, so the usual line"
        }
    ));
    out.push("Leg by leg (each is a row of stage.csv):".into());
    for r in by_leg(results, &s.legs) {
        let leg = r.leg.expect("a leg");
        let mut line = format!(
            "  {}, {}: {}",
            capital(leg.name()).replace(" (the baseline)", ", the baseline"),
            if r.round > 1 {
                format!("{} rounds, {:.0} s measured", r.round, r.secs)
            } else {
                format!("{:.0} s measured", r.secs)
            },
            leg_words(&r)
        );
        if let Some(ms) = r.stage.mean_cpu_us() {
            line.push_str(&format!(
                "; {:.1} ms of processor time a 3D picture on average",
                ms as f64 / 1000.0
            ));
        }
        if leg == Leg::Moving {
            line.push_str(&format!("; the screen put somewhere new {} times", r.moves));
        }
        if leg.mapped() {
            line.push_str(&format!(
                "; {} lens map{} made{}",
                r.maps.0,
                if r.maps.0 == 1 { "" } else { "s" },
                if r.maps.1 > 0 {
                    format!(", {} refused", r.maps.1)
                } else {
                    String::new()
                }
            ));
        }
        if let Some(fps) = r.display_fps() {
            line.push_str(&format!(
                "; the display showing the stage drew {fps:.0} a second{}",
                match r.refresh_hz {
                    Some(hz) => format!(" on a {hz:.0} Hz monitor"),
                    None => String::new(),
                }
            ));
        }
        line.push('.');
        out.push(line);
    }
    if let Some((ms, parts, tris)) = cx.venue {
        out.push(format!(
            "The venue was read in {}: {parts} parts, {tris} triangles.",
            if ms < 1000 {
                format!("{ms} ms")
            } else {
                format!("{:.1} s", ms as f64 / 1000.0)
            }
        ));
    }
    out.push("The graphics card's own time can't be measured here; the processor time is NewQ's work for each picture.".into());
    out.push(format!("In short: {}", in_short(results, cx, findings)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> Setup {
        Setup {
            sample: true,
            screens: 3,
            projectors: 2,
            stage_display: None,
            mapped: None,
            legs: vec![Leg::Baseline, Leg::View, Leg::Light, Leg::Display],
        }
    }

    /// A leg whose one window showed `pictures`, `late` of them late, with
    /// the 3D stage drawn `views` times and the previews stepping `down`.
    fn leg(leg: Leg, pictures: u32, late: u32, views: u64, down: u32) -> LegResult {
        let mut shown = [Shown::default(); WINDOWS];
        shown[0] = Shown { pictures, late };
        LegResult {
            leg: Some(leg),
            round: 1,
            secs: 41.0,
            shown,
            down,
            up: 0,
            below_full: if down > 0 { 20.0 } else { 0.0 },
            lowest: if down > 0 { Level::Fps15 } else { Level::Full },
            stage: StageTotals {
                views,
                displays: if leg == Leg::Display { views } else { 0 },
                ..Default::default()
            },
            refresh_hz: None,
            maps: (0, 0),
            moves: 0,
        }
    }

    fn cx(s: &Setup, strictness: crate::preview_pace::Strictness) -> Context<'_> {
        Context {
            setup: s,
            settings: Settings {
                strictness,
                ..Default::default()
            },
            previews_off: false,
            venue: None,
        }
    }

    fn problems(f: &[Finding]) -> Vec<&str> {
        f.iter()
            .filter(|f| f.severity == Severity::Problem)
            .map(|f| f.text.as_str())
            .collect()
    }

    #[test]
    fn all_good_when_3d_costs_the_displays_nothing() {
        use crate::preview_pace::Strictness;
        let s = setup();
        let legs = [
            leg(Leg::Baseline, 2460, 2, 0, 0),
            leg(Leg::View, 2460, 2, 900, 0),
            leg(Leg::Light, 2460, 3, 900, 0),
            leg(Leg::Display, 2460, 2, 2400, 0),
        ];
        let c = cx(&s, Strictness::Relaxed);
        let f = judge(&legs, &c);
        assert!(f.is_empty(), "{f:?}");
        assert_eq!(
            in_short(&legs, &c, &f),
            "With the 3D view open and its light on the displays missed no more pictures than without it (0.1% against 0.1%). The previews drew at full rate throughout."
        );
    }

    #[test]
    fn giving_way_is_worth_knowing_and_never_a_problem() {
        use crate::preview_pace::Strictness;
        let s = setup();
        let legs = [
            leg(Leg::Baseline, 2460, 2, 0, 0),
            leg(Leg::View, 2460, 4, 300, 3),
            leg(Leg::Light, 2460, 5, 200, 3),
            leg(Leg::Display, 2460, 2, 2400, 0),
        ];
        let c = cx(&s, Strictness::Relaxed);
        let f = judge(&legs, &c);
        assert!(problems(&f).is_empty(), "{f:?}");
        assert_eq!(
            f[0].text,
            "With the 3D stage the previews gave way 6 times, as far as 15 a second, for 40 s, and the displays stayed smooth. This computer can't draw the 3D view at full rate while a show plays; the show itself is not affected."
        );
        assert!(in_short(&legs, &c, &f).ends_with("The previews gave way to keep it so."));
    }

    #[test]
    fn missing_more_with_3d_by_the_governors_line_is_a_problem() {
        use crate::preview_pace::Strictness;
        let s = setup();
        // 4% against 0.3%: over Strict's 1% and under Relaxed's 5%.
        let legs = [
            leg(Leg::Baseline, 3000, 9, 0, 0),
            leg(Leg::View, 3000, 120, 900, 0),
        ];
        let strict = cx(&s, Strictness::Strict);
        let f = judge(&legs, &strict);
        assert_eq!(
            problems(&f),
            [
                "With the 3D view open the displays missed 4.0% of their pictures; with previews off, 0.3%. On this computer the 3D stage costs the show. Set the previews to Strict, or turn them off, for the performance (the Previews settings for this computer)."
            ]
        );
        assert_eq!(
            in_short(&legs, &strict, &f),
            "The 3D stage costs the displays on this computer: see above."
        );
        assert!(problems(&judge(&legs, &cx(&s, Strictness::Relaxed))).is_empty());
        // Set never to give way: judged by 5%, and said.
        let legs = [
            leg(Leg::Baseline, 3000, 9, 0, 0),
            leg(Leg::View, 3000, 240, 900, 0),
        ];
        let p = judge(&legs, &cx(&s, Strictness::Off));
        assert!(problems(&p)[0].contains("set never to give way"), "{p:?}");
        // A baseline that already misses: said, not blamed on 3D.
        let legs = [
            leg(Leg::Baseline, 3000, 300, 0, 0),
            leg(Leg::View, 3000, 330, 900, 0),
        ];
        let f = judge(&legs, &cx(&s, Strictness::Relaxed));
        assert!(problems(&f).is_empty(), "{f:?}");
        assert!(
            f[0].text
                .starts_with("With every preview off the displays already missed 10.0%")
        );
    }

    #[test]
    fn what_could_not_be_measured_is_said_not_passed() {
        use crate::preview_pace::Strictness;
        let s = setup();
        // No display window: no pictures to compare.
        let legs = [leg(Leg::Baseline, 0, 0, 0, 0), leg(Leg::View, 0, 0, 900, 0)];
        let c = cx(&s, Strictness::Relaxed);
        let f = judge(&legs, &c);
        assert!(f[0].text.contains("couldn't compare"), "{f:?}");
        assert!(in_short(&legs, &c, &f).starts_with("Not measured"));
        // A leg in which the stage was never drawn.
        let legs = [
            leg(Leg::Baseline, 2000, 1, 0, 0),
            leg(Leg::View, 2000, 1, 0, 0),
        ];
        let f = judge(&legs, &c);
        assert!(f[0].text.contains("was not drawn in the leg"), "{f:?}");
        let off = Context {
            previews_off: true,
            ..c
        };
        let f = judge(&legs, &off);
        assert!(f[0].text.contains("previews are off on this computer"));
        // ...and its short sentence takes no comfort from it.
        assert_eq!(
            in_short(&legs, &off, &f),
            "Not measured: the 3D stage was never drawn in its legs."
        );
        // Nothing but the baseline.
        assert!(judge(&legs[..1], &c)[0].text.contains("ended before"));
    }

    #[test]
    fn the_light_refused_a_slow_stage_display_restarts_and_a_slow_venue_are_said() {
        use crate::preview_pace::Strictness;
        let mut s = setup();
        let mut d = Show::default();
        let o = d.outputs[0].clone();
        s.stage_display = Some(StageDisplay {
            id: o.id,
            windowed: true,
            monitor: Some(1),
            during: o.clone(),
            before: o,
        });
        let mut light = leg(Leg::Light, 2400, 2, 900, 0);
        light.stage.light_left_out = 900;
        let mut display = leg(Leg::Display, 2400, 2, 574, 0);
        display.refresh_hz = Some(60.0);
        display.stage.thread_stops = 3;
        let legs = [leg(Leg::Baseline, 2400, 2, 0, 0), light, display];
        let c = Context {
            venue: Some((41_000, 30, 90_000)),
            ..cx(&s, Strictness::Relaxed)
        };
        let f = judge(&legs, &c);
        let texts: Vec<&str> = f.iter().map(|f| f.text.as_str()).collect();
        assert!(texts.contains(&"This graphics card would not draw the projectors' light; the 3D view shows the pictures without it."), "{texts:?}");
        assert!(texts.contains(&"The display that shows the stage drew 14 pictures a second on a 60 Hz monitor: a 3D stage on a display here would look jerky."), "{texts:?}");
        assert_eq!(
            problems(&f),
            ["The 3D drawing stopped and had to be started again 3 times. newq.log says why."]
        );
        assert!(texts.contains(&"The venue took 41 seconds to load."));
        let part = report_part(&legs, &c, &f).join("\n");
        assert!(part.contains("Proscenium sample"), "{part}");
        assert!(
            part.contains("display showing the stage drew 14 a second on a 60 Hz monitor"),
            "{part}"
        );
        let _ = &mut d;
    }

    #[test]
    fn the_worst_window_is_compared_not_the_sum_of_the_windows() {
        use crate::preview_pace::Strictness;
        let s = setup();
        let mut base = leg(Leg::Baseline, 2400, 2, 0, 0);
        let mut view = leg(Leg::View, 2400, 2, 900, 0);
        // Three windows: two smooth, one missing 8% with the view open.
        for (i, late) in [(1, 2), (2, 2)] {
            base.shown[i] = Shown {
                pictures: 2400,
                late,
            };
            view.shown[i] = Shown {
                pictures: 2400,
                late,
            };
        }
        view.shown[2].late = 192;
        let f = judge(&[base, view], &cx(&s, Strictness::Relaxed));
        assert_eq!(problems(&f).len(), 1, "{f:?}");
        assert!(problems(&f)[0].contains("missed 8.0%"), "{f:?}");
    }

    #[test]
    fn rounds_of_a_leg_are_added_together() {
        let a = leg(Leg::View, 1000, 10, 100, 1);
        let mut b = a.clone();
        b.round = 2;
        let s = by_leg(&[a, b], &[Leg::Baseline, Leg::View]);
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].all().pictures, s[0].all().late), (2000, 20));
        assert_eq!((s[0].round, s[0].down, s[0].stage.views), (2, 2, 200));
        assert!((s[0].secs - 82.0).abs() < 1e-9);
    }

    #[test]
    fn the_legs_go_by_turning_the_hold_and_the_stage_display_on_and_off() {
        let mut show = Show::default();
        let mut st = crate::ui::UiState::default();
        let id = show.outputs[0].id;
        let before = show.outputs[0].clone();
        let mut during = before.clone();
        during.source = Some(crate::stage3d::StageSource::default());
        let mut s = setup();
        s.stage_display = Some(StageDisplay {
            id,
            windowed: false,
            monitor: None,
            during,
            before: before.clone(),
        });
        let dir = std::env::temp_dir().join(format!("newq_stage_legs_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut run = StageRun::new(s, &show, 0, false, Some(&dir));
        assert_eq!(run.total, 4.0 * SHORT_LEG_SECS);
        let mut legs = Vec::new();
        let mut t = 0.0;
        while !run.finished && t < 1000.0 {
            run.tick(&mut show, &mut st, t);
            if let Some(l) = run.leg() {
                if legs.last() != Some(&l) {
                    legs.push(l);
                }
                assert_eq!(st.preview_governor.held(), l == Leg::Baseline);
                assert_eq!(run.holds_previews(), l == Leg::Baseline);
                assert_eq!(show.shows_stage(id), l == Leg::Display, "{l:?}");
                assert_eq!(run.show.shows_stage(id), l == Leg::Display);
            }
            t += 0.5;
        }
        assert_eq!(legs, [Leg::Baseline, Leg::View, Leg::Light, Leg::Display]);
        assert!((t - 4.0 * SHORT_LEG_SECS).abs() < 1.0, "{t}");
        assert_eq!(run.results.len(), 4);
        assert!(
            run.results
                .iter()
                .all(|r| (r.secs - (SHORT_LEG_SECS - SETTLE)).abs() < 1.0)
        );
        // Put back, and let go.
        assert!(same(&show.outputs[0], &before));
        assert!(!st.preview_governor.held() && !run.holds_previews());
        let csv = std::fs::read_to_string(dir.join(CSV)).unwrap();
        assert_eq!(csv.lines().count(), 5, "{csv}");
        assert!(csv.lines().nth(1).unwrap().starts_with("previews_off,1,"));
        let _ = std::fs::remove_dir_all(&dir);

        // Medium: round and round while a leg fits.
        let mut run = StageRun::new(setup(), &show, 20, false, None);
        let mut t = 0.0;
        while !run.finished && t < 3000.0 {
            run.tick(&mut show, &mut st, t);
            t += 1.0;
        }
        assert_eq!(run.results.len(), 10, "20 minutes of 2-minute legs");
        assert_eq!(run.results.last().unwrap().round, 3);
    }

    #[test]
    fn a_mapped_projectors_leg_is_judged_by_the_same_line() {
        use crate::preview_pace::Strictness;
        let mut s = setup();
        s.legs.push(Leg::Mapped);
        let mapped = |late: u32, maps: (u64, u64)| LegResult {
            maps,
            ..leg(Leg::Mapped, 3000, late, 0, 0)
        };
        let base = leg(Leg::Baseline, 3000, 9, 0, 0);
        // Within the line: nothing to say, and the short sentence counts it.
        let legs = [base.clone(), mapped(12, (1, 0))];
        let c = cx(&s, Strictness::Strict);
        let f = judge(&legs, &c);
        assert!(f.is_empty(), "{f:?}");
        assert!(
            in_short(&legs, &c, &f).starts_with("With a mapped projector showing the lap"),
            "{}",
            in_short(&legs, &c, &f)
        );
        // 4% against 0.3%: over Strict's 1%, a Problem in its own words.
        let legs = [base.clone(), mapped(120, (1, 0))];
        let f = judge(&legs, &c);
        assert_eq!(
            problems(&f),
            [
                "With a mapped projector showing the lap the displays missed 4.0% of their pictures; with previews off, 0.3%. On this computer a mapped projector costs the show: its picture is drawn for every frame and never gives way. Try a smaller canvas, or fewer canvases a projector."
            ]
        );
        assert_eq!(
            in_short(&legs, &c, &f),
            "The 3D stage costs the displays on this computer: see above."
        );
        // Under Relaxed's 5%: none.
        assert!(problems(&judge(&legs, &cx(&s, Strictness::Relaxed))).is_empty());
        // A card that refused the map: a Problem, and the leg not compared.
        let legs = [base.clone(), mapped(120, (0, 1))];
        let f = judge(&legs, &c);
        assert_eq!(problems(&f).len(), 1);
        assert!(problems(&f)[0].contains("stays black"), "{f:?}");
        // No map made: said, never passed.
        let legs = [base, mapped(2, (0, 0))];
        let f = judge(&legs, &c);
        assert!(f[0].text.starts_with("No lens map was made"), "{f:?}");
        assert!(in_short(&legs, &c, &f).starts_with("Not measured"));
        let part = report_part(&legs, &c, &f).join(" / ");
        assert!(
            part.contains("A mapped projector's display, 41 s measured"),
            "{part}"
        );
        assert!(part.contains("0 lens maps made"), "{part}");
    }

    #[test]
    fn the_moving_screens_leg_is_judged_by_the_same_line() {
        use crate::preview_pace::Strictness;
        let mut s = setup();
        s.legs.extend([Leg::Mapped, Leg::Moving]);
        let moving = |late: u32, maps: (u64, u64), moves: u64| LegResult {
            maps,
            moves,
            ..leg(Leg::Moving, 3000, late, 0, 0)
        };
        let mapped = LegResult {
            maps: (1, 0),
            ..leg(Leg::Mapped, 3000, 10, 0, 0)
        };
        let base = leg(Leg::Baseline, 3000, 9, 0, 0);
        let c = cx(&s, Strictness::Strict);
        // Within the line: nothing to say.
        let legs = [base.clone(), mapped.clone(), moving(12, (1, 0), 400)];
        let f = judge(&legs, &c);
        assert!(f.is_empty(), "{f:?}");
        // 4% against 0.3%: over Strict's 1%, a Problem in its own words,
        // the mapped leg's own fine.
        let legs = [base.clone(), mapped.clone(), moving(120, (1, 0), 400)];
        let f = judge(&legs, &c);
        assert_eq!(
            problems(&f),
            [
                "With a screen moving in front of a mapped projector the displays missed 4.0% of their pictures; with previews off, 0.3%. On this computer a moving screen costs the show: it is drawn for every frame and never gives way. Try fewer moving screens, or a smaller canvas."
            ]
        );
        assert_eq!(
            in_short(&legs, &c, &f),
            "The 3D stage costs the displays on this computer: see above."
        );
        assert!(problems(&judge(&legs, &cx(&s, Strictness::Relaxed))).is_empty());
        // A screen that never moved: said, and the leg not compared.
        let legs = [base.clone(), mapped.clone(), moving(120, (1, 0), 0)];
        let f = judge(&legs, &c);
        assert!(problems(&f).is_empty(), "{f:?}");
        assert!(
            f.iter()
                .any(|f| f.text.starts_with("The screen didn't move")),
            "{f:?}"
        );
        // The map refused: a Problem (once), not compared.
        let legs = [base.clone(), mapped, moving(120, (0, 1), 400)];
        let f = judge(&legs, &c);
        assert_eq!(problems(&f).len(), 1, "{f:?}");
        assert!(problems(&f)[0].contains("stays black"));
        let part = report_part(&legs, &c, &f).join(" / ");
        assert!(
            part.contains("the screen put somewhere new 400 times"),
            "{part}"
        );
    }

    #[test]
    fn the_screen_is_put_only_in_the_moving_leg_and_swings_about_home() {
        let mut show = Show::default();
        let screen = show.add_surface(None);
        let home = show.stage3d.surface(screen).unwrap().placement;
        let cue = show.make_cue(crate::show::CueType::Screen, None, 0);
        let mv = super::super::mapped::MovingLeg {
            screen,
            screen_name: "Screen 1".into(),
            home,
            cue,
        };
        assert_eq!(mv.place_at(0.0), home);
        let quarter = mv.place_at(super::super::mapped::SWING_SECS / 4.0);
        assert!(
            (quarter.position[0] - home.position[0] - super::super::mapped::SWING_M).abs() < 1e-4
        );
        assert!((quarter.yaw - home.yaw - super::super::mapped::SWING_DEG).abs() < 1e-3);
        let mut s = setup();
        s.legs = vec![Leg::Baseline, Leg::Moving];
        s.mapped = Some(super::super::mapped::MappedLeg {
            projector: crate::stage3d::ProjectorId(1),
            display: show.outputs[0].id,
            display_name: String::new(),
            canvas: show.outputs[0].id,
            before: crate::stage3d::Projector::new(crate::stage3d::ProjectorId(1), "P".into()),
            during: crate::stage3d::Projector::new(crate::stage3d::ProjectorId(1), "P".into()),
            moving: Some(mv),
        });
        let mut st = crate::ui::UiState::default();
        let mut run = StageRun::new(s, &show, 0, false, None);
        run.shorten(10.0);
        let mut puts = 0;
        let mut t = 0.0;
        while !run.finished && t < 100.0 {
            run.tick(&mut show, &mut st, t);
            let put = run.screen_to_put(t);
            assert_eq!(put.is_some(), run.leg() == Some(Leg::Moving), "at {t}");
            if put.is_some() {
                puts += 1;
                assert!(show.movable(screen), "movable while it moves");
            } else {
                assert!(!show.movable(screen), "not movable at {t}");
            }
            t += 0.5;
        }
        assert!(puts > 0);
        assert_eq!(run.results.last().map(|r| r.moves), Some(puts));
    }
}
