//! Controllers over `/dev/hidraw*`: ioctls, feature reports, read loop and the hotplug rescan.

use std::collections::HashMap;
use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::os::fd::AsFd;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use anyhow::anyhow;
use crossbeam_channel::Sender;
use log::debug;
use log::info;
use log::trace;
use log::warn;
use nix::poll::PollFd;
use nix::poll::PollFlags;
use nix::poll::PollTimeout;

use crate::Result;
use crate::device::ControllerId;
use crate::device::Event;
use crate::device::ibex;
use crate::device::ibex::Report;
use crate::device::ibex::Wireless;

const SYSFS: &str = "/sys/class/hidraw";
const RESCAN: Duration = Duration::from_secs(2);
/// How long a reader waits for a report before it looks at [`Pause`] again.
const POLL_MS: u16 = 200;
/// How often and how long a feature report is retried on EIO (the kernel's own numbers).
const FEATURE_RETRIES: u32 = 50;
const FEATURE_RETRY: Duration = Duration::from_millis(20);
/// How often a reader retries lizard off and IMU on while the controller still answers EIO.
const SETUP_RETRY: Duration = Duration::from_secs(1);
const SETUP_RETRIES: u32 = 30;

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
    /// The node's inode: a node removed and created again under the same name gets a new one.
    ino: u64,
}
impl Hidraw {
    /// Open a hidraw node and check it is a 2026 Steam Controller (puck, wired or Bluetooth).
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let file = File::options().read(true).write(true).open(&path)?;
        let ino = file.metadata()?.ino();
        let mut info = DevInfo::default();
        unsafe { hidiocgrawinfo(file.as_raw_fd(), &mut info) }?;
        let vendor = info.vendor as u16;
        let product = info.product as u16;
        if vendor != ibex::VENDOR || !ibex::PRODUCTS.contains(&(info.bustype, product)) {
            return Err(anyhow!(
                "{} is {:04x}:{vendor:04x}:{product:04x}, not a Steam Controller",
                path.display(),
                info.bustype
            ));
        }
        Ok(Self { path, file, ino })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn ino(&self) -> u64 {
        self.ino
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
    ///
    /// Over Bluetooth the controller answers EIO for a moment after it announces its link
    /// (`0x79` connect), so EIO is retried for up to [`FEATURE_RETRIES`]; EPIPE is not, it is
    /// what an empty puck slot answers.
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
        for _ in 0..FEATURE_RETRIES {
            match unsafe { hidiocsfeature(self.file.as_raw_fd(), &mut buf) } {
                Err(nix::Error::EIO) => std::thread::sleep(FEATURE_RETRY),
                result => return result.map(drop).map_err(Into::into),
            }
        }
        Err(nix::Error::EIO.into())
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

    /// Stream raw accel and gyro, like hid-steam's `steam_sensor_open`; the puck needs it.
    pub fn imu_on(&self) -> Result<()> {
        self.send_feature(
            ibex::ID_SET_SETTINGS_VALUES,
            &[ibex::SETTING_IMU_MODE, ibex::IMU_MODE_RAW, 0],
        )
    }

    /// Power the controller down; through a puck slot it reaches the controller on it.
    pub fn power_off(&self) -> Result<()> {
        self.send_feature(ibex::ID_TURN_OFF_CONTROLLER, &[])
    }

    /// Give the controller back its keyboard/mouse emulation.
    pub fn lizard_on(&self) -> Result<()> {
        self.send_feature(ibex::ID_SET_DEFAULT_DIGITAL_MAPPINGS, &[])?;
        self.send_feature(ibex::ID_LOAD_DEFAULT_SETTINGS, &[])
    }

    /// Whether a report is ready within [`POLL_MS`].
    fn ready(&self) -> Result<bool> {
        let mut fds = [PollFd::new(self.file.as_fd(), PollFlags::POLLIN)];
        Ok(nix::poll::poll(&mut fds, PollTimeout::from(POLL_MS))? > 0)
    }

    /// Turn lizard mode off and the IMU on as asked. Ok(false) when the controller answered
    /// EIO, which a Bluetooth link does for a while after it comes up: worth another try.
    /// Any other error but EPIPE is retried the same way by [`Hidraw::run`].
    fn setup(
        &self,
        lizard_off: bool,
        imu: bool,
    ) -> Result<bool> {
        let busy = |err: &anyhow::Error| err.downcast_ref::<nix::Error>() == Some(&nix::Error::EIO);
        for (wanted, apply) in [
            (lizard_off, Self::lizard_off as fn(&Self) -> Result<()>),
            (imu, Self::imu_on),
        ] {
            if !wanted {
                continue;
            }
            match apply(self) {
                Ok(()) => {}
                Err(err) if busy(&err) => return Ok(false),
                Err(err) => return Err(err),
            }
        }
        Ok(true)
    }

    /// Read reports until the node goes away, `stop` or `pause` is set, sending decoded events.
    /// EIO, ENODEV or end of file on a read or poll means the node is gone. An
    /// [`Event::Lizard`] tells how the setup ended, once it has.
    pub fn run(
        self,
        tx_event: Sender<impl From<Event>>,
        lizard_off: bool,
        imu: bool,
        pause: &Pause,
        stop: &Pause,
    ) {
        let id = self.id();
        // None once set up, or once the controller refused for good
        let mut pending = Some((Instant::now(), 0_u32));
        let setup = |hidraw: &Self, pending: &mut Option<(Instant, u32)>| {
            let Some((_, tries)) = *pending else {
                return;
            };
            let again = |tries: u32| Some((Instant::now(), tries + 1));
            let is = |err: &anyhow::Error, errno| err.downcast_ref::<nix::Error>() == Some(&errno);
            let (next, lizard) = match hidraw.setup(lizard_off, imu) {
                Ok(true) => {
                    if tries > 0 {
                        info!("{id} set up after {tries} retries");
                    }
                    (None, Some(!lizard_off))
                }
                Ok(false) if tries + 1 < SETUP_RETRIES => (again(tries), None),
                Ok(false) => {
                    warn!("{id} still busy, lizard mode stays on");
                    (None, Some(true))
                }
                // Empty puck slots and the pogo interface refuse commands
                Err(err) if is(&err, nix::Error::EPIPE) => {
                    debug!("{id} setup: {err}");
                    (None, None)
                }
                Err(err) if tries + 1 < SETUP_RETRIES => {
                    if tries == 0 {
                        warn!("{id} setup: {err}, retrying");
                    }
                    (again(tries), None)
                }
                Err(err) => {
                    warn!("{id} setup: {err}, lizard mode stays on");
                    (None, Some(true))
                }
            };
            *pending = next;
            if let Some(on) = lizard {
                let _ = tx_event.send(Event::Lizard(id, on).into());
            }
        };
        // Opened first, so the setup's outcome finds the node known
        if tx_event
            .send(Event::Opened(id, self.path.clone()).into())
            .is_err()
        {
            return;
        }
        setup(&self, &mut pending);
        let mut buf = [0_u8; 64];
        while !pause.is_set() && !stop.is_set() {
            if let Some((last, _)) = pending
                && last.elapsed() >= SETUP_RETRY
            {
                setup(&self, &mut pending);
            }
            match self.ready() {
                Ok(true) => {}
                Ok(false) => continue,
                Err(err) => {
                    info!("{id} poll: {err}, closing");
                    break;
                }
            }
            let len = match self.read(&mut buf) {
                Ok(0) => {
                    info!("{id} end of file, closing");
                    break;
                }
                Ok(len) => len,
                Err(err) => {
                    info!("{id} read: {err}, closing");
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
                    if tx_event.send(Event::State(id, state).into()).is_err() {
                        return;
                    }
                }
                Report::Battery(battery) => {
                    if tx_event.send(Event::Battery(id, battery).into()).is_err() {
                        return;
                    }
                }
                Report::Wireless(wireless) => {
                    if wireless == Wireless::Connect {
                        pending = Some((Instant::now(), 0));
                        setup(&self, &mut pending);
                    }
                    if tx_event.send(Event::Wireless(id, wireless).into()).is_err() {
                        return;
                    }
                }
                Report::Other(report_id) => trace!("{id} unhandled report {report_id:#04x}"),
            }
        }
        if stop.is_set() {
            info!("{id} replaced by a new node, closing");
        }
        let _ = tx_event.send(Event::Closed(id).into());
    }
}

/// The link a node sits on, `HID_PHYS` from sysfs: a puck slot's USB interface or the
/// controller's Bluetooth address. The controller's own keyboard and mouse on that link, an
/// evdev device of another HID device, report the same string (see [`super::grab`]).
pub fn phys(path: &Path) -> Option<String> {
    let name = path.file_name()?;
    let uevent = std::fs::read_to_string(Path::new(SYSFS).join(name).join("device/uevent")).ok()?;
    uevent
        .lines()
        .find_map(|line| line.strip_prefix("HID_PHYS="))
        .map(str::to_owned)
}

/// The product id of a node, from `HID_ID` in sysfs: puck, wired or Bluetooth.
pub fn product(path: &Path) -> Option<u16> {
    let name = path.file_name()?;
    let uevent = std::fs::read_to_string(Path::new(SYSFS).join(name).join("device/uevent")).ok()?;
    let id = uevent
        .lines()
        .find_map(|line| line.strip_prefix("HID_ID="))?;
    u16::from_str_radix(id.rsplit(':').next()?, 16).ok()
}

/// hidraw nodes whose HID_ID is a 2026 Steam Controller: puck, wired or Bluetooth.
pub fn scan() -> Vec<PathBuf> {
    nodes().into_iter().map(|(path, _)| path).collect()
}

/// Like [`scan`], with each node's inode.
fn nodes() -> Vec<(PathBuf, u64)> {
    let wanted = ibex::PRODUCTS.map(|(bustype, product)| {
        format!("HID_ID={bustype:04X}:{:08X}:{product:08X}", ibex::VENDOR)
    });
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
        if !matches {
            continue;
        }
        let path = Path::new("/dev").join(entry.file_name());
        if let Ok(metadata) = std::fs::metadata(&path) {
            paths.push((path, metadata.ino()));
        }
    }
    paths.sort();
    paths
}

