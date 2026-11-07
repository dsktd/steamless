use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::anyhow;
use clap::Args;
use clap::Parser;
use clap::Subcommand;
use crossbeam_channel::Receiver;
use log::warn;
use yakin_core::Result;
use yakin_core::device::ControllerId;
use yakin_core::device::haptics::Haptics;
use yakin_core::device::hidraw;
use yakin_core::device::hidraw::Hidraw;
use yakin_core::device::ibex;
use yakin_core::device::ibex::Report;
use yakin_core::menu::MenuCommand;
use yakin_core::profile::Config;
use yakin_core::state::Button;
use yakin_core::state::ControllerState;
use yakin_ipc::IpcConfig;
use yakin_ipc::IpcEvent;
use yakin_ipc::IpcRequest;
use yakin_ipc::IpcResponse;
use yakin_ipc::IpcTuning;

/// Stick and trigger moves smaller than this are not printed.
const MOVE: i32 = 2000;

#[derive(Parser)]
struct Cli {
    /// Daemon socket; default $XDG_RUNTIME_DIR/yakin.sock, else /run/yakin/yakin.sock
    #[arg(long, global = true, env = "YAKIN_SOCKET")]
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

    /// List the profiles, `*` before the active one
    #[command(name = "profiles")]
    Profiles,

    /// Change knobs of a profile until the next reload, e.g. `mouse_speed=1.5 scroll_natural=false`
    #[command(name = "tune")]
    Tune {
        profile: String,
        /// mouse_speed, mouse_accel, mouse_release_guard, mouse_smoothing, mouse_stick_speed,
        /// mouse_stick_curve, mouse_flick_degrees, scroll_speed, scroll_natural,
        /// scroll_stick_speed, deadzone_stick, deadzone_trigger, trigger_click,
        /// haptics_click, haptics_pad_ticks or haptics_menu
        #[arg(required = true)]
        values: Vec<String>,
    },

    /// Make the daemon re-read its config
    #[command(name = "reload")]
    Reload,

    /// Take the controller back now from the process the daemon yielded to
    #[command(name = "reclaim")]
    Reclaim,

    /// Open, close or toggle the quick menu: toggle, open or close
    #[command(name = "menu")]
    Menu { command: String },

    /// Open, close or toggle the on-screen keyboard: toggle, open or close
    #[command(name = "keyboard")]
    Keyboard { command: String },

    /// Print or check config files
    #[command(name = "config", subcommand)]
    Config(ConfigAction),

    /// The controller itself
    #[command(name = "controller", subcommand)]
    Controller(ControllerAction),

    /// Play the lizard cue on a controller's right trackpad, as a `lizard:` binding does: two
    /// pulses for on, one for off
    #[command(name = "cue")]
    Cue {
        /// on or off
        state: String,
        /// hidraw node; default every Steam Controller node
        #[arg(long)]
        hidraw: Option<PathBuf>,
    },

    /// Print the daemon's live events: input, profile and controller changes
    #[command(name = "watch")]
    Watch,

