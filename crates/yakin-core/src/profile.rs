//! TOML config, profiles and the mapper from controller state to outputs.

use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;
use std::time::Instant;

use anyhow::anyhow;
use bincode::Decode;
use bincode::Encode;
use serde::Deserialize;
use serde::Serialize;

use crate::Result;
use crate::auto::Auto;
use crate::auto::AutoToml;
use crate::desktop::Action;
use crate::desktop::Backend;
use crate::desktop::NAMES;
use crate::keyboard;
use crate::menu::Direction;
use crate::menu::Menu;
use crate::menu::MenuCommand;
use crate::menu::MenuToml;
use crate::menu::stick_direction;
use crate::output::MouseButton;
use crate::output::NOTCH;
use crate::output::Output;
use crate::output::PadButton;
use crate::output::PadStick;
use crate::output::PadTrigger;
use crate::state::Button;
use crate::state::Buttons;
use crate::state::ControllerState;
use crate::state::Imu;
use crate::state::Side;
use crate::state::Stick;
use crate::state::Trackpad;

/// Analog sources of the Ibex report.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub enum Axis {
    LStick,
    RStick,
    LPad,
    RPad,
    Lt,
    Rt,
    /// The IMU's angular rate, as a mouse.
    Gyro,
}
impl Axis {
    pub const fn is_trigger(self) -> bool {
        matches!(self, Axis::Lt | Axis::Rt)
    }

    /// In report order.
    pub const ALL: [Axis; 7] = [
        Axis::LStick,
        Axis::RStick,
        Axis::LPad,
        Axis::RPad,
        Axis::Lt,
        Axis::Rt,
        Axis::Gyro,
    ];

    /// The profile's name for it, as [`Button::name`].
    pub const fn name(self) -> &'static str {
        match self {
            Axis::LStick => "lstick",
            Axis::RStick => "rstick",
            Axis::LPad => "lpad",
            Axis::RPad => "rpad",
            Axis::Lt => "lt",
            Axis::Rt => "rt",
            Axis::Gyro => "gyro",
        }
    }
}

/// Anything on the controller a profile can bind, by its profile name.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub enum Source {
    Button(Button),
    Axis(Axis),
}
impl FromStr for Source {
    type Err = anyhow::Error;

    fn from_str(name: &str) -> Result<Self> {
        match Axis::ALL.into_iter().find(|axis| axis.name() == name) {
            Some(axis) => Ok(Source::Axis(axis)),
            None => Ok(Source::Button(name.parse()?)),
        }
    }
}

/// Which profile a [`Target::Profile`] binding switches to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProfileSwitch {
    Named(String),
    /// The next profile by name, wrapping around.
    Next,
    Prev,
}
impl ProfileSwitch {
    /// The profile this switch picks from `active`, out of the sorted `profiles`.
    pub fn pick<'a>(
        &'a self,
        active: &str,
        profiles: &'a [String],
    ) -> Option<&'a str> {
        let step = match self {
            ProfileSwitch::Named(name) => return Some(name),
            ProfileSwitch::Next => 1,
            ProfileSwitch::Prev => profiles.len().checked_sub(1)?,
        };
        if profiles.is_empty() {
            return None;
        }
        let index = profiles.iter().position(|name| name == active).unwrap_or(0);
        profiles
            .get((index + step) % profiles.len())
            .map(String::as_str)
    }
}
impl std::fmt::Display for ProfileSwitch {
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        match self {
            ProfileSwitch::Named(name) => write!(f, "{name}"),
            ProfileSwitch::Next => write!(f, "next"),
            ProfileSwitch::Prev => write!(f, "prev"),
        }
    }
}

/// What a [`Target::Layer`] binding does to its layer of the profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LayerSwitch {
    /// Active while the binding is held.
    Hold(String),
    /// Flipped on each press.
    Toggle(String),
}
impl LayerSwitch {
    pub fn name(&self) -> &str {
        match self {
            LayerSwitch::Hold(name) | LayerSwitch::Toggle(name) => name,
        }
    }
}
impl std::fmt::Display for LayerSwitch {
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        match self {
            LayerSwitch::Hold(name) => write!(f, "hold:{name}"),
            LayerSwitch::Toggle(name) => write!(f, "toggle:{name}"),
        }
    }
}

/// What a [`Target::Lizard`] binding does to the controller's own keyboard and mouse.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum LizardSwitch {
    /// On while the binding is held.
    Hold,
    /// Flipped on each press.
    Toggle,
}
impl std::fmt::Display for LizardSwitch {
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        match self {
            LizardSwitch::Hold => write!(f, "hold"),
            LizardSwitch::Toggle => write!(f, "toggle"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Target {
    Button(PadButton),
    Stick(PadStick),
    Trigger(PadTrigger),
    /// Evdev key code on the virtual keyboard.
    Key(u16),
    /// Keys pressed in order and released in reverse: `key:LEFTMETA+D`.
    Combo(Vec<u16>),
    /// A desktop action run on press: `desktop:overview`, or `niri:<action>` raw.
    Desktop(Action),
    /// A stick flicked up, down, left or right holds one of these keys.
    Keys([u16; 4]),
    /// A stick flicked up, down, left or right holds one of these button-like targets.
    Flicks(Box<[Target; 4]>),
    Mouse(MouseButton),
    /// A trackpad moves the mouse cursor by its finger's motion, a stick by its deflection.
    Cursor,
    /// A trackpad scrolls like a touchpad, a stick like a held wheel.
    Scroll,
    /// A stick turns the cursor horizontally by its angle: flick stick.
    Flick,
    /// Switches the active profile when the button is released.
    Profile(ProfileSwitch),
    /// Lays one of the profile's layers over it, see [`LayerSwitch`].
    Layer(LayerSwitch),
    /// Opens or closes the quick menu on press.
    Menu(MenuCommand),
    /// Opens or closes the on-screen keyboard on press.
    Keyboard(MenuCommand),
    /// Hands the controller to its own keyboard and mouse (lizard mode), see [`LizardSwitch`].
    Lizard(LizardSwitch),
    /// Puts the TV to standby on press with `[daemon] tv_off`; yakin idles until a button wakes
    /// it, see [`crate::tv::Tv`].
    TvOff,
    None,
}
impl Target {
    /// Fired by a press and a release, so a button can drive it.
    pub fn is_button_like(&self) -> bool {
        allows(Source::Button(Button::A), self) && !matches!(self, Target::None)
    }
}
impl FromStr for Target {
    type Err = anyhow::Error;

    fn from_str(target: &str) -> Result<Self> {
        match target {
            "none" => return Ok(Target::None),
            "cursor" => return Ok(Target::Cursor),
            "scroll" => return Ok(Target::Scroll),
            "flick" => return Ok(Target::Flick),
            "tv:off" => return Ok(Target::TvOff),
            _ => {}
        }
        if let Some(name) = target.strip_prefix("mouse:") {
            let Some((_, button)) = MOUSE_TARGETS.iter().find(|(known, _)| *known == name) else {
                return Err(anyhow!("Unknown mouse button {name}"));
            };
            return Ok(Target::Mouse(*button));
        }
        if let Some(name) = target.strip_prefix("profile:") {
            let switch = match name {
                "" => return Err(anyhow!("{target} names no profile")),
                "next" => ProfileSwitch::Next,
                "prev" => ProfileSwitch::Prev,
                name => ProfileSwitch::Named(name.to_string()),
            };
            return Ok(Target::Profile(switch));
        }
        if let Some(rest) = target.strip_prefix("layer:") {
            let switch = match rest.split_once(':') {
                Some(("hold", name)) if !name.is_empty() => LayerSwitch::Hold(name.to_string()),
                Some(("toggle", name)) if !name.is_empty() => LayerSwitch::Toggle(name.to_string()),
                _ => {
                    return Err(anyhow!(
                        "{target} is not layer:hold:<name> or layer:toggle:<name>"
                    ));
                }
            };
            return Ok(Target::Layer(switch));
        }
        if let Some(name) = target.strip_prefix("menu:") {
            return Ok(Target::Menu(MenuCommand::parse(name)?));
        }
        if let Some(name) = target.strip_prefix("lizard:") {
            let switch = match name {
                "hold" => LizardSwitch::Hold,
                "toggle" => LizardSwitch::Toggle,
                _ => return Err(anyhow!("{target} is not lizard:hold or lizard:toggle")),
            };
            return Ok(Target::Lizard(switch));
        }
        if let Some(name) = target.strip_prefix("keyboard:") {
            let command =
                MenuCommand::parse(name).map_err(|_| anyhow!("Unknown keyboard command {name}"))?;
            return Ok(Target::Keyboard(command));
        }
        if let Some(name) = target.strip_prefix("key:") {
            let codes: Vec<u16> = name.split('+').map(key).collect::<Result<_>>()?;
            return match codes[..] {
                [code] => Ok(Target::Key(code)),
                _ => Ok(Target::Combo(codes)),
            };
        }
        if let Some(name) = target.strip_prefix("desktop:") {
            return Ok(Target::Desktop(Action::parse(name)?));
        }
        // Niri's own actions, for what no common one covers
        if let Some(name) = target.strip_prefix("niri:") {
            return Ok(Target::Desktop(Action::raw(name)?));
        }
        if let Some(slots) = target.strip_prefix("flicks:") {
            // A bare slot is a key name, so `flicks:` reads like `keys:`
            let targets: Vec<Target> = slots
                .split(',')
                .map(|slot| match slot.contains(':') || slot == "none" {
                    true => slot.parse(),
                    false => Ok(Target::Key(key(slot)?)),
                })
                .collect::<Result<_>>()?;
            let Ok(targets) = <[Target; 4]>::try_from(targets) else {
                return Err(anyhow!(
                    "{target} needs four targets: up, down, left, right"
                ));
            };
            return Ok(Target::Flicks(Box::new(targets)));
        }
        if let Some(names) = target.strip_prefix("keys:") {
            let keys: Vec<u16> = names.split(',').map(key).collect::<Result<_>>()?;
            let Ok(keys) = keys.try_into() else {
                return Err(anyhow!("{target} needs four keys: up, down, left, right"));
            };
            return Ok(Target::Keys(keys));
        }
        let Some(name) = target.strip_prefix("pad:") else {
            return Err(anyhow!("Unknown target {target}"));
        };
        match PAD_TARGETS.iter().find(|(known, _)| *known == name) {
            Some((_, target)) => Ok(target.clone()),
            None => Err(anyhow!("Unknown pad target {name}")),
        }
    }
}
impl std::fmt::Display for Target {
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        match self {
            Target::Button(_) | Target::Stick(_) | Target::Trigger(_) => {
                let (name, _) = PAD_TARGETS.iter().find(|(_, known)| known == self).unwrap();
                write!(f, "pad:{name}")
            }
            Target::Key(code) => write!(f, "key:{}", key_name(*code)),
            Target::Keys(codes) => {
                let names: Vec<String> = codes.iter().map(|code| key_name(*code)).collect();
                write!(f, "keys:{}", names.join(","))
            }
            Target::Combo(codes) => {
                let names: Vec<String> = codes.iter().map(|code| key_name(*code)).collect();
                write!(f, "key:{}", names.join("+"))
            }
            Target::Desktop(action) => write!(f, "{action}"),
            Target::Flicks(targets) => {
                let names: Vec<String> = targets.iter().map(Target::to_string).collect();
                write!(f, "flicks:{}", names.join(","))
            }
            Target::Mouse(button) => {
                let (name, _) = MOUSE_TARGETS
                    .iter()
                    .find(|(_, known)| known == button)
                    .unwrap();
                write!(f, "mouse:{name}")
            }
            Target::Cursor => write!(f, "cursor"),
            Target::Scroll => write!(f, "scroll"),
            Target::Flick => write!(f, "flick"),
            Target::Profile(switch) => write!(f, "profile:{switch}"),
            Target::Layer(switch) => write!(f, "layer:{switch}"),
            Target::Menu(command) => write!(f, "menu:{command}"),
            Target::Keyboard(command) => write!(f, "keyboard:{command}"),
            Target::Lizard(switch) => write!(f, "lizard:{switch}"),
            Target::TvOff => write!(f, "tv:off"),
            Target::None => write!(f, "none"),
        }
    }
}

/// `pad:<name>` targets: buttons, then sticks, then triggers.
const PAD_TARGETS: [(&str, Target); 19] = [
    ("south", Target::Button(PadButton::South)),
    ("east", Target::Button(PadButton::East)),
    ("west", Target::Button(PadButton::West)),
    ("north", Target::Button(PadButton::North)),
    ("lb", Target::Button(PadButton::Lb)),
    ("rb", Target::Button(PadButton::Rb)),
    ("select", Target::Button(PadButton::Select)),
    ("start", Target::Button(PadButton::Start)),
    ("guide", Target::Button(PadButton::Guide)),
    ("thumbl", Target::Button(PadButton::ThumbL)),
    ("thumbr", Target::Button(PadButton::ThumbR)),
    ("dpad_up", Target::Button(PadButton::DpadUp)),
    ("dpad_down", Target::Button(PadButton::DpadDown)),
    ("dpad_left", Target::Button(PadButton::DpadLeft)),
    ("dpad_right", Target::Button(PadButton::DpadRight)),
    ("left", Target::Stick(PadStick::Left)),
    ("right", Target::Stick(PadStick::Right)),
    ("lt", Target::Trigger(PadTrigger::Lt)),
    ("rt", Target::Trigger(PadTrigger::Rt)),
];

/// `mouse:<name>` targets.
const MOUSE_TARGETS: [(&str, MouseButton); 3] = [
    ("left", MouseButton::Left),
    ("right", MouseButton::Right),
    ("middle", MouseButton::Middle),
];

/// A `KEY_*` code by name, the `KEY_` prefix optional.
fn key(name: &str) -> Result<u16> {
    let prefixed = match name.starts_with("KEY_") {
        true => name.to_string(),
        false => format!("KEY_{name}"),
    };
    let key = evdev::KeyCode::from_str(&prefixed).or(Err(anyhow!("Unknown key {name}")))?;
    Ok(key.0)
}

fn key_name(code: u16) -> String {
    format!("{:?}", evdev::KeyCode(code))
}

/// Key codes `target` can send.
fn target_keys(target: &Target) -> Vec<u16> {
    match target {
        Target::Key(code) => vec![*code],
        Target::Keys(codes) => codes.to_vec(),
        Target::Combo(codes) => codes.clone(),
        Target::Flicks(targets) => targets.iter().flat_map(target_keys).collect(),
        _ => vec![],
    }
}

/// Every `KEY_*` name a `key:` or `keys:` target accepts, by code, without `KEY_RESERVED`.
pub fn key_names() -> Vec<String> {
    (1..KEY_COUNT)
        .map(key_name)
        .filter(|name| name.starts_with("KEY_") && key(name).is_ok())
        .collect()
}

/// Key codes are below this (`KEY_CNT` of input-event-codes.h).
const KEY_COUNT: u16 = 0x300;

/// How long a stick's touch bit may drop out before the stick counts as let go. A thumb
/// settling on the cap flickers the bit in runs of 0.1-0.6 s for its first seconds (settop,
/// 2026-11-05), so the release side waits; touch itself counts at once.
pub const TOUCH_DEBOUNCE: Duration = Duration::from_millis(150);

/// Hold time after which `long` fires, by default.
pub const LONG_MS: u32 = 400;

/// Length of the `warn_ms` buzz: one long pulse, unlike the lizard cue's short ones.
const WARN_BUZZ_MS: u16 = 400;

/// The audible half of the `warn_ms` warning, on the other pad so it does not cut the buzz.
// TODO Unverified: pitch and loudness on hardware
const WARN_BEEP: Output = Output::Tone {
    side: Side::Left,
    hz: 1000,
    ms: 200,
};

/// Window in which a second press is a `double`, by default.
pub const DOUBLE_MS: u32 = 250;

/// What one button does, by Steam's activators. Unset slots are [`Target::None`].
#[derive(Clone, Debug, PartialEq)]
pub struct Binding {
    /// Held from press to release; waits for release when `long` is set, and for the double
    /// window to pass when `double` is set.
    pub press: Target,
    /// Tapped on every press edge.
    pub start: Target,
    /// Tapped on every release edge.
    pub release: Target,
    /// Held once the button has been down `long_ms`, until release.
    pub long: Target,
    /// Held from a second press within `double_ms` of the first, until release.
    pub double: Target,
    pub long_ms: u32,
    /// A warning buzz plays this many milliseconds before `long` fires, so a release can still
    /// cancel it.
    pub warn_ms: Option<u32>,
    pub double_ms: u32,
    /// `press` taps every this many milliseconds while held.
    pub turbo_ms: Option<u32>,
}
impl Binding {
    /// Every target, `press` first.
    pub fn targets(&self) -> [&Target; 5] {
        [
            &self.press,
            &self.start,
            &self.release,
            &self.long,
            &self.double,
        ]
    }
}
impl From<Target> for Binding {
    fn from(press: Target) -> Self {
        Self {
            press,
            start: Target::None,
            release: Target::None,
            long: Target::None,
            double: Target::None,
            long_ms: LONG_MS,
            warn_ms: None,
            double_ms: DOUBLE_MS,
            turbo_ms: None,
        }
    }
}

/// While `hold` is down, `buttons` map to their chord bindings and `hold` emits nothing.
#[derive(Clone, Debug, PartialEq)]
pub struct Chord {
    pub hold: Button,
    pub buttons: BTreeMap<Button, Binding>,
}

/// Bindings laid over a profile's while the layer is active; the rest stay the profile's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layer {
    pub buttons: BTreeMap<Button, Binding>,
    pub axes: BTreeMap<Axis, Target>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub deadzone_stick: f32,
    /// A stick counts as centred while its cap is not touched.
    pub stick_touch: bool,
    pub deadzone_trigger: f32,
    /// Share of travel past which a trigger bound to a button presses it.
    pub trigger_click: f32,
    pub buttons: BTreeMap<Button, Binding>,
    pub axes: BTreeMap<Axis, Target>,
    pub chords: Vec<Chord>,
    /// Layers by name, activated by [`Target::Layer`] bindings.
    pub layers: BTreeMap<String, Layer>,
    /// Cursor gain, 1.0 = [`CURSOR_GAIN`].
    pub mouse_speed: f32,
    /// Extra gain at fast swipes, see [`accelerate`].
    pub mouse_accel: f32,
    /// Reports a trackpad delta waits before it moves anything, so the twitch of a lifting
    /// finger is dropped with the pad's release; at most [`RELEASE_GUARD_MAX`].
    pub mouse_release_guard: u8,
    /// Strength of the trackpad's 1€ filter, 0 (off) to 1, see [`OneEuro::new`].
    pub mouse_smoothing: f32,
    /// Pixels a second of a stick bound to the cursor at full deflection.
    pub mouse_stick_speed: f32,
    /// Exponent of a stick's response curve past the deadzone, 1 = linear.
    pub mouse_stick_curve: f32,
    /// Pixels a flick stick turns the cursor per full turn of the stick.
    pub mouse_flick_degrees: f32,
    /// How long a flick stick's first turn takes, eased.
    pub mouse_flick_ms: u32,
    /// Scroll gain, 1.0 = [`SCROLL_GAIN`].
    pub scroll_speed: f32,
    /// Content follows the finger, like a touchpad.
    pub scroll_natural: bool,
    /// Notches a second of a stick bound to scroll at full deflection.
    pub scroll_stick_speed: f32,
    /// While set, the gyro moves the cursor only with this button held; unset, always.
    pub gyro_button: Option<Button>,
    /// Gyro gain, 1.0 = [`GYRO_GAIN`].
    pub gyro_speed: f32,
    /// Pulse on a button press that clicks the mouse or presses a key, 0 (off) to 1.
    pub haptics_click: f32,
    /// Tick every [`PAD_TICK_UNITS`] a trackpad moves the cursor or scrolls, 0 (off) to 1.
    pub haptics_pad_ticks: f32,
    /// Tick when the quick menu's or on-screen keyboard's focus moves or fires, 0 (off) to 1.
    pub haptics_menu: f32,
}
impl Profile {
    /// Plain Xbox layout, Steam = guide, no chords.
    pub fn xbox() -> Self {
        let buttons = BTreeMap::from([
            (Button::A, Target::Button(PadButton::South)),
            (Button::B, Target::Button(PadButton::East)),
            (Button::X, Target::Button(PadButton::West)),
            (Button::Y, Target::Button(PadButton::North)),
            (Button::Lb, Target::Button(PadButton::Lb)),
            (Button::Rb, Target::Button(PadButton::Rb)),
            (Button::View, Target::Button(PadButton::Select)),
            (Button::Menu, Target::Button(PadButton::Start)),
            (Button::Steam, Target::Button(PadButton::Guide)),
            (Button::LStickClick, Target::Button(PadButton::ThumbL)),
            (Button::RStickClick, Target::Button(PadButton::ThumbR)),
            (Button::DpadUp, Target::Button(PadButton::DpadUp)),
            (Button::DpadDown, Target::Button(PadButton::DpadDown)),
            (Button::DpadLeft, Target::Button(PadButton::DpadLeft)),
            (Button::DpadRight, Target::Button(PadButton::DpadRight)),
        ])
        .into_iter()
        .map(|(button, target)| (button, Binding::from(target)))
        .collect();
        let axes = BTreeMap::from([
            (Axis::LStick, Target::Stick(PadStick::Left)),
            (Axis::RStick, Target::Stick(PadStick::Right)),
            (Axis::Lt, Target::Trigger(PadTrigger::Lt)),
            (Axis::Rt, Target::Trigger(PadTrigger::Rt)),
        ]);
        Self {
            deadzone_stick: 0.10,
            stick_touch: true,
            deadzone_trigger: 0.05,
            trigger_click: 0.3,
            buttons,
            axes,
            chords: Vec::new(),
            layers: BTreeMap::new(),
            mouse_speed: 1.0,
            mouse_accel: 1.3,
            mouse_release_guard: 3,
            mouse_smoothing: 0.5,
            mouse_stick_speed: 1000.0,
            mouse_stick_curve: 1.6,
            mouse_flick_degrees: 2400.0,
            mouse_flick_ms: 100,
            scroll_speed: 1.0,
            scroll_natural: true,
            scroll_stick_speed: 10.0,
            gyro_button: None,
            gyro_speed: 1.0,
            haptics_click: 0.3,
            haptics_pad_ticks: 0.0,
            haptics_menu: 0.4,
        }
    }

