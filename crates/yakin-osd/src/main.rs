//! On-screen toasts for the daemon's changes (profile, Steam yield, controllers, battery) and
//! its quick menu and on-screen keyboard, drawn on wlr-layer-shell overlay surfaces. Wayland only.

mod draw;
mod keyboard;
mod menu;
mod overlay;
mod toast;

use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use clap::Parser;
use clap::ValueEnum;
use log::error;
use log::warn;
use smithay_client_toolkit::reexports::calloop::EventLoop;
use smithay_client_toolkit::reexports::calloop::channel;
use smithay_client_toolkit::reexports::calloop::channel::Sender;
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::reexports::client::Connection;
use smithay_client_toolkit::reexports::client::globals::registry_queue_init;
use smithay_client_toolkit::shell::wlr_layer::Anchor;
use yakin_core::keyboard::Keyboard;
use yakin_core::menu::Menu;
use yakin_ipc::IpcEvent;
use yakin_ipc::IpcKeyboard;
use yakin_ipc::IpcMenu;
use yakin_ipc::client::Backoff;

use crate::overlay::Exit;
use crate::overlay::Overlay;
use crate::toast::State;
use crate::toast::Toast;

/// How long `--once` waits for the compositor's first frame.
const ONCE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Parser)]
struct Cli {
    /// Daemon socket; default $XDG_RUNTIME_DIR/yakin.sock, else /run/yakin/yakin.sock
    #[arg(long, env = "YAKIN_SOCKET")]
    socket: Option<PathBuf>,
    /// Screen edge the toast sits on
    #[arg(long, value_enum, default_value_t = Edge::Top)]
    anchor: Edge,
    /// How long a toast stays before it fades, in milliseconds
    #[arg(long, default_value_t = 1500)]
    duration: u64,
    /// Show one toast with this title and exit once it fades, no daemon
    #[arg(long)]
    show: Option<String>,
    /// Second line for --show
    #[arg(long, requires = "show")]
    subtitle: Option<String>,
    /// Show a toast, exit 0 at the first frame (1 on error, 2 after 10 s without a frame)
    #[arg(long)]
    once: bool,
    /// Show the default quick menu instead of a toast, no daemon
    #[arg(long, conflicts_with = "show")]
    menu_demo: bool,
    /// Write the default menu as SVG over a 960x540 screen (1080p at scale 2) and exit
    #[arg(long)]
    menu_svg: Option<PathBuf>,
    /// Show a sample on-screen keyboard instead of a toast, no daemon
    #[arg(long, conflicts_with_all = ["show", "menu_demo"])]
    keyboard_demo: bool,
    /// Write the sample keyboard as SVG over a 1080p screen at scale 2 and exit
    #[arg(long)]
    keyboard_svg: Option<PathBuf>,
}

/// What the subscription hands the overlay.
enum Update {
    Toast(Toast),
    Menu(Option<IpcMenu>),
    Keyboard(Option<IpcKeyboard>),
}

#[derive(Clone, Copy, ValueEnum)]
enum Edge {
    Top,
    Bottom,
    Center,
}

