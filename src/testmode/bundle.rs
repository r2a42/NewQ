//! One file to send: everything a test wrote, with NewQ's own log and crash
//! reports, in a zip beside the run's folder. Nothing is sent anywhere; the
//! window shows where the file is, and the person attaches it themselves.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The most of one file that goes in (the end of it, where a log is newest).
const MAX_PER_FILE: u64 = 24 * 1024 * 1024;

/// The zip being made on a thread, and what came of it.
#[derive(Clone, Default)]
pub struct Bundle(Arc<parking_lot::Mutex<Option<Result<PathBuf, String>>>>);

impl Bundle {
    /// None while it is being made.
    pub fn get(&self) -> Option<Result<PathBuf, String>> {
        self.0.lock().clone()
    }

    /// Make the zip for the run in `folder`. On a thread unless `wait` (the
    /// program is closing and there is no later).
    pub fn start(folder: PathBuf, log_dir: PathBuf, wait: bool) -> Bundle {
        let bundle = Bundle::default();
        let slot = bundle.0.clone();
        let work = move || {
            crate::logging::flush();
            let result = make(&folder, &log_dir);
            match &result {
                Ok(p) => log::info!("test mode: the files to send are in {}", p.display()),
                Err(e) => log::warn!("test mode: the zip could not be made: {e}"),
            }
            *slot.lock() = Some(result);
        };
        if wait {
            work();
        } else if let Err(e) = std::thread::Builder::new()
            .name("test-bundle".into())
            .spawn(work)
        {
            *bundle.0.lock() = Some(Err(format!("no thread to make it on: {e}")));
        }
        bundle
    }
}

/// Where the zip for the run in `folder` goes: beside the folder, with its name.
pub fn zip_path(folder: &Path) -> PathBuf {
    let name = folder
        .file_name()
        .map_or_else(|| "run".into(), |n| n.to_string_lossy().into_owned());
    folder.with_file_name(format!("NewQ-test-{name}.zip"))
}

/// The files of the run that go in: everything directly in its folder (not
/// the media it made or a recording, which are big and say nothing), then
/// NewQ's log, the log before it and the crash reports from the log folder.
pub fn contents(folder: &Path, log_dir: &Path) -> Vec<(String, PathBuf)> {
    let mut files: Vec<(String, PathBuf)> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        // (The excerpt is the same log again, which goes in whole.)
        .filter(|(name, _)| name != "newq-log-excerpt.txt" && !name.ends_with(".part"))
        .collect();
    files.sort();
    for name in ["newq.log", "newq.previous.log", "newq-crash.log"] {
        let path = log_dir.join(name);
        if path.is_file() && !files.iter().any(|(n, _)| n == name) {
            files.push((name.to_string(), path));
        }
    }
    files
}

/// Write the zip. It is made under another name and renamed, so a file with
/// the zip's name is always a whole one.
pub fn make(folder: &Path, log_dir: &Path) -> Result<PathBuf, String> {
    let out = zip_path(folder);
    let part = out.with_extension("zip.part");
    let file = std::fs::File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let mut options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    // Dated now (UTC), not the 1980 a zip has when nothing is said.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let (y, mo, d, h, mi) = crate::export::civil_from_unix(secs);
    if let Ok(when) = zip::DateTime::from_date_and_time(
        u16::try_from(y).unwrap_or(1980),
        mo as u8,
        d as u8,
        h as u8,
        mi as u8,
        (secs.rem_euclid(60)) as u8,
    ) {
        options = options.last_modified_time(when);
    }
    let mut listed = String::from("In this zip\n-----------\n");
    let mut added = 0;
    for (name, path) in contents(folder, log_dir) {
        // A file that can't be read (in use, gone) is left out, and said so.
        match tail_of(&path, MAX_PER_FILE) {
            Ok((bytes, whole)) => {
                let ok = zip
                    .start_file(name.as_str(), options)
                    .map_err(|e| e.to_string())
                    .and_then(|()| zip.write_all(&bytes).map_err(|e| e.to_string()));
                match ok {
                    Ok(()) => {
                        added += 1;
                        listed.push_str(&format!(
                            "{name}  ({} bytes{})\n",
                            bytes.len(),
                            if whole {
                                ""
                            } else {
                                ", the end of a longer file"
                            }
                        ));
                    }
                    Err(e) => listed.push_str(&format!("{name}  (left out: {e})\n")),
                }
            }
            Err(e) => listed.push_str(&format!("{name}  (left out: {e})\n")),
        }
    }
    zip.start_file("contents.txt", options)
        .map_err(|e| e.to_string())
        .and_then(|()| zip.write_all(listed.as_bytes()).map_err(|e| e.to_string()))?;
    zip.finish().map_err(|e| e.to_string())?;
    if added == 0 {
        let _ = std::fs::remove_file(&part);
        return Err("there were no files to put in it".into());
    }
    // (An older zip of the same run, should there be one, is replaced.)
    let _ = std::fs::remove_file(&out);
    std::fs::rename(&part, &out).map_err(|e| format!("{}: {e}", out.display()))?;
    Ok(out)
}