    /// Key codes any binding of this profile can send.
    pub fn keys(&self) -> Vec<u16> {
        let chords = self.chords.iter().flat_map(|chord| chord.buttons.values());
        let layers = self.layers.values();
        self.buttons
            .values()
            .chain(chords)
            .chain(layers.clone().flat_map(|layer| layer.buttons.values()))
            .flat_map(Binding::targets)
            .chain(self.axes.values())
            .chain(layers.flat_map(|layer| layer.axes.values()))
            .flat_map(target_keys)
            .collect()
    }

    /// The profile with the `active` layers laid over it in order, the last on top.
    fn overlay(
        &self,
        active: &[String],
    ) -> Profile {
        let mut profile = self.clone();
        for layer in active.iter().filter_map(|name| self.layers.get(name)) {
            profile.buttons.extend(layer.buttons.clone());
            profile.axes.extend(layer.axes.clone());
        }
        profile
    }
}
impl Profile {
    /// Every profile switch this profile binds, by its binding path (`buttons.y`, `chord.0.y`).
    fn switches(&self) -> Vec<(String, &ProfileSwitch)> {
        self.targets()
            .into_iter()
            .filter_map(|(path, target)| match target {
                Target::Profile(switch) => Some((path, switch)),
                _ => None,
            })
            .collect()
    }

    /// Every button target of this profile, by its binding path (`buttons.y`, `chord.0.y`,
    /// `layer.aim.buttons.y`).
    fn targets(&self) -> Vec<(String, &Target)> {
        let buttons = self
            .buttons
            .iter()
            .map(|(button, binding)| (format!("buttons.{}", button.name()), binding));
        let chords = self.chords.iter().enumerate().flat_map(|(index, chord)| {
            chord
                .buttons
                .iter()
                .map(move |(button, binding)| (format!("chord.{index}.{}", button.name()), binding))
        });
        let layers = self.layers.iter().flat_map(|(name, layer)| {
            layer.buttons.iter().map(move |(button, binding)| {
                (format!("layer.{name}.buttons.{}", button.name()), binding)
            })
        });
        buttons
            .chain(chords)
            .chain(layers)
            .flat_map(|(path, binding)| {
                binding
                    .targets()
                    .into_iter()
                    .map(move |target| (path.clone(), target))
            })
            .collect()
    }
}
impl Default for Profile {
    fn default() -> Self {
        Self::xbox()
    }
}
impl TryFrom<ProfileToml> for Profile {
    type Error = anyhow::Error;

    fn try_from(toml: ProfileToml) -> Result<Self> {
        let mut profile = Profile::xbox();
        if let Some(layout) = toml.layout
            && layout != "xbox"
        {
            return Err(anyhow!("Unknown layout {layout}"));
        }
        if let Some(deadzone) = toml.deadzone.stick {
            profile.deadzone_stick = deadzone;
        }
        if let Some(deadzone) = toml.deadzone.trigger {
            profile.deadzone_trigger = deadzone;
        }
        if let Some(touch) = toml.stick.touch {
            profile.stick_touch = touch;
        }
        if let Some(click) = toml.trigger.click {
            profile.trigger_click = click;
        }
        if let Some(speed) = toml.mouse.speed {
            profile.mouse_speed = speed;
        }
        if let Some(accel) = toml.mouse.accel {
            profile.mouse_accel = accel;
        }
        if let Some(guard) = toml.mouse.release_guard {
            if guard > RELEASE_GUARD_MAX {
                return Err(anyhow!(
                    "mouse.release_guard: {guard} is over {RELEASE_GUARD_MAX}"
                ));
            }
            profile.mouse_release_guard = guard;
        }
        if let Some(smoothing) = toml.mouse.smoothing {
            if !(0.0..=1.0).contains(&smoothing) {
                return Err(anyhow!("mouse.smoothing: {smoothing} is not within 0 to 1"));
            }
            profile.mouse_smoothing = smoothing;
        }
        if let Some(speed) = toml.mouse.stick_speed {
            profile.mouse_stick_speed = speed;
        }
        if let Some(curve) = toml.mouse.stick_curve {
            if !(1.0..=3.0).contains(&curve) {
                return Err(anyhow!("mouse.stick_curve: {curve} is not within 1 to 3"));
            }
            profile.mouse_stick_curve = curve;
        }
        if let Some(degrees) = toml.mouse.flick_degrees {
            profile.mouse_flick_degrees = degrees;
        }
        if let Some(ms) = toml.mouse.flick_ms {
            profile.mouse_flick_ms = ms;
        }
        if let Some(speed) = toml.scroll.stick_speed {
            profile.scroll_stick_speed = speed;
        }
        if let Some(speed) = toml.scroll.speed {
            profile.scroll_speed = speed;
        }
        if let Some(natural) = toml.scroll.natural {
            profile.scroll_natural = natural;
        }
        if let Some(button) = &toml.gyro.button {
            let Ok(Source::Button(button)) = button.parse() else {
                return Err(anyhow!("gyro.button: {button} is not a button"));
            };
            profile.gyro_button = Some(button);
        }
        if let Some(speed) = toml.gyro.speed {
            profile.gyro_speed = speed;
        }
        for (name, value, knob) in [
            ("click", toml.haptics.click, &mut profile.haptics_click),
            (
                "pad_ticks",
                toml.haptics.pad_ticks,
                &mut profile.haptics_pad_ticks,
            ),
            ("menu", toml.haptics.menu, &mut profile.haptics_menu),
        ] {
            let Some(value) = value else {
                continue;
            };
            if !(0.0..=1.0).contains(&value) {
                return Err(anyhow!("haptics.{name}: {value} is not within 0 to 1"));
            }
            *knob = value;
        }
        for (source, toml) in &toml.buttons {
            let (button, mut binding) =
                button_binding(source, toml).map_err(|err| anyhow!("buttons.{source}: {err}"))?;
            // A table without `press` keeps the layout's
            if toml.press().is_none()
                && let Some(layout) = profile.buttons.get(&button)
            {
                binding.press = layout.press.clone();
            }
            profile.buttons.insert(button, binding);
        }
        for (source, target) in &toml.axes {
            let (axis, target) =
                axis_binding(source, target).map_err(|err| anyhow!("axes.{source}: {err}"))?;
            profile.axes.insert(axis, target);
        }
        for (index, chord) in toml.chord.into_iter().enumerate() {
            let Ok(Source::Button(hold)) = chord.hold.parse() else {
                return Err(anyhow!(
                    "chord.{index}.hold: {} is not a button",
                    chord.hold
                ));
            };
            let mut buttons = BTreeMap::new();
            for (source, toml) in &chord.buttons {
                let (button, binding) = button_binding(source, toml)
                    .map_err(|err| anyhow!("chord.{index}.{source}: {err}"))?;
                buttons.insert(button, binding);
            }
            profile.chords.push(Chord { hold, buttons });
        }
        for (name, toml) in &toml.layer {
            let mut layer = Layer::default();
            for (source, toml) in &toml.buttons {
                let (button, mut binding) = button_binding(source, toml)
                    .map_err(|err| anyhow!("layer.{name}.buttons.{source}: {err}"))?;
                // A table without `press` keeps the profile's
                if toml.press().is_none()
                    && let Some(below) = profile.buttons.get(&button)
                {
                    binding.press = below.press.clone();
                }
                layer.buttons.insert(button, binding);
            }
            for (source, target) in &toml.axes {
                let (axis, target) = axis_binding(source, target)
                    .map_err(|err| anyhow!("layer.{name}.axes.{source}: {err}"))?;
                layer.axes.insert(axis, target);
            }
            profile.layers.insert(name.clone(), layer);
        }
        for (path, target) in profile.targets() {
            if let Target::Layer(switch) = target
                && !profile.layers.contains_key(switch.name())
            {
                return Err(anyhow!("{path}: No layer named {}", switch.name()));
            }
        }
        Ok(profile)
    }
}

/// Whether `source` can drive `target`: the one place the binding rules live.
fn allows(
    source: Source,
    target: &Target,
) -> bool {
    match (source, target) {
        (_, Target::None) => true,
        (Source::Axis(Axis::Gyro), target) => *target == Target::Cursor,
        (
            Source::Button(_),
            Target::Button(_)
            | Target::Trigger(_)
            | Target::Key(_)
            | Target::Combo(_)
            | Target::Desktop(_)
            | Target::Mouse(_)
            | Target::Profile(_)
            | Target::Layer(_)
            | Target::Menu(_)
            | Target::Keyboard(_)
            | Target::Lizard(_)
            | Target::TvOff,
        ) => true,
        (
            Source::Axis(axis),
            Target::Trigger(_)
            | Target::Button(_)
            | Target::Key(_)
            | Target::Combo(_)
            | Target::Desktop(_)
            | Target::Mouse(_),
        ) => axis.is_trigger(),
        (Source::Axis(axis), Target::Stick(_) | Target::Keys(_)) => !axis.is_trigger(),
        (Source::Axis(axis), Target::Flicks(targets)) => {
            !axis.is_trigger()
                && targets.iter().all(|target| {
                    matches!(
                        target,
                        Target::Button(_)
                            | Target::Key(_)
                            | Target::Combo(_)
                            | Target::Desktop(_)
                            | Target::Mouse(_)
                            | Target::Profile(_)
                            | Target::None
                    )
                })
        }
        (Source::Axis(Axis::LPad | Axis::RPad), Target::Cursor | Target::Scroll) => true,
        (
            Source::Axis(Axis::LStick | Axis::RStick),
            Target::Cursor | Target::Scroll | Target::Flick,
        ) => true,
        _ => false,
    }
}

fn binding(
    source: &str,
    target: &str,
) -> Result<(Source, Target)> {
    let parsed: Source = source.parse()?;
    let target: Target = target.parse()?;
    if !allows(parsed, &target) {
        return Err(anyhow!("Cannot map {source} to {target}"));
    }
    Ok((parsed, target))
}

fn button_target(
    source: &str,
    target: &str,
) -> Result<(Button, Target)> {
    match binding(source, target)? {
        (Source::Button(button), target) => Ok((button, target)),
        (Source::Axis(_), _) => Err(anyhow!("{source} is an axis, not a button")),
    }
}

/// A button's binding; a slot the table leaves out is [`Target::None`].
fn button_binding(
    source: &str,
    toml: &BindingToml,
) -> Result<(Button, Binding)> {
    let activators = match toml {
        BindingToml::Target(target) => {
            let (button, target) = button_target(source, target)?;
            return Ok((button, Binding::from(target)));
        }
        BindingToml::Activators(activators) => activators,
    };
    let Ok(Source::Button(button)) = source.parse() else {
        return Err(anyhow!("{source} is not a button"));
    };
    let slot = |name: &str, target: &Option<String>| -> Result<Target> {
        let Some(target) = target else {
            return Ok(Target::None);
        };
        let (_, target) = button_target(source, target).map_err(|err| anyhow!("{name}: {err}"))?;
        Ok(target)
    };
    let ms = |name: &str, ms: Option<u32>, default: u32| -> Result<u32> {
        match ms {
            Some(0) => Err(anyhow!("{name}: must be above 0")),
            Some(ms) => Ok(ms),
            None => Ok(default),
        }
    };
    let binding = Binding {
        press: slot("press", &activators.press)?,
        start: slot("start", &activators.start)?,
        release: slot("release", &activators.release)?,
        long: slot("long", &activators.long)?,
        double: slot("double", &activators.double)?,
        long_ms: ms("long_ms", activators.long_ms, LONG_MS)?,
        warn_ms: match activators.warn_ms {
            Some(warn) if warn >= activators.long_ms.unwrap_or(LONG_MS) => {
                return Err(anyhow!("warn_ms: must be below long_ms"));
            }
            Some(warn) => Some(ms("warn_ms", Some(warn), 0)?),
            None => None,
        },
        double_ms: ms("double_ms", activators.double_ms, DOUBLE_MS)?,
        turbo_ms: match activators.turbo_ms {
            Some(turbo) => Some(ms("turbo_ms", Some(turbo), 0)?),
            None => None,
        },
    };
    Ok((button, binding))
}

fn axis_binding(
    source: &str,
    target: &str,
) -> Result<(Axis, Target)> {
    match binding(source, target)? {
        (Source::Axis(axis), target) => Ok((axis, target)),
        (Source::Button(_), _) => Err(anyhow!("{source} is a button, not an axis")),
    }
}

/// A kind of target as an editor offers it: `<prefix><arg>`, `count` args joined by commas, or
/// just `name` when it takes none.
#[derive(Encode, Decode, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TargetKind {
    pub name: String,
    pub prefix: String,
    pub args: Vec<String>,
    pub count: u8,
}
impl TargetKind {
    fn new(
        name: &str,
        prefix: &str,
        args: Vec<String>,
        count: u8,
    ) -> Self {
        Self {
            name: name.to_string(),
            prefix: prefix.to_string(),
            args,
            count,
        }
    }

    /// The target string with every argument slot set to `arg`.
    pub fn target(
        &self,
        arg: &str,
    ) -> String {
        if self.count == 0 {
            return self.name.clone();
        }
        let args = vec![arg; self.count as usize];
        format!("{}{}", self.prefix, args.join(","))
    }
}

/// What a binding editor needs to offer only valid choices.
#[derive(Encode, Decode, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Vocabulary {
    pub buttons: Vec<String>,
    pub axes: Vec<String>,
    pub layouts: Vec<String>,
    pub kinds: Vec<TargetKind>,
    /// The kind names each source accepts, by source name.
    pub allowed: BTreeMap<String, Vec<String>>,
    /// The Xbox layout's target of each source it binds.
    pub defaults: BTreeMap<String, String>,
    /// The icon names a `[menu]` item may use.
    pub menu_icons: Vec<String>,
}
impl Default for Vocabulary {
    fn default() -> Self {
        Self::new()
    }
}
impl Vocabulary {
    pub fn new() -> Self {
        let pad = |kind: fn(&Target) -> bool| -> Vec<String> {
            PAD_TARGETS
                .iter()
                .filter(|(_, target)| kind(target))
                .map(|(name, _)| name.to_string())
                .collect()
        };
        let keys = key_names();
        let kinds = vec![
            TargetKind::new("button", "pad:", pad(|t| matches!(t, Target::Button(_))), 1),
            TargetKind::new("stick", "pad:", pad(|t| matches!(t, Target::Stick(_))), 1),
            TargetKind::new(
                "trigger",
                "pad:",
                pad(|t| matches!(t, Target::Trigger(_))),
                1,
            ),
            TargetKind::new("key", "key:", keys.clone(), 1),
            TargetKind::new("keys", "keys:", keys.clone(), 4),
            // Each slot any button-like target; a bare key name is a key
            TargetKind::new("flicks", "flicks:", keys, 4),
            // `spawn:<command>` and `workspace:<n>` are open-ended, so not listed
            TargetKind::new(
                "desktop",
                "desktop:",
                NAMES.iter().map(|(name, _)| name.to_string()).collect(),
                1,
            ),
            // Advanced: niri's own actions; `spawn:` and `<name>:<arg>` are not listed
            TargetKind::new("niri (raw)", "niri:", Action::raw_names(), 1),
            TargetKind::new(
                "mouse",
                "mouse:",
                MOUSE_TARGETS
                    .iter()
                    .map(|(name, _)| name.to_string())
                    .collect(),
                1,
            ),
            TargetKind::new(
                "profile",
                "profile:",
                vec!["next".to_string(), "prev".to_string()],
                1,
            ),
            TargetKind::new(
                "menu",
                "menu:",
                MenuCommand::NAMES
                    .iter()
                    .map(|name| name.to_string())
                    .collect(),
                1,
            ),
            TargetKind::new(
                "keyboard",
                "keyboard:",
                MenuCommand::NAMES
                    .iter()
                    .map(|name| name.to_string())
                    .collect(),
                1,
            ),
            TargetKind::new(
                "lizard",
                "lizard:",
                vec!["hold".to_string(), "toggle".to_string()],
                1,
            ),
            TargetKind::new("cursor", "", Vec::new(), 0),
            TargetKind::new("scroll", "", Vec::new(), 0),
            TargetKind::new("flick", "", Vec::new(), 0),
            TargetKind::new("none", "", Vec::new(), 0),
        ];
        let buttons: Vec<String> = Button::ALL.iter().map(|b| b.name().to_string()).collect();
        let axes: Vec<String> = Axis::ALL.iter().map(|a| a.name().to_string()).collect();
        let allowed = buttons
            .iter()
            .chain(&axes)
            .map(|source| {
                let names = kinds
                    .iter()
                    .filter(|kind| {
                        let arg = kind.args.first().map_or("", String::as_str);
                        binding(source, &kind.target(arg)).is_ok()
                    })
                    .map(|kind| kind.name.clone())
                    .collect();
                (source.clone(), names)
            })
            .collect();
        let xbox = Profile::xbox();
        let defaults = xbox
            .buttons
            .iter()
            .map(|(button, binding)| (button.name().to_string(), binding.press.to_string()))
            .chain(
                xbox.axes
                    .iter()
                    .map(|(axis, target)| (axis.name().to_string(), target.to_string())),
            )
            .collect();
        Self {
            buttons,
            axes,
            layouts: vec!["xbox".to_string()],
            kinds,
            allowed,
            defaults,
            menu_icons: crate::menu::ICONS.iter().map(|i| i.to_string()).collect(),
        }
    }
}

#[derive(Encode, Decode, Serialize, Deserialize, Copy, Clone, Debug, Default, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Lizard {
    #[default]
    Off,
    On,
    Auto,
}

/// What the daemon does while Steam has the controller open.
#[derive(Encode, Decode, Serialize, Deserialize, Copy, Clone, Debug, Default, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Steam {
    /// Close the controller and drop the virtual devices until Steam lets go.
    #[default]
    Yield,
    /// Keep running alongside Steam.
    Ignore,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Daemon {
    pub active: Option<String>,
    pub lizard: Lizard,
    pub imu: bool,
    pub steam: Steam,
    /// Games' rumble on the virtual pad plays on the controller's motors.
    pub rumble: bool,
    /// A `lizard:` binding taking over or handing back is felt on the right trackpad.
    pub lizard_cue: bool,
    /// Shell commands that put the TV to standby (a `tv:off` binding) and wake it (the first
    /// button held `tv_wake_ms` after).
    pub tv_off: Option<String>,
    pub tv_on: Option<String>,
    pub tv_wake_ms: u32,
}
impl Daemon {
    /// How long a button is held to wake the TV after `tv:off`.
    pub fn tv_wake(&self) -> Duration {
        ms(self.tv_wake_ms)
    }
}
impl Default for Daemon {
    fn default() -> Self {
        Self {
            active: None,
            lizard: Lizard::default(),
            imu: false,
            steam: Steam::default(),
            rumble: true,
            lizard_cue: true,
            tv_off: None,
            tv_on: None,
            tv_wake_ms: 1000,
        }
    }
}

/// Share of the deadzone a flicked stick must fall below to release its key.
const FLICK_RELEASE: f32 = 0.8;

/// Share of `trigger.click` a pulled trigger must fall below to release its button.
const CLICK_RELEASE: f32 = 0.8;

/// Pixels per trackpad unit at `mouse.speed = 1`: the pad is 65534 units wide and one full
/// unaccelerated swipe crosses three quarters of a 1920 px screen, so 1440 / 65534.
const CURSOR_GAIN: f32 = 1440.0 / 65534.0;

/// Pad units a touch travels before it scrolls, 1/50 of the pad.
const SCROLL_START: f32 = 65534.0 / 50.0;

/// How many times faster the other axis must move to take a scroll's axis lock.
const SCROLL_SWITCH: f32 = 2.5;

/// Cutoff in Hz of the scroll output's exponential filter.
const SCROLL_CUTOFF: f32 = 15.0;

