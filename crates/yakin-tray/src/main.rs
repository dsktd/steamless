//! A system tray icon (StatusNotifierItem over D-Bus) for the yakin daemon: its state at a
//! glance and the everyday actions. Wayland sessions only; no X11 tray.

mod model;

use std::path::Path;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::sync::mpsc::Receiver;
use std::sync::mpsc::Sender;
use std::time::Instant;

use clap::Parser;
use ksni::blocking::Handle;
use ksni::blocking::TrayMethods;
use ksni::menu::MenuItem;
use ksni::menu::RadioGroup;
use ksni::menu::RadioItem;
use ksni::menu::StandardItem;
use log::error;
use log::warn;
use yakin_core::menu::MenuCommand;
use yakin_ipc::IpcEvent;
use yakin_ipc::IpcRequest;
use yakin_ipc::IpcResponse;
use yakin_ipc::client::Backoff;

use crate::model::Entry;
use crate::model::Look;
use crate::model::Snapshot;

/// The app icon's PNGs, made from Valve's CAD (`just assets`).
const ICON_32: &[u8] = include_bytes!("../../../ui/assets/icons/32.png");
const ICON_128: &[u8] = include_bytes!("../../../ui/assets/icons/128.png");

static COLOUR: LazyLock<Vec<ksni::Icon>> = LazyLock::new(|| icons(false));
static GREY: LazyLock<Vec<ksni::Icon>> = LazyLock::new(|| icons(true));

#[derive(Parser)]
struct Cli {
    /// Daemon socket; default $XDG_RUNTIME_DIR/yakin.sock, else /run/yakin/yakin.sock
    #[arg(long, env = "YAKIN_SOCKET")]
    socket: Option<PathBuf>,
}

/// The pixmaps of the icon, ARGB32, faded to grey when `grey`.
fn icons(grey: bool) -> Vec<ksni::Icon> {
    [ICON_32, ICON_128]
        .into_iter()
        .filter_map(|bytes| icon(bytes, grey).ok())
        .collect()
}

fn icon(
    bytes: &[u8],
    grey: bool,
) -> anyhow::Result<ksni::Icon> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size().unwrap_or_default()];
    let info = reader.next_frame(&mut buf)?;
    anyhow::ensure!(
        info.color_type == png::ColorType::Rgba && info.bit_depth == png::BitDepth::Eight,
        "Icon is not 8 bit RGBA"
    );
    buf.truncate(info.buffer_size());
    for pixel in buf.as_chunks_mut::<4>().0.iter_mut() {
        if grey {
            let [r, g, b, a] = [pixel[0], pixel[1], pixel[2], pixel[3]].map(u16::from);
            let luma = ((r * 30 + g * 59 + b * 11) / 100) as u8;
            pixel.copy_from_slice(&[luma, luma, luma, (a * 2 / 3) as u8]);
        }
        pixel.rotate_right(1); // RGBA to ARGB
    }
    Ok(ksni::Icon {
        width: info.width as i32,
        height: info.height as i32,
        data: buf,
    })
}

struct Tray {
    /// Requests for the worker thread, so a hung daemon never blocks ksni's D-Bus thread.
    requests: Sender<IpcRequest>,
    snapshot: Snapshot,
}
impl Tray {
    /// Queue `request` for the daemon; the worker logs a failure, the tray has nobody to tell.
    fn send(
        &self,
        request: IpcRequest,
    ) {
        if self.requests.send(request).is_err() {
            warn!("request worker is gone");
        }
    }
}

