//! A log of how the program is doing, for shows that run unattended (an
//! installation, an overnight soak test): one CSV row every few seconds with
//! memory, processor use, FFmpeg processes, how many cues are playing, late
//! frames, the audio load and the longest stall of the main loop.
//!
//! Switched on by `NEWQ_STATS` (a file, or `1` for `newq-stats.csv` in the log
//! folder); `NEWQ_STATS_SECS` sets the interval (default 10). Afterwards
//! `newq --stats-report FILE` reads the file and says whether memory grew,
//! whether the main loop stalled and how the graphics and audio kept up.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessesToUpdate, System};

pub const HEADER: &str = "time_utc,uptime_s,rss_mb,cpu_pct,ffmpeg,ffmpeg_rss_mb,ffmpeg_cpu_pct,\
                          fds,threads,playing,started_total,layers,late_permille,audio_permille,\
                          stall_max_ms,log_kb,gpu_mb,commit_mb";

/// What the app tells the log at each sample.
#[derive(Clone, Copy, Debug, Default)]
pub struct Inputs {
    pub playing: usize,
    pub started_total: u64,
    pub layers: u32,
    pub late_permille: u32,
    pub audio_permille: u32,
}

/// One row of the log, as numbers (the Test Mode window shows the latest).
#[derive(Clone, Copy, Debug, Default)]
pub struct Sample {
    pub uptime_s: u64,
    pub rss_mb: u64,
    pub cpu_pct: f32,
    pub ffmpeg: usize,
    pub ffmpeg_rss_mb: u64,
    pub ffmpeg_cpu_pct: f32,
    pub fds: Option<usize>,
    pub threads: Option<usize>,
    pub inputs: Inputs,
    pub stall_ms: u64,
    /// What NewQ has asked the graphics card for, if the backend says.
    pub gpu_mb: Option<u64>,
    /// Windows: the memory NewQ has committed (private bytes), which a leak in
    /// the graphics driver can raise without the working set moving.
    pub commit_mb: Option<u64>,
}

pub struct StatsLog {
    /// Asks the sampling thread for a row.
    jobs: Option<crossbeam_channel::Sender<Job>>,
    /// Its rows, as they are finished.
    rows: crossbeam_channel::Receiver<Sample>,
    /// Set when the thread has written its last row and gone.
    done: crossbeam_channel::Receiver<()>,
    /// A row has been asked for and not yet come back.
    asked: bool,
    every: Duration,
    started: Instant,
    last_row: Instant,
    last_frame: Instant,
    stall: Duration,
}

/// What the main loop knew when it asked for a row.
struct Job {
    when: i64,
    uptime: u64,
    inputs: Inputs,
    stall: u64,
}

/// What looking at the processes found: the slow part of a row, done on the
/// sampling thread (on Windows, going through every process took up to 3 s at
/// times in an hour's test, and the show stopped while the main loop did it).
#[derive(Clone, Copy, Debug, Default)]
pub struct Looked {
    pub rss: u64,
    pub cpu: f32,
    pub commit: Option<u64>,
    pub children: usize,
    pub children_rss: u64,
    pub children_cpu: f32,
    pub open: Option<usize>,
    pub threads: Option<usize>,
    pub log_kb: u64,
}

/// Look at this process and its children (the FFmpegs), and the log's size.
pub fn look(sys: &mut System, me: Pid, log_path: &Path) -> Looked {
    sys.refresh_processes(ProcessesToUpdate::All, true);
    let mut l = Looked::default();
    for (pid, p) in sys.processes() {
        // On Linux each thread is listed as a process, with the whole
        // process's memory: they are not children.
        if p.thread_kind().is_some() {
            continue;
        }
        if *pid == me {
            l.rss = p.memory();
            l.cpu = p.cpu_usage();
            // Only Windows' figure means committed memory; elsewhere it is
            // the whole address space, which says nothing.
            if cfg!(windows) {
                l.commit = Some(p.virtual_memory());
            }
        } else if p.parent() == Some(me) {
            l.children += 1;
            l.children_rss += p.memory();
            l.children_cpu += p.cpu_usage();
        }
    }
    l.log_kb = std::fs::metadata(log_path).map_or(0, |m| m.len() / 1024);
    l.open = open_files();
    l.threads = threads();
    l
}