/// Hi-res wheel units per trackpad unit at `scroll.speed = 1`: a full swipe scrolls 10 notches.
const SCROLL_GAIN: f32 = 10.0 * NOTCH as f32 / 65534.0;

/// Most reports `mouse.release_guard` may hold back.
pub const RELEASE_GUARD_MAX: u8 = 20;

/// Trackpad units per report at which acceleration reaches its full `mouse.accel`.
const ACCEL_FULL: f32 = 2000.0;

/// Pixels per degree the controller turns at `gyro.speed = 1`: a 1920 px screen is 48°.
const GYRO_GAIN: f32 = 40.0;

/// Gyro units per degree a second (`STEAM_GYRO_RES_PER_DPS` in hid-steam).
const GYRO_UNITS_PER_DPS: f32 = 16.0;

/// Angular rate below which the gyro counts as still, so a resting controller does not drift.
const GYRO_NOISE_DPS: f32 = 1.0;

/// Controller reports per second (measured: ~265 on the 2026 controller's puck, 2026-11-01);
/// the state carries no timestamp, so this is assumed.
const REPORT_RATE: f32 = 250.0;

/// Cutoff of the 1€ filter's speed estimate. The paper's 1 Hz needs most of a second to see a
/// flick start, so its first report would lag; 25 Hz sees it within one report.
const SMOOTHING_D_CUTOFF: f32 = 25.0;

/// Cutoff of the 1€ filter at rest and full `mouse.smoothing`. Tuned by feel at 0.5 when the
/// code assumed 100 Hz; scaled by 2.5 with the rate so each report smooths exactly as before.
const SMOOTHING_MIN_CUTOFF: f32 = 2.5;

/// Cutoff added per pad unit a second of speed at full `mouse.smoothing`.
const SMOOTHING_BETA: f32 = 0.002;

/// Trackpad units a finger travels between two `haptics.pad_ticks`: 1/32 of the pad's width.
pub const PAD_TICK_UNITS: f32 = 65534.0 / 32.0;

/// Length of a `haptics.click` pulse.
pub const CLICK_MS: u16 = 10;

/// Length of a `haptics.pad_ticks` or `haptics.menu` tick.
pub const TICK_MS: u16 = 4;

/// Name of the built-in Xbox layout.
pub const PAD: &str = "pad";

/// A whole config file. The default has no profiles and runs the Xbox layout.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config {
    pub daemon: Daemon,
    pub profiles: BTreeMap<String, Profile>,
    /// `[menu]`, else [`Menu::default`].
    pub menu: Menu,
    pub auto: Auto,
    /// `[desktop] backend`.
    pub desktop: Backend,
}
impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .or(Err(anyhow!("Cannot read config {}", path.display())))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self> {
        Self::try_from(ConfigToml::parse(text)?)
    }

    /// `daemon.active`, else the only profile, else the built-in [`PAD`].
    pub fn active(&self) -> Result<String> {
        if let Some(name) = &self.daemon.active {
            return Ok(name.clone());
        }
        match self.profiles.len() {
            0 => Ok(PAD.to_string()),
            1 => Ok(self.profiles.keys().next().cloned().unwrap()),
            _ => Err(anyhow!("Several profiles and no daemon.active")),
        }
    }

    /// The named profile; [`PAD`] is the Xbox layout unless the file defines it.
    pub fn get(
        &self,
        name: &str,
    ) -> Result<Profile> {
        if let Some(profile) = self.profiles.get(name) {
            return Ok(profile.clone());
        }
        if name == PAD {
            return Ok(Profile::xbox());
        }
        Err(anyhow!("No profile named {name}"))
    }

    /// The active profile.
    pub fn profile(&self) -> Result<Profile> {
        self.get(&self.active()?)
    }

    /// Key codes any profile or the on-screen keyboard can send.
    pub fn keys(&self) -> Vec<u16> {
        self.profiles
            .values()
            .flat_map(Profile::keys)
            .chain(keyboard::keys())
            .collect()
    }
}

impl TryFrom<ConfigToml> for Config {
    type Error = anyhow::Error;

    fn try_from(toml: ConfigToml) -> Result<Self> {
        let mut profiles = BTreeMap::new();
        for (name, profile) in toml.profile {
            let profile = match Profile::try_from(profile) {
                Ok(profile) => profile,
                Err(err) => return Err(anyhow!("Profile {name}: {err}")),
            };
            profiles.insert(name, profile);
        }
        for (name, profile) in &profiles {
            for (binding, switch) in profile.switches() {
                let ProfileSwitch::Named(to) = switch else {
                    continue;
                };
                if to != PAD && !profiles.contains_key(to) {
                    return Err(anyhow!("Profile {name}: {binding}: No profile named {to}"));
                }
            }
        }
        // The default menu's profile items name profiles this config may not have
        let menu = match &toml.menu {
            Some(menu) => Menu::try_from(menu)?,
            None => Menu::default(),
        };
        if toml.menu.is_some() {
            for (index, target) in menu.targets().enumerate() {
                let Target::Profile(ProfileSwitch::Named(to)) = target else {
                    continue;
                };
                if to != PAD && !profiles.contains_key(to) {
                    return Err(anyhow!("menu.items.{index}: No profile named {to}"));
                }
            }
        }
        let auto = Auto::try_from(&toml.auto)?;
        let named = auto
            .rules
            .iter()
            .enumerate()
            .map(|(index, rule)| (format!("auto.rule.{index}"), &rule.profile))
            .chain(
                auto.default
                    .iter()
                    .map(|to| ("auto.default".to_string(), to)),
            );
        for (field, to) in named {
            if to != PAD && !profiles.contains_key(to) {
                return Err(anyhow!("{field}: No profile named {to}"));
            }
        }
        Ok(Self {
            daemon: Daemon {
                active: toml.daemon.active,
                lizard: toml.daemon.lizard,
                imu: toml.daemon.imu,
                steam: toml.daemon.steam,
                rumble: toml.daemon.rumble.unwrap_or(true),
                lizard_cue: toml.daemon.lizard_cue.unwrap_or(true),
                tv_off: toml.daemon.tv_off,
                tv_on: toml.daemon.tv_on,
                tv_wake_ms: toml.daemon.tv_wake_ms.unwrap_or(1000),
            },
            profiles,
            menu,
            auto,
            desktop: toml.desktop.backend,
        })
    }
}

// TODO serde is the one departure from hand-rolled parsing; these mirror the file only
/// A config file as written: names and target strings, unresolved. Editors round-trip this.
#[derive(Encode, Decode, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ConfigToml {
    #[serde(skip_serializing_if = "is_default")]
    pub daemon: DaemonToml,
    pub profile: BTreeMap<String, ProfileToml>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub menu: Option<MenuToml>,
    #[serde(skip_serializing_if = "is_default")]
    pub auto: AutoToml,
    #[serde(skip_serializing_if = "is_default")]
    pub desktop: DesktopToml,
}
impl ConfigToml {
    pub fn parse(text: &str) -> Result<Self> {
        Ok(toml::from_str(text)?)
    }

    pub fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string(self)?)
    }
}

/// `[desktop]`: which desktop the pad drives.
#[derive(Encode, Decode, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct DesktopToml {
    pub backend: Backend,
}

#[derive(Encode, Decode, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct DaemonToml {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<String>,
    pub lizard: Lizard,
    pub imu: bool,
    pub steam: Steam,
    /// Unset is on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rumble: Option<bool>,
    /// Unset is on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lizard_cue: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tv_off: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tv_on: Option<String>,
    /// Unset is 1000.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tv_wake_ms: Option<u32>,
}

#[derive(Encode, Decode, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ProfileToml {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout: Option<String>,
    #[serde(skip_serializing_if = "is_default")]
    pub deadzone: DeadzoneToml,
    #[serde(skip_serializing_if = "is_default")]
    pub stick: StickToml,
    #[serde(skip_serializing_if = "is_default")]
    pub trigger: TriggerToml,
    #[serde(skip_serializing_if = "is_default")]
    pub mouse: MouseToml,
    #[serde(skip_serializing_if = "is_default")]
    pub scroll: ScrollToml,
    #[serde(skip_serializing_if = "is_default")]
    pub haptics: HapticsToml,
    #[serde(skip_serializing_if = "is_default")]
    pub gyro: GyroToml,
    #[serde(skip_serializing_if = "is_default")]
    pub buttons: BTreeMap<String, BindingToml>,
    #[serde(skip_serializing_if = "is_default")]
    pub axes: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "is_default")]
    pub chord: Vec<ChordToml>,
    #[serde(skip_serializing_if = "is_default")]
    pub layer: BTreeMap<String, LayerToml>,
}

/// A layer as written: only the bindings it replaces while active.
#[derive(Encode, Decode, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct LayerToml {
    #[serde(skip_serializing_if = "is_default")]
    pub buttons: BTreeMap<String, BindingToml>,
    #[serde(skip_serializing_if = "is_default")]
    pub axes: BTreeMap<String, String>,
}

#[derive(Encode, Decode, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct MouseToml {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accel: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_guard: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smoothing: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stick_speed: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stick_curve: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flick_degrees: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flick_ms: Option<u32>,
}

#[derive(Encode, Decode, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ScrollToml {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub natural: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stick_speed: Option<f32>,
}

#[derive(Encode, Decode, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct GyroToml {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub button: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
}

#[derive(Encode, Decode, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct HapticsToml {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub click: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pad_ticks: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub menu: Option<f32>,
}

#[derive(Encode, Decode, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct TriggerToml {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub click: Option<f32>,
}

#[derive(Encode, Decode, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct DeadzoneToml {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stick: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger: Option<f32>,
}

#[derive(Encode, Decode, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct StickToml {
    /// Unset is on: a stick only counts while its cap is touched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub touch: Option<bool>,
}

#[derive(Encode, Decode, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ChordToml {
    pub hold: String,
    #[serde(flatten)]
    pub buttons: BTreeMap<String, BindingToml>,
}

/// A button binding as written: a target string, the same as `{ press = "..." }`, or a table
/// of activators.
#[derive(Encode, Decode, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub enum BindingToml {
    Target(String),
    Activators(ActivatorsToml),
}
impl BindingToml {
    /// The `press` target, if the binding names one.
    pub fn press(&self) -> Option<&str> {
        match self {
            BindingToml::Target(target) => Some(target),
            BindingToml::Activators(activators) => activators.press.as_deref(),
        }
    }
}
impl From<String> for BindingToml {
    fn from(target: String) -> Self {
        BindingToml::Target(target)
    }
}
impl From<&str> for BindingToml {
    fn from(target: &str) -> Self {
        BindingToml::Target(target.to_string())
    }
}
impl PartialEq<&str> for BindingToml {
    fn eq(
        &self,
        other: &&str,
    ) -> bool {
        matches!(self, BindingToml::Target(target) if target == other)
    }
}

/// Steam's activators on one button, see [`Binding`].
#[derive(Encode, Decode, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ActivatorsToml {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub press: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub long: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub double: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub long_ms: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warn_ms: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub double_ms: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turbo_ms: Option<u32>,
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// Turns state changes into outputs for one controller.
pub struct Mapper {
    /// The profile as configured, without layers.
    base: Profile,
    /// [`Mapper::base`] with the active layers laid over it: what the controller runs.
    profile: Profile,
    /// Names of the active layers, the newest last.
    layers: Vec<String>,
    /// The chords whose hold is down, the newest last, each with whether a member was pressed
    /// since: a touch hold such as `grip_r_touch` stays down under the others.
    chords: Vec<(usize, bool)>,
    /// The activators of each pressed button, and of each released one still owing output.
    pressed: BTreeMap<Button, Activator>,
    /// The key each flicked axis holds.
    flicks: BTreeMap<Axis, u16>,
    /// The slot and target each [`Target::Flicks`] stick holds.
    flicked: BTreeMap<Axis, (u16, Target)>,
    /// Finger tracking of each trackpad bound to the cursor or scroll; a stick's keeps only
    /// its fraction of a unit.
    motions: BTreeMap<Axis, Motion>,
    /// Each flick stick's angle and turn in progress.
    turns: BTreeMap<Axis, Turn>,
    /// What each trigger pulled past its click emitted, so its release matches.
    pulls: BTreeMap<Axis, Target>,
    /// Until when each stick still counts as touched, left then right: its touch bit was on
    /// less than [`TOUCH_DEBOUNCE`] ago.
    touched: [Option<Instant>; 2],
    /// When the last report was mapped, to read `prev` as it was seen then.
    last: Option<Instant>,
    /// The virtual stick directions the last report pressed.
    directions: Buttons,
    /// Everything was released: the chords whose hold is still down take effect again on the
    /// next report, as that hold makes no new press edge.
    rearm: bool,
}
impl Mapper {
    pub fn new(profile: Profile) -> Self {
        Self {
            base: profile.clone(),
            profile,
            layers: Vec::new(),
            chords: Vec::new(),
            pressed: BTreeMap::new(),
            flicks: BTreeMap::new(),
            flicked: BTreeMap::new(),
            motions: BTreeMap::new(),
            turns: BTreeMap::new(),
            pulls: BTreeMap::new(),
            touched: [None; 2],
            last: None,
            directions: Buttons::empty(),
            rearm: false,
        }
    }

    /// `state` as the profile sees it: with `stick_touch`, a stick whose cap has not been
    /// touched for [`TOUCH_DEBOUNCE`] is centred. The menu and keyboard read this too.
    pub fn touched(
        &mut self,
        state: &ControllerState,
        now: Instant,
    ) -> ControllerState {
        let touched = [state.buttons.lstick_touch(), state.buttons.rstick_touch()];
        for (touched, until) in touched.into_iter().zip(&mut self.touched) {
            if touched {
                *until = Some(now + TOUCH_DEBOUNCE);
            }
        }
        self.gate(state, now)
    }

    /// [`Mapper::touched`] without the bookkeeping: `state` with every stick centred whose cap
    /// is not touched at `now`.
    fn gate(
        &self,
        state: &ControllerState,
        now: Instant,
    ) -> ControllerState {
        let mut seen = *state;
        if !self.profile.stick_touch {
            return seen;
        }
        let sticks = [
            (state.buttons.lstick_touch(), &mut seen.lstick),
            (state.buttons.rstick_touch(), &mut seen.rstick),
        ];
        for ((touched, stick), until) in sticks.into_iter().zip(self.touched) {
            if !touched && until.is_none_or(|until| now >= until) {
                *stick = Stick::default();
            }
        }
        seen
    }

    pub fn map(
        &mut self,
        prev: &ControllerState,
        state: &ControllerState,
    ) -> Vec<Output> {
        self.map_at(prev, state, Instant::now())
    }

    /// [`Mapper::map`] at `now`. Activator timers run on reports, so they fire at the next one
    /// past their deadline (4 ms apart at 250 Hz).
    pub fn map_at(
        &mut self,
        prev: &ControllerState,
        state: &ControllerState,
        now: Instant,
    ) -> Vec<Output> {
        let mut prev = self.gate(prev, self.last.unwrap_or(now));
        prev.buttons = with_directions(prev.buttons, self.directions);
        let mut state = self.touched(state, now);
        self.directions = directions(&state, self.directions);
        state.buttons = with_directions(state.buttons, self.directions);
        let (prev, state) = (&prev, &state);
        if std::mem::take(&mut self.rearm) {
            self.rearm_chords(prev);
        }
        self.last = Some(now);
        let mut outputs = Vec::new();
        self.pressed.retain(|button, activator| {
            !activator.tick(now, state.buttons.contains(*button), &mut outputs)
        });
        for button in Button::ALL {
            let was = prev.buttons.contains(button);
            let is = state.buttons.contains(button);
            if !was && is {
                let start = outputs.len();
                self.press(button, now, &mut outputs);
                let clicks = outputs[start..].iter().any(|output| {
                    matches!(output, Output::Key(_, true) | Output::MouseButton(_, true))
                });
                if clicks {
                    outputs.extend(Output::haptic(
                        button.side(),
                        self.profile.haptics_click,
                        CLICK_MS,
                    ));
                }
            } else if was && !is {
                self.release(button, now, &mut outputs);
            }
        }
        self.switch_layers(&mut outputs);
        let state = &self.unclaimed(state);
        for (axis, target) in &self.profile.axes {
            if let Target::Keys(keys) = target {
                let held = self.flicks.get(axis).copied();
                let flick = flick(
                    axis_value(*axis, state),
                    *keys,
                    self.profile.deadzone_stick,
                    held,
                );
                if flick == held {
                    continue;
                }
                if let Some(key) = held {
                    outputs.push(Output::Key(key, false));
                }
                if let Some(key) = flick {
                    outputs.push(Output::Key(key, true));
                }
                match flick {
                    Some(key) => self.flicks.insert(*axis, key),
                    None => self.flicks.remove(axis),
                };
                continue;
            }
            if let Target::Flicks(targets) = target {
                let held = self.flicked.get(axis).map(|(index, _)| *index);
                let flick = flick(
                    axis_value(*axis, state),
                    [0, 1, 2, 3],
                    self.profile.deadzone_stick,
                    held,
                );
                if flick == held {
                    continue;
                }
                if let Some((_, target)) = self.flicked.remove(axis) {
                    emit(&target, false, &mut outputs);
                }
                if let Some(index) = flick {
                    let target = targets[index as usize].clone();
                    emit(&target, true, &mut outputs);
                    self.flicked.insert(*axis, (index, target));
                }
                continue;
            }
            if let Target::Button(_)
            | Target::Key(_)
            | Target::Combo(_)
            | Target::Desktop(_)
            | Target::Mouse(_) = target
            {
                let value = match axis {
                    Axis::Lt => state.lt,
                    Axis::Rt => state.rt,
                    _ => continue,
                };
                let held = self.pulls.contains_key(axis);
                if pull(value, self.profile.trigger_click, held) == held {
                    continue;
                }
                match self.pulls.remove(axis) {
                    Some(target) => emit(&target, false, &mut outputs),
                    None => {
                        self.pulls.insert(*axis, target.clone());
                        emit(target, true, &mut outputs);
                    }
                }
                continue;
            }
            if let Target::Cursor | Target::Scroll = target {
                let pad = match axis {
                    Axis::LPad => Some((Side::Left, state.lpad)),
                    Axis::RPad => Some((Side::Right, state.rpad)),
                    _ => None,
                };
                let output = match (axis, pad) {
                    (_, Some((side, pad))) => {
                        let motions = &mut self.motions;
                        let output = motion(&self.profile, motions, *axis, target, pad);
                        if output.is_some()
                            && let Some(motion) = motions.get_mut(axis)
                            && motion.tick()
                        {
                            outputs.extend(Output::haptic(
                                side,
                                self.profile.haptics_pad_ticks,
                                TICK_MS,
                            ));
                        }
                        output
                    }
                    (Axis::LStick | Axis::RStick, _) => {
                        let motion = self.motions.entry(*axis).or_default();
                        velocity(&self.profile, motion, target, axis_value(*axis, state))
                    }
                    (Axis::Gyro, _) => {
                        let enabled = self
                            .profile
                            .gyro_button
                            .is_none_or(|button| state.buttons.contains(button));
                        let motion = self.motions.entry(*axis).or_default();
                        gyro(&self.profile, motion, state.imu).filter(|_| enabled)
                    }
                    _ => None,
                };
                outputs.extend(output);
                continue;
            }
            if let Target::Flick = target {
                let turn = self.turns.entry(*axis).or_default();
                let x = turn.turn(&self.profile, axis_value(*axis, state));
                if x != 0 {
                    outputs.push(Output::MouseMove(x, 0));
                }
                continue;
            }
            let before = self.axis(*axis, target, prev);
            let after = self.axis(*axis, target, state);
            if before != after
                && let Some(output) = after
            {
                outputs.push(output);
            }
        }
        outputs
    }

    /// Release everything still held, without the late tap of a chord hold, nor the switch of
    /// a held profile binding.
    pub fn release_all(&mut self) -> Vec<Output> {
        let mut outputs = Vec::new();
        self.chords.clear();
        self.rearm = true;
        self.motions.clear();
        self.turns.clear();
        for activator in std::mem::take(&mut self.pressed).into_values() {
            activator.release_all(&mut outputs);
        }
        for target in std::mem::take(&mut self.pulls).into_values() {
            emit(&target, false, &mut outputs);
        }
        for key in std::mem::take(&mut self.flicks).into_values() {
            outputs.push(Output::Key(key, false));
        }
        for (_, target) in std::mem::take(&mut self.flicked).into_values() {
            emit(&target, false, &mut outputs);
        }
        self.switch_layers(&mut outputs);
        outputs
    }

    /// `state` with each stick centred whose directions a held chord binds, so pushing it for
    /// the chord does not also drive the stick's own target.
    fn unclaimed(
        &self,
        state: &ControllerState,
    ) -> ControllerState {
        let mut state = *state;
        for (index, _) in &self.chords {
            let buttons = &self.profile.chords[*index].buttons;
            let [left, right] =
                Button::STICKS.map(|names| names.iter().any(|name| buttons.contains_key(name)));
            if left {
                state.lstick = Stick::default();
            }
            if right {
                state.rstick = Stick::default();
            }
        }
        state
    }

