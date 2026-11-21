//! What the previews and the 3D stage did while a test ran, for every test's
//! report (step B3 of `.claude/notes/organise-and-test3d-plan.md`).
//!
//! The previews give way to the displays (`preview_pace`); a test should say
//! how far they had to, and whether the displays' missed pictures were theirs.
//! A `Recorder` takes a look at the start (the governor's `Tally`, the 3D
//! stage's `stage_render::totals`, the display windows' `render::shown_totals`
//! and this computer's preview settings), one at each pass of the test (each
//! change of level the governor made since: a row of `previews.csv` and a line
//! for `events.log`; the windows' pictures counted by the level the previews
//! were drawn at) and one at the end, and gives an `Account` of the
//! difference. `report_part` says it in words; `analysis::analyse_previews`
//! judges it.
//!
//! Three calls (`start`, `tick`, `finish`), so a test of any kind can keep
//! one: the cue list and timeline tests do (`session::Run`).

use crate::preview_pace::{Cap, Governor, Level, News, Shown, SizeCap, Step, Strictness, Tally};
use crate::stage_render::StageTotals;
use crate::ui::UiState;
use std::io::Write;
use std::path::Path;
use std::time::Instant;

/// The file the recorder writes in the test's folder.
pub const CSV: &str = "previews.csv";

/// Its header: a row per change of level (`down`, `up`) and per thing the
/// governor said with none (`said`).
pub const CSV_HEADER: &str =
    "t_s,what,from,to,worst_seen_missed,worst_new_pictures,stalls,forced,news";

/// The display windows the counts are kept for (`render`'s slots).
const WINDOWS: usize = crate::preview_pace::WINDOWS;

/// This computer's preview settings when the test began.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Settings {
    pub cap: Cap,
    pub size: SizeCap,
    /// No preview is drawn at all (`Prefs::previews_off`).
    pub off: bool,
    pub strictness: Strictness,
}

impl Settings {
    pub fn of(prefs: &crate::prefs::Prefs) -> Settings {
        Settings {
            cap: prefs.preview_cap,
            size: prefs.preview_size,
            off: prefs.previews_off,
            strictness: prefs.preview_strictness,
        }
    }

    /// The share of a display's new pictures that may be late before the
    /// previews give way, and whether they may: 5% when they never do (the
    /// test judges by the relaxed line then).
    pub fn line(&self) -> (f64, bool) {
        match self.strictness.share() {
            Some(s) => (s, true),
            None => (0.05, false),
        }
    }

    pub fn words(&self) -> String {
        let rate = match self.cap {
            Cap::Automatic => "automatic".to_string(),
            c => format!("at most {}", crate::stats::preview_level_words(c.level())),
        };
        let size = match self.size {
            SizeCap::Full => "full",
            SizeCap::Half => "half",
            SizeCap::Quarter => "a quarter",
        };
        let give = match self.strictness.share() {
            Some(s) => format!(
                "give way once about {:.0}% of a display's new pictures come late ({})",
                s * 100.0,
                self.strictness.label()
            ),
            None => "never give way (Off)".to_string(),
        };
        format!(
            "rate {rate}, size {size}, {give}; previews {}.",
            if self.off {
                "OFF (none were drawn)"
            } else {
                "on"
            }
        )
    }
}

/// The level the previews are drawn at now, as `App::pace_previews` works it
/// out: the governor's, under this computer's cap, paused while they are off.
pub fn drawn_level(st: &UiState) -> Level {
    if st.prefs.previews_off || st.preview_governor.held() {
        Level::Paused
    } else {
        crate::preview_pace::effective(st.preview_governor.governed(), st.prefs.preview_cap)
    }
}