/// The last `max` bytes of a file, and whether that is all of it.
fn tail_of(path: &Path, max: u64) -> std::io::Result<(Vec<u8>, bool)> {
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    let from = len.saturating_sub(max);
    f.seek(SeekFrom::Start(from))?;
    let mut bytes = Vec::with_capacity((len - from) as usize);
    f.take(max).read_to_end(&mut bytes)?;
    Ok((bytes, from == 0))
}

/// The program and arguments that show `file` selected in the system's file
/// manager (Explorer, the Finder), or its folder where there is no such thing.
pub fn reveal_command(os: crate::crash::Os, file: &Path) -> (String, Vec<String>, Option<String>) {
    use crate::crash::Os;
    match os {
        // Explorer wants `/select,"path"` as it is written, quotes and all:
        // the third part is that argument, to be passed without re-quoting.
        Os::Windows => (
            "explorer.exe".into(),
            Vec::new(),
            Some(format!("/select,\"{}\"", file.display())),
        ),
        Os::Mac => (
            "open".into(),
            vec!["-R".into(), file.display().to_string()],
            None,
        ),
        Os::Other => (
            "xdg-open".into(),
            vec![
                file.parent()
                    .unwrap_or(Path::new("."))
                    .display()
                    .to_string(),
            ],
            None,
        ),
    }
}

/// Show `file` in the system's file manager.
pub fn reveal(file: &Path) -> Result<(), String> {
    let (program, args, raw) = reveal_command(crate::crash::Os::current(), file);
    let mut cmd = std::process::Command::new(program);
    cmd.args(args);
    #[cfg(windows)]
    if let Some(raw) = &raw {
        std::os::windows::process::CommandExt::raw_arg(&mut cmd, raw);
    }
    #[cfg(not(windows))]
    let _ = raw;
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Can't show {}: {e}", file.display()))
}

/// The settings in effect, for the bundle: this computer's settings as they
/// are saved, the log level in use, and what the environment asks of NewQ.
pub fn settings_text(prefs: &crate::prefs::Prefs, env: &[(String, String)]) -> String {
    let mut out = String::from("Settings of this computer (prefs.json)\n");
    out.push_str(
        &serde_json::to_string_pretty(prefs).unwrap_or_else(|e| format!("(not readable: {e})")),
    );
    out.push_str(&format!(
        "\n\nLog detail in use: {}{}\n",
        crate::logging::level().name(),
        if crate::logging::forced().is_some() {
            " (set by NEWQ_LOG)"
        } else {
            ""
        }
    ));
    out.push_str("\nEnvironment\n");
    let ours: Vec<&(String, String)> = env.iter().filter(|(k, _)| env_shown(k)).collect();
    if ours.is_empty() {
        out.push_str("  (nothing set that NewQ reads)\n");
    }
    for (k, v) in ours {
        out.push_str(&format!("  {k}={v}\n"));
    }
    out
}

