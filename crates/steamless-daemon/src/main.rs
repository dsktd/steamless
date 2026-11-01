use std::collections::HashMap;
use std::path::PathBuf;

use clap::Parser;
use log::debug;
use log::info;
use log::warn;
use steamless_core::Result;
use steamless_core::device::ControllerId;
use steamless_core::device::Event as DeviceEvent;
use steamless_core::device::hidraw;
use steamless_core::device::ibex::Wireless;
use steamless_core::output::Keys;
use steamless_core::output::Pad;
use steamless_core::profile::Config;
use steamless_core::profile::Lizard;
use steamless_core::profile::Mapper;
use steamless_core::state::ControllerState;

#[derive(Parser)]
struct Cli {
    /// Config file; default ~/.config/steamless/config.toml, else the built-in Xbox layout
    #[arg(short, long)]
    config: Option<PathBuf>,
}

/// A controller that has sent input, with its virtual pad.
struct Controller {
    pad: Pad,
    mapper: Mapper,
    prev: ControllerState,
}

fn main() -> Result<()> {
    env_logger::init();
    let cli = Cli::parse();
    start(cli.config)
}

fn start(path: Option<PathBuf>) -> Result<()> {
    let config = load(path)?;
    let profile = config.profile()?;
    let mut keys = Keys::new(config.keys())?;
    // TODO "auto" acts as "off" until there is a pad client to watch
    let lizard_off = config.daemon.lizard != Lizard::On;
    let (tx_event, rx_event) = crossbeam_channel::unbounded();
    hidraw::start(tx_event, lizard_off);
    let mut paths = HashMap::new();
    let mut controllers: HashMap<ControllerId, Controller> = HashMap::new();
    while let Ok(event) = rx_event.recv() {
        match event {
            DeviceEvent::Opened(id, path) => {
                debug!("{id} opened {}", path.display());
                paths.insert(id, path);
            }
            DeviceEvent::State(id, state) => {
                if !controllers.contains_key(&id) {
                    let phys = paths.get(&id).map(|path| path.display().to_string());
                    let pad = Pad::new(&phys.unwrap_or(id.to_string()))?;
                    info!("{id} connected");
                    let controller = Controller {
                        pad,
                        mapper: Mapper::new(profile.clone()),
                        prev: ControllerState::default(),
                    };
                    controllers.insert(id, controller);
                }
                let controller = controllers.get_mut(&id).unwrap();
                let outputs = controller.mapper.map(&controller.prev, &state);
                controller.prev = state;
                if let Err(err) = controller.pad.send(&outputs) {
                    warn!("{id} pad: {err}");
                }
                if let Err(err) = keys.send(&outputs) {
                    warn!("{id} keys: {err}");
                }
            }
            DeviceEvent::Battery(id, battery) => debug!("{id} battery {battery:?}"),
            DeviceEvent::Wireless(id, Wireless::Disconnect) => {
                disconnect(id, &mut controllers, &mut keys);
            }
            DeviceEvent::Wireless(id, wireless) => debug!("{id} {wireless:?}"),
            DeviceEvent::Closed(id) => {
                debug!("{id} closed");
                paths.remove(&id);
                disconnect(id, &mut controllers, &mut keys);
            }
        }
    }
    Ok(())
}

/// Drop the virtual pad, releasing any key it still holds.
fn disconnect(
    id: ControllerId,
    controllers: &mut HashMap<ControllerId, Controller>,
    keys: &mut Keys,
) {
    let Some(mut controller) = controllers.remove(&id) else {
        return;
    };
    let outputs = controller
        .mapper
        .map(&controller.prev, &ControllerState::default());
    let _ = keys.send(&outputs);
    info!("{id} disconnected");
}

fn load(path: Option<PathBuf>) -> Result<Config> {
    let user = dirs::config_dir().map(|dir| dir.join("steamless/config.toml"));
    let Some(path) = path.or(user.filter(|path| path.exists())) else {
        info!("No config, using the Xbox layout");
        return Ok(Config::default());
    };
    info!("Config {}", path.display());
    Config::load(path)
}
