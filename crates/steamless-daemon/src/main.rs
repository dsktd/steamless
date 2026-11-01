use std::path::PathBuf;

use clap::Parser;
use log::info;
use steamless_core::Result;
use steamless_core::device::hidraw;
use steamless_core::profile::Config;
use steamless_core::profile::Lizard;

use crate::engine::Engine;

mod engine;

#[derive(Parser)]
struct Cli {
    /// Config file; default ~/.config/steamless/config.toml, else the built-in Xbox layout
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Control socket; default $XDG_RUNTIME_DIR/steamless.sock
    #[arg(short, long)]
    socket: Option<PathBuf>,
}

fn main() -> Result<()> {
    env_logger::init();
    let cli = Cli::parse();
    start(cli.config, cli.socket)
}

fn start(
    path: Option<PathBuf>,
    socket: Option<PathBuf>,
) -> Result<()> {
    let user = dirs::config_dir().map(|dir| dir.join("steamless/config.toml"));
    let path = path.or(user.filter(|path| path.exists()));
    let config = load(path.as_deref())?;
    // TODO "auto" acts as "off" until there is a pad client to watch
    let lizard_off = config.daemon.lizard != Lizard::On;
    let mut engine = Engine::new(path, config, lizard_off)?;
    let (tx_event, rx_event) = crossbeam_channel::unbounded();
    let socket = socket.unwrap_or(steamless_ipc::path::default_socket());
    steamless_ipc::server::start(&socket, tx_event.clone())?;
    info!("Socket {}", socket.display());
    hidraw::start(tx_event, lizard_off);
    while let Ok(event) = rx_event.recv() {
        engine.handle(event);
    }
    Ok(())
}

fn load(path: Option<&std::path::Path>) -> Result<Config> {
    let Some(path) = path else {
        info!("No config, using the Xbox layout");
        return Ok(Config::default());
    };
    info!("Config {}", path.display());
    Config::load(path)
}
