//! Snapshot of one controller, decoded from an input report.

use anyhow::anyhow;
use bincode::Decode;
use bincode::Encode;
use serde::Serialize;

/// A half of the controller: a trackpad, a haptic actuator, the buttons under one hand.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub enum Side {
    Left,
    Right,
}

/// One bit of the Ibex button field. The discriminant is the bit index in bytes 2-5 read as a
/// little-endian u32, so `byte.bit` is `(byte - 2) * 8 + bit`. Past those, the stick directions
/// are virtual: the profile presses them from a deflected stick.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum Button {
    A = 0,
    B = 1,
    X = 2,
    Y = 3,
    Quick = 4,
    RStickClick = 5,
    Menu = 6,
    GripR1 = 7,
    GripR2 = 8,
    Rb = 9,
    DpadDown = 10,
    DpadRight = 11,
    DpadLeft = 12,
    DpadUp = 13,
    View = 14,
    LStickClick = 15,
    Steam = 16,
    GripL1 = 17,
    GripL2 = 18,
    Lb = 19,
    RStickTouch = 20,
    RPadTouch = 21,
    RPadClick = 22,
    RtFull = 23,
    LStickTouch = 24,
    LPadTouch = 25,
    LPadClick = 26,
    LtFull = 27,
    GripRTouch = 28,
    GripLTouch = 29,
    LStickUp = 30,
    LStickDown = 31,
    LStickLeft = 32,
    LStickRight = 33,
    RStickUp = 34,
    RStickDown = 35,
    RStickLeft = 36,
    RStickRight = 37,
}
impl Button {
    pub const ALL: [Button; 38] = [
        Button::A,
        Button::B,
        Button::X,
        Button::Y,
        Button::Quick,
        Button::RStickClick,
        Button::Menu,
        Button::GripR1,
        Button::GripR2,
        Button::Rb,
        Button::DpadDown,
        Button::DpadRight,
        Button::DpadLeft,
        Button::DpadUp,
        Button::View,
        Button::LStickClick,
        Button::Steam,
        Button::GripL1,
        Button::GripL2,
        Button::Lb,
        Button::RStickTouch,
        Button::RPadTouch,
        Button::RPadClick,
        Button::RtFull,
        Button::LStickTouch,
        Button::LPadTouch,
        Button::LPadClick,
        Button::LtFull,
        Button::GripRTouch,
        Button::GripLTouch,
        Button::LStickUp,
        Button::LStickDown,
        Button::LStickLeft,
        Button::LStickRight,
        Button::RStickUp,
        Button::RStickDown,
        Button::RStickLeft,
        Button::RStickRight,
    ];

    /// The virtual buttons of the left and right stick: up, down, left, right.
    pub const STICKS: [[Button; 4]; 2] = [
        [
            Button::LStickUp,
            Button::LStickDown,
            Button::LStickLeft,
            Button::LStickRight,
        ],
        [
            Button::RStickUp,
            Button::RStickDown,
            Button::RStickLeft,
            Button::RStickRight,
        ],
    ];

    pub const fn mask(self) -> u64 {
        1 << self as u8
    }

    /// Touch sensors, as opposed to things that click.
    pub const fn is_touch(self) -> bool {
        matches!(
            self,
            Button::RStickTouch
                | Button::RPadTouch
                | Button::LStickTouch
                | Button::LPadTouch
                | Button::GripRTouch
                | Button::GripLTouch
        )
    }

    /// The hand that presses it; Steam, in the middle, counts as left.
    pub const fn side(self) -> Side {
        match self {
            Button::A
            | Button::B
            | Button::X
            | Button::Y
            | Button::Quick
            | Button::RStickClick
            | Button::Menu
            | Button::GripR1
            | Button::GripR2
            | Button::Rb
            | Button::RStickTouch
            | Button::RPadTouch
            | Button::RPadClick
            | Button::RtFull
            | Button::GripRTouch
            | Button::RStickUp
            | Button::RStickDown
            | Button::RStickLeft
            | Button::RStickRight => Side::Right,
            Button::DpadDown
            | Button::DpadRight
            | Button::DpadLeft
            | Button::DpadUp
            | Button::View
            | Button::LStickClick
            | Button::Steam
            | Button::GripL1
            | Button::GripL2
            | Button::Lb
            | Button::LStickTouch
            | Button::LPadTouch
            | Button::LPadClick
            | Button::LtFull
            | Button::GripLTouch
            | Button::LStickUp
            | Button::LStickDown
            | Button::LStickLeft
            | Button::LStickRight => Side::Left,
        }
    }

