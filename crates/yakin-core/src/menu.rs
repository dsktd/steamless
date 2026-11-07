//! The quick menu: a grid of targets the daemon drives from the controller while it is open,
//! and the overlay draws.

use std::time::Duration;
use std::time::Instant;

use anyhow::anyhow;
use bincode::Decode;
use bincode::Encode;
use serde::Deserialize;
use serde::Serialize;

use crate::Result;
use crate::profile::Target;
use crate::state::Button;
use crate::state::ControllerState;
use crate::state::Stick;

/// Glyphs the overlay draws, by the name a `[menu]` item gives as its `icon`.
pub const ICONS: [&str; 14] = [
    "apps",
    "keyboard",
    "overview",
    "fullscreen",
    "maximize",
    "close",
    "volume-up",
    "volume-down",
    "mute",
    "gamepad",
    "mouse",
    "screenshot",
    "display-off",
    "power",
];

/// Most columns a menu may have.
const COLUMNS_MAX: u8 = 8;

/// The menu when the config has no `[menu]`: label, icon, target.
const DEFAULT_ITEMS: [(&str, &str, &str); 12] = [
    ("Launcher", "apps", "desktop:spawn:fuzzel"),
    ("Keyboard", "keyboard", "keyboard:toggle"),
    ("Overview", "overview", "desktop:overview"),
    ("Fullscreen", "fullscreen", "desktop:fullscreen"),
    ("Maximize", "maximize", "desktop:maximize"),
    ("Close", "close", "desktop:close-window"),
    ("Volume up", "volume-up", "key:KEY_VOLUMEUP"),
    ("Volume down", "volume-down", "key:KEY_VOLUMEDOWN"),
    ("Mute", "mute", "key:KEY_MUTE"),
    ("Pad mode", "gamepad", "profile:pad"),
    ("Desktop mode", "mouse", "profile:desktop"),
    ("Screen off", "display-off", "desktop:screen-off"),
];

/// Columns of the default menu.
const DEFAULT_COLUMNS: u8 = 4;

/// Stick deflection, share of full, past which it moves the focus...
const STICK_PUSH: f32 = 0.5;

/// ...and below which it lets go, so one push at the edge moves once.
const STICK_RELEASE: f32 = 0.4;

/// A held stick repeats its move after this long...
const REPEAT_DELAY: Duration = Duration::from_millis(400);

/// ...then this often.
const REPEAT_EVERY: Duration = Duration::from_millis(150);

/// Share of travel past which the right trigger activates.
const TRIGGER_PULL: f32 = 0.5;

/// What a `menu:` binding does.
#[derive(Encode, Decode, Copy, Clone, Debug, Eq, PartialEq)]
pub enum MenuCommand {
    Toggle,
    Open,
    Close,
}
impl MenuCommand {
    /// Whether the menu should be open afterwards, or `None` when `open` already is that.
    pub const fn change(
        self,
        open: bool,
    ) -> Option<bool> {
        match (self, open) {
            (Self::Toggle | Self::Open, false) => Some(true),
            (Self::Toggle | Self::Close, true) => Some(false),
            _ => None,
        }
    }

    pub fn parse(name: &str) -> Result<Self> {
        match name {
            "toggle" => Ok(MenuCommand::Toggle),
            "open" => Ok(MenuCommand::Open),
            "close" => Ok(MenuCommand::Close),
            _ => Err(anyhow!("Unknown menu command {name}")),
        }
    }

