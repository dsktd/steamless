//! Controllers over `/dev/hidraw*`: ioctls, feature reports, read loop and the hotplug rescan.

use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::anyhow;
use crossbeam_channel::Sender;
use log::debug;
use log::trace;
use log::warn;

use crate::Result;
use crate::device::ControllerId;
use crate::device::Event;
use crate::device::ibex;
use crate::device::ibex::Report;
use crate::device::ibex::Wireless;

const SYSFS: &str = "/sys/class/hidraw";
const RESCAN: Duration = Duration::from_secs(2);

/// `struct hidraw_devinfo` from `linux/hidraw.h`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct DevInfo {
    pub bustype: u32,
    pub vendor: i16,
    pub product: i16,
}

nix::ioctl_read!(hidiocgrawinfo, b'H', 0x03, DevInfo);
nix::ioctl_readwrite_buf!(hidiocsfeature, b'H', 0x06, u8);

pub struct Hidraw {
    path: PathBuf,
    file: File,
}
impl Hidraw {
    /// Open a hidraw node and check it is a 2026 Steam Controller (puck or wired).
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let file = File::options().read(true).write(true).open(&path)?;
        let mut info = DevInfo::default();
        unsafe { hidiocgrawinfo(file.as_raw_fd(), &mut info) }?;
        let vendor = info.vendor as u16;
        let product = info.product as u16;
        if vendor != ibex::VENDOR || !matches!(product, ibex::PRODUCT_PUCK | ibex::PRODUCT_WIRED) {
            return Err(anyhow!(
                "{} is {vendor:04x}:{product:04x}, not a Steam Controller",
                path.display()
            ));
        }
        Ok(Self { path, file })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn id(&self) -> ControllerId {
        ControllerId::from(self.path.as_path())
    }

    /// Blocking read of one input report, report id included.
    pub fn read(
        &self,
        buf: &mut [u8],
    ) -> Result<usize> {
        Ok((&self.file).read(buf)?)
    }

    /// Send `[0x01, cmd, len, payload..]` as feature report 1, padded to 64 bytes.
    // TODO Retry on EPIPE like the kernel does for wireless
    pub fn send_feature(
        &self,
        cmd: u8,
        payload: &[u8],
    ) -> Result<()> {
        let mut buf = [0_u8; 64];
        if payload.len() > buf.len() - 3 {
            return Err(anyhow!(
                "Feature payload of {} bytes is too long",
                payload.len()
            ));
        }
        buf[0] = ibex::REPORT_ID_FEATURES;
        buf[1] = cmd;
        buf[2] = payload.len() as u8;
        buf[3..3 + payload.len()].copy_from_slice(payload);
        trace!(
            "{} feature {:02x?}",
            self.path.display(),
            &buf[..3 + payload.len()]
        );
        unsafe { hidiocsfeature(self.file.as_raw_fd(), &mut buf) }?;
        Ok(())
    }

    /// Turn off the firmware's keyboard/mouse emulation and the Steam watchdog.
    pub fn lizard_off(&self) -> Result<()> {
        self.send_feature(ibex::ID_CLEAR_DIGITAL_MAPPINGS, &[])?;
        self.send_feature(
            ibex::ID_SET_SETTINGS_VALUES,
            &[
                ibex::SETTING_LIZARD_MODE,
                0,
                0,
                ibex::SETTING_STEAM_WATCHDOG_ENABLE,
                0,
                0,
            ],
        )
    }

    /// Give the controller back its keyboard/mouse emulation.
    pub fn lizard_on(&self) -> Result<()> {
        self.send_feature(ibex::ID_SET_DEFAULT_DIGITAL_MAPPINGS, &[])?;
        self.send_feature(ibex::ID_LOAD_DEFAULT_SETTINGS, &[])
    }

