//! The full check as Test Mode's window runs it: `feature`'s parts played
//! through the app's own engine, windows and graphics, with the parts only
//! the app can do (a recording of a display, a stream to a server NewQ runs
//! on this computer alone), the measurements the other tests take, and at the
//! end one zip of everything (`bundle`).
//!
//! The user's show is put aside for the whole of it and put back whatever
//! happens, as for the other tests (`UiState::begin_test`/`end_test`).

use super::analysis::{Finding, Severity, Verdict, analyse};
use super::bundle::Bundle;
use super::feature::{self, Ask, Check, Media, Runner};
use super::plan::{DisplayChoice, SoundChoice};
use super::session::{Done, End, MemoryWatch, clock};
use crate::awake::{KeepAwake, SleepWatch};
use crate::engine::Engine;
use crate::media::ffmpeg::Ffmpeg;
use crate::show::{Show, StreamKind, StreamOut};
use crate::stats::{Inputs, Sample, StatsLog};
use crate::ui::{TestStash, UiState};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

/// The check's media being made (FFmpeg, on a thread).
pub struct FullPrep {
    media: Receiver<Result<Media, String>>,
    cancel: Arc<AtomicBool>,
    pub folder: PathBuf,
    log_dir: PathBuf,
    displays: Vec<DisplayChoice>,
    sound: SoundChoice,
    pub since: Instant,
}

impl FullPrep {
    /// Begin: make the run's folder and its media.
    pub fn start(
        ff: Ffmpeg,
        log_dir: &Path,
        displays: Vec<DisplayChoice>,
        sound: SoundChoice,
    ) -> Result<FullPrep, String> {
        let folder = super::session::run_folder_named(log_dir, "full", super::session::unix_now());
        std::fs::create_dir_all(&folder).map_err(|e| {
            format!(
                "Can't make the folder for the test's files ({}): {e}. Nothing was changed.",
                folder.display()
            )
        })?;
        let (tx, media) = std::sync::mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let (dir, stop) = (folder.join("media"), cancel.clone());
        std::thread::Builder::new()
            .name("test-media".into())
            .spawn(move || {
                let _ = tx.send(feature::make_media(&ff, &dir, &stop));
            })
            .map_err(|e| {
                format!("Can't start making the test's media: {e}. Nothing was changed.")
            })?;
        Ok(FullPrep {
            media,
            cancel,
            folder,
            log_dir: log_dir.to_path_buf(),
            displays,
            sound,
            since: Instant::now(),
        })
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn log_dir(&self) -> &Path {
        &self.log_dir
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// The media, once it is made (or why it couldn't be).
    pub fn ready(&self) -> Option<Result<Media, String>> {
        match self.media.try_recv() {
            Ok(m) => Some(m),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Some(Err("making the media stopped unexpectedly".into()))
            }
        }
    }
}

/// The recording of the "Send out" part, from switched on to looked at.
enum Rec {
    Idle,
    On {
        dir: PathBuf,
    },
    /// Switched off: FFmpeg is closing the file.
    Closing {
        dir: PathBuf,
        since: Instant,
        size: u64,
        steady: Instant,
    },
    Looking(Receiver<feature::Verdict>),
    Done,
}

/// The server and the stream to it, while that part runs.
struct Serve {
    since: Instant,
    port: u16,
    path: String,
    saw_running: bool,
    status: String,
    reached: bool,
    asking: Option<Receiver<bool>>,
    asked: Instant,
}

/// The full check under way.
pub struct FullRun {
    pub started: Instant,
    pub folder: PathBuf,
    log_dir: PathBuf,
    pub runner: Runner,
    stash: Option<TestStash>,
    stats: StatsLog,
    pub history: Vec<Sample>,
    pub last: Option<Sample>,
    events: Option<std::io::BufWriter<std::fs::File>>,
    pub stop: bool,
    awake: KeepAwake,
    memory: MemoryWatch,
    sleep_watch: SleepWatch,
    sleeps: Vec<(f64, f64)>,
    media_notes: Vec<String>,
    /// How long it should take, for the window's bar.
    pub planned: f64,
    rec: Rec,
    serve: Option<Serve>,
    /// What the engine said of streams, for the recording's and the server's verdicts.
    stream_said: Vec<String>,
    /// When the last part ended, while a recording is still being looked at.
    settling: Option<Instant>,
    panics_before: u32,
    ff: Option<Ffmpeg>,
}

/// Whether a streaming server can be run for the check: the program is with
/// this copy of NewQ, and this computer has been allowed to run one.
fn hosting_possible(st: &UiState) -> Result<(), String> {
    if crate::mediamtx::locate().is_none() {
        return Err(
            "not run: the streaming server (MediaMTX) is not with this copy of NewQ".into(),
        );
    }
    if !st.prefs.allow_hosting {
        return Err(
            "not run: this computer has not been allowed to run a streaming server (Settings ▸ Media), and a test changes no settings"
                .into(),
        );
    }
    Ok(())
}

/// A port nothing is listening on, on this computer.
fn free_port() -> Option<u16> {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .ok()
}

/// Ask the server on this computer whether a stream is being published at `path`.
fn rtsp_has_stream(port: u16, path: &str) -> bool {
    use std::io::Read;
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut s) = std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(500)) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
    let ask = format!(
        "DESCRIBE rtsp://127.0.0.1:{port}/{path} RTSP/1.0\r\nCSeq: 1\r\nAccept: application/sdp\r\n\r\n"
    );
    if s.write_all(ask.as_bytes()).is_err() {
        return false;
    }
    let mut buf = [0u8; 64];
    let n = s.read(&mut buf).unwrap_or(0);
    rtsp_ok(&buf[..n])
}

