//! The controller's own keyboard and mouse ("lizard mode") as the kernel shows them: one evdev
//! device per link, grabbed (`EVIOCGRAB`) while yakin drives the controller, so that nothing the
//! firmware types on its own reaches the desktop next to yakin's. The grabbed devices are read
//! too, never forwarded: input there while lizard mode should be off is a leak, the firmware
//! having turned it back on behind yakin's back.

use std::collections::HashMap;
use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::path::PathBuf;

use evdev::Device;
use evdev::EventType;
use log::info;
use log::warn;

use crate::device::ibex;

pub const SYSFS: &str = "/sys/class/input";
pub const DEV: &str = "/dev/input";

/// A lizard device on one link.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Link {
    pub event: PathBuf,
    /// `phys`, the same string as the hidraw node's `HID_PHYS` on that link.
    pub phys: String,
    /// The node's inode: a link that comes back may reuse the name.
    pub ino: u64,
}

/// The lizard devices under `sys` (`/sys/class/input`) with nodes under `dev` (`/dev/input`):
/// a 2026 Steam Controller's evdev device with relative axes. hid-steam's own gamepad and
/// motion sensors, which share the vendor and product, have none.
pub fn links(
    sys: &Path,
    dev: &Path,
) -> Vec<Link> {
    let Ok(entries) = std::fs::read_dir(sys) else {
        return Vec::new();
    };
    let read = |dir: &Path, name: &str| {
        std::fs::read_to_string(dir.join(name)).map(|text| text.trim().to_owned())
    };
    let mut links = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("event") {
            continue;
        }
        let device = entry.path().join("device");
        let id = |name: &str| u16::from_str_radix(&read(&device, name).ok()?, 16).ok();
        let vendor = id("id/vendor");
        let product = id("id/product");
        let rel = read(&device, "capabilities/rel").unwrap_or_default();
        let is_lizard = vendor == Some(ibex::VENDOR)
            && product.is_some_and(|product| ibex::PRODUCTS.iter().any(|(_, p)| *p == product))
            && rel.chars().any(|c| c != '0' && c != ' ');
        if !is_lizard {
            continue;
        }
        let Ok(phys) = read(&device, "phys") else {
            continue;
        };
        let event = dev.join(name);
        if let Ok(metadata) = std::fs::metadata(&event) {
            links.push(Link {
                event,
                phys,
                ino: metadata.ino(),
            });
        }
    }
    links.sort_by(|a, b| a.event.cmp(&b.event));
    links
}

/// A held grab: the device is kept open for it and ungrabbed by its drop.
struct Grab {
    device: Device,
    ino: u64,
    phys: String,
    /// Leaking at the last read, so a burst over several reads is one leak.
    burst: bool,
}

/// A burst of keys or motion on a grabbed lizard device while lizard mode should be off.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Leak {
    pub phys: String,
    pub events: usize,
}

/// What `events` keys or motions read from a link's lizard device since the last read call for:
/// a leak to act on once at the start of a burst; while lizard mode is on they are expected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Verdict {
    Quiet,
    Expected,
    /// A new burst: warn and turn lizard mode off again.
    Leak,
    /// The burst goes on; already acted on.
    Leaking,
}
impl Verdict {
    pub fn judge(
        events: usize,
        off: bool,
        burst: bool,
    ) -> Self {
        match (events, off, burst) {
            (0, _, _) => Verdict::Quiet,
            (_, false, _) => Verdict::Expected,
            (_, true, false) => Verdict::Leak,
            (_, true, true) => Verdict::Leaking,
        }
    }

    /// Whether a burst is under way after this read.
    pub fn burst(self) -> bool {
        matches!(self, Verdict::Leak | Verdict::Leaking)
    }
}