/// The Preview panes open now, in words (the new interface's Operator view
/// and the timeline workspace have a preview of their own, not a pane).
pub fn open_previews(st: &UiState) -> Vec<String> {
    let mut v: Vec<String> = st
        .preview_panes_open
        .iter()
        .map(|n| {
            let mode = st
                .layout
                .previews
                .get(n)
                .map(|p| p.mode)
                .unwrap_or_default();
            let what = match mode {
                crate::dock::PreviewMode::Single => "one display".to_string(),
                crate::dock::PreviewMode::Multiview => "multiview".to_string(),
                crate::dock::PreviewMode::Stage => format!(
                    "3D stage, light {}",
                    if st.preview_stage.get(n).is_some_and(|s| s.light) {
                        "on"
                    } else {
                        "off"
                    }
                ),
            };
            let sight = if st.preview_panes_seen.contains(n) {
                ""
            } else {
                ", out of sight"
            };
            format!("Preview pane {} ({what}{sight})", u32::from(*n) + 1)
        })
        .collect();
    if st.prefs.new_layout {
        v.push("the new interface, whose Operator view has a preview of its own".into());
    }
    v
}

/// What the previews and the stage did over a test.
#[derive(Clone, Debug, Default)]
pub struct Account {
    /// The test's own length (seconds).
    pub secs: f64,
    pub settings: Settings,
    /// The previews open when it began (`open_previews`).
    pub open: Vec<String>,
    /// What the governor did meanwhile (`Tally::since`): its levels are
    /// before this computer's cap and its previews-off setting.
    pub tally: Tally,
    /// Each display window's new pictures and seen misses over the test...
    pub shown: [Shown; WINDOWS],
    /// ...and those while the previews were drawn at `Level::Paused` (or off).
    pub while_paused: [Shown; WINDOWS],
    /// What the 3D stage drew over the test (`StageTotals::since`).
    pub stage: StageTotals,
}

/// A window counts once it has shown this many new pictures (a few pictures
/// with one late is chance, as the governor judges, `Governor::LEAST_PICTURES`).
pub const LEAST_PICTURES: u32 = Governor::LEAST_PICTURES;
/// Pictures shown while the previews were paused before those are judged on
/// their own (about three seconds at 50 a second).
pub const LEAST_PAUSED: u32 = 150;

impl Account {
    /// The worst share of new pictures late among `windows` that showed at
    /// least `least`, and that window's (number from 1, counts).
    pub fn worst_of(windows: &[Shown; WINDOWS], least: u32) -> Option<(usize, Shown, f64)> {
        windows
            .iter()
            .enumerate()
            .filter(|(_, s)| s.pictures >= least.max(1))
            .map(|(i, s)| (i + 1, *s, f64::from(s.late) / f64::from(s.pictures)))
            .max_by(|a, b| a.2.total_cmp(&b.2))
    }

    /// The worst display window over the whole test.
    pub fn worst(&self) -> Option<(usize, Shown, f64)> {
        Account::worst_of(&self.shown, LEAST_PICTURES)
    }

    /// The worst while the previews were paused, if it says anything.
    pub fn worst_paused(&self) -> Option<(usize, Shown, f64)> {
        Account::worst_of(&self.while_paused, LEAST_PAUSED)
            .filter(|(_, s, _)| s.late >= Governor::MIN_SEEN)
    }

    /// Seconds the governor held the previews below full rate or size.
    pub fn secs_below_full(&self) -> f64 {
        (self.tally.secs() - self.tally.secs_at(Level::Full)).max(0.0)
    }

    /// The lowest level the governor held them at (Full: never lower).
    pub fn lowest(&self) -> Level {
        Level::ALL
            .iter()
            .rev()
            .copied()
            .find(|l| self.tally.secs_at(*l) > 0.0)
            .unwrap_or(Level::Full)
            .max(
                self.tally
                    .steps
                    .iter()
                    .map(|s| s.to)
                    .max()
                    .unwrap_or(Level::Full),
            )
    }

    /// How many times it said the displays' trouble wasn't the previews'.
    pub fn not_theirs(&self) -> usize {
        self.tally
            .news
            .iter()
            .filter(|(_, n)| matches!(n, News::NotTheirs { .. }))
            .count()
    }
}

