use std::path::Path;
use std::path::PathBuf;

use crate::device::ibex::Wireless;
use crate::state::Battery;
use crate::state::ControllerState;

pub mod bluetooth;
pub mod grab;
pub mod haptics;
pub mod hidraw;
pub mod holders;
pub mod ibex;

/// The N of the controller's `/dev/hidrawN`.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub struct ControllerId(pub u8);
impl From<&Path> for ControllerId {
    fn from(path: &Path) -> Self {
        let number = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix("hidraw"))
            .and_then(|number| number.parse().ok())
            .unwrap_or(u8::MAX);
        Self(number)
    }
}
impl From<ControllerId> for u8 {
    fn from(id: ControllerId) -> Self {
        id.0
    }
}
impl std::fmt::Display for ControllerId {
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        write!(f, "hidraw{}", self.0)
    }
}

#[derive(Clone, Debug)]
pub enum Event {
    Opened(ControllerId, PathBuf),
    State(ControllerId, ControllerState),
    Battery(ControllerId, Battery),
    Wireless(ControllerId, Wireless),
    /// Whether the controller's own keyboard and mouse are on, once the reader has set it up.
    Lizard(ControllerId, bool),
    Closed(ControllerId),
}