    /// Hold again each chord whose hold is down in `held`, as used so its release taps nothing.
    fn rearm_chords(
        &mut self,
        held: &ControllerState,
    ) {
        for (index, chord) in self.profile.chords.iter().enumerate() {
            let armed = self.chords.iter().any(|(armed, _)| *armed == index);
            if held.buttons.contains(chord.hold) && !armed {
                self.chords.push((index, true));
            }
        }
    }

    /// Apply and drop the layer outputs in `outputs`. An axis whose target the change replaces
    /// lets go of what it held.
    fn switch_layers(
        &mut self,
        outputs: &mut Vec<Output>,
    ) {
        let before = self.layers.clone();
        let layers = &mut self.layers;
        outputs.retain(|output| {
            let Output::Layer(switch, pressed) = output else {
                return true;
            };
            let name = switch.name();
            let active = layers.iter().position(|known| known == name);
            match (switch, pressed, active) {
                (_, true, None) => layers.push(name.to_string()),
                (LayerSwitch::Hold(_), false, Some(index))
                | (LayerSwitch::Toggle(_), true, Some(index)) => {
                    layers.remove(index);
                }
                _ => {}
            }
            false
        });
        if self.layers == before {
            return;
        }
        let profile = self.base.overlay(&self.layers);
        let replaced: Vec<(Axis, Target)> = self
            .profile
            .axes
            .iter()
            .filter(|(axis, target)| profile.axes.get(axis) != Some(target))
            .map(|(axis, target)| (*axis, target.clone()))
            .collect();
        for (axis, target) in replaced {
            self.release_axis(axis, &target, outputs);
        }
        self.profile = profile;
    }

    /// Let go of what `axis`, bound to `target`, holds, and centre it.
    fn release_axis(
        &mut self,
        axis: Axis,
        target: &Target,
        outputs: &mut Vec<Output>,
    ) {
        if let Some(key) = self.flicks.remove(&axis) {
            outputs.push(Output::Key(key, false));
        }
        if let Some((_, held)) = self.flicked.remove(&axis) {
            emit(&held, false, outputs);
        }
        if let Some(held) = self.pulls.remove(&axis) {
            emit(&held, false, outputs);
        }
        self.motions.remove(&axis);
        self.turns.remove(&axis);
        match *target {
            Target::Stick(stick) => outputs.push(Output::Stick(stick, 0, 0)),
            Target::Trigger(trigger) => outputs.push(Output::Trigger(trigger, 0)),
            _ => {}
        }
    }

    /// Swap the profile, returning the releases of everything the old one held and centred axes.
    pub fn activate(
        &mut self,
        profile: Profile,
    ) -> Vec<Output> {
        let outputs = self.hand_off();
        self.layers.clear();
        self.base = profile.clone();
        self.profile = profile;
        outputs
    }

    /// Release everything held and centre the bound sticks and triggers, keeping the profile
    /// and its toggled layers: for handing the controller to the menu or keyboard and back.
    pub fn hand_off(&mut self) -> Vec<Output> {
        let mut outputs = self.release_all();
        for target in self.profile.axes.values() {
            match *target {
                Target::Stick(stick) => outputs.push(Output::Stick(stick, 0, 0)),
                Target::Trigger(trigger) => outputs.push(Output::Trigger(trigger, 0)),
                _ => {}
            }
        }
        outputs
    }

    /// Swap in a profile that differs only in its knobs: nothing is released.
    pub fn retune(
        &mut self,
        profile: Profile,
    ) {
        self.layers.retain(|name| profile.layers.contains_key(name));
        self.profile = profile.overlay(&self.layers);
        self.base = profile;
    }

    fn press(
        &mut self,
        button: Button,
        now: Instant,
        outputs: &mut Vec<Output>,
    ) {
        // A second press inside the double window
        if let Some(activator) = self.pressed.get_mut(&button) {
            activator.again(now, outputs);
            return;
        }
        // The newest held chord with this member takes it
        let chords = &self.profile.chords;
        let member = self.chords.iter_mut().rev().find_map(|(index, used)| {
            let binding = chords[*index].buttons.get(&button)?;
            *used = true;
            Some(binding.clone())
        });
        if let Some(binding) = member {
            let activator = Activator::press(binding, now, outputs);
            self.pressed.insert(button, activator);
            return;
        }
        // Chord holds emit nothing until released
        if let Some(index) = chords.iter().position(|chord| chord.hold == button) {
            self.chords.push((index, false));
            let activator = Activator::press(Binding::from(Target::None), now, outputs);
            self.pressed.insert(button, activator);
            return;
        }
        let binding = self
            .profile
            .buttons
            .get(&button)
            .cloned()
            .unwrap_or(Binding::from(Target::None));
        let activator = Activator::press(binding, now, outputs);
        self.pressed.insert(button, activator);
    }

    fn release(
        &mut self,
        button: Button,
        now: Instant,
        outputs: &mut Vec<Output>,
    ) {
        let activator = self.pressed.remove(&button);
        let chords = &self.profile.chords;
        if let Some(position) = self
            .chords
            .iter()
            .position(|(index, _)| chords[*index].hold == button)
        {
            let (_, used) = self.chords.remove(position);
            // Tapped alone: the hold's own button, late
            if !used && let Some(binding) = self.profile.buttons.get(&button) {
                tap(&binding.press, outputs);
            }
            return;
        }
        let Some(mut activator) = activator else {
            return;
        };
        if !activator.release(now, outputs) {
            self.pressed.insert(button, activator);
        }
    }

    fn axis(
        &self,
        axis: Axis,
        target: &Target,
        state: &ControllerState,
    ) -> Option<Output> {
        match *target {
            Target::Stick(stick) => {
                if axis.is_trigger() {
                    return None;
                }
                let (x, y) = axis_value(axis, state);
                let (x, y) = deadzone_stick(x, y, self.profile.deadzone_stick);
                Some(Output::Stick(stick, x, y.saturating_neg()))
            }
            Target::Trigger(trigger) => {
                let value = match axis {
                    Axis::Lt => state.lt,
                    Axis::Rt => state.rt,
                    Axis::LStick | Axis::RStick | Axis::LPad | Axis::RPad | Axis::Gyro => {
                        return None;
                    }
                };
                Some(Output::Trigger(
                    trigger,
                    deadzone_trigger(value, self.profile.deadzone_trigger),
                ))
            }
            Target::Button(_)
            | Target::Key(_)
            | Target::Combo(_)
            | Target::Keys(_)
            | Target::Flicks(_)
            | Target::Desktop(_)
            | Target::Mouse(_)
            | Target::Cursor
            | Target::Scroll
            | Target::Flick
            | Target::Profile(_)
            | Target::Layer(_)
            | Target::Menu(_)
            | Target::Keyboard(_)
            | Target::Lizard(_)
            | Target::TvOff
            | Target::None => None,
        }
    }
}

/// Cursor or scroll output of a trackpad: its finger's delta since the last report, scaled.
fn motion(
    profile: &Profile,
    motions: &mut BTreeMap<Axis, Motion>,
    axis: Axis,
    target: &Target,
    pad: Trackpad,
) -> Option<Output> {
    if !pad.touched {
        motions.remove(&axis);
        return None;
    }
    let motion = motions.entry(axis).or_default();
    // Y up on the pad
    let (dx, dy) = motion.delta(pad, profile.mouse_release_guard, profile.mouse_smoothing)?;
    let output = match target {
        Target::Cursor => {
            let gain = CURSOR_GAIN * profile.mouse_speed * accelerate(dx, dy, profile.mouse_accel);
            let (x, y) = motion.whole(dx * gain, -dy * gain);
            Output::MouseMove(x, y)
        }
        Target::Scroll => {
            let gain = SCROLL_GAIN * profile.scroll_speed;
            let gain = if profile.scroll_natural { -gain } else { gain };
            let (dx, dy) = motion.scroll.filter(dx, dy);
            let (x, y) = motion.whole(dx * gain, dy * gain);
            Output::Scroll(x, y)
        }
        _ => return None,
    };
    if output == Output::MouseMove(0, 0) || output == Output::Scroll(0, 0) {
        return None;
    }
    Some(output)
}

/// Cursor output of the gyro: its angular rate over one report, in degrees, times
/// [`GYRO_GAIN`]. Turning right moves right, tilting the top away moves up (signs unverified).
fn gyro(
    profile: &Profile,
    motion: &mut Motion,
    imu: Imu,
) -> Option<Output> {
    // Report order X, Z, Y: pitch is X, yaw is Y (`steam_ibex_imu_mappings` in hid-steam)
    let [pitch, _, yaw] = imu.gyro;
    let degrees = |rate: i16| {
        let dps = rate as f32 / GYRO_UNITS_PER_DPS;
        match dps.abs() < GYRO_NOISE_DPS {
            true => 0.0,
            false => dps / REPORT_RATE,
        }
    };
    let gain = GYRO_GAIN * profile.gyro_speed;
    let (x, y) = motion.whole(-degrees(yaw) * gain, -degrees(pitch) * gain);
    if (x, y) == (0, 0) {
        return None;
    }
    Some(Output::MouseMove(x, y))
}

/// Cursor or scroll output of a stick: its deflection past the deadzone, curved, as a speed
/// integrated over one report.
fn velocity(
    profile: &Profile,
    motion: &mut Motion,
    target: &Target,
    (x, y): (i16, i16),
) -> Option<Output> {
    let (x, y) = (x as f32 / i16::MAX as f32, y as f32 / i16::MAX as f32);
    let length = (x.powi(2) + y.powi(2)).sqrt();
    let deadzone = profile.deadzone_stick;
    if length < deadzone || length == 0.0 {
        return None;
    }
    let reach = ((length.min(1.0) - deadzone) / (1.0 - deadzone)).clamp(0.0, 1.0);
    // Units per report along the stick's direction, Y up on the stick
    let scale = reach.powf(profile.mouse_stick_curve) / length / REPORT_RATE;
    let output = match target {
        Target::Cursor => {
            let gain = profile.mouse_stick_speed * scale;
            let (x, y) = motion.whole(x * gain, -y * gain);
            Output::MouseMove(x, y)
        }
        Target::Scroll => {
            let gain = profile.scroll_stick_speed * NOTCH as f32 * scale;
            let (x, y) = motion.whole(x * gain, y * gain);
            Output::Scroll(x, y)
        }
        _ => return None,
    };
    if output == Output::MouseMove(0, 0) || output == Output::Scroll(0, 0) {
        return None;
    }
    Some(output)
}

/// One flick stick: the stick's angle last report while out of the deadzone, the first turn
/// still being eased in, and the fraction of a pixel not yet sent.
#[derive(Clone, Debug, Default)]
struct Turn {
    angle: Option<f32>,
    /// Pixels of the flick and reports of it done.
    flick: Option<(f32, u32)>,
    rest: f32,
}
impl Turn {
    /// Pixels the cursor turns this report: leaving the deadzone starts a flick to the stick's
    /// angle, spread over `mouse.flick_ms`; held, the stick's rotation turns it as is.
    fn turn(
        &mut self,
        profile: &Profile,
        (x, y): (i16, i16),
    ) -> i32 {
        let pixels = profile.mouse_flick_degrees / std::f32::consts::TAU;
        let radius = profile.deadzone_stick * i16::MAX as f32;
        let mut moved = 0.0;
        if ((x as f32).powi(2) + (y as f32).powi(2)).sqrt() < radius.max(1.0) {
            self.angle = None;
        } else {
            let angle = stick_angle(x, y);
            match self.angle.replace(angle) {
                Some(last) => moved += angle_delta(last, angle) * pixels,
                None => self.flick = Some((angle * pixels, 0)),
            }
        }
        if let Some((flick, done)) = self.flick {
            let steps = (profile.mouse_flick_ms as f32 * REPORT_RATE / 1000.0)
                .round()
                .max(1.0);
            let done = done + 1;
            moved += flick * (ease(done as f32 / steps) - ease((done - 1) as f32 / steps));
            self.flick = (done < steps as u32).then_some((flick, done));
        }
        let moved = self.rest + moved;
        self.rest = moved.fract();
        moved.trunc() as i32
    }
}

/// Angle of a stick in radians from straight up, clockwise positive, within -π to π.
fn stick_angle(
    x: i16,
    y: i16,
) -> f32 {
    (x as f32).atan2(y as f32)
}

/// The shortest rotation from `from` to `to`, within -π to π, so crossing straight down turns
/// a little rather than nearly a full turn back.
fn angle_delta(
    from: f32,
    to: f32,
) -> f32 {
    use std::f32::consts::PI;
    use std::f32::consts::TAU;
    let delta = (to - from).rem_euclid(TAU);
    if delta > PI { delta - TAU } else { delta }
}

/// Ease-out over 0 to 1: fast start, soft landing.
fn ease(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(2)
}

/// Where a button's [`Activator`] is between its press and its last output.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum Phase {
    /// Down, `press` held back for `long` or `double`.
    Waiting,
    /// Down, `press` held; with turbo, `on` says whether it is down right now.
    Pressed { on: bool, toggle: Option<Instant> },
    /// Down, `long` held.
    Long,
    /// Down for the second time, `double` held.
    Double,
    /// Released short, waiting out the double window before `press` taps.
    Tapped,
}

/// One button's activators: Steam's Full, Start, Release, Long and Double Press and turbo.
#[derive(Clone, Debug)]
struct Activator {
    binding: Binding,
    /// When the button last went down.
    since: Instant,
    phase: Phase,
    /// The `warn_ms` buzz has played for this press.
    warned: bool,
}
impl Activator {
    fn press(
        binding: Binding,
        now: Instant,
        outputs: &mut Vec<Output>,
    ) -> Self {
        tap(&binding.start, outputs);
        let mut activator = Self {
            binding,
            since: now,
            phase: Phase::Waiting,
            warned: false,
        };
        let waits =
            activator.binding.long != Target::None || activator.binding.double != Target::None;
        if !waits {
            activator.hold(now, outputs);
        }
        activator
    }

    /// Down again while still owing output: a double inside the window, else a fresh press.
    fn again(
        &mut self,
        now: Instant,
        outputs: &mut Vec<Output>,
    ) {
        if self.phase != Phase::Tapped {
            return;
        }
        tap(&self.binding.start, outputs);
        self.since = now;
        self.phase = Phase::Double;
        emit(&self.binding.double, true, outputs);
    }

    fn hold(
        &mut self,
        now: Instant,
        outputs: &mut Vec<Output>,
    ) {
        let toggle = self.binding.turbo_ms.map(|ms| now + half(ms));
        self.phase = Phase::Pressed { on: true, toggle };
        emit(&self.binding.press, true, outputs);
    }

    /// The release edge; true when nothing more is owed.
    fn release(
        &mut self,
        now: Instant,
        outputs: &mut Vec<Output>,
    ) -> bool {
        tap(&self.binding.release, outputs);
        match self.phase {
            Phase::Pressed { on, .. } => {
                if on {
                    emit(&self.binding.press, false, outputs);
                }
            }
            Phase::Long => emit(&self.binding.long, false, outputs),
            Phase::Double => emit(&self.binding.double, false, outputs),
            Phase::Waiting
                if self.binding.double != Target::None
                    && now < self.since + ms(self.binding.double_ms) =>
            {
                self.phase = Phase::Tapped;
                return false;
            }
            Phase::Waiting => tap(&self.binding.press, outputs),
            Phase::Tapped => {}
        }
        true
    }

    /// Timers at `now`, with the button `down` in this report (a release in it cancels the
    /// warning); true when nothing more is owed.
    fn tick(
        &mut self,
        now: Instant,
        down: bool,
        outputs: &mut Vec<Output>,
    ) -> bool {
        let binding = &self.binding;
        if let Some(warn_ms) = binding.warn_ms
            && down
            && self.phase == Phase::Waiting
            && binding.long != Target::None
            && !self.warned
            && now + ms(warn_ms) >= self.since + ms(binding.long_ms)
        {
            self.warned = true;
            outputs.push(Output::Haptic {
                side: Side::Right,
                strength: u8::MAX,
                ms: WARN_BUZZ_MS,
            });
            outputs.push(WARN_BEEP);
        }
        match self.phase {
            Phase::Waiting
                if binding.long != Target::None && now >= self.since + ms(binding.long_ms) =>
            {
                self.phase = Phase::Long;
                emit(&binding.long, true, outputs);
            }
            // Held past the double window with nothing to wait for but release
            Phase::Waiting
                if binding.long == Target::None && now >= self.since + ms(binding.double_ms) =>
            {
                self.hold(now, outputs);
            }
            Phase::Tapped if now >= self.since + ms(binding.double_ms) => {
                tap(&binding.press, outputs);
                return true;
            }
            Phase::Pressed {
                on,
                toggle: Some(at),
            } if now >= at => {
                let half = half(binding.turbo_ms.unwrap());
                // Catch up without bursts if reports stalled
                let next = if now >= at + half {
                    now + half
                } else {
                    at + half
                };
                self.phase = Phase::Pressed {
                    on: !on,
                    toggle: Some(next),
                };
                emit(&binding.press, !on, outputs);
            }
            _ => {}
        }
        false
    }

    /// Release whatever is held, dropping pending timers. A held profile switch is dropped, not
    /// fired.
    fn release_all(
        &self,
        outputs: &mut Vec<Output>,
    ) {
        let held = match self.phase {
            Phase::Pressed { on: true, .. } => &self.binding.press,
            Phase::Long => &self.binding.long,
            Phase::Double => &self.binding.double,
            Phase::Pressed { on: false, .. } | Phase::Waiting | Phase::Tapped => return,
        };
        if !matches!(held, Target::Profile(_)) {
            emit(held, false, outputs);
        }
    }
}

fn ms(ms: u32) -> Duration {
    Duration::from_millis(ms as u64)
}

/// Half a turbo period: `press` is down for one, up for the other.
fn half(turbo_ms: u32) -> Duration {
    ms(turbo_ms) / 2
}

/// A press and release of `target` at once.
fn tap(
    target: &Target,
    outputs: &mut Vec<Output>,
) {
    emit(target, true, outputs);
    emit(target, false, outputs);
}

fn emit(
    target: &Target,
    pressed: bool,
    outputs: &mut Vec<Output>,
) {
    match *target {
        Target::Button(button) => outputs.push(Output::Button(button, pressed)),
        Target::Key(code) => outputs.push(Output::Key(code, pressed)),
        Target::Combo(ref codes) if pressed => {
            outputs.extend(codes.iter().map(|code| Output::Key(*code, true)));
        }
        Target::Combo(ref codes) => {
            outputs.extend(codes.iter().rev().map(|code| Output::Key(*code, false)));
        }
        Target::Desktop(ref action) if pressed => outputs.push(Output::Desktop(action.clone())),
        Target::Trigger(trigger) => outputs.push(Output::Trigger(trigger, pressed as u8 * 255)),
        Target::Mouse(button) => outputs.push(Output::MouseButton(button, pressed)),
        Target::Profile(ref switch) if !pressed => outputs.push(Output::Profile(switch.clone())),
        Target::Profile(_) => {}
        Target::Layer(ref switch) => outputs.push(Output::Layer(switch.clone(), pressed)),
        Target::Menu(command) if pressed => outputs.push(Output::Menu(command)),
        Target::Keyboard(command) if pressed => outputs.push(Output::Keyboard(command)),
        Target::Lizard(switch) => outputs.push(Output::Lizard(switch, pressed)),
        Target::TvOff if pressed => outputs.push(Output::TvOff),
        Target::TvOff => {}
        Target::Stick(_)
        | Target::Keys(_)
        | Target::Flicks(_)
        | Target::Desktop(_)
        | Target::Cursor
        | Target::Scroll
        | Target::Flick
        | Target::Menu(_)
        | Target::Keyboard(_)
        | Target::None => {}
    }
}

/// A press and release of a button-like `target`, as a menu item fires it.
pub fn tap_outputs(target: &Target) -> Vec<Output> {
    let mut outputs = Vec::new();
    tap(target, &mut outputs);
    outputs
}

/// X and Y of a stick or trackpad, Y up positive; triggers are (0, 0).
fn axis_value(
    axis: Axis,
    state: &ControllerState,
) -> (i16, i16) {
    match axis {
        Axis::LStick => stick_value(state.lstick),
        Axis::RStick => stick_value(state.rstick),
        Axis::LPad => pad_value(state.lpad),
        Axis::RPad => pad_value(state.rpad),
        Axis::Lt | Axis::Rt | Axis::Gyro => (0, 0),
    }
}

/// The virtual stick buttons `state` presses, each stick past [`stick_direction`]'s push;
/// `held`, the last report's, keep their direction down to its release.
fn directions(
    state: &ControllerState,
    held: Buttons,
) -> Buttons {
    let sticks = [state.lstick, state.rstick];
    sticks
        .into_iter()
        .zip(Button::STICKS)
        .fold(Buttons::empty(), |buttons, (stick, names)| {
            let [up, down, left, right] = names;
            let to = |direction| match direction {
                Direction::Up => up,
                Direction::Down => down,
                Direction::Left => left,
                Direction::Right => right,
            };
            let was = [
                Direction::Up,
                Direction::Down,
                Direction::Left,
                Direction::Right,
            ]
            .into_iter()
            .find(|direction| held.contains(to(*direction)));
            match stick_direction(stick, was) {
                Some(direction) => buttons.with(to(direction)),
                None => buttons,
            }
        })
}

