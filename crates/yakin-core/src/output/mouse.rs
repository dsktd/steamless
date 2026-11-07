//! Virtual mouse for cursor, scroll and mouse button bindings, one per daemon.

use anyhow::anyhow;
use evdev::AttributeSet;
use evdev::BusType;
use evdev::EventType;
use evdev::InputEvent;
use evdev::InputId;
use evdev::KeyCode;
use evdev::RelativeAxisCode;
use evdev::uinput::VirtualDevice;

use crate::Result;
use crate::output::MouseButton;
use crate::output::Output;
use crate::output::UINPUT;

const NAME: &str = "yakin mouse";
const VENDOR: u16 = 0x28de;
const PRODUCT: u16 = 0x0002;
const VERSION: u16 = 0x0001;

/// Hi-res wheel units per notch of a classic wheel.
pub const NOTCH: i32 = 120;

pub struct Mouse {
    device: VirtualDevice,
    /// Hi-res horizontal and vertical scroll not yet sent as a whole notch.
    wheel: (i32, i32),
}
impl Mouse {
    pub fn new() -> Result<Self> {
        crate::output::check_uinput()?;
        let keys =
            AttributeSet::from_iter([KeyCode::BTN_LEFT, KeyCode::BTN_RIGHT, KeyCode::BTN_MIDDLE]);
        let axes = AttributeSet::from_iter([
            RelativeAxisCode::REL_X,
            RelativeAxisCode::REL_Y,
            RelativeAxisCode::REL_WHEEL,
            RelativeAxisCode::REL_HWHEEL,
            RelativeAxisCode::REL_WHEEL_HI_RES,
            RelativeAxisCode::REL_HWHEEL_HI_RES,
        ]);
        let builder = match VirtualDevice::builder() {
            Ok(builder) => builder,
            Err(err) => return Err(anyhow!("Cannot open {UINPUT}: {err}")),
        };
        let device = builder
            .name(NAME)
            .input_id(InputId::new(BusType::BUS_VIRTUAL, VENDOR, PRODUCT, VERSION))
            .with_keys(&keys)?
            .with_relative_axes(&axes)?
            .build()?;
        Ok(Self {
            device,
            wheel: (0, 0),
        })
    }

    /// Emit the mouse outputs as one report; pad and keyboard outputs are skipped.
    pub fn send(
        &mut self,
        outputs: &[Output],
    ) -> Result<()> {
        let mut events = Vec::new();
        for output in outputs {
            match *output {
                Output::MouseMove(dx, dy) => {
                    events.push(rel(RelativeAxisCode::REL_X, dx));
                    events.push(rel(RelativeAxisCode::REL_Y, dy));
                }
                Output::Scroll(dx, dy) => {
                    // Hi-res for smooth clients, whole notches for the rest, like a real wheel
                    if dx != 0 {
                        events.push(rel(RelativeAxisCode::REL_HWHEEL_HI_RES, dx));
                        let notches = notches(&mut self.wheel.0, dx);
                        if notches != 0 {
                            events.push(rel(RelativeAxisCode::REL_HWHEEL, notches));
                        }
                    }
                    if dy != 0 {
                        events.push(rel(RelativeAxisCode::REL_WHEEL_HI_RES, dy));
                        let notches = notches(&mut self.wheel.1, dy);
                        if notches != 0 {
                            events.push(rel(RelativeAxisCode::REL_WHEEL, notches));
                        }
                    }
                }
                Output::MouseButton(button, pressed) => {
                    let code = match button {
                        MouseButton::Left => KeyCode::BTN_LEFT,
                        MouseButton::Right => KeyCode::BTN_RIGHT,
                        MouseButton::Middle => KeyCode::BTN_MIDDLE,
                    };
                    events.push(InputEvent::new(EventType::KEY.0, code.0, pressed as i32));
                }
                Output::Button(..)
                | Output::Stick(..)
                | Output::Trigger(..)
                | Output::Key(..)
                | Output::Desktop(_)
                | Output::Profile(_)
                | Output::Layer(..)
                | Output::Menu(_)
                | Output::Keyboard(_)
                | Output::Lizard(..)
                | Output::TvOff
                | Output::Haptic { .. }
                | Output::Tone { .. } => {}
            }
        }
        if events.is_empty() {
            return Ok(());
        }
        self.device.emit(&events)?;
        Ok(())
    }
}

/// Whole notches in `rest` plus `value`, keeping the remainder.
fn notches(
    rest: &mut i32,
    value: i32,
) -> i32 {
    *rest += value;
    let notches = *rest / NOTCH;
    *rest -= notches * NOTCH;
    notches
}

fn rel(
    code: RelativeAxisCode,
    value: i32,
) -> InputEvent {
    InputEvent::new(EventType::RELATIVE.0, code.0, value)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_notches() {
        let mut rest = 0;
        assert_eq!(notches(&mut rest, 60), 0);
        assert_eq!(notches(&mut rest, 70), 1);
        assert_eq!(rest, 10);
        assert_eq!(notches(&mut rest, -250), -2);
        assert_eq!(rest, 0);
    }
}