impl StatsLog {
    /// The log the environment asks for, if it does.
    pub fn from_env(log_dir: &Path) -> Option<StatsLog> {
        let v = std::env::var("NEWQ_STATS").ok().filter(|v| !v.is_empty())?;
        let path = if v == "1" || v.eq_ignore_ascii_case("true") {
            log_dir.join("newq-stats.csv")
        } else {
            PathBuf::from(v)
        };
        let secs = std::env::var("NEWQ_STATS_SECS")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|s| *s >= 1.0)
            .unwrap_or(10.0);
        match StatsLog::create(
            &path,
            Duration::from_secs_f64(secs),
            log_dir.join("newq.log"),
        ) {
            Ok(l) => {
                log::info!("stats: writing {} every {secs} s", path.display());
                Some(l)
            }
            Err(e) => {
                log::warn!("stats: can't write {}: {e}", path.display());
                None
            }
        }
    }

    /// A sampler that writes nothing: the statistics window's.
    pub fn in_memory(every: Duration, log_path: PathBuf) -> StatsLog {
        StatsLog::with(None, every, real_looker(log_path))
    }

    pub fn create(path: &Path, every: Duration, log_path: PathBuf) -> std::io::Result<StatsLog> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let new = !path.exists() || std::fs::metadata(path).is_ok_and(|m| m.len() == 0);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        let mut file = std::io::BufWriter::new(file);
        if new {
            writeln!(file, "{HEADER}")?;
            file.flush()?;
        }
        Ok(StatsLog::with(Some(file), every, real_looker(log_path)))
    }

    /// A log whose rows are measured by `looker` on a thread of its own, and
    /// written there to `file` if there is one: the main loop only asks.
    pub fn with(
        file: Option<std::io::BufWriter<std::fs::File>>,
        every: Duration,
        mut looker: impl FnMut() -> Looked + Send + 'static,
    ) -> StatsLog {
        let (jobs, job_rx) = crossbeam_channel::unbounded::<Job>();
        let (row_tx, rows) = crossbeam_channel::unbounded();
        let (done_tx, done) = crossbeam_channel::bounded(1);
        let mut file = file;
        let _ = std::thread::Builder::new()
            .name("stats-sampler".into())
            .spawn(move || {
                for job in job_rx {
                    let l = looker();
                    let sample = Sample {
                        uptime_s: job.uptime,
                        rss_mb: l.rss >> 20,
                        cpu_pct: l.cpu,
                        ffmpeg: l.children,
                        ffmpeg_cpu_pct: l.children_cpu,
                        ffmpeg_rss_mb: l.children_rss >> 20,
                        fds: l.open,
                        threads: l.threads,
                        inputs: job.inputs,
                        stall_ms: job.stall,
                        gpu_mb: crate::gpu::gpu_allocated_mb(),
                        commit_mb: l.commit.map(|c| c >> 20),
                    };
                    if let Some(f) = &mut file {
                        let _ = writeln!(f, "{}", row_of(job.when, &sample, &l));
                        let _ = f.flush();
                    }
                    if row_tx.send(sample).is_err() {
                        break;
                    }
                }
                let _ = done_tx.send(());
            });
        let now = Instant::now();
        StatsLog {
            jobs: Some(jobs),
            rows,
            done,
            asked: false,
            every,
            started: now,
            last_row: now,
            last_frame: now,
            stall: Duration::ZERO,
        }
    }

    /// Called each time round the main loop: asks for a row when one is due
    /// and gives back a row that has been finished, if one has. Never waits.
    pub fn frame(&mut self, now: Instant, inputs: impl FnOnce() -> Inputs) -> Option<Sample> {
        self.stall = self.stall.max(now.duration_since(self.last_frame));
        self.last_frame = now;
        // One row at a time: a slow look is not asked for again meanwhile.
        if !self.asked
            && now.duration_since(self.last_row) >= self.every
            && let Some(jobs) = &self.jobs
        {
            self.last_row = now;
            let job = Job {
                when: unix_now(),
                uptime: now.duration_since(self.started).as_secs(),
                inputs: inputs(),
                stall: self.stall.as_millis() as u64,
            };
            self.stall = Duration::ZERO;
            self.asked = jobs.send(job).is_ok();
        }
        let row = self.rows.try_recv().ok();
        if row.is_some() {
            self.asked = false;
        }
        row
    }
}