/// `buttons` with its stick directions replaced by those of `directions`.
fn with_directions(
    buttons: Buttons,
    directions: Buttons,
) -> Buttons {
    Button::STICKS
        .into_iter()
        .flatten()
        .fold(buttons, |buttons, button| {
            match directions.contains(button) {
                true => buttons.with(button),
                false => buttons.without(button),
            }
        })
}

/// The key a flick holds: past the deadzone, the dominant direction. A held key stays until the
/// stick drops below [`FLICK_RELEASE`] of the deadzone, so noise at the edge sends one key.
fn flick(
    (x, y): (i16, i16),
    [up, down, left, right]: [u16; 4],
    deadzone: f32,
    held: Option<u16>,
) -> Option<u16> {
    let radius = deadzone * i16::MAX as f32;
    let length = ((x as f32).powi(2) + (y as f32).powi(2)).sqrt();
    if length < radius * FLICK_RELEASE {
        return None;
    }
    if length < radius {
        return held;
    }
    let key = match (x.unsigned_abs() > y.unsigned_abs(), x > 0, y > 0) {
        (true, true, _) => right,
        (true, false, _) => left,
        (false, _, true) => up,
        (false, _, false) => down,
    };
    Some(key)
}

/// Whether a trigger bound to a button holds it: pressed past `click` of its travel, released
/// below [`CLICK_RELEASE`] of that, so a pull resting on the edge sends one press.
fn pull(
    value: u16,
    click: f32,
    held: bool,
) -> bool {
    let travel = value.min(i16::MAX as u16) as f32 / i16::MAX as f32;
    if held {
        return travel >= click * CLICK_RELEASE;
    }
    travel >= click
}

/// One finger on one trackpad: its smoothing filters, where it was last report after them,
/// the deltas still held back by the release guard, and the fraction of a pixel or wheel unit
/// not yet sent, so slow motion adds up instead of rounding away.
#[derive(Clone, Debug, Default)]
struct Motion {
    filter: [OneEuro; 2],
    last: Option<(f32, f32)>,
    queue: VecDeque<(f32, f32)>,
    rest: (f32, f32),
    /// Pad units moved since the last `haptics.pad_ticks` tick.
    travel: f32,
    /// Axis lock and smoothing of a pad bound to scroll.
    scroll: ScrollFilter,
}
impl Motion {
    /// Pad units moved `guard` reports ago; none on the first touched report, so a finger
    /// landing elsewhere does not jump, nor while the guard fills.
    fn delta(
        &mut self,
        pad: Trackpad,
        guard: u8,
        smoothing: f32,
    ) -> Option<(f32, f32)> {
        let [fx, fy] = &mut self.filter;
        let at = (
            fx.filter(pad.x as f32, smoothing),
            fy.filter(pad.y as f32, smoothing),
        );
        let (x, y) = self.last.replace(at)?;
        self.queue.push_back((at.0 - x, at.1 - y));
        // A guard lowered mid-touch drops the surplus
        while self.queue.len() > guard as usize + 1 {
            self.queue.pop_front();
        }
        if self.queue.len() <= guard as usize {
            return None;
        }
        let delta = self.queue.pop_front()?;
        self.travel += delta.0.hypot(delta.1);
        Some(delta)
    }

    /// Whether the finger has travelled another [`PAD_TICK_UNITS`] since the last tick.
    fn tick(&mut self) -> bool {
        if self.travel < PAD_TICK_UNITS {
            return false;
        }
        self.travel %= PAD_TICK_UNITS;
        true
    }

    /// The whole units of `rest` plus the move, keeping the fraction.
    fn whole(
        &mut self,
        x: f32,
        y: f32,
    ) -> (i32, i32) {
        let x = self.rest.0 + x;
        let y = self.rest.1 + y;
        self.rest = (x.fract(), y.fract());
        (x.trunc() as i32, y.trunc() as i32)
    }
}

/// Scroll of one touch: still until the finger is [`SCROLL_START`] from where it landed, so a resting thumb
/// does not creep; then locked to the dominant axis, switching only once the other one is
/// [`SCROLL_SWITCH`] times faster, so a diagonal wobble does not scroll sideways; and eased by
/// an exponential filter at [`SCROLL_CUTOFF`], so uneven reports do not stutter.
#[derive(Clone, Debug, Default)]
struct ScrollFilter {
    /// Where the finger is from where it landed, until it is [`SCROLL_START`] away.
    moved: (f32, f32),
    /// Whether the lock is vertical, none until the finger starts.
    vertical: Option<bool>,
    /// Smoothed speed of each axis, in pad units a report.
    speed: (f32, f32),
    /// The last output.
    last: (f32, f32),
}
impl ScrollFilter {
    fn filter(
        &mut self,
        dx: f32,
        dy: f32,
    ) -> (f32, f32) {
        if self.vertical.is_none() {
            self.moved = (self.moved.0 + dx, self.moved.1 + dy);
            if self.moved.0.hypot(self.moved.1) < SCROLL_START {
                return (0.0, 0.0);
            }
        }
        let weight = alpha(SCROLL_CUTOFF);
        self.speed = (
            lerp(self.speed.0, dx.abs(), weight),
            lerp(self.speed.1, dy.abs(), weight),
        );
        let (sx, sy) = self.speed;
        let vertical = match self.vertical {
            None => dy.abs() >= dx.abs(),
            Some(true) => sx <= sy * SCROLL_SWITCH,
            Some(false) => sy > sx * SCROLL_SWITCH,
        };
        if self.vertical != Some(vertical) {
            self.last = (0.0, 0.0);
        }
        self.vertical = Some(vertical);
        let (dx, dy) = if vertical { (0.0, dy) } else { (dx, 0.0) };
        self.last = (lerp(self.last.0, dx, weight), lerp(self.last.1, dy, weight));
        self.last
    }
}

/// 1€ filter (Casiez, Roussel, Vogel, CHI 2012) of one pad coordinate: exponential smoothing
/// whose cutoff rises with the smoothed speed, so a resting finger's jitter is damped while a
/// swipe passes nearly as is.
#[derive(Clone, Debug, Default)]
struct OneEuro {
    /// The last output and the smoothed speed in pad units a second.
    last: Option<(f32, f32)>,
}
impl OneEuro {
    /// `smoothing` 1 rests at [`SMOOTHING_MIN_CUTOFF`] (1 Hz, still) and adds
    /// [`SMOOTHING_BETA`] Hz per unit/s: a slow 20 unit/report drift (2000 unit/s) stays near
    /// 5 Hz, damping jitter, while a 2000 unit/report flick (200000 unit/s) is past 100 Hz after
    /// one report. Both scale by `1 / smoothing`, so 0.5 is 2 Hz and 0.004, and 0 is off.
    fn filter(
        &mut self,
        x: f32,
        smoothing: f32,
    ) -> f32 {
        let Some((last, speed)) = self.last else {
            self.last = Some((x, 0.0));
            return x;
        };
        if smoothing <= 0.0 {
            self.last = Some((x, 0.0));
            return x;
        }
        let speed = lerp(speed, (x - last) * REPORT_RATE, alpha(SMOOTHING_D_CUTOFF));
        let cutoff = (SMOOTHING_MIN_CUTOFF + SMOOTHING_BETA * speed.abs()) / smoothing;
        let x = lerp(last, x, alpha(cutoff));
        self.last = Some((x, speed));
        x
    }
}

/// Weight of the new sample of an exponential filter with `cutoff` Hz at [`REPORT_RATE`].
fn alpha(cutoff: f32) -> f32 {
    let tau = 1.0 / (2.0 * std::f32::consts::PI * cutoff);
    1.0 / (1.0 + tau * REPORT_RATE)
}

fn lerp(
    from: f32,
    to: f32,
    weight: f32,
) -> f32 {
    from + (to - from) * weight
}

/// Gain of a swipe: 1 when slow, up to `1 + accel` at [`ACCEL_FULL`] units per report.
fn accelerate(
    dx: f32,
    dy: f32,
    accel: f32,
) -> f32 {
    let speed = (dx.powi(2) + dy.powi(2)).sqrt() / ACCEL_FULL;
    1.0 + accel * speed.min(1.0)
}

fn stick_value(stick: Stick) -> (i16, i16) {
    (stick.x, stick.y)
}

fn pad_value(pad: Trackpad) -> (i16, i16) {
    if !pad.touched {
        return (0, 0);
    }
    (pad.x, pad.y)
}

fn deadzone_stick(
    x: i16,
    y: i16,
    deadzone: f32,
) -> (i16, i16) {
    let radius = deadzone * i16::MAX as f32;
    let length = ((x as f32).powi(2) + (y as f32).powi(2)).sqrt();
    if length < radius {
        return (0, 0);
    }
    (x, y)
}

fn deadzone_trigger(
    value: u16,
    deadzone: f32,
) -> u8 {
    let value = value.min(i16::MAX as u16);
    if (value as f32) < deadzone * i16::MAX as f32 {
        return 0;
    }
    (value as u32 * 255 / i16::MAX as u32) as u8
}

#[cfg(test)]
mod test {
    use super::*;

    const SETTOP: &str = r#"
        [daemon]
        active = "pad"

        [profile.pad.buttons]
        steam = "key:KEY_HOMEPAGE"
        quick = "menu:toggle"

        [[profile.pad.chord]]
        hold = "view"
        dpad_up = "key:KEY_VOLUMEUP"
        dpad_down = "key:KEY_VOLUMEDOWN"
        dpad_left = "key:KEY_MUTE"

        [[profile.pad.chord]]
        hold = "steam"
        y = "profile:desktop"

        [profile.desktop]
    "#;

    fn pressed(buttons: &[Button]) -> ControllerState {
        let mut state = ControllerState::default();
        for button in buttons {
            state.buttons = state.buttons.with(*button);
        }
        state
    }

    /// `profile` without click pulses, for tests about something else.
    fn quiet(mut profile: Profile) -> Profile {
        profile.haptics_click = 0.0;
        profile
    }

    fn settop() -> Mapper {
        Mapper::new(quiet(Config::parse(SETTOP).unwrap().profile().unwrap()))
    }

    #[test]
    fn test_mapper_press() {
        let mut mapper = Mapper::new(Profile::xbox());
        let idle = ControllerState::default();
        let a = pressed(&[Button::A]);
        assert_eq!(
            mapper.map(&idle, &a),
            vec![Output::Button(PadButton::South, true)]
        );
        assert_eq!(mapper.map(&a, &a), vec![]);
        assert_eq!(
            mapper.map(&a, &idle),
            vec![Output::Button(PadButton::South, false)]
        );
    }

    #[test]
    fn test_mapper_chord() {
        let mut mapper = settop();
        let idle = ControllerState::default();
        let view = pressed(&[Button::View]);
        let view_up = pressed(&[Button::View, Button::DpadUp]);
        let volumeup = evdev::KeyCode::KEY_VOLUMEUP.0;
        assert_eq!(mapper.map(&idle, &view), vec![]);
        assert_eq!(
            mapper.map(&view, &view_up),
            vec![Output::Key(volumeup, true)]
        );
        assert_eq!(
            mapper.map(&view_up, &view),
            vec![Output::Key(volumeup, false)]
        );
        assert_eq!(mapper.map(&view, &idle), vec![]);
        // Outside the chord dpad_up is the hat again
        let up = pressed(&[Button::DpadUp]);
        assert_eq!(
            mapper.map(&idle, &up),
            vec![Output::Button(PadButton::DpadUp, true)]
        );
    }

    #[test]
    fn test_mapper_chord_tap() {
        let mut mapper = settop();
        let idle = ControllerState::default();
        let view = pressed(&[Button::View]);
        assert_eq!(mapper.map(&idle, &view), vec![]);
        assert_eq!(
            mapper.map(&view, &idle),
            vec![
                Output::Button(PadButton::Select, true),
                Output::Button(PadButton::Select, false),
            ]
        );
    }

    #[test]
    fn test_mapper_release_all() {
        let mut mapper = settop();
        let idle = ControllerState::default();
        let view_up = pressed(&[Button::View, Button::DpadUp]);
        mapper.map(&idle, &pressed(&[Button::View]));
        mapper.map(&pressed(&[Button::View]), &view_up);
        assert_eq!(
            mapper.release_all(),
            vec![Output::Key(evdev::KeyCode::KEY_VOLUMEUP.0, false)]
        );
        mapper.map(&idle, &pressed(&[Button::View]));
        assert_eq!(mapper.release_all(), vec![]);
    }

    #[test]
    fn test_mapper_steam_key() {
        let mut mapper = settop();
        let idle = ControllerState::default();
        let steam = pressed(&[Button::Steam]);
        let home = evdev::KeyCode::KEY_HOMEPAGE.0;
        // Steam holds a chord, so its own key taps on release
        assert_eq!(mapper.map(&idle, &steam), vec![]);
        assert_eq!(
            mapper.map(&steam, &idle),
            vec![Output::Key(home, true), Output::Key(home, false)]
        );
    }

    #[test]
    fn test_mapper_stick() {
        let mut mapper = Mapper::new(Profile::xbox());
        let idle = ControllerState::default();
        let mut moved = idle;
        moved.lstick = Stick { x: 0, y: 20000 };
        moved.buttons = moved.buttons.with(Button::LStickTouch);
        moved.rt = 32767;
        assert_eq!(
            mapper.map(&idle, &moved),
            vec![
                Output::Stick(PadStick::Left, 0, -20000),
                Output::Trigger(PadTrigger::Rt, 255),
            ]
        );
        // Inside the deadzone nothing moves
        let mut drift = idle;
        drift.lstick = Stick { x: 0x0139, y: -123 };
        drift.buttons = drift.buttons.with(Button::LStickTouch);
        assert_eq!(mapper.map(&idle, &drift), vec![]);
    }

    /// A stick nobody touches is centred, after a debounce; `stick.touch = false` reads it raw.
    #[test]
    fn test_mapper_stick_touch() {
        let mut mapper = Mapper::new(Profile::xbox());
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        let idle = ControllerState::default();
        let mut held = idle;
        held.lstick = Stick { x: 0, y: 20000 };
        held.buttons = held.buttons.with(Button::LStickTouch);
        let mut untouched = held;
        untouched.buttons = idle.buttons;
        let out = Output::Stick(PadStick::Left, 0, -20000);
        let centre = Output::Stick(PadStick::Left, 0, 0);
        assert_eq!(mapper.map_at(&idle, &untouched, at(0)), vec![]);
        assert_eq!(mapper.map_at(&untouched, &untouched, at(100)), vec![]);
        assert_eq!(mapper.map_at(&untouched, &held, at(104)), vec![out.clone()]);
        // A dropout shorter than the debounce keeps the stick where it is
        assert_eq!(mapper.map_at(&held, &untouched, at(108)), vec![]);
        assert_eq!(mapper.map_at(&untouched, &held, at(112)), vec![]);
        assert_eq!(mapper.map_at(&held, &untouched, at(116)), vec![]);
        assert_eq!(mapper.map_at(&untouched, &untouched, at(266)), vec![centre]);
        // A click without touch keeps the click
        let mut clicked = untouched;
        clicked.buttons = clicked.buttons.with(Button::LStickClick);
        assert_eq!(
            mapper.map_at(&untouched, &clicked, at(300)),
            vec![Output::Button(PadButton::ThumbL, true)]
        );

        let text = "[profile.p]\nstick.touch = false\n";
        let mut raw = Mapper::new(Config::parse(text).unwrap().profile().unwrap());
        assert_eq!(raw.map_at(&idle, &untouched, at(0)), vec![out]);
    }

    /// A thumb settling on the cap, as captured on settop over BLE (2026-11-05): the touch bit
    /// drops out in runs of 13 to 77 reports (0.1 to 0.6 s at 133 Hz) before it holds. Only a
    /// lift past the debounce centres the stick.
    #[test]
    fn test_mapper_stick_touch_flicker() {
        let mut mapper = Mapper::new(Profile::xbox());
        let start = Instant::now();
        let idle = ControllerState::default();
        let mut held = idle;
        held.lstick = Stick { x: 0, y: 20000 };
        held.buttons = held.buttons.with(Button::LStickTouch);
        let mut untouched = held;
        untouched.buttons = idle.buttons;
        let out = Output::Stick(PadStick::Left, 0, -20000);
        let centre = Output::Stick(PadStick::Left, 0, 0);
        // (touched, reports) runs from the capture at t=42.1 s
        let runs = [
            (true, 42),
            (false, 13),
            (true, 32),
            (false, 11),
            (true, 14),
            (false, 18),
            (true, 95),
            (false, 10),
            (true, 914),
        ];
        let mut report = 0_u64;
        let mut prev = idle;
        let mut outputs = Vec::new();
        for (touched, count) in runs {
            for _ in 0..count {
                let state = if touched { held } else { untouched };
                let now = start + Duration::from_millis(report * 1000 / 133);
                outputs.extend(mapper.map_at(&prev, &state, now));
                prev = state;
                report += 1;
            }
        }
        assert_eq!(outputs, vec![out]);
        // A lift of 20 reports (150 ms) centres it, the touch bit back on moves it again
        let mut outputs = Vec::new();
        for _ in 0..21 {
            let now = start + Duration::from_millis(report * 1000 / 133);
            outputs.extend(mapper.map_at(&prev, &untouched, now));
            prev = untouched;
            report += 1;
        }
        assert_eq!(outputs, vec![centre]);
    }

    /// Letting go of a flicked stick releases its key at once.
    #[test]
    fn test_mapper_flick_touch() {
        let config = Config::parse(include_str!("../../../examples/settop.toml")).unwrap();
        let mut mapper = Mapper::new(config.get("browser").unwrap());
        let start = Instant::now();
        let idle = ControllerState::default();
        let mut up = idle;
        up.lstick = Stick { x: 0, y: 20000 };
        up.buttons = up.buttons.with(Button::LStickTouch);
        let mut let_go = up;
        let_go.buttons = idle.buttons;
        let key_up = evdev::KeyCode::KEY_UP.0;
        assert_eq!(
            mapper.map_at(&idle, &up, start),
            vec![Output::Key(key_up, true)]
        );
        let later = start + TOUCH_DEBOUNCE;
        assert_eq!(
            mapper.map_at(&up, &let_go, later),
            vec![Output::Key(key_up, false)]
        );
        assert_eq!(
            mapper.map_at(&let_go, &let_go, later + TOUCH_DEBOUNCE),
            vec![]
        );
    }

    #[test]
    fn test_mapper_activate() {
        let mut mapper = Mapper::new(Profile::xbox());
        let idle = ControllerState::default();
        let a = pressed(&[Button::A]);
        let steam = pressed(&[Button::Steam]);
        mapper.map(&idle, &a);
        let outputs = mapper.activate(Config::parse(SETTOP).unwrap().profile().unwrap());
        assert_eq!(outputs[0], Output::Button(PadButton::South, false));
        assert!(outputs.contains(&Output::Stick(PadStick::Left, 0, 0)));
        assert!(outputs.contains(&Output::Trigger(PadTrigger::Rt, 0)));
        // Releasing A after the switch sends nothing, Steam is the new profile's key
        assert_eq!(mapper.map(&a, &idle), vec![]);
        let home = evdev::KeyCode::KEY_HOMEPAGE.0;
        assert_eq!(mapper.map(&idle, &steam), vec![]);
        assert_eq!(
            mapper.map(&steam, &idle),
            vec![Output::Key(home, true), Output::Key(home, false)]
        );
    }

    const LAYERS: &str = r#"
        [profile.l.buttons]
        lb = "layer:hold:aim"
        rb = "layer:toggle:aim"

        [profile.l.layer.aim.buttons]
        a = "key:KEY_A"

        [profile.l.layer.aim.axes]
        lstick = "keys:KEY_W,KEY_S,KEY_A,KEY_D"
    "#;

    fn layers() -> Mapper {
        Mapper::new(quiet(Config::parse(LAYERS).unwrap().get("l").unwrap()))
    }

    #[test]
    fn test_mapper_layer_hold() {
        let mut mapper = layers();
        let idle = ControllerState::default();
        let a = pressed(&[Button::A]);
        let lb = pressed(&[Button::Lb]);
        let lb_a = pressed(&[Button::Lb, Button::A]);
        let key_a = evdev::KeyCode::KEY_A.0;
        // The layer's press never leaves the mapper; the stick it replaces is centred
        assert_eq!(
            mapper.map(&idle, &lb),
            vec![Output::Stick(PadStick::Left, 0, 0)]
        );
        assert_eq!(mapper.map(&lb, &lb_a), vec![Output::Key(key_a, true)]);
        // Released under the layer it was pressed in, whatever is active then
        assert_eq!(mapper.map(&lb_a, &a), vec![]);
        assert_eq!(mapper.map(&a, &idle), vec![Output::Key(key_a, false)]);
        // Unset in the layer: the profile's binding
        assert_eq!(
            mapper.map(&idle, &a),
            vec![Output::Button(PadButton::South, true)]
        );
    }