/// Watches the previews through a test (see the module's account).
pub struct Recorder {
    tally: Tally,
    stage: StageTotals,
    shown: [Shown; WINDOWS],
    last_shown: [Shown; WINDOWS],
    while_paused: [Shown; WINDOWS],
    steps_seen: u64,
    news_seen: u64,
    settings: Settings,
    open: Vec<String>,
    csv: Option<std::io::BufWriter<std::fs::File>>,
    /// The engine's `jolts` at the last look, and until when what the
    /// windows show isn't counted towards `while_paused` (a cue started or a
    /// jump landed: what it costs isn't the previews' doing either way, as
    /// the governor's own `Governor::GRACE`).
    jolts: u64,
    grace_until: Option<Instant>,
}

impl Recorder {
    /// The first look, as a test begins (`jolts`: `Engine::jolts`);
    /// `previews.csv` in `folder` (none: no file).
    pub fn start(folder: Option<&Path>, st: &UiState, jolts: u64) -> Recorder {
        let tally = st.preview_governor.tally().clone();
        let shown = crate::render::shown_totals();
        let csv = folder
            .and_then(|f| std::fs::File::create(f.join(CSV)).ok())
            .map(std::io::BufWriter::new)
            .and_then(|mut f| {
                writeln!(f, "{CSV_HEADER}").ok()?;
                f.flush().ok()?;
                Some(f)
            });
        Recorder {
            steps_seen: tally.steps_total,
            news_seen: tally.news_total,
            tally,
            stage: crate::stage_render::totals(),
            shown,
            last_shown: shown,
            while_paused: [Shown::default(); WINDOWS],
            settings: Settings::of(&st.prefs),
            open: open_previews(st),
            csv,
            jolts,
            grace_until: None,
        }
    }

    /// A look each pass, `t` seconds into the test: a row of `previews.csv`
    /// for each change of level since the last, and the lines for
    /// `events.log`.
    pub fn tick(&mut self, st: &UiState, jolts: u64, t: f64) -> Vec<String> {
        let shown = crate::render::shown_totals();
        let now = Instant::now();
        if jolts != self.jolts {
            self.jolts = jolts;
            self.grace_until = Some(now + Governor::GRACE);
        }
        let in_grace = self.grace_until.is_some_and(|g| now < g);
        if drawn_level(st) == Level::Paused && !in_grace {
            for ((p, now), was) in self
                .while_paused
                .iter_mut()
                .zip(&shown)
                .zip(&self.last_shown)
            {
                let d = now.since(*was);
                p.pictures += d.pictures;
                p.late += d.late;
            }
        }
        self.last_shown = shown;
        let tally = st.preview_governor.tally();
        let steps = newest(&tally.steps, tally.steps_total, self.steps_seen);
        let news: Vec<News> = newest(&tally.news, tally.news_total, self.news_seen)
            .into_iter()
            .map(|(_, n)| n)
            .collect();
        self.steps_seen = tally.steps_total;
        self.news_seen = tally.news_total;
        let mut lines = Vec::new();
        for s in &steps {
            self.row(t, s);
            lines.push(step_line(s));
        }
        // What it said with a step is in that step's row and line.
        for n in news
            .iter()
            .filter(|n| !steps.iter().any(|s| s.news == Some(**n)))
        {
            self.said(t, drawn_level(st), n);
            lines.push(format!("Previews: {}", news_words(n)));
        }
        lines
    }

    fn row(&mut self, t: f64, s: &Step) {
        if let Some(f) = &mut self.csv {
            let _ = writeln!(
                f,
                "{t:.1},{},{},{},{},{},{},{},{}",
                if s.is_down() { "down" } else { "up" },
                level_token(s.from),
                level_token(s.to),
                s.worst.late,
                s.worst.pictures,
                s.stalls,
                u8::from(s.forced),
                s.news.as_ref().map_or(String::new(), csv_news)
            );
            let _ = f.flush();
        }
    }

