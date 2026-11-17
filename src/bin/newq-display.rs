//! NewQ Display: the launcher that makes a computer a display computer of a
//! show network with nothing to do on it. It finds NewQ and starts it with
//! `--display` (and any arguments it was given), then ends. Nothing else: no
//! window, no settings. Only when NewQ can't be found or started does it say
//! so, in a way a person at the computer sees.
//!
//! Where it looks (`find`): `NEWQ_PROGRAM` (for tests and odd set-ups); NewQ's
//! program next to its own (`newq` / `newq.exe`; on a Mac also inside its own
//! bundle); on a Mac, `NewQ.app` beside its bundle, then in `/Applications`
//! and `~/Applications`, then whatever NewQ the Mac knows of (`open -b`).

// No console window on Windows (as NewQ's own).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::process::Command;

/// The NewQ Mac app's bundle identifier (`scripts/package_release.sh`).
const BUNDLE_ID: &str = "io.github.r2a42.newq";

/// The systems that are looked for differently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Os {
    Windows,
    Mac,
    Other,
}

impl Os {
    fn here() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::Mac
        } else {
            Os::Other
        }
    }

    fn program(self) -> &'static str {
        match self {
            Os::Windows => "newq.exe",
            _ => "newq",
        }
    }
}

/// How NewQ is to be started.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Found {
    /// Its program, run directly.
    Program(PathBuf),
    /// A Mac's NewQ.app, opened (so the Mac starts it as its own app).
    App(PathBuf),
    /// Whatever NewQ.app the Mac knows of, by its bundle identifier.
    ByBundleId,
}

/// Where NewQ is, for this launcher at `me`: `env` is `NEWQ_PROGRAM`, `home`
/// the home folder, `exists` whether a file or folder is there. Pure, for the
/// tests.
fn find(
    me: &Path,
    os: Os,
    env: Option<PathBuf>,
    home: Option<PathBuf>,
    exists: impl Fn(&Path) -> bool,
) -> Option<Found> {
    if let Some(p) = env.filter(|p| !p.as_os_str().is_empty()) {
        return Some(Found::Program(p));
    }
    let dir = me.parent()?;
    // Next to itself (on a Mac, that is inside its own bundle).
    let beside = dir.join(os.program());
    if exists(&beside) {
        return Some(Found::Program(beside));
    }
    if os != Os::Mac {
        return None;
    }
    // `…/NewQ Display.app/Contents/MacOS/newq-display`: NewQ.app beside the bundle.
    let mut apps = Vec::new();
    if let Some(bundle) = dir
        .parent()
        .filter(|c| c.file_name().is_some_and(|n| n == "Contents"))
        .and_then(Path::parent)
        .filter(|b| b.extension().is_some_and(|e| e == "app"))
        && let Some(folder) = bundle.parent()
    {
        apps.push(folder.join("NewQ.app"));
    }
    apps.push(PathBuf::from("/Applications/NewQ.app"));
    if let Some(h) = home {
        apps.push(h.join("Applications/NewQ.app"));
    }
    for app in apps {
        if exists(&app.join("Contents/MacOS/newq")) {
            return Some(Found::App(app));
        }
    }
    // Moved somewhere else, or this launcher was opened from a copy the Mac
    // made (an app opened straight from Downloads is run from elsewhere).
    Some(Found::ByBundleId)
}

/// The command that starts NewQ as a display computer.
fn command(found: &Found, args: &[std::ffi::OsString]) -> Command {
    match found {
        Found::Program(p) => {
            let mut c = Command::new(p);
            c.arg("--display").args(args);
            c
        }
        Found::App(app) => {
            let mut c = Command::new("/usr/bin/open");
            c.arg("-a")
                .arg(app)
                .arg("--args")
                .arg("--display")
                .args(args);
            c
        }
        Found::ByBundleId => {
            let mut c = Command::new("/usr/bin/open");
            c.args(["-b", BUNDLE_ID, "--args", "--display"]).args(args);
            c
        }
    }
}

/// What to tell the person at the computer when NewQ can't be started.
fn not_found_words(os: Os) -> String {
    let keep = match os {
        Os::Mac => "Put NewQ Display and NewQ in the same folder (Applications is best)",
        Os::Windows => "Keep NewQ Display.exe in the same folder as newq.exe",
        Os::Other => "Keep newq-display in the same folder as newq",
    };
    format!("NewQ Display couldn't find NewQ on this computer. {keep}, then open it again.")
}

