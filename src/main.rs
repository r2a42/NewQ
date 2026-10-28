// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
#[cfg(test)]
mod blend;
mod branding;
mod clock;
mod color;
mod crash;
mod dock;
mod edid;
mod engine;
mod export;
mod fx;
mod gpu;
mod history;
mod live_text;
mod media;
mod mesh;
mod monitor;
mod ndi;
mod output;
mod pattern;
mod prefs;
mod projector;
mod recovery;
mod relink;
mod remote;
mod render;
mod show;
mod speed;
mod stream;
mod text;
mod timecode;
mod ui;
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

fn main() -> anyhow::Result<()> {
    // A copy of this program started to talk to NDI's library for the show
    // (see ndi.rs): it does that and nothing else.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = ndi::run_helper_if_asked(&args) {
        std::process::exit(code);
    }
    let mut logger = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu=warn,naga=warn"),
    );
    logger.format_timestamp_millis();
    // With no console to read (a Windows release), the log goes to a file.
    let asked = std::env::var_os("NEWQ_LOG_FILE").is_some();
    if crash::log_to_file(crash::Os::current(), cfg!(debug_assertions), asked)
        && let Some(file) = crash::open_log(&crash::log_dir())
    {
        logger.target(env_logger::Target::Pipe(Box::new(file)));
    }
    logger.init();
    crash::install();
    log::info!(
        "NewQ {} on {} ({}), logs in {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        crash::log_dir().display()
    );
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
    let mut app = app::App::new(show_path, main_window_only, move || {
        let _ = proxy.send_event(());
    });
    event_loop.run_app(&mut app)?;
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
