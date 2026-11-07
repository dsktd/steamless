//! The daemon's side of desktop control, shared by every backend: actions out with their
//! failures logged, and the focused window in, reconnecting whenever the desktop is gone.

use std::time::Duration;
use std::time::Instant;

use crossbeam_channel::Sender;
use log::debug;
use log::info;
use log::warn;
use yakin_core::desktop::Action;
use yakin_core::desktop::Desktop;
use yakin_core::desktop::NONE;
use yakin_core::output::Output;

use crate::engine::Event;

/// Failures are logged at most this often, so a missing desktop does not flood the log.
const WARN_EVERY: Duration = Duration::from_secs(60);

/// First wait before watching again; doubles up to [`BACKOFF_MAX`].
const BACKOFF: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// Runs the desktop outputs of a report.
pub struct Control {
    desktop: Box<dyn Desktop>,
    warned: Option<Instant>,
}
impl Control {
    pub fn new(desktop: Box<dyn Desktop>) -> Self {
        Self {
            desktop,
            warned: None,
        }
    }

    /// The desktop actions go to, none when there is none.
    pub fn name(&self) -> Option<String> {
        Some(self.desktop.name())
            .filter(|name| *name != NONE)
            .map(String::from)
    }

    /// Run the desktop outputs; failures are logged, never returned.
    pub fn send(
        &mut self,
        outputs: &[Output],
    ) {
        for output in outputs {
            if let Output::Desktop(action) = output {
                self.act(action);
            }
        }
    }

    /// Whether the desktop ran `action`.
    fn act(
        &mut self,
        action: &Action,
    ) -> bool {
        let problem = match self.desktop.act(action) {
            Ok(true) => {
                info!("{action}");
                return true;
            }
            Ok(false) => format!("not supported by desktop {}", self.desktop.name()),
            Err(err) => err.to_string(),
        };
        if self
            .warned
            .is_none_or(|warned| warned.elapsed() >= WARN_EVERY)
        {
            warn!("{action}: {problem}");
            self.warned = Some(Instant::now());
        }
        false
    }
}

/// Follow the desktop's focus on a thread of its own, watching again whenever it ends.
pub fn start(
    mut desktop: Box<dyn Desktop>,
    tx_event: Sender<Event>,
) {
    let _handle = std::thread::spawn(move || {
        let mut backoff = BACKOFF;
        loop {
            let mut heard = false;
            let mut gone = false;
            let result = desktop.watch(&mut |window| {
                heard = true;
                gone |= tx_event
                    .send(Event::Focus(window.unwrap_or_default()))
                    .is_err();
            });
            if gone {
                return;
            }
            if let Err(err) = result {
                debug!("{} focus: {err}", desktop.name());
            }
            if heard {
                backoff = BACKOFF;
            }
            std::thread::sleep(backoff);
            backoff = (backoff * 2).min(BACKOFF_MAX);
        }
    });
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use yakin_core::Result;
    use yakin_core::auto::auto_profile;
    use yakin_core::desktop::Window;
    use yakin_core::profile::Config;

    use super::*;

    /// A desktop that records what it ran and reports a scripted focus.
    struct Fake {
        acts: Arc<Mutex<Vec<Action>>>,
        focus: Vec<Option<Window>>,
    }
    impl Desktop for Fake {
        fn act(
            &mut self,
            action: &Action,
        ) -> Result<bool> {
            self.acts.lock().unwrap().push(action.clone());
            Ok(!matches!(action, Action::Raw(_)))
        }

        fn watch(
            &mut self,
            focus: &mut dyn FnMut(Option<Window>),
        ) -> Result<()> {
            for window in self.focus.drain(..) {
                focus(window);
            }
            Ok(())
        }
    }

    fn window(app_id: &str) -> Option<Window> {
        Some(Window {
            app_id: app_id.to_string(),
            title: String::new(),
        })
    }

    #[test]
    fn runs_actions_and_tells_unsupported() {
        let acts = Arc::new(Mutex::new(Vec::new()));
        let fake = Fake {
            acts: acts.clone(),
            focus: Vec::new(),
        };
        let mut control = Control::new(Box::new(fake));
        assert!(control.act(&Action::Overview));
        assert!(!control.act(&Action::raw("quit").unwrap()));
        control.send(&[Output::Key(1, true), Output::Desktop(Action::Workspace(2))]);
        assert_eq!(
            *acts.lock().unwrap(),
            vec![
                Action::Overview,
                Action::raw("quit").unwrap(),
                Action::Workspace(2)
            ]
        );
    }

    #[test]
    fn focus_drives_auto_rules() {
        let config = Config::parse(
            "[profile.game.buttons]\na = \"key:KEY_A\"\n[profile.desk.buttons]\na = \"key:KEY_B\"\n[daemon]\nactive = \"desk\"\n[auto]\nenabled = true\ndefault = \"desk\"\n[[auto.rule]]\napp_id = \"steam_app_.*\"\nprofile = \"game\"",
        )
        .unwrap();
        let fake = Fake {
            acts: Arc::default(),
            focus: vec![window("steam_app_7"), None, window("foot")],
        };
        let (tx, rx) = crossbeam_channel::unbounded();
        start(Box::new(fake), tx);
        let picked: Vec<Option<&str>> = (0..3)
            .map(|_| match rx.recv_timeout(Duration::from_secs(5)) {
                Ok(Event::Focus(focus)) => auto_profile(&config.auto, &focus.app_id, &focus.title),
                _ => panic!("no focus"),
            })
            .collect();
        assert_eq!(picked, vec![Some("game"), Some("desk"), Some("desk")]);
    }
}