    fn said(&mut self, t: f64, at: Level, n: &News) {
        if let Some(f) = &mut self.csv {
            let l = level_token(at);
            let _ = writeln!(f, "{t:.1},said,{l},{l},,,,,{}", csv_news(n));
            let _ = f.flush();
        }
    }

    /// The last look, `secs` into the test (after a last `tick`).
    pub fn finish(&mut self, st: &UiState, secs: f64) -> Account {
        let now = crate::render::shown_totals();
        Account {
            secs,
            settings: self.settings,
            open: self.open.clone(),
            tally: st.preview_governor.tally().since(&self.tally),
            shown: std::array::from_fn(|i| now[i].since(self.shown[i])),
            while_paused: self.while_paused,
            stage: crate::stage_render::totals().since(&self.stage),
        }
    }
}

/// The items of a kept list (`Tally::steps`, `news`) after the first `seen`
/// of `total` (those let go before are missing).
fn newest<T: Clone>(all: &std::collections::VecDeque<T>, total: u64, seen: u64) -> Vec<T> {
    let new = total.saturating_sub(seen).min(all.len() as u64) as usize;
    all.iter().skip(all.len() - new).cloned().collect()
}

/// A level in `previews.csv`.
pub fn level_token(l: Level) -> &'static str {
    match l {
        Level::Full => "full",
        Level::Fps60 => "60",
        Level::Fps30 => "30",
        Level::Fps15 => "15",
        Level::Fps5 => "5",
        Level::Half => "half",
        Level::Quarter => "quarter",
        Level::Paused => "paused",
    }
}

fn csv_news(n: &News) -> String {
    format!("\"{}\"", news_words(n).replace('"', "'"))
}

fn words(l: Level) -> &'static str {
    crate::stats::preview_level_words(l)
}

/// What the governor said, for people (its own `News::words` is the log's).
pub fn news_words(n: &News) -> String {
    match n {
        News::NotTheirs { at, back_to, .. } => format!(
            "the displays went on missing pictures with the previews at {}: not the previews' doing, so they went back to {}",
            words(*at),
            words(*back_to)
        ),
        News::TheirsAfterAll { level } => format!(
            "back at {} the displays missed more again: the previews' doing after all, so they gave way again",
            words(*level)
        ),
        News::Surge { level } => format!(
            "at {} the displays missed clearly more than they do by themselves, so the previews gave way again",
            words(*level)
        ),
    }
}

/// A change of level for `events.log`.
pub fn step_line(s: &Step) -> String {
    let mut line = if s.is_down() {
        format!(
            "Previews gave way: {} to {} (the worst display window: {} of {} new pictures late over the last {} s{})",
            words(s.from),
            words(s.to),
            s.worst.late,
            s.worst.pictures,
            Governor::WINDOW.as_secs(),
            match s.stalls {
                0 => String::new(),
                1 => ", and a stall of the main loop".to_string(),
                n => format!(", and {n} stalls of the main loop"),
            }
        )
    } else {
        format!("Previews came back: {} to {}", words(s.from), words(s.to))
    };
    if s.forced {
        line.push_str(" [made-up pressure, NEWQ_PREVIEW_PRESSURE]");
    }
    if let Some(n) = &s.news {
        line.push_str(&format!("; {}", news_words(n)));
    }
    line
}

fn secs_words(s: f64) -> String {
    if s < 90.0 {
        format!("{s:.0} s")
    } else {
        super::session::clock(std::time::Duration::from_secs_f64(s))
    }
}

fn times(n: impl Into<u64>) -> String {
    match n.into() {
        1 => "once".to_string(),
        n => format!("{n} times"),
    }
}

/// `n` thousands-separated.
fn count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A share as a percentage.
pub fn percent(share: f64) -> String {
    format!("{:.1}%", share * 100.0)
}

