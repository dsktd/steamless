//! TOML config, profiles and the mapper from controller state to outputs.

use std::collections::BTreeMap;
use std::path::Path;
use std::str::FromStr;

use anyhow::anyhow;
use serde::Deserialize;

use crate::Result;
use crate::output::Output;
use crate::output::PadButton;
use crate::output::PadStick;
use crate::output::PadTrigger;
use crate::state::Button;
use crate::state::ControllerState;
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
}
impl Axis {
    pub const fn is_trigger(self) -> bool {
        matches!(self, Axis::Lt | Axis::Rt)
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
        Ok(match name {
            "lstick" => Source::Axis(Axis::LStick),
            "rstick" => Source::Axis(Axis::RStick),
            "lpad" => Source::Axis(Axis::LPad),
            "rpad" => Source::Axis(Axis::RPad),
            "lt" => Source::Axis(Axis::Lt),
            "rt" => Source::Axis(Axis::Rt),
            name => Source::Button(name.parse()?),
        })
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Target {
    Button(PadButton),
    Stick(PadStick),
    Trigger(PadTrigger),
    /// Evdev key code on the virtual keyboard.
    Key(u16),
    None,
}
impl FromStr for Target {
    type Err = anyhow::Error;

    fn from_str(target: &str) -> Result<Self> {
        if target == "none" {
            return Ok(Target::None);
        }
        if let Some(name) = target.strip_prefix("key:") {
            let key = evdev::KeyCode::from_str(name).or(Err(anyhow!("Unknown key {name}")))?;
            return Ok(Target::Key(key.0));
        }
        let Some(name) = target.strip_prefix("pad:") else {
            return Err(anyhow!("Unknown target {target}"));
        };
        Ok(match name {
            "south" => Target::Button(PadButton::South),
            "east" => Target::Button(PadButton::East),
            "west" => Target::Button(PadButton::West),
            "north" => Target::Button(PadButton::North),
            "lb" => Target::Button(PadButton::Lb),
            "rb" => Target::Button(PadButton::Rb),
            "select" => Target::Button(PadButton::Select),
            "start" => Target::Button(PadButton::Start),
            "guide" => Target::Button(PadButton::Guide),
            "thumbl" => Target::Button(PadButton::ThumbL),
            "thumbr" => Target::Button(PadButton::ThumbR),
            "dpad_up" => Target::Button(PadButton::DpadUp),
            "dpad_down" => Target::Button(PadButton::DpadDown),
            "dpad_left" => Target::Button(PadButton::DpadLeft),
            "dpad_right" => Target::Button(PadButton::DpadRight),
            "left" => Target::Stick(PadStick::Left),
            "right" => Target::Stick(PadStick::Right),
            "lt" => Target::Trigger(PadTrigger::Lt),
            "rt" => Target::Trigger(PadTrigger::Rt),
            _ => return Err(anyhow!("Unknown pad target {name}")),
        })
    }
}

/// While `hold` is down, `buttons` map to their chord targets and `hold` emits nothing.
#[derive(Clone, Debug, PartialEq)]
pub struct Chord {
    pub hold: Button,
    pub buttons: BTreeMap<Button, Target>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub deadzone_stick: f32,
    pub deadzone_trigger: f32,
    pub buttons: BTreeMap<Button, Target>,
    pub axes: BTreeMap<Axis, Target>,
    pub chords: Vec<Chord>,
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
        ]);
        let axes = BTreeMap::from([
            (Axis::LStick, Target::Stick(PadStick::Left)),
            (Axis::RStick, Target::Stick(PadStick::Right)),
            (Axis::Lt, Target::Trigger(PadTrigger::Lt)),
            (Axis::Rt, Target::Trigger(PadTrigger::Rt)),
        ]);
        Self {
            deadzone_stick: 0.10,
            deadzone_trigger: 0.05,
            buttons,
            axes,
            chords: Vec::new(),
        }
    }

    /// Key codes any binding of this profile can send.
    pub fn keys(&self) -> Vec<u16> {
        let chords = self.chords.iter().flat_map(|chord| chord.buttons.values());
        self.buttons
            .values()
            .chain(chords)
            .filter_map(|target| match target {
                Target::Key(code) => Some(*code),
                _ => None,
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
        for (source, target) in &toml.buttons {
            let (button, target) = button_binding(source, target)?;
            profile.buttons.insert(button, target);
        }
        for (source, target) in &toml.axes {
            let Source::Axis(axis) = source.parse()? else {
                return Err(anyhow!("{source} is a button, not an axis"));
            };
            let target: Target = target.parse()?;
            match (axis.is_trigger(), target) {
                (_, Target::None) | (true, Target::Trigger(_)) | (false, Target::Stick(_)) => {}
                _ => return Err(anyhow!("Cannot map axis {source} to {target:?}")),
            }
            profile.axes.insert(axis, target);
        }
        for chord in toml.chord {
            let Source::Button(hold) = chord.hold.parse()? else {
                return Err(anyhow!("Chord hold {} is not a button", chord.hold));
            };
            let mut buttons = BTreeMap::new();
            for (source, target) in &chord.buttons {
                let (button, target) = button_binding(source, target)?;
                buttons.insert(button, target);
            }
            profile.chords.push(Chord { hold, buttons });
        }
        Ok(profile)
    }
}

fn button_binding(
    source: &str,
    target: &str,
) -> Result<(Button, Target)> {
    let Source::Button(button) = source.parse()? else {
        return Err(anyhow!("{source} is an axis, not a button"));
    };
    let target: Target = target.parse()?;
    if let Target::Stick(_) = target {
        return Err(anyhow!("Cannot map button {source} to a stick"));
    }
    Ok((button, target))
}

#[derive(Deserialize, Copy, Clone, Debug, Default, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Lizard {
    #[default]
    Off,
    On,
    Auto,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Daemon {
    pub active: Option<String>,
    pub lizard: Lizard,
    pub imu: bool,
}

/// Name of the built-in Xbox layout.
pub const PAD: &str = "pad";

/// A whole config file. The default has no profiles and runs the Xbox layout.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config {
    pub daemon: Daemon,
    pub profiles: BTreeMap<String, Profile>,
}
impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .or(Err(anyhow!("Cannot read config {}", path.display())))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self> {
        let toml: ConfigToml = toml::from_str(text)?;
        let mut profiles = BTreeMap::new();
        for (name, profile) in toml.profile {
            let profile = match Profile::try_from(profile) {
                Ok(profile) => profile,
                Err(err) => return Err(anyhow!("Profile {name}: {err}")),
            };
            profiles.insert(name, profile);
        }
        Ok(Self {
            daemon: Daemon {
                active: toml.daemon.active,
                lizard: toml.daemon.lizard,
                imu: toml.daemon.imu,
            },
            profiles,
        })
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

    /// Key codes any profile can send.
    pub fn keys(&self) -> Vec<u16> {
        self.profiles.values().flat_map(Profile::keys).collect()
    }
}

