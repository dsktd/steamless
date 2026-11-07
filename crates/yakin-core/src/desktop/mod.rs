//! Desktop control behind one small trait: run an action, report the focused window.
//!
//! A backend implements [`Desktop`]; everything else (which backend, reconnecting, logging,
//! focus rules) is shared. [`Action`] is the vocabulary every backend maps from.

use anyhow::anyhow;
use bincode::Decode;
use bincode::Encode;
use serde::Deserialize;
use serde::Serialize;

use crate::Result;

mod niri;
pub use niri::Niri;

/// A desktop the pad drives.
pub trait Desktop: Send {
    /// Run one action; Ok(false) when this desktop has no such action.
    fn act(
        &mut self,
        action: &Action,
    ) -> Result<bool>;

    /// Report every focus change (None = nothing focused) until the connection ends; blocks.
    fn watch(
        &mut self,
        focus: &mut dyn FnMut(Option<Window>),
    ) -> Result<()>;

    fn name(&self) -> &'static str {
        "desktop"
    }
}

/// The focused window.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Window {
    pub app_id: String,
    pub title: String,
}

/// What [`Desktop::name`] answers with no desktop to drive.
pub const NONE: &str = "none";

/// Spawn and workspace actions take an argument: `spawn:fuzzel`, `workspace:2`.
const SPAWN: &str = "spawn:";
const WORKSPACE: &str = "workspace:";

/// What a `desktop:` target asks for, named alike on every desktop.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    Overview,
    CloseWindow,
    Fullscreen,
    Maximize,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    WorkspaceUp,
    WorkspaceDown,
    ScreenOff,
    Screenshot,
    /// A workspace by number, from 1.
    Workspace(u8),
    /// A command line, split on whitespace.
    Spawn(String),
    /// A niri action as `niri:<name>[:arg]` names it, for what no common action covers.
    Raw(String),
}

/// The actions without an argument, by their config name.
pub const NAMES: [(&str, Action); 16] = [
    ("overview", Action::Overview),
    ("close-window", Action::CloseWindow),
    ("fullscreen", Action::Fullscreen),
    ("maximize", Action::Maximize),
    ("focus-left", Action::FocusLeft),
    ("focus-right", Action::FocusRight),
    ("focus-up", Action::FocusUp),
    ("focus-down", Action::FocusDown),
    ("move-left", Action::MoveLeft),
    ("move-right", Action::MoveRight),
    ("move-up", Action::MoveUp),
    ("move-down", Action::MoveDown),
    ("workspace-up", Action::WorkspaceUp),
    ("workspace-down", Action::WorkspaceDown),
    ("screen-off", Action::ScreenOff),
    ("screenshot", Action::Screenshot),
];

impl Action {
    /// The action of a `desktop:` target's name: `overview`, `workspace:2` or `spawn:fuzzel`.
    pub fn parse(name: &str) -> Result<Self> {
        if let Some(command) = name.strip_prefix(SPAWN) {
            if command.trim().is_empty() {
                return Err(anyhow!("desktop:{name} spawns no command"));
            }
            return Ok(Action::Spawn(command.to_string()));
        }
        if let Some(number) = name.strip_prefix(WORKSPACE) {
            return match number.parse::<u8>() {
                Ok(number) if number > 0 => Ok(Action::Workspace(number)),
                _ => Err(anyhow!(
                    "desktop:{name}: workspace is a number from 1 to 255"
                )),
            };
        }
        NAMES
            .iter()
            .find(|(known, _)| *known == name)
            .map(|(_, action)| action.clone())
            .ok_or(anyhow!("Unknown desktop action {name}"))
    }

    /// The action of a `niri:` target's name, checked against niri's own list.
    pub fn raw(name: &str) -> Result<Self> {
        niri::NiriAction::parse(name)?;
        Ok(Action::Raw(name.to_string()))
    }

    /// Names `niri:<name>` accepts bare, for editors.
    pub fn raw_names() -> Vec<String> {
        niri::niri_names()
    }
}
impl std::fmt::Display for Action {
    /// The target that names it: `desktop:overview`, `niri:toggle-overview`.
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        match self {
            Action::Workspace(number) => write!(f, "desktop:{WORKSPACE}{number}"),
            Action::Spawn(command) => write!(f, "desktop:{SPAWN}{command}"),
            Action::Raw(name) => write!(f, "niri:{name}"),
            action => {
                let (name, _) = NAMES
                    .iter()
                    .find(|(_, known)| known == action)
                    .expect("every bare action is named");
                write!(f, "desktop:{name}")
            }
        }
    }
}

/// `[desktop] backend`: which desktop the pad drives.
#[derive(Encode, Decode, Serialize, Deserialize, Copy, Clone, Debug, Default, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    /// The first desktop found running, looked for again on every use.
    #[default]
    Auto,
    Niri,
    /// No desktop: actions are unsupported, focus never moves.
    None,
}
impl Backend {
    /// The desktop this selects; detection runs again on every call, so a session that starts
    /// after the daemon is found.
    pub fn desktop(self) -> Box<dyn Desktop> {
        Box::new(Selected(self))
    }
}

/// A [`Backend`], resolved to a desktop per call.
struct Selected(Backend);
impl Selected {
    fn detect(&self) -> Option<Box<dyn Desktop>> {
        match self.0 {
            Backend::Niri => Some(Box::new(Niri)),
            // One line per backend, in order of preference
            Backend::Auto if niri::running() => Some(Box::new(Niri)),
            Backend::Auto | Backend::None => None,
        }
    }
}
impl Desktop for Selected {
    fn act(
        &mut self,
        action: &Action,
    ) -> Result<bool> {
        match self.detect() {
            Some(mut desktop) => desktop.act(action),
            None => Ok(false),
        }
    }

    fn watch(
        &mut self,
        focus: &mut dyn FnMut(Option<Window>),
    ) -> Result<()> {
        match self.detect() {
            Some(mut desktop) => desktop.watch(focus),
            None if self.0 == Backend::None => loop {
                std::thread::park();
            },
            None => Err(anyhow!("No desktop found")),
        }
    }

    fn name(&self) -> &'static str {
        self.detect().map_or(NONE, |desktop| desktop.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for (name, action) in NAMES {
            assert_eq!(Action::parse(name).unwrap(), action);
            assert_eq!(action.to_string(), format!("desktop:{name}"));
        }
        let cases = [
            ("workspace:3", Action::Workspace(3)),
            (
                "spawn:foot -e top",
                Action::Spawn("foot -e top".to_string()),
            ),
        ];
        for (name, action) in cases {
            assert_eq!(Action::parse(name).unwrap(), action);
            assert_eq!(action.to_string(), format!("desktop:{name}"));
        }
    }

    #[test]
    fn bad_names_fail() {
        for name in [
            "toggle-overview",
            "workspace:0",
            "workspace:256",
            "workspace:web",
            "spawn: ",
            "",
        ] {
            assert!(Action::parse(name).is_err(), "{name}");
        }
    }

    #[test]
    fn raw_is_checked_against_niri() {
        let raw = Action::raw("switch-preset-column-width").unwrap();
        assert_eq!(raw.to_string(), "niri:switch-preset-column-width");
        assert!(Action::raw("fly").is_err());
        assert!(Action::raw("focus-workspace").is_err());
    }

    #[test]
    fn none_supports_nothing() {
        let mut desktop = Backend::None.desktop();
        assert_eq!(desktop.name(), NONE);
        assert!(!desktop.act(&Action::Overview).unwrap());
    }
}