/// The variables that change what NewQ does: its own, and the graphics library's.
fn env_shown(name: &str) -> bool {
    name.starts_with("NEWQ_") || name.starts_with("WGPU_") || name == "RUST_LOG"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("newq_bundle_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn names_in(zip: &Path) -> Vec<String> {
        let mut z = zip::ZipArchive::new(std::fs::File::open(zip).unwrap()).unwrap();
        (0..z.len())
            .map(|i| z.by_index(i).unwrap().name().unwrap().to_string())
            .collect()
    }

    fn text_in(zip: &Path, name: &str) -> String {
        let mut z = zip::ZipArchive::new(std::fs::File::open(zip).unwrap()).unwrap();
        let mut s = String::new();
        z.by_name(name).unwrap().read_to_string(&mut s).unwrap();
        s
    }

    #[test]
    fn the_zip_has_the_runs_files_the_log_and_the_crash_reports_and_not_the_media() {
        let root = temp("make");
        let logs = root.join("logs");
        let run = logs
            .join("test-mode")
            .join("runs")
            .join("2026-11-11_19-00-00_full");
        std::fs::create_dir_all(run.join("media")).unwrap();
        std::fs::create_dir_all(run.join("recording")).unwrap();
        for (name, text) in [
            ("report.txt", "Result: Passed"),
            ("events.log", "[    0.000] began"),
            ("system.txt", "NewQ 0.19.1"),
            ("settings.txt", "{}"),
            ("stats.csv", "t,rss"),
            ("newq-log-excerpt.txt", "the end of the log"),
        ] {
            std::fs::write(run.join(name), text).unwrap();
        }
        std::fs::write(run.join("media").join("clip.mp4"), vec![0u8; 4096]).unwrap();
        std::fs::write(run.join("recording").join("take.mkv"), vec![0u8; 4096]).unwrap();
        std::fs::write(
            logs.join("newq.log"),
            "2026-11-11 19:00:00.000 INFO  [main] x",
        )
        .unwrap();
        std::fs::write(logs.join("newq-crash.log"), "NewQ 0.19.1 crashed").unwrap();
        let zip = make(&run, &logs).expect("a zip");
        assert_eq!(
            zip,
            logs.join("test-mode/runs/NewQ-test-2026-11-11_19-00-00_full.zip")
        );
        let mut names = names_in(&zip);
        names.sort();
        assert_eq!(
            names,
            [
                "contents.txt",
                "events.log",
                "newq-crash.log",
                "newq.log",
                "report.txt",
                "settings.txt",
                "stats.csv",
                "system.txt"
            ]
        );
        assert_eq!(text_in(&zip, "report.txt"), "Result: Passed");
        assert!(text_in(&zip, "newq.log").contains("INFO  [main] x"));
        let listed = text_in(&zip, "contents.txt");
        assert!(listed.contains("report.txt  (14 bytes)"), "{listed}");
        // No half-made file is left, and making it again replaces it.
        assert!(!zip.with_extension("zip.part").exists());
        std::fs::write(run.join("report.txt"), "Result: Problems found").unwrap();
        let again = make(&run, &logs).unwrap();
        assert_eq!(text_in(&again, "report.txt"), "Result: Problems found");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_run_with_nothing_in_it_or_nowhere_to_write_is_an_error_not_a_panic() {
        let root = temp("empty");
        let run = root.join("runs").join("empty_run");
        std::fs::create_dir_all(&run).unwrap();
        let err = make(&run, &root.join("no-logs")).unwrap_err();
        assert!(err.contains("no files"), "{err}");
        assert!(!zip_path(&run).exists());
        // The folder the zip would go in isn't there.
        let gone = root.join("nowhere").join("deeper").join("run");
        assert!(make(&gone, &root).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_long_log_goes_in_by_its_end() {
        let root = temp("tail");
        let f = root.join("long.log");
        std::fs::write(&f, b"0123456789").unwrap();
        assert_eq!(tail_of(&f, 4).unwrap(), (b"6789".to_vec(), false));
        assert_eq!(tail_of(&f, 10).unwrap(), (b"0123456789".to_vec(), true));
        assert!(tail_of(&f, 100).unwrap().1);
        assert!(tail_of(&root.join("missing"), 4).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_file_is_shown_selected_in_explorer_and_the_finder() {
        use crate::crash::Os;
        let (p, args, raw) = reveal_command(Os::Windows, Path::new("C:\\Users\\a b\\NewQ\\t.zip"));
        assert_eq!(p, "explorer.exe");
        assert!(args.is_empty());
        // Quoted as Explorer reads it, spaces and all.
        assert_eq!(
            raw.as_deref(),
            Some("/select,\"C:\\Users\\a b\\NewQ\\t.zip\"")
        );
        let (p, args, raw) = reveal_command(Os::Mac, Path::new("/Users/a/Library/Logs/NewQ/t.zip"));
        assert_eq!((p.as_str(), raw), ("open", None));
        assert_eq!(args, ["-R", "/Users/a/Library/Logs/NewQ/t.zip"]);
        // Elsewhere there is no way to select a file that every desktop has: its folder.
        let (p, args, _) = reveal_command(Os::Other, Path::new("/home/a/.local/state/newq/t.zip"));
        assert_eq!(p, "xdg-open");
        assert_eq!(args, ["/home/a/.local/state/newq"]);
    }

    #[test]
    fn the_settings_in_effect_are_written_out_without_what_is_not_newqs() {
        let prefs = crate::prefs::Prefs {
            hw_decode: true,
            ..Default::default()
        };
        let env = vec![
            ("NEWQ_LOG".to_string(), "debug".to_string()),
            ("WGPU_BACKEND".to_string(), "vulkan".to_string()),
            ("HOME".to_string(), "/home/someone".to_string()),
            ("SECRET_TOKEN".to_string(), "abc".to_string()),
        ];
        let text = settings_text(&prefs, &env);
        assert!(text.contains("\"hw_decode\": true"), "{text}");
        assert!(text.contains("NEWQ_LOG=debug") && text.contains("WGPU_BACKEND=vulkan"));
        assert!(
            !text.contains("SECRET_TOKEN") && !text.contains("/home/someone"),
            "{text}"
        );
        assert!(settings_text(&prefs, &[]).contains("nothing set that NewQ reads"));
    }
}
