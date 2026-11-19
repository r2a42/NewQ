// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod adapters;
mod app;
mod autostart;
mod awake;
mod bench;
#[cfg(test)]
mod blend;
mod branding;
mod bulk_edit;
mod clicker;
mod clock;
mod color;
mod crash;
mod crumbs;
mod dock;
mod edid;
mod engine;
mod export;
mod feedback;
mod fps;
mod freeze;
mod fx;
mod gpu;
mod history;
mod live_text;
mod logging;
#[cfg(target_os = "macos")]
mod macwin;
mod media;
mod mediamtx;
mod mesh;
mod monitor;
mod ndi;
mod net;
mod output;
mod pattern;
mod pdf;
mod power;
mod pptx;
mod prefs;
mod projector;
mod reaper;
mod recovery;
mod relink;
mod remote;
mod render;
mod shape;
mod show;
mod slides_export;
mod soak;
mod speed;
mod splash;
mod stats;
mod stream;
mod streamlink;
mod testmode;
mod text;
mod timecode;
mod timeline;
mod ui;
mod upload;
mod web;

use winit::event_loop::EventLoop;

/// The show to open (the first argument that isn't an option) and whether to
/// keep every output in the main window (`--main-window-only`, or asked for by
/// the environment).
fn startup_options(
    args: impl Iterator<Item = std::ffi::OsString>,
    env_asks: bool,
) -> (Option<std::path::PathBuf>, bool) {
    let mut show = None;
    let mut main_only = env_asks;
    for a in args {
        if a == "--main-window-only" {
            main_only = true;
        } else if show.is_none() && !a.to_string_lossy().starts_with("--") {
            show = Some(std::path::PathBuf::from(a));
        }
    }
    (show, main_only)
}

/// Hold the node folder's lock (`net::node::lock_node_dir`). Another NewQ
/// holding it: say so and end (the exit code), or, started with nobody to
/// read a dialog (`net::node::when_held`), wait for it to close. A lock that
/// can't be tried is no reason not to start.
fn hold_node_folder() -> Result<Option<net::node::NodeLock>, i32> {
    use net::node::WhenHeld;
    let dir = net::node::node_dir();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let when = net::node::when_held(
        std::env::var_os(net::node::RESTARTING_ENV).is_some(),
        net::node::startup_flag(&args) == Some(net::node::Startup::Display),
    );
    // Waits for as long as the other takes to close, saying so on the way
    // (the log isn't begun until the folder is held).
    let wait_it_out = || {
        let said = |so_far: std::time::Duration| {
            eprintln!(
                "NewQ: waiting for the NewQ that holds {} to close ({} s so far)",
                dir.display(),
                so_far.as_secs()
            )
        };
        match net::node::wait_for_node_dir(&dir, std::time::Duration::from_secs(30), said) {
            Ok(lock) => Ok(Some(lock)),
            Err(e) => {
                eprintln!(
                    "NewQ couldn't lock {} ({e}); starting anyway",
                    dir.display()
                );
                Ok(None)
            }
        }
    };
    // Started again by its primary: the old copy is still closing.
    let wait = if when == WhenHeld::Wait {
        std::time::Duration::from_secs(20)
    } else {
        std::time::Duration::ZERO
    };
    match net::node::lock_node_dir(&dir, wait) {
        Ok(lock) => Ok(Some(lock)),
        Err(net::node::LockRefused::Held) if when == WhenHeld::Wait => wait_it_out(),
        Err(net::node::LockRefused::Held) => {
            // Hand the show (if one was given) to the open NewQ, which looks
            // for requests about twice a second and comes to the front.
            let (show, _) = startup_options(std::env::args_os().skip(1), false);
            let show = show.map(|p| std::path::absolute(&p).unwrap_or(p));
            let left = net::node::hand_over(&dir, &net::node::Handoff { show: show.clone() });
            let began = std::time::Instant::now();
            let taken = match &left {
                Ok(path) => loop {
                    if !path.exists() {
                        break true;
                    }
                    if began.elapsed() > std::time::Duration::from_secs(3) {
                        // Not taken: it isn't left to surprise a later NewQ.
                        let _ = std::fs::remove_file(path);
                        break false;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                },
                Err(_) => false,
            };
            // Not taken: the other one may be closing (a restart whose new
            // copy wasn't marked, a quit under way). It is waited for a
            // little; once it has gone this copy starts as usual.
            if !taken
                && let Ok(lock) = net::node::lock_node_dir(&dir, std::time::Duration::from_secs(5))
            {
                eprintln!("the NewQ that was open has closed: this one starts");
                return Ok(Some(lock));
            }
            // A display computer's start: the open NewQ came forward, or it
            // is closing and this one starts once it has.
            if when == WhenHeld::HandOverOrWait && !taken {
                return wait_it_out();
            }
            let words = net::node::already_open_words(show.as_deref(), taken);
            eprintln!("{} ({})", words.replace("\n\n", " "), dir.display());
            if when == WhenHeld::HandOverOrWait {
                return Err(3);
            }
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Info)
                .set_title("NewQ is already open")
                .set_description(words)
                .show();
            Err(3)
        }
        Err(net::node::LockRefused::Untried(e)) => {
            eprintln!(
                "NewQ couldn't lock {} ({e}); starting anyway",
                dir.display()
            );
            Ok(None)
        }
    }
}

