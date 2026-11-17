//! Several parts as one test: the cue list, the timeline and the network
//! check ticked together run one after another (`TestMode::start_plan`),
//! the user's show put aside once and back once (`UiState::test_carry`), and
//! end as one result with one report and one zip.
//!
//! Each part runs as it does alone, in a folder of its own; at the end
//! `gather` (on the zip's thread) brings their files into the run's folder:
//! one `report.txt` and one `events.log` with a section per part, and every
//! other file under its own name, or, where two parts wrote the same name,
//! under the part's name first (`cue-list-stats.csv`, `timeline-stats.csv`).

use super::analysis::Verdict;
use super::choice::{Part, PartPlan};
use super::plan::{DisplayChoice, SoundChoice};
use super::session::{Done, End, clock};
use crate::media::ffmpeg::Ffmpeg;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// What every part starts with.
#[derive(Clone)]
pub struct Go {
    pub ff: Ffmpeg,
    pub log_dir: PathBuf,
    pub displays: Vec<DisplayChoice>,
    pub sound: SoundChoice,
    pub heavy: bool,
}

/// One part of a finished run: its own account, or why it didn't run.
pub struct PartDone {
    pub part: PartPlan,
    pub done: Option<Box<Done>>,
    pub why_not: Option<String>,
}

/// A run of several parts under way.
pub struct Sequence {
    plan: Vec<PartPlan>,
    current: usize,
    results: Vec<Done>,
    pub go: Go,
    /// The user asked to stop.
    pub stopped: bool,
    /// NewQ is closing.
    pub quit: bool,
    started: Instant,
    /// The tests of the test: the shows of the parts after the first, made.
    #[cfg(test)]
    pub made: std::collections::VecDeque<Made>,
}

/// A part's show already made (the tests of the test).
#[cfg(test)]
pub enum Made {
    Cues(Box<super::plan::Built>),
    Network(
        Box<super::network::Built>,
        Vec<(f64, super::network::Act)>,
        f64,
    ),
}

impl Sequence {
    pub fn new(plan: Vec<PartPlan>, go: Go) -> Sequence {
        Sequence {
            plan,
            current: 0,
            results: Vec::new(),
            go,
            stopped: false,
            quit: false,
            started: Instant::now(),
            #[cfg(test)]
            made: Default::default(),
        }
    }

    /// The part playing (from 0), of how many, and which.
    pub fn at(&self) -> (usize, usize, Part) {
        (self.current, self.plan.len(), self.plan[self.current].part)
    }

    /// More parts come after the one playing.
    pub fn more_follow(&self) -> bool {
        self.current + 1 < self.plan.len()
    }

    /// The part playing has ended with this account.
    pub fn record(&mut self, done: Done) {
        self.results.push(done);
    }

    /// Move on to the next part, if there is one.
    pub fn next_part(&mut self) -> Option<PartPlan> {
        if self.stopped || !self.more_follow() {
            return None;
        }
        self.current += 1;
        Some(self.plan[self.current])
    }

