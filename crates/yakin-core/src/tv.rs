//! The TV gate: a `tv:off` binding puts the TV to standby and yakin idles, sending nothing, until
//! a button is held for the wake time, which wakes the TV; that press is swallowed down to its
//! release, and short presses do nothing. yakin knows only what it did itself: a TV turned on by
//! its remote idles yakin until that hold.

use std::time::Duration;
use std::time::Instant;

use crate::output::Output;
use crate::state::Button;
use crate::state::ControllerState;

/// What the engine runs on the TV for a report.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Power {
    Standby,
    Wake,
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
enum Gate {
    #[default]
    On,
    /// Idle, with when the current hold began (pressed while idle, so a button still held from
    /// the `tv:off` press does not count).
    Off(Option<Instant>),
    /// Woken, swallowing until every button is up.
    Waking,
}

/// Whether yakin put the TV to standby, and the outputs it lets through.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Tv {
    gate: Gate,
}
impl Tv {
    /// Whether the gate swallows every report.
    pub fn is_off(&self) -> bool {
        self.gate != Gate::On
    }

    /// The TV went to standby by other means (its remote): idle as after a `tv:off` press.
    pub fn standby(&mut self) {
        if self.gate == Gate::On {
            self.gate = Gate::Off(None);
        }
    }

    /// The outputs of one report from `prev` to `state` at `now`, with `Some` when it switched
    /// the TV. While off only a button (not a touch) held for `wake` gets through, as a wake.
    pub fn filter(
        &mut self,
        prev: &ControllerState,
        state: &ControllerState,
        outputs: Vec<Output>,
        now: Instant,
        wake: Duration,
    ) -> (Vec<Output>, Option<Power>) {
        let held = |state: &ControllerState, button: Button| {
            !button.is_touch() && state.buttons.contains(button)
        };
        let any = Button::ALL.into_iter().any(|button| held(state, button));
        match self.gate {
            // The rest of the report still goes out: a release in it would be lost otherwise
            Gate::On if outputs.contains(&Output::TvOff) => {
                self.gate = Gate::Off(None);
                let outputs = outputs
                    .into_iter()
                    .filter(|output| *output != Output::TvOff)
                    .collect();
                (outputs, Some(Power::Standby))
            }
            Gate::On => (outputs, None),
            Gate::Waking => {
                if !any {
                    self.gate = Gate::On;
                }
                (Vec::new(), None)
            }
            Gate::Off(since) => {
                let pressed = Button::ALL
                    .into_iter()
                    .any(|button| !held(prev, button) && held(state, button));
                let since = match since {
                    _ if !any => None,
                    None if pressed => Some(now),
                    since => since,
                };
                if since.is_some_and(|since| now >= since + wake) {
                    self.gate = Gate::Waking;
                    return (Vec::new(), Some(Power::Wake));
                }
                self.gate = Gate::Off(since);
                (Vec::new(), None)
            }
        }
    }

    /// A controller connected. A connect is no press: a Bluetooth controller reconnects by
    /// itself after a power-down, so while idle it stays idle (only the wake hold wakes the TV),
    /// with any hold restarted. `true` when on, so the TV is to be made active source.
    pub fn connect(&mut self) -> bool {
        if let Gate::Off(_) = self.gate {
            self.gate = Gate::Off(None);
        }
        self.gate == Gate::On
    }
}

#[cfg(test)]
mod test {
    use super::*;

    const WAKE: Duration = Duration::from_millis(1000);

    #[test]
    fn test_standby() {
        let mut tv = Tv::default();
        assert!(!tv.is_off());
        tv.standby();
        assert!(tv.is_off());
        // A held button wakes it as after tv:off
        let idle = ControllerState::default();
        let t0 = Instant::now();
        let (_, power) = tv.filter(&idle, &pressed(&[Button::A]), Vec::new(), t0, WAKE);
        assert_eq!(power, None);
        let (_, power) = tv.filter(
            &pressed(&[Button::A]),
            &pressed(&[Button::A]),
            Vec::new(),
            t0 + WAKE,
            WAKE,
        );
        assert_eq!(power, Some(Power::Wake));
        // Standby while already off or waking changes nothing
        tv.standby();
        assert_eq!(tv.gate, Gate::Waking);
    }

    fn pressed(buttons: &[Button]) -> ControllerState {
        let mut state = ControllerState::default();
        for button in buttons {
            state.buttons = state.buttons.with(*button);
        }
        state
    }

    fn key() -> Output {
        Output::Key(30, true)
    }

    /// A gate put to standby at the returned instant.
    fn off() -> (Tv, Instant) {
        let mut tv = Tv::default();
        let t0 = Instant::now();
        let idle = ControllerState::default();
        assert_eq!(
            tv.filter(&idle, &idle, vec![Output::TvOff], t0, WAKE),
            (vec![], Some(Power::Standby))
        );
        (tv, t0)
    }

    #[test]
    fn test_standby_keeps_the_rest_of_its_report() {
        let mut tv = Tv::default();
        let idle = ControllerState::default();
        let release = Output::Key(30, false);
        assert_eq!(
            tv.filter(
                &idle,
                &idle,
                vec![release.clone(), Output::TvOff],
                Instant::now(),
                WAKE
            ),
            (vec![release], Some(Power::Standby))
        );
    }

    fn at(
        t0: Instant,
        ms: u64,
    ) -> Instant {
        t0 + Duration::from_millis(ms)
    }

