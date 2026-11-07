//! Which daemon events deserve a toast. A fresh subscription first sends the current profile,
//! controllers and yield: that is state, not a change, so it only fills `State`.

use yakin_ipc::IpcController;
use yakin_ipc::IpcEvent;
use yakin_ipc::IpcHolder;

/// Below this battery percentage a controller warns once...
const BATTERY_LOW: u8 = 15;

/// ...until it charges above this one.
const BATTERY_RECOVERED: u8 = 20;

#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub title: String,
    pub subtitle: Option<String>,
}

/// What the subscription has told us so far; `None` until the first event of that kind.
#[derive(Default)]
pub struct State {
    profile: Option<String>,
    controllers: Option<Vec<IpcController>>,
    yielded: Option<Option<IpcHolder>>,
    /// Controllers already warned about a low battery.
    low: Vec<u8>,
}

impl State {
    pub fn apply(
        &mut self,
        event: &IpcEvent,
    ) {
        match event {
            IpcEvent::Profile(name) => self.profile = Some(name.clone()),
            IpcEvent::Controllers(controllers) => {
                for controller in controllers {
                    match controller.battery {
                        Some(level)
                            if level < BATTERY_LOW && !self.low.contains(&controller.id) =>
                        {
                            self.low.push(controller.id)
                        }
                        Some(level) if level > BATTERY_RECOVERED => {
                            self.low.retain(|id| *id != controller.id)
                        }
                        _ => {}
                    }
                }
                self.controllers = Some(controllers.clone());
            }
            IpcEvent::Yielded(holder) => self.yielded = Some(holder.clone()),
            _ => {}
        }
    }
}

/// The toast `event` deserves after `prev`, if any.
pub fn toast(
    prev: &State,
    event: &IpcEvent,
) -> Option<Toast> {
    match event {
        IpcEvent::Profile(name) => match &prev.profile {
            Some(old) if old != name => Some(Toast::new(name, Some("profile"))),
            _ => None,
        },
        IpcEvent::Controllers(controllers) => {
            let old = prev.controllers.as_ref()?;
            if controllers
                .iter()
                .any(|c| !old.iter().any(|o| o.id == c.id))
            {
                return Some(Toast::new("Controller connected", None));
            }
            if old
                .iter()
                .any(|o| !controllers.iter().any(|c| c.id == o.id))
            {
                return Some(Toast::new("Controller disconnected", None));
            }
            // A `lizard:` binding took the controller over or handed it back
            for c in controllers {
                if let Some(o) = old.iter().find(|o| o.id == c.id)
                    && o.mouse != c.mouse
                {
                    let title = if c.mouse {
                        "Mouse mode"
                    } else {
                        "Mouse mode off"
                    };
                    return Some(Toast::new(title, None));
                }
            }
            controllers.iter().find_map(|c| match c.battery {
                Some(level) if level < BATTERY_LOW && !prev.low.contains(&c.id) => {
                    Some(Toast::new("Battery low", Some(&format!("{level} %"))))
                }
                _ => None,
            })
        }
        IpcEvent::Yielded(Some(holder)) => match &prev.yielded {
            Some(None) => Some(Toast::new("Steam has the controller", Some(&holder.comm))),
            _ => None,
        },
        IpcEvent::Yielded(None) => match &prev.yielded {
            Some(Some(_)) => Some(Toast::new("Controller back", None)),
            _ => None,
        },
        _ => None,
    }
}