/// Whether an RTSP answer says the stream is there.
fn rtsp_ok(answer: &[u8]) -> bool {
    String::from_utf8_lossy(answer)
        .lines()
        .next()
        .is_some_and(|l| l.starts_with("RTSP/1.0 200"))
}

/// The newest file in a folder, and its size.
fn newest_file(dir: &Path) -> Option<(PathBuf, u64)> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            meta.is_file()
                .then(|| (meta.modified().ok(), e.path(), meta.len()))
        })
        .max_by_key(|(when, _, _)| *when)
        .map(|(_, path, len)| (path, len))
}

/// What a recording that has been closed came to.
fn judge_recording(ff: Option<&Ffmpeg>, dir: &Path, said: &[String]) -> feature::Verdict {
    use feature::Verdict::{Failed, NotChecked, Ok as Fine};
    let complaint = said.last().cloned();
    let Some((path, size)) = newest_file(dir) else {
        return match complaint {
            // No encoder that works is this computer's FFmpeg, not a fault in the recording.
            Some(why) if why.to_lowercase().contains("encoder") => NotChecked(format!(
                "the FFmpeg in use has no H.264 encoder that works on this computer ({why})"
            )),
            Some(why) => Failed(format!("no recording was written: {why}")),
            None => Failed(format!("no recording was written to {}", dir.display())),
        };
    };
    let Some(ff) = ff else {
        return NotChecked("FFmpeg was not there to read the recording back".into());
    };
    let file = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    match ff.probe(&path) {
        Ok(info) => match (&info.video, info.duration) {
            (Some(v), Some(d)) if d >= 2.0 => Fine(format!(
                "a recording of {d:.1} s ({}x{}, {} kB) was written and FFmpeg reads it back: recording/{file}",
                v.width,
                v.height,
                size / 1024
            )),
            (Some(_), d) => Failed(format!(
                "the recording is only {:.1} s long (about 6 s were recorded): recording/{file}",
                d.unwrap_or(0.0)
            )),
            (None, _) => Failed(format!(
                "the recording has no picture in it: recording/{file}"
            )),
        },
        Err(e) => Failed(format!(
            "the recording can't be read back ({e:#}): recording/{file}"
        )),
    }
}

/// Start the check: put the user's show aside and the check's in its place.
pub fn launch(
    prep: FullPrep,
    media: Media,
    show: &mut Show,
    engine: &mut Engine,
    st: &mut UiState,
) -> Result<FullRun, String> {
    if let Some(why) = st.test_refusal() {
        return Err(why.to_string());
    }
    // The displays and sound chosen in the window, as the other tests use them.
    let mut base = show.clone();
    super::plan::use_displays(&mut base, &prep.displays);
    super::plan::use_sound(&mut base, &prep.sound);
    let plan = feature::build(&base, &media, hosting_possible(st));
    let folder = prep.folder.clone();
    let stats = StatsLog::create(
        &folder.join("stats.csv"),
        Duration::from_secs(2),
        prep.log_dir.join("newq.log"),
    )
    .map_err(|e| {
        format!(
            "Can't write the test's log in {}: {e}. Nothing was changed.",
            folder.display()
        )
    })?;
    let events = std::fs::File::create(folder.join("events.log"))
        .ok()
        .map(std::io::BufWriter::new);
    let _ = std::fs::write(
        folder.join("system.txt"),
        super::session::system_report(
            engine,
            st,
            show,
            &[],
            &Default::default(),
            &prep.displays,
            &prep.sound,
            false,
        ),
    );
    let env: Vec<(String, String)> = std::env::vars().collect();
    let _ = std::fs::write(
        folder.join("settings.txt"),
        super::bundle::settings_text(&st.prefs, &env),
    );
    // The show it plays, should someone want to play it again by hand.
    if let Ok(json) = serde_json::to_string_pretty(&plan.show) {
        let _ = std::fs::write(folder.join("full-check.newq"), json);
    }
    let awake = KeepAwake::new("NewQ is running a test");
    let planned = plan.secs;
    let parts = plan.parts.len();
    // The point of no return: the user's show is put aside.
    let stash = st.begin_test(show, engine, plan.show);
    crate::media::audio::set_muted(prep.sound.muted);
    engine.set_log_tap(true);
    engine.log(format!("Test mode: the sound {}", prep.sound.describe()));
    engine.log(format!(
        "Test mode: the full check begins ({parts} parts, about {:.0} s)",
        planned
    ));
    Ok(FullRun {
        started: Instant::now(),
        folder,
        log_dir: prep.log_dir,
        runner: Runner::new(plan.parts, plan.osc, true),
        stash: Some(stash),
        stats,
        history: Vec::new(),
        last: None,
        events,
        stop: false,
        awake,
        memory: MemoryWatch::new(),
        sleep_watch: SleepWatch::new(),
        sleeps: Vec::new(),
        media_notes: media.notes,
        planned,
        rec: Rec::Idle,
        serve: None,
        stream_said: Vec::new(),
        settling: None,
        panics_before: crate::crash::panic_count(),
        ff: engine.ff.clone(),
    })
}