    /// Name used in profiles and CLI output.
    pub const fn name(self) -> &'static str {
        match self {
            Button::A => "a",
            Button::B => "b",
            Button::X => "x",
            Button::Y => "y",
            Button::Quick => "quick",
            Button::RStickClick => "rstick_click",
            Button::Menu => "menu",
            Button::GripR1 => "grip_r1",
            Button::GripR2 => "grip_r2",
            Button::Rb => "rb",
            Button::DpadDown => "dpad_down",
            Button::DpadRight => "dpad_right",
            Button::DpadLeft => "dpad_left",
            Button::DpadUp => "dpad_up",
            Button::View => "view",
            Button::LStickClick => "lstick_click",
            Button::Steam => "steam",
            Button::GripL1 => "grip_l1",
            Button::GripL2 => "grip_l2",
            Button::Lb => "lb",
            Button::RStickTouch => "rstick_touch",
            Button::RPadTouch => "rpad_touch",
            Button::RPadClick => "rpad_click",
            Button::RtFull => "rt_full",
            Button::LStickTouch => "lstick_touch",
            Button::LPadTouch => "lpad_touch",
            Button::LPadClick => "lpad_click",
            Button::LtFull => "lt_full",
            Button::GripRTouch => "grip_r_touch",
            Button::GripLTouch => "grip_l_touch",
            Button::LStickUp => "lstick_up",
            Button::LStickDown => "lstick_down",
            Button::LStickLeft => "lstick_left",
            Button::LStickRight => "lstick_right",
            Button::RStickUp => "rstick_up",
            Button::RStickDown => "rstick_down",
            Button::RStickLeft => "rstick_left",
            Button::RStickRight => "rstick_right",
        }
    }
}
impl std::fmt::Display for Button {
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}
impl std::str::FromStr for Button {
    type Err = anyhow::Error;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Button::ALL
            .into_iter()
            .find(|button| button.name() == name)
            .ok_or(anyhow!("Unknown button {name}"))
    }
}

/// Bytes 2-5 of an input report, and the virtual stick directions above them.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Hash, Encode, Decode, Serialize)]
pub struct Buttons(u64);
impl Buttons {
    const TOUCH: u64 = Button::RStickTouch.mask()
        | Button::RPadTouch.mask()
        | Button::LStickTouch.mask()
        | Button::LPadTouch.mask()
        | Button::GripRTouch.mask()
        | Button::GripLTouch.mask();