/// The report's "Previews" part: its lines, without the heading.
pub fn report_part(a: &Account) -> Vec<String> {
    let mut out = Vec::new();
    out.push(format!("This computer's settings: {}", a.settings.words()));
    out.push(if a.open.is_empty() {
        "Open when the test began: no Preview pane.".to_string()
    } else {
        format!(
            "Open when the test began: {}. (Panes opened or closed during the test are not followed.)",
            a.open.join("; ")
        )
    });
    let measured = a.tally.secs();
    if measured <= 0.0 {
        out.push(
            "The previews were not paced during the test (no window drew them), so there is no account of their levels."
                .into(),
        );
    } else {
        let of = if measured + 5.0 < a.secs {
            format!(
                "of {} measured (the test ran {})",
                secs_words(measured),
                secs_words(a.secs)
            )
        } else {
            format!("of {}", secs_words(measured))
        };
        out.push(format!(
            "Time at each level the previews' governor set (before this computer's own limits), {of}:"
        ));
        for l in Level::ALL {
            out.push(format!(
                "  {:38} {:>6}",
                words(l),
                secs_words(a.tally.secs_at(l))
            ));
        }
        if a.tally.secs_off > 0.0 {
            out.push(format!(
                "  ({} of it set never to give way.)",
                secs_words(a.tally.secs_off)
            ));
        }
        if a.tally.secs_forced > 0.0 {
            out.push(format!(
                "  ({} of it under made-up pressure, NEWQ_PREVIEW_PRESSURE.)",
                secs_words(a.tally.secs_forced)
            ));
        }
        if a.tally.down == 0 && a.tally.up == 0 {
            out.push("They never changed level.".into());
        } else {
            out.push(format!(
                "They gave way {} and came back {} (each change is a row of {CSV}).",
                times(a.tally.down),
                times(a.tally.up)
            ));
        }
        if (a.tally.steps.len() as u64) < a.tally.steps_total {
            out.push(format!(
                "The first {} changes were too many to keep and are not in {CSV}.",
                a.tally.steps_total - a.tally.steps.len() as u64
            ));
        }
        if a.tally.news.is_empty() {
            out.push("The governor said nothing beyond its steps.".into());
        } else {
            out.push("What the governor said:".into());
            for (at, n) in &a.tally.news {
                out.push(format!(
                    "  at {}: {}",
                    super::session::clock(std::time::Duration::from_secs_f64(*at)),
                    news_words(n)
                ));
            }
        }
    }
    let windows: Vec<(usize, Shown)> = a
        .shown
        .iter()
        .enumerate()
        .filter(|(_, s)| s.pictures > 0)
        .map(|(i, s)| (i + 1, *s))
        .collect();
    if windows.is_empty() {
        out.push(
            "No display window drew a picture during the test, so there was nothing to judge the previews against."
                .into(),
        );
    } else {
        out.push(
            "Display windows: their new pictures, and those that came late (seen misses):".into(),
        );
        for (n, s) in windows {
            let paused = a.while_paused[n - 1];
            out.push(format!(
                "  window {n}: {} new, {} late ({}){}",
                count(s.pictures.into()),
                count(s.late.into()),
                percent(f64::from(s.late) / f64::from(s.pictures)),
                if paused.pictures > 0 {
                    format!(
                        "; with the previews paused {} new, {} late",
                        count(paused.pictures.into()),
                        count(paused.late.into())
                    )
                } else {
                    String::new()
                }
            ));
        }
    }
    out.extend(stage_lines(&a.stage));
    out.push(super::analysis::previews_sentence(a));
    out
}