fn main() -> anyhow::Result<()> {
    // A copy of this program started to talk to NDI's library for the show
    // (see ndi.rs): it does that and nothing else.
    let args: Vec<String> = std::env::args().skip(1).collect();
    // The watcher that ends what NewQ started once NewQ is gone (see reaper.rs).
    if let Some(code) = reaper::run_if_asked(&args) {
        std::process::exit(code);
    }
    if let Some(code) = ndi::run_helper_if_asked(&args) {
        std::process::exit(code);
    }
    // The video benchmark (see bench.rs) is a command line tool, not the app.
    if let Some(code) = bench::run_if_asked(&args) {
        std::process::exit(code);
    }
    if let Some(code) = stats::run_if_asked(&args) {
        std::process::exit(code);
    }
    if let Some(code) = soak::run_if_asked(&args) {
        std::process::exit(code);
    }
    // The full check with no window (see testmode/selftest.rs).
    if let Some(code) = testmode::selftest::run_if_asked(&args) {
        std::process::exit(code);
    }
    // One NewQ per node folder (this computer's place in a show network):
    // taken before the log is begun, so a second copy doesn't put the open
    // one's log aside, and before `node.json` is written.
    let node_lock = match hold_node_folder() {
        Ok(lock) => lock,
        Err(code) => std::process::exit(code),
    };
    // The log goes to a file in the log folder (and to the console where
    // there is one), at the level of `NEWQ_LOG` or, once the settings of
    // this computer are read, the one chosen there.
    let has_file = logging::init(&crash::log_dir());
    crash::install();
    reaper::start();
    log::info!(
        "NewQ {} on {} ({}), logs in {}{}, log detail: {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        crash::log_dir().display(),
        if has_file {
            ""
        } else {
            " (which can't be written to: no log file)"
        },
        logging::level().name()
    );
    if node_lock.is_none() {
        log::warn!(
            "the node folder couldn't be locked: a second NewQ on this computer wouldn't be stopped"
        );
    }
    // `--join`, `--primary` and `--display`: this computer's place in a show
    // network. `--display` (what NewQ Display starts) makes it a display
    // computer from the first moment: no splash, no control window, no question.
    let mut display = false;
    if let Some(flag) = net::node::startup_flag(&args) {
        let applied = net::node::apply_startup(flag, &net::node::node_dir());
        log::info!("{}", applied.said);
        display = applied.display;
    }
    let (show_path, main_window_only) = startup_options(
        std::env::args_os().skip(1),
        std::env::var_os("NEWQ_MAIN_WINDOW_ONLY").is_some(),
    );
    if main_window_only {
        log::info!("main window only: no output windows will be opened");
    }
    // The user event is how remote-control threads wake the loop.
    let event_loop = EventLoop::<()>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let mut app = app::App::new(show_path, main_window_only, display, move || {
        let _ = proxy.send_event(());
    });
    let ran = event_loop.run_app(&mut app);
    log::info!("NewQ is closing");
    logging::flush();
    // Held until here: a NewQ started again may take it now.
    drop(node_lock);
    ran?;
    if let Some(why) = app.start_failed() {
        anyhow::bail!("{why}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::startup_options;
    use std::ffi::OsString;
    use std::path::PathBuf;

    fn opts(args: &[&str], env: bool) -> (Option<PathBuf>, bool) {
        startup_options(args.iter().map(OsString::from), env)
    }

    #[test]
    fn the_show_and_the_main_window_switch_are_read_from_the_arguments() {
        assert_eq!(opts(&[], false), (None, false));
        assert_eq!(opts(&["a.newq"], false), (Some("a.newq".into()), false));
        assert_eq!(opts(&["--main-window-only"], false), (None, true));
        // In either order, and with a path that has spaces.
        assert_eq!(
            opts(&["--main-window-only", "my show.newq"], false),
            (Some("my show.newq".into()), true)
        );
        assert_eq!(
            opts(&["my show.newq", "--main-window-only"], false),
            (Some("my show.newq".into()), true)
        );
        // The environment asks for it too; other options are ignored; the first show wins.
        assert_eq!(
            opts(&["a.newq", "b.newq"], true),
            (Some("a.newq".into()), true)
        );
        assert_eq!(
            opts(&["--whatever", "a.newq"], false),
            (Some("a.newq".into()), false)
        );
    }
}