impl FullRun {
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    pub fn progress(&self) -> f32 {
        (self.elapsed().as_secs_f32() / (self.planned as f32).max(1.0)).clamp(0.0, 1.0)
    }

    fn log_at(&mut self, t: f64, text: &str) {
        if let Some(f) = &mut self.events {
            let _ = writeln!(f, "[{t:>9.3}] {text}");
            let _ = f.flush();
        }
    }

    /// The display the app-only parts use: the first that shows a picture of its own.
    fn first_display(show: &mut Show) -> Option<&mut crate::show::OutputConfig> {
        show.outputs
            .iter_mut()
            .find(|o| o.enabled && !o.is_sub_display() && o.part_of.is_none())
    }

    /// Do what the runner asks of the app.
    fn obey(&mut self, ask: Ask, show: &mut Show, now: Instant) {
        match ask {
            Ask::RecordOn => {
                let dir = self.folder.join("recording");
                let _ = std::fs::create_dir_all(&dir);
                self.stream_said.clear();
                if let Some(o) = Self::first_display(show) {
                    o.stream = Some(StreamOut {
                        enabled: true,
                        kind: StreamKind::Record {
                            folder: dir.display().to_string(),
                            codec: Default::default(),
                            container: Default::default(),
                            kbps: 2000,
                            area: None,
                            audio: None,
                        },
                        fps: 30,
                        height: 360,
                    });
                }
                self.rec = Rec::On { dir };
            }
            Ask::RecordOff => {
                if let Some(o) = Self::first_display(show) {
                    o.stream = None;
                }
                if let Rec::On { dir } = std::mem::replace(&mut self.rec, Rec::Idle) {
                    self.rec = Rec::Closing {
                        dir,
                        since: now,
                        size: 0,
                        steady: now,
                    };
                }
            }
            Ask::ServeOn => {
                self.stream_said.clear();
                let (Some(port), Some(player)) = (free_port(), free_port()) else {
                    self.runner.set_served(
                        feature::Verdict::NotChecked(
                            "no free port could be found on this computer".into(),
                        ),
                        show,
                    );
                    return;
                };
                let path = "newq-full-check".to_string();
                if let Some(o) = Self::first_display(show) {
                    o.stream = Some(StreamOut {
                        enabled: true,
                        kind: StreamKind::Server {
                            protocol: crate::show::ServerProtocol::Rtsp,
                            host: "127.0.0.1".into(),
                            port,
                            path: path.clone(),
                            kbps: 2000,
                            adapter: crate::streamlink::LOOPBACK.into(),
                            player_port: player,
                            host_here: true,
                            encoder: String::new(),
                        },
                        fps: 30,
                        height: 360,
                    });
                }
                self.serve = Some(Serve {
                    since: now,
                    port,
                    path,
                    saw_running: false,
                    status: "it was never started".into(),
                    reached: false,
                    asking: None,
                    asked: now,
                });
            }
            Ask::ServeOff => {
                if let Some(o) = Self::first_display(show) {
                    o.stream = None;
                }
                if let Some(s) = self.serve.take() {
                    let verdict = served_verdict(&s, &self.stream_said);
                    self.runner.set_served(verdict, show);
                }
            }
        }
    }