    /// Convert a Steam Input config (.vdf) to a profile, or with --report tally every .vdf under
    /// the given directories
    #[command(name = "import")]
    Import {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Name of the printed profile
        #[arg(long, default_value = "imported")]
        profile: String,
        /// Print what the importer runs and skips over every file instead of a profile
        #[arg(long)]
        report: bool,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print the config the daemon runs, every layer merged, as TOML
    #[command(name = "get")]
    Get,

    /// Print each config file the daemon reads, in order, then their merge
    #[command(name = "show")]
    Show,

    /// Validate a config file without the daemon
    #[command(name = "check")]
    Check { file: PathBuf },
}

#[derive(Subcommand)]
enum ControllerAction {
    /// Power every controller down (and drop its Bluetooth link), as the daemon does after
    /// tv:off, then idle until a button is held as after tv:off: for a TV put to standby by
    /// its own remote
    #[command(name = "off")]
    Off,
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
    let socket = cli.socket.unwrap_or_else(yakin_ipc::path::client_socket);
    let request = match cli.action {
        Action::Dump(source) => return Ok(dump(source)?),
        Action::Decode(source) => return Ok(decode(source)?),
        Action::Cue { state, hidraw } => return Ok(cue(&state, hidraw)?),
        Action::Activate { name } => IpcRequest::Activate(name),
        Action::Status => IpcRequest::Status,
        Action::Reload => IpcRequest::Reload,
        Action::Reclaim => IpcRequest::Reclaim,
        Action::Menu { command } => IpcRequest::Menu(MenuCommand::parse(&command)?),
        Action::Keyboard { command } => IpcRequest::Keyboard(MenuCommand::parse(&command)?),
        Action::Profiles => return Ok(profiles(&socket)?),
        Action::Tune { profile, values } => IpcRequest::Tune(tune(&socket, &profile, &values)?),
        Action::Watch => return Ok(watch(&socket)?),
        Action::Import {
            paths,
            profile,
            report,
        } => return Ok(import(&paths, &profile, report)?),
        Action::Controller(ControllerAction::Off) => IpcRequest::ControllerOff,
        Action::Config(ConfigAction::Get) => return Ok(config_get(&socket)?),
        Action::Config(ConfigAction::Show) => return Ok(config_show(&socket)?),
        Action::Config(ConfigAction::Check { file }) => {
            Config::load(&file)?;
            println!("{} ok", file.display());
            return Ok(());
        }
    };
    match yakin_ipc::client::request(&socket, request)? {
        IpcResponse::Ok => {}
        IpcResponse::Status(status) => {
            if let Some(holder) = status.yielded {
                println!("yielded to {} (pid {})", holder.comm, holder.pid);
            }
            println!("profile {}", status.profile);
            println!("desktop {}", status.desktop.as_deref().unwrap_or("none"));
            if let Some(auto) = status.auto {
                let profile = auto.profile.as_deref().unwrap_or("(unchanged)");
                let rule = match auto.rule {
                    Some(index) => format!(" (rule {index})"),
                    None => " (default)".to_string(),
                };
                let manual = if auto.manual { ", held by hand" } else { "" };
                println!("focus {} -> {profile}{rule}{manual}", auto.focused_app_id);
            }
            // Empty puck slots carry no controller
            for controller in status.controllers.iter().filter(|c| c.connected) {
                let battery = match controller.battery {
                    Some(level) => format!("{level}%"),
                    None => "?".to_string(),
                };
                let lizard = controller
                    .lizard
                    .map_or("?", |on| if on { "on" } else { "off" });
                println!(
                    "hidraw{} {} {} battery {battery} lizard {lizard} leaks {}",
                    controller.id,
                    controller.path,
                    controller.link.name(),
                    controller.leaks
                );
            }
        }
        IpcResponse::Error(err) => {
            println!("{err}");
            std::process::exit(1);
        }
        response => return Err(anyhow!("Unexpected response {response:?}").into()),
    }
    Ok(())
}

fn status(socket: &std::path::Path) -> Result<yakin_ipc::IpcStatus> {
    match yakin_ipc::client::request(socket, IpcRequest::Status)? {
        IpcResponse::Status(status) => Ok(status),
        IpcResponse::Error(err) => Err(anyhow!(err)),
        response => Err(anyhow!("Unexpected response {response:?}")),
    }
}

fn config(socket: &std::path::Path) -> Result<IpcConfig> {
    match yakin_ipc::client::request(socket, IpcRequest::GetConfig)? {
        IpcResponse::Config(config) => Ok(*config),
        IpcResponse::Error(err) => Err(anyhow!(err)),
        response => Err(anyhow!("Unexpected response {response:?}")),
    }
}

fn config_get(socket: &std::path::Path) -> Result<()> {
    let config = config(socket)?;
    if config.baseline.is_none() && config.dropins.is_empty() {
        eprintln!("No config file, the daemon runs the Xbox layout");
    }
    print!("{}", config.effective.to_toml()?);
    Ok(())
}

fn config_show(socket: &std::path::Path) -> Result<()> {
    let config = config(socket)?;
    let mut first = true;
    for (kind, layer) in config
        .baseline
        .iter()
        .map(|layer| ("baseline", layer))
        .chain(config.dropins.iter().map(|layer| ("drop-in", layer)))
    {
        if !first {
            println!();
        }
        first = false;
        println!("# {kind} {}", layer.path);
        print!("{}", layer.toml.to_toml()?);
    }
    let writable = if config.writable { "" } else { ", read-only" };
    if !first {
        println!();
    }
    println!(
        "# effective, editor writes {}{writable}",
        config.editor_path
    );
    print!("{}", config.effective.to_toml()?);
    Ok(())
}

fn profiles(socket: &std::path::Path) -> Result<()> {
    let status = status(socket)?;
    for name in status.profiles {
        let mark = if name == status.profile { '*' } else { ' ' };
        println!("{mark} {name}");
    }
    Ok(())
}

/// The profile's current tuning with `key=value` pairs applied.
fn tune(
    socket: &std::path::Path,
    profile: &str,
    values: &[String],
) -> Result<IpcTuning> {
    let mut tuning = status(socket)?
        .tunings
        .into_iter()
        .find(|tuning| tuning.profile == profile)
        .ok_or(anyhow!("No profile named {profile}"))?;
    for pair in values {
        let Some((key, value)) = pair.split_once('=') else {
            return Err(anyhow!("Expected key=value, got {pair}"));
        };
        let number = || value.parse::<f32>().map_err(|err| anyhow!("{key}: {err}"));
        match key {
            "mouse_speed" => tuning.mouse_speed = number()?,
            "mouse_accel" => tuning.mouse_accel = number()?,
            "mouse_release_guard" => {
                tuning.mouse_release_guard = value.parse().map_err(|err| anyhow!("{key}: {err}"))?
            }
            "mouse_smoothing" => tuning.mouse_smoothing = number()?,
            "mouse_stick_speed" => tuning.mouse_stick_speed = number()?,
            "mouse_stick_curve" => tuning.mouse_stick_curve = number()?,
            "mouse_flick_degrees" => tuning.mouse_flick_degrees = number()?,
            "scroll_speed" => tuning.scroll_speed = number()?,
            "scroll_stick_speed" => tuning.scroll_stick_speed = number()?,
            "deadzone_stick" => tuning.deadzone_stick = number()?,
            "deadzone_trigger" => tuning.deadzone_trigger = number()?,
            "trigger_click" => tuning.trigger_click = number()?,
            "haptics_click" => tuning.haptics_click = number()?,
            "haptics_pad_ticks" => tuning.haptics_pad_ticks = number()?,
            "haptics_menu" => tuning.haptics_menu = number()?,
            "scroll_natural" => {
                tuning.scroll_natural = value.parse().map_err(|err| anyhow!("{key}: {err}"))?
            }
            _ => return Err(anyhow!("Unknown key {key}")),
        }
    }
    Ok(tuning)
}

fn watch(socket: &std::path::Path) -> Result<()> {
    for event in yakin_ipc::client::subscribe(socket)? {
        match event? {
            IpcEvent::State { id, state } => {
                let pressed: Vec<&str> = Button::ALL
                    .into_iter()
                    .filter(|button| state.buttons.contains(*button))
                    .map(Button::name)
                    .collect();
                println!(
                    "hidraw{id} lstick {} {} rstick {} {} lt {} rt {} lpad {} {} rpad {} {} [{}]",
                    state.lstick.x,
                    state.lstick.y,
                    state.rstick.x,
                    state.rstick.y,
                    state.lt,
                    state.rt,
                    state.lpad.x,
                    state.lpad.y,
                    state.rpad.x,
                    state.rpad.y,
                    pressed.join(" ")
                );
            }
            IpcEvent::Profile(name) => println!("profile {name}"),
            IpcEvent::Controllers(controllers) => {
                let ids: Vec<String> = controllers
                    .iter()
                    .map(|controller| format!("hidraw{}", controller.id))
                    .collect();
                println!("controllers [{}]", ids.join(" "));
            }
            IpcEvent::Tuning(tuning) => println!("tuning {tuning:?}"),
            IpcEvent::Config(config) => println!("config {}", config.editor_path),
            IpcEvent::Yielded(Some(holder)) => {
                println!("yielded to {} (pid {})", holder.comm, holder.pid)
            }
            IpcEvent::Yielded(None) => println!("reclaimed"),
            IpcEvent::Menu(Some(menu)) => {
                let focus = menu.items.get(menu.focus as usize);
                println!("menu {}", focus.map_or("", |item| item.label.as_str()))
            }
            IpcEvent::Menu(None) => println!("menu closed"),
            IpcEvent::Keyboard(Some(keyboard)) => println!("keyboard {}", keyboard.layer),
            IpcEvent::Keyboard(None) => println!("keyboard closed"),
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

fn import(
    paths: &[PathBuf],
    profile: &str,
    report: bool,
) -> Result<()> {
    if !report {
        let [path] = paths else {
            return Err(anyhow!("import takes one file without --report"));
        };
        let text = std::fs::read_to_string(path)?;
        print!("{}", yakin_import::import(&text, profile)?.toml()?);
        return Ok(());
    }
    let mut files = Vec::new();
    for path in paths {
        vdf_files(path, &mut files)?;
    }
    files.sort();
    let mut tally = yakin_import::tally::Tally::default();
    let mut failures = Vec::new();
    // Files with several action sets, with layers, with the gyro bound
    let mut features = [0; 3];
    for file in &files {
        let imported = std::fs::read_to_string(file)
            .map_err(anyhow::Error::from)
            .and_then(|text| yakin_import::import(&text, profile));
        match imported {
            Ok(imported) => {
                tally.merge(&imported.tally);
                let profiles = imported.profiles.values();
                let found = [
                    imported.profiles.len() > 1,
                    profiles.clone().any(|profile| !profile.layer.is_empty()),
                    profiles
                        .clone()
                        .any(|profile| profile.axes.contains_key("gyro")),
                ];
                for (count, found) in features.iter_mut().zip(found) {
                    *count += found as u32;
                }
            }
            Err(err) => {
                let err = err
                    .to_string()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                failures.push(format!("{} {err}", file.display()));
            }
        }
    }
    println!(
        "{} files, {} parsed\n",
        files.len(),
        files.len() - failures.len()
    );
    let [sets, layers, gyro] = features;
    println!("action sets {sets}, layers {layers}, gyro {gyro} files\n");
    print!("{}", tally.table(20));
    println!("\nparse failures {}", failures.len());
    for failure in failures {
        println!("  {failure}");
    }
    Ok(())
}

/// Every `.vdf` file at or under `path`.
fn vdf_files(
    path: &std::path::Path,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    if !path.is_dir() {
        if path.extension().is_some_and(|ext| ext == "vdf") {
            files.push(path.to_path_buf());
        }
        return Ok(());
    }
    for entry in std::fs::read_dir(path)? {
        vdf_files(&entry?.path(), files)?;
    }
    Ok(())
}

/// Play the lizard cue on `hidraw`, or on every node.
fn cue(
    state: &str,
    hidraw: Option<PathBuf>,
) -> Result<()> {
    let on = match state {
        "on" => true,
        "off" => false,
        other => return Err(anyhow!("Cue {other}: on or off")),
    };
    let paths = match hidraw {
        Some(path) => vec![path],
        None => hidraw::scan(),
    };
    for path in paths {
        match Haptics::open(&path).and_then(|mut haptics| haptics.cue(on)) {
            Ok(()) => println!("{} cue {state}", path.display()),
            Err(err) => println!("{} cue: {err}", path.display()),
        }
    }
    Ok(())
}