/// The grabs held, by node.
#[derive(Default)]
pub struct Grabs {
    held: HashMap<PathBuf, Grab>,
    /// Nodes whose grab failed, warned about once.
    failed: HashSet<PathBuf>,
}
impl Grabs {
    /// Grab every lizard device whose link `wanted` says yes to and release the others, from
    /// a fresh look at sysfs.
    pub fn sync(
        &mut self,
        wanted: impl Fn(&str) -> bool,
    ) {
        let links = links(Path::new(SYSFS), Path::new(DEV));
        let (grab, release) = plan(
            self.held.iter().map(|(path, grab)| (path, grab.ino)),
            &links,
            wanted,
        );
        for path in release {
            if self.held.remove(&path).is_some() {
                info!("Released {}", path.display());
            }
        }
        for link in grab {
            let grabbed = Device::open(&link.event).and_then(|mut device| {
                device.grab()?;
                device.set_nonblocking(true)?;
                Ok(device)
            });
            match grabbed {
                Ok(device) => {
                    info!("Grabbed {} ({})", link.event.display(), link.phys);
                    self.failed.remove(&link.event);
                    self.held.insert(
                        link.event,
                        Grab {
                            device,
                            ino: link.ino,
                            phys: link.phys,
                            burst: false,
                        },
                    );
                }
                Err(err) => {
                    if self.failed.insert(link.event.clone()) {
                        warn!("Grab {}: {err}", link.event.display());
                    }
                }
            }
        }
    }

    /// Drain what the grabbed devices queued since the last call and return the new leaks: keys
    /// or motion on a link that `off` says should have lizard mode off.
    pub fn read(
        &mut self,
        off: impl Fn(&str) -> bool,
    ) -> Vec<Leak> {
        let mut leaks = Vec::new();
        for grab in self.held.values_mut() {
            let events = match grab.device.fetch_events() {
                Ok(events) => events
                    .filter(|event| {
                        matches!(event.event_type(), EventType::KEY | EventType::RELATIVE)
                    })
                    .count(),
                Err(_) => 0,
            };
            let verdict = Verdict::judge(events, off(&grab.phys), grab.burst);
            grab.burst = verdict.burst();
            if verdict == Verdict::Leak {
                leaks.push(Leak {
                    phys: grab.phys.clone(),
                    events,
                });
            }
        }
        leaks.sort_by(|a, b| a.phys.cmp(&b.phys));
        leaks
    }

    /// The nodes held.
    pub fn held(&self) -> Vec<PathBuf> {
        let mut held: Vec<PathBuf> = self.held.keys().cloned().collect();
        held.sort();
        held
    }
}

/// Which links to grab and which nodes to let go: a node held is released when its link is
/// gone or replaced (new inode) or no longer wanted; dropping the device ungrabs it.
fn plan<'a>(
    held: impl Iterator<Item = (&'a PathBuf, u64)>,
    links: &[Link],
    wanted: impl Fn(&str) -> bool,
) -> (Vec<Link>, Vec<PathBuf>) {
    let held: HashMap<&PathBuf, u64> = held.collect();
    let mut grab = Vec::new();
    let mut keep = HashSet::new();
    for link in links {
        let fresh = held.get(&link.event) == Some(&link.ino);
        match (wanted(&link.phys), fresh) {
            (true, true) => {
                keep.insert(link.event.clone());
            }
            (true, false) => grab.push(link.clone()),
            (false, _) => {}
        }
    }
    let release = held
        .into_keys()
        .filter(|path| !keep.contains(*path))
        .cloned()
        .collect();
    (grab, release)
}

#[cfg(test)]
mod test {
    use super::*;

    fn link(
        event: &str,
        phys: &str,
        ino: u64,
    ) -> Link {
        Link {
            event: PathBuf::from(event),
            phys: phys.to_owned(),
            ino,
        }
    }

    fn sorted(mut paths: Vec<PathBuf>) -> Vec<PathBuf> {
        paths.sort();
        paths
    }

    #[test]
    fn test_leak_verdicts() {
        // Nothing read: quiet, and a burst ends
        assert_eq!(Verdict::judge(0, true, true), Verdict::Quiet);
        assert!(!Verdict::Quiet.burst());
        // Lizard mode held on by a binding: its input is expected
        assert_eq!(Verdict::judge(5, false, false), Verdict::Expected);
        assert_eq!(Verdict::judge(5, false, true), Verdict::Expected);
        // Input while it should be off: one leak, then the burst goes on quietly
        assert_eq!(Verdict::judge(3, true, false), Verdict::Leak);
        assert_eq!(Verdict::judge(3, true, true), Verdict::Leaking);
        assert!(Verdict::Leak.burst());
        assert!(Verdict::Leaking.burst());
    }