impl Drop for StatsLog {
    /// The last row asked for is written (a test's report reads the file),
    /// waiting a moment for it at most.
    fn drop(&mut self) {
        self.jobs = None;
        let _ = self.done.recv_timeout(Duration::from_secs(2));
    }
}

fn real_looker(log_path: PathBuf) -> impl FnMut() -> Looked + Send + 'static {
    let mut sys = System::new();
    let me = Pid::from_u32(std::process::id());
    move || look(&mut sys, me, &log_path)
}

fn row_of(when: i64, s: &Sample, l: &Looked) -> String {
    row(
        when,
        s.uptime_s,
        [l.rss, l.children_rss],
        [l.cpu, l.children_cpu],
        l.children,
        [l.open, l.threads],
        s.inputs,
        s.stall_ms,
        l.log_kb,
        [s.gpu_mb, s.commit_mb],
    )
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// How many files and sockets the process has open: file descriptors on Linux
/// and a Mac, handles on Windows.
#[cfg(target_os = "linux")]
fn open_files() -> Option<usize> {
    std::fs::read_dir("/proc/self/fd").ok().map(|d| d.count())
}

#[cfg(target_os = "macos")]
fn open_files() -> Option<usize> {
    std::fs::read_dir("/dev/fd").ok().map(|d| d.count())
}

#[cfg(windows)]
fn open_files() -> Option<usize> {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};
    let mut handles = 0u32;
    // SAFETY: the pseudo handle for this process is always valid, and `handles`
    // is a u32 that lives through the call.
    let ok = unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut handles) };
    (ok != 0).then_some(handles as usize)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn open_files() -> Option<usize> {
    None
}

/// How many threads the process has (Linux only; empty elsewhere).
fn threads() -> Option<usize> {
    std::fs::read_dir("/proc/self/task").ok().map(|d| d.count())
}

pub fn utc(secs: i64) -> String {
    let (y, mo, d, h, mi) = crate::export::civil_from_unix(secs);
    format!(
        "{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{:02}Z",
        secs.rem_euclid(60)
    )
}

#[allow(clippy::too_many_arguments)]
fn row(
    when: i64,
    uptime: u64,
    rss: [u64; 2],
    cpu: [f32; 2],
    ffmpeg: usize,
    open: [Option<usize>; 2],
    i: Inputs,
    stall_ms: u64,
    log_kb: u64,
    extra: [Option<u64>; 2],
) -> String {
    let cell = |v: Option<u64>| v.map_or(String::new(), |v| v.to_string());
    format!(
        "{},{uptime},{},{:.1},{ffmpeg},{},{:.1},{},{},{},{},{},{},{},{stall_ms},{log_kb},{},{}",
        utc(when),
        rss[0] >> 20,
        cpu[0],
        rss[1] >> 20,
        cpu[1],
        open[0].map_or(String::new(), |f| f.to_string()),
        open[1].map_or(String::new(), |f| f.to_string()),
        i.playing,
        i.started_total,
        i.layers,
        i.late_permille,
        i.audio_permille,
        cell(extra[0]),
        cell(extra[1]),
    )
}

// ----- The report ------------------------------------------------------------

struct Rows {
    uptime: Vec<f64>,
    col: std::collections::HashMap<&'static str, Vec<f64>>,
}