    /// The run's one account (None if no part got as far as one: the
    /// window's notice says why). `unstarted`: why the part after the last
    /// account didn't begin.
    pub fn finish(self, unstarted: Option<String>) -> Option<Done> {
        if self.results.is_empty() {
            return None;
        }
        let elapsed = self.started.elapsed();
        let ran = self.results.len();
        let mut ended = self
            .results
            .iter()
            .map(|d| d.ended.clone())
            .find(|e| *e != End::Finished)
            .unwrap_or(End::Finished);
        if ended == End::Finished
            && let Some(why) = &unstarted
        {
            ended = if self.quit {
                End::Quit
            } else if self.stopped {
                End::Stopped
            } else {
                End::Failed(why.clone())
            };
        }
        let mut results = self.results.into_iter();
        let parts: Vec<PartDone> = self
            .plan
            .iter()
            .enumerate()
            .map(|(i, p)| PartDone {
                part: *p,
                done: results.next().map(Box::new),
                why_not: (i >= ran).then(|| not_run(i == ran, &ended)),
            })
            .collect();
        let verdict = overall(&parts, &ended);
        let headline = headline(&parts, &ended, verdict);
        let level = self.plan[0].level;
        let report = report(&parts, verdict, &headline, elapsed);
        let log_dir = self.go.log_dir.clone();
        let name = format!(
            "{}-{}",
            self.plan
                .iter()
                .map(|p| p.part.slug())
                .collect::<Vec<_>>()
                .join("-"),
            level.label().to_lowercase()
        );
        let folder = super::session::run_folder_named(&log_dir, &name, super::session::unix_now());
        let _ = std::fs::create_dir_all(&folder);
        let from: Vec<(&'static str, PathBuf)> = parts
            .iter()
            .filter_map(|p| {
                p.done
                    .as_ref()
                    .map(|d| (p.part.part.slug(), d.folder.clone()))
            })
            .collect();
        let into = folder.clone();
        let text = report.clone();
        let bundle = super::bundle::Bundle::start_after(
            move || gather(&into, &from, &text),
            folder.clone(),
            log_dir,
            matches!(ended, End::Quit),
            super::bundle::TIDY,
        );
        Some(Done {
            compare: None,
            flavour: super::Flavour::Cues,
            full: None,
            network: None,
            computers: Vec::new(),
            bundle,
            level,
            verdict,
            headline,
            findings: Vec::new(),
            steady: None,
            folder,
            report,
            laps: parts
                .iter()
                .filter_map(|p| p.done.as_ref())
                .map(|d| d.laps)
                .sum(),
            elapsed,
            ended,
            parts,
        })
    }
}

/// Why a part didn't run: the one after the last that did (`next`), or a later one.
fn not_run(next: bool, ended: &End) -> String {
    match (next, ended) {
        (true, End::Failed(why)) => format!("It did not start: {why}"),
        (_, End::Stopped) => "Not run: you stopped the test before it.".into(),
        (_, End::Quit) => "Not run: NewQ was closed before it.".into(),
        _ => "Not run: the test ended before it.".into(),
    }
}

/// The worst of the parts' verdicts; a run cut short by a failure is a problem.
pub fn overall(parts: &[PartDone], ended: &End) -> Verdict {
    let worst = parts
        .iter()
        .filter_map(|p| p.done.as_ref())
        .map(|d| d.verdict)
        .fold(Verdict::Passed, |a, b| match (a, b) {
            (Verdict::ProblemsFound, _) | (_, Verdict::ProblemsFound) => Verdict::ProblemsFound,
            (Verdict::PassedWithNotes, _) | (_, Verdict::PassedWithNotes) => {
                Verdict::PassedWithNotes
            }
            _ => Verdict::Passed,
        });
    if matches!(ended, End::Failed(_)) {
        Verdict::ProblemsFound
    } else if worst == Verdict::Passed && parts.iter().any(|p| p.done.is_none()) {
        // Not everything ran: not a clean pass.
        Verdict::PassedWithNotes
    } else {
        worst
    }
}

/// The run's one line, overall.
pub fn headline(parts: &[PartDone], ended: &End, verdict: Verdict) -> String {
    let last_ran = parts.iter().rev().find(|p| p.done.is_some());
    let first_not = parts.iter().find(|p| p.done.is_none());
    let name = |p: Option<&PartDone>| p.map_or("", |p| p.part.part.name());
    let cut = last_ran.is_some_and(|p| p.done.as_ref().is_some_and(|d| d.ended != End::Finished));
    match ended {
        End::Failed(why) if cut => format!("The {} part could not go on: {why}", name(last_ran)),
        End::Failed(why) => format!("The {} part did not start: {why}", name(first_not)),
        End::Stopped if cut => format!("You stopped it during the {} part.", name(last_ran)),
        End::Stopped => format!("You stopped it before the {} part.", name(first_not)),
        End::Quit => "NewQ was closed before the test was done.".into(),
        End::Finished => match verdict {
            Verdict::Passed => "Every part held up.".into(),
            Verdict::PassedWithNotes => "Nothing went wrong; there are notes.".into(),
            Verdict::ProblemsFound => {
                let bad: Vec<&str> = parts
                    .iter()
                    .filter(|p| {
                        p.done
                            .as_ref()
                            .is_some_and(|d| d.verdict == Verdict::ProblemsFound)
                    })
                    .map(|p| p.part.part.name())
                    .collect();
                format!("Problems were found in: {}.", bad.join(", "))
            }
        },
    }
}

/// What a part's line says: "Passed", or why it didn't run.
pub fn part_word(p: &PartDone) -> String {
    match (&p.done, &p.why_not) {
        (Some(d), _) => match &d.ended {
            End::Finished => d.verdict.label().to_string(),
            End::Stopped => format!("{} (stopped early)", d.verdict.label()),
            End::Quit => format!("{} (NewQ was closed)", d.verdict.label()),
            End::Failed(_) => "Could not go on".to_string(),
        },
        (None, Some(why)) => why.clone(),
        (None, None) => "Not run".into(),
    }
}

/// The part's title in the window and the report: "Cue list (Short)".
pub fn part_title(p: &PartPlan) -> String {
    match p.part {
        Part::Network => super::network::title(p.laps),
        _ if p.minutes > 0 => format!("{} ({} minutes)", p.part.name(), p.minutes),
        _ => format!("{} ({})", p.part.name(), p.level.label()),
    }
}

/// The whole run's report.txt: the overall result, a line per part, then each
/// part's own report in order.
pub fn report(
    parts: &[PartDone],
    verdict: Verdict,
    headline: &str,
    elapsed: std::time::Duration,
) -> String {
    let mut out = String::new();
    out.push_str("NewQ Test Mode report\n=====================\n");
    out.push_str(&format!("Result: {}\n", verdict.label()));
    out.push_str(&format!(
        "Test: {} parts, one after another, ran {}.\n",
        parts.len(),
        clock(elapsed)
    ));
    out.push_str(&format!("{headline}\n\n"));
    for (i, p) in parts.iter().enumerate() {
        out.push_str(&format!(
            "Part {} of {}: {}: {}\n",
            i + 1,
            parts.len(),
            part_title(&p.part),
            part_word(p)
        ));
    }
    out.push_str("\nYour show was put aside once, for the whole test, and is back, unchanged.\n");
    out.push_str(
        "Files: each part's report and events are below and in events.log; a file two parts both wrote starts with the part's name (cue-list-stats.csv).\n",
    );
    for (i, p) in parts.iter().enumerate() {
        let Some(d) = &p.done else { continue };
        let title = format!("Part {}: {}", i + 1, part_title(&p.part));
        out.push_str(&format!(
            "\n\n{}\n{title}\n{}\n\n",
            "#".repeat(60),
            "#".repeat(60)
        ));
        out.push_str(&d.report);
    }
    out
}

/// The files that are joined into one (a section per part), not copied, and
/// the log excerpt, which the zip has whole as `newq.log`.
const JOINED: [&str; 3] = ["report.txt", "events.log", "newq-log-excerpt.txt"];

/// Where each part's files go in the run's folder: `(part, its name, the
/// name in the run's folder)`. A name only one part wrote stays as it is;
/// one several wrote gets the part's name in front, except in the part whose
/// own it is (`network.csv` of the network part, `timeline.csv` of the
/// timeline part).
pub fn file_names(parts: &[(&str, Vec<String>)]) -> Vec<(usize, String, String)> {
    let count = |name: &str| {
        parts
            .iter()
            .filter(|(_, names)| names.iter().any(|n| n == name))
            .count()
    };
    let mut v = Vec::new();
    for (i, (slug, names)) in parts.iter().enumerate() {
        for n in names {
            if JOINED.contains(&n.as_str()) {
                continue;
            }
            // A part's own file keeps its name (the network part's
            // network.csv; a cue list part across a network writes one too).
            let own = n == &format!("{slug}.csv");
            let to = if count(n) > 1 && !own {
                format!("{slug}-{n}")
            } else {
                n.clone()
            };
            v.push((i, n.clone(), to));
        }
    }
    v
}

/// Bring the parts' files into the run's `folder` (on the zip's thread):
/// the joined report and events, every other file moved in, and the parts'
/// folders removed.
pub fn gather(folder: &Path, parts: &[(&'static str, PathBuf)], report: &str) {
    let _ = std::fs::create_dir_all(folder);
    let listed: Vec<(&str, Vec<String>)> = parts
        .iter()
        .map(|(slug, dir)| {
            let mut names: Vec<String> = std::fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            (*slug, names)
        })
        .collect();
    for (i, from, to) in file_names(&listed) {
        let src = parts[i].1.join(&from);
        let dst = folder.join(&to);
        if std::fs::rename(&src, &dst).is_err() && std::fs::copy(&src, &dst).is_ok() {
            let _ = std::fs::remove_file(&src);
        }
    }
    let mut events = String::new();
    for (slug, dir) in parts {
        let text = std::fs::read_to_string(dir.join("events.log")).unwrap_or_default();
        events.push_str(&format!("===== {slug} =====\n{text}\n"));
    }
    let _ = std::fs::write(folder.join("events.log"), events);
    let _ = std::fs::write(folder.join("report.txt"), report);
    for (_, dir) in parts {
        if dir != folder {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testmode::Level;

    fn plan(part: Part) -> PartPlan {
        PartPlan {
            part,
            level: Level::Short,
            minutes: 0,
            laps: 1,
        }
    }

    fn done(verdict: Verdict, ended: End, report: &str) -> Box<Done> {
        Box::new(Done {
            compare: None,
            flavour: crate::testmode::Flavour::Cues,
            full: None,
            network: None,
            parts: Vec::new(),
            computers: Vec::new(),
            bundle: Default::default(),
            level: Level::Short,
            verdict,
            headline: String::new(),
            findings: Vec::new(),
            steady: None,
            folder: PathBuf::new(),
            report: report.into(),
            laps: 1,
            elapsed: std::time::Duration::from_secs(60),
            ended,
        })
    }

    fn part(p: Part, d: Option<Box<Done>>, why: Option<&str>) -> PartDone {
        PartDone {
            part: plan(p),
            done: d,
            why_not: why.map(Into::into),
        }
    }

    #[test]
    fn two_parts_writing_the_same_name_are_told_apart_by_the_parts_name() {
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let got = file_names(&[
            (
                "cue-list",
                names(&[
                    "events.log",
                    "laps.csv",
                    "report.txt",
                    "stats.csv",
                    "system.txt",
                ]),
            ),
            (
                "timeline",
                names(&["events.log", "laps.csv", "stats.csv", "timeline.csv"]),
            ),
            (
                "network",
                names(&["computers.txt", "events.log", "network.csv", "report.txt"]),
            ),
            (
                // A cue list part across a network writes network.csv too.
                "cue-list",
                names(&["network.csv"]),
            ),
        ]);
        let to: Vec<&str> = got.iter().map(|(_, _, t)| t.as_str()).collect();
        assert_eq!(
            to,
            [
                "cue-list-laps.csv",
                "cue-list-stats.csv",
                "system.txt",
                "timeline-laps.csv",
                "timeline-stats.csv",
                "timeline.csv",
                "computers.txt",
                "network.csv",
                "cue-list-network.csv",
            ]
        );
        // No two the same.
        let mut seen = std::collections::HashSet::new();
        assert!(to.iter().all(|t| seen.insert(*t)));
    }

    #[test]
    fn the_parts_files_are_gathered_into_one_folder_with_one_report_and_one_events_log() {
        let root = std::env::temp_dir().join(format!("newq_seq_gather_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let a = root.join("a");
        let b = root.join("b");
        for (dir, events) in [(&a, "cue things"), (&b, "network things")] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("events.log"), events).unwrap();
            std::fs::write(dir.join("report.txt"), "part report").unwrap();
            std::fs::write(dir.join("system.txt"), "same name").unwrap();
        }
        std::fs::write(b.join("network.csv"), "x").unwrap();
        let into = root.join("run");
        gather(
            &into,
            &[("cue-list", a.clone()), ("network", b.clone())],
            "WHOLE",
        );
        let mut names: Vec<String> = std::fs::read_dir(&into)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "cue-list-system.txt",
                "events.log",
                "network-system.txt",
                "network.csv",
                "report.txt"
            ]
        );
        assert_eq!(
            std::fs::read_to_string(into.join("report.txt")).unwrap(),
            "WHOLE"
        );
        let events = std::fs::read_to_string(into.join("events.log")).unwrap();
        let (c, n) = (
            events.find("===== cue-list").unwrap(),
            events.find("===== network").unwrap(),
        );
        assert!(c < n && events.contains("cue things") && events.contains("network things"));
        assert!(!a.exists() && !b.exists(), "the parts' folders are gone");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_overall_verdict_is_the_worst_part_and_says_which() {
        let parts = vec![
            part(
                Part::CueList,
                Some(done(Verdict::Passed, End::Finished, "A")),
                None,
            ),
            part(
                Part::Network,
                Some(done(Verdict::ProblemsFound, End::Finished, "B")),
                None,
            ),
        ];
        let v = overall(&parts, &End::Finished);
        assert_eq!(v, Verdict::ProblemsFound);
        assert_eq!(
            headline(&parts, &End::Finished, v),
            "Problems were found in: Network."
        );
        let r = report(&parts, v, "H", std::time::Duration::from_secs(250));
        assert!(r.starts_with("NewQ Test Mode report"));
        assert!(r.contains("Result: Problems found"));
        let (one, two) = (
            r.find("Part 1 of 2: Cue list (Short): Passed").unwrap(),
            r.find("Part 2 of 2: Network test: Problems found").unwrap(),
        );
        assert!(one < two);
        // Each part's own report, in order, after the summary.
        let (a, b) = (r.find("\nA").unwrap(), r.find("\nB").unwrap());
        assert!(two < a && a < b, "{r}");
        let fine = vec![
            part(
                Part::CueList,
                Some(done(Verdict::Passed, End::Finished, "")),
                None,
            ),
            part(
                Part::Timeline,
                Some(done(Verdict::Passed, End::Finished, "")),
                None,
            ),
        ];
        assert_eq!(overall(&fine, &End::Finished), Verdict::Passed);
        assert_eq!(
            headline(&fine, &End::Finished, Verdict::Passed),
            "Every part held up."
        );
    }

    #[test]
    fn a_run_cut_short_says_which_part_and_what_did_not_run() {
        // Stopped in the second part: the third never ran.
        let parts = vec![
            part(
                Part::CueList,
                Some(done(Verdict::Passed, End::Finished, "")),
                None,
            ),
            part(
                Part::Timeline,
                Some(done(Verdict::PassedWithNotes, End::Stopped, "")),
                None,
            ),
            part(Part::Network, None, Some(&not_run(false, &End::Stopped))),
        ];
        let v = overall(&parts, &End::Stopped);
        assert_eq!(v, Verdict::PassedWithNotes);
        assert_eq!(
            headline(&parts, &End::Stopped, v),
            "You stopped it during the Timeline part."
        );
        assert_eq!(part_word(&parts[1]), "Passed, with notes (stopped early)");
        assert_eq!(
            part_word(&parts[2]),
            "Not run: you stopped the test before it."
        );
        // The network part couldn't start: a problem, said plainly.
        let why = End::Failed("Left is not in step".into());
        let parts = vec![
            part(
                Part::CueList,
                Some(done(Verdict::Passed, End::Finished, "")),
                None,
            ),
            part(Part::Network, None, Some(&not_run(true, &why))),
        ];
        assert_eq!(overall(&parts, &why), Verdict::ProblemsFound);
        assert_eq!(
            headline(&parts, &why, Verdict::ProblemsFound),
            "The Network part did not start: Left is not in step"
        );
        assert_eq!(
            part_word(&parts[1]),
            "It did not start: Left is not in step"
        );
    }

    #[test]
    fn a_sequence_moves_on_only_while_more_follow_and_nobody_stopped_it() {
        let ff = Ffmpeg {
            ffmpeg: PathBuf::from("ffmpeg"),
            ffprobe: PathBuf::from("ffprobe"),
        };
        let go = Go {
            ff,
            log_dir: std::env::temp_dir(),
            displays: Vec::new(),
            sound: SoundChoice::default(),
            heavy: false,
        };
        let mut s = Sequence::new(
            vec![
                plan(Part::CueList),
                plan(Part::Timeline),
                plan(Part::Network),
            ],
            go,
        );
        assert_eq!(s.at(), (0, 3, Part::CueList));
        assert!(s.more_follow());
        assert_eq!(s.next_part().map(|p| p.part), Some(Part::Timeline));
        s.stopped = true;
        assert_eq!(s.next_part(), None, "stopped: no more");
        assert_eq!(s.at(), (1, 3, Part::Timeline));
        // No part got as far as an account: nothing to show but the notice.
        assert!(s.finish(Some("Cancelled.".into())).is_none());
    }
}