/// The 3D stage's part of it.
pub fn stage_lines(s: &StageTotals) -> Vec<String> {
    let mut out = Vec::new();
    if s.views + s.displays == 0 && s.jobs == 0 && s.venue_loads + s.venue_failures == 0 {
        out.push("3D stage: nothing was drawn during the test.".into());
        return out;
    }
    let mut line = format!(
        "3D stage: {} views drawn for the previews and {} for displays that show the stage",
        count(s.views),
        count(s.displays)
    );
    if let Some(mean) = s.mean_cpu_us() {
        line.push_str(&format!(
            ", {:.1} ms of the processor each on average",
            mean as f64 / 1000.0
        ));
    }
    match s.worst_band_ms() {
        Some(u64::MAX) => line.push_str(", the slowest over 133 ms"),
        Some(ms) => line.push_str(&format!(", the slowest up to {ms} ms")),
        None => {}
    }
    line.push_str(" (the graphics card's own time isn't known).");
    out.push(line);
    if s.missing_pictures > 0 {
        out.push(format!(
            "  {} screens were drawn without their display's picture ({} of them in displays that show the stage).",
            count(s.missing_pictures),
            count(s.display_missing_pictures)
        ));
    }
    if s.light_left_out > 0 {
        out.push(format!(
            "  {} views were drawn without the projectors' light: the graphics card would not build it.",
            count(s.light_left_out)
        ));
    }
    if s.throws_left_out > 0 {
        out.push(format!(
            "  At the end, {} projectors were past the {} that can throw light.",
            s.throws_left_out,
            crate::stage3d::light::MAX_THROWS
        ));
    }
    if s.restarts > 0 || s.thread_stops > 0 {
        out.push(format!(
            "  The 3D drawing stopped {} and was started again {}.",
            times(s.thread_stops),
            times(s.restarts)
        ));
    }
    if s.venue_loads > 0 {
        out.push(format!(
            "  The venue was read {} during the test, the last in {} ({} parts, {} triangles).",
            times(s.venue_loads),
            secs_or_ms(s.venue_load_ms),
            count(s.venue_parts),
            count(s.venue_triangles)
        ));
    }
    if s.venue_failures > 0 {
        out.push(format!(
            "  The venue could not be read {}.",
            times(s.venue_failures)
        ));
    }
    out
}

