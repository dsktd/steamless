use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::anyhow;
use clap::Args;
use clap::Parser;
use clap::Subcommand;
use crossbeam_channel::Receiver;
use log::warn;
use steamless_core::Result;
use steamless_core::device::ControllerId;
use steamless_core::device::hidraw;
use steamless_core::device::hidraw::Hidraw;
use steamless_core::device::ibex;
use steamless_core::device::ibex::Report;
use steamless_core::state::Button;
use steamless_core::state::ControllerState;
use steamless_ipc::IpcRequest;
use steamless_ipc::IpcResponse;

/// Stick and trigger moves smaller than this are not printed.
const MOVE: i32 = 2000;

#[derive(Parser)]
struct Cli {
    /// Daemon socket; default $XDG_RUNTIME_DIR/steamless.sock
    #[arg(long, global = true, env = "STEAMLESS_SOCKET")]
    socket: Option<PathBuf>,

    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Print raw reports as `id len hex`, `*` after bytes changed since the last report of that id
    #[command(name = "dump")]
    Dump(Source),

    /// Print button presses and releases and stick moves
    #[command(name = "decode")]
    Decode(Source),

    /// Switch every controller to a profile
    #[command(name = "activate")]
    Activate { name: String },

    /// Print the active profile and the controllers
    #[command(name = "status")]
    Status,

    /// Make the daemon re-read its config
    #[command(name = "reload")]
    Reload,
}

#[derive(Args)]
struct Source {
    /// hidraw node; default every Steam Controller node, prefixed with its name
    #[arg(long)]
    hidraw: Option<PathBuf>,
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    let cli = Cli::parse();
    let request = match cli.action {
        Action::Dump(source) => return Ok(dump(source)?),
        Action::Decode(source) => return Ok(decode(source)?),
        Action::Activate { name } => IpcRequest::Activate(name),
        Action::Status => IpcRequest::Status,
        Action::Reload => IpcRequest::Reload,
    };
    let socket = cli.socket.unwrap_or(steamless_ipc::path::default_socket());
    match steamless_ipc::client::request(&socket, request)? {
        IpcResponse::Ok => {}
        IpcResponse::Status(status) => {
            println!("profile {}", status.profile);
            for controller in status.controllers {
                let battery = match controller.battery {
                    Some(level) => format!("{level}%"),
                    None => "?".to_string(),
                };
                let lizard = if controller.lizard { "on" } else { "off" };
                println!(
                    "hidraw{} {} battery {battery} lizard {lizard}",
                    controller.id, controller.path
                );
            }
        }
        IpcResponse::Error(err) => {
            println!("{err}");
            std::process::exit(1);
        }
    }
    Ok(())
}

/// One reader thread per node; reports arrive tagged with their node.
fn read(source: Source) -> Result<Receiver<(ControllerId, Vec<u8>)>> {
    let paths = match source.hidraw {
        Some(path) => vec![path],
        None => hidraw::scan(),
    };
    if paths.is_empty() {
        return Err(anyhow!("No Steam Controller hidraw node found"));
    }
    let (tx_report, rx_report) = crossbeam_channel::unbounded();
    for path in paths {
        let hidraw = Hidraw::open(&path)?;
        let tx_report_ = tx_report.clone();
        let _handle = std::thread::spawn(move || {
            let mut buf = [0_u8; 64];
            loop {
                let Ok(len) = hidraw.read(&mut buf) else {
                    break;
                };
                if len == 0 {
                    break;
                }
                tx_report_.send((hidraw.id(), buf[..len].to_vec())).unwrap();
            }
        });
    }
    Ok(rx_report)
}

fn dump(source: Source) -> Result<()> {
    let tag = source.hidraw.is_none();
    let rx_report = read(source)?;
    let mut prevs: HashMap<(ControllerId, u8), Vec<u8>> = HashMap::new();
    while let Ok((id, report)) = rx_report.recv() {
        let prev = prevs.get(&(id, report[0]));
        let mut hex = String::new();
        for (offset, byte) in report.iter().enumerate() {
            let changed = prev.is_some_and(|prev| prev.get(offset) != Some(byte));
            hex.push_str(&format!("{byte:02x}{}", if changed { '*' } else { ' ' }));
        }
        let line = format!("{:02x} {} {}", report[0], report.len(), hex.trim_end());
        match tag {
            true => println!("{id} {line}"),
            false => println!("{line}"),
        }
        prevs.insert((id, report[0]), report);
    }
    Ok(())
}

fn decode(source: Source) -> Result<()> {
    let rx_report = read(source)?;
    let mut prevs: HashMap<ControllerId, ControllerState> = HashMap::new();
    let mut shown: HashMap<ControllerId, [i32; 6]> = HashMap::new();
    while let Ok((id, report)) = rx_report.recv() {
        let state = match ibex::decode(&report) {
            Ok(Report::Input(state)) => state,
            Ok(Report::Battery(battery)) => {
                println!("{id} battery {}% {:?}", battery.level, battery.charge);
                continue;
            }
            Ok(Report::Wireless(wireless)) => {
                println!("{id} wireless {wireless:?}");
                continue;
            }
            Ok(Report::Other(_report_id)) => continue,
            Err(err) => {
                warn!("{id} {err}");
                continue;
            }
        };
        let prev = prevs.insert(id, state).unwrap_or_default();
        for button in Button::ALL {
            match (
                prev.buttons.contains(button),
                state.buttons.contains(button),
            ) {
                (false, true) => println!("{id} {button} pressed"),
                (true, false) => println!("{id} {button} released"),
                _ => {}
            }
        }
        // Print an axis pair when either value moved far enough since it was last printed
        let axes = [
            state.lstick.x as i32,
            state.lstick.y as i32,
            state.rstick.x as i32,
            state.rstick.y as i32,
            state.lt as i32,
            state.rt as i32,
        ];
        let shown = shown.entry(id).or_default();
        for (pair, name) in [(0, "lstick"), (2, "rstick"), (4, "triggers")] {
            let moved = (shown[pair] - axes[pair]).abs() > MOVE
                || (shown[pair + 1] - axes[pair + 1]).abs() > MOVE;
            if moved {
                println!("{id} {name} {} {}", axes[pair], axes[pair + 1]);
                shown[pair] = axes[pair];
                shown[pair + 1] = axes[pair + 1];
            }
        }
    }
    Ok(())
}
