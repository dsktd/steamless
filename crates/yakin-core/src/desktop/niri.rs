//! The niri backend: actions and the focused window over niri's IPC socket.
//!
//! One JSON request per line, `{"Action":{"FocusColumnLeft":{}}}`; niri answers one line.
//! After `"EventStream"` niri answers `{"Ok":"Handled"}`, then sends one event per line, as
//! niri-ipc 25.11's `Event`; only the window events matter here.

use std::collections::BTreeMap;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::anyhow;
use serde::Deserialize;

use super::Action;
use super::Desktop;
use super::Window;
use crate::Result;

/// Every action of niri 25.11, as `niri msg action --help` lists them (less `help`): a
/// `niri:<name>` target sends `{"<PascalCase>":{...}}`, the fields from [`FIELDS`] or none.
pub const NIRI_ACTIONS: [&str; 140] = [
    "quit",
    "power-off-monitors",
    "power-on-monitors",
    "spawn",
    "spawn-sh",
    "do-screen-transition",
    "screenshot",
    "screenshot-screen",
    "screenshot-window",
    "toggle-keyboard-shortcuts-inhibit",
    "close-window",
    "fullscreen-window",
    "toggle-windowed-fullscreen",
    "focus-window",
    "focus-window-in-column",
    "focus-window-previous",
    "focus-column-left",
    "focus-column-right",
    "focus-column-first",
    "focus-column-last",
    "focus-column-right-or-first",
    "focus-column-left-or-last",
    "focus-column",
    "focus-window-or-monitor-up",
    "focus-window-or-monitor-down",
    "focus-column-or-monitor-left",
    "focus-column-or-monitor-right",
    "focus-window-down",
    "focus-window-up",
    "focus-window-down-or-column-left",
    "focus-window-down-or-column-right",
    "focus-window-up-or-column-left",
    "focus-window-up-or-column-right",
    "focus-window-or-workspace-down",
    "focus-window-or-workspace-up",
    "focus-window-top",
    "focus-window-bottom",
    "focus-window-down-or-top",
    "focus-window-up-or-bottom",
    "move-column-left",
    "move-column-right",
    "move-column-to-first",
    "move-column-to-last",
    "move-column-left-or-to-monitor-left",
    "move-column-right-or-to-monitor-right",
    "move-column-to-index",
    "move-window-down",
    "move-window-up",
    "move-window-down-or-to-workspace-down",
    "move-window-up-or-to-workspace-up",
    "consume-or-expel-window-left",
    "consume-or-expel-window-right",
    "consume-window-into-column",
    "expel-window-from-column",
    "swap-window-right",
    "swap-window-left",
    "toggle-column-tabbed-display",
    "set-column-display",
    "center-column",
    "center-window",
    "center-visible-columns",
    "focus-workspace-down",
    "focus-workspace-up",
    "focus-workspace",
    "focus-workspace-previous",
    "move-window-to-workspace-down",
    "move-window-to-workspace-up",
    "move-window-to-workspace",
    "move-column-to-workspace-down",
    "move-column-to-workspace-up",
    "move-column-to-workspace",
    "move-workspace-down",
    "move-workspace-up",
    "move-workspace-to-index",
    "set-workspace-name",
    "unset-workspace-name",
    "focus-monitor-left",
    "focus-monitor-right",
    "focus-monitor-down",
    "focus-monitor-up",
    "focus-monitor-previous",
    "focus-monitor-next",
    "focus-monitor",
    "move-window-to-monitor-left",
    "move-window-to-monitor-right",
    "move-window-to-monitor-down",
    "move-window-to-monitor-up",
    "move-window-to-monitor-previous",
    "move-window-to-monitor-next",
    "move-window-to-monitor",
    "move-column-to-monitor-left",
    "move-column-to-monitor-right",
    "move-column-to-monitor-down",
    "move-column-to-monitor-up",
    "move-column-to-monitor-previous",
    "move-column-to-monitor-next",
    "move-column-to-monitor",
    "set-window-width",
    "set-window-height",
    "reset-window-height",
    "switch-preset-column-width",
    "switch-preset-column-width-back",
    "switch-preset-window-width",
    "switch-preset-window-width-back",
    "switch-preset-window-height",
    "switch-preset-window-height-back",
    "maximize-column",
    "maximize-window-to-edges",
    "set-column-width",
    "expand-column-to-available-width",
    "switch-layout",
    "show-hotkey-overlay",
    "move-workspace-to-monitor-left",
    "move-workspace-to-monitor-right",
    "move-workspace-to-monitor-down",
    "move-workspace-to-monitor-up",
    "move-workspace-to-monitor-previous",
    "move-workspace-to-monitor-next",
    "move-workspace-to-monitor",
    "toggle-debug-tint",
    "debug-toggle-opaque-regions",
    "debug-toggle-damage",
    "toggle-window-floating",
    "move-window-to-floating",
    "move-window-to-tiling",
    "focus-floating",
    "focus-tiling",
    "switch-focus-between-floating-and-tiling",
    "move-floating-window",
    "toggle-window-rule-opacity",
    "set-dynamic-cast-window",
    "set-dynamic-cast-monitor",
    "clear-dynamic-cast-target",
    "toggle-overview",
    "open-overview",
    "close-overview",
    "toggle-window-urgent",
    "set-window-urgent",
    "unset-window-urgent",
    "load-config-file",
];