    #[test]
    fn test_grabs_wanted_links() {
        let links = [
            link("/dev/input/event2", "usb-1/input2", 1),
            link("/dev/input/event8", "aa:bb", 2),
        ];
        let (grab, release) = plan(std::iter::empty(), &links, |phys| phys == "aa:bb");
        assert_eq!(grab, vec![links[1].clone()]);
        assert!(release.is_empty());
    }

    #[test]
    fn test_keeps_and_releases() {
        let held = [
            (PathBuf::from("/dev/input/event2"), 1),
            (PathBuf::from("/dev/input/event8"), 2),
        ];
        let links = [
            link("/dev/input/event2", "usb-1/input2", 1),
            link("/dev/input/event8", "aa:bb", 2),
        ];
        // Both wanted: nothing to do
        let (grab, release) = plan(held.iter().map(|(p, i)| (p, *i)), &links, |_| true);
        assert!(grab.is_empty());
        assert!(release.is_empty());
        // A lizard binding hands the Bluetooth link over: release it, keep the puck
        let (grab, release) = plan(held.iter().map(|(p, i)| (p, *i)), &links, |phys| {
            phys != "aa:bb"
        });
        assert!(grab.is_empty());
        assert_eq!(release, vec![PathBuf::from("/dev/input/event8")]);
    }

    #[test]
    fn test_replaced_and_gone_nodes() {
        let held = [
            (PathBuf::from("/dev/input/event2"), 1),
            (PathBuf::from("/dev/input/event8"), 2),
        ];
        // event8 came back as a new device under the same name, event2 is gone
        let links = [link("/dev/input/event8", "aa:bb", 3)];
        let (grab, release) = plan(held.iter().map(|(p, i)| (p, *i)), &links, |_| true);
        assert_eq!(grab, vec![links[0].clone()]);
        assert_eq!(
            sorted(release),
            vec![
                PathBuf::from("/dev/input/event2"),
                PathBuf::from("/dev/input/event8")
            ]
        );
    }

    #[test]
    fn test_links_from_sysfs() {
        let root = std::env::temp_dir().join(format!("yakin-grab-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let sys = root.join("sys");
        let dev = root.join("dev");
        std::fs::create_dir_all(&dev).unwrap();
        let device = |event: &str, vendor: &str, product: &str, rel: &str, phys: &str| {
            let dir = sys.join(event).join("device");
            std::fs::create_dir_all(dir.join("id")).unwrap();
            std::fs::create_dir_all(dir.join("capabilities")).unwrap();
            std::fs::write(dir.join("id/vendor"), format!("{vendor}\n")).unwrap();
            std::fs::write(dir.join("id/product"), format!("{product}\n")).unwrap();
            std::fs::write(dir.join("capabilities/rel"), format!("{rel}\n")).unwrap();
            std::fs::write(dir.join("phys"), format!("{phys}\n")).unwrap();
            std::fs::write(dev.join(event), "").unwrap();
        };
        device(
            "event2",
            "28de",
            "1304",
            "1943",
            "usb-0000:00:14.0-3/input2",
        );
        device("event8", "28de", "1303", "1943", "70:cd:0d:43:a0:bb");
        // hid-steam's gamepad: no relative axes
        device("event9", "28de", "1303", "0", "70:cd:0d:43:a0:bb");
        device("event0", "0001", "0001", "0", "isa0060/serio0/input0");
        std::fs::create_dir_all(sys.join("mouse0")).unwrap();
        let found = links(&sys, &dev);
        std::fs::remove_dir_all(&root).unwrap();
        let phys: Vec<(String, String)> = found
            .iter()
            .map(|link| {
                (
                    link.event
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    link.phys.clone(),
                )
            })
            .collect();
        assert_eq!(
            phys,
            vec![
                ("event2".to_owned(), "usb-0000:00:14.0-3/input2".to_owned()),
                ("event8".to_owned(), "70:cd:0d:43:a0:bb".to_owned()),
            ]
        );
        assert!(links(Path::new("/nonexistent"), &dev).is_empty());
    }
}