    pub const NAMES: [&str; 3] = ["toggle", "open", "close"];
}
impl std::fmt::Display for MenuCommand {
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        let name = match self {
            MenuCommand::Toggle => "toggle",
            MenuCommand::Open => "open",
            MenuCommand::Close => "close",
        };
        write!(f, "{name}")
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MenuItem {
    pub label: String,
    /// One of [`ICONS`].
    pub icon: String,
    /// A button-like target, fired as a press and release.
    pub target: Target,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

/// The grid, row by row, and the focused item.
#[derive(Clone, Debug, PartialEq)]
pub struct Menu {
    pub items: Vec<MenuItem>,
    pub columns: usize,
    pub focus: usize,
}
impl Default for Menu {
    fn default() -> Self {
        let toml = MenuToml {
            items: DEFAULT_ITEMS
                .iter()
                .map(|(label, icon, target)| MenuItemToml {
                    label: label.to_string(),
                    icon: icon.to_string(),
                    target: target.to_string(),
                })
                .collect(),
            columns: DEFAULT_COLUMNS,
        };
        Menu::try_from(&toml).unwrap()
    }
}
impl Menu {
    /// Move the focus one cell, wrapping around each edge; whether it moved.
    pub fn move_focus(
        &mut self,
        direction: Direction,
    ) -> bool {
        let count = self.items.len();
        let columns = self.columns.max(1);
        let rows = count.div_ceil(columns);
        if count == 0 {
            return false;
        }
        let (row, column) = (self.focus / columns, self.focus % columns);
        let focus = match direction {
            Direction::Left | Direction::Right => {
                let width = columns.min(count - row * columns);
                let column = match direction {
                    Direction::Left => (column + width - 1) % width,
                    _ => (column + 1) % width,
                };
                row * columns + column
            }
            // A short last row is skipped where it has no cell in this column
            Direction::Up | Direction::Down => {
                let step = match direction {
                    Direction::Up => rows - 1,
                    _ => 1,
                };
                let mut row = (row + step) % rows;
                while row * columns + column >= count {
                    row = (row + step) % rows;
                }
                row * columns + column
            }
        };
        let moved = focus != self.focus;
        self.focus = focus;
        moved
    }

    /// The focused item's target.
    pub fn activate(&self) -> Option<&Target> {
        self.items.get(self.focus).map(|item| &item.target)
    }

    /// Every target, for checks across the config.
    pub fn targets(&self) -> impl Iterator<Item = &Target> {
        self.items.iter().map(|item| &item.target)
    }
}
impl TryFrom<&MenuToml> for Menu {
    type Error = anyhow::Error;

    fn try_from(toml: &MenuToml) -> Result<Self> {
        if toml.items.is_empty() {
            return Err(anyhow!("menu.items is empty"));
        }
        if !(1..=COLUMNS_MAX).contains(&toml.columns) {
            return Err(anyhow!("menu.columns is not within 1 to {COLUMNS_MAX}"));
        }
        let items = toml
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let at = |err: anyhow::Error| anyhow!("menu.items.{index}: {err}");
                if !ICONS.contains(&item.icon.as_str()) {
                    return Err(at(anyhow!("Unknown icon {}", item.icon)));
                }
                let target: Target = item.target.parse().map_err(at)?;
                if !target.is_button_like() || matches!(target, Target::Menu(_)) {
                    return Err(at(anyhow!("A menu item cannot fire {target}")));
                }
                Ok(MenuItem {
                    label: item.label.clone(),
                    icon: item.icon.clone(),
                    target,
                })
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            items,
            columns: toml.columns as usize,
            focus: 0,
        })
    }
}

/// `[menu]` as written.
#[derive(Encode, Decode, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct MenuToml {
    pub items: Vec<MenuItemToml>,
    pub columns: u8,
}
impl Default for MenuToml {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            columns: DEFAULT_COLUMNS,
        }
    }
}

#[derive(Encode, Decode, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MenuItemToml {
    pub label: String,
    pub icon: String,
    pub target: String,
}

/// What the controller asks of an open menu.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum MenuInput {
    Move(Direction),
    Activate,
    Close,
}