fn secs_or_ms(ms: u64) -> String {
    if ms >= 10_000 {
        format!("{:.0} s", ms as f64 / 1000.0)
    } else {
        format!("{ms} ms")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_are_written_with_commas() {
        assert_eq!(count(0), "0");
        assert_eq!(count(999), "999");
        assert_eq!(count(1000), "1,000");
        assert_eq!(count(1_234_567), "1,234,567");
    }

    #[test]
    fn only_new_items_of_a_kept_list_are_taken() {
        let all: std::collections::VecDeque<u32> = (5..10).collect();
        // Ten in all, the last five kept.
        assert_eq!(newest(&all, 10, 8), vec![8, 9]);
        assert_eq!(newest(&all, 10, 10), Vec::<u32>::new());
        // More new than kept: those kept.
        assert_eq!(newest(&all, 10, 2), vec![5, 6, 7, 8, 9]);
    }

    #[test]
    fn a_recorder_without_an_app_writes_its_header_and_an_empty_account() {
        let dir = std::env::temp_dir().join(format!("newq_previews_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let st = UiState::default();
        let mut r = Recorder::start(Some(&dir), &st, 0);
        assert!(r.tick(&st, 0, 1.0).is_empty());
        let a = r.finish(&st, 2.0);
        assert_eq!(a.tally.secs(), 0.0);
        let text = std::fs::read_to_string(dir.join(CSV)).unwrap();
        assert_eq!(text.trim(), CSV_HEADER);
        let part = report_part(&a).join("\n");
        assert!(part.contains("no Preview pane"), "{part}");
        assert!(part.contains("not paced"), "{part}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn each_step_the_governor_takes_is_a_row_and_a_line_and_counted() {
        use crate::preview_pace::Reading;
        use std::time::Duration;
        let dir = std::env::temp_dir().join(format!("newq_previews_steps_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut st = UiState::default();
        let t0 = Instant::now();
        // A look before the test, so the tally's clock runs from here.
        let calm = |t| {
            let mut shown = [Shown::default(); WINDOWS];
            shown[0].pictures = 6;
            (t, Reading::of(None, true, shown, 0, false))
        };
        let (t, r) = calm(t0);
        st.preview_governor.observe(t, r, Strictness::Relaxed);
        let mut rec = Recorder::start(Some(&dir), &st, 0);
        // Ten seconds of every new picture late (made-up pressure)...
        let mut at = t0;
        for _ in 0..100 {
            at += Duration::from_millis(100);
            let r = Reading::of(Some(1000), true, [Shown::default(); WINDOWS], 0, false);
            st.preview_governor.observe(at, r, Strictness::Relaxed);
        }
        let down = rec.tick(&st, 0, 10.0);
        assert!(!down.is_empty(), "no step down");
        assert!(
            down.iter().all(|l| l.starts_with("Previews gave way: ")
                && l.contains("[made-up pressure, NEWQ_PREVIEW_PRESSURE]")),
            "{down:?}"
        );
        // ...then a minute of calm: they come back.
        for _ in 0..600 {
            at += Duration::from_millis(100);
            let (t, r) = calm(at);
            st.preview_governor.observe(t, r, Strictness::Relaxed);
        }
        let up = rec.tick(&st, 0, 70.0);
        assert!(
            up.iter().any(|l| l.starts_with("Previews came back: ")),
            "{up:?}"
        );
        // Nothing new: nothing written again.
        assert!(rec.tick(&st, 0, 71.0).is_empty());
        let a = rec.finish(&st, 71.0);
        assert_eq!(a.tally.down as usize, down.len());
        assert!(a.tally.up >= 1);
        assert!(a.tally.secs_forced > 9.0 && (a.tally.secs() - 70.0).abs() < 0.5);
        assert_eq!(st.preview_governor.governed(), Level::Full);
        drop(rec);
        let csv = std::fs::read_to_string(dir.join(CSV)).unwrap();
        let rows: Vec<&str> = csv.lines().skip(1).collect();
        assert_eq!(rows.len() as u64, a.tally.steps_total, "{csv}");
        assert!(rows[0].starts_with("10.0,down,full,"), "{csv}");
        assert!(rows[0].split(',').nth(7) == Some("1"), "forced: {csv}");
        assert!(rows.last().unwrap().starts_with("70.0,up,"), "{csv}");
        assert!(rows.last().unwrap().contains(",full,"), "{csv}");
        // And the report part says it.
        let part = report_part(&a).join("\n");
        assert!(
            part.contains(&format!("They gave way {}", times(a.tally.down))),
            "{part}"
        );
        assert!(part.contains("under made-up pressure"), "{part}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_stage_is_said_only_when_it_drew_and_over_the_last_band_is_said_so() {
        assert_eq!(
            stage_lines(&StageTotals::default()),
            ["3D stage: nothing was drawn during the test."]
        );
        let mut s = StageTotals {
            jobs: 10,
            views: 1200,
            displays: 3,
            cpu_us_views: 1203 * 2000,
            restarts: 1,
            thread_stops: 1,
            venue_loads: 1,
            venue_load_ms: 41_000,
            venue_parts: 3,
            venue_triangles: 12_000,
            ..StageTotals::default()
        };
        s.frames_by_ms[3] = 1203;
        let l = stage_lines(&s).join("\n");
        assert!(
            l.contains("1,200 views drawn for the previews and 3 for displays"),
            "{l}"
        );
        assert!(l.contains("2.0 ms of the processor each"), "{l}");
        assert!(l.contains("the slowest up to 8 ms"), "{l}");
        assert!(l.contains("stopped once and was started again once"), "{l}");
        assert!(
            l.contains("the last in 41 s (3 parts, 12,000 triangles)"),
            "{l}"
        );
        s.frames_by_ms[8] = 1;
        assert!(stage_lines(&s)[0].contains("the slowest over 133 ms"));
    }
}