/// A running reader: the inode it holds and the flag that stops it.
struct Reader {
    ino: u64,
    stop: Pause,
}

/// Readers by path.
#[derive(Clone, Default)]
struct Open(Arc<Mutex<HashMap<PathBuf, Reader>>>);
impl Open {
    /// Paths to open, stopping readers whose node was replaced; those reopen once they end.
    fn plan(
        &self,
        nodes: &[(PathBuf, u64)],
    ) -> Vec<PathBuf> {
        let open = self.0.lock().unwrap();
        let mut paths = Vec::new();
        for (path, ino) in nodes {
            match open.get(path) {
                None => paths.push(path.clone()),
                Some(reader) if reader.ino != *ino && !reader.stop.is_set() => {
                    info!("{} was replaced, reopening", path.display());
                    reader.stop.set(true);
                }
                Some(_) => {}
            }
        }
        paths
    }

    fn insert(
        &self,
        path: &Path,
        ino: u64,
    ) -> Pause {
        let stop = Pause::default();
        let reader = Reader {
            ino,
            stop: stop.clone(),
        };
        self.0.lock().unwrap().insert(path.to_owned(), reader);
        stop
    }

    fn remove(
        &self,
        path: &Path,
    ) {
        self.0.lock().unwrap().remove(path);
    }
}

