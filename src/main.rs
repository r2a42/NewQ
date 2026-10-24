// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod engine;
mod gpu;
mod history;
mod media;
mod recovery;
mod remote;
mod render;
mod show;
mod text;
mod ui;

use winit::event_loop::EventLoop;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu=warn,naga=warn"),
    )
    .format_timestamp_millis()
    .init();
    let show_path = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    // The user event is how remote-control threads wake the loop.
    let event_loop = EventLoop::<()>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let mut app = app::App::new(show_path, move || {
        let _ = proxy.send_event(());
    });
    event_loop.run_app(&mut app)?;
    Ok(())
}
