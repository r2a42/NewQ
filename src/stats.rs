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
                          stall_max_ms,log_kb";

/// What the app tells the log at each sample.
#[derive(Clone, Copy, Debug, Default)]
pub struct Inputs {
    pub playing: usize,
    pub started_total: u64,
    pub layers: u32,
    pub late_permille: u32,
    pub audio_permille: u32,
}

pub struct StatsLog {
    file: std::io::BufWriter<std::fs::File>,
    sys: System,
    me: Pid,
    every: Duration,
    started: Instant,
    last_row: Instant,
    last_frame: Instant,
    stall: Duration,
    log_path: PathBuf,
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

    fn create(path: &Path, every: Duration, log_path: PathBuf) -> std::io::Result<StatsLog> {
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
        }
        let now = Instant::now();
        Ok(StatsLog {
            file,
            sys: System::new(),
            me: Pid::from_u32(std::process::id()),
            every,
            started: now,
            last_row: now,
            last_frame: now,
            stall: Duration::ZERO,
            log_path,
        })
    }

    /// Called each time round the main loop; writes a row when one is due.
    pub fn frame(&mut self, now: Instant, inputs: impl FnOnce() -> Inputs) {
        self.stall = self.stall.max(now.duration_since(self.last_frame));
        self.last_frame = now;
        if now.duration_since(self.last_row) < self.every {
            return;
        }
        self.last_row = now;
        self.sys.refresh_processes(ProcessesToUpdate::All, true);
        let (mut rss, mut cpu) = (0u64, 0.0f32);
        let (mut n, mut f_rss, mut f_cpu) = (0usize, 0u64, 0.0f32);
        for (pid, p) in self.sys.processes() {
            // On Linux each thread is listed as a process, with the whole
            // process's memory: they are not children.
            if p.thread_kind().is_some() {
                continue;
            }
            if *pid == self.me {
                rss = p.memory();
                cpu = p.cpu_usage();
            } else if p.parent() == Some(self.me) {
                n += 1;
                f_rss += p.memory();
                f_cpu += p.cpu_usage();
            }
        }
        let log_kb = std::fs::metadata(&self.log_path).map_or(0, |m| m.len() / 1024);
        let line = row(
            unix_now(),
            now.duration_since(self.started).as_secs(),
            [rss, f_rss],
            [cpu, f_cpu],
            n,
            [open_files(), threads()],
            inputs(),
            self.stall.as_millis() as u64,
            log_kb,
        );
        self.stall = Duration::ZERO;
        let _ = writeln!(self.file, "{line}");
        let _ = self.file.flush();
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// How many files and sockets the process has open (Linux; empty elsewhere).
fn open_files() -> Option<usize> {
    std::fs::read_dir("/proc/self/fd").ok().map(|d| d.count())
}

/// How many threads the process has (Linux; empty elsewhere).
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
) -> String {
    format!(
        "{},{uptime},{},{:.1},{ffmpeg},{},{:.1},{},{},{},{},{},{},{},{stall_ms},{log_kb}",
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
        if cells.len() != names.len() {
            continue;
        }
        rows.uptime.push(cells[1].parse().unwrap_or(0.0));
        for (n, c) in names.iter().zip(&cells) {
            rows.col
                .get_mut(n)
                .expect("column")
                .push(c.parse().unwrap_or(f64::NAN));
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
            flags.push(format!("{label} keeps growing ({g:+.1} per hour)"));
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
    let (stall, late, audio) = (
        max(&r.col["stall_max_ms"]),
        max(&r.col["late_permille"]),
        max(&r.col["audio_permille"]),
    );
    out += &format!(
        "Longest stall of the main loop: {stall} ms; most late frames: {}%; audio load: up to {}%\n",
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
            );
            text.push('\n');
        }
        text
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
    fn growing_memory_a_stall_and_late_frames_are_flagged() {
        // 2000 rows 10 s apart is 5.5 hours; 1 MB a row is 360 MB an hour.
        let text = sample_file(2000, 1, |n| if n == 1500 { (4000, 400) } else { (20, 0) });
        let r = report(&text).unwrap();
        assert!(r.contains("NewQ memory keeps growing"), "{r}");
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
}