    #[test]
    fn test_mapper_layer_toggle() {
        let mut mapper = layers();
        let idle = ControllerState::default();
        let a = pressed(&[Button::A]);
        let rb = pressed(&[Button::Rb]);
        let key_a = evdev::KeyCode::KEY_A.0;
        mapper.map(&idle, &rb);
        mapper.map(&rb, &idle);
        assert_eq!(mapper.map(&idle, &a), vec![Output::Key(key_a, true)]);
        mapper.map(&a, &idle);
        mapper.map(&idle, &rb);
        mapper.map(&rb, &idle);
        assert_eq!(
            mapper.map(&idle, &a),
            vec![Output::Button(PadButton::South, true)]
        );
    }

    #[test]
    fn test_mapper_layer_reset_on_activate() {
        let mut mapper = layers();
        let idle = ControllerState::default();
        let rb = pressed(&[Button::Rb]);
        let a = pressed(&[Button::A]);
        mapper.map(&idle, &rb);
        mapper.map(&rb, &idle);
        mapper.activate(quiet(Config::parse(LAYERS).unwrap().get("l").unwrap()));
        assert_eq!(
            mapper.map(&idle, &a),
            vec![Output::Button(PadButton::South, true)]
        );
    }

    #[test]
    fn test_mapper_layer_toggle_survives_hand_off() {
        let mut mapper = layers();
        let idle = ControllerState::default();
        let rb = pressed(&[Button::Rb]);
        let a = pressed(&[Button::A]);
        let lb = pressed(&[Button::Lb]);
        let key_a = evdev::KeyCode::KEY_A.0;
        mapper.map(&idle, &rb);
        mapper.map(&rb, &idle);
        // The menu takes the controller and gives it back: the toggled layer stays on
        mapper.hand_off();
        assert_eq!(mapper.map(&idle, &a), vec![Output::Key(key_a, true)]);
        mapper.map(&a, &idle);
        // A held layer is let go with its button
        mapper.map(&idle, &rb);
        mapper.map(&rb, &idle);
        mapper.map(&idle, &lb);
        mapper.hand_off();
        assert_eq!(
            mapper.map(&idle, &a),
            vec![Output::Button(PadButton::South, true)]
        );
    }

    #[test]
    fn test_config_layer_errors() {
        let unknown = "[profile.l.buttons]\nlb = \"layer:hold:nope\"\n";
        let err = Config::parse(unknown).unwrap_err().to_string();
        assert!(err.contains("buttons.lb: No layer named nope"), "{err}");
        assert!(Config::parse("[profile.l.buttons]\nlb = \"layer:nope\"\n").is_err());
        let axis = "[profile.l.layer.x.axes]\nlstick = \"layer:hold:x\"\n";
        assert!(Config::parse(axis).is_err());
        let target: Target = "layer:toggle:aim".parse().unwrap();
        assert_eq!(target.to_string(), "layer:toggle:aim");
    }

    const GYRO: &str = r#"
        [profile.g]
        gyro.button = "rpad_touch"
        gyro.speed = 2.0

        [profile.g.axes]
        gyro = "cursor"
    "#;

    /// `buttons` held and the gyro turning at `yaw` and `pitch` degrees a second.
    fn turning(
        buttons: &[Button],
        yaw: i16,
        pitch: i16,
    ) -> ControllerState {
        let mut state = pressed(buttons);
        state.imu.gyro = [pitch * 16, 0, yaw * 16];
        state
    }

    #[test]
    fn test_mapper_gyro() {
        let mut mapper = Mapper::new(Config::parse(GYRO).unwrap().get("g").unwrap());
        let idle = ControllerState::default();
        // Off until the enable button is held
        assert_eq!(mapper.map(&idle, &turning(&[], 250, 0)), vec![]);
        let touch = [Button::RPadTouch];
        // 250 °/s over one report at 250 Hz is 1°: 40 px at speed 1, times 2
        assert_eq!(
            mapper.map(&idle, &turning(&touch, 250, 0)),
            vec![Output::MouseMove(-80, 0)]
        );
        assert_eq!(
            mapper.map(&idle, &turning(&touch, 0, -125)),
            vec![Output::MouseMove(0, 40)]
        );
        // Below the noise floor nothing drifts
        let still = turning(&touch, 0, 0);
        let mut state = still;
        state.imu.gyro = [8, 0, -8];
        assert_eq!(mapper.map(&still, &state), vec![]);
    }

    #[test]
    fn test_config_gyro() {
        assert_eq!(Profile::xbox().axes.get(&Axis::Gyro), None);
        assert!(Config::parse("[profile.g.axes]\ngyro = \"pad:left\"\n").is_err());
        assert!(Config::parse("[profile.g]\ngyro.button = \"lstick\"\n").is_err());
        let always = Config::parse("[profile.g.axes]\ngyro = \"cursor\"\n").unwrap();
        assert_eq!(always.get("g").unwrap().gyro_button, None);
    }

    const MOUSE: &str = r#"
        [profile.m]
        mouse.release_guard = 0
        mouse.smoothing = 0.0

        [profile.m.buttons]
        a = "mouse:left"

        [profile.m.axes]
        rpad = "cursor"
        lpad = "scroll"
    "#;

    fn mouse() -> Mapper {
        Mapper::new(quiet(Config::parse(MOUSE).unwrap().profile().unwrap()))
    }

    fn rpad(
        x: i16,
        y: i16,
    ) -> ControllerState {
        ControllerState {
            rpad: Trackpad {
                x,
                y,
                touched: true,
                ..Trackpad::default()
            },
            ..ControllerState::default()
        }
    }

    fn lpad(
        x: i16,
        y: i16,
    ) -> ControllerState {
        ControllerState {
            lpad: Trackpad {
                x,
                y,
                touched: true,
                ..Trackpad::default()
            },
            ..ControllerState::default()
        }
    }

    #[test]
    fn test_haptics_config() {
        let profile = Profile::xbox();
        assert_eq!(
            (
                profile.haptics_click,
                profile.haptics_pad_ticks,
                profile.haptics_menu
            ),
            (0.3, 0.0, 0.4)
        );
        let config =
            Config::parse("[profile.x]\nhaptics = { click = 0, pad_ticks = 0.5 }").unwrap();
        let profile = config.get("x").unwrap();
        assert_eq!(profile.haptics_click, 0.0);
        assert_eq!(profile.haptics_pad_ticks, 0.5);
        assert_eq!(profile.haptics_menu, 0.4);
        assert!(Config::parse("[profile.x]\nhaptics.click = 1.5").is_err());
        assert!(Config::parse("[profile.x]\nhaptics.menu = -0.1").is_err());
        assert!(Config::parse("[profile.x]\nhaptics.rumble = 1").is_err());
    }

    #[test]
    fn test_mapper_click_pulse() {
        let text = "[profile.p.buttons]\na = \"mouse:left\"\nlb = \"key:KEY_A\"\nx = \"pad:south\"";
        let mut mapper = Mapper::new(Config::parse(text).unwrap().get("p").unwrap());
        let idle = ControllerState::default();
        let pulse = |side| Output::Haptic {
            side,
            strength: 77,
            ms: CLICK_MS,
        };
        assert_eq!(
            mapper.map(&idle, &pressed(&[Button::A])),
            vec![
                Output::MouseButton(MouseButton::Left, true),
                pulse(Side::Right)
            ]
        );
        // Release: nothing
        assert_eq!(
            mapper.map(&pressed(&[Button::A]), &idle),
            vec![Output::MouseButton(MouseButton::Left, false)]
        );
        assert_eq!(
            mapper.map(&idle, &pressed(&[Button::Lb])),
            vec![
                Output::Key(evdev::KeyCode::KEY_A.0, true),
                pulse(Side::Left)
            ]
        );
        // A pad button is not a click
        assert_eq!(
            mapper.map(&pressed(&[Button::Lb]), &pressed(&[Button::X])),
            vec![
                Output::Button(PadButton::South, true),
                Output::Key(evdev::KeyCode::KEY_A.0, false)
            ]
        );
    }

    #[test]
    fn test_mapper_pad_ticks() {
        let mut profile = quiet(Config::parse(MOUSE).unwrap().profile().unwrap());
        profile.haptics_pad_ticks = 1.0;
        let mut mapper = Mapper::new(profile);
        let mut prev = ControllerState::default();
        let mut ticks = Vec::new();
        // 500 units a report: a tick about every 4.1 reports, 65534 / 32 apart
        for x in (0..=10_000).step_by(500) {
            let state = rpad(x, 0);
            let outputs = mapper.map(&prev, &state);
            ticks.extend(
                outputs
                    .into_iter()
                    .filter(|output| matches!(output, Output::Haptic { .. })),
            );
            prev = state;
        }
        assert_eq!(ticks.len(), 4, "{ticks:?}");
        assert_eq!(
            ticks[0],
            Output::Haptic {
                side: Side::Right,
                strength: 255,
                ms: TICK_MS
            }
        );
        // Off by default
        let mut mapper = mouse();
        let mut prev = ControllerState::default();
        for x in (0..=10_000).step_by(500) {
            let state = rpad(x, 0);
            let outputs = mapper.map(&prev, &state);
            assert!(
                !outputs
                    .iter()
                    .any(|output| matches!(output, Output::Haptic { .. }))
            );
            prev = state;
        }
    }

    #[test]
    fn test_mapper_cursor() {
        let mut mapper = mouse();
        let idle = ControllerState::default();
        let mut prev = idle;
        let mut moves = Vec::new();
        for x in [0, 250, 500, 750, 1000] {
            let state = rpad(x, 0);
            moves.push(mapper.map(&prev, &state));
            prev = state;
        }
        // The first touched frame does not move
        assert_eq!(moves[0], vec![]);
        // 250 units * 1440 / 65534 * (1 + 1.3 * 250 / 2000) = 6.39 px a frame, fraction kept
        let xs: Vec<i32> = moves[1..]
            .iter()
            .map(|outputs| match outputs[..] {
                [Output::MouseMove(x, 0)] => x,
                _ => panic!(),
            })
            .collect();
        assert!(xs.iter().all(|x| (6..=7).contains(x)));
        assert_eq!(xs.iter().sum::<i32>(), 25);
        // Lifting and landing elsewhere does not jump
        assert_eq!(mapper.map(&prev, &idle), vec![]);
        assert_eq!(mapper.map(&idle, &rpad(-5000, -5000)), vec![]);
        // Pad Y up is screen Y down negative
        assert_eq!(
            mapper.map(&rpad(-5000, -5000), &rpad(-5000, -4750)),
            vec![Output::MouseMove(0, -6)]
        );
    }

    #[test]
    fn test_mapper_cursor_slow() {
        let mut mapper = mouse();
        let mut prev = rpad(0, 0);
        mapper.map(&ControllerState::default(), &prev);
        // 10 units a frame is under half a pixel, yet adds up
        let mut total = 0;
        for x in (10..=200).step_by(10) {
            let state = rpad(x, 0);
            for output in mapper.map(&prev, &state) {
                let Output::MouseMove(x, _) = output else {
                    panic!();
                };
                total += x;
            }
            prev = state;
        }
        assert_eq!(total, 4);
    }

    fn guarded(guard: u8) -> Mapper {
        let mut profile = Config::parse(MOUSE).unwrap().profile().unwrap();
        profile.mouse_release_guard = guard;
        Mapper::new(profile)
    }

    /// The X of every cursor move while the right pad follows `xs`, then lifts.
    fn trace(
        mapper: &mut Mapper,
        xs: &[i16],
    ) -> Vec<Vec<Output>> {
        let mut prev = ControllerState::default();
        let mut outputs = Vec::new();
        for x in xs {
            let state = rpad(*x, 0);
            outputs.push(mapper.map(&prev, &state));
            prev = state;
        }
        outputs.push(mapper.map(&prev, &ControllerState::default()));
        outputs
    }

    #[test]
    fn test_mapper_release_guard_drops_tail() {
        let mut mapper = guarded(3);
        // Steady 250 a report, then the lifting finger jumps 4000 three times
        let xs = [0, 250, 500, 750, 1000, 1250, 5250, 9250, 13250];
        let moves: Vec<Output> = trace(&mut mapper, &xs).concat();
        let total: i32 = moves
            .iter()
            .map(|output| match output {
                Output::MouseMove(x, 0) => *x,
                _ => panic!("{output:?}"),
            })
            .sum();
        // Only the five steady deltas, 6.39 px each
        assert_eq!(total, 31);
        // A fresh touch starts empty
        assert_eq!(trace(&mut mapper, &[0, 250, 500]).concat(), vec![]);
    }

    #[test]
    fn test_mapper_release_guard_delays() {
        let xs: Vec<i16> = (0..10).map(|i| i * 250).collect();
        let plain = trace(&mut guarded(0), &xs);
        for guard in [1, 3] {
            let delayed = trace(&mut guarded(guard), &xs);
            let n = guard as usize;
            assert!(delayed[..=n].iter().all(Vec::is_empty), "{delayed:?}");
            assert_eq!(delayed[n + 1..xs.len()], plain[1..xs.len() - n]);
            assert_eq!(delayed[xs.len()], vec![]);
        }
    }

    fn smoothed(smoothing: f32) -> Mapper {
        let mut profile = Config::parse(MOUSE).unwrap().profile().unwrap();
        profile.mouse_smoothing = smoothing;
        // Ten times the gain and no accel, so pad jitter shows in whole pixels
        profile.mouse_speed = 10.0;
        profile.mouse_accel = 0.0;
        Mapper::new(profile)
    }

    /// The X of each cursor move, 0 for reports without one.
    fn moves_x(outputs: &[Vec<Output>]) -> Vec<i32> {
        outputs
            .iter()
            .map(|outputs| match outputs.as_slice() {
                [] => 0,
                [Output::MouseMove(x, 0)] => *x,
                _ => panic!("{outputs:?}"),
            })
            .collect()
    }

