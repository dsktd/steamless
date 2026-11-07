//! What the daemon streams, kept in signals the pages read: the connection, the active profile,
//! every profile's tuning, the controllers and the latest input.

use std::time::Duration;

use dioxus::prelude::*;
use yakin_core::state::ControllerState;
use yakin_ipc::IpcAuto;
use yakin_ipc::IpcController;
use yakin_ipc::IpcEvent;
use yakin_ipc::IpcHolder;
use yakin_ipc::IpcTuning;

use crate::daemon::Daemon;
use crate::daemon::Event;
use crate::editor::Editor;

/// A stick past this share of its range, or a trigger past it, counts as moved.
const MOVED: f32 = 0.3;
const RANGE: f32 = 32767.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Live {
    pub connected: bool,
    /// What the connection strip shows while not connected.
    pub status: String,
    pub profile: Option<String>,
    pub controllers: Vec<IpcController>,
    pub names: Vec<String>,
    pub tunings: Vec<IpcTuning>,
    /// The focused app id and the rule it matched, while `[auto]` is enabled.
    pub auto: Option<IpcAuto>,
    pub yielded: Option<IpcHolder>,
    /// A tune request is on its way: the daemon's echo of an older one is not taken.
    pub tuning_pending: bool,
    /// The last request's error, cleared by the next one that works.
    pub error: String,
}
impl Live {
    pub fn tuning(
        &self,
        profile: &str,
    ) -> Option<&IpcTuning> {
        self.tunings.iter().find(|tuning| tuning.profile == profile)
    }

    pub fn set_tuning(
        &mut self,
        tuning: IpcTuning,
    ) {
        match self
            .tunings
            .iter_mut()
            .find(|t| t.profile == tuning.profile)
        {
            Some(slot) => *slot = tuning,
            None => self.tunings.push(tuning),
        }
    }

    /// The outcome of a request, shown in the sidebar.
    pub fn report(
        &mut self,
        result: Result<(), String>,
    ) {
        self.error = result.err().unwrap_or_default();
    }
}

/// The sources that are pressed, touched or moved, in the vocabulary's order (bit order).
pub fn lit(
    state: &ControllerState,
    buttons: &[String],
) -> Vec<String> {
    let bits = state.buttons.bits();
    let mut lit: Vec<String> = buttons
        .iter()
        .enumerate()
        .filter(|(bit, _)| (bits >> bit) & 1 == 1)
        .map(|(_, name)| name.clone())
        .collect();
    let far = |x: i16, y: i16| (x as f32).hypot(y as f32) > MOVED * RANGE;
    if far(state.lstick.x, state.lstick.y) {
        lit.push("lstick".to_string());
    }
    if far(state.rstick.x, state.rstick.y) {
        lit.push("rstick".to_string());
    }
    if state.lpad.touched {
        lit.push("lpad".to_string());
    }
    if state.rpad.touched {
        lit.push("rpad".to_string());
    }
    if state.lt as f32 > MOVED * RANGE {
        lit.push("lt".to_string());
    }
    if state.rt as f32 > MOVED * RANGE {
        lit.push("rt".to_string());
    }
    lit
}

/// Subscribe for the page's lifetime and keep the signals current.
pub fn use_follow(
    daemon: Daemon,
    mut live: Signal<Live>,
    mut state: Signal<ControllerState>,
    mut editor: Signal<Editor>,
) {
    use_future(move || {
        let daemon = daemon.clone();
        async move {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            daemon.follow(tx);
            while let Some(event) = rx.recv().await {
                match event {
                    Event::Connection(connected) => {
                        let mut live = live.write();
                        live.connected = connected;
                        live.status = if connected {
                            String::new()
                        } else {
                            "daemon not running, retrying".to_string()
                        };
                        if connected {
                            // A daemon that comes back may have another file
                            editor.write().load(&daemon);
                        } else {
                            live.controllers.clear();
                            live.yielded = None;
                        }
                    }
                    Event::Failed(err) => live.write().status = err,
                    Event::Ipc(IpcEvent::State { state: next, .. }) => {
                        if *state.peek() != next {
                            state.set(next);
                        }
                    }
                    Event::Ipc(IpcEvent::Profile(name)) => {
                        // Sent on subscribe, activate and reload: the moments the tunings may
                        // have changed
                        let status = daemon.status();
                        let mut live = live.write();
                        live.profile = Some(name.clone());
                        if let Ok(status) = status {
                            live.names = status.profiles;
                            live.tunings = status.tunings;
                            live.auto = status.auto;
                        }
                        // Follow the live profile until the user picks another one here
                        let mut editor = editor.write();
                        if editor.editing.is_none() {
                            editor.editing = Some(name);
                        }
                    }
                    Event::Ipc(IpcEvent::Controllers(controllers)) => {
                        live.write().controllers = controllers;
                    }
                    Event::Ipc(IpcEvent::Tuning(tuning)) => {
                        let mut live = live.write();
                        let active = live.profile.as_deref() == Some(tuning.profile.as_str());
                        if !(active && live.tuning_pending) {
                            live.set_tuning(tuning);
                        }
                    }
                    Event::Ipc(IpcEvent::Config(config)) => editor.write().set_file(*config, false),
                    Event::Ipc(IpcEvent::Yielded(holder)) => live.write().yielded = holder,
                    // The overlay draws the menu and keyboard; the window has nothing to show
                    Event::Ipc(IpcEvent::Menu(_) | IpcEvent::Keyboard(_)) => {}
                }
            }
        }
    });
}

/// At most one tune request per this long while a slider moves: the first move goes out at
/// once, the last one always goes out.
pub const THROTTLE: Duration = Duration::from_millis(100);