fn read_rows(text: &str) -> Result<Rows, String> {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header: Vec<&str> = lines
        .next()
        .ok_or("the file is empty")?
        .split(',')
        .collect();
    if header.first() != Some(&"time_utc") {
        return Err("not a NewQ stats file (no header row)".into());
    }
    let names: Vec<&'static str> = HEADER.split(',').map(str::trim).collect();
    // Where each column is in this file: a file from an earlier version has
    // fewer, and its missing columns read as blanks.
    let at: Vec<Option<usize>> = names
        .iter()
        .map(|n| header.iter().position(|h| h.trim() == *n))
        .collect();
    let mut rows = Rows {
        uptime: Vec::new(),
        col: names.iter().map(|n| (*n, Vec::new())).collect(),
    };
    for line in lines {
        // A header from an earlier run appended to the same file starts afresh.
        if line.starts_with("time_utc") {
            continue;
        }
        let cells: Vec<&str> = line.split(',').collect();
        if cells.len() != header.len() {
            continue;
        }
        rows.uptime.push(cells[1].parse().unwrap_or(0.0));
        for (n, i) in names.iter().zip(&at) {
            let value = i
                .and_then(|i| cells.get(i))
                .and_then(|c| c.parse().ok())
                .unwrap_or(f64::NAN);
            rows.col.get_mut(n).expect("column").push(value);
        }
    }
    if rows.uptime.is_empty() {
        return Err("no rows".into());
    }
    Ok(rows)
}

/// Least-squares slope of `y` against `x`.
fn slope(x: &[f64], y: &[f64]) -> f64 {
    let n = x.len() as f64;
    if x.len() < 2 {
        return 0.0;
    }
    let (mx, my) = (x.iter().sum::<f64>() / n, y.iter().sum::<f64>() / n);
    let num: f64 = x.iter().zip(y).map(|(a, b)| (a - mx) * (b - my)).sum();
    let den: f64 = x.iter().map(|a| (a - mx).powi(2)).sum();
    if den == 0.0 { 0.0 } else { num / den }
}

fn finite(v: &[f64]) -> Vec<f64> {
    v.iter().copied().filter(|x| x.is_finite()).collect()
}

fn max(v: &[f64]) -> f64 {
    finite(v).into_iter().fold(0.0, f64::max)
}