fn main() -> anyhow::Result<()> {
    // Warnings by default: why a subscription ended must reach the journal
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let cli = Cli::parse();
    if let Some(path) = &cli.menu_svg {
        let svg = menu::menu_svg(&IpcMenu::from(&Menu::default()), (960, 540));
        std::fs::write(path, svg)?;
        return Ok(());
    }
    if let Some(path) = &cli.keyboard_svg {
        let size = (960, keyboard::keyboard_height(540));
        std::fs::write(path, keyboard::keyboard_svg(&demo_keyboard(), size))?;
        return Ok(());
    }
    let anchor = match cli.anchor {
        Edge::Top => Anchor::TOP,
        Edge::Bottom => Anchor::BOTTOM,
        Edge::Center => Anchor::empty(),
    };
    let exit = match (cli.once, &cli.show) {
        (true, _) => Exit::Shown,
        (false, Some(_)) => Exit::Faded,
        (false, None) if cli.menu_demo || cli.keyboard_demo => Exit::Faded,
        (false, None) => Exit::Never,
    };
    if cli.once {
        let _timeout = std::thread::spawn(|| {
            std::thread::sleep(ONCE_TIMEOUT);
            eprintln!("osd: no frame");
            std::process::exit(2);
        });
    }

    let conn = Connection::connect_to_env()?;
    let (globals, queue) = registry_queue_init(&conn)?;
    let mut event_loop = EventLoop::<Overlay>::try_new()?;
    let handle = event_loop.handle();
    let mut overlay = Overlay::new(
        &globals,
        queue.handle(),
        handle.clone(),
        anchor,
        Duration::from_millis(cli.duration),
        exit,
    )?;
    WaylandSource::new(conn, queue)
        .insert(handle.clone())
        .map_err(|err| anyhow::anyhow!("Wayland source: {}", err.error))?;

    match exit {
        _ if cli.menu_demo => overlay.set_menu(Some(IpcMenu::from(&Menu::default()))),
        _ if cli.keyboard_demo => overlay.set_keyboard(Some(demo_keyboard())),
        Exit::Never => {
            let (sender, updates) = channel::channel();
            handle
                .insert_source(updates, |event, _, overlay| match event {
                    channel::Event::Msg(Update::Toast(toast)) => overlay.show(toast),
                    channel::Event::Msg(Update::Menu(menu)) => overlay.set_menu(menu),
                    channel::Event::Msg(Update::Keyboard(keyboard)) => {
                        overlay.set_keyboard(keyboard)
                    }
                    channel::Event::Closed => {}
                })
                .map_err(|err| anyhow::anyhow!("Update channel: {}", err.error))?;
            let socket = cli.socket.unwrap_or_else(yakin_ipc::path::client_socket);
            let _handle = std::thread::spawn(move || follow(&socket, &sender));
        }
        _ => {
            let title = cli.show.unwrap_or("Yakin".to_string());
            overlay.show(Toast::new(&title, cli.subtitle.as_deref()));
        }
    }

    loop {
        if let Some(status) = overlay.status.take() {
            return status;
        }
        event_loop.dispatch(None, &mut overlay)?;
    }
}

/// Stay subscribed, reconnecting with backoff; each new subscription starts from a blank state
/// so its opening events stay silent.
fn follow(
    socket: &Path,
    updates: &Sender<Update>,
) {
    let mut backoff = Backoff::default();
    loop {
        let start = Instant::now();
        match yakin_ipc::client::subscribe(socket) {
            Ok(events) => {
                let mut end = "end of stream".to_string();
                let mut state = State::default();
                for event in events {
                    match event {
                        Ok(IpcEvent::Menu(menu)) => {
                            let _ = updates.send(Update::Menu(menu));
                        }
                        Ok(IpcEvent::Keyboard(keyboard)) => {
                            let _ = updates.send(Update::Keyboard(keyboard));
                        }
                        Ok(event) => {
                            if let Some(toast) = toast::toast(&state, &event) {
                                let _ = updates.send(Update::Toast(toast));
                            }
                            state.apply(&event);
                        }
                        Err(err) => {
                            // A frame that is not an io error is one this build cannot
                            // decode: the daemon is newer (a switch restarts the system
                            // daemon, not this user service). Exit, and systemd starts the
                            // current build in our place.
                            if err.downcast_ref::<std::io::Error>().is_none() {
                                error!("daemon speaks a newer protocol ({err}); exiting");
                                std::process::exit(3);
                            }
                            end = err.to_string();
                        }
                    }
                }
                let ms = start.elapsed().as_millis();
                warn!("subscription ended after {ms} ms: {end}");
            }
            Err(err) => warn!("subscribe: {err}"),
        }
        // A gone daemon cannot close its menu or keyboard
        let _ = updates.send(Update::Menu(None));
        let _ = updates.send(Update::Keyboard(None));
        std::thread::sleep(backoff.delay(start.elapsed()));
    }
}

/// A keyboard with a finger on each pad and the focus on backspace, for the demos.
fn demo_keyboard() -> IpcKeyboard {
    IpcKeyboard::from(&Keyboard {
        hover_left: Some(12),
        hover_right: Some(17),
        focus: 43,
        ..Keyboard::default()
    })
}
