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

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Output {
    Button(PadButton, bool),
    /// Evdev convention: Y down is positive.
    Stick(PadStick, i16, i16),
    Trigger(PadTrigger, u8),
    /// Evdev key code on the virtual keyboard.
    Key(u16, bool),
}