    /// The hardware field; bits past the known buttons are dropped.
    pub const fn new(bits: u32) -> Self {
        Self(bits as u64 & (Button::LStickUp.mask() - 1))
    }

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn bits(self) -> u64 {
        self.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn contains(
        self,
        button: Button,
    ) -> bool {
        self.0 & button.mask() != 0
    }

    pub const fn with(
        self,
        button: Button,
    ) -> Self {
        Self(self.0 | button.mask())
    }

    pub const fn without(
        self,
        button: Button,
    ) -> Self {
        Self(self.0 & !button.mask())
    }

    /// Only the bits that click, without the touch sensors.
    pub const fn pressed(self) -> Self {
        Self(self.0 & !Self::TOUCH)
    }

    pub const fn a(self) -> bool {
        self.contains(Button::A)
    }

    pub const fn b(self) -> bool {
        self.contains(Button::B)
    }

    pub const fn x(self) -> bool {
        self.contains(Button::X)
    }

    pub const fn y(self) -> bool {
        self.contains(Button::Y)
    }

    pub const fn quick(self) -> bool {
        self.contains(Button::Quick)
    }

    pub const fn rstick_click(self) -> bool {
        self.contains(Button::RStickClick)
    }

    pub const fn menu(self) -> bool {
        self.contains(Button::Menu)
    }

    pub const fn grip_r1(self) -> bool {
        self.contains(Button::GripR1)
    }

    pub const fn grip_r2(self) -> bool {
        self.contains(Button::GripR2)
    }

    pub const fn rb(self) -> bool {
        self.contains(Button::Rb)
    }

    pub const fn dpad_down(self) -> bool {
        self.contains(Button::DpadDown)
    }

    pub const fn dpad_right(self) -> bool {
        self.contains(Button::DpadRight)
    }

    pub const fn dpad_left(self) -> bool {
        self.contains(Button::DpadLeft)
    }

    pub const fn dpad_up(self) -> bool {
        self.contains(Button::DpadUp)
    }

    pub const fn view(self) -> bool {
        self.contains(Button::View)
    }

    pub const fn lstick_click(self) -> bool {
        self.contains(Button::LStickClick)
    }

    pub const fn steam(self) -> bool {
        self.contains(Button::Steam)
    }

    pub const fn grip_l1(self) -> bool {
        self.contains(Button::GripL1)
    }

    pub const fn grip_l2(self) -> bool {
        self.contains(Button::GripL2)
    }

    pub const fn lb(self) -> bool {
        self.contains(Button::Lb)
    }

    pub const fn rstick_touch(self) -> bool {
        self.contains(Button::RStickTouch)
    }

    pub const fn rpad_touch(self) -> bool {
        self.contains(Button::RPadTouch)
    }

    pub const fn rpad_click(self) -> bool {
        self.contains(Button::RPadClick)
    }

    pub const fn rt_full(self) -> bool {
        self.contains(Button::RtFull)
    }

    pub const fn lstick_touch(self) -> bool {
        self.contains(Button::LStickTouch)
    }

    pub const fn lpad_touch(self) -> bool {
        self.contains(Button::LPadTouch)
    }

    pub const fn lpad_click(self) -> bool {
        self.contains(Button::LPadClick)
    }

    pub const fn lt_full(self) -> bool {
        self.contains(Button::LtFull)
    }

    pub const fn grip_r_touch(self) -> bool {
        self.contains(Button::GripRTouch)
    }

    pub const fn grip_l_touch(self) -> bool {
        self.contains(Button::GripLTouch)
    }
}
impl From<u64> for Buttons {
    fn from(bits: u64) -> Self {
        Self(bits)
    }
}
impl From<Buttons> for u64 {
    fn from(buttons: Buttons) -> Self {
        buttons.0
    }
}
impl From<&Buttons> for u64 {
    fn from(buttons: &Buttons) -> Self {
        buttons.0
    }
}

/// Y is as the controller sends it: up is positive.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Encode, Decode, Serialize)]
pub struct Stick {
    pub x: i16,
    pub y: i16,
}

/// Trackpad position, only meaningful while `touched` is set.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Encode, Decode, Serialize)]
pub struct Trackpad {
    pub x: i16,
    pub y: i16,
    pub pressure: u16,
    pub touched: bool,
    pub pressed: bool,
}

/// Raw IMU values. Accel and gyro are in report order (X, Z, Y); the quaternion is zero on
/// 0x45 reports.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Encode, Decode, Serialize)]
pub struct Imu {
    pub timestamp: u32,
    pub accel: [i16; 3],
    pub gyro: [i16; 3],
    pub quaternion: [i16; 4],
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Encode, Decode, Serialize)]
pub struct ControllerState {
    pub seq: u8,
    pub buttons: Buttons,
    pub lt: u16,
    pub rt: u16,
    pub lstick: Stick,
    pub rstick: Stick,
    pub lpad: Trackpad,
    pub rpad: Trackpad,
    pub imu: Imu,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Charge {
    Discharging,
    Charging,
    Done,
    Unknown(u8),
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Battery {
    pub charge: Charge,
    pub level: u8,
}