/// A readable summary of a stats file, with warnings for what looks wrong.
pub fn report(text: &str) -> Result<String, String> {
    let r = read_rows(text)?;
    let hours = (r.uptime[r.uptime.len() - 1] - r.uptime[0]) / 3600.0;
    let mut out = format!(
        "{} samples over {hours:.2} hours (up to uptime {:.0} s)\n",
        r.uptime.len(),
        r.uptime[r.uptime.len() - 1]
    );
    let mut flags: Vec<String> = Vec::new();
    // Growth is judged after the first tenth, which is filling caches.
    let skip = r.uptime.len() / 10;
    let growth = |name: &str| {
        let y = &r.col[name];
        let pairs: Vec<(f64, f64)> = r.uptime[skip..]
            .iter()
            .zip(&y[skip..])
            .filter(|(_, v)| v.is_finite())
            .map(|(a, b)| (*a / 3600.0, *b))
            .collect();
        let (x, y): (Vec<f64>, Vec<f64>) = pairs.into_iter().unzip();
        slope(&x, &y)
    };
    for (name, label, limit) in [
        ("rss_mb", "NewQ memory", 5.0),
        ("ffmpeg_rss_mb", "FFmpeg memory", 5.0),
        ("fds", "open files", 20.0),
        ("threads", "threads", 20.0),
        ("log_kb", "log size", 50_000.0),
        ("gpu_mb", "graphics card memory asked for", 20.0),
        ("commit_mb", "committed memory", 20.0),
    ] {
        let y = finite(&r.col[name]);
        if y.is_empty() {
            continue;
        }
        let g = growth(name);
        out += &format!(
            "{label}: {:.0} → {:.0} (max {:.0}), trend {g:+.1} per hour\n",
            y[0],
            y[y.len() - 1],
            y.iter().copied().fold(0.0, f64::max)
        );
        if g > limit && hours >= 0.5 {
            flags.push(format!(
                "{label} rose through the run ({g:+.1} per hour): worth tracking, and only a problem if it keeps on over a longer run"
            ));
        }
    }
    let ff = &r.col["ffmpeg"];
    out += &format!(
        "FFmpeg processes: {} to {}\n",
        finite(ff).iter().copied().fold(f64::MAX, f64::min),
        max(ff)
    );
    out += &format!(
        "Cues playing: up to {}; cues started in all: {}\n",
        max(&r.col["playing"]),
        max(&r.col["started_total"])
    );
    let drawn = |name: &str| -> Vec<f64> {
        r.col[name]
            .iter()
            .zip(&r.col["layers"])
            .filter(|(_, layers)| **layers > 0.0)
            .map(|(v, _)| *v)
            .collect()
    };
    let (stall, late, audio) = (
        max(&r.col["stall_max_ms"]),
        // Late frames mean nothing while no picture is drawn.
        max(&drawn("late_permille")),
        max(&r.col["audio_permille"]),
    );
    out += &format!(
        "Longest stall of the main loop: {stall} ms; most late frames while drawing: {}%; audio load: up to {}%\n",
        late / 10.0,
        audio / 10.0
    );
    if stall > 1000.0 {
        flags.push(format!("the main loop stalled for {stall} ms"));
    }
    if late > 200.0 {
        flags.push(format!("{}% of frames were late at worst", late / 10.0));
    }
    if audio > 600.0 {
        flags.push(format!("audio used {}% of its time at worst", audio / 10.0));
    }
    // Rows further apart than they should be: the program was frozen or not running.
    let gaps: Vec<f64> = r.uptime.windows(2).map(|w| w[1] - w[0]).collect();
    let mut sorted = gaps.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    if let Some(median) = sorted.get(sorted.len() / 2).copied() {
        let worst = sorted.last().copied().unwrap_or(0.0);
        if median > 0.0 && worst > median * 3.0 + 5.0 {
            flags.push(format!(
                "a gap of {worst:.0} s between samples (usually {median:.0} s): frozen, asleep or restarted"
            ));
        }
    }
    if flags.is_empty() {
        out += "\nNothing looks wrong.\n";
    } else {
        out += "\nLook at:\n";
        for f in flags {
            out += &format!("  - {f}\n");
        }
    }
    Ok(out)
}

