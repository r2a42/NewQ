//! Turning a PowerPoint (or Keynote, or OpenDocument) presentation into one
//! picture per slide, with whatever program on this computer can draw them.
//!
//! NewQ can't draw a slide itself: fonts, themes, smart art and effects are the
//! office program's business. So it asks one:
//!
//! - **Windows:** PowerPoint, through its COM interface (PowerShell).
//! - **Mac:** PowerPoint, or Keynote, through AppleScript.
//! - **Any:** LibreOffice makes a PDF and `pdftoppm` (poppler) makes the pictures.
//! - **Your own:** a program named by `NEWQ_SLIDE_EXPORTER` is run as
//!   `program <presentation> <folder>` and must leave the pictures in the folder.
//!
//! Each is tried in turn until one leaves pictures behind. None of them has been
//! tried by the author on its own platform: what each says when it fails is
//! kept and shown, so a failure says why.

use crate::media::ffmpeg::output_cancellable;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How long one program is given to make all the pictures.
const LIMIT: Duration = Duration::from_secs(300);

/// What was made.
#[derive(Debug)]
pub struct Exported {
    /// The pictures, in slide order.
    pub files: Vec<PathBuf>,
    /// Which program made them.
    pub how: String,
}

#[derive(Clone, Debug, PartialEq)]
enum Tool {
    Custom(PathBuf),
    PowerPointWindows,
    PowerPointMac,
    Keynote,
    LibreOffice,
}

impl Tool {
    fn name(&self) -> &'static str {
        match self {
            Tool::Custom(_) => "the program named by NEWQ_SLIDE_EXPORTER",
            Tool::PowerPointWindows | Tool::PowerPointMac => "PowerPoint",
            Tool::Keynote => "Keynote",
            Tool::LibreOffice => "LibreOffice",
        }
    }
}

/// The programs to try for a file with this extension, best first.
fn tools_for(ext: &str, custom: Option<PathBuf>) -> Vec<Tool> {
    let ext = ext.to_lowercase();
    let mut tools = Vec::new();
    if let Some(c) = custom {
        tools.push(Tool::Custom(c));
    }
    let office = ["pptx", "ppt", "pps", "ppsx", "pptm", "potx", "odp"].contains(&ext.as_str());
    if cfg!(windows) && office && ext != "odp" {
        tools.push(Tool::PowerPointWindows);
    }
    if cfg!(target_os = "macos") {
        if office && ext != "odp" {
            tools.push(Tool::PowerPointMac);
        }
        if office || ext == "key" {
            tools.push(Tool::Keynote);
        }
    }
    if office || ext == "pdf" {
        tools.push(Tool::LibreOffice);
    }
    tools
}

/// Pictures (png, jpg) under `dir`, in the order the slides come: by the
/// numbers in their names ("Slide2" before "Slide10").
fn pictures_in(dir: &Path) -> Vec<PathBuf> {
    // Two levels deep at most, and links are not followed (a loop of them
    // would never end).
    fn walk(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            let Ok(kind) = e.file_type() else { continue };
            if kind.is_dir() {
                if depth < 2 {
                    walk(&p, depth + 1, out);
                }
            } else if kind.is_file()
                && p.extension()
                    .and_then(|x| x.to_str())
                    .is_some_and(|x| ["png", "jpg", "jpeg"].contains(&x.to_lowercase().as_str()))
            {
                out.push(p);
            }
        }
    }
    let mut found = Vec::new();
    walk(dir, 0, &mut found);
    found.sort_by_key(|p| natural_key(p));
    found
}

/// A name as text and numbers, so that numbers sort as numbers.
fn natural_key(p: &Path) -> Vec<(String, u64)> {
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let mut key = Vec::new();
    let (mut text, mut digits) = (String::new(), String::new());
    for c in name.chars() {
        if c.is_ascii_digit() {
            digits.push(c);
        } else {
            if !digits.is_empty() {
                key.push((std::mem::take(&mut text), digits.parse().unwrap_or(0)));
                digits.clear();
            }
            text.push(c);
        }
    }
    key.push((text, digits.parse().unwrap_or(0)));
    key
}