/// Set to close every node and open none until it is cleared again.
#[derive(Clone, Default)]
pub struct Pause(Arc<AtomicBool>);
impl Pause {
    pub fn set(
        &self,
        paused: bool,
    ) {
        self.0.store(paused, Ordering::Relaxed);
    }

    pub fn is_set(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Rescan every 2 s and start a reader thread for every new or replaced node, unless paused.
pub fn start(
    tx_event: Sender<impl From<Event> + Send + 'static>,
    lizard_off: bool,
    imu: bool,
    pause: Pause,
) {
    let _handle = std::thread::spawn(move || {
        let open = Open::default();
        let mut failed = HashSet::new();
        loop {
            let nodes = if pause.is_set() { Vec::new() } else { nodes() };
            for path in open.plan(&nodes) {
                let hidraw = match Hidraw::open(&path) {
                    Ok(hidraw) => hidraw,
                    Err(err) => {
                        if failed.insert(path.clone()) {
                            warn!("{err}");
                        }
                        continue;
                    }
                };
                failed.remove(&path);
                debug!("Opened {} (inode {})", path.display(), hidraw.ino());
                let stop = open.insert(&path, hidraw.ino());
                let open_ = open.clone();
                let tx_event_ = tx_event.clone();
                let pause_ = pause.clone();
                let _handle = std::thread::spawn(move || {
                    let path = hidraw.path().to_owned();
                    hidraw.run(tx_event_, lizard_off, imu, &pause_, &stop);
                    open_.remove(&path);
                });
            }
            std::thread::sleep(RESCAN);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(
        name: &str,
        ino: u64,
    ) -> (PathBuf, u64) {
        (PathBuf::from(name), ino)
    }

    #[test]
    fn opens_new_nodes() {
        let open = Open::default();
        let plan = open.plan(&[node("/dev/hidraw8", 1)]);
        assert_eq!(plan, vec![PathBuf::from("/dev/hidraw8")]);
    }

    #[test]
    fn keeps_a_held_node() {
        let open = Open::default();
        let stop = open.insert(Path::new("/dev/hidraw8"), 1);
        assert!(open.plan(&[node("/dev/hidraw8", 1)]).is_empty());
        assert!(!stop.is_set());
    }

    #[test]
    fn stops_a_reader_whose_node_was_replaced() {
        let open = Open::default();
        let stop = open.insert(Path::new("/dev/hidraw8"), 1);
        // Same name, new node: stop the old reader, reopen once it is gone
        assert!(open.plan(&[node("/dev/hidraw8", 2)]).is_empty());
        assert!(stop.is_set());
        open.remove(Path::new("/dev/hidraw8"));
        let plan = open.plan(&[node("/dev/hidraw8", 2)]);
        assert_eq!(plan, vec![PathBuf::from("/dev/hidraw8")]);
    }

    #[test]
    fn a_gone_node_opens_nothing() {
        let open = Open::default();
        let stop = open.insert(Path::new("/dev/hidraw8"), 1);
        assert!(open.plan(&[]).is_empty());
        assert!(!stop.is_set());
    }
}
