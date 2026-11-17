//! The computers of a show network taking part in a test: every test, not
//! only the network test, runs across the network when it is started on a
//! primary with computers in step (stage 6, "every test on every computer").
//!
//! The test show and its media reach the computers the way any show does
//! (show sync, the media manager); this waits until every computer has them
//! (`media_ready`), tells each to measure (`NetTest::Begin`; with `Mute` when
//! the test runs muted), takes in what each sends back: how it keeps to this
//! computer (`net::measure`) and how it copes itself (`net::measure_local`:
//! memory, processor, frame rate per output, dropouts), notes silences, and
//! at the end judges each computer with Test Mode's own findings
//! (`analysis`) and the network test's (`net_judge`), a part per computer.
//!
//! Nothing here touches a socket or the show: `out` holds what is to be sent
//! (`TestMode::net_out`), and the test that owns it plays and restores.

use super::analysis::{self, Finding, Severity, Verdict};
use super::net_judge::{self, ComputerNotes, ComputerVerdict, Transfer};
use super::session::FindingLog;
use crate::net::hub::NetView;
use crate::net::measure::NetTest;
use crate::net::measure_local::{LocalReport, LocalSample, OutputRate};
use crate::net::media::manager::{MediaReport, MediaView};
use crate::net::node::NodeId;
use crate::net::store::paths::path_key;
use crate::show::Show;
use crate::ui::UiState;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::time::{Duration, Instant};

/// A computer silent this long is said in the window to be away (s).
pub const AWAY_SECS: f64 = 3.0;
/// After the end, the computers' last reports are waited for this long (s).
pub const ENDING_LIMIT: f64 = 8.0;
/// How long the media may take to reach every computer before the test gives up.
pub const MEDIA_LIMIT: Duration = Duration::from_secs(20 * 60);
/// A computer with all the media that isn't in step after this long ends the
/// wait (it plays what the primary plays only once it is).
pub const STEP_LIMIT: Duration = Duration::from_secs(90);
/// A computer that stops answering while the test waits for it ends the wait
/// after this long.
pub const GONE_LIMIT: Duration = Duration::from_secs(30);
/// An output drawing fewer frames a second than this, while it draws one after
/// another, stutters: the test's slowest film is 24 frames a second.
pub const STUTTER_FPS: f32 = 15.0;
/// ...for at least this many readings (one about every second or two).
pub const STUTTER_READINGS: usize = 2;

// ----- Media on the computers ---------------------------------------------------

/// Where a computer's copy of the test's files stands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MediaState {
    /// It hasn't the primary's list of them yet.
    Waiting,
    /// On their way: bytes still to come, of all the test's.
    Coming {
        left: u64,
        total: u64,
    },
    Here,
}

/// From a computer's media report: does it have the test's `files` of the
/// version `current` the primary serves?
pub fn media_state(
    r: Option<&MediaReport>,
    current: Option<&str>,
    files: &[(String, u64)],
) -> MediaState {
    let total: u64 = files.iter().map(|(_, s)| s).sum();
    let Some(r) = r else {
        return MediaState::Waiting;
    };
    if current.is_none() || r.progress.manifest_id.as_deref() != current {
        return MediaState::Waiting;
    }
    let keys: Vec<String> = files.iter().map(|(f, _)| path_key(f)).collect();
    let left: u64 = r
        .lacking
        .iter()
        .filter(|l| keys.contains(&path_key(&l.path)))
        .map(|l| l.total.saturating_sub(l.done).max(1))
        .sum();
    if left == 0 {
        MediaState::Here
    } else {
        MediaState::Coming { left, total }
    }
}

/// The files a test show plays, as the show names them, and their sizes (the
/// list `media_ready` waits for); not those of a list that never plays.
pub fn files_of(show: &Show, skip_list: Option<u32>) -> Vec<(String, u64)> {
    let mut out: Vec<(String, u64)> = Vec::new();
    for c in show.cues.iter().filter(|c| Some(c.list) != skip_list) {
        let Some(m) = c.kind.file_media() else {
            continue;
        };
        if m.file.as_os_str().is_empty() {
            continue;
        }
        let s = m.file.display().to_string();
        if !out.iter().any(|(f, _)| *f == s) {
            let size = std::fs::metadata(&m.file).map(|x| x.len()).unwrap_or(0);
            out.push((s, size));
        }
    }
    out
}

/// A computer's transfer, as the test watched it.
#[derive(Clone, Copy, Debug, Default)]
struct Watched {
    /// When it first had the list, and how much was then still to come.
    first: Option<(Instant, u64)>,
    done: Option<Instant>,
}

// ----- Frame rates ----------------------------------------------------------------

/// Each output's frame rate as it was read, over a test.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OutputWatch {
    /// By output name: (seconds into the test, frames a second, refresh).
    pub readings: BTreeMap<String, Vec<(f64, f32, Option<f32>)>>,
}

impl OutputWatch {
    /// The outputs' rates at `t` seconds into the test.
    pub fn note(&mut self, t: f64, rates: &[OutputRate]) {
        for r in rates.iter().filter(|r| r.fps.is_finite()) {
            let v = self.readings.entry(r.name.clone()).or_default();
            // (A reading is fresh for two seconds: one a second is enough.)
            if v.last().is_none_or(|(at, _, _)| t - at >= 1.0) {
                v.push((t, r.fps, r.refresh_hz));
            }
        }
    }