    #[test]
    fn test_mapper_smoothing_slow() {
        // 20 units a report with up to 30 of jitter either way, from a fixed LCG
        let mut seed = 7u32;
        let xs: Vec<i16> = (0..150)
            .map(|i| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (i * 20 + (seed >> 16) as i32 % 61 - 30) as i16
            })
            .collect();
        let raw = moves_x(&trace(&mut smoothed(0.0), &xs));
        let smooth = moves_x(&trace(&mut smoothed(1.0), &xs));
        let variance = |xs: &[i32]| {
            let mean = xs.iter().sum::<i32>() as f32 / xs.len() as f32;
            xs.iter().map(|x| (*x as f32 - mean).powi(2)).sum::<f32>() / xs.len() as f32
        };
        // Moves begin with the second report and the lift adds none
        let (raw, smooth) = (&raw[1..xs.len()], &smooth[1..xs.len()]);
        assert!(raw.iter().any(|x| *x < 0), "{raw:?}");
        assert!(smooth.iter().all(|x| *x >= 0), "{smooth:?}");
        assert!(variance(smooth) * 4.0 < variance(raw), "{smooth:?}");
        let raw: i32 = raw.iter().sum();
        let smooth: i32 = smooth.iter().sum();
        assert!((smooth - raw).abs() * 10 <= raw, "{smooth} vs {raw}");
    }

    #[test]
    fn test_mapper_smoothing_flick() {
        let xs = [0, 2000, 4000];
        let raw = moves_x(&trace(&mut smoothed(0.0), &xs))[1];
        let smooth = moves_x(&trace(&mut smoothed(1.0), &xs))[1];
        assert!(smooth * 100 >= raw * 85, "{smooth} vs {raw}");
    }

    #[test]
    fn test_mapper_trigger_click() {
        let config = Config::parse(
            "[profile.t]\ntrigger.click = 0.5\n[profile.t.axes]\nrt = \"mouse:left\"",
        )
        .unwrap();
        let mut mapper = Mapper::new(config.profile().unwrap());
        let rt = |value: u16| ControllerState {
            rt: value,
            ..ControllerState::default()
        };
        let full = i16::MAX as u16;
        let press = vec![Output::MouseButton(MouseButton::Left, true)];
        let release = vec![Output::MouseButton(MouseButton::Left, false)];
        assert_eq!(mapper.map(&rt(0), &rt(full / 4)), vec![]);
        assert_eq!(mapper.map(&rt(full / 4), &rt(full / 2 + 10)), press);
        assert_eq!(mapper.map(&rt(full / 2), &rt(full)), vec![]);
        // Between 0.4 and 0.5 of travel it stays held
        assert_eq!(mapper.map(&rt(full), &rt(full / 20 * 9)), vec![]);
        assert_eq!(mapper.map(&rt(full / 20 * 9), &rt(full / 3)), release);
        assert_eq!(mapper.map(&rt(full / 3), &rt(full)), press);
        assert_eq!(mapper.release_all(), release);
    }

    /// Sum of the scrolls of a touch through `points`, then a lift.
    fn swipe(
        mapper: &mut Mapper,
        points: &[(i16, i16)],
    ) -> (i32, i32) {
        let mut prev = ControllerState::default();
        let mut sum = (0, 0);
        for &(x, y) in points {
            let state = lpad(x, y);
            for output in mapper.map(&prev, &state) {
                let Output::Scroll(x, y) = output else {
                    panic!("{output:?}");
                };
                sum = (sum.0 + x, sum.1 + y);
            }
            prev = state;
        }
        assert_eq!(mapper.map(&prev, &ControllerState::default()), vec![]);
        sum
    }

    #[test]
    fn test_mapper_scroll() {
        let mut mapper = mouse();
        // Natural: the finger moving up pulls the content up, the wheel turns down
        let up: Vec<_> = (0..=100).map(|i| (0, i * 300)).collect();
        let (x, y) = swipe(&mut mapper, &up);
        assert_eq!(x, 0);
        assert!((-560..=-450).contains(&y), "{y}");
    }

    #[test]
    fn test_scroll_rest() {
        // A resting thumb's jitter never scrolls
        let mut mapper = mouse();
        let rest: Vec<_> = (0..200).map(|i| (i % 2 * 400, i % 3 * 400)).collect();
        assert_eq!(swipe(&mut mapper, &rest), (0, 0));
    }

    #[test]
    fn test_scroll_axis_lock() {
        let mut mapper = mouse();
        // A wobbly vertical swipe stays vertical
        let wobble: Vec<_> = (0..=100).map(|i| (i % 2 * 300 - 150, i * 300)).collect();
        let (x, y) = swipe(&mut mapper, &wobble);
        assert_eq!(x, 0);
        assert!(y < -450, "{y}");
        // A turn to a clearly horizontal swipe takes the lock
        let turn: Vec<_> = (0..=100)
            .map(|i| match i < 50 {
                true => (0, i * 300),
                false => ((i - 50) * 400, 15000),
            })
            .collect();
        let (x, _) = swipe(&mut mapper, &turn);
        assert!(x < -100, "{x}");
    }

    #[test]
    fn test_scroll_smoothing() {
        let mut filter = ScrollFilter {
            vertical: Some(true),
            ..ScrollFilter::default()
        };
        // Uneven reports come out evener, and the filter converges on a steady swipe
        let (_, a) = filter.filter(0.0, 400.0);
        let (_, b) = filter.filter(0.0, 0.0);
        assert!(a < 400.0 && b > 0.0, "{a} {b}");
        let mut last = 0.0;
        for _ in 0..100 {
            (_, last) = filter.filter(0.0, 200.0);
        }
        assert!((last - 200.0).abs() < 1.0, "{last}");
    }

    fn sticks(axes: &str) -> Mapper {
        let text = format!(
            "[profile.pad]\ndeadzone.stick = 0.1\nmouse.stick_curve = 1.0\n[profile.pad.axes]\n{axes}"
        );
        Mapper::new(Config::parse(&text).unwrap().profile().unwrap())
    }

    fn rstick(
        x: i16,
        y: i16,
    ) -> ControllerState {
        ControllerState {
            rstick: Stick { x, y },
            buttons: crate::state::Buttons::default().with(Button::RStickTouch),
            ..ControllerState::default()
        }
    }

    /// Sum of the cursor moves and scrolls over `reports` reports of `state`.
    fn hold(
        mapper: &mut Mapper,
        state: &ControllerState,
        reports: usize,
    ) -> (i32, i32, i32, i32) {
        let mut sum = (0, 0, 0, 0);
        for _ in 0..reports {
            for output in mapper.map(state, state) {
                match output {
                    Output::MouseMove(x, y) => (sum.0, sum.1) = (sum.0 + x, sum.1 + y),
                    Output::Scroll(x, y) => (sum.2, sum.3) = (sum.2 + x, sum.3 + y),
                    other => panic!("{other:?}"),
                }
            }
        }
        sum
    }

    #[test]
    fn test_mapper_stick_cursor() {
        let mut mapper = sticks("rstick = \"cursor\"");
        let full = i16::MAX;
        // One second at full deflection crosses mouse.stick_speed, Y down on screen
        let (x, y, _, _) = hold(&mut mapper, &rstick(full, 0), REPORT_RATE as usize);
        assert!((999..=1000).contains(&x), "{x}");
        assert_eq!(y, 0);
        let (x, y, _, _) = hold(&mut mapper, &rstick(0, full), REPORT_RATE as usize);
        assert_eq!(x, 0);
        assert!((-1000..=-999).contains(&y), "{y}");
        // Inside the deadzone nothing moves
        assert_eq!(
            hold(&mut mapper, &rstick(3000, 0), REPORT_RATE as usize),
            (0, 0, 0, 0)
        );
        // Linear: halfway between the deadzone and the edge is half the speed
        let half = (i16::MAX as f32 * 0.55) as i16;
        let (x, _, _, _) = hold(&mut mapper, &rstick(half, 0), REPORT_RATE as usize);
        assert!((498..=500).contains(&x), "{x}");
    }

    #[test]
    fn test_mapper_stick_curve() {
        let mut mapper = sticks("rstick = \"cursor\"");
        let mut profile = mapper.profile.clone();
        profile.mouse_stick_curve = 2.0;
        mapper.retune(profile);
        let half = (i16::MAX as f32 * 0.55) as i16;
        let (x, _, _, _) = hold(&mut mapper, &rstick(half, 0), REPORT_RATE as usize);
        assert!((248..=250).contains(&x), "{x}");
        // The edge is unchanged
        let (x, _, _, _) = hold(&mut mapper, &rstick(i16::MAX, 0), REPORT_RATE as usize);
        assert!((999..=1000).contains(&x), "{x}");
    }

    #[test]
    fn test_mapper_stick_scroll() {
        let mut mapper = sticks("rstick = \"scroll\"");
        // Up scrolls up, scroll.stick_speed notches a second
        let (_, _, x, y) = hold(&mut mapper, &rstick(0, i16::MAX), REPORT_RATE as usize);
        assert_eq!(x, 0);
        assert!((10 * NOTCH - 1..=10 * NOTCH).contains(&y), "{y}");
        assert_eq!(
            hold(&mut mapper, &rstick(0, 3000), REPORT_RATE as usize),
            (0, 0, 0, 0)
        );
    }

    #[test]
    fn test_angle_math() {
        use std::f32::consts::FRAC_PI_2;
        use std::f32::consts::PI;
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(close(stick_angle(0, 100), 0.0));
        assert!(close(stick_angle(100, 0), FRAC_PI_2));
        assert!(close(stick_angle(-100, 0), -FRAC_PI_2));
        assert!(close(stick_angle(0, -100).abs(), PI));
        // Small deltas either way
        assert!(close(angle_delta(0.1, 0.12), 0.02));
        assert!(close(angle_delta(0.12, 0.1), -0.02));
        // Across straight down, the short way round
        assert!(close(angle_delta(PI - 0.05, -PI + 0.05), 0.1));
        assert!(close(angle_delta(-PI + 0.05, PI - 0.05), -0.1));
        assert!(close(ease(0.0), 0.0));
        assert!(close(ease(1.0), 1.0));
        assert!(ease(0.5) > 0.5);
    }

    #[test]
    fn test_mapper_flick_stick() {
        let mut mapper = sticks("rstick = \"flick\"");
        let full = i16::MAX;
        // Flicked right: a quarter of mouse.flick_degrees over mouse.flick_ms (25 reports),
        // front-loaded
        let mut moves = Vec::new();
        for _ in 0..30 {
            for output in mapper.map(&rstick(full, 0), &rstick(full, 0)) {
                let Output::MouseMove(x, 0) = output else {
                    panic!("{output:?}");
                };
                moves.push(x);
            }
        }
        assert_eq!(moves.len(), 25, "{moves:?}");
        assert!(moves[0] > moves[24], "{moves:?}");
        let sum: i32 = moves.iter().sum();
        assert!((599..=600).contains(&sum), "{sum}");
        // Rotated on to straight down while held: another quarter, as it goes
        let (x, _, _, _) = hold(&mut mapper, &rstick(full / 2, -full / 2), 1);
        assert!((299..=300).contains(&x), "{x}");
        let (x, _, _, _) = hold(&mut mapper, &rstick(0, -full), 1);
        assert!((299..=300).contains(&x), "{x}");
        // Released and flicked left: a quarter turn back
        assert_eq!(hold(&mut mapper, &rstick(0, 0), 5), (0, 0, 0, 0));
        let (x, _, _, _) = hold(&mut mapper, &rstick(-full, 0), 30);
        assert!((-600..=-599).contains(&x), "{x}");
    }

    #[test]
    fn test_mapper_mouse_button() {
        let mut mapper = mouse();
        let idle = ControllerState::default();
        let a = pressed(&[Button::A]);
        assert_eq!(
            mapper.map(&idle, &a),
            vec![Output::MouseButton(MouseButton::Left, true)]
        );
        assert_eq!(
            mapper.map(&a, &idle),
            vec![Output::MouseButton(MouseButton::Left, false)]
        );
    }

    #[test]
    fn test_config_active() {
        let config = Config::default();
        assert_eq!(config.active().unwrap(), PAD);
        assert_eq!(config.get(PAD).unwrap(), Profile::xbox());
        assert!(config.get("browser").is_err());
    }

    #[test]
    fn test_mapper_flick() {
        let config = Config::parse(include_str!("../../../examples/settop.toml")).unwrap();
        let mut mapper = Mapper::new(config.get("browser").unwrap());
        let idle = ControllerState::default();
        let touched = pressed(&[Button::LStickTouch]);
        let mut up = touched;
        up.lstick = Stick { x: 1000, y: 20000 };
        let mut edge = touched;
        edge.lstick = Stick { x: 0, y: 15000 };
        let mut left = touched;
        left.lstick = Stick { x: -30000, y: 0 };
        let key_up = evdev::KeyCode::KEY_UP.0;
        let key_left = evdev::KeyCode::KEY_LEFT.0;
        // Inside the deadzone nothing; one press per flick, held while it stays out
        assert_eq!(mapper.map(&idle, &edge), vec![]);
        assert_eq!(mapper.map(&edge, &up), vec![Output::Key(key_up, true)]);
        assert_eq!(mapper.map(&up, &up), vec![]);
        assert_eq!(mapper.map(&up, &edge), vec![]);
        assert_eq!(
            mapper.map(&edge, &left),
            vec![Output::Key(key_up, false), Output::Key(key_left, true)]
        );
        assert_eq!(mapper.release_all(), vec![Output::Key(key_left, false)]);
        assert_eq!(mapper.map(&left, &idle), vec![]);
    }

    #[test]
    fn test_target_combo() {
        let combo = Target::Combo(vec![K::KEY_LEFTMETA.0, K::KEY_D.0]);
        assert_eq!("key:LEFTMETA+KEY_D".parse::<Target>().unwrap(), combo);
        assert_eq!(combo.to_string(), "key:KEY_LEFTMETA+KEY_D");
        assert_eq!("key:D".parse::<Target>().unwrap(), Target::Key(K::KEY_D.0));
        assert!("key:LEFTMETA+".parse::<Target>().is_err());
        let config = Config::parse(
            "[profile.p.buttons]\na = \"key:LEFTCTRL+LEFTSHIFT+T\"\n[profile.p.axes]\nrt = \"key:LEFTALT+TAB\"",
        )
        .unwrap();
        let keys = config.keys();
        for code in [
            K::KEY_LEFTCTRL,
            K::KEY_LEFTSHIFT,
            K::KEY_T,
            K::KEY_LEFTALT,
            K::KEY_TAB,
        ] {
            assert!(keys.contains(&code.0), "{code:?}");
        }
        let mut mapper = Mapper::new(quiet(config.get("p").unwrap()));
        let idle = ControllerState::default();
        let a = pressed(&[Button::A]);
        assert_eq!(
            mapper.map(&idle, &a),
            vec![
                key(K::KEY_LEFTCTRL, true),
                key(K::KEY_LEFTSHIFT, true),
                key(K::KEY_T, true)
            ]
        );
        assert_eq!(
            mapper.map(&a, &idle),
            vec![
                key(K::KEY_T, false),
                key(K::KEY_LEFTSHIFT, false),
                key(K::KEY_LEFTCTRL, false)
            ]
        );
    }

    #[test]
    fn test_menu_config() {
        assert_eq!(Config::default().menu, Menu::default());
        let config = Config::parse(
            "[profile.p.buttons]\nquick = \"menu:toggle\"\n[menu]\ncolumns = 2\nitems = [{ label = \"Up\", icon = \"volume-up\", target = \"key:KEY_VOLUMEUP\" }]",
        )
        .unwrap();
        assert_eq!(config.menu.columns, 2);
        assert_eq!(config.menu.items[0].target.to_string(), "key:KEY_VOLUMEUP");
        let quick = &config.profiles["p"].buttons[&Button::Quick].press;
        assert_eq!(quick.to_string(), "menu:toggle");
        let err = Config::parse(
            "[menu]\nitems = [{ label = \"x\", icon = \"apps\", target = \"profile:gone\" }]",
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "menu.items.0: No profile named gone");
        assert!(Config::parse("[profile.x.axes]\nlstick = \"menu:open\"").is_err());
        assert!(Config::parse("[profile.x.buttons]\na = \"menu:spin\"").is_err());
        // The example writes out the default
        let settop = Config::parse(include_str!("../../../examples/settop.toml")).unwrap();
        assert_eq!(settop.menu, Menu::default());
    }

    #[test]
    fn test_desktop_backend() {
        assert_eq!(Config::default().desktop, Backend::Auto);
        let config = Config::parse("[desktop]\nbackend = \"none\"").unwrap();
        assert_eq!(config.desktop, Backend::None);
        assert!(Config::parse("[desktop]\nbackend = \"sway\"").is_err());
    }

    #[test]
    fn test_target_desktop() {
        for name in [
            "desktop:overview",
            "desktop:workspace:4",
            "desktop:spawn:foot -e top",
        ] {
            assert_eq!(name.parse::<Target>().unwrap().to_string(), name);
        }
        let err = Config::parse("[profile.x.buttons]\na = \"desktop:fly\"").unwrap_err();
        assert_eq!(
            err.to_string(),
            "Profile x: buttons.a: Unknown desktop action fly"
        );
        let config = Config::parse("[profile.p.buttons]\nlb = \"desktop:workspace-up\"").unwrap();
        let mut mapper = Mapper::new(config.get("p").unwrap());
        let lb = pressed(&[Button::Lb]);
        assert_eq!(
            mapper.map(&ControllerState::default(), &lb),
            vec![Output::Desktop(Action::WorkspaceUp)]
        );
    }

    #[test]
    fn test_target_niri() {
        let target: Target = "niri:focus-column-left".parse().unwrap();
        assert_eq!(target.to_string(), "niri:focus-column-left");
        assert_eq!(
            "niri:spawn:fuzzel".parse::<Target>().unwrap().to_string(),
            "niri:spawn:fuzzel"
        );
        let err = Config::parse("[profile.x.buttons]\na = \"niri:fly\"").unwrap_err();
        assert_eq!(
            err.to_string(),
            "Profile x: buttons.a: Unknown niri action fly"
        );
        assert!(Config::parse("[profile.x.axes]\nlstick = \"niri:quit\"").is_err());
        let config = Config::parse("[profile.p.buttons]\nlb = \"niri:quit\"").unwrap();
        let mut mapper = Mapper::new(config.get("p").unwrap());
        let idle = ControllerState::default();
        let lb = pressed(&[Button::Lb]);
        assert_eq!(
            mapper.map(&idle, &lb),
            vec![Output::Desktop(Action::raw("quit").unwrap())]
        );
        assert_eq!(mapper.map(&lb, &idle), vec![]);
    }

    #[test]
    fn test_mapper_flicks() {
        let mut mapper =
            sticks("rstick = \"flicks:desktop:focus-up,DOWN,key:LEFTALT+TAB,mouse:left\"");
        let up = Output::Desktop(Action::FocusUp);
        let full = i16::MAX;
        assert_eq!(mapper.map(&rstick(0, 0), &rstick(0, full)), vec![up]);
        assert_eq!(mapper.map(&rstick(0, full), &rstick(0, full)), vec![]);
        assert_eq!(
            mapper.map(&rstick(0, full), &rstick(-full, 0)),
            vec![key(K::KEY_LEFTALT, true), key(K::KEY_TAB, true)]
        );
        assert_eq!(
            mapper.release_all(),
            vec![key(K::KEY_TAB, false), key(K::KEY_LEFTALT, false)]
        );
        let target: Target = "flicks:UP,KEY_DOWN,niri:focus-column-left,none"
            .parse()
            .unwrap();
        assert_eq!(
            target.to_string(),
            "flicks:key:KEY_UP,key:KEY_DOWN,niri:focus-column-left,none"
        );
        assert!("flicks:UP,DOWN,LEFT".parse::<Target>().is_err());
        assert!(
            Config::parse("[profile.x.axes]\nrstick = \"flicks:UP,DOWN,LEFT,cursor\"").is_err()
        );
        assert!(Config::parse("[profile.x.axes]\nrt = \"flicks:UP,DOWN,LEFT,RIGHT\"").is_err());
    }

    #[test]
    fn test_config_default() {
        assert_eq!(Config::default().profile().unwrap(), Profile::xbox());
        assert_eq!(
            Config::parse("").unwrap().profile().unwrap(),
            Profile::xbox()
        );
    }

    #[test]
    fn test_config_errors() {
        assert!(Config::parse("[profile.x.buttons]\na = \"pad:nope\"").is_err());
        assert!(Config::parse("[profile.x.buttons]\nlstick = \"pad:south\"").is_err());
        assert!(Config::parse("[profile.x.axes]\nlt = \"pad:left\"").is_err());
        assert!(Config::parse("[profile.x.axes]\nlt = \"cursor\"").is_err());
        assert!(Config::parse("[profile.x.axes]\nrpad = \"flick\"").is_err());
        assert!(Config::parse("[profile.x]\nmouse.stick_curve = 0.5").is_err());
        assert!(Config::parse("[profile.x]\nmouse.release_guard = 21").is_err());
        assert!(Config::parse("[profile.x]\nmouse.smoothing = 1.5").is_err());
        assert!(Config::parse("[profile.x.buttons]\na = \"scroll\"").is_err());
        assert!(
            Config::parse("[daemon]\nactive = \"missing\"")
                .unwrap()
                .profile()
                .is_err()
        );
    }

    #[test]
    fn test_config_lizard_cue() {
        assert!(Config::default().daemon.lizard_cue);
        assert!(Config::parse("").unwrap().daemon.lizard_cue);
        assert!(
            !Config::parse("[daemon]\nlizard_cue = false")
                .unwrap()
                .daemon
                .lizard_cue
        );
    }

    #[test]
    fn test_config_rumble() {
        assert!(Config::default().daemon.rumble);
        assert!(Config::parse("").unwrap().daemon.rumble);
        assert!(
            !Config::parse("[daemon]\nrumble = false")
                .unwrap()
                .daemon
                .rumble
        );
    }

    #[test]
    fn test_config_steam() {
        assert_eq!(Config::parse("").unwrap().daemon.steam, Steam::Yield);
        let config = Config::parse("[daemon]\nsteam = \"ignore\"").unwrap();
        assert_eq!(config.daemon.steam, Steam::Ignore);
        assert!(Config::parse("[daemon]\nsteam = \"share\"").is_err());
    }

    #[test]
    fn test_config_example_settop() {
        let config = Config::parse(include_str!("../../../examples/settop.toml")).unwrap();
        let profile = config.profile().unwrap();
        assert_eq!(profile, Config::parse(SETTOP).unwrap().profile().unwrap());
        assert_eq!(config.daemon.lizard, Lizard::Off);
        // The keyboard enables the keys of every profile, not just the active one
        let keys = config.keys();
        assert!(keys.contains(&evdev::KeyCode::KEY_BACKSPACE.0));
        assert!(keys.contains(&evdev::KeyCode::KEY_LEFT.0));
        assert!(keys.contains(&evdev::KeyCode::KEY_ENTER.0));
        let desktop = config.get("desktop").unwrap();
        assert_eq!(desktop.axes[&Axis::RPad], Target::Cursor);
        assert_eq!(
            desktop.buttons[&Button::RPadClick].press,
            Target::Mouse(MouseButton::Left)
        );
    }

    #[test]
    fn test_vocabulary_menu_icons() {
        let vocabulary = Vocabulary::new();
        assert_eq!(vocabulary.menu_icons.len(), crate::menu::ICONS.len());
        assert!(vocabulary.menu_icons.contains(&"power".to_string()));
    }

    #[test]
    fn test_vocabulary_matches_validation() {
        let vocabulary = Vocabulary::new();
        let sources = vocabulary.buttons.iter().chain(&vocabulary.axes);
        for source in sources {
            let allowed = &vocabulary.allowed[source];
            assert!(!allowed.is_empty(), "{source}");
            for kind in &vocabulary.kinds {
                let ok = allowed.contains(&kind.name);
                if kind.args.is_empty() {
                    assert_eq!(
                        binding(source, &kind.target("")).is_ok(),
                        ok,
                        "{source} {kind:?}"
                    );
                }
                for arg in &kind.args {
                    let target = kind.target(arg);
                    assert_eq!(binding(source, &target).is_ok(), ok, "{source} {target}");
                }
            }
        }
        // Each class takes something another refuses
        assert!(vocabulary.allowed["a"].contains(&"key".to_string()));
        assert!(!vocabulary.allowed["a"].contains(&"stick".to_string()));
        assert!(vocabulary.allowed["rpad"].contains(&"cursor".to_string()));
        assert!(vocabulary.allowed["lstick"].contains(&"cursor".to_string()));
        assert!(vocabulary.allowed["rstick"].contains(&"scroll".to_string()));
        assert!(vocabulary.allowed["rstick"].contains(&"flick".to_string()));
        assert!(!vocabulary.allowed["rpad"].contains(&"flick".to_string()));
        assert!(!vocabulary.allowed["rt"].contains(&"cursor".to_string()));
        assert!(!vocabulary.allowed["lt"].contains(&"keys".to_string()));
        assert!(vocabulary.allowed["rt"].contains(&"mouse".to_string()));
        assert!(vocabulary.allowed["rt"].contains(&"key".to_string()));
        assert!(vocabulary.allowed["rt"].contains(&"button".to_string()));
        assert!(!vocabulary.allowed["rstick"].contains(&"mouse".to_string()));
        assert_eq!(vocabulary.defaults["a"], "pad:south");
        assert_eq!(vocabulary.defaults["lt"], "pad:lt");
        assert!(key_names().contains(&"KEY_HOMEPAGE".to_string()));
    }

    #[test]
    fn test_config_toml_round_trip() {
        let text = include_str!("../../../examples/settop.toml");
        let toml = ConfigToml::parse(text).unwrap();
        let written = toml.to_toml().unwrap();
        assert_eq!(ConfigToml::parse(&written).unwrap(), toml);
        assert_eq!(
            Config::parse(&written).unwrap(),
            Config::parse(text).unwrap()
        );
        assert!(written.contains("stick = 0.1\n"), "{written}");
    }

    #[test]
    fn test_config_error_names_binding() {
        let err = Config::parse("[profile.x.buttons]\na = \"key:KEY_NOPE\"").unwrap_err();
        assert_eq!(
            err.to_string(),
            "Profile x: buttons.a: Unknown key KEY_NOPE"
        );
        let err =
            Config::parse("[[profile.x.chord]]\nhold = \"view\"\nb = \"cursor\"").unwrap_err();
        assert_eq!(
            err.to_string(),
            "Profile x: chord.0.b: Cannot map b to cursor"
        );
    }

    #[test]
    fn test_target_profile_parse() {
        let named = Target::Profile(ProfileSwitch::Named("desktop".to_string()));
        assert_eq!("profile:desktop".parse::<Target>().unwrap(), named);
        assert_eq!(
            "profile:next".parse::<Target>().unwrap(),
            Target::Profile(ProfileSwitch::Next)
        );
        assert_eq!(
            "profile:prev".parse::<Target>().unwrap(),
            Target::Profile(ProfileSwitch::Prev)
        );
        assert!("profile:".parse::<Target>().is_err());
        for target in ["profile:desktop", "profile:next", "profile:prev"] {
            assert_eq!(target.parse::<Target>().unwrap().to_string(), target);
        }
    }

    #[test]
    fn test_target_profile_allows() {
        let next = Target::Profile(ProfileSwitch::Next);
        assert!(allows(Source::Button(Button::Y), &next));
        assert!(!allows(Source::Axis(Axis::Rt), &next));
        assert!(!allows(Source::Axis(Axis::LStick), &next));
        assert!(Config::parse("[profile.x.axes]\nrt = \"profile:next\"").is_err());
        let vocabulary = Vocabulary::new();
        assert!(vocabulary.allowed["y"].contains(&"profile".to_string()));
        assert!(!vocabulary.allowed["rt"].contains(&"profile".to_string()));
        assert!(!vocabulary.allowed["rpad"].contains(&"profile".to_string()));
    }

    #[test]
    fn test_mapper_profile_fires_on_release() {
        let config = Config::parse("[profile.x.buttons]\ny = \"profile:next\"").unwrap();
        let mut mapper = Mapper::new(config.get("x").unwrap());
        let idle = ControllerState::default();
        let y = pressed(&[Button::Y]);
        assert_eq!(mapper.map(&idle, &y), vec![]);
        assert_eq!(
            mapper.map(&y, &idle),
            vec![Output::Profile(ProfileSwitch::Next)]
        );
        // Held through a switch from elsewhere: dropped, not fired
        assert_eq!(mapper.map(&idle, &y), vec![]);
        assert_eq!(mapper.release_all(), vec![]);
    }

    #[test]
    fn test_mapper_profile_chord() {
        let mut mapper = settop();
        let idle = ControllerState::default();
        let steam = pressed(&[Button::Steam]);
        let both = pressed(&[Button::Steam, Button::Y]);
        let switch = Output::Profile(ProfileSwitch::Named("desktop".to_string()));
        assert_eq!(mapper.map(&idle, &steam), vec![]);
        assert_eq!(mapper.map(&steam, &both), vec![]);
        assert_eq!(mapper.map(&both, &steam), vec![switch]);
        // The hold was used: no late home key
        assert_eq!(mapper.map(&steam, &idle), vec![]);
        // Tapped alone, steam still goes home on its release
        let home = evdev::KeyCode::KEY_HOMEPAGE.0;
        assert_eq!(mapper.map(&idle, &steam), vec![]);
        assert_eq!(
            mapper.map(&steam, &idle),
            vec![Output::Key(home, true), Output::Key(home, false)]
        );
    }

    #[test]
    fn test_config_profile_unknown() {
        let err = Config::parse("[profile.x.buttons]\ny = \"profile:nope\"").unwrap_err();
        assert_eq!(
            err.to_string(),
            "Profile x: buttons.y: No profile named nope"
        );
        let err = Config::parse("[[profile.x.chord]]\nhold = \"steam\"\ny = \"profile:nope\"")
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Profile x: chord.0.y: No profile named nope"
        );
        assert!(Config::parse("[profile.x.buttons]\ny = \"profile:pad\"").is_ok());
        assert!(Config::parse("[profile.x.buttons]\ny = \"profile:x\"").is_ok());
    }

    #[test]
    fn test_profile_switch_pick() {
        let profiles: Vec<String> = ["browser", "desktop", "pad"]
            .iter()
            .map(|name| name.to_string())
            .collect();
        let next = ProfileSwitch::Next;
        let prev = ProfileSwitch::Prev;
        assert_eq!(next.pick("browser", &profiles), Some("desktop"));
        assert_eq!(next.pick("pad", &profiles), Some("browser"));
        assert_eq!(prev.pick("browser", &profiles), Some("pad"));
        assert_eq!(prev.pick("desktop", &profiles), Some("browser"));
        let named = ProfileSwitch::Named("pad".to_string());
        assert_eq!(named.pick("browser", &profiles), Some("pad"));
        assert_eq!(next.pick("pad", &[]), None);
        assert_eq!(prev.pick("pad", &[]), None);
    }

    const ACTIVATORS: &str = r#"
        [profile.p.buttons]
        a = { press = "key:KEY_A", long = "key:KEY_L" }
        b = { press = "key:KEY_B", double = "key:KEY_D", double_ms = 200 }
        x = { press = "key:KEY_X", start = "key:KEY_S", release = "key:KEY_R" }
        y = { press = "key:KEY_Y", turbo_ms = 100 }
        steam = { press = "desktop:overview", long = "lizard:hold", long_ms = 300 }
        quick = "lizard:toggle"
        menu = { press = "key:KEY_M", long = "key:KEY_W", long_ms = 3000, warn_ms = 1000 }
    "#;

    /// A mapper over [`ACTIVATORS`] and a clock: `step` moves time on and feeds one report.
    struct Clock {
        mapper: Mapper,
        start: Instant,
        prev: ControllerState,
    }
    impl Clock {
        fn new() -> Self {
            let profile = Config::parse(ACTIVATORS).unwrap().get("p").unwrap();
            Self {
                mapper: Mapper::new(quiet(profile)),
                start: Instant::now(),
                prev: ControllerState::default(),
            }
        }

        /// The outputs of a report at `at` ms holding `buttons`.
        fn step(
            &mut self,
            at: u64,
            buttons: &[Button],
        ) -> Vec<Output> {
            let state = pressed(buttons);
            let now = self.start + Duration::from_millis(at);
            let outputs = self.mapper.map_at(&self.prev, &state, now);
            self.prev = state;
            outputs
        }
    }

    fn key(
        code: evdev::KeyCode,
        pressed: bool,
    ) -> Output {
        Output::Key(code.0, pressed)
    }

    use evdev::KeyCode as K;

    #[test]
    fn test_activator_string_is_press() {
        let toml = ConfigToml::parse(
            "[profile.p.buttons]\na = \"pad:east\"\nb = { press = \"pad:east\" }",
        )
        .unwrap();
        let profile = Profile::try_from(toml.profile["p"].clone()).unwrap();
        assert_eq!(profile.buttons[&Button::A], profile.buttons[&Button::B]);
        // A table without press keeps the layout's
        let toml = ConfigToml::parse("[profile.p.buttons]\na = { long = \"pad:east\" }").unwrap();
        let profile = Profile::try_from(toml.profile["p"].clone()).unwrap();
        assert_eq!(
            profile.buttons[&Button::A].press,
            Target::Button(PadButton::South)
        );
    }

    #[test]
    fn test_activator_rejects_axis_target() {
        let err = Config::parse("[profile.p.buttons]\na = { long = \"pad:left\" }").unwrap_err();
        assert_eq!(
            err.to_string(),
            "Profile p: buttons.a: long: Cannot map a to pad:left"
        );
        assert!(Config::parse("[profile.p.buttons]\na = { turbo_ms = 0 }").is_err());
        assert!(Config::parse("[profile.p.buttons]\na = { held = \"pad:east\" }").is_err());
    }

    #[test]
    fn test_activator_long() {
        let mut clock = Clock::new();
        assert_eq!(clock.step(0, &[Button::A]), vec![]);
        assert_eq!(clock.step(399, &[Button::A]), vec![]);
        assert_eq!(clock.step(400, &[Button::A]), vec![key(K::KEY_L, true)]);
        assert_eq!(clock.step(900, &[]), vec![key(K::KEY_L, false)]);
    }

    #[test]
    fn test_activator_warn() {
        let buzz = Output::Haptic {
            side: Side::Right,
            strength: u8::MAX,
            ms: WARN_BUZZ_MS,
        };
        let mut clock = Clock::new();
        assert_eq!(clock.step(0, &[Button::Menu]), vec![]);
        assert_eq!(clock.step(1999, &[Button::Menu]), vec![]);
        assert_eq!(
            clock.step(2000, &[Button::Menu]),
            vec![buzz.clone(), WARN_BEEP]
        );
        assert_eq!(clock.step(2500, &[Button::Menu]), vec![]);
        assert_eq!(clock.step(3000, &[Button::Menu]), vec![key(K::KEY_W, true)]);
        assert_eq!(clock.step(3100, &[]), vec![key(K::KEY_W, false)]);
        // A short tap never warns
        assert_eq!(clock.step(4000, &[Button::Menu]), vec![]);
        assert_eq!(
            clock.step(4200, &[]),
            vec![key(K::KEY_M, true), key(K::KEY_M, false)]
        );
        assert_eq!(clock.step(7000, &[]), vec![]);
        // Released after the warning: cancelled, nothing more fires
        assert_eq!(clock.step(8000, &[Button::Menu]), vec![]);
        assert_eq!(clock.step(10000, &[Button::Menu]), vec![buzz, WARN_BEEP]);
        assert_eq!(
            clock.step(10500, &[]),
            vec![key(K::KEY_M, true), key(K::KEY_M, false)]
        );
        assert_eq!(clock.step(13000, &[]), vec![]);
        // Released in the report the warning falls due: no warning
        assert_eq!(clock.step(14000, &[Button::Menu]), vec![]);
        assert_eq!(
            clock.step(16000, &[]),
            vec![key(K::KEY_M, true), key(K::KEY_M, false)]
        );
        assert!(
            Config::parse("[profile.p.buttons]\na = { long = \"key:KEY_L\", warn_ms = 400 }")
                .is_err()
        );
    }

    #[test]
    fn test_activator_long_excludes_press() {
        let mut clock = Clock::new();
        assert_eq!(clock.step(0, &[Button::A]), vec![]);
        assert_eq!(
            clock.step(200, &[]),
            vec![key(K::KEY_A, true), key(K::KEY_A, false)]
        );
        assert_eq!(clock.step(700, &[]), vec![]);
    }

    /// Steam taps to the overview, held it hands the controller to lizard mode until release.
    #[test]
    fn test_activator_lizard() {
        let mut clock = Clock::new();
        assert_eq!(clock.step(0, &[Button::Steam]), vec![]);
        assert_eq!(clock.step(299, &[Button::Steam]), vec![]);
        assert_eq!(
            clock.step(300, &[Button::Steam]),
            vec![Output::Lizard(LizardSwitch::Hold, true)]
        );
        assert_eq!(
            clock.step(900, &[]),
            vec![Output::Lizard(LizardSwitch::Hold, false)]
        );
        assert_eq!(clock.step(1000, &[Button::Steam]), vec![]);
        assert_eq!(
            clock.step(1100, &[]),
            vec![Output::Desktop(Action::parse("overview").unwrap())]
        );
        assert_eq!(
            clock.step(2000, &[Button::Quick]),
            vec![Output::Lizard(LizardSwitch::Toggle, true)]
        );
        assert_eq!(
            clock.step(2100, &[]),
            vec![Output::Lizard(LizardSwitch::Toggle, false)]
        );
        assert_eq!(
            "lizard:hold".parse::<Target>().unwrap().to_string(),
            "lizard:hold"
        );
        assert!("lizard:on".parse::<Target>().is_err());
        assert!(Config::parse("[profile.p.axes]\nlstick = \"lizard:hold\"\n").is_err());
    }

    #[test]
    fn test_activator_double() {
        let mut clock = Clock::new();
        assert_eq!(clock.step(0, &[Button::B]), vec![]);
        assert_eq!(clock.step(50, &[]), vec![]);
        assert_eq!(clock.step(150, &[Button::B]), vec![key(K::KEY_D, true)]);
        assert_eq!(clock.step(600, &[Button::B]), vec![]);
        assert_eq!(clock.step(650, &[]), vec![key(K::KEY_D, false)]);
        assert_eq!(clock.step(2000, &[]), vec![]);
    }

    #[test]
    fn test_activator_double_delays_single() {
        let mut clock = Clock::new();
        assert_eq!(clock.step(0, &[Button::B]), vec![]);
        assert_eq!(clock.step(50, &[]), vec![]);
        assert_eq!(clock.step(199, &[]), vec![]);
        assert_eq!(
            clock.step(200, &[]),
            vec![key(K::KEY_B, true), key(K::KEY_B, false)]
        );
        // Held past the window: press goes down then and holds
        assert_eq!(clock.step(1000, &[Button::B]), vec![]);
        assert_eq!(clock.step(1200, &[Button::B]), vec![key(K::KEY_B, true)]);
        assert_eq!(clock.step(1500, &[]), vec![key(K::KEY_B, false)]);
    }

    #[test]
    fn test_activator_start_release() {
        let mut clock = Clock::new();
        assert_eq!(
            clock.step(0, &[Button::X]),
            vec![
                key(K::KEY_S, true),
                key(K::KEY_S, false),
                key(K::KEY_X, true)
            ]
        );
        assert_eq!(
            clock.step(300, &[]),
            vec![
                key(K::KEY_R, true),
                key(K::KEY_R, false),
                key(K::KEY_X, false)
            ]
        );
    }

    #[test]
    fn test_activator_turbo() {
        let mut clock = Clock::new();
        let mut outputs = clock.step(0, &[Button::Y]);
        // Reports every 4 ms for just under a second: down at 0, 100, .. 900
        for at in (4..992).step_by(4) {
            outputs.extend(clock.step(at, &[Button::Y]));
        }
        outputs.extend(clock.step(992, &[]));
        let presses = outputs
            .iter()
            .filter(|o| **o == key(K::KEY_Y, true))
            .count();
        let releases = outputs
            .iter()
            .filter(|o| **o == key(K::KEY_Y, false))
            .count();
        assert_eq!(presses, 10);
        assert_eq!(releases, 10);
        assert_eq!(outputs.last(), Some(&key(K::KEY_Y, false)));
    }

    #[test]
    fn test_stick_directions() {
        let config = Config::parse(
            "[profile.p.buttons]\nlstick_up = \"key:KEY_U\"\n\
             [[profile.p.chord]]\nhold = \"grip_l2\"\n\
             lstick_down = { press = \"key:KEY_D\", turbo_ms = 100 }",
        )
        .unwrap();
        let mut mapper = Mapper::new(quiet(config.get("p").unwrap()));
        let start = Instant::now();
        let mut prev = ControllerState::default();
        let mut step = |at: u64, buttons: &[Button], y: i16| {
            let mut state = pressed(buttons);
            state.buttons = state.buttons.with(Button::LStickTouch);
            state.lstick.y = y;
            let outputs = mapper.map_at(&prev, &state, start + Duration::from_millis(at));
            prev = state;
            // The stick itself still drives the virtual pad
            outputs
                .into_iter()
                .filter(|output| matches!(output, Output::Key(..)))
                .collect::<Vec<_>>()
        };
        let full = i16::MAX;
        // Pressed past the push, held down to the release, released below it
        assert_eq!(step(0, &[], full / 4), vec![]);
        assert_eq!(step(4, &[], full), vec![key(K::KEY_U, true)]);
        assert_eq!(step(8, &[], full / 20 * 9), vec![]);
        assert_eq!(step(12, &[], full / 4), vec![key(K::KEY_U, false)]);
        // Under the chord, held down repeats
        let mut outputs = step(16, &[Button::GripL2], 0);
        for at in (20..400).step_by(4) {
            outputs.extend(step(at, &[Button::GripL2], -full));
        }
        outputs.extend(step(400, &[Button::GripL2], 0));
        let presses = outputs
            .iter()
            .filter(|o| **o == key(K::KEY_D, true))
            .count();
        assert_eq!(presses, 4);
        assert_eq!(outputs.last(), Some(&key(K::KEY_D, false)));
    }

    #[test]
    fn test_activator_release_all() {
        let mut clock = Clock::new();
        clock.step(0, &[Button::A, Button::X]);
        clock.step(300, &[Button::A, Button::B, Button::X]);
        clock.step(310, &[Button::A, Button::X]);
        assert_eq!(
            clock.step(400, &[Button::A, Button::X]),
            vec![key(K::KEY_L, true)]
        );
        let mut outputs = clock.mapper.release_all();
        outputs.sort_by_key(|output| format!("{output:?}"));
        assert_eq!(outputs, vec![key(K::KEY_L, false), key(K::KEY_X, false)]);
        // The pending tap of b is gone
        assert_eq!(clock.step(2000, &[]), vec![]);
    }

    #[test]
    fn test_activator_chord_member() {
        let config = Config::parse(
            "[[profile.p.chord]]\nhold = \"view\"\na = { press = \"key:KEY_A\", long = \"key:KEY_L\" }",
        )
        .unwrap();
        let mut clock = Clock::new();
        clock.mapper = Mapper::new(config.get("p").unwrap());
        assert_eq!(clock.step(0, &[Button::View]), vec![]);
        assert_eq!(clock.step(10, &[Button::View, Button::A]), vec![]);
        assert_eq!(
            clock.step(410, &[Button::View, Button::A]),
            vec![key(K::KEY_L, true)]
        );
    }

    #[test]
    fn test_activator_profile_slots() {
        let config = Config::parse(
            "[profile.p.buttons]\ny = { press = \"key:KEY_Y\", long = \"profile:next\" }\n\
             [[profile.p.chord]]\nhold = \"view\"\na = { double = \"profile:prev\" }",
        )
        .unwrap();
        let mut clock = Clock::new();
        clock.mapper = Mapper::new(config.get("p").unwrap());
        assert_eq!(clock.step(0, &[Button::Y]), vec![]);
        assert_eq!(clock.step(500, &[Button::Y]), vec![]);
        assert_eq!(
            clock.step(600, &[]),
            vec![Output::Profile(ProfileSwitch::Next)]
        );
        let err = Config::parse("[profile.p.buttons]\ny = { start = \"profile:nope\" }");
        assert!(err.is_err());
    }

    #[test]
    fn test_chord_touch_hold() {
        let config = Config::parse(
            "[profile.p.buttons]\nmenu = \"key:KEY_F\"\n\
             [[profile.p.chord]]\nhold = \"view\"\ndpad_up = \"key:KEY_VOLUMEUP\"\n\
             [[profile.p.chord]]\nhold = \"grip_r_touch\"\n\
             menu = { press = \"key:KEY_F\", long = \"tv:off\", long_ms = 3000 }",
        )
        .unwrap();
        let mut clock = Clock::new();
        clock.mapper = Mapper::new(quiet(config.get("p").unwrap()));
        let grip = Button::GripRTouch;
        assert_eq!(clock.step(0, &[grip]), vec![]);
        // A short tap of Start while gripped is still Start, on release
        assert_eq!(clock.step(10, &[grip, Button::Menu]), vec![]);
        assert_eq!(
            clock.step(100, &[grip]),
            vec![key(K::KEY_F, true), key(K::KEY_F, false)]
        );
        // Held 3 s, the TV goes off
        assert_eq!(clock.step(200, &[grip, Button::Menu]), vec![]);
        assert_eq!(clock.step(3300, &[grip, Button::Menu]), vec![Output::TvOff]);
        assert_eq!(clock.step(3400, &[grip]), vec![]);
        // Another chord's hold still works under the grip
        assert_eq!(clock.step(3500, &[grip, Button::View]), vec![]);
        assert_eq!(
            clock.step(3600, &[grip, Button::View, Button::DpadUp]),
            vec![key(K::KEY_VOLUMEUP, true)]
        );
        assert_eq!(clock.step(3700, &[grip]), vec![key(K::KEY_VOLUMEUP, false)]);
        // Without the grip, Start is plain Start again
        assert_eq!(clock.step(3800, &[]), vec![]);
        assert_eq!(clock.step(3900, &[Button::Menu]), vec![key(K::KEY_F, true)]);
    }

    /// A stick direction a held chord binds does not also drive the stick's own target.
    #[test]
    fn test_chord_claims_its_stick() {
        let config = Config::parse(
            "[profile.p.axes]\nlstick = \"keys:KEY_W,KEY_S,KEY_A,KEY_D\"\n\
             [[profile.p.chord]]\nhold = \"lb\"\nlstick_up = \"key:KEY_VOLUMEUP\"",
        )
        .unwrap();
        let mut mapper = Mapper::new(quiet(config.get("p").unwrap()));
        let now = Instant::now();
        let lb = pressed(&[Button::Lb, Button::LStickTouch]);
        let mut lb_up = lb;
        lb_up.lstick = Stick { x: 0, y: i16::MAX };
        let mut up = pressed(&[Button::LStickTouch]);
        up.lstick = lb_up.lstick;
        assert_eq!(mapper.map_at(&ControllerState::default(), &lb, now), vec![]);
        assert_eq!(
            mapper.map_at(&lb, &lb_up, now),
            vec![key(K::KEY_VOLUMEUP, true)]
        );
        assert_eq!(
            mapper.map_at(&lb_up, &lb, now),
            vec![key(K::KEY_VOLUMEUP, false)]
        );
        // Without the chord the stick is its own again
        assert_eq!(mapper.map_at(&lb, &ControllerState::default(), now), vec![]);
        assert_eq!(
            mapper.map_at(&ControllerState::default(), &up, now),
            vec![key(K::KEY_W, true)]
        );
    }

    /// Releasing everything (menu, lizard, wake) keeps a chord whose hold stays down.
    #[test]
    fn test_chord_survives_release_all() {
        let config = Config::parse(
            "[[profile.p.chord]]\nhold = \"grip_r_touch\"\n\
             menu = { press = \"key:KEY_F\", long = \"tv:off\", long_ms = 3000 }",
        )
        .unwrap();
        let mut clock = Clock::new();
        clock.mapper = Mapper::new(quiet(config.get("p").unwrap()));
        let grip = Button::GripRTouch;
        assert_eq!(clock.step(0, &[grip]), vec![]);
        assert_eq!(clock.mapper.release_all(), vec![]);
        assert_eq!(clock.step(100, &[grip]), vec![]);
        assert_eq!(clock.step(200, &[grip, Button::Menu]), vec![]);
        assert_eq!(clock.step(3300, &[grip, Button::Menu]), vec![Output::TvOff]);
        // Its release taps nothing of its own
        assert_eq!(clock.step(3400, &[]), vec![]);
    }

    #[test]
    fn test_target_tv_off() {
        let target: Target = "tv:off".parse().unwrap();
        assert_eq!(target, Target::TvOff);
        assert_eq!(target.to_string(), "tv:off");
        let config = Config::parse("[daemon]\ntv_off = \"a\"\ntv_on = \"b\"").unwrap();
        assert_eq!(config.daemon.tv_off.as_deref(), Some("a"));
        assert_eq!(config.daemon.tv_on.as_deref(), Some("b"));
    }
}