// TODO serde is the one departure from hand-rolled parsing; these mirror the file only
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct ConfigToml {
    daemon: DaemonToml,
    profile: BTreeMap<String, ProfileToml>,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct DaemonToml {
    active: Option<String>,
    lizard: Lizard,
    imu: bool,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct ProfileToml {
    layout: Option<String>,
    deadzone: DeadzoneToml,
    buttons: BTreeMap<String, String>,
    axes: BTreeMap<String, String>,
    chord: Vec<ChordToml>,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct DeadzoneToml {
    stick: Option<f32>,
    trigger: Option<f32>,
}

#[derive(Deserialize)]
struct ChordToml {
    hold: String,
    #[serde(flatten)]
    buttons: BTreeMap<String, String>,
}

/// Turns state changes into outputs for one controller.
pub struct Mapper {
    profile: Profile,
    /// Index of the chord whose hold is down.
    chord: Option<usize>,
    /// A chord member was pressed since the hold went down.
    chord_used: bool,
    /// What each pressed button emitted, so its release matches.
    pressed: BTreeMap<Button, Target>,
}
impl Mapper {
    pub fn new(profile: Profile) -> Self {
        Self {
            profile,
            chord: None,
            chord_used: false,
            pressed: BTreeMap::new(),
        }
    }

    pub fn map(
        &mut self,
        prev: &ControllerState,
        state: &ControllerState,
    ) -> Vec<Output> {
        let mut outputs = Vec::new();
        for button in Button::ALL {
            let was = prev.buttons.contains(button);
            let is = state.buttons.contains(button);
            if !was && is {
                self.press(button, &mut outputs);
            } else if was && !is {
                self.release(button, &mut outputs);
            }
        }
        for (axis, target) in &self.profile.axes {
            let before = self.axis(*axis, *target, prev);
            let after = self.axis(*axis, *target, state);
            if before != after
                && let Some(output) = after
            {
                outputs.push(output);
            }
        }
        outputs
    }

    /// Release everything still held, without the late tap of a chord hold.
    pub fn release_all(&mut self) -> Vec<Output> {
        let mut outputs = Vec::new();
        self.chord = None;
        for target in std::mem::take(&mut self.pressed).into_values() {
            emit(target, false, &mut outputs);
        }
        outputs
    }

    /// Swap the profile, returning the releases of everything the old one held and centred axes.
    pub fn activate(
        &mut self,
        profile: Profile,
    ) -> Vec<Output> {
        let mut outputs = self.release_all();
        for target in self.profile.axes.values() {
            match *target {
                Target::Stick(stick) => outputs.push(Output::Stick(stick, 0, 0)),
                Target::Trigger(trigger) => outputs.push(Output::Trigger(trigger, 0)),
                _ => {}
            }
        }
        self.profile = profile;
        outputs
    }

    fn press(
        &mut self,
        button: Button,
        outputs: &mut Vec<Output>,
    ) {
        // A held chord takes its members
        if let Some(index) = self.chord
            && let Some(target) = self.profile.chords[index].buttons.get(&button)
        {
            self.chord_used = true;
            self.pressed.insert(button, *target);
            emit(*target, true, outputs);
            return;
        }
        // Chord holds emit nothing until released
        if self.chord.is_none()
            && let Some(index) = self
                .profile
                .chords
                .iter()
                .position(|chord| chord.hold == button)
        {
            self.chord = Some(index);
            self.chord_used = false;
            self.pressed.insert(button, Target::None);
            return;
        }
        let target = self
            .profile
            .buttons
            .get(&button)
            .copied()
            .unwrap_or(Target::None);
        self.pressed.insert(button, target);
        emit(target, true, outputs);
    }

    fn release(
        &mut self,
        button: Button,
        outputs: &mut Vec<Output>,
    ) {
        let target = self.pressed.remove(&button).unwrap_or(Target::None);
        if let Some(index) = self.chord
            && self.profile.chords[index].hold == button
        {
            self.chord = None;
            // Tapped alone: the hold's own button, late
            if !self.chord_used {
                let target = self
                    .profile
                    .buttons
                    .get(&button)
                    .copied()
                    .unwrap_or(Target::None);
                emit(target, true, outputs);
                emit(target, false, outputs);
            }
            return;
        }
        emit(target, false, outputs);
    }

    fn axis(
        &self,
        axis: Axis,
        target: Target,
        state: &ControllerState,
    ) -> Option<Output> {
        match target {
            Target::Stick(stick) => {
                let (x, y) = match axis {
                    Axis::LStick => stick_value(state.lstick),
                    Axis::RStick => stick_value(state.rstick),
                    Axis::LPad => pad_value(state.lpad),
                    Axis::RPad => pad_value(state.rpad),
                    Axis::Lt | Axis::Rt => return None,
                };
                let (x, y) = deadzone_stick(x, y, self.profile.deadzone_stick);
                Some(Output::Stick(stick, x, y.saturating_neg()))
            }
            Target::Trigger(trigger) => {
                let value = match axis {
                    Axis::Lt => state.lt,
                    Axis::Rt => state.rt,
                    Axis::LStick | Axis::RStick | Axis::LPad | Axis::RPad => return None,
                };
                Some(Output::Trigger(
                    trigger,
                    deadzone_trigger(value, self.profile.deadzone_trigger),
                ))
            }
            Target::Button(_) | Target::Key(_) | Target::None => None,
        }
    }
}

fn emit(
    target: Target,
    pressed: bool,
    outputs: &mut Vec<Output>,
) {
    match target {
        Target::Button(button) => outputs.push(Output::Button(button, pressed)),
        Target::Key(code) => outputs.push(Output::Key(code, pressed)),
        Target::Trigger(trigger) => outputs.push(Output::Trigger(trigger, pressed as u8 * 255)),
        Target::Stick(_) | Target::None => {}
    }
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
        active = "settop"

        [profile.settop.buttons]
        steam = "key:KEY_HOMEPAGE"

        [[profile.settop.chord]]
        hold = "view"
        dpad_up = "key:KEY_VOLUMEUP"
        dpad_down = "key:KEY_VOLUMEDOWN"
        dpad_left = "key:KEY_MUTE"
    "#;

    fn pressed(buttons: &[Button]) -> ControllerState {
        let mut state = ControllerState::default();
        for button in buttons {
            state.buttons = state.buttons.with(*button);
        }
        state
    }

    fn settop() -> Mapper {
        Mapper::new(Config::parse(SETTOP).unwrap().profile().unwrap())
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
        assert_eq!(
            mapper.map(&idle, &steam),
            vec![Output::Key(evdev::KeyCode::KEY_HOMEPAGE.0, true)]
        );
    }

    #[test]
    fn test_mapper_stick() {
        let mut mapper = Mapper::new(Profile::xbox());
        let idle = ControllerState::default();
        let mut moved = idle;
        moved.lstick = Stick { x: 0, y: 20000 };
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
        assert_eq!(mapper.map(&idle, &drift), vec![]);
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
        assert_eq!(
            mapper.map(&idle, &steam),
            vec![Output::Key(evdev::KeyCode::KEY_HOMEPAGE.0, true)]
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
        assert!(
            Config::parse("[daemon]\nactive = \"missing\"")
                .unwrap()
                .profile()
                .is_err()
        );
    }

    #[test]
    fn test_config_example_settop() {
        let config = Config::parse(include_str!("../../../examples/settop.toml")).unwrap();
        let profile = config.profile().unwrap();
        assert_eq!(profile, Config::parse(SETTOP).unwrap().profile().unwrap());
        assert_eq!(config.daemon.lizard, Lizard::Off);
    }
}