    /// The latest reading of each output.
    pub fn now(&self) -> Vec<(String, f32)> {
        self.readings
            .iter()
            .filter_map(|(n, v)| v.last().map(|(_, f, _)| (n.clone(), *f)))
            .collect()
    }

    /// "Display 1 59.9 fps, Display 2 4.1 fps".
    pub fn now_text(&self) -> String {
        let now = self.now();
        if now.is_empty() {
            return "no output drawing".into();
        }
        now.iter()
            .map(|(n, f)| format!("{n} {f:.0} fps"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The stretches an output stuttered: `STUTTER_READINGS` or more readings
    /// in a row under `STUTTER_FPS`: (output, from, to, worst fps, refresh).
    pub fn stutters(&self) -> Vec<(String, f64, f64, f32, Option<f32>)> {
        let mut out = Vec::new();
        for (name, v) in &self.readings {
            let mut run: Vec<&(f64, f32, Option<f32>)> = Vec::new();
            let mut close = |run: &mut Vec<&(f64, f32, Option<f32>)>| {
                if run.len() >= STUTTER_READINGS {
                    let worst = run.iter().map(|r| r.1).fold(f32::MAX, f32::min);
                    out.push((
                        name.clone(),
                        run[0].0,
                        run[run.len() - 1].0,
                        worst,
                        run[0].2,
                    ));
                }
                run.clear();
            };
            for r in v {
                if r.1 < STUTTER_FPS {
                    run.push(r);
                } else {
                    close(&mut run);
                }
            }
            close(&mut run);
        }
        out
    }

    /// The findings about the outputs of computer `who` ("This computer").
    pub fn findings(&self, who: &str) -> Vec<Finding> {
        self.stutters()
            .into_iter()
            .map(|(name, from, to, worst, refresh)| Finding {
                severity: Severity::Problem,
                text: format!(
                    "{who}: the output {name} drew only about {worst:.0} frames a second for {} (from {} into the test){}: its picture stutters there.",
                    net_judge::span((to - from).max(1.0)),
                    net_judge::at(from),
                    refresh.map_or(String::new(), |r| format!(
                        "; its display runs at {r:.0} Hz"
                    ))
                ),
            })
            .collect()
    }

    /// The report's line: each output's lowest and usual rate.
    pub fn summary(&self) -> Vec<String> {
        self.readings
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(n, v)| {
                let mut f: Vec<f32> = v.iter().map(|r| r.1).collect();
                f.sort_by(f32::total_cmp);
                format!(
                    "{n}: usually {:.0} frames a second, lowest {:.0}{}",
                    f[f.len() / 2],
                    f[0],
                    v.last()
                        .and_then(|r| r.2)
                        .map_or(String::new(), |r| format!(" (display at {r:.0} Hz)"))
                )
            })
            .collect()
    }
}

/// This computer's output windows' rates now (`UiState::output_fps`), named
/// as the show names them, with their monitors' refresh.
pub fn own_rates(show: &Show, st: &UiState) -> Vec<OutputRate> {
    let mut v: Vec<OutputRate> = st
        .output_fps
        .iter()
        .map(|(id, fps)| {
            let name = show
                .output(*id)
                .map_or_else(|| format!("output {}", id.0), |o| o.name.clone());
            let refresh = match st.display_states.get(id) {
                Some(crate::show::DisplayState::Connected(m)) => {
                    st.monitors.get(*m).and_then(|m| m.refresh_hz)
                }
                _ => None,
            };
            OutputRate::new(&name, *fps, refresh)
        })
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

// ----- One computer ------------------------------------------------------------------

/// What a computer measured of itself (`NetTest::Local`).
#[derive(Clone, Debug, Default)]
pub struct LocalNotes {
    pub samples: Vec<LocalSample>,
    pub dropouts: Vec<f64>,
    pub outputs: OutputWatch,
    pub sound_device: Option<String>,
    pub ram_mb: u64,
    pub reports: u32,
    pub ended: bool,
}

impl LocalNotes {
    pub fn take(&mut self, r: LocalReport) {
        self.reports += 1;
        let t = r.samples.last().map(|s| s.t);
        self.samples.extend(r.samples);
        self.dropouts.extend(r.dropouts);
        if let Some(t) = t.or_else(|| self.samples.last().map(|s| s.t)) {
            self.outputs.note(t, &r.outputs);
        }
        if r.sound_device.is_some() {
            self.sound_device = r.sound_device;
        }
        if r.ram_mb > 0 {
            self.ram_mb = r.ram_mb;
        }
        self.ended |= r.last;
    }

    /// Its findings over a test `hours` long, named by `who`.
    pub fn findings(&self, who: &str, hours: f64) -> Vec<Finding> {
        let samples: Vec<crate::stats::Sample> =
            self.samples.iter().map(LocalSample::sample).collect();
        let mut f = analysis::analyse(&samples, &[], 0.0, &[], hours);
        f.extend(analysis::analyse_dropouts(
            &self.dropouts,
            hours,
            self.sound_device.as_deref(),
        ));
        for x in &mut f {
            x.text = format!("{who}: {}", x.text);
        }
        f.extend(self.outputs.findings(who));
        f
    }

    /// The report's lines: memory, processor, FFmpeg, sound, frame rates.
    pub fn lines(&self) -> Vec<String> {
        let mut l = Vec::new();
        if self.samples.is_empty() {
            return l;
        }
        let peak = self.samples.iter().map(|s| s.rss_mb).max().unwrap_or(0);
        let cpu: Vec<f32> = self.samples.iter().map(|s| s.cpu_pct).collect();
        let mean = cpu.iter().sum::<f32>() / cpu.len().max(1) as f32;
        let ffmpeg = self.samples.iter().map(|s| s.ffmpeg).max().unwrap_or(0);
        let started = self
            .samples
            .last()
            .zip(self.samples.first())
            .map_or(0, |(a, b)| a.started_total.saturating_sub(b.started_total));
        l.push(format!(
            "NewQ's memory up to {peak} MB{}, processor {mean:.0}% of a core on average, up to {ffmpeg} FFmpeg at once ({started} started)",
            if self.ram_mb > 0 {
                format!(" of {:.0} GB", self.ram_mb as f64 / 1024.0)
            } else {
                String::new()
            }
        ));
        l.push(format!(
            "Sound: {}, {} dropout{}",
            self.sound_device.as_deref().unwrap_or("no working device"),
            self.dropouts.len(),
            if self.dropouts.len() == 1 { "" } else { "s" }
        ));
        l.extend(self.outputs.summary());
        l
    }
}

/// One computer taking part.
pub struct Computer {
    pub id: NodeId,
    pub notes: ComputerNotes,
    pub local: LocalNotes,
    /// Shared time of the last report (None: none yet).
    pub last_heard: Option<f64>,
    watched: Watched,
    pub media: MediaState,
    pub log_tail: Option<String>,
    finding_log: FindingLog,
}

impl Computer {
    fn new(id: NodeId, name: &str) -> Computer {
        Computer {
            id,
            notes: ComputerNotes::new(name),
            local: LocalNotes::default(),
            last_heard: None,
            watched: Watched::default(),
            media: MediaState::Waiting,
            log_tail: None,
            finding_log: FindingLog::default(),
        }
    }

    /// Its verdict over a test `secs` long: how it kept to this computer and
    /// how it coped.
    pub fn verdict(&self, secs: f64) -> ComputerVerdict {
        let mut v = net_judge::judge_computer(&self.notes, secs);
        let name = &self.notes.name;
        let had = v.verdict;
        let mut own = self.local.findings(name, secs / 3600.0);
        if self.notes.reports > 0 && self.local.reports == 0 {
            own.push(Finding {
                severity: Severity::Problem,
                text: format!(
                    "No measurements of its own came back from {name} (memory, processor, frame rates): it was not measured, so it can't be said to have coped."
                ),
            });
        }
        v.lines.extend(self.local.lines());
        v.findings.extend(own);
        v.verdict = Verdict::of(&v.findings);
        if v.verdict == Verdict::ProblemsFound
            && had != Verdict::ProblemsFound
            && let Some(f) = v.findings.iter().find(|f| f.severity == Severity::Problem)
        {
            v.headline = f.text.trim_start_matches(&format!("{name}: ")).to_string();
        }
        v
    }
}

// ----- The computers together ------------------------------------------------------

/// The computers of a test across the show network.
pub struct Fleet {
    pub run: u64,
    pub computers: BTreeMap<NodeId, Computer>,
    /// The files the test plays (as the show names them) and their sizes.
    pub files: Vec<(String, u64)>,
    /// The media version served when the test began (the test's is the next).
    before: Option<String>,
    version_seen: Option<Instant>,
    /// The shared time of the first look, and when every computer had all
    /// the media (for the in-step wait).
    first_look: Option<f64>,
    all_here: Option<Instant>,
    /// Since when each computer that isn't connected has been away.
    away_since: HashMap<NodeId, Instant>,
    /// When each computer was last told to measure (shared time).
    told: HashMap<NodeId, f64>,
    /// The shared time playing began, once it has.
    pub start: Option<f64>,
    /// The shared time it was told it is over.
    pub ended: Option<f64>,
    pub muted: bool,
    /// For the computers: what the app sends (`TestMode::net_out`).
    pub out: Vec<(Option<NodeId>, NetTest)>,
    /// Lines for `events.log`, with the seconds into the test.
    pub said: Vec<(f64, String)>,
    began: Instant,
    last_judged: Option<f64>,
}

impl Fleet {
    /// The computers of the network connected now (`st.net`) take part.
    pub fn new(st: &UiState, files: Vec<(String, u64)>, muted: bool) -> Fleet {
        let computers = st
            .net
            .peers
            .iter()
            .filter(|p| p.ours && p.connected)
            .map(|p| {
                let mut c = Computer::new(p.id, &p.name);
                if let Some(s) = &p.status {
                    c.notes.link = s.link;
                    c.notes.refresh_hz = s
                        .monitors
                        .iter()
                        .filter_map(|m| m.refresh_hz)
                        .reduce(f32::max);
                }
                (p.id, c)
            })
            .collect();
        Fleet {
            run: crate::net::node::random_u64(),
            computers,
            files,
            before: st.media.current.clone(),
            version_seen: None,
            first_look: None,
            all_here: None,
            away_since: HashMap::new(),
            told: HashMap::new(),
            start: None,
            ended: None,
            muted,
            out: Vec::new(),
            said: Vec::new(),
            began: Instant::now(),
            last_judged: None,
        }
    }

    fn say(&mut self, text: String) {
        let t = self.began.elapsed().as_secs_f64();
        log::info!("test across the network: {text}");
        self.said.push((t, text));
    }

    /// The names of the computers taking part: "Left, Right".
    pub fn names(&self) -> String {
        self.computers
            .values()
            .map(|c| c.notes.name.clone())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Seconds played at shared time `now` (0 before playing).
    pub fn played(&self, now: f64) -> f64 {
        self.start.map_or(0.0, |s| (now - s).max(0.0))
    }

    /// Every computer has the test's files and the test show, and is in step.
    pub fn media_ready(&mut self, st: &UiState) -> bool {
        let m: &MediaView = &st.media;
        let current = m
            .current
            .as_deref()
            .filter(|c| Some(*c) != self.before.as_deref());
        let hashing = m.hashing.is_some();
        let now = Instant::now();
        if current.is_some() && !hashing && self.version_seen.is_none() {
            self.version_seen = Some(now);
        }
        let version_seen = self.version_seen.unwrap_or(now);
        let total: u64 = self.files.iter().map(|f| f.1).sum();
        let mut all = !self.computers.is_empty();
        let mut here = !self.computers.is_empty();
        for c in self.computers.values_mut() {
            let p = st.net.peers.iter().find(|p| p.id == c.id);
            let report = p
                .and_then(|p| p.status.as_ref())
                .and_then(|s| s.media_progress.as_deref());
            let state = if hashing {
                MediaState::Waiting
            } else {
                media_state(report, current, &self.files)
            };
            if c.watched.first.is_none() && state != MediaState::Waiting {
                let left = match state {
                    MediaState::Coming { left, .. } => left,
                    _ => 0,
                };
                c.watched.first = Some((now, left));
            }
            if state == MediaState::Here && c.watched.done.is_none() {
                c.watched.done = Some(now);
                let (t0, left) = c.watched.first.unwrap_or((now, 0));
                c.notes.transfer = Some(if left == 0 {
                    Transfer {
                        bytes: total,
                        secs: now.duration_since(version_seen).as_secs_f64().max(0.1),
                        within: true,
                    }
                } else {
                    Transfer {
                        bytes: left,
                        secs: now.duration_since(t0).as_secs_f64().max(0.1),
                        within: false,
                    }
                });
            }
            c.media = state;
            if p.is_some_and(|p| p.connected) {
                self.away_since.remove(&c.id);
            } else {
                self.away_since.entry(c.id).or_insert(now);
            }
            let in_step = p
                .and_then(|p| p.status.as_ref())
                .is_some_and(|s| s.in_step && p.is_some_and(|p| p.connected));
            here &= state == MediaState::Here;
            all &= state == MediaState::Here && in_step;
        }
        if here && self.all_here.is_none() {
            self.all_here = Some(now);
        }
        all
    }

    /// Why the wait for the computers should end, if it should: what was
    /// waited for, and for how long (a plain sentence).
    pub fn waited_too_long(&self, st: &UiState, since: Instant) -> Option<String> {
        let gone: Vec<String> = self
            .away_since
            .iter()
            .filter(|(_, t)| t.elapsed() > GONE_LIMIT)
            .filter_map(|(id, _)| self.computers.get(id))
            .map(|c| c.notes.name.clone())
            .collect();
        if !gone.is_empty() {
            return Some(format!(
                "{} stopped answering while the test waited for it, and was still away after {} s",
                gone.join(", "),
                GONE_LIMIT.as_secs()
            ));
        }
        if let Some(t) = self.all_here
            && t.elapsed() > STEP_LIMIT
        {
            let out: Vec<String> = self
                .computers
                .values()
                .filter(|c| {
                    !st.net.peers.iter().any(|p| {
                        p.id == c.id && p.connected && p.status.as_ref().is_some_and(|s| s.in_step)
                    })
                })
                .map(|c| c.notes.name.clone())
                .collect();
            return Some(format!(
                "{} had the test's media but {} not in step with this computer after {} s (the Show network page says why)",
                out.join(", "),
                if out.len() == 1 { "was" } else { "were" },
                STEP_LIMIT.as_secs()
            ));
        }
        if since.elapsed() > MEDIA_LIMIT {
            let lacking: Vec<String> = self
                .computers
                .values()
                .filter(|c| c.media != MediaState::Here)
                .map(|c| c.notes.name.clone())
                .collect();
            return Some(format!(
                "the test's media hadn't reached {} after {} minutes",
                lacking.join(", "),
                MEDIA_LIMIT.as_secs() / 60
            ));
        }
        None
    }

    /// The first look's shared time (a moment for the show to reach the
    /// computers before the media is judged).
    pub fn first_look(&mut self, now: f64) -> f64 {
        *self.first_look.get_or_insert(now)
    }

    /// The window's line per computer while the media is on its way.
    pub fn media_lines(&self) -> Vec<String> {
        self.computers
            .values()
            .map(|c| match c.media {
                MediaState::Waiting => format!("{}: getting the list of media", c.notes.name),
                MediaState::Coming { left, total } => {
                    let done = total.saturating_sub(left);
                    format!(
                        "{}: {:.0} % ({} of {})",
                        c.notes.name,
                        done as f64 * 100.0 / total.max(1) as f64,
                        megabytes(done),
                        megabytes(total)
                    )
                }
                MediaState::Here => match &c.notes.transfer {
                    Some(t) => format!(
                        "{}: all here, {}",
                        c.notes.name,
                        net_judge::transfer_words(t)
                    ),
                    None => format!("{}: all here", c.notes.name),
                },
            })
            .collect()
    }

    /// Each computer's transfer, in a line.
    pub fn transfers_line(&self) -> String {
        self.computers
            .values()
            .map(|c| match &c.notes.transfer {
                Some(t) => format!("{}: {}", c.notes.name, net_judge::transfer_words(t)),
                None => format!("{}: already there", c.notes.name),
            })
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// Playing begins at shared time `now`: every computer measures.
    pub fn begin(&mut self, now: f64) {
        self.start = Some(now);
        let line = self.transfers_line();
        self.say(format!("All here. {line}"));
        self.tell_all(now);
        for id in self.computers.keys() {
            self.told.insert(*id, now);
        }
        self.say("The computers are measuring; the test plays".into());
    }

    fn tell_all(&mut self, now: f64) {
        let run = self.run;
        self.out.push((
            None,
            NetTest::Begin {
                run,
                start: self.start.unwrap_or(now),
            },
        ));
        if self.muted {
            self.out.push((None, NetTest::Mute { run, muted: true }));
        }
    }

    /// Each look while playing: a computer that came back (or joined) is
    /// told again; the findings so far are logged as they appear and go.
    pub fn keep_up(&mut self, st: &UiState, now: f64) {
        if self.start.is_none() || self.ended.is_some() {
            return;
        }
        let run = self.run;
        let begin = NetTest::Begin {
            run,
            start: self.start.unwrap_or(now),
        };
        for p in st.net.peers.iter().filter(|p| p.ours && p.connected) {
            let told = self.told.get(&p.id).copied();
            let quiet = self
                .computers
                .get(&p.id)
                .and_then(|c| c.last_heard)
                .is_none_or(|h| now - h > AWAY_SECS);
            if quiet && told.is_none_or(|t| now - t > AWAY_SECS) {
                self.told.insert(p.id, now);
                self.out.push((Some(p.id), begin.clone()));
                if self.muted {
                    self.out
                        .push((Some(p.id), NetTest::Mute { run, muted: true }));
                }
            }
        }
        // The findings as they stand, every few seconds, into the log.
        if self.last_judged.is_none_or(|t| now - t >= 5.0) {
            self.last_judged = Some(now);
            let played = self.played(now);
            let mut lines = Vec::new();
            for c in self.computers.values_mut() {
                let f = c.local.findings(&c.notes.name, played / 3600.0);
                for l in c.finding_log.update(played, &f) {
                    lines.push(l);
                }
            }
            for l in lines {
                self.say(l);
            }
        }
    }

    /// A message from a computer, at shared time `now`.
    pub fn take(&mut self, from: NodeId, m: NetTest, now: f64, net: &NetView) {
        let t = self.played(now);
        let start = self.start;
        let c = self
            .computers
            .entry(from)
            .or_insert_with(|| Computer::new(from, &net.name_of(from)));
        let mut lines = Vec::new();
        match m {
            NetTest::Report(r) if r.run == self.run => {
                if let Some(h) = c.last_heard {
                    let gap = now - h;
                    if gap > net_judge::GAP_SECS {
                        let at = start.map_or(0.0, |s| (h - s).max(0.0));
                        c.notes.gaps.push((at, gap));
                    }
                }
                c.last_heard = Some(now);
                lines.extend(
                    r.commands
                        .iter()
                        .filter(|k| k.lead_ms < 0.0)
                        .map(|k| {
                            format!(
                                "{}: command {} arrived {:.1} ms late",
                                c.notes.name, k.seq, -k.lead_ms
                            )
                        })
                        .chain(
                            r.corrections
                                .iter()
                                .map(|k| format!("{}: put right: {}", c.notes.name, k.what)),
                        )
                        .chain(r.stalls.iter().map(|s| {
                            format!(
                                "{}: its main loop stalled for {} ms ({})",
                                c.notes.name, s.ms, s.part
                            )
                        })),
                );
                c.notes.take(*r);
            }
            NetTest::Local(r) if r.run == self.run => {
                c.last_heard = Some(c.last_heard.map_or(now, |h| h.max(now)));
                for d in &r.dropouts {
                    lines.push(format!(
                        "{}: the sound device reported a dropout at {}",
                        c.notes.name,
                        net_judge::at(*d)
                    ));
                }
                c.local.take(*r);
            }
            NetTest::LogTail { run, text } if run == self.run => {
                c.notes.log_tail = true;
                c.log_tail = Some(text);
            }
            _ => {}
        }
        for l in lines {
            self.said.push((t, l));
        }
    }

    /// The test is over at shared time `now`: the computers send their last.
    pub fn end(&mut self, now: f64) {
        if self.ended.is_some() {
            return;
        }
        self.ended = Some(now);
        let run = self.run;
        self.out.push((None, NetTest::End { run }));
        if self.muted {
            self.out.push((None, NetTest::Mute { run, muted: false }));
        }
        self.say("The test is over; waiting for the computers' last reports".into());
    }

    /// Every computer still connected has sent its last report and log, or
    /// they have been waited for long enough.
    pub fn all_in(&self, st: &UiState, now: f64) -> bool {
        let all = self.computers.iter().all(|(id, c)| {
            (c.notes.ended && c.log_tail.is_some())
                || !st.net.peers.iter().any(|p| p.id == *id && p.connected)
        });
        all || self.ended.is_some_and(|e| now - e > ENDING_LIMIT)
    }

    /// A computer silent at the end dropped out then.
    pub fn close_gaps(&mut self, now: f64) {
        let Some(start) = self.start else {
            return;
        };
        for c in self.computers.values_mut() {
            if let Some(h) = c.last_heard
                && !c.notes.ended
                && now - h > net_judge::GAP_SECS
            {
                c.notes.gaps.push(((h - start).max(0.0), now - h));
            }
        }
    }

    /// The window's line per computer while it plays: in step or not, each
    /// output's frame rate, and anything wrong (warn: something is).
    pub fn live_lines(&self, st: &UiState, now: f64) -> Vec<(String, bool)> {
        self.computers
            .values()
            .map(|c| {
                let away = c.last_heard.is_none_or(|h| now - h > AWAY_SECS);
                let step = st
                    .net
                    .peer(c.id)
                    .and_then(|p| p.status.as_ref())
                    .is_some_and(|s| s.in_step);
                let mut wrong = Vec::new();
                if away && self.played(now) > AWAY_SECS {
                    wrong.push("not heard from".to_string());
                }
                let late = c.notes.commands.iter().filter(|k| k.lead_ms < 0.0).count();
                if late > 0 {
                    wrong.push(format!("{late} commands late"));
                }
                for (name, fps) in c.local.outputs.now() {
                    if fps < STUTTER_FPS {
                        wrong.push(format!("{name} stutters"));
                    }
                }
                if !c.local.dropouts.is_empty() {
                    wrong.push(format!("{} sound dropouts", c.local.dropouts.len()));
                }
                if let Some(s) = c.notes.stalls.iter().map(|s| s.ms).max() {
                    wrong.push(format!("held up {s} ms"));
                }
                let frame = c.notes.frame_ms();
                let apart = c
                    .notes
                    .rows
                    .iter()
                    .rev()
                    .find_map(|r| r.apart_ms)
                    .map_or(String::new(), |a| {
                        format!(", {:.1} frames apart", a / frame)
                    });
                let text = format!(
                    "{}: {}{apart} · {}{}",
                    c.notes.name,
                    if step { "in step" } else { "NOT in step" },
                    c.local.outputs.now_text(),
                    if wrong.is_empty() {
                        String::new()
                    } else {
                        format!(" · {}", wrong.join(", "))
                    }
                );
                (text, !step || !wrong.is_empty())
            })
            .collect()
    }

    /// Each computer's verdict over a test `secs` long.
    pub fn verdicts(&self, secs: f64) -> Vec<ComputerVerdict> {
        self.computers.values().map(|c| c.verdict(secs)).collect()
    }

    /// Findings that were shown along the way and had gone by the end.
    pub fn gone(&self, finals: &[Finding]) -> Vec<Finding> {
        self.computers
            .values()
            .flat_map(|c| c.finding_log.gone(finals))
            .collect()
    }

    /// The report's part for the computers.
    pub fn report(&self, verdicts: &[ComputerVerdict]) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "The other computers");
        let _ = writeln!(s, "-------------------");
        let _ = writeln!(
            s,
            "  The test played on {} as well, each with its own engine in step with this computer. Media: {}.",
            self.names(),
            self.transfers_line()
        );
        let _ = writeln!(
            s,
            "  What the test adds on each computer: the displays ticked for it (each its own window; with several, the others show the first one's picture), a look at memory and processor every {} s on a thread of its own, and a short report over the network every second. Nothing is drawn on the outputs for the test, so what they show is what a show would.",
            crate::net::measure_local::ROW_EVERY
        );
        for v in verdicts {
            let _ = writeln!(s);
            let _ = writeln!(s, "  {} — {}", v.name, v.verdict.label());
            let _ = writeln!(s, "    {}", v.headline);
            for l in &v.lines {
                let _ = writeln!(s, "    {l}");
            }
            for f in &v.findings {
                let tag = match f.severity {
                    Severity::Problem => "PROBLEM",
                    Severity::Note => "note   ",
                    Severity::Watch => "tracked",
                };
                let _ = writeln!(s, "    {tag}  {}", f.text);
            }
        }
        let _ = writeln!(s);
        let _ = writeln!(s, "  {}", net_judge::CANNOT_SEE);
        s
    }

    /// The files of the computers' part: network.csv, computers.csv,
    /// outputs.csv, computers.txt and each computer's log.
    pub fn write(&self, folder: &std::path::Path, net: &NetView) {
        let notes: Vec<ComputerNotes> = self.computers.values().map(|c| c.notes.clone()).collect();
        let _ = std::fs::write(folder.join("network.csv"), net_judge::csv(&notes));
        let _ = std::fs::write(folder.join("computers.csv"), self.computers_csv());
        let _ = std::fs::write(folder.join("outputs.csv"), self.outputs_csv());
        let _ = std::fs::write(folder.join("computers.txt"), computers_text(net));
        for c in self.computers.values() {
            if let Some(t) = &c.log_tail {
                let _ = std::fs::write(
                    folder.join(format!("computer-{}-newq.log", file_name(&c.notes.name))),
                    t,
                );
            }
        }
    }

    fn computers_csv(&self) -> String {
        let mut s = String::from(
            "computer,t_s,rss_mb,cpu_pct,ffmpeg,ffmpeg_cpu_pct,playing,started_total,layers,late_permille,audio_permille,audio_errors,stall_ms,gpu_mb\n",
        );
        for c in self.computers.values() {
            for x in &c.local.samples {
                let _ = writeln!(
                    s,
                    "{},{:.2},{},{:.1},{},{:.1},{},{},{},{},{},{},{},{}",
                    csv_field(&c.notes.name),
                    x.t,
                    x.rss_mb,
                    x.cpu_pct,
                    x.ffmpeg,
                    x.ffmpeg_cpu_pct,
                    x.playing,
                    x.started_total,
                    x.layers,
                    x.late_permille,
                    x.audio_permille,
                    x.audio_errors,
                    x.stall_ms,
                    x.gpu_mb.map_or(String::new(), |g| g.to_string())
                );
            }
        }
        s
    }

    fn outputs_csv(&self) -> String {
        let mut s = String::from("computer,output,t_s,fps,refresh_hz\n");
        for c in self.computers.values() {
            for (name, v) in &c.local.outputs.readings {
                for (t, f, r) in v {
                    let _ = writeln!(
                        s,
                        "{},{},{t:.2},{f:.1},{}",
                        csv_field(&c.notes.name),
                        csv_field(name),
                        r.map_or(String::new(), |r| format!("{r:.2}"))
                    );
                }
            }
        }
        s
    }
}

/// At the end of a test across a show network (`fleet` with the shared time
/// it ended): each computer's verdict, its findings added to the test's (`findings`),
/// the computers' files written to `folder`, and the report's part for them.
/// Nothing alone.
pub fn fold_in(
    fleet: Option<(Box<Fleet>, f64)>,
    findings: &mut Vec<Finding>,
    folder: &std::path::Path,
    net: &NetView,
) -> (Vec<ComputerVerdict>, String) {
    let Some((mut f, now)) = fleet else {
        return (Vec::new(), String::new());
    };
    f.close_gaps(now);
    let secs = f.played(now);
    let verdicts = f.verdicts(secs);
    for v in &verdicts {
        findings.extend(v.findings.iter().cloned());
    }
    let gone = f.gone(findings);
    findings.extend(gone);
    f.write(folder, net);
    let mut part = String::from("\n");
    part.push_str(&f.report(&verdicts));
    (verdicts, part)
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn file_name(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

pub fn megabytes(b: u64) -> String {
    format!("{:.0} MB", b as f64 / 1_000_000.0)
}

/// The computers as the primary sees them, for `computers.txt`: each one's
/// address, the way it is reached (wired, Wi-Fi, a VPN), its clock and
/// monitors.
pub fn computers_text(v: &NetView) -> String {
    let mut s = format!(
        "This computer: {} (NewQ {})\n",
        v.name,
        crate::net::proto::VERSION
    );
    for a in crate::ui::own_addresses(&v.adapters, v.port) {
        s.push_str(&format!("  address: {a}\n"));
    }
    if crate::net::paths::vpn_on(&v.adapters) {
        s.push_str("  a VPN is on on this computer\n");
    }
    s.push('\n');
    for p in v.peers.iter().filter(|p| p.ours) {
        s.push_str(&format!(
            "{}: {}, NewQ {}, {}\n",
            p.name,
            p.role.described(),
            p.version,
            if p.connected {
                "connected"
            } else {
                "not connected"
            }
        ));
        if let Some((a, last)) = crate::ui::peer_address(v, p.id) {
            s.push_str(&format!(
                "  address {a}{}{}\n",
                if last { " (last known)" } else { "" },
                crate::ui::path_to(v, p.id)
                    .map_or(String::new(), |k| format!(", reached over {}", k.label()))
            ));
        }
        if let Some(st) = &p.status {
            s.push_str(&format!(
                "  link {}, {}clock ±{} ms, round trip {} ms, {}\n",
                st.path.map_or_else(
                    || match st.link.label() {
                        "" => "unknown",
                        l => l,
                    }
                    .to_string(),
                    |k| k.label().to_string()
                ),
                if st.vpn_on { "a VPN is on there, " } else { "" },
                st.clock_error_ms.map_or("?".into(), |e| format!("{e:.2}")),
                st.rtt_ms.map_or("?".into(), |e| format!("{e:.2}")),
                st.standing.clone().unwrap_or_default()
            ));
            for (i, m) in st.monitors.iter().enumerate() {
                s.push_str(&format!(
                    "  monitor {}: {} {}x{} {}\n",
                    i + 1,
                    m.identity,
                    m.size.0,
                    m.size.1,
                    m.refresh_hz.map_or("?".into(), |r| format!("{r:.2} Hz"))
                ));
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate(name: &str, fps: f32) -> OutputRate {
        OutputRate::new(name, fps, Some(60.0))
    }

    #[test]
    fn an_output_that_draws_a_few_frames_a_second_is_a_problem_and_a_moment_isnt() {
        let mut w = OutputWatch::default();
        for t in 0..20 {
            let t = f64::from(t);
            // Display 2 stutters from 5 s to 9 s; Display 1 dips once.
            let two = if (5.0..10.0).contains(&t) { 4.0 } else { 59.0 };
            let one = if t == 12.0 { 10.0 } else { 60.0 };
            w.note(t, &[rate("Display 1", one), rate("Display 2", two)]);
            // A second reading within the second isn't kept.
            w.note(t + 0.5, &[rate("Display 1", 1.0)]);
        }
        let f = w.findings("Left");
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].severity, Severity::Problem);
        assert!(
            f[0].text.starts_with("Left: the output Display 2 drew only about 4 frames a second for 4 s (from 0:05 into the test); its display runs at 60 Hz"),
            "{}",
            f[0].text
        );
        assert_eq!(w.now_text(), "Display 1 60 fps, Display 2 59 fps");
        assert!(w.summary()[1].contains("lowest 4"), "{:?}", w.summary());
    }

    #[test]
    fn a_computer_whose_own_measurements_never_came_is_a_problem() {
        let mut c = Computer::new(NodeId(2), "Left");
        // It reported how it keeps to the primary, but nothing of itself.
        c.notes.take(crate::net::measure::Report {
            run: 1,
            seq: 1,
            ..Default::default()
        });
        let v = c.verdict(60.0);
        assert_eq!(v.verdict, Verdict::ProblemsFound);
        assert!(
            v.findings.iter().any(|f| f
                .text
                .starts_with("No measurements of its own came back from Left")),
            "{:?}",
            v.findings
        );
        // Nothing at all: the network test's own words.
        let none = Computer::new(NodeId(3), "Right").verdict(60.0);
        assert_eq!(none.verdict, Verdict::ProblemsFound);
        assert!(none.headline.contains("No measurements came from Right"));
    }

    #[test]
    fn a_computers_own_rows_are_judged_with_test_modes_findings_named_by_it() {
        let mut c = Computer::new(NodeId(2), "Left");
        c.notes.take(crate::net::measure::Report {
            run: 1,
            seq: 1,
            ..Default::default()
        });
        let row = |t: f64, stall: u64| LocalSample {
            t,
            rss_mb: 400,
            cpu_pct: 30.0,
            stall_ms: stall,
            ..Default::default()
        };
        c.local.take(LocalReport {
            run: 1,
            seq: 1,
            samples: vec![row(1.0, 20), row(3.0, 1500)],
            dropouts: vec![2.0, 2.1, 2.2],
            outputs: vec![rate("Left 1", 3.0)],
            sound_device: Some("Speakers".into()),
            ram_mb: 16384,
            last: false,
        });
        c.local.take(LocalReport {
            run: 1,
            seq: 2,
            samples: vec![row(5.0, 10)],
            outputs: vec![rate("Left 1", 2.0)],
            last: true,
            ..Default::default()
        });
        let v = c.verdict(60.0);
        let text: Vec<&str> = v.findings.iter().map(|f| f.text.as_str()).collect();
        assert!(
            text.iter()
                .any(|t| t.starts_with("Left: The program froze")),
            "{text:?}"
        );
        assert!(
            text.iter()
                .any(|t| t.starts_with("Left: ") && t.contains("dropout")),
            "{text:?}"
        );
        assert!(
            text.iter()
                .any(|t| t.contains("the output Left 1 drew only"))
        );
        assert_eq!(v.verdict, Verdict::ProblemsFound);
        assert!(
            v.lines
                .iter()
                .any(|l| l.contains("memory up to 400 MB of 16 GB")),
            "{:?}",
            v.lines
        );
        assert!(v.lines.iter().any(|l| l == "Sound: Speakers, 3 dropouts"));
    }

    #[test]
    fn waiting_for_the_computers_ends_with_what_was_waited_for_and_how_long() {
        let mut st = UiState::default();
        let mut v = NetView::default();
        v.peers.push(crate::net::hub::PeerView {
            id: NodeId(2),
            name: "Left".into(),
            ours: true,
            connected: true,
            ..Default::default()
        });
        st.net = std::sync::Arc::new(v);
        let mut f = Fleet::new(&st, vec![("/m/a.mp4".into(), 10)], false);
        let now = Instant::now();
        assert!(f.waited_too_long(&st, now).is_none());
        let long_ago = |d: Duration| now.checked_sub(d).unwrap_or(now);
        // The media never came.
        let why = f
            .waited_too_long(&st, long_ago(MEDIA_LIMIT + Duration::from_secs(1)))
            .unwrap_or_default();
        if !why.is_empty() {
            assert_eq!(why, "the test's media hadn't reached Left after 20 minutes");
        }
        // All here, never in step.
        f.all_here = Some(long_ago(STEP_LIMIT + Duration::from_secs(1)));
        if f.all_here != Some(now) {
            assert_eq!(
                f.waited_too_long(&st, now).as_deref(),
                Some(
                    "Left had the test's media but was not in step with this computer after 90 s (the Show network page says why)"
                )
            );
        }
        // Gone quiet.
        f.away_since
            .insert(NodeId(2), long_ago(GONE_LIMIT + Duration::from_secs(1)));
        if f.away_since[&NodeId(2)] != now {
            assert!(
                f.waited_too_long(&st, now)
                    .unwrap()
                    .starts_with("Left stopped answering while the test waited for it")
            );
        }
        // Once connected again (seen by `media_ready`) it waits on.
        assert!(!f.media_ready(&st));
        assert!(!f.away_since.contains_key(&NodeId(2)));
    }

    #[test]
    fn the_files_a_test_waits_for_are_those_it_plays() {
        let mut show = Show::default();
        let a = show.make_cue(crate::show::CueType::Video, Some("/m/a.mp4".into()), 0);
        let mut b = show.make_cue(crate::show::CueType::Video, Some("/m/kept.mp4".into()), 1);
        let list = show.add_list("kept");
        b.list = list;
        let again = show.make_cue(crate::show::CueType::Video, Some("/m/a.mp4".into()), 2);
        show.cues.extend([a, b, again]);
        let f = files_of(&show, Some(list));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].0, "/m/a.mp4");
        assert_eq!(files_of(&show, None).len(), 2);
    }
}