/// Fields niri-ipc 25.11 cannot do without (only its `Option` fields may be left out), as a
/// bare `niri:<name>` sends them.
const FIELDS: [(&str, &str); 8] = [
    // Niri's own dialog asks before quitting
    ("quit", r#""skip_confirmation":false"#),
    ("screenshot", r#""show_pointer":true"#),
    (
        "screenshot-screen",
        r#""write_to_disk":true,"show_pointer":true"#,
    ),
    ("screenshot-window", r#""write_to_disk":true"#),
    ("move-window-to-workspace-down", r#""focus":true"#),
    ("move-window-to-workspace-up", r#""focus":true"#),
    ("move-column-to-workspace-down", r#""focus":true"#),
    ("move-column-to-workspace-up", r#""focus":true"#),
];

/// Actions written `niri:<name>:<arg>`, and what their argument is.
const ARGS: [(&str, Arg); 7] = [
    ("spawn-sh", Arg::Command),
    ("focus-workspace", Arg::Workspace),
    ("move-column-to-workspace", Arg::WorkspaceFocus),
    ("move-window-to-workspace", Arg::WorkspaceFocus),
    ("set-column-width", Arg::Size),
    ("set-window-width", Arg::Size),
    ("set-window-height", Arg::Size),
];

/// Actions with a field no target form fills, refused at load rather than by niri.
const UNSUPPORTED: [&str; 17] = [
    "spawn",
    "focus-window",
    "focus-window-in-column",
    "focus-column",
    "move-column-to-index",
    "set-column-display",
    "move-workspace-to-index",
    "set-workspace-name",
    "focus-monitor",
    "move-window-to-monitor",
    "move-column-to-monitor",
    "switch-layout",
    "move-workspace-to-monitor",
    "move-floating-window",
    "toggle-window-urgent",
    "set-window-urgent",
    "unset-window-urgent",
];

/// The `niri:` target prefix of a spawn: `niri:spawn:fuzzel`, arguments split on whitespace.
pub const SPAWN: &str = "spawn:";

/// The argument of an [`ARGS`] action, in niri's own CLI syntax.
#[derive(Copy, Clone, Debug)]
enum Arg {
    /// A shell command line: `spawn-sh:wpctl set-mute @DEFAULT_SINK@ toggle`.
    Command,
    /// A workspace index, else its name: `focus-workspace:3`.
    Workspace,
    /// A workspace, focus following the moved window or column.
    WorkspaceFocus,
    /// Pixels or percent, set or `+`/`-` adjusted: `set-column-width:+10%`.
    Size,
}
impl Arg {
    /// The action's fields, without braces.
    fn fields(
        self,
        arg: &str,
    ) -> Result<String> {
        if arg.is_empty() {
            return Err(anyhow!("empty argument"));
        }
        match self {
            Arg::Command => Ok(format!(r#""command":{}"#, quote(arg))),
            Arg::Workspace => Ok(format!(r#""reference":{}"#, workspace(arg)?)),
            Arg::WorkspaceFocus => Ok(format!(r#""reference":{},"focus":true"#, workspace(arg)?)),
            Arg::Size => Ok(format!(r#""change":{}"#, size(arg)?)),
        }
    }
}

/// A `WorkspaceReferenceArg`: a number is an index (0-255), anything else a name.
fn workspace(arg: &str) -> Result<String> {
    let Ok(index) = arg.parse::<i32>() else {
        return Ok(format!(r#"{{"Name":{}}}"#, quote(arg)));
    };
    match u8::try_from(index) {
        Ok(index) => Ok(format!(r#"{{"Index":{index}}}"#)),
        Err(_) => Err(anyhow!("workspace index {index} is not within 0 to 255")),
    }
}

/// A `SizeChange`: `640` and `50%` set, `+40` and `-10%` adjust.
fn size(arg: &str) -> Result<String> {
    let adjust = arg.starts_with(['+', '-']);
    let bad = || anyhow!("{arg} is not a size: 640, 50%, +40 or -10%");
    match (arg.strip_suffix('%'), adjust) {
        (Some(percent), _) => {
            let value: f64 = percent.parse().map_err(|_| bad())?;
            if !value.is_finite() {
                return Err(bad());
            }
            let variant = if adjust {
                "AdjustProportion"
            } else {
                "SetProportion"
            };
            Ok(format!(r#"{{"{variant}":{value:?}}}"#))
        }
        (None, adjust) => {
            let value: i32 = arg.parse().map_err(|_| bad())?;
            let variant = if adjust { "AdjustFixed" } else { "SetFixed" };
            Ok(format!(r#"{{"{variant}":{value}}}"#))
        }
    }
}

/// `focus-column-left` as niri-ipc's variant, `FocusColumnLeft`.
fn pascal(name: &str) -> String {
    name.split('-')
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|first| first.to_ascii_uppercase().to_string() + chars.as_str())
                .unwrap_or_default()
        })
        .collect()
}

/// Names a bare `niri:<name>` accepts, for editors: no argument needed.
pub fn niri_names() -> Vec<String> {
    NIRI_ACTIONS
        .iter()
        .filter(|name| !UNSUPPORTED.contains(name) && !ARGS.iter().any(|(known, _)| known == *name))
        .map(|name| name.to_string())
        .collect()
}

/// What a `niri:` binding sends on press.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NiriAction {
    /// Index into [`NIRI_ACTIONS`], the argument of a `name:arg` target, and the fields sent.
    Named {
        index: usize,
        arg: Option<String>,
        fields: String,
    },
    Spawn(Vec<String>),
}
impl NiriAction {
    /// The action of a `niri:` target's name: `focus-column-left`, `focus-workspace:3` or
    /// `spawn:fuzzel`.
    pub fn parse(name: &str) -> Result<Self> {
        if let Some(command) = name.strip_prefix(SPAWN) {
            let argv: Vec<String> = command.split_whitespace().map(String::from).collect();
            if argv.is_empty() {
                return Err(anyhow!("niri:{name} spawns no command"));
            }
            return Ok(NiriAction::Spawn(argv));
        }
        let (name, arg) = match name.split_once(':') {
            Some((name, arg)) => (name, Some(arg)),
            None => (name, None),
        };
        let Some(index) = NIRI_ACTIONS.iter().position(|known| *known == name) else {
            return Err(anyhow!("Unknown niri action {name}"));
        };
        let kind = ARGS
            .iter()
            .find(|(known, _)| *known == name)
            .map(|(_, kind)| *kind);
        let fields = match (kind, arg) {
            (Some(kind), Some(arg)) => kind
                .fields(arg)
                .map_err(|err| anyhow!("niri:{name}:{arg}: {err}"))?,
            (Some(_), None) => {
                return Err(anyhow!("niri:{name} needs an argument, niri:{name}:<arg>"));
            }
            (None, Some(_)) => return Err(anyhow!("niri:{name} takes no argument")),
            (None, None) if UNSUPPORTED.contains(&name) => {
                return Err(anyhow!("niri:{name} needs fields yakin cannot send"));
            }
            (None, None) => FIELDS
                .iter()
                .find(|(known, _)| *known == name)
                .map_or(String::new(), |(_, fields)| fields.to_string()),
        };
        Ok(NiriAction::Named {
            index,
            arg: arg.map(String::from),
            fields,
        })
    }

    /// The request line, without its newline.
    pub fn request(&self) -> String {
        let action = match self {
            NiriAction::Named { index, fields, .. } => {
                format!(r#"{{"{}":{{{fields}}}}}"#, pascal(NIRI_ACTIONS[*index]))
            }
            NiriAction::Spawn(argv) => {
                let argv: Vec<String> = argv.iter().map(|arg| quote(arg)).collect();
                format!(r#"{{"Spawn":{{"command":[{}]}}}}"#, argv.join(","))
            }
        };
        format!(r#"{{"Action":{action}}}"#)
    }
}
impl std::fmt::Display for NiriAction {
    fn fmt(
        &self,
        f: &mut std::fmt::Formatter<'_>,
    ) -> std::fmt::Result {
        match self {
            NiriAction::Named {
                index,
                arg: Some(arg),
                ..
            } => write!(f, "{}:{arg}", NIRI_ACTIONS[*index]),
            NiriAction::Named { index, .. } => write!(f, "{}", NIRI_ACTIONS[*index]),
            NiriAction::Spawn(argv) => write!(f, "{SPAWN}{}", argv.join(" ")),
        }
    }
}

/// A JSON string literal.
fn quote(text: &str) -> String {
    let mut quoted = String::from('"');
    for char in text.chars() {
        match char {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            char if (char as u32) < 0x20 => quoted.push_str(&format!("\\u{:04x}", char as u32)),
            char => quoted.push(char),
        }
    }
    quoted.push('"');
    quoted
}

/// How long niri gets to answer an action.
const TIMEOUT: Duration = Duration::from_millis(500);

/// Niri, over its socket: a connection per call, so a restarted niri is found again.
pub struct Niri;
impl Desktop for Niri {
    fn act(
        &mut self,
        action: &Action,
    ) -> Result<bool> {
        request(&NiriAction::parse(&name(action))?)?;
        Ok(true)
    }

    fn watch(
        &mut self,
        focus: &mut dyn FnMut(Option<Window>),
    ) -> Result<()> {
        let mut stream = connect()?;
        stream.write_all(b"\"EventStream\"\n")?;
        let mut windows = Windows::default();
        for line in BufReader::new(stream).lines() {
            if let Some(window) = windows.line(&line?) {
                focus(window);
            }
        }
        Err(anyhow!("niri closed the event stream"))
    }

    fn name(&self) -> &'static str {
        "niri"
    }
}

/// The niri action a common one maps to, as a `niri:` target names it.
fn name(action: &Action) -> String {
    let name = match action {
        Action::Overview => "toggle-overview",
        Action::CloseWindow => "close-window",
        Action::Fullscreen => "fullscreen-window",
        Action::Maximize => "maximize-column",
        Action::FocusLeft => "focus-column-left",
        Action::FocusRight => "focus-column-right",
        Action::FocusUp => "focus-window-up",
        Action::FocusDown => "focus-window-down",
        Action::MoveLeft => "move-column-left",
        Action::MoveRight => "move-column-right",
        Action::MoveUp => "move-window-up",
        Action::MoveDown => "move-window-down",
        Action::WorkspaceUp => "focus-workspace-up",
        Action::WorkspaceDown => "focus-workspace-down",
        Action::ScreenOff => "power-off-monitors",
        Action::Screenshot => "screenshot",
        Action::Workspace(number) => return format!("focus-workspace:{number}"),
        Action::Spawn(command) => return format!("{SPAWN}{command}"),
        Action::Raw(name) => return name.clone(),
    };
    name.to_string()
}

/// Whether a niri session is there to drive.
pub fn running() -> bool {
    niri_socket().is_some()
}

fn connect() -> Result<UnixStream> {
    let path = niri_socket().ok_or(anyhow!(
        "no niri socket: NIRI_SOCKET unset, none in /run/user"
    ))?;
    UnixStream::connect(&path).map_err(|err| anyhow!("Cannot connect to {}: {err}", path.display()))
}

fn request(action: &NiriAction) -> Result<()> {
    let mut stream = connect()?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    stream.write_all(format!("{}\n", action.request()).as_bytes())?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply)?;
    if !reply.starts_with(r#"{"Ok""#) {
        return Err(anyhow!("niri answered {}", reply.trim_end()));
    }
    Ok(())
}

/// `$NIRI_SOCKET`, else the newest `niri*.sock` in this user's `/run/user/<uid>`: the system
/// service runs as the session user but without the session's environment.
fn niri_socket() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("NIRI_SOCKET") {
        return Some(path.into());
    }
    let dir = PathBuf::from(format!("/run/user/{}", unsafe { nix::libc::getuid() }));
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with("niri") && name.ends_with(".sock")
        })
        .max_by_key(|entry| entry.metadata().and_then(|meta| meta.modified()).ok())
        .map(|entry| entry.path())
}

/// The fields of niri's `Window` used here.
#[derive(Deserialize, Clone, Debug)]
struct NiriWindow {
    id: u64,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    app_id: Option<String>,
    #[serde(default)]
    is_focused: bool,
}

/// The events of niri's `Event` used here; any other is skipped.
#[derive(Deserialize, Debug)]
enum NiriEvent {
    WindowsChanged { windows: Vec<NiriWindow> },
    WindowOpenedOrChanged { window: NiriWindow },
    WindowClosed { id: u64 },
    WindowFocusChanged { id: Option<u64> },
}

/// Niri's windows and which has focus, as the stream tells them.
#[derive(Default)]
struct Windows {
    windows: BTreeMap<u64, NiriWindow>,
    focused: Option<u64>,
    /// The last focus reported, `Some(None)` for no window.
    last: Option<Option<Window>>,
}
impl Windows {
    /// Take one line of the stream; the focus when it changed, title included.
    fn line(
        &mut self,
        line: &str,
    ) -> Option<Option<Window>> {
        // Unknown variants and the Ok line fail to parse and are skipped
        let event: NiriEvent = serde_json::from_str(line).ok()?;
        match event {
            NiriEvent::WindowsChanged { windows } => {
                self.focused = windows
                    .iter()
                    .find(|window| window.is_focused)
                    .map(|window| window.id);
                self.windows = windows
                    .into_iter()
                    .map(|window| (window.id, window))
                    .collect();
            }
            NiriEvent::WindowOpenedOrChanged { window } => {
                if window.is_focused {
                    self.focused = Some(window.id);
                } else if self.focused == Some(window.id) {
                    self.focused = None;
                }
                self.windows.insert(window.id, window);
            }
            NiriEvent::WindowClosed { id } => {
                self.windows.remove(&id);
                if self.focused == Some(id) {
                    self.focused = None;
                }
            }
            NiriEvent::WindowFocusChanged { id } => self.focused = id,
        }
        let focus = self
            .focused
            .and_then(|id| self.windows.get(&id))
            .map(|window| Window {
                app_id: window.app_id.clone().unwrap_or_default(),
                title: window.title.clone().unwrap_or_default(),
            });
        if self.last.as_ref() == Some(&focus) {
            return None;
        }
        self.last = Some(focus.clone());
        Some(focus)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_actions_map_to_niri() {
        let table = [
            ("overview", "toggle-overview"),
            ("close-window", "close-window"),
            ("fullscreen", "fullscreen-window"),
            ("maximize", "maximize-column"),
            ("focus-left", "focus-column-left"),
            ("focus-right", "focus-column-right"),
            ("focus-up", "focus-window-up"),
            ("focus-down", "focus-window-down"),
            ("move-left", "move-column-left"),
            ("move-right", "move-column-right"),
            ("move-up", "move-window-up"),
            ("move-down", "move-window-down"),
            ("workspace-up", "focus-workspace-up"),
            ("workspace-down", "focus-workspace-down"),
            ("screen-off", "power-off-monitors"),
            ("screenshot", "screenshot"),
            ("workspace:2", "focus-workspace:2"),
            ("spawn:fuzzel -l 5", "spawn:fuzzel -l 5"),
        ];
        assert_eq!(table.len(), super::super::NAMES.len() + 2);
        for (common, niri) in table {
            let mapped = name(&Action::parse(common).unwrap());
            assert_eq!(mapped, niri, "{common}");
            assert!(NiriAction::parse(&mapped).is_ok(), "{common}");
        }
        assert_eq!(name(&Action::raw("quit").unwrap()), "quit");
    }

    #[test]
    fn names_map_to_pascal_case() {
        assert_eq!(pascal("focus-column-left"), "FocusColumnLeft");
        assert_eq!(pascal("quit"), "Quit");
        assert_eq!(
            pascal("switch-focus-between-floating-and-tiling"),
            "SwitchFocusBetweenFloatingAndTiling"
        );
    }

    #[test]
    fn fieldless_action() {
        let action = NiriAction::parse("focus-column-left").unwrap();
        assert_eq!(action.request(), r#"{"Action":{"FocusColumnLeft":{}}}"#);
        assert_eq!(action.to_string(), "focus-column-left");
    }

    #[test]
    fn optional_fields_are_left_out_required_ones_sent() {
        let close = NiriAction::parse("close-window").unwrap();
        assert_eq!(close.request(), r#"{"Action":{"CloseWindow":{}}}"#);
        let quit = NiriAction::parse("quit").unwrap();
        assert_eq!(
            quit.request(),
            r#"{"Action":{"Quit":{"skip_confirmation":false}}}"#
        );
    }

    #[test]
    fn argument_actions() {
        let cases = [
            (
                "focus-workspace:3",
                r#"{"FocusWorkspace":{"reference":{"Index":3}}}"#,
            ),
            (
                "focus-workspace:web",
                r#"{"FocusWorkspace":{"reference":{"Name":"web"}}}"#,
            ),
            (
                "move-column-to-workspace:2",
                r#"{"MoveColumnToWorkspace":{"reference":{"Index":2},"focus":true}}"#,
            ),
            (
                "set-column-width:+10%",
                r#"{"SetColumnWidth":{"change":{"AdjustProportion":10.0}}}"#,
            ),
            (
                "set-window-height:50%",
                r#"{"SetWindowHeight":{"change":{"SetProportion":50.0}}}"#,
            ),
            (
                "set-column-width:-40",
                r#"{"SetColumnWidth":{"change":{"AdjustFixed":-40}}}"#,
            ),
            ("spawn-sh:a && b", r#"{"SpawnSh":{"command":"a && b"}}"#),
        ];
        for (name, json) in cases {
            let action = NiriAction::parse(name).unwrap();
            assert_eq!(
                action.request(),
                format!(r#"{{"Action":{json}}}"#),
                "{name}"
            );
            assert_eq!(action.to_string(), name);
        }
    }

    #[test]
    fn every_listed_name_parses_or_needs_an_argument() {
        for name in NIRI_ACTIONS {
            let bare = NiriAction::parse(name).is_ok();
            let takes_arg = ARGS.iter().any(|(known, _)| *known == name);
            assert_eq!(bare, !takes_arg && !UNSUPPORTED.contains(&name), "{name}");
        }
        let named = FIELDS.iter().map(|(name, _)| name);
        for name in named.chain(ARGS.iter().map(|(name, _)| name)) {
            assert!(NIRI_ACTIONS.contains(name), "{name}");
        }
    }

    #[test]
    fn spawn_splits_and_quotes_its_command() {
        let action = NiriAction::parse(r#"spawn:foot -e "x\"#).unwrap();
        assert_eq!(
            action.request(),
            r#"{"Action":{"Spawn":{"command":["foot","-e","\"x\\"]}}}"#
        );
        assert_eq!(action.to_string(), r#"spawn:foot -e "x\"#);
    }

    #[test]
    fn unknown_and_empty_fail() {
        assert!(NiriAction::parse("focus-sideways").is_err());
        assert!(NiriAction::parse("spawn:").is_err());
        assert!(NiriAction::parse("FocusColumnLeft").is_err());
        assert!(NiriAction::parse("focus-workspace").is_err());
        assert!(NiriAction::parse("focus-workspace:").is_err());
        assert!(NiriAction::parse("focus-workspace:300").is_err());
        assert!(NiriAction::parse("set-column-width:wide").is_err());
        assert!(NiriAction::parse("close-window:3").is_err());
        assert!(NiriAction::parse("focus-monitor").is_err());
    }

    fn focus(
        app_id: &str,
        title: &str,
    ) -> Option<Option<Window>> {
        Some(Some(Window {
            app_id: app_id.to_string(),
            title: title.to_string(),
        }))
    }

    /// A niri 25.11 `Window`, every field as niri sends it.
    fn window(
        id: u64,
        app_id: &str,
        title: &str,
        focused: bool,
    ) -> String {
        format!(
            r#"{{"id":{id},"title":"{title}","app_id":"{app_id}","pid":1000,"workspace_id":1,"is_focused":{focused},"is_floating":false,"is_urgent":false,"layout":{{"pos_in_scrolling_layout":[1,1],"tile_size":[960.0,1080.0],"window_size":[960,1080],"tile_pos_in_workspace_view":null,"window_offset_in_tile":[0.0,0.0]}},"focus_timestamp":null}}"#
        )
    }

    #[test]
    fn follows_the_stream() {
        let mut windows = Windows::default();
        assert_eq!(windows.line(r#"{"Ok":"Handled"}"#), None);
        assert_eq!(
            windows.line(r#"{"WorkspacesChanged":{"workspaces":[]}}"#),
            None
        );
        let all = format!(
            r#"{{"WindowsChanged":{{"windows":[{},{}]}}}}"#,
            window(1, "firefox", "News", true),
            window(2, "foot", "~", false)
        );
        assert_eq!(windows.line(&all), focus("firefox", "News"));
        // Same focus again: nothing new
        assert_eq!(windows.line(&all), None);
        assert_eq!(
            windows.line(r#"{"WindowFocusChanged":{"id":2}}"#),
            focus("foot", "~")
        );
        let opened = format!(
            r#"{{"WindowOpenedOrChanged":{{"window":{}}}}}"#,
            window(3, "steam_app_1234", "Game", true)
        );
        assert_eq!(windows.line(&opened), focus("steam_app_1234", "Game"));
        // A title change of the focused window
        let renamed = format!(
            r#"{{"WindowOpenedOrChanged":{{"window":{}}}}}"#,
            window(3, "steam_app_1234", "Game - paused", true)
        );
        assert_eq!(
            windows.line(&renamed),
            focus("steam_app_1234", "Game - paused")
        );
        // An unfocused window changing is not news
        let other = format!(
            r#"{{"WindowOpenedOrChanged":{{"window":{}}}}}"#,
            window(1, "firefox", "Mail", false)
        );
        assert_eq!(windows.line(&other), None);
        assert_eq!(windows.line(r#"{"WindowClosed":{"id":3}}"#), Some(None));
        assert_eq!(
            windows.line(r#"{"WindowFocusChanged":{"id":1}}"#),
            focus("firefox", "Mail")
        );
        assert_eq!(
            windows.line(r#"{"WindowFocusChanged":{"id":null}}"#),
            Some(None)
        );
    }

    #[test]
    fn null_app_id_is_empty() {
        let mut windows = Windows::default();
        let line = r#"{"WindowOpenedOrChanged":{"window":{"id":7,"title":null,"app_id":null,"pid":null,"workspace_id":null,"is_focused":true,"is_floating":true,"is_urgent":false,"layout":{}}}}"#;
        assert_eq!(windows.line(line), focus("", ""));
    }
}