    /// Follow the recording to its end and the server while it runs.
    fn watch_send_out(&mut self, show: &Show, st: &UiState, now: Instant) {
        if let Some(s) = &mut self.serve {
            use crate::mediamtx::Status;
            match &st.hosting {
                Status::Running(_) => s.saw_running = true,
                Status::Failed(why) => s.status = why.clone(),
                Status::Missing => s.status = "the server's program was not found".into(),
                Status::NotAllowed => s.status = "this computer has not allowed it".into(),
                Status::Off => {}
            }
            if let Some(rx) = &s.asking {
                match rx.try_recv() {
                    Ok(there) => {
                        s.reached |= there;
                        s.asking = None;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                    Err(_) => s.asking = None,
                }
            }
            // Asked once a second, on a thread, until the stream is there.
            if s.saw_running
                && !s.reached
                && s.asking.is_none()
                && now.duration_since(s.asked) >= Duration::from_secs(1)
            {
                s.asked = now;
                let (tx, rx) = std::sync::mpsc::channel();
                let (port, path) = (s.port, s.path.clone());
                if std::thread::Builder::new()
                    .name("test-rtsp".into())
                    .spawn(move || {
                        let _ = tx.send(rtsp_has_stream(port, &path));
                    })
                    .is_ok()
                {
                    s.asking = Some(rx);
                }
            }
        }
        match &mut self.rec {
            Rec::Closing {
                dir,
                since,
                size,
                steady,
            } => {
                let len = newest_file(dir).map_or(0, |(_, len)| len);
                if len != *size {
                    *size = len;
                    *steady = now;
                }
                let waited = now.duration_since(*since);
                // The file has stopped growing for a while after the stream
                // was told to stop, or it has had long enough.
                let closed = waited >= Duration::from_secs(3)
                    && now.duration_since(*steady) >= Duration::from_millis(1500);
                if closed || waited >= Duration::from_secs(20) {
                    let (tx, rx) = std::sync::mpsc::channel();
                    let (ff, dir, said) = (self.ff.clone(), dir.clone(), self.stream_said.clone());
                    let spawned = std::thread::Builder::new()
                        .name("test-recording".into())
                        .spawn(move || {
                            let _ = tx.send(judge_recording(ff.as_ref(), &dir, &said));
                        });
                    self.rec = match spawned {
                        Ok(_) => Rec::Looking(rx),
                        Err(_) => Rec::Done,
                    };
                }
            }
            Rec::Looking(rx) => match rx.try_recv() {
                Ok(verdict) => {
                    self.runner.set_recorded(verdict, show);
                    self.rec = Rec::Done;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(_) => self.rec = Rec::Done,
            },
            Rec::Idle | Rec::On { .. } | Rec::Done => {}
        }
    }

    /// One look at the check. Returns why it should end, if it should.
    pub fn tick(
        &mut self,
        show: &mut Show,
        engine: &mut Engine,
        st: &UiState,
        now: Instant,
    ) -> Option<End> {
        let elapsed = now.duration_since(self.started);
        let t = elapsed.as_secs_f64();
        if let Some(gap) = self.sleep_watch.check(now, std::time::SystemTime::now()) {
            self.sleeps.push((t, gap.as_secs_f64()));
            let line = format!(
                "The computer was asleep or suspended for about {}",
                clock(gap)
            );
            self.log_at(t, &line);
        }
        for (at, text) in engine.take_log_tap() {
            let rel = at.saturating_duration_since(self.started).as_secs_f64();
            self.log_at(rel, &text);
        }
        let problems = engine.take_problems();
        self.stream_said.extend(
            problems
                .iter()
                .filter(|p| p.starts_with("Stream from"))
                .cloned(),
        );
        if let Some(sample) = self.stats.frame(now, || Inputs {
            playing: engine.active.len(),
            started_total: engine.started_total(),
            layers: engine.picture_layers(),
            late_permille: crate::render::late_frames_permille(),
            audio_permille: engine
                .audio
                .device_status(None)
                .map_or(0, |d| d.load_permille),
        }) {
            self.last = Some(sample);
            self.history.push(sample);
        }
        if let Some(mb) = self.memory.over(now) {
            return Some(End::Failed(format!(
                "NewQ and its FFmpeg processes were using {:.1} GB of this computer's {:.0} GB of memory, so the test was stopped before it made the computer unusable",
                mb as f64 / 1024.0,
                self.memory.ram_mb as f64 / 1024.0
            )));
        }
        if self.stop {
            return Some(End::Stopped);
        }
        let before = self.runner.results.len();
        let done = self.runner.tick(show, engine, now, problems);
        for ask in std::mem::take(&mut self.runner.asks) {
            self.obey(ask, show, now);
        }
        self.watch_send_out(show, st, now);
        // Each part's result, as it comes, in the events file.
        let said: Vec<String> = self.runner.results[before.min(self.runner.results.len())..]
            .iter()
            .map(|r| {
                format!(
                    "Test mode: {} ({}): {}",
                    r.kind,
                    r.title,
                    match r.outcome {
                        feature::Outcome::Passed => "ok",
                        feature::Outcome::Failed => "FAILED",
                        feature::Outcome::Skipped => "not run",
                    }
                )
            })
            .collect();
        for line in said {
            engine.log(line);
        }
        if !done {
            return None;
        }
        // Every part has been played. A recording still being closed and
        // looked at is waited for, but not for ever.
        let since = *self.settling.get_or_insert(now);
        let waiting = self.runner.awaits(&Check::Recorded)
            && matches!(self.rec, Rec::Closing { .. } | Rec::Looking(_));
        if waiting && now.duration_since(since) < Duration::from_secs(25) {
            return None;
        }
        Some(End::Finished)
    }

    /// Where the window shows the run is: the part, and what has passed so far.
    pub fn status(&self) -> (usize, usize, &'static str, &'static str) {
        if self.settling.is_some() {
            let (_, n, _, _) = self.runner.progress();
            return (
                n,
                n,
                "Send out",
                "waiting for the recording's file to be closed",
            );
        }
        self.runner.progress()
    }
}

/// The check could not begin (its media could not be made, its files not
/// written): nothing was changed, and what there is to send says why.
pub fn could_not_start(folder: &Path, log_dir: &Path, why: &str) -> Done {
    let findings = vec![Finding {
        severity: Severity::Problem,
        text: format!(
            "The full check could not start: {why}. Nothing about your show was changed."
        ),
    }];
    let report = format!(
        "NewQ Test Mode report: the full check\n=====================================\nResult: {}\nIt could not start: {why}\nNewQ {}\nNothing about your show was changed.\n",
        Verdict::ProblemsFound.label(),
        env!("CARGO_PKG_VERSION")
    );
    let _ = std::fs::create_dir_all(folder);
    let _ = std::fs::write(folder.join("report.txt"), &report);
    Done {
        compare: None,
        level: super::Level::Short,
        full: Some(Vec::new()),
        verdict: Verdict::ProblemsFound,
        headline: format!("The full check could not start: {why}"),
        steady: None,
        findings,
        folder: folder.to_path_buf(),
        report,
        laps: 0,
        elapsed: Duration::ZERO,
        ended: End::Failed(why.to_string()),
        bundle: Bundle::start(folder.to_path_buf(), log_dir.to_path_buf(), false),
    }
}

/// What the server part came to, when its stream is switched off.
fn served_verdict(s: &Serve, said: &[String]) -> feature::Verdict {
    use feature::Verdict::{Failed, NotChecked, Ok as Fine};
    let ran = s.since.elapsed().as_secs();
    if s.reached {
        return Fine(format!(
            "the streaming server ran on 127.0.0.1 only and the display's stream was being published to it (asked over RTSP on port {})",
            s.port
        ));
    }
    let complaint = said.last().cloned().unwrap_or_default();
    if !s.saw_running {
        let why = s.status.clone();
        return if why.contains("address already in use") || why.contains("not found") {
            NotChecked(format!(
                "the streaming server could not be started here: {why}"
            ))
        } else {
            Failed(format!(
                "the streaming server did not come up in {ran} s: {why}"
            ))
        };
    }
    if complaint.to_lowercase().contains("encoder") {
        return NotChecked(format!(
            "the FFmpeg in use has no H.264 encoder that works on this computer ({complaint})"
        ));
    }
    Failed(format!(
        "the streaming server ran, but no stream from the display reached it in {ran} s{}",
        if complaint.is_empty() {
            String::new()
        } else {
            format!(" ({complaint})")
        }
    ))
}

/// End the check: the show goes back first, then the account is written and
/// the zip made.
pub fn finish(
    mut run: FullRun,
    end: End,
    show: &mut Show,
    engine: &mut Engine,
    st: &mut UiState,
) -> Done {
    let elapsed = run.elapsed();
    let t_end = elapsed.as_secs_f64();
    let why = match &end {
        End::Finished => None,
        End::Stopped => Some("the test was stopped"),
        End::Quit => Some("NewQ was closed"),
        End::Failed(_) => Some("the test could not go on"),
    };
    if let Some(why) = why {
        run.runner.abandon(show, engine, why);
    }
    engine.stop_all(0.0);
    // The user's show comes back before anything else is done.
    if let Some(stash) = run.stash.take() {
        st.end_test(show, engine, stash);
    }
    crate::media::audio::set_muted(false);
    for (at, text) in engine.take_log_tap() {
        let rel = at.saturating_duration_since(run.started).as_secs_f64();
        run.log_at(rel, &text);
    }
    engine.set_log_tap(false);
    run.log_at(t_end, &format!("Test mode: ended ({end:?})"));

    let results = std::mem::take(&mut run.runner.results);
    let mut findings = feature::findings(&results);
    let hours = t_end / 3600.0;
    findings.extend(analyse(&run.history, &[], 0.0, &[], hours));
    let panics = crate::crash::panic_count().saturating_sub(run.panics_before);
    if panics > 0 {
        findings.insert(
            0,
            Finding {
                severity: Severity::Problem,
                text: format!(
                    "A part of NewQ met an error it did not expect {panics} time{} during the test and carried on (a panic). The report is in newq-crash.log, which is in the zip.",
                    if panics == 1 { "" } else { "s" }
                ),
            },
        );
    }
    if !run.sleeps.is_empty() {
        findings.push(Finding {
            severity: Severity::Problem,
            text: format!(
                "The computer went to sleep {} time{} during the test{}. What was playing then may have failed for that reason alone: run it again with the computer set not to sleep.",
                run.sleeps.len(),
                if run.sleeps.len() == 1 { "" } else { "s" },
                if run.awake.held() {
                    ", although NewQ asked the system to stay awake"
                } else {
                    ", and NewQ could not keep it awake"
                }
            ),
        });
    }
    match &end {
        End::Failed(why) => findings.insert(
            0,
            Finding {
                severity: Severity::Problem,
                text: format!("The test could not go on: {why}."),
            },
        ),
        End::Stopped | End::Quit => findings.push(Finding {
            severity: Severity::Note,
            text: format!(
                "The test was stopped after {} of about {}; the parts not reached are listed as not run.",
                clock(elapsed),
                clock(Duration::from_secs_f64(run.planned))
            ),
        }),
        End::Finished => {}
    }
    for n in &run.media_notes {
        findings.push(Finding {
            severity: Severity::Note,
            text: n.clone(),
        });
    }
    let since = std::time::SystemTime::now()
        .checked_sub(elapsed + Duration::from_secs(5))
        .unwrap_or(std::time::UNIX_EPOCH);
    let freeze_files = crate::freeze::copy_samples(&run.log_dir, &run.folder, since);
    if !freeze_files.is_empty() {
        findings.push(Finding {
            severity: Severity::Note,
            text: format!(
                "While the program was frozen it was looked at to see what it was waiting for: {} in the test's folder.",
                freeze_files.join(", ")
            ),
        });
    }
    let verdict = Verdict::of(&findings);
    let report = compose(
        &run,
        &end,
        elapsed,
        verdict,
        &findings,
        &results,
        &freeze_files,
    );
    let _ = std::fs::write(run.folder.join("report.txt"), &report);
    drop(run.events.take());
    let headline = match (&end, verdict) {
        (End::Failed(why), _) => format!("The test could not go on: {why}"),
        (_, Verdict::Passed) => "Every kind of cue did what it should.".into(),
        (_, Verdict::PassedWithNotes) => "Nothing went wrong; there are notes.".into(),
        (_, Verdict::ProblemsFound) => "Problems were found.".into(),
    };
    let bundle = Bundle::start(
        run.folder.clone(),
        run.log_dir.clone(),
        matches!(end, End::Quit),
    );
    Done {
        compare: None,
        level: super::Level::Short,
        full: Some(feature::by_kind(&results)),
        verdict,
        headline,
        steady: None,
        findings,
        folder: run.folder.clone(),
        report,
        laps: 0,
        elapsed,
        ended: end,
        bundle,
    }
}

fn compose(
    run: &FullRun,
    end: &End,
    elapsed: Duration,
    verdict: Verdict,
    findings: &[Finding],
    results: &[feature::PartResult],
    freeze_files: &[String],
) -> String {
    let mut out = String::new();
    let line = |out: &mut String, s: &str| {
        out.push_str(s);
        out.push('\n');
    };
    line(&mut out, "NewQ Test Mode report: the full check");
    line(&mut out, "=====================================");
    line(&mut out, &format!("Result: {}", verdict.label()));
    line(
        &mut out,
        &format!(
            "Ran {}. Ended: {}.",
            clock(elapsed),
            match end {
                End::Finished => "it ran its course".to_string(),
                End::Stopped => "stopped by you".to_string(),
                End::Failed(w) => format!("could not go on ({w})"),
                End::Quit => "NewQ was closed".to_string(),
            }
        ),
    );
    line(&mut out, &format!("NewQ {}", env!("CARGO_PKG_VERSION")));
    line(
        &mut out,
        "Your show was put aside while it ran and is back, unchanged. Nothing was sent outside this computer.",
    );
    line(&mut out, "");
    line(&mut out, "What was found");
    line(&mut out, "--------------");
    if findings.is_empty() {
        line(&mut out, "  Nothing to report.");
    }
    for f in findings {
        let tag = match f.severity {
            Severity::Problem => "PROBLEM",
            Severity::Note => "note   ",
            Severity::Watch => "tracked",
        };
        line(&mut out, &format!("  {tag}  {}", f.text));
    }
    line(&mut out, "");
    out.push_str(&feature::account(results));
    line(&mut out, "");
    line(&mut out, "Measurements");
    line(&mut out, "------------");
    match std::fs::read_to_string(run.folder.join("stats.csv"))
        .map_err(|e| e.to_string())
        .and_then(|t| crate::stats::report(&t))
    {
        Ok(r) => {
            for l in r.lines().filter(|l| {
                !l.starts_with("Look at")
                    && !l.starts_with("  - ")
                    && !l.contains("Nothing looks wrong")
            }) {
                line(&mut out, &format!("  {l}"));
            }
        }
        Err(e) => line(&mut out, &format!("  (none: {e})")),
    }
    line(&mut out, "");
    line(&mut out, "Files");
    line(&mut out, "-----");
    for (f, what) in [
        ("report.txt", "this report"),
        ("events.log", "what the show engine said, with times"),
        ("stats.csv", "a row of measurements every two seconds"),
        (
            "system.txt",
            "this computer, its displays, sound, FFmpeg and MediaMTX",
        ),
        ("settings.txt", "the settings in effect"),
        ("full-check.newq", "the show that was played"),
        ("newq.log", "NewQ's own log (in the zip)"),
        (
            "newq-crash.log",
            "crash reports, if there are any (in the zip)",
        ),
        ("media/", "the clips and pictures it made (not in the zip)"),
        ("recording/", "the recording it made (not in the zip)"),
    ] {
        line(&mut out, &format!("  {f:18} {what}"));
    }
    for f in freeze_files {
        line(
            &mut out,
            &format!("  {f:18} what the program was doing while it was frozen"),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_rtsp_answer_is_read_for_whether_the_stream_is_there() {
        assert!(rtsp_ok(b"RTSP/1.0 200 OK\r\nCSeq: 1\r\n"));
        assert!(!rtsp_ok(b"RTSP/1.0 404 Not Found\r\n"));
        assert!(!rtsp_ok(b""));
        assert!(!rtsp_ok(b"HTTP/1.1 200 OK\r\n"));
        // Nothing listening is "no", quickly, not a wait.
        let port = free_port().expect("a port");
        let began = Instant::now();
        assert!(!rtsp_has_stream(port, "nothing"));
        assert!(began.elapsed() < Duration::from_secs(3));
    }

    fn serve(saw_running: bool, reached: bool, status: &str) -> Serve {
        Serve {
            since: Instant::now(),
            port: 8554,
            path: "p".into(),
            saw_running,
            status: status.into(),
            reached,
            asking: None,
            asked: Instant::now(),
        }
    }

    #[test]
    fn the_server_part_is_judged_by_whether_the_stream_reached_it() {
        use feature::Verdict::{Failed, NotChecked, Ok as Fine};
        assert!(matches!(
            served_verdict(&serve(true, true, ""), &[]),
            Fine(_)
        ));
        // The server never came up: a fault, unless the port was simply taken.
        assert!(matches!(
            served_verdict(&serve(false, false, "it stopped"), &[]),
            Failed(_)
        ));
        assert!(matches!(
            served_verdict(
                &serve(
                    false,
                    false,
                    "listen tcp :1935: bind: address already in use"
                ),
                &[]
            ),
            NotChecked(_)
        ));
        // It ran and nothing reached it.
        let said = vec!["Stream from Display 1: connection refused".to_string()];
        let v = served_verdict(&serve(true, false, ""), &said);
        assert!(
            matches!(&v, Failed(why) if why.contains("connection refused")),
            "{v:?}"
        );
        // No encoder on this computer is not the stream's fault.
        let said = vec!["Stream from Display 1: no H.264 encoder works here".to_string()];
        assert!(matches!(
            served_verdict(&serve(true, false, ""), &said),
            NotChecked(_)
        ));
    }

    /// The window's run, stopped part way: the show and the settings are as
    /// they were, and the zip is made all the same.
    #[test]
    fn a_full_check_stopped_part_way_gives_everything_back_and_still_makes_the_zip() {
        use super::super::session::{TestMode, View};
        let Some(ff) = Ffmpeg::locate() else {
            assert!(std::env::var("NEWQ_REQUIRE_MEDIA").is_err(), "no ffmpeg");
            return;
        };
        let dir = std::env::temp_dir().join(format!("newq_tm_full_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // The user's show: a cue, a stream to somewhere else, OSC feedback.
        let mut show = Show::default();
        let c = show.make_cue(crate::show::CueType::Wait, None, 0);
        show.cues.push(c);
        show.outputs[0].stream = Some(StreamOut {
            enabled: false,
            kind: StreamKind::Udp {
                host: "203.0.113.9".into(),
                port: 5000,
            },
            ..StreamOut::default()
        });
        show.remote.osc_feedback = "203.0.113.9:9000".into();
        show.own_layers = false;
        let before = serde_json::to_string(&show).unwrap();
        let mut engine = Engine::for_tests();
        engine.ff = Some(ff.clone());
        engine.audio = crate::media::audio::AudioOut::silent();
        let mut st = UiState::default();
        st.prefs.hw_decode = true;
        let prefs_before = st.prefs.clone();
        let mut test = TestMode::default();
        test.start_full(ff, &dir, Vec::new(), SoundChoice::default());
        assert!(test.busy(), "{:?}", test.notice);
        let began = Instant::now();
        let mut last = began;
        let (mut was_locked, mut stopped, mut seen_parts) = (false, false, 0);
        while began.elapsed() < Duration::from_secs(120) {
            let now = Instant::now();
            engine.tick(&show, now.duration_since(last).as_secs_f64());
            last = now;
            test.update(&mut show, &mut engine, &mut st);
            was_locked |= st.test_locked;
            match test.view() {
                View::Checking(r) => {
                    // While it runs, nothing of the show's own can leave the computer.
                    assert!(show.remote.osc_feedback.is_empty());
                    assert!(show.outputs.iter().all(|o| match &o.stream {
                        None => true,
                        Some(s) => matches!(&s.kind, StreamKind::Record { .. }),
                    }));
                    seen_parts = r.runner.results.len();
                    if !stopped && seen_parts >= 2 {
                        stopped = true;
                    }
                }
                View::Done(_) => break,
                _ => {}
            }
            if stopped {
                test.stop();
            }
            std::thread::sleep(Duration::from_millis(4));
        }
        let View::Done(d) = test.view() else {
            panic!("the check did not end ({seen_parts} parts seen)");
        };
        assert!(was_locked && !st.test_locked);
        assert_eq!(
            serde_json::to_string(&show).unwrap(),
            before,
            "the show is back"
        );
        assert_eq!(st.prefs, prefs_before, "no setting was changed");
        assert!(!dir.join("prefs.json").exists(), "and none was saved");
        assert_eq!(d.ended, End::Stopped);
        let kinds = d.full.as_ref().expect("the full check's table");
        assert!(
            kinds.iter().any(|k| k.1 > 0),
            "some part passed: {}",
            d.report
        );
        assert!(
            kinds.iter().any(|k| k.3 > 0),
            "the rest are listed as not run"
        );
        assert!(d.report.contains("stopped by you"), "{}", d.report);
        assert!(
            d.report.contains("What each kind of cue did"),
            "{}",
            d.report
        );
        // The zip is made although the test was stopped.
        let zip = loop {
            match d.bundle.get() {
                Some(made) => break made.expect("a zip"),
                None if began.elapsed() > Duration::from_secs(150) => panic!("no zip"),
                None => std::thread::sleep(Duration::from_millis(20)),
            }
        };
        assert!(zip.starts_with(&dir) && zip.is_file(), "{zip:?}");
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&zip).unwrap()).unwrap();
        for name in [
            "report.txt",
            "events.log",
            "system.txt",
            "settings.txt",
            "stats.csv",
            "full-check.newq",
            "contents.txt",
        ] {
            assert!(archive.by_name(name).is_ok(), "{name} is not in the zip");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_check_that_cannot_start_still_leaves_a_zip_that_says_why() {
        let dir = std::env::temp_dir().join(format!("newq_tm_nostart_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let folder = dir.join("test-mode").join("runs").join("now_full");
        let d = could_not_start(&folder, &dir, "the test clip: Unknown encoder");
        assert_eq!(d.verdict, Verdict::ProblemsFound);
        assert!(matches!(&d.ended, End::Failed(why) if why.contains("Unknown encoder")));
        let began = Instant::now();
        let zip = loop {
            match d.bundle.get() {
                Some(made) => break made.expect("a zip"),
                None if began.elapsed() > Duration::from_secs(20) => panic!("no zip"),
                None => std::thread::sleep(Duration::from_millis(10)),
            }
        };
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&zip).unwrap()).unwrap();
        let mut text = String::new();
        std::io::Read::read_to_string(&mut archive.by_name("report.txt").unwrap(), &mut text)
            .unwrap();
        assert!(
            text.contains("It could not start: the test clip: Unknown encoder"),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_recording_is_judged_by_reading_it_back() {
        use feature::Verdict::{Failed, NotChecked, Ok as Fine};
        let dir = std::env::temp_dir().join(format!("newq_fullrec_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Nothing written: a fault, with what the stream said; no encoder is not.
        assert!(matches!(judge_recording(None, &dir, &[]), Failed(_)));
        let said = vec!["Stream from Display 1: no H.264 encoder was found".to_string()];
        assert!(matches!(judge_recording(None, &dir, &said), NotChecked(_)));
        assert!(matches!(
            judge_recording(None, &dir.join("missing"), &[]),
            Failed(_)
        ));
        let Some((ff, clip)) = crate::media::test_media("testsrc_10s.mp4") else {
            return;
        };
        // A real film in the folder is read back and found long enough.
        std::fs::copy(&clip, dir.join("take.mp4")).unwrap();
        let v = judge_recording(Some(&ff), &dir, &[]);
        assert!(
            matches!(&v, Fine(what) if what.contains("recording/take.mp4")),
            "{v:?}"
        );
        // A file that is no film is a fault.
        std::fs::remove_file(dir.join("take.mp4")).unwrap();
        std::fs::write(dir.join("take.mkv"), b"not a film").unwrap();
        assert!(matches!(judge_recording(Some(&ff), &dir, &[]), Failed(_)));
        assert_eq!(newest_file(&dir).map(|(_, len)| len), Some(10));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
