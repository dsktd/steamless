use std::any::Any;
use std::path::PathBuf;

use clap::Parser;
use dioxus_native::LogicalSize;
use dioxus_native::WindowAttributes;
use dioxus_native::winit::platform::wayland::WindowAttributesWayland;
use yakin_ui::app::App;
use yakin_ui::app::Page;
use yakin_ui::daemon::Daemon;

/// The Wayland app_id, the desktop file's name.
const APP_ID: &str = "dev.dsktd.yakin";

#[derive(Parser)]
struct Cli {
    /// Daemon socket; default $XDG_RUNTIME_DIR/yakin.sock, else /run/yakin/yakin.sock
    #[arg(long, env = "YAKIN_SOCKET")]
    socket: Option<PathBuf>,
    /// The tab to open on: live, edit or model
    #[arg(long, default_value = "live")]
    page: String,
}

fn main() {
    // Warnings by default: why a subscription ended must reach the journal
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let cli = Cli::parse();
    // Wayland only: winit takes Wayland over X11 when both are there, and never gets X11 here
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        eprintln!("yakin-ui: no WAYLAND_DISPLAY, a Wayland session is needed");
        std::process::exit(1);
    }
    let socket = cli.socket.unwrap_or_else(yakin_ipc::path::client_socket);
    let daemon = Daemon::new(socket);
    let page = Page(cli.page);
    // The app_id pairs the window with dev.dsktd.yakin.desktop
    let window = WindowAttributes::default()
        .with_title("yakin")
        .with_surface_size(LogicalSize::new(900, 820))
        .with_platform_attributes(Box::new(
            WindowAttributesWayland::default().with_name(APP_ID, APP_ID),
        ));
    let contexts: Vec<Box<dyn Fn() -> Box<dyn Any> + Send + Sync>> = vec![
        Box::new(move || Box::new(daemon.clone())),
        Box::new(move || Box::new(page.clone())),
    ];
    // The scene draws on Blitz's device: it needs what Bevy's own set-up would ask wgpu for
    let features = wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES;
    dioxus_native::launch_cfg(App, contexts, vec![Box::new(window), Box::new(features)]);
}