    /// Read reports until the node goes away, sending decoded events.
    pub fn run(
        self,
        tx_event: Sender<Event>,
        lizard_off: bool,
    ) {
        let id = self.id();
        if lizard_off && let Err(err) = self.lizard_off() {
            // Empty puck slots and the pogo interface refuse commands
            debug!("{id} lizard off: {err}");
        }
        if tx_event.send(Event::Opened(id, self.path.clone())).is_err() {
            return;
        }
        let mut buf = [0_u8; 64];
        loop {
            let len = match self.read(&mut buf) {
                Ok(0) => break,
                Ok(len) => len,
                Err(err) => {
                    debug!("{id} read: {err}");
                    break;
                }
            };
            let report = &buf[..len];
            trace!("{id} {report:02x?}");
            let report = match ibex::decode(report) {
                Ok(report) => report,
                Err(err) => {
                    warn!("{id} {err}");
                    continue;
                }
            };
            match report {
                Report::Input(state) => {
                    if tx_event.send(Event::State(id, state)).is_err() {
                        return;
                    }
                }
                Report::Battery(battery) => {
                    if tx_event.send(Event::Battery(id, battery)).is_err() {
                        return;
                    }
                }
                Report::Wireless(wireless) => {
                    if lizard_off
                        && wireless == Wireless::Connect
                        && let Err(err) = self.lizard_off()
                    {
                        warn!("{id} lizard off: {err}");
                    }
                    if tx_event.send(Event::Wireless(id, wireless)).is_err() {
                        return;
                    }
                }
                Report::Other(report_id) => trace!("{id} unhandled report {report_id:#04x}"),
            }
        }
        let _ = tx_event.send(Event::Closed(id));
    }
}

/// hidraw nodes whose HID_ID is a 2026 Steam Controller, puck or wired.
pub fn scan() -> Vec<PathBuf> {
    let wanted = [ibex::PRODUCT_PUCK, ibex::PRODUCT_WIRED]
        .map(|product| format!("HID_ID=0003:{:08X}:{:08X}", ibex::VENDOR, product));
    let Ok(entries) = std::fs::read_dir(SYSFS) else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let Ok(uevent) = std::fs::read_to_string(entry.path().join("device/uevent")) else {
            continue;
        };
        let matches = uevent
            .lines()
            .any(|line| wanted.iter().any(|id| line.eq_ignore_ascii_case(id)));
        if matches {
            paths.push(Path::new("/dev").join(entry.file_name()));
        }
    }
    paths.sort();
    paths
}

/// Paths with a reader thread running.
#[derive(Clone, Default)]
struct Open(Arc<Mutex<HashSet<PathBuf>>>);
impl Open {
    fn insert(
        &self,
        path: &Path,
    ) -> bool {
        let mut open = self.0.lock().unwrap();
        open.insert(path.to_owned())
    }

    fn remove(
        &self,
        path: &Path,
    ) {
        let mut open = self.0.lock().unwrap();
        open.remove(path);
    }
}

/// Rescan every 2 s and start a reader thread for every new node.
pub fn start(
    tx_event: Sender<Event>,
    lizard_off: bool,
) {
    let _handle = std::thread::spawn(move || {
        let open = Open::default();
        let mut failed = HashSet::new();
        loop {
            for path in scan() {
                if !open.insert(&path) {
                    continue;
                }
                let hidraw = match Hidraw::open(&path) {
                    Ok(hidraw) => hidraw,
                    Err(err) => {
                        if failed.insert(path.clone()) {
                            warn!("{err}");
                        }
                        open.remove(&path);
                        continue;
                    }
                };
                failed.remove(&path);
                debug!("Opened {}", path.display());
                let open_ = open.clone();
                let tx_event_ = tx_event.clone();
                let _handle = std::thread::spawn(move || {
                    let path = hidraw.path().to_owned();
                    hidraw.run(tx_event_, lizard_off);
                    open_.remove(&path);
                });
            }
            std::thread::sleep(RESCAN);
        }
    });
}
