//! Virtual Xbox 360 pad, one per controller.

use anyhow::anyhow;
use evdev::AbsInfo;
use evdev::AbsoluteAxisCode;
use evdev::AttributeSet;
use evdev::BusType;
use evdev::EventType;
use evdev::InputEvent;
use evdev::InputId;
use evdev::KeyCode;
use evdev::UinputAbsSetup;
use evdev::uinput::VirtualDevice;

use crate::Result;
use crate::output::Output;
use crate::output::PadButton;
use crate::output::PadStick;
use crate::output::PadTrigger;
use crate::output::UINPUT;

const NAME: &str = "Microsoft X-Box 360 pad";
const VENDOR: u16 = 0x045e;
const PRODUCT: u16 = 0x028e;
const VERSION: u16 = 0x0114;

pub struct Pad {
    device: VirtualDevice,
    /// Up, down, left, right; folded into the hat.
    dpad: [bool; 4],
}
impl Pad {
    /// `phys` tells two pads apart, e.g. the hidraw path.
    pub fn new(phys: &str) -> Result<Self> {
        crate::output::check_uinput()?;
        let keys = AttributeSet::from_iter([
            KeyCode::BTN_SOUTH,
            KeyCode::BTN_EAST,
            KeyCode::BTN_NORTH,
            KeyCode::BTN_WEST,
            KeyCode::BTN_TL,
            KeyCode::BTN_TR,
            KeyCode::BTN_SELECT,
            KeyCode::BTN_START,
            KeyCode::BTN_MODE,
            KeyCode::BTN_THUMBL,
            KeyCode::BTN_THUMBR,
        ]);
        let stick = AbsInfo::new(0, i16::MIN as i32, i16::MAX as i32, 16, 128, 0);
        let trigger = AbsInfo::new(0, 0, 255, 0, 0, 0);
        let hat = AbsInfo::new(0, -1, 1, 0, 0, 0);
        let _ = phys; // TODO uniq/phys: UI_SET_PHYS returns EINVAL here, investigate
        let builder = match VirtualDevice::builder() {
            Ok(builder) => builder,
            Err(err) => return Err(anyhow!("Cannot open {UINPUT}: {err}")),
        };
        let device = builder
            .name(NAME)
            .input_id(InputId::new(BusType::BUS_USB, VENDOR, PRODUCT, VERSION))
            .with_keys(&keys)?
            .with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisCode::ABS_X, stick))?
            .with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisCode::ABS_Y, stick))?
            .with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisCode::ABS_Z, trigger))?
            .with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisCode::ABS_RX, stick))?
            .with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisCode::ABS_RY, stick))?
            .with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisCode::ABS_RZ, trigger))?
            .with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisCode::ABS_HAT0X, hat))?
            .with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisCode::ABS_HAT0Y, hat))?
            .build()?;
        Ok(Self {
            device,
            dpad: [false; 4],
        })
    }

    /// Emit the pad outputs as one report; keyboard outputs are skipped.
    pub fn send(
        &mut self,
        outputs: &[Output],
    ) -> Result<()> {
        let mut events = Vec::new();
        for output in outputs {
            match *output {
                Output::Button(button, pressed) => {
                    let code = match button {
                        PadButton::South => KeyCode::BTN_SOUTH,
                        PadButton::East => KeyCode::BTN_EAST,
                        PadButton::West => KeyCode::BTN_WEST,
                        PadButton::North => KeyCode::BTN_NORTH,
                        PadButton::Lb => KeyCode::BTN_TL,
                        PadButton::Rb => KeyCode::BTN_TR,
                        PadButton::Select => KeyCode::BTN_SELECT,
                        PadButton::Start => KeyCode::BTN_START,
                        PadButton::Guide => KeyCode::BTN_MODE,
                        PadButton::ThumbL => KeyCode::BTN_THUMBL,
                        PadButton::ThumbR => KeyCode::BTN_THUMBR,
                        PadButton::DpadUp
                        | PadButton::DpadDown
                        | PadButton::DpadLeft
                        | PadButton::DpadRight => {
                            self.hat(button, pressed, &mut events);
                            continue;
                        }
                    };
                    events.push(InputEvent::new(EventType::KEY.0, code.0, pressed as i32));
                }
                Output::Stick(stick, x, y) => {
                    let (code_x, code_y) = match stick {
                        PadStick::Left => (AbsoluteAxisCode::ABS_X, AbsoluteAxisCode::ABS_Y),
                        PadStick::Right => (AbsoluteAxisCode::ABS_RX, AbsoluteAxisCode::ABS_RY),
                    };
                    events.push(abs(code_x, x as i32));
                    events.push(abs(code_y, y as i32));
                }
                Output::Trigger(trigger, value) => {
                    let code = match trigger {
                        PadTrigger::Lt => AbsoluteAxisCode::ABS_Z,
                        PadTrigger::Rt => AbsoluteAxisCode::ABS_RZ,
                    };
                    events.push(abs(code, value as i32));
                }
                Output::Key(..) => {}
            }
        }
        if events.is_empty() {
            return Ok(());
        }
        self.device.emit(&events)?;
        Ok(())
    }

    fn hat(
        &mut self,
        button: PadButton,
        pressed: bool,
        events: &mut Vec<InputEvent>,
    ) {
        let index = match button {
            PadButton::DpadUp => 0,
            PadButton::DpadDown => 1,
            PadButton::DpadLeft => 2,
            PadButton::DpadRight => 3,
            _ => unreachable!(),
        };
        self.dpad[index] = pressed;
        let [up, down, left, right] = self.dpad.map(|held| held as i32);
        events.push(abs(AbsoluteAxisCode::ABS_HAT0X, right - left));
        events.push(abs(AbsoluteAxisCode::ABS_HAT0Y, down - up));
    }
}

fn abs(
    code: AbsoluteAxisCode,
    value: i32,
) -> InputEvent {
    InputEvent::new(EventType::ABSOLUTE.0, code.0, value)
}