/// Run queued requests on the daemon, one at a time, until the tray is dropped.
fn work(
    socket: &Path,
    requests: &Receiver<IpcRequest>,
) {
    for request in requests {
        match yakin_ipc::client::request(socket, request) {
            Ok(IpcResponse::Error(err)) => warn!("daemon: {err}"),
            Ok(_) => {}
            Err(err) => warn!("{err}"),
        }
    }
}
impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "yakin".to_string()
    }

    fn title(&self) -> String {
        model::title(&self.snapshot)
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::Hardware
    }

    fn status(&self) -> ksni::Status {
        match Look::of(&self.snapshot) {
            Look::Active => ksni::Status::Active,
            Look::NoController => ksni::Status::Passive,
            Look::Yielded | Look::Offline => ksni::Status::NeedsAttention,
        }
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        pixmap(Look::of(&self.snapshot))
    }

    fn attention_icon_pixmap(&self) -> Vec<ksni::Icon> {
        pixmap(Look::of(&self.snapshot))
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            icon_pixmap: pixmap(Look::of(&self.snapshot)),
            title: model::title(&self.snapshot),
            description: model::description(&self.snapshot),
            ..ksni::ToolTip::default()
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        model::menu(&self.snapshot)
            .into_iter()
            .map(|entry| match entry {
                Entry::Separator => MenuItem::Separator,
                Entry::Profiles { names, selected } => RadioGroup {
                    selected,
                    select: Box::new(move |this: &mut Self, index| {
                        let Some(name) = this.names().get(index).cloned() else {
                            return;
                        };
                        this.send(IpcRequest::Activate(name));
                    }),
                    options: names
                        .into_iter()
                        .map(|label| RadioItem {
                            label,
                            ..RadioItem::default()
                        })
                        .collect(),
                }
                .into(),
                Entry::OpenConfiguration => StandardItem {
                    label: "Open configuration".to_string(),
                    icon_name: "preferences-system".to_string(),
                    activate: Box::new(|_| {
                        if let Err(err) = std::process::Command::new("yakin-ui").spawn() {
                            warn!("Cannot start yakin-ui: {err}");
                        }
                    }),
                    ..StandardItem::default()
                }
                .into(),
                Entry::Reload => StandardItem {
                    label: "Reload config".to_string(),
                    icon_name: "view-refresh".to_string(),
                    activate: Box::new(|this: &mut Self| this.send(IpcRequest::Reload)),
                    ..StandardItem::default()
                }
                .into(),
                Entry::QuickMenu { enabled } => StandardItem {
                    enabled,
                    label: "Quick menu".to_string(),
                    icon_name: "view-grid".to_string(),
                    activate: Box::new(|this: &mut Self| {
                        this.send(IpcRequest::Menu(MenuCommand::Toggle));
                    }),
                    ..StandardItem::default()
                }
                .into(),
                Entry::Keyboard { enabled } => StandardItem {
                    enabled,
                    label: "Keyboard".to_string(),
                    icon_name: "input-keyboard".to_string(),
                    activate: Box::new(|this: &mut Self| {
                        this.send(IpcRequest::Keyboard(MenuCommand::Toggle));
                    }),
                    ..StandardItem::default()
                }
                .into(),
                Entry::TakeBack(holder) => StandardItem {
                    label: format!("Take controller back from {holder}"),
                    icon_name: "media-playback-start".to_string(),
                    activate: Box::new(|this: &mut Self| this.send(IpcRequest::Reclaim)),
                    ..StandardItem::default()
                }
                .into(),
            })
            .collect()
    }
}
impl Tray {
    fn names(&self) -> &[String] {
        self.snapshot
            .as_ref()
            .map_or(&[], |status| status.profiles.as_slice())
    }
}

fn pixmap(look: Look) -> Vec<ksni::Icon> {
    if look.grey() {
        GREY.clone()
    } else {
        COLOUR.clone()
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::init();
    let cli = Cli::parse();
    let socket = cli.socket.unwrap_or_else(yakin_ipc::path::client_socket);
    let (requests, queue) = std::sync::mpsc::channel();
    let worker_socket = socket.clone();
    std::thread::spawn(move || work(&worker_socket, &queue));
    let tray = Tray {
        requests,
        snapshot: None,
    };
    let handle = tray.spawn()?;
    follow(&socket, &handle)
}

/// Stay subscribed, reconnecting with backoff; every event but input refetches the status.
fn follow(
    socket: &Path,
    handle: &Handle<Tray>,
) -> ! {
    let mut backoff = Backoff::default();
    loop {
        let start = Instant::now();
        match yakin_ipc::client::subscribe(socket) {
            Ok(events) => {
                refresh(socket, handle);
                let mut end = "end of stream".to_string();
                for event in events {
                    match event {
                        Ok(IpcEvent::State { .. } | IpcEvent::Menu(_) | IpcEvent::Keyboard(_)) => {}
                        Ok(_) => refresh(socket, handle),
                        Err(err) => {
                            // Not an io error: a frame this build cannot decode, so the daemon
                            // is newer. Exit and let systemd start the current build.
                            if err.downcast_ref::<std::io::Error>().is_none() {
                                error!("daemon speaks a newer protocol ({err}); exiting");
                                std::process::exit(3);
                            }
                            end = err.to_string();
                        }
                    }
                }
                warn!(
                    "subscription ended after {} ms: {end}",
                    start.elapsed().as_millis()
                );
            }
            Err(err) => warn!("subscribe: {err}"),
        }
        handle.update(|tray| tray.snapshot = None);
        std::thread::sleep(backoff.delay(start.elapsed()));
    }
}

fn refresh(
    socket: &Path,
    handle: &Handle<Tray>,
) {
    match yakin_ipc::client::request(socket, IpcRequest::Status) {
        Ok(IpcResponse::Status(status)) => {
            handle.update(|tray| tray.snapshot = Some(status));
        }
        Ok(response) => warn!("Unexpected response {response:?}"),
        Err(err) => warn!("{err}"),
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_icons() {
        for grey in [false, true] {
            let icons = icons(grey);
            assert_eq!(icons.len(), 2);
            for icon in icons {
                assert_eq!(icon.data.len(), (icon.width * icon.height * 4) as usize);
            }
        }
    }

    #[test]
    fn test_grey_icon_has_no_colour() {
        let icon = icon(ICON_32, true).unwrap();
        assert!(
            icon.data
                .as_chunks::<4>()
                .0
                .iter()
                .all(|argb| argb[1] == argb[2] && argb[2] == argb[3])
        );
    }
}