/// A program by name: on the PATH, or at one of the usual places.
fn find_program(names: &[&str], extra: &[&str]) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    dirs.extend(extra.iter().map(PathBuf::from));
    for d in dirs {
        for n in names {
            let candidate = d.join(n);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    for e in extra {
        let p = PathBuf::from(e);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// Run `cmd` to its end, but no longer than `LIMIT` and not past `cancel`.
fn run(cmd: &mut Command, cancel: &Arc<AtomicBool>) -> Result<std::process::Output, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let watcher = {
        let (stop, finished, cancel) = (stop.clone(), finished.clone(), cancel.clone());
        std::thread::spawn(move || {
            let began = Instant::now();
            while !finished.load(Ordering::Relaxed) {
                if cancel.load(Ordering::Relaxed) || began.elapsed() > LIMIT {
                    stop.store(true, Ordering::Relaxed);
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        })
    };
    let out = output_cancellable(cmd, &stop);
    finished.store(true, Ordering::Relaxed);
    let _ = watcher.join();
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    if stop.load(Ordering::Relaxed) {
        return Err(format!(
            "it took longer than {} s (if PowerPoint or Keynote is still open with the file, close it before trying again)",
            LIMIT.as_secs()
        ));
    }
    out.map_err(|e| format!("couldn't be started: {e}"))
}

fn failure_text(out: &std::process::Output) -> String {
    let mut text = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if text.is_empty() {
        text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    }
    if text.chars().count() > 400 {
        text = text.chars().take(400).collect::<String>() + "…";
    }
    if text.is_empty() {
        format!("it stopped ({})", out.status)
    } else {
        text
    }
}

/// Make the pictures with `tool`, into `out_dir`.
fn export_with(
    tool: &Tool,
    input: &Path,
    out_dir: &Path,
    cancel: &Arc<AtomicBool>,
) -> Result<(), String> {
    let ok = |out: std::process::Output| {
        if out.status.success() {
            Ok(())
        } else {
            Err(failure_text(&out))
        }
    };
    match tool {
        Tool::Custom(program) => ok(run(Command::new(program).arg(input).arg(out_dir), cancel)?),
        Tool::PowerPointWindows => {
            const SCRIPT: &str = "$ErrorActionPreference = 'Stop'; \
                $app = New-Object -ComObject PowerPoint.Application; \
                try { $pres = $app.Presentations.Open($env:NEWQ_IN, -1, 0, 0); \
                try { $pres.Export($env:NEWQ_OUT, 'PNG', 1920, 1080) } finally { $pres.Close() } } \
                finally { $app.Quit() }";
            let mut cmd = Command::new("powershell");
            cmd.args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                SCRIPT,
            ])
            .env("NEWQ_IN", input)
            .env("NEWQ_OUT", out_dir);
            crate::media::ffmpeg::no_console(&mut cmd);
            ok(run(&mut cmd, cancel)?)
        }
        Tool::PowerPointMac => {
            if !Path::new("/Applications/Microsoft PowerPoint.app").exists() {
                return Err("Microsoft PowerPoint isn't installed".into());
            }
            const SCRIPT: &str = "on run argv\n\
                set inFile to POSIX file (item 1 of argv)\n\
                set outDir to POSIX file (item 2 of argv)\n\
                tell application \"Microsoft PowerPoint\"\n\
                open inFile\n\
                set p to active presentation\n\
                save p in outDir as save as PNG\n\
                close p saving no\n\
                end tell\n\
                end run";
            ok(run(
                Command::new("/usr/bin/osascript")
                    .args(["-e", SCRIPT])
                    .arg(input)
                    .arg(out_dir),
                cancel,
            )?)
        }
        Tool::Keynote => {
            if !Path::new("/Applications/Keynote.app").exists() {
                return Err("Keynote isn't installed".into());
            }
            const SCRIPT: &str = "on run argv\n\
                set inFile to POSIX file (item 1 of argv)\n\
                set outDir to POSIX file (item 2 of argv)\n\
                tell application \"Keynote\"\n\
                set theDoc to open inFile\n\
                export theDoc to outDir as slide images with properties {image format:PNG, skipped slides:false}\n\
                close theDoc saving no\n\
                end tell\n\
                end run";
            ok(run(
                Command::new("/usr/bin/osascript")
                    .args(["-e", SCRIPT])
                    .arg(input)
                    .arg(out_dir),
                cancel,
            )?)
        }
        Tool::LibreOffice => {
            let pdf_dir = out_dir.join(".pdf");
            std::fs::create_dir_all(&pdf_dir).map_err(|e| e.to_string())?;
            let is_pdf = input
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
            let pdf = if is_pdf {
                input.to_path_buf()
            } else {
                let soffice = find_program(
                    &["soffice", "soffice.exe", "libreoffice"],
                    &[
                        "/Applications/LibreOffice.app/Contents/MacOS/soffice",
                        r"C:\Program Files\LibreOffice\program\soffice.exe",
                        r"C:\Program Files (x86)\LibreOffice\program\soffice.exe",
                    ],
                )
                .ok_or("LibreOffice isn't installed")?;
                let mut cmd = Command::new(soffice);
                cmd.args(["--headless", "--convert-to", "pdf", "--outdir"])
                    .arg(&pdf_dir)
                    .arg(input);
                crate::media::ffmpeg::no_console(&mut cmd);
                ok(run(&mut cmd, cancel)?)?;
                let stem = input.file_stem().unwrap_or_default();
                pdf_dir.join(stem).with_extension("pdf")
            };
            let pdftoppm = find_program(
                &["pdftoppm", "pdftoppm.exe"],
                &["/opt/homebrew/bin/pdftoppm", "/usr/local/bin/pdftoppm"],
            )
            .ok_or("pdftoppm (part of poppler) isn't installed, so a PDF can't be turned into pictures")?;
            let mut cmd = Command::new(pdftoppm);
            cmd.args(["-png", "-r", "110"])
                .arg(&pdf)
                .arg(out_dir.join("slide"));
            crate::media::ffmpeg::no_console(&mut cmd);
            let result = ok(run(&mut cmd, cancel)?);
            let _ = std::fs::remove_dir_all(&pdf_dir);
            result
        }
    }
}

/// Make a picture of each slide of `input` in `out_dir` (made if need be, and
/// emptied of pictures from an earlier go).
pub fn export(input: &Path, out_dir: &Path, cancel: &Arc<AtomicBool>) -> Result<Exported, String> {
    let ext = input
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_default();
    let custom = std::env::var_os("NEWQ_SLIDE_EXPORTER").map(PathBuf::from);
    let tools = tools_for(&ext, custom);
    if tools.is_empty() {
        return Err(format!(
            "NewQ doesn't know how to get slides out of a .{ext} file (it takes PowerPoint, Keynote, OpenDocument and PDF files)"
        ));
    }
    let mut tried = Vec::new();
    for tool in tools {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        // Start from nothing, so another tool's leftovers aren't taken for its pictures.
        for old in pictures_in(out_dir) {
            let _ = std::fs::remove_file(old);
        }
        std::fs::create_dir_all(out_dir)
            .map_err(|e| format!("can't make the folder {}: {e}", out_dir.display()))?;
        match export_with(&tool, input, out_dir, cancel) {
            Ok(()) => {
                let files = pictures_in(out_dir);
                if files.is_empty() {
                    tried.push(format!("{}: it made no pictures", tool.name()));
                } else {
                    return Ok(Exported {
                        files,
                        how: tool.name().to_string(),
                    });
                }
            }
            Err(e) if e == "cancelled" => return Err(e),
            Err(e) => tried.push(format!("{}: {e}", tool.name())),
        }
    }
    Err(format!(
        "No program on this computer could turn the slides into pictures. {} \
         Install PowerPoint, Keynote, or LibreOffice with poppler's pdftoppm; or in your \
         presentation program use Export ▸ PNG and add those pictures to a slide show.",
        tried.join("; ")
    ))
}

/// Which of the exported pictures are shown, and each one's timing, given the
/// slides the file describes: (picture, advance, fade). Hidden slides are left
/// out; when the number of pictures matches neither all the slides nor the
/// ones not hidden, the file's timings are not used.
pub fn match_timings(
    files: Vec<PathBuf>,
    slides: &[crate::pptx::Slide],
) -> Vec<(PathBuf, Option<f64>, Option<f64>)> {
    let visible: Vec<&crate::pptx::Slide> = slides.iter().filter(|s| !s.hidden).collect();
    let (kept, with): (Vec<PathBuf>, Vec<Option<&crate::pptx::Slide>>) =
        if files.len() == slides.len() {
            files
                .into_iter()
                .zip(slides)
                .filter(|(_, s)| !s.hidden)
                .map(|(f, s)| (f, Some(s)))
                .unzip()
        } else if files.len() == visible.len() {
            files
                .into_iter()
                .zip(visible)
                .map(|(f, s)| (f, Some(s)))
                .unzip()
        } else {
            files.into_iter().map(|f| (f, None)).unzip()
        };
    // If some slides have a transition and some don't, those without are cuts.
    let any_fade = with.iter().flatten().any(|s| s.fade.is_some());
    kept.into_iter()
        .zip(with)
        .map(|(f, s)| {
            let advance = s.and_then(|s| s.advance);
            let fade = s.and_then(|s| s.fade.or(any_fade.then_some(0.0)));
            (f, advance, fade)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pptx::Slide;

    fn slide(hidden: bool, advance: Option<f64>, fade: Option<f64>) -> Slide {
        Slide {
            hidden,
            advance,
            fade,
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_loop_of_links_in_the_folder_is_not_followed() {
        let dir = std::env::temp_dir().join(format!("newq_loop_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Slide1.png"), "x").unwrap();
        std::os::unix::fs::symlink(&dir, dir.join("again")).unwrap();
        assert_eq!(pictures_in(&dir).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pictures_sort_by_the_numbers_in_their_names() {
        let dir = std::env::temp_dir().join(format!("newq_pics_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        for n in ["Slide10.PNG", "Slide2.PNG", "Slide1.PNG"] {
            std::fs::write(dir.join(n), "x").unwrap();
        }
        std::fs::write(dir.join("sub").join("Slide3.png"), "x").unwrap();
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        let names: Vec<String> = pictures_in(&dir)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            ["Slide1.PNG", "Slide2.PNG", "Slide3.png", "Slide10.PNG"]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_programs_tried_depend_on_the_file_and_a_custom_one_comes_first() {
        let t = tools_for("pdf", None);
        assert_eq!(t, vec![Tool::LibreOffice]);
        let t = tools_for("PPTX", Some(PathBuf::from("/x/mine")));
        assert_eq!(t[0], Tool::Custom(PathBuf::from("/x/mine")));
        assert_eq!(*t.last().unwrap(), Tool::LibreOffice);
        assert!(tools_for("docx", None).is_empty());
        assert!(!tools_for("key", None).contains(&Tool::LibreOffice));
    }

    #[test]
    fn hidden_slides_are_left_out_and_timings_follow_their_pictures() {
        let files = |n: usize| -> Vec<PathBuf> {
            (1..=n)
                .map(|i| PathBuf::from(format!("s{i}.png")))
                .collect()
        };
        let slides = [
            slide(false, Some(4.0), Some(1.0)),
            slide(true, Some(9.0), Some(2.0)),
            slide(false, None, None),
        ];
        // A picture for every slide, hidden ones too: the hidden one goes.
        let m = match_timings(files(3), &slides);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0], (PathBuf::from("s1.png"), Some(4.0), Some(1.0)));
        // This slide has no transition but others do: it is a cut.
        assert_eq!(m[1], (PathBuf::from("s3.png"), None, Some(0.0)));
        // Pictures for the visible ones only (Keynote skips them): same result.
        let m = match_timings(files(2), &slides);
        assert_eq!(m[1], (PathBuf::from("s2.png"), None, Some(0.0)));
        // Any other number: no way to tell which is which, so no timings.
        let m = match_timings(files(5), &slides);
        assert_eq!(m.len(), 5);
        assert!(m.iter().all(|(_, a, f)| a.is_none() && f.is_none()));
        // A deck with no transitions at all leaves the fades to the slide show.
        let plain = [slide(false, Some(3.0), None), slide(false, None, None)];
        let m = match_timings(files(2), &plain);
        assert_eq!(m[0].2, None);
        assert_eq!(m[0].1, Some(3.0));
    }

    #[cfg(unix)]
    #[test]
    fn a_program_of_your_own_can_make_the_pictures_and_a_failure_says_why() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("newq_exp_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("deck.pptx");
        std::fs::write(&input, "x").unwrap();
        let good = dir.join("good.sh");
        std::fs::write(
            &good,
            "#!/bin/sh\nfor n in 1 2 3; do echo p > \"$2/slide-$n.png\"; done\n",
        )
        .unwrap();
        let bad = dir.join("bad.sh");
        std::fs::write(&bad, "#!/bin/sh\necho 'no PowerPoint here' >&2\nexit 3\n").unwrap();
        for f in [&good, &bad] {
            std::fs::set_permissions(f, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let out = dir.join("out");
        let done = export_with(&Tool::Custom(good.clone()), &input, &out, &cancel);
        // export_with makes nothing itself: the folder is made by `export`.
        assert!(done.is_err() || pictures_in(&out).len() == 3);
        std::fs::create_dir_all(&out).unwrap();
        export_with(&Tool::Custom(good), &input, &out, &cancel).unwrap();
        assert_eq!(pictures_in(&out).len(), 3);
        let err = export_with(&Tool::Custom(bad), &input, &out, &cancel).unwrap_err();
        assert!(err.contains("no PowerPoint here"), "{err}");
        // Cancelled before it starts.
        cancel.store(true, Ordering::Relaxed);
        let slow = dir.join("slow.sh");
        std::fs::write(&slow, "#!/bin/sh\nsleep 30\n").unwrap();
        std::fs::set_permissions(&slow, std::fs::Permissions::from_mode(0o755)).unwrap();
        let began = Instant::now();
        let err = export_with(&Tool::Custom(slow), &input, &out, &cancel).unwrap_err();
        assert_eq!(err, "cancelled");
        assert!(began.elapsed() < Duration::from_secs(10));
        let _ = std::fs::remove_dir_all(dir);
    }
}