/// `newq --stats-report FILE`: prints the report, and (a Windows release has no
/// console) also saves it as `FILE.report.txt` beside the log.
pub fn run_if_asked(args: &[String]) -> Option<i32> {
    if args.first().map(String::as_str) != Some("--stats-report") {
        return None;
    }
    let result = args
        .get(1)
        .ok_or_else(|| "usage: newq --stats-report FILE".to_string())
        .and_then(|f| {
            let text = std::fs::read_to_string(f).map_err(|e| format!("{f}: {e}"))?;
            let report = report(&text)?;
            let saved = Path::new(f).with_extension("report.txt");
            let _ = std::fs::write(&saved, &report);
            Ok((report, saved))
        });
    Some(match result {
        Ok((text, saved)) => {
            print!("{text}");
            println!("(saved in {})", saved.display());
            0
        }
        Err(e) => {
            eprintln!("stats-report: {e}");
            2
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_file(rows: usize, rss_per_row: u64, extra: impl Fn(usize) -> (u64, u64)) -> String {
        let mut text = format!("{HEADER}\n");
        for n in 0..rows {
            let (stall, late) = extra(n);
            let i = Inputs {
                playing: 3,
                started_total: n as u64 * 2,
                layers: 2,
                late_permille: late as u32,
                audio_permille: 100,
            };
            text += &row(
                1_700_000_000 + n as i64 * 10,
                n as u64 * 10,
                [(500 + n as u64 * rss_per_row) << 20, 200 << 20],
                [12.5, 30.0],
                2,
                [Some(40), Some(60)],
                i,
                stall,
                100,
                [None, None],
            );
            text.push('\n');
        }
        text
    }

    /// A file from before the graphics and commit columns existed: those two
    /// columns cut off its header and rows.
    fn without_the_last_two_columns(text: &str) -> String {
        text.lines()
            .map(|l| l.rsplitn(3, ',').last().unwrap().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_file_from_an_earlier_version_still_reports() {
        let new = sample_file(100, 0, |_| (20, 0));
        let old = without_the_last_two_columns(&new);
        assert!(!old.lines().next().unwrap().contains("gpu_mb"));
        let r = report(&old).expect("an older file");
        assert!(r.contains("100 samples"), "{r}");
        // And no line about columns it never had.
        assert!(!r.contains("graphics card"), "{r}");
    }

    #[test]
    fn growth_in_what_the_graphics_card_was_asked_for_is_reported() {
        let mut text = format!("{HEADER}\n");
        for n in 0..720u64 {
            let i = Inputs {
                playing: 2,
                started_total: n,
                layers: 1,
                late_permille: 0,
                audio_permille: 0,
            };
            // Working set flat; the card's side grows 100 MB an hour (a sample
            // every 10 s over two hours).
            text += &row(
                1_700_000_000 + n as i64 * 10,
                n * 10,
                [300 << 20, 0],
                [10.0, 0.0],
                1,
                [Some(100), None],
                i,
                10,
                1,
                [Some(500 + n * 100 / 360), Some(400)],
            );
            text.push('\n');
        }
        let r = report(&text).unwrap();
        assert!(r.contains("graphics card memory asked for"), "{r}");
        assert!(r.contains("rose through the run"), "{r}");
        assert!(!r.contains("committed memory rose"), "{r}");
    }

    #[test]
    fn a_row_has_one_cell_for_each_column() {
        let text = sample_file(1, 0, |_| (5, 0));
        let cells = text.lines().nth(1).unwrap().split(',').count();
        assert_eq!(cells, HEADER.split(',').count());
        assert!(text.contains("2023-11-14T22:13:20Z"), "{text}");
    }

    #[test]
    fn a_steady_run_is_reported_as_fine() {
        let text = sample_file(2000, 0, |_| (20, 0));
        let r = report(&text).unwrap();
        assert!(r.contains("Nothing looks wrong"), "{r}");
        assert!(r.contains("NewQ memory: 500 → 500"), "{r}");
    }

    #[test]
    fn a_sampler_without_a_file_still_measures() {
        let mut log = StatsLog::in_memory(Duration::from_millis(50), PathBuf::from("none.log"));
        let t = Instant::now();
        assert!(log.frame(t, Inputs::default).is_none());
        let s = wait_for_row(&mut log, t + Duration::from_millis(80));
        assert!(s.rss_mb > 0);
        assert!(s.stall_ms >= 80);
    }

    /// Frames from `t` on until a row comes back.
    fn wait_for_row(log: &mut StatsLog, t: Instant) -> Sample {
        let began = Instant::now();
        loop {
            if let Some(s) = log.frame(t + began.elapsed(), Inputs::default) {
                return s;
            }
            assert!(
                began.elapsed() < Duration::from_secs(10),
                "no row came back"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Looking at the processes can take seconds (Windows, in an hour's test):
    /// the main loop must not wait for it.
    #[test]
    fn a_slow_look_at_the_processes_does_not_hold_up_the_frame() {
        let mut log = StatsLog::with(None, Duration::from_millis(10), || {
            std::thread::sleep(Duration::from_millis(600));
            Looked {
                rss: 7 << 20,
                ..Looked::default()
            }
        });
        let t0 = Instant::now();
        let mut row = None;
        let mut slowest = Duration::ZERO;
        while row.is_none() && t0.elapsed() < Duration::from_secs(10) {
            let began = Instant::now();
            row = log.frame(Instant::now() + Duration::from_millis(20), Inputs::default);
            slowest = slowest.max(began.elapsed());
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(row.expect("the row came back").rss_mb, 7);
        assert!(
            t0.elapsed() >= Duration::from_millis(550),
            "it was not slow"
        );
        assert!(
            slowest < Duration::from_millis(100),
            "a frame waited {slowest:?}"
        );
    }

    #[test]
    fn growing_memory_a_stall_and_late_frames_are_flagged() {
        // 2000 rows 10 s apart is 5.5 hours; 1 MB a row is 360 MB an hour.
        let text = sample_file(2000, 1, |n| if n == 1500 { (4000, 400) } else { (20, 0) });
        let r = report(&text).unwrap();
        assert!(r.contains("NewQ memory rose through the run"), "{r}");
        assert!(r.contains("main loop stalled for 4000"), "{r}");
        assert!(r.contains("40% of frames were late"), "{r}");
    }

    #[test]
    fn a_gap_in_the_samples_and_bad_files_are_noticed() {
        let mut text = sample_file(100, 0, |_| (20, 0));
        // A restart: uptime drops back and a new header is appended.
        text += &sample_file(10, 0, |_| (20, 0));
        let r = report(&text).unwrap();
        assert!(r.contains("samples"), "{r}");
        assert!(report("").is_err());
        assert!(report("a,b\n1,2\n").is_err());
        assert!(report(&format!("{HEADER}\n")).is_err());
    }

    #[test]
    fn the_log_writes_a_header_once_and_rows_as_they_fall_due() {
        let dir = std::env::temp_dir().join(format!("newq_stats_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("s.csv");
        let mut log =
            StatsLog::create(&path, Duration::from_millis(50), dir.join("x.log")).unwrap();
        let t0 = Instant::now();
        log.frame(t0, Inputs::default);
        std::thread::sleep(Duration::from_millis(60));
        log.frame(Instant::now(), || Inputs {
            playing: 4,
            ..Inputs::default()
        });
        // Dropped with the row asked for and not back: it is written all the same.
        drop(log);
        // A second run appends without a second header.
        let mut again =
            StatsLog::create(&path, Duration::from_millis(1), dir.join("x.log")).unwrap();
        std::thread::sleep(Duration::from_millis(5));
        again.frame(Instant::now(), Inputs::default);
        drop(again);
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("time_utc").count(), 1, "{text}");
        assert_eq!(text.lines().count(), 3, "{text}");
        assert!(text.lines().nth(1).unwrap().contains(",4,"), "{text}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_report_command_also_saves_the_report_beside_the_log() {
        let dir = std::env::temp_dir().join(format!("newq_stats_report_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let csv = dir.join("run.csv");
        std::fs::write(&csv, sample_file(50, 0, |_| (20, 0))).unwrap();
        let args = vec!["--stats-report".to_string(), csv.display().to_string()];
        assert_eq!(run_if_asked(&args), Some(0));
        let saved = std::fs::read_to_string(dir.join("run.report.txt")).unwrap();
        assert!(saved.contains("samples over"), "{saved}");
        assert_eq!(
            run_if_asked(&["--stats-report".into(), "/no/such.csv".into()]),
            Some(2)
        );
        assert_eq!(run_if_asked(&["--other".into()]), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn late_frames_with_nothing_drawn_are_not_counted() {
        // The gauge reads 100% all night; whether a picture is drawn decides if that counts.
        let make = |layers: u32| {
            let mut text = format!("{HEADER}\n");
            for n in 0..200u64 {
                let i = Inputs {
                    playing: 1,
                    started_total: n,
                    layers,
                    late_permille: 1000,
                    audio_permille: 5,
                };
                text += &row(
                    1_700_000_000 + n as i64 * 10,
                    n * 10,
                    [500 << 20, 0],
                    [1.0, 0.0],
                    0,
                    [None, None],
                    i,
                    10,
                    1,
                    [None, None],
                );
                text.push('\n');
            }
            text
        };
        let r = report(&make(0)).unwrap();
        assert!(r.contains("Nothing looks wrong"), "{r}");
        let r = report(&make(2)).unwrap();
        assert!(r.contains("100% of frames were late"), "{r}");
    }
}