    #[test]
    fn test_on_passes_through() {
        let mut tv = Tv::default();
        let idle = ControllerState::default();
        assert_eq!(
            tv.filter(&idle, &idle, vec![key()], Instant::now(), WAKE),
            (vec![key()], None)
        );
    }

    #[test]
    fn test_short_tap_stays_idle() {
        let (mut tv, t0) = off();
        let idle = ControllerState::default();
        let grip = pressed(&[Button::GripRTouch]);
        let a = pressed(&[Button::A]);
        // Touches never count, however long
        assert_eq!(
            tv.filter(&idle, &grip, vec![key()], at(t0, 0), WAKE),
            (vec![], None)
        );
        assert_eq!(
            tv.filter(&grip, &grip, vec![key()], at(t0, 2000), WAKE),
            (vec![], None)
        );
        assert_eq!(
            tv.filter(&grip, &a, vec![key()], at(t0, 2100), WAKE),
            (vec![], None)
        );
        assert_eq!(
            tv.filter(&a, &a, vec![key()], at(t0, 2500), WAKE),
            (vec![], None)
        );
        assert_eq!(
            tv.filter(&a, &idle, vec![key()], at(t0, 2600), WAKE),
            (vec![], None)
        );
        // The released tap leaves no timer behind
        assert_eq!(
            tv.filter(&idle, &idle, vec![], at(t0, 5000), WAKE),
            (vec![], None)
        );
        assert!(tv.is_off());
    }

    #[test]
    fn test_hold_wakes_once_and_release_is_swallowed() {
        let (mut tv, t0) = off();
        let idle = ControllerState::default();
        let a = pressed(&[Button::A]);
        assert_eq!(
            tv.filter(&idle, &a, vec![key()], at(t0, 100), WAKE),
            (vec![], None)
        );
        assert_eq!(
            tv.filter(&a, &a, vec![key()], at(t0, 1099), WAKE),
            (vec![], None)
        );
        assert_eq!(
            tv.filter(&a, &a, vec![key()], at(t0, 1100), WAKE),
            (vec![], Some(Power::Wake))
        );
        assert_eq!(
            tv.filter(&a, &a, vec![key()], at(t0, 3000), WAKE),
            (vec![], None)
        );
        assert_eq!(
            tv.filter(&a, &idle, vec![key()], at(t0, 3100), WAKE),
            (vec![], None)
        );
        assert!(!tv.is_off());
        assert_eq!(
            tv.filter(&idle, &a, vec![key()], at(t0, 3200), WAKE),
            (vec![key()], None)
        );
    }

    #[test]
    fn test_second_button_keeps_the_hold() {
        let (mut tv, t0) = off();
        let idle = ControllerState::default();
        let a = pressed(&[Button::A]);
        let ab = pressed(&[Button::A, Button::B]);
        let b = pressed(&[Button::B]);
        assert_eq!(
            tv.filter(&idle, &a, vec![], at(t0, 0), WAKE),
            (vec![], None)
        );
        // B joining and A leaving keep timing from A's press
        assert_eq!(
            tv.filter(&a, &ab, vec![], at(t0, 500), WAKE),
            (vec![], None)
        );
        assert_eq!(
            tv.filter(&ab, &b, vec![], at(t0, 800), WAKE),
            (vec![], None)
        );
        assert_eq!(
            tv.filter(&b, &b, vec![], at(t0, 1000), WAKE),
            (vec![], Some(Power::Wake))
        );
        // Swallowed until both are up, even with A pressed again
        assert_eq!(
            tv.filter(&b, &ab, vec![key()], at(t0, 1200), WAKE),
            (vec![], None)
        );
        assert_eq!(
            tv.filter(&ab, &a, vec![key()], at(t0, 1300), WAKE),
            (vec![], None)
        );
        assert_eq!(
            tv.filter(&a, &idle, vec![key()], at(t0, 1400), WAKE),
            (vec![], None)
        );
        assert!(!tv.is_off());
    }

    #[test]
    fn test_held_through_standby_does_not_wake() {
        let mut tv = Tv::default();
        let t0 = Instant::now();
        let menu = pressed(&[Button::Menu]);
        tv.filter(&menu, &menu, vec![Output::TvOff], t0, WAKE);
        assert_eq!(
            tv.filter(&menu, &menu, vec![], at(t0, 5000), WAKE),
            (vec![], None)
        );
        assert!(tv.is_off());
    }

    #[test]
    fn test_connect_while_idle_stays_idle() {
        let (mut tv, t0) = off();
        let idle = ControllerState::default();
        let steam = pressed(&[Button::Steam]);
        assert!(!tv.connect());
        assert!(tv.is_off());
        // A short press after the reconnect does nothing; only the wake hold wakes
        assert_eq!(
            tv.filter(&idle, &steam, vec![key()], at(t0, 100), WAKE),
            (vec![], None)
        );
        assert_eq!(
            tv.filter(&steam, &idle, vec![key()], at(t0, 200), WAKE),
            (vec![], None)
        );
        assert!(tv.is_off());
        assert_eq!(
            tv.filter(&idle, &steam, vec![key()], at(t0, 300), WAKE),
            (vec![], None)
        );
        assert_eq!(
            tv.filter(&steam, &steam, vec![key()], at(t0, 1300), WAKE).1,
            Some(Power::Wake)
        );
        // Connecting while on asks for active source
        let mut tv = Tv::default();
        assert!(tv.connect());
        assert!(!tv.is_off());
    }
}
