// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod engine;
mod gpu;
mod media;
mod render;
mod show;
mod ui;

use winit::event_loop::EventLoop;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu=warn,naga=warn"),
    )
    .format_timestamp_millis()
    .init();
    let show_path = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    let event_loop = EventLoop::new()?;
    let mut app = app::App::new(show_path);
    event_loop.run_app(&mut app)?;
    Ok(())
}
