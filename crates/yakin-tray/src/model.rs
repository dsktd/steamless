//! What the tray shows, as pure functions of the daemon's status.

use yakin_ipc::IpcHolder;
use yakin_ipc::IpcStatus;

/// What the daemon last told us; `None` while it is unreachable.
pub type Snapshot = Option<IpcStatus>;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Look {
    /// A controller is connected and the daemon has it.
    Active,
    /// The daemon is up but no controller is connected.
    NoController,
    /// The daemon stepped aside for another process.
    Yielded,
    Offline,
}
impl Look {
    pub fn of(snapshot: &Snapshot) -> Self {
        let Some(status) = snapshot else {
            return Self::Offline;
        };
        if status.yielded.is_some() {
            return Self::Yielded;
        }
        if status.controllers.is_empty() {
            return Self::NoController;
        }
        Self::Active
    }

    /// Whether the icon is drawn grey.
    pub const fn grey(self) -> bool {
        !matches!(self, Self::Active)
    }
}

/// One line of the tray menu; the tray turns these into widgets and actions.
#[derive(Debug, PartialEq)]
pub enum Entry {
    /// The profiles, the active one selected.
    Profiles {
        names: Vec<String>,
        selected: usize,
    },
    Separator,
    OpenConfiguration,
    Reload,
    /// Greyed out unless `Look::Active`.
    QuickMenu {
        enabled: bool,
    },
    Keyboard {
        enabled: bool,
    },
    /// Shown while yielded, with the holder's name.
    TakeBack(String),
}

pub fn menu(snapshot: &Snapshot) -> Vec<Entry> {
    let mut entries = Vec::new();
    if let Some(status) = snapshot {
        let selected = status
            .profiles
            .iter()
            .position(|name| *name == status.profile)
            .unwrap_or_default();
        entries.push(Entry::Profiles {
            names: status.profiles.clone(),
            selected,
        });
        entries.push(Entry::Separator);
        if let Some(IpcHolder { comm, .. }) = &status.yielded {
            entries.push(Entry::TakeBack(comm.clone()));
        }
        let enabled = Look::of(snapshot) == Look::Active;
        entries.push(Entry::QuickMenu { enabled });
        entries.push(Entry::Keyboard { enabled });
        entries.push(Entry::Reload);
    }
    entries.push(Entry::OpenConfiguration);
    entries
}

pub fn title(snapshot: &Snapshot) -> String {
    match Look::of(snapshot) {
        Look::Offline => "Yakin: daemon unreachable".to_string(),
        Look::NoController => "Yakin: no controller".to_string(),
        Look::Yielded => {
            let comm = snapshot
                .as_ref()
                .and_then(|status| status.yielded.as_ref())
                .map_or("another program", |holder| holder.comm.as_str());
            format!("Yakin: yielded to {comm}")
        }
        Look::Active => "Yakin".to_string(),
    }
}

/// Profile, controllers with their battery, and the holder while yielded, one per line.
pub fn description(snapshot: &Snapshot) -> String {
    let Some(status) = snapshot else {
        return String::new();
    };
    let mut lines = vec![format!("Profile: {}", status.profile)];
    for controller in status.controllers.iter().filter(|c| c.connected) {
        let battery = controller
            .battery
            .map(|percent| format!(", battery {percent}%"))
            .unwrap_or_default();
        lines.push(format!("Controller {}{battery}", controller.id));
    }
    if let Some(holder) = &status.yielded {
        lines.push(format!("Held by {}", holder.comm));
    }
    lines.join("\n")
}

#[cfg(test)]
mod test {
    use yakin_ipc::IpcController;

    use super::*;

    fn controller(battery: Option<u8>) -> IpcController {
        IpcController {
            id: 0,
            path: "/dev/hidraw0".to_string(),
            battery,
            wireless: true,
            link: yakin_ipc::IpcLink::Puck,
            connected: true,
            lizard: Some(false),
            leaks: 0,
            mouse: false,
        }
    }

    fn status() -> IpcStatus {
        IpcStatus {
            profile: "desktop".to_string(),
            controllers: vec![controller(Some(80))],
            profiles: vec!["desktop".to_string(), "game".to_string()],
            ..IpcStatus::default()
        }
    }

    #[test]
    fn test_look() {
        assert_eq!(Look::of(&None), Look::Offline);
        assert_eq!(Look::of(&Some(status())), Look::Active);
        let none = IpcStatus {
            controllers: vec![],
            ..status()
        };
        assert_eq!(Look::of(&Some(none)), Look::NoController);
        let yielded = IpcStatus {
            yielded: Some(IpcHolder {
                pid: 7,
                comm: "steam".to_string(),
            }),
            ..status()
        };
        assert_eq!(Look::of(&Some(yielded)), Look::Yielded);
        assert!(Look::Offline.grey());
        assert!(!Look::Active.grey());
    }

    #[test]
    fn test_menu() {
        let entries = menu(&Some(IpcStatus {
            profile: "game".to_string(),
            ..status()
        }));
        assert_eq!(
            entries,
            vec![
                Entry::Profiles {
                    names: vec!["desktop".to_string(), "game".to_string()],
                    selected: 1,
                },
                Entry::Separator,
                Entry::QuickMenu { enabled: true },
                Entry::Keyboard { enabled: true },
                Entry::Reload,
                Entry::OpenConfiguration,
            ]
        );
    }

    #[test]
    fn test_menu_yielded() {
        let entries = menu(&Some(IpcStatus {
            yielded: Some(IpcHolder {
                pid: 7,
                comm: "steam".to_string(),
            }),
            ..status()
        }));
        assert!(entries.contains(&Entry::TakeBack("steam".to_string())));
        assert!(entries.contains(&Entry::QuickMenu { enabled: false }));
        assert!(entries.contains(&Entry::Keyboard { enabled: false }));
    }

    #[test]
    fn test_menu_no_controller() {
        let entries = menu(&Some(IpcStatus {
            controllers: vec![],
            ..status()
        }));
        assert!(entries.contains(&Entry::QuickMenu { enabled: false }));
        assert!(entries.contains(&Entry::Keyboard { enabled: false }));
    }

    #[test]
    fn test_menu_offline() {
        assert_eq!(menu(&None), vec![Entry::OpenConfiguration]);
    }

    #[test]
    fn test_title() {
        assert_eq!(title(&None), "Yakin: daemon unreachable");
        assert_eq!(title(&Some(status())), "Yakin");
        let yielded = |comm: &str| {
            title(&Some(IpcStatus {
                yielded: Some(IpcHolder {
                    pid: 7,
                    comm: comm.to_string(),
                }),
                ..status()
            }))
        };
        assert_eq!(yielded("steam"), "Yakin: yielded to steam");
        assert_eq!(yielded("sunshine"), "Yakin: yielded to sunshine");
    }

    #[test]
    fn test_description() {
        assert_eq!(
            description(&Some(status())),
            "Profile: desktop\nController 0, battery 80%"
        );
        assert_eq!(description(&None), "");
    }
}
