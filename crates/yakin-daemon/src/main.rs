use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use anyhow::anyhow;
use clap::Parser;
use crossbeam_channel::RecvTimeoutError;
use crossbeam_channel::Sender;
use log::info;
use log::warn;
use nix::sys::resource::Resource;
use nix::sys::resource::getrlimit;
use nix::sys::resource::setrlimit;
use nix::sys::signal::SigSet;
use nix::sys::signal::Signal;
use yakin_core::Result;
use yakin_core::device::hidraw;
use yakin_core::profile::Lizard;

use crate::config::Layers;
use crate::engine::Engine;

mod config;
mod desktop;
mod engine;
mod steam;

/// How often the daemon looks for Steam holding the controller.
const HOLDERS: Duration = Duration::from_secs(2);

#[derive(Parser)]
struct Cli {
    /// Baseline config file; default ~/.config/yakin/config.toml. With no file at all
    /// the daemon runs the built-in Xbox layout
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Drop-in directory, every *.toml applied over the config by file name;
    /// default ~/.config/yakin/config.d
    #[arg(short = 'd', long, env = "YAKIN_CONFIG_DIR")]
    config_dir: Option<PathBuf>,

    /// Control socket; default $XDG_RUNTIME_DIR/yakin.sock
    #[arg(short, long)]
    socket: Option<PathBuf>,
}

fn main() -> Result<()> {
    // Warnings by default: a failed desktop action or reclaim must reach the journal
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let cli = Cli::parse();
    raise_nofile();
    start(cli.config, cli.config_dir, cli.socket)
}

fn start(
    path: Option<PathBuf>,
    dir: Option<PathBuf>,
    socket: Option<PathBuf>,
) -> Result<()> {
    if let Some(root) = dirs::config_dir() {
        migrate(&root);
    }
    let user = dirs::config_dir().map(|dir| dir.join("yakin"));
    let layers = Layers {
        baseline: path.or(user.as_ref().map(|user| user.join("config.toml"))),
        dir: match dir.or(user.map(|user| user.join("config.d"))) {
            Some(dir) => dir,
            None => return Err(anyhow!("No config directory, pass --config-dir")),
        },
    };
    // Before any thread starts, so they all inherit the mask and only `stop` takes them
    let mut signals = SigSet::empty();
    signals.add(Signal::SIGTERM);
    signals.add(Signal::SIGINT);
    signals.thread_block()?;
    let sources = layers.read()?;
    if sources.is_empty() {
        info!("No config, using the Xbox layout");
    }
    for source in sources.iter() {
        info!("Config {}", source.path.display());
    }
    let (_, config) = config::merged(sources)?;
    // TODO "auto" acts as "off" until there is a pad client to watch
    let lizard_off = config.daemon.lizard != Lizard::On;
    let imu = config.daemon.imu;
    let pause = hidraw::Pause::default();
    // A reload changes where actions go; the focus watch keeps the backend it started with
    let backend = config.desktop;
    let mut engine = Engine::new(layers, config, lizard_off, pause.clone())?;
    let (tx_event, rx_event) = crossbeam_channel::unbounded();
    let socket = socket.unwrap_or(yakin_ipc::path::default_socket());
    yakin_ipc::server::start(&socket, tx_event.clone())?;
    info!("Socket {}", socket.display());
    stop(signals, tx_event.clone());
    let tx_tick = tx_event.clone();
    let _handle = std::thread::spawn(move || {
        while tx_tick.send(engine::Event::Tick).is_ok() {
            std::thread::sleep(HOLDERS);
        }
    });
    desktop::start(backend.desktop(), tx_event.clone());
    hidraw::start(tx_event, lizard_off, imu, pause);
    loop {
        // Wake for settled tuning as well as events
        let event = match engine.deadline() {
            Some(deadline) => match rx_event.recv_deadline(deadline) {
                Ok(event) => Some(event),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            },
            None => match rx_event.recv() {
                Ok(event) => Some(event),
                Err(_) => break,
            },
        };
        match event {
            Some(engine::Event::Shutdown) => {
                engine.handle(engine::Event::Shutdown);
                info!("Stopped");
                break;
            }
            Some(event) => engine.handle(event),
            None => {}
        }
        engine.save_settled(Instant::now());
    }
    Ok(())
}

/// Turn SIGTERM and SIGINT, blocked in every thread, into [`engine::Event::Shutdown`]; a second
/// one while shutting down exits at once.
fn stop(
    signals: SigSet,
    tx_event: Sender<engine::Event>,
) {
    std::thread::spawn(move || {
        let signal = watch(|| signals.wait().map_err(|err| err.to_string()), &tx_event);
        if let Some(signal) = signal {
            warn!("{signal} again, exiting now");
            std::process::exit(128 + signal as i32);
        }
    });
}

/// Send one [`engine::Event::Shutdown`] for the first signal `next` gives, and return the second,
/// or None if waiting fails.
fn watch(
    mut next: impl FnMut() -> std::result::Result<Signal, String>,
    tx_event: &Sender<engine::Event>,
) -> Option<Signal> {
    let mut wait = || {
        next()
            .map_err(|err| warn!("Cannot wait for signals: {err}"))
            .ok()
    };
    let first = wait()?;
    info!("{first}");
    let _ = tx_event.send(engine::Event::Shutdown);
    wait()
}

/// Every client connection and controller takes fds; use all the hard limit allows.
fn raise_nofile() {
    match getrlimit(Resource::RLIMIT_NOFILE) {
        Ok((soft, hard)) if soft < hard => {
            if let Err(err) = setrlimit(Resource::RLIMIT_NOFILE, hard, hard) {
                warn!("Cannot raise the fd limit to {hard}: {err}");
            }
        }
        Ok(_) => {}
        Err(err) => warn!("Cannot read the fd limit: {err}"),
    }
}

/// One-time move of `<root>/steamless` (the project's former name) to `<root>/yakin`.
fn migrate(root: &Path) {
    let old = root.join("steamless");
    let new = root.join("yakin");
    if new.exists() || !old.is_dir() {
        return;
    }
    match std::fs::rename(&old, &new) {
        Ok(()) => info!("Moved config {} to {}", old.display(), new.display()),
        Err(err) => warn!("Could not move config {}: {err}", old.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_signal_ends_the_watch() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut signals = [Signal::SIGTERM, Signal::SIGINT].into_iter();
        let second = watch(|| signals.next().ok_or_else(|| "none".to_string()), &tx);
        assert_eq!(second, Some(Signal::SIGINT));
        assert!(matches!(rx.try_recv(), Ok(engine::Event::Shutdown)));
        assert!(rx.try_recv().is_err(), "one shutdown only");
        // Waiting fails before any signal: no shutdown, no exit
        let (tx, rx) = crossbeam_channel::unbounded();
        assert_eq!(watch(|| Err("broken".to_string()), &tx), None);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn migrates_steamless_config() {
        let home = std::env::temp_dir().join(format!("yakin-migrate-{}", std::process::id()));
        let root = home.join(".config");
        std::fs::create_dir_all(root.join("steamless/config.d")).unwrap();
        std::fs::write(root.join("steamless/config.toml"), "").unwrap();
        migrate(&root);
        assert!(root.join("yakin/config.toml").exists());
        assert!(root.join("yakin/config.d").is_dir());
        assert!(!root.join("steamless").exists());
        // A second run, or an existing yakin dir, leaves things alone.
        std::fs::create_dir_all(root.join("steamless")).unwrap();
        migrate(&root);
        assert!(root.join("steamless").is_dir());
        std::fs::remove_dir_all(&home).unwrap();
    }
}