/// Reads one controller while the menu is open: D-pad and left stick move, A, the right pad
/// click and the right trigger activate, B closes. A held stick repeats.
#[derive(Clone, Debug, Default)]
pub struct MenuNav {
    /// The direction the stick holds and when it moves again.
    held: Option<(Direction, Instant)>,
}
impl MenuNav {
    /// What changed from `prev` to `state`; `close` are the buttons bound to close the menu.
    pub fn read(
        &mut self,
        prev: &ControllerState,
        state: &ControllerState,
        close: &[Button],
        now: Instant,
    ) -> Vec<MenuInput> {
        let pressed =
            |button: Button| !prev.buttons.contains(button) && state.buttons.contains(button);
        let mut inputs = Vec::new();
        for (button, direction) in [
            (Button::DpadUp, Direction::Up),
            (Button::DpadDown, Direction::Down),
            (Button::DpadLeft, Direction::Left),
            (Button::DpadRight, Direction::Right),
        ] {
            if pressed(button) {
                inputs.push(MenuInput::Move(direction));
            }
        }
        let stick = stick_direction(state.lstick, self.held.map(|(direction, _)| direction));
        match (stick, self.held) {
            (None, _) => self.held = None,
            (Some(direction), Some((held, at))) if direction == held => {
                if now >= at {
                    inputs.push(MenuInput::Move(direction));
                    self.held = Some((direction, now + REPEAT_EVERY));
                }
            }
            (Some(direction), _) => {
                inputs.push(MenuInput::Move(direction));
                self.held = Some((direction, now + REPEAT_DELAY));
            }
        }
        let pull = TRIGGER_PULL * i16::MAX as f32;
        let pulled = (prev.rt as f32) < pull && state.rt as f32 >= pull;
        if pressed(Button::A) || pressed(Button::RPadClick) || pulled {
            inputs.push(MenuInput::Activate);
        }
        if pressed(Button::B) || close.iter().any(|button| pressed(*button)) {
            inputs.push(MenuInput::Close);
        }
        inputs
    }
}