fn main() {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let os = Os::here();
    let me = std::env::current_exe().unwrap_or_default();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let env = std::env::var_os("NEWQ_PROGRAM").map(PathBuf::from);
    let Some(found) = find(&me, os, env, home, |p| p.exists()) else {
        tell(&not_found_words(os));
        std::process::exit(1);
    };
    let mut c = command(&found, &args);
    #[cfg(unix)]
    {
        // Its own process group: NewQ goes on when this launcher's terminal
        // (if any) is closed with Ctrl-C.
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
    c.stdin(std::process::Stdio::null());
    let started = match found {
        // `open` says whether the Mac could start it.
        Found::App(_) | Found::ByBundleId => match c.status() {
            Ok(s) if s.success() => Ok(()),
            Ok(_) if found == Found::ByBundleId => Err(not_found_words(os)),
            Ok(s) => Err(format!("the Mac couldn't open it ({s})")),
            Err(e) => Err(e.to_string()),
        },
        Found::Program(_) => c.spawn().map(|_| ()).map_err(|e| e.to_string()),
    };
    if let Err(why) = started {
        let text = if why.starts_with("NewQ Display") {
            why
        } else {
            format!("NewQ Display found NewQ but couldn't start it: {why}")
        };
        tell(&text);
        std::process::exit(1);
    }
}

/// Say `text` where a person sees it: a message box (Windows), a dialog
/// (Mac), or the terminal and a desktop notice or dialog (Linux).
fn tell(text: &str) {
    eprintln!("{text}");
    #[cfg(windows)]
    {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn MessageBoxW(
                hwnd: *mut core::ffi::c_void,
                text: *const u16,
                caption: *const u16,
                kind: u32,
            ) -> i32;
        }
        let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        let (t, c) = (wide(text), wide("NewQ Display"));
        // SAFETY: both strings are NUL-terminated and outlive the call; no owner window.
        // 0x10: MB_ICONERROR.
        unsafe {
            MessageBoxW(std::ptr::null_mut(), t.as_ptr(), c.as_ptr(), 0x10);
        }
    }
    #[cfg(target_os = "macos")]
    {
        let quoted = text.replace('\\', "\\\\").replace('"', "\\\"");
        let _ = Command::new("/usr/bin/osascript")
            .arg("-e")
            .arg(format!(
                "display dialog \"{quoted}\" with title \"NewQ Display\" buttons {{\"OK\"}} \
                 default button \"OK\" with icon caution"
            ))
            .status();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let shown = Command::new("zenity")
            .args(["--error", "--title=NewQ Display", "--no-wrap"])
            .arg(format!("--text={text}"))
            .status()
            .is_ok_and(|s| s.success());
        if !shown {
            let _ = Command::new("notify-send")
                .args(["NewQ Display", text])
                .status();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with<'a>(files: &'a [&'a str]) -> impl Fn(&Path) -> bool + 'a {
        move |p| files.iter().any(|f| Path::new(f) == p)
    }

    #[test]
    fn the_program_next_to_it_is_found_first_and_the_override_before_that() {
        let me = Path::new("/opt/newq/newq-display");
        let files = ["/opt/newq/newq"];
        assert_eq!(
            find(me, Os::Other, None, None, with(&files)),
            Some(Found::Program("/opt/newq/newq".into()))
        );
        assert_eq!(find(me, Os::Other, None, None, with(&[])), None);
        assert_eq!(
            find(me, Os::Other, Some("/x/newq".into()), None, with(&files)),
            Some(Found::Program("/x/newq".into()))
        );
        // An empty override is none.
        assert_eq!(
            find(me, Os::Other, Some("".into()), None, with(&files)),
            Some(Found::Program("/opt/newq/newq".into()))
        );
        let me = Path::new(r"C:\NewQ\NewQ Display.exe");
        let found = find(me, Os::Windows, None, None, |p| p.ends_with("newq.exe"));
        assert!(
            matches!(&found, Some(Found::Program(p)) if p.ends_with("newq.exe")),
            "{found:?}"
        );
    }

    #[test]
    fn on_a_mac_newq_app_is_looked_for_beside_the_bundle_then_in_applications() {
        let me = Path::new("/Users/a/Show/NewQ Display.app/Contents/MacOS/newq-display");
        let home = Some(PathBuf::from("/Users/a"));
        let beside = ["/Users/a/Show/NewQ.app/Contents/MacOS/newq"];
        assert_eq!(
            find(me, Os::Mac, None, home.clone(), with(&beside)),
            Some(Found::App("/Users/a/Show/NewQ.app".into()))
        );
        // Inside its own bundle first.
        let inside = [
            "/Users/a/Show/NewQ Display.app/Contents/MacOS/newq",
            "/Users/a/Show/NewQ.app/Contents/MacOS/newq",
        ];
        assert_eq!(
            find(me, Os::Mac, None, home.clone(), with(&inside)),
            Some(Found::Program(
                "/Users/a/Show/NewQ Display.app/Contents/MacOS/newq".into()
            ))
        );
        let apps = ["/Applications/NewQ.app/Contents/MacOS/newq"];
        assert_eq!(
            find(me, Os::Mac, None, home.clone(), with(&apps)),
            Some(Found::App("/Applications/NewQ.app".into()))
        );
        let mine = ["/Users/a/Applications/NewQ.app/Contents/MacOS/newq"];
        assert_eq!(
            find(me, Os::Mac, None, home.clone(), with(&mine)),
            Some(Found::App("/Users/a/Applications/NewQ.app".into()))
        );
        // Nowhere known: the Mac is asked for NewQ by its identifier.
        assert_eq!(
            find(me, Os::Mac, None, home, with(&[])),
            Some(Found::ByBundleId)
        );
    }

    #[test]
    fn newq_is_started_with_display_and_the_arguments_given() {
        let args = vec!["show.newq".into()];
        let c = command(&Found::Program("/opt/newq/newq".into()), &args);
        assert_eq!(c.get_program(), "/opt/newq/newq");
        assert_eq!(c.get_args().collect::<Vec<_>>(), ["--display", "show.newq"]);
        let c = command(&Found::App("/Applications/NewQ.app".into()), &args);
        assert_eq!(c.get_program(), "/usr/bin/open");
        assert_eq!(
            c.get_args().collect::<Vec<_>>(),
            [
                "-a",
                "/Applications/NewQ.app",
                "--args",
                "--display",
                "show.newq"
            ]
        );
        let c = command(&Found::ByBundleId, &[]);
        assert_eq!(
            c.get_args().collect::<Vec<_>>(),
            ["-b", BUNDLE_ID, "--args", "--display"]
        );
        for os in [Os::Windows, Os::Mac, Os::Other] {
            assert!(not_found_words(os).starts_with("NewQ Display couldn't find NewQ"));
        }
    }

    /// The launcher itself, built beside NewQ (`cargo build` makes both):
    /// started with `NEWQ_PROGRAM` pointing at a stand-in, it starts that with
    /// `--display` and the arguments, and ends.
    #[cfg(unix)]
    #[test]
    fn the_launcher_starts_the_program_with_display() {
        let me = std::env::current_exe().unwrap();
        // target/debug/deps/<test> -> target/debug/newq-display
        let Some(launcher) = me
            .parent()
            .and_then(Path::parent)
            .map(|d| d.join("newq-display"))
            .filter(|p| p.exists())
        else {
            eprintln!("skipped: the launcher isn't built next to the tests");
            return;
        };
        let dir = std::env::temp_dir().join(format!("newq-display-launch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("args.txt");
        let fake = dir.join("fake-newq");
        std::fs::write(
            &fake,
            format!(
                "#!/bin/sh\necho \"$@\" > \"{}.tmp\"\nmv \"{0}.tmp\" \"{0}\"\n",
                out.display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let status = Command::new(&launcher)
            .arg("my show.newq")
            .env("NEWQ_PROGRAM", &fake)
            .status()
            .unwrap();
        assert!(status.success());
        let began = std::time::Instant::now();
        while !out.exists() {
            assert!(began.elapsed().as_secs() < 30, "the stand-in never ran");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(
            std::fs::read_to_string(&out).unwrap(),
            "--display my show.newq\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
