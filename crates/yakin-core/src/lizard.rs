//! The lizard gate: while a `lizard:` binding hands the controller to its own keyboard and
//! mouse, nothing of yakin's goes out, so the two do not double up.

use crate::output::Output;
use crate::profile::LizardSwitch;

/// Whether one controller is in lizard mode by a binding, and the outputs it lets through.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Lizard {
    on: bool,
}
impl Lizard {
    pub fn is_on(&self) -> bool {
        self.on
    }

    /// Apply the lizard outputs of one report and keep the rest only while lizard is off, with
    /// `Some(on)` when this report switched it. A report that switches it either way lets
    /// nothing else out: on the way in the engine releases everything held first, on the way
    /// out nothing was held.
    pub fn filter(
        &mut self,
        outputs: Vec<Output>,
    ) -> (Vec<Output>, Option<bool>) {
        let before = self.on;
        for output in &outputs {
            match output {
                Output::Lizard(LizardSwitch::Hold, pressed) => self.on = *pressed,
                Output::Lizard(LizardSwitch::Toggle, true) => self.on = !self.on,
                _ => {}
            }
        }
        let change = (self.on != before).then_some(self.on);
        let outputs = if self.on || change.is_some() {
            Vec::new()
        } else {
            outputs
                .into_iter()
                .filter(|output| !matches!(output, Output::Lizard(..)))
                .collect()
        };
        (outputs, change)
    }

    /// Back to off without a release, as on disconnect.
    pub fn reset(&mut self) {
        self.on = false;
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::output::PadButton;

    fn key() -> Output {
        Output::Key(30, true)
    }

    #[test]
    fn test_off_passes_through() {
        let mut lizard = Lizard::default();
        let (outputs, change) = lizard.filter(vec![key(), Output::MouseMove(1, 2)]);
        assert_eq!(outputs, vec![key(), Output::MouseMove(1, 2)]);
        assert_eq!(change, None);
        assert!(!lizard.is_on());
    }

    #[test]
    fn test_hold() {
        let mut lizard = Lizard::default();
        let (outputs, change) =
            lizard.filter(vec![key(), Output::Lizard(LizardSwitch::Hold, true)]);
        assert!(outputs.is_empty());
        assert_eq!(change, Some(true));
        assert!(lizard.is_on());
        // Everything else is swallowed while held
        let (outputs, change) = lizard.filter(vec![
            Output::Button(PadButton::South, true),
            Output::Desktop(crate::desktop::Action::parse("overview").unwrap()),
        ]);
        assert!(outputs.is_empty());
        assert_eq!(change, None);
        let (outputs, change) = lizard.filter(vec![Output::Lizard(LizardSwitch::Hold, false)]);
        assert!(outputs.is_empty());
        assert_eq!(change, Some(false));
        assert!(!lizard.is_on());
        let (outputs, _) = lizard.filter(vec![key()]);
        assert_eq!(outputs, vec![key()]);
    }

    #[test]
    fn test_toggle() {
        let mut lizard = Lizard::default();
        assert_eq!(
            lizard
                .filter(vec![Output::Lizard(LizardSwitch::Toggle, true)])
                .1,
            Some(true)
        );
        // Its release changes nothing
        assert_eq!(
            lizard
                .filter(vec![Output::Lizard(LizardSwitch::Toggle, false)])
                .1,
            None
        );
        assert!(lizard.is_on());
        assert_eq!(
            lizard
                .filter(vec![Output::Lizard(LizardSwitch::Toggle, true)])
                .1,
            Some(false)
        );
        assert!(!lizard.is_on());
    }

    #[test]
    fn test_reset() {
        let mut lizard = Lizard::default();
        lizard.filter(vec![Output::Lizard(LizardSwitch::Hold, true)]);
        lizard.reset();
        assert!(!lizard.is_on());
    }
}