/// The stick's dominant direction past [`STICK_PUSH`]; `held` stays down to [`STICK_RELEASE`].
pub(crate) fn stick_direction(
    stick: Stick,
    held: Option<Direction>,
) -> Option<Direction> {
    let (x, y) = (stick.x as f32, stick.y as f32);
    let length = (x * x + y * y).sqrt() / i16::MAX as f32;
    let threshold = match held {
        Some(_) => STICK_RELEASE,
        None => STICK_PUSH,
    };
    if length < threshold {
        return None;
    }
    let direction = match (x.abs() > y.abs(), x > 0.0, y > 0.0) {
        (true, true, _) => Direction::Right,
        (true, false, _) => Direction::Left,
        (false, _, true) => Direction::Up,
        (false, _, false) => Direction::Down,
    };
    // Below the push, a held stick keeps its direction rather than turning
    match held {
        Some(held) if length < STICK_PUSH => Some(held),
        _ => Some(direction),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_change() {
        assert_eq!(MenuCommand::Toggle.change(false), Some(true));
        assert_eq!(MenuCommand::Toggle.change(true), Some(false));
        assert_eq!(MenuCommand::Open.change(true), None);
        assert_eq!(MenuCommand::Close.change(false), None);
    }
    use crate::state::Buttons;
    use crate::state::Stick;

    fn grid(
        count: usize,
        columns: usize,
    ) -> Menu {
        let item = MenuItem {
            label: String::new(),
            icon: "apps".to_string(),
            target: Target::None,
        };
        Menu {
            items: vec![item; count],
            columns,
            focus: 0,
        }
    }

    #[test]
    fn test_move_wraps() {
        let mut menu = grid(12, 4);
        assert!(menu.move_focus(Direction::Left));
        assert_eq!(menu.focus, 3);
        menu.move_focus(Direction::Right);
        assert_eq!(menu.focus, 0);
        menu.move_focus(Direction::Up);
        assert_eq!(menu.focus, 8);
        menu.move_focus(Direction::Down);
        assert_eq!(menu.focus, 0);
        menu.move_focus(Direction::Down);
        menu.move_focus(Direction::Right);
        assert_eq!(menu.focus, 5);
    }

    #[test]
    fn test_move_skips_missing_cells() {
        // Two rows: 0 1 2 / 3 4
        let mut menu = grid(5, 3);
        menu.focus = 2;
        menu.move_focus(Direction::Down);
        assert_eq!(menu.focus, 2);
        menu.focus = 4;
        menu.move_focus(Direction::Right);
        assert_eq!(menu.focus, 3);
        menu.move_focus(Direction::Left);
        assert_eq!(menu.focus, 4);
        assert!(!grid(1, 4).move_focus(Direction::Down));
    }

    #[test]
    fn test_activate_returns_the_focused_target() {
        let mut menu = Menu::default();
        assert_eq!(menu.items.len(), 12);
        assert_eq!(menu.activate().unwrap().to_string(), "desktop:spawn:fuzzel");
        menu.move_focus(Direction::Up);
        assert_eq!(menu.activate().unwrap().to_string(), "key:KEY_MUTE");
    }

    #[test]
    fn test_items_are_checked() {
        let item = |icon: &str, target: &str| MenuToml {
            items: vec![MenuItemToml {
                label: "x".to_string(),
                icon: icon.to_string(),
                target: target.to_string(),
            }],
            columns: 4,
        };
        assert!(Menu::try_from(&item("apps", "key:KEY_A")).is_ok());
        let err = Menu::try_from(&item("cat", "key:KEY_A")).unwrap_err();
        assert_eq!(err.to_string(), "menu.items.0: Unknown icon cat");
        assert!(Menu::try_from(&item("apps", "menu:close")).is_err());
        assert!(Menu::try_from(&item("apps", "cursor")).is_err());
        assert!(Menu::try_from(&item("apps", "niri:fly")).is_err());
        let mut wide = item("apps", "none");
        wide.columns = 0;
        assert!(Menu::try_from(&wide).is_err());
    }

    fn state(
        buttons: &[Button],
        lstick: (i16, i16),
        rt: u16,
    ) -> ControllerState {
        ControllerState {
            buttons: Buttons::from(buttons.iter().map(|button| button.mask()).sum::<u64>()),
            lstick: Stick {
                x: lstick.0,
                y: lstick.1,
            },
            rt,
            ..ControllerState::default()
        }
    }

    #[test]
    fn test_nav_buttons() {
        let mut nav = MenuNav::default();
        let now = Instant::now();
        let idle = state(&[], (0, 0), 0);
        let read =
            |nav: &mut MenuNav, to: &ControllerState| nav.read(&idle, to, &[Button::Quick], now);
        assert_eq!(
            read(&mut nav, &state(&[Button::DpadLeft], (0, 0), 0)),
            vec![MenuInput::Move(Direction::Left)]
        );
        assert_eq!(
            read(&mut nav, &state(&[Button::A], (0, 0), 0)),
            vec![MenuInput::Activate]
        );
        assert_eq!(
            read(&mut nav, &state(&[], (0, 0), i16::MAX as u16)),
            vec![MenuInput::Activate]
        );
        assert_eq!(
            read(&mut nav, &state(&[Button::Quick], (0, 0), 0)),
            vec![MenuInput::Close]
        );
        // Held, not pressed: nothing
        let held = state(&[Button::A], (0, 0), 0);
        assert!(nav.read(&held, &held, &[], now).is_empty());
    }

    #[test]
    fn test_nav_stick_repeats() {
        let mut nav = MenuNav::default();
        let start = Instant::now();
        let idle = state(&[], (0, 0), 0);
        let up = state(&[], (0, 30000), 0);
        let at = |ms: u64| start + Duration::from_millis(ms);
        let push = MenuInput::Move(Direction::Up);
        assert_eq!(nav.read(&idle, &up, &[], at(0)), vec![push]);
        assert!(nav.read(&up, &up, &[], at(100)).is_empty());
        assert_eq!(nav.read(&up, &up, &[], at(400)), vec![push]);
        assert!(nav.read(&up, &up, &[], at(500)).is_empty());
        assert_eq!(nav.read(&up, &up, &[], at(550)), vec![push]);
        // Easing off to between release and push keeps holding, letting go stops
        let soft = state(&[], (0, 14000), 0);
        assert!(nav.read(&up, &soft, &[], at(600)).is_empty());
        assert!(nav.read(&soft, &idle, &[], at(700)).is_empty());
        assert_eq!(nav.read(&idle, &up, &[], at(710)), vec![push]);
    }
}
