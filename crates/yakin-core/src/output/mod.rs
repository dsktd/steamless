use std::path::Path;

use anyhow::anyhow;

use crate::Result;
use crate::desktop::Action;
use crate::menu::MenuCommand;
use crate::profile::LayerSwitch;
use crate::profile::LizardSwitch;
use crate::profile::ProfileSwitch;
use crate::state::Side;

mod keys;
mod mouse;
mod pad;
pub use keys::*;
pub use mouse::*;
pub use pad::*;

pub const UINPUT: &str = "/dev/uinput";

/// Fail early, by name, when the uinput module is not loaded.
pub fn check_uinput() -> Result<()> {
    if !Path::new(UINPUT).exists() {
        return Err(anyhow!("{UINPUT} is missing; load the uinput module"));
    }
    Ok(())
}

/// Buttons of the virtual Xbox 360 pad; the D-pad becomes the hat.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum PadButton {
    South,
    East,
    West,
    North,
    Lb,
    Rb,
    Select,
    Start,
    Guide,
    ThumbL,
    ThumbR,
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum PadStick {
    Left,
    Right,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum PadTrigger {
    Lt,
    Rt,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Output {
    Button(PadButton, bool),
    /// Evdev convention: Y down is positive.
    Stick(PadStick, i16, i16),
    Trigger(PadTrigger, u8),
    /// Evdev key code on the virtual keyboard.
    Key(u16, bool),
    /// Pixels, evdev convention: Y down is positive.
    MouseMove(i32, i32),
    /// Hi-res wheel units, 120 per notch: horizontal right and vertical up are positive.
    Scroll(i32, i32),
    MouseButton(MouseButton, bool),
    /// A desktop action, run on press by the selected desktop.
    Desktop(Action),
    /// Switch the active profile; the engine executes it, no device.
    Profile(ProfileSwitch),
    /// A layer binding's press (true) or release; the mapper consumes it, it never leaves it.
    Layer(LayerSwitch, bool),
    /// Open or close the quick menu; the engine executes it, no device.
    Menu(MenuCommand),
    /// Open or close the on-screen keyboard; the engine executes it, no device.
    Keyboard(MenuCommand),
    /// A lizard binding's press (true) or release; the engine's [`crate::lizard::Lizard`] gate
    /// consumes it.
    Lizard(LizardSwitch, bool),
    /// A `tv:off` press; the engine's [`crate::tv::Tv`] gate consumes it.
    TvOff,
    /// A pulse on one trackpad of the controller the input came from, `strength` of 255.
    Haptic {
        side: Side,
        strength: u8,
        ms: u16,
    },
    /// An audible tone of `hz` from one trackpad's actuator of the controller the input came
    /// from.
    Tone {
        side: Side,
        hz: u16,
        ms: u16,
    },
}
impl Output {
    /// A pulse at `share` 0 to 1 of full strength; none at 0.
    pub fn haptic(
        side: Side,
        share: f32,
        ms: u16,
    ) -> Option<Output> {
        if share <= 0.0 || !share.is_finite() {
            return None;
        }
        Some(Output::Haptic {
            side,
            strength: (share.min(1.0) * 255.0).round() as u8,
            ms,
        })
    }
}