impl Toast {
    pub fn new(
        title: &str,
        subtitle: Option<&str>,
    ) -> Self {
        Self {
            title: title.to_string(),
            subtitle: subtitle.map(str::to_string),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn controller(
        id: u8,
        battery: Option<u8>,
    ) -> IpcController {
        IpcController {
            id,
            path: format!("/dev/hidraw{id}"),
            battery,
            wireless: true,
            link: yakin_ipc::IpcLink::Puck,
            connected: true,
            lizard: Some(false),
            leaks: 0,
            mouse: false,
        }
    }

    fn holder() -> IpcHolder {
        IpcHolder {
            pid: 42,
            comm: "steam".into(),
        }
    }

    /// The three events a fresh subscription opens with.
    fn subscribed() -> State {
        let mut state = State::default();
        for event in [
            IpcEvent::Profile("desktop".into()),
            IpcEvent::Controllers(vec![controller(0, Some(80))]),
            IpcEvent::Yielded(None),
        ] {
            assert_eq!(toast(&state, &event), None);
            state.apply(&event);
        }
        state
    }

    /// Feed `events` in order, returning the toasts they produced.
    fn run(
        state: &mut State,
        events: &[IpcEvent],
    ) -> Vec<Toast> {
        let mut toasts = Vec::new();
        for event in events {
            toasts.extend(toast(state, event));
            state.apply(event);
        }
        toasts
    }

    #[test]
    fn test_mouse_mode() {
        let mut state = subscribed();
        let mouse = IpcController {
            mouse: true,
            ..controller(0, Some(80))
        };
        let toasts = run(
            &mut state,
            &[
                IpcEvent::Controllers(vec![mouse]),
                IpcEvent::Controllers(vec![controller(0, Some(80))]),
            ],
        );
        assert_eq!(
            toasts,
            vec![
                Toast::new("Mouse mode", None),
                Toast::new("Mouse mode off", None)
            ]
        );
    }

    #[test]
    fn test_initial_events_are_silent() {
        let state = subscribed();
        assert_eq!(state.profile.as_deref(), Some("desktop"));
    }

    #[test]
    fn test_initial_yield_is_silent() {
        let mut state = State::default();
        assert!(run(&mut state, &[IpcEvent::Yielded(Some(holder()))]).is_empty());
        let toasts = run(&mut state, &[IpcEvent::Yielded(None)]);
        assert_eq!(toasts, vec![Toast::new("Controller back", None)]);
    }

    #[test]
    fn test_profile_change() {
        let mut state = subscribed();
        let toasts = run(
            &mut state,
            &[
                IpcEvent::Profile("desktop".into()),
                IpcEvent::Profile("couch".into()),
            ],
        );
        assert_eq!(toasts, vec![Toast::new("couch", Some("profile"))]);
    }

    #[test]
    fn test_yield_and_back() {
        let mut state = subscribed();
        let toasts = run(
            &mut state,
            &[
                IpcEvent::Yielded(Some(holder())),
                IpcEvent::Yielded(None),
                IpcEvent::Yielded(None),
            ],
        );
        assert_eq!(
            toasts,
            vec![
                Toast::new("Steam has the controller", Some("steam")),
                Toast::new("Controller back", None),
            ]
        );
    }

    #[test]
    fn test_controllers_connect_and_disconnect() {
        let mut state = subscribed();
        let toasts = run(
            &mut state,
            &[
                IpcEvent::Controllers(vec![controller(0, Some(80)), controller(1, None)]),
                IpcEvent::Controllers(vec![controller(0, Some(79)), controller(1, None)]),
                IpcEvent::Controllers(vec![controller(1, None)]),
            ],
        );
        assert_eq!(
            toasts,
            vec![
                Toast::new("Controller connected", None),
                Toast::new("Controller disconnected", None),
            ]
        );
    }

    #[test]
    fn test_battery_hysteresis() {
        let mut state = subscribed();
        let toasts = run(
            &mut state,
            &[
                IpcEvent::Controllers(vec![controller(0, Some(15))]),
                IpcEvent::Controllers(vec![controller(0, Some(12))]),
                IpcEvent::Controllers(vec![controller(0, Some(11))]),
                // back up, but not above 20: still warned
                IpcEvent::Controllers(vec![controller(0, Some(20))]),
                IpcEvent::Controllers(vec![controller(0, Some(10))]),
                // charged: warns again on the next drop
                IpcEvent::Controllers(vec![controller(0, Some(21))]),
                IpcEvent::Controllers(vec![controller(0, Some(9))]),
            ],
        );
        assert_eq!(
            toasts,
            vec![
                Toast::new("Battery low", Some("12 %")),
                Toast::new("Battery low", Some("9 %")),
            ]
        );
    }

    #[test]
    fn test_initial_low_battery_is_silent() {
        let mut state = State::default();
        let toasts = run(
            &mut state,
            &[
                IpcEvent::Controllers(vec![controller(0, Some(10))]),
                IpcEvent::Controllers(vec![controller(0, Some(9))]),
            ],
        );
        assert!(toasts.is_empty());
    }
}
