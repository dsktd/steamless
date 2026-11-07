//! Requests and responses, u32 big-endian length + bincode on the wire.

use std::io::Read;
use std::io::Write;

use anyhow::anyhow;
use bincode::Decode;
use bincode::Encode;
use serde::Deserialize;
use serde::Serialize;
use yakin_core::keyboard::Keyboard;
use yakin_core::menu::Menu;
use yakin_core::menu::MenuCommand;
use yakin_core::profile::ConfigToml;
use yakin_core::profile::Vocabulary;
use yakin_core::state::ControllerState;

use crate::Result;

/// Larger frames are refused rather than allocated.
const MAX_FRAME: u32 = 1 << 20;

#[derive(Encode, Decode, Clone, Debug, PartialEq)]
pub enum IpcRequest {
    Status,
    /// Re-read every config layer and re-apply the active profile.
    Reload,
    /// Switch every controller to the named profile.
    Activate(String),
    /// Answered with `Ok`, then the connection streams `IpcEvent`s until the client closes it.
    Subscribe,
    /// Change the knobs of a profile at once; the daemon writes them to the editor drop-in
    /// once they rest.
    Tune(IpcTuning),
    /// The config layers and their merge, answered with `Config`.
    GetConfig,
    /// Write what this effective config changes over the other layers to the editor's drop-in,
    /// after validating the result, and apply it as `Reload` does.
    SetConfig(ConfigToml),
    /// The names a binding editor offers, answered with `Vocabulary`.
    Vocabulary,
    /// Take the controller back now from whoever the daemon yielded to.
    Reclaim,
    /// Open, close or toggle the quick menu, as a `menu:` binding does.
    Menu(MenuCommand),
    /// Open, close or toggle the on-screen keyboard, as a `keyboard:` binding does.
    Keyboard(MenuCommand),
    /// Power every controller down and idle as after a `tv:off` press, without touching the TV:
    /// for when the TV went to standby by its own remote.
    ControllerOff,
}

#[derive(Encode, Decode, Clone, Debug, PartialEq)]
pub enum IpcResponse {
    Ok,
    Status(IpcStatus),
    Config(Box<IpcConfig>),
    Vocabulary(Vocabulary),
    Error(String),
}

/// The daemon's config layers and what they add up to.
#[derive(Encode, Decode, Clone, Debug, PartialEq, Serialize)]
pub struct IpcConfig {
    /// The baseline file, when it exists.
    pub baseline: Option<IpcLayer>,
    /// The drop-ins in the order they apply, the editor's own included.
    pub dropins: Vec<IpcLayer>,
    pub effective: ConfigToml,
    /// The drop-in `SetConfig` writes.
    pub editor_path: String,
    pub writable: bool,
}

/// One config file as written.
#[derive(Encode, Decode, Clone, Debug, PartialEq, Serialize)]
pub struct IpcLayer {
    pub path: String,
    pub toml: ConfigToml,
}

#[derive(Encode, Decode, Clone, Debug, Default, PartialEq)]
pub struct IpcStatus {
    pub profile: String,
    pub controllers: Vec<IpcController>,
    /// Every profile name, sorted, `pad` included.
    pub profiles: Vec<String>,
    /// The current knobs of each of `profiles`, in the same order.
    pub tunings: Vec<IpcTuning>,
    /// The process the daemon stepped aside for, while it has the controller.
    pub yielded: Option<IpcHolder>,
    /// What `[auto]` makes of the focused window, while it is enabled.
    pub auto: Option<IpcAuto>,
    /// The desktop actions go to, `niri`; none when there is none to drive.
    pub desktop: Option<String>,
}

/// The focused window and the profile `[auto]` picks for it.
#[derive(Encode, Decode, Clone, Debug, PartialEq, Serialize)]
pub struct IpcAuto {
    /// Empty before the desktop reports a focus, or with no window focused.
    pub focused_app_id: String,
    /// Index of the matching `[[auto.rule]]`.
    pub rule: Option<usize>,
    /// The rule's profile, else `auto.default`.
    pub profile: Option<String>,
    /// A manual switch holds until the focus moves to another app.
    pub manual: bool,
}

/// Another process holding the controller open.
#[derive(Encode, Decode, Clone, Debug, PartialEq, Serialize)]
pub struct IpcHolder {
    pub pid: u32,
    pub comm: String,
}

/// The knobs of one profile that can change while it runs.
#[derive(Encode, Decode, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IpcTuning {
    pub profile: String,
    pub mouse_speed: f32,
    pub mouse_accel: f32,
    pub mouse_release_guard: u8,
    pub mouse_smoothing: f32,
    pub mouse_stick_speed: f32,
    pub mouse_stick_curve: f32,
    pub mouse_flick_degrees: f32,
    pub scroll_speed: f32,
    pub scroll_natural: bool,
    pub scroll_stick_speed: f32,
    pub deadzone_stick: f32,
    pub deadzone_trigger: f32,
    pub trigger_click: f32,
    /// `haptics.click`, 0 to 1.
    pub haptics_click: f32,
    /// `haptics.pad_ticks`, 0 to 1.
    pub haptics_pad_ticks: f32,
    /// `haptics.menu`, 0 to 1.
    pub haptics_menu: f32,
}

/// Streamed after `Subscribe`.
#[derive(Encode, Decode, Clone, Debug, PartialEq, Serialize)]
pub enum IpcEvent {
    /// The latest input of a controller, at most one per tick.
    State { id: u8, state: ControllerState },
    /// The active profile, on subscribe, activate and reload.
    Profile(String),
    /// Every open controller, on subscribe and whenever one changes.
    Controllers(Vec<IpcController>),
    /// A profile was tuned.
    Tuning(IpcTuning),
    /// The editor drop-in was written.
    Config(Box<IpcConfig>),
    /// The process the daemon yielded to, on subscribe and whenever it changes.
    Yielded(Option<IpcHolder>),
    /// The open quick menu, on subscribe, open, every focus change and close.
    Menu(Option<IpcMenu>),
    /// The open on-screen keyboard, on subscribe, open, every change and close.
    Keyboard(Option<IpcKeyboard>),
}

/// The quick menu as the overlay draws it.
#[derive(Encode, Decode, Clone, Debug, PartialEq, Serialize)]
pub struct IpcMenu {
    pub items: Vec<IpcMenuItem>,
    pub columns: u32,
    pub focus: u32,
}
impl From<&Menu> for IpcMenu {
    fn from(menu: &Menu) -> Self {
        Self {
            items: menu
                .items
                .iter()
                .map(|item| IpcMenuItem {
                    label: item.label.clone(),
                    icon: item.icon.clone(),
                })
                .collect(),
            columns: menu.columns as u32,
            focus: menu.focus as u32,
        }
    }
}

#[derive(Encode, Decode, Clone, Debug, PartialEq, Serialize)]
pub struct IpcMenuItem {
    pub label: String,
    /// One of `yakin_core::menu::ICONS`.
    pub icon: String,
}

/// The on-screen keyboard as the overlay draws it: its keys row by row under the current layer
/// and modifiers, and the indices, counted row by row, of the keys under each finger and the
/// D-pad's focus.
#[derive(Encode, Decode, Clone, Debug, PartialEq, Serialize)]
pub struct IpcKeyboard {
    pub layer: String,
    pub shift: bool,
    pub caps: bool,
    pub hover_left: Option<u32>,
    pub hover_right: Option<u32>,
    pub focus: u32,
    pub rows: Vec<Vec<IpcKey>>,
}
impl From<&Keyboard> for IpcKeyboard {
    fn from(keyboard: &Keyboard) -> Self {
        let rows = keyboard
            .layer
            .rows()
            .iter()
            .map(|row| {
                row.iter()
                    .map(|(key, span)| IpcKey {
                        label: keyboard.label(*key),
                        span: *span as u32,
                    })
                    .collect()
            })
            .collect();
        Self {
            layer: keyboard.layer.name().to_string(),
            shift: keyboard.shift,
            caps: keyboard.caps,
            hover_left: keyboard.hover_left.map(|cell| cell as u32),
            hover_right: keyboard.hover_right.map(|cell| cell as u32),
            focus: keyboard.focus as u32,
            rows,
        }
    }
}

#[derive(Encode, Decode, Clone, Debug, PartialEq, Serialize)]
pub struct IpcKey {
    pub label: String,
    /// Columns of the row's ten it spans.
    pub span: u32,
}

/// How a node reaches its controller.
#[derive(Encode, Decode, Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub enum IpcLink {
    /// A slot of the wireless dongle.
    #[default]
    Puck,
    Bluetooth,
    Wired,
}
impl IpcLink {
    pub fn name(self) -> &'static str {
        match self {
            IpcLink::Puck => "wireless",
            IpcLink::Bluetooth => "bluetooth",
            IpcLink::Wired => "wired",
        }
    }
}

#[derive(Encode, Decode, Clone, Debug, PartialEq, Serialize)]
pub struct IpcController {
    pub id: u8,
    pub path: String,
    /// Percent, once the controller has reported it.
    pub battery: Option<u8>,
    /// Behind a wireless dongle, once it has reported its link.
    pub wireless: bool,
    pub link: IpcLink,
    /// A controller is on the node: always on a wired or Bluetooth one, on a puck slot from
    /// its connect report or first input until its disconnect report.
    pub connected: bool,
    /// Whether its own keyboard and mouse are on, once the daemon has set the node up.
    pub lizard: Option<bool>,
    /// Bursts of its own keyboard and mouse input while lizard mode should have been off.
    pub leaks: u32,
    /// Handed to its own keyboard and mouse by a `lizard:` binding.
    pub mouse: bool,
}

pub fn write(
    mut writer: impl Write,
    message: &impl Encode,
) -> Result<()> {
    let buf = bincode::encode_to_vec(message, bincode::config::standard())?;
    writer.write_all(&(buf.len() as u32).to_be_bytes())?;
    writer.write_all(&buf)?;
    writer.flush()?;
    Ok(())
}

pub fn read<T: Decode<()>>(mut reader: impl Read) -> Result<T> {
    let mut len_buf = [0_u8; 4];
    reader.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME {
        return Err(anyhow!("Frame of {len} bytes is too large"));
    }
    let mut buf = vec![0_u8; len as usize];
    reader.read_exact(&mut buf)?;
    let (message, _len) = bincode::decode_from_slice(&buf, bincode::config::standard())?;
    Ok(message)
}

#[cfg(test)]
mod test {
    use yakin_core::state::Button;
    use yakin_core::state::Buttons;
    use yakin_core::state::Stick;

    use super::*;

    fn tuning() -> IpcTuning {
        IpcTuning {
            profile: "pad".into(),
            mouse_speed: 1.5,
            mouse_accel: 0.0,
            mouse_release_guard: 3,
            mouse_smoothing: 0.5,
            mouse_stick_speed: 1000.0,
            mouse_stick_curve: 1.6,
            mouse_flick_degrees: 2400.0,
            scroll_speed: 0.2,
            scroll_natural: false,
            scroll_stick_speed: 10.0,
            deadzone_stick: 0.1,
            deadzone_trigger: 0.5,
            trigger_click: 0.3,
            haptics_click: 0.3,
            haptics_pad_ticks: 0.0,
            haptics_menu: 0.4,
        }
    }

    fn config_toml() -> ConfigToml {
        ConfigToml::parse(include_str!("../../../examples/settop.toml")).unwrap()
    }

    fn config() -> IpcConfig {
        IpcConfig {
            baseline: Some(IpcLayer {
                path: "/etc/yakin/config.toml".into(),
                toml: config_toml(),
            }),
            dropins: vec![IpcLayer {
                path: "/var/lib/yakin/config.d/90-editor.toml".into(),
                toml: ConfigToml::default(),
            }],
            effective: config_toml(),
            editor_path: "/var/lib/yakin/config.d/90-editor.toml".into(),
            writable: true,
        }
    }

    #[test]
    fn test_framing_round_trip() {
        let mut buf = Vec::new();
        write(&mut buf, &IpcRequest::Activate("browser".into())).unwrap();
        write(&mut buf, &IpcRequest::Status).unwrap();
        write(&mut buf, &IpcRequest::Tune(tuning())).unwrap();
        let status = IpcResponse::Status(IpcStatus {
            profile: "pad".into(),
            controllers: vec![IpcController {
                id: 3,
                path: "/dev/hidraw3".into(),
                battery: Some(80),
                wireless: true,
                link: IpcLink::Puck,
                connected: true,
                lizard: Some(false),
                leaks: 0,
                mouse: false,
            }],
            profiles: vec!["pad".into()],
            tunings: vec![tuning()],
            yielded: Some(IpcHolder {
                pid: 1234,
                comm: "steam".into(),
            }),
            desktop: Some("niri".into()),
            auto: Some(IpcAuto {
                focused_app_id: "steam_app_1234".into(),
                rule: Some(2),
                profile: Some("pad".into()),
                manual: false,
            }),
        });
        write(&mut buf, &status).unwrap();
        let config = IpcResponse::Config(Box::new(config()));
        write(&mut buf, &config).unwrap();
        write(&mut buf, &IpcRequest::SetConfig(config_toml())).unwrap();
        let vocabulary = IpcResponse::Vocabulary(Vocabulary::new());
        write(&mut buf, &vocabulary).unwrap();
        // Variant, string length, "browser"
        assert_eq!(buf[..4], 9_u32.to_be_bytes());
        let mut reader = buf.as_slice();
        assert_eq!(
            read::<IpcRequest>(&mut reader).unwrap(),
            IpcRequest::Activate("browser".into())
        );
        assert_eq!(read::<IpcRequest>(&mut reader).unwrap(), IpcRequest::Status);
        assert_eq!(
            read::<IpcRequest>(&mut reader).unwrap(),
            IpcRequest::Tune(tuning())
        );
        assert_eq!(read::<IpcResponse>(&mut reader).unwrap(), status);
        assert_eq!(read::<IpcResponse>(&mut reader).unwrap(), config);
        assert_eq!(
            read::<IpcRequest>(&mut reader).unwrap(),
            IpcRequest::SetConfig(config_toml())
        );
        assert_eq!(read::<IpcResponse>(&mut reader).unwrap(), vocabulary);
        assert!(read::<IpcRequest>(&mut reader).is_err());
    }

    #[test]
    fn test_event_round_trip() {
        let mut state = ControllerState {
            buttons: Buttons::empty().with(Button::A).with(Button::LPadTouch),
            lt: 32767,
            lstick: Stick { x: -32767, y: 120 },
            ..Default::default()
        };
        state.lpad.x = 1000;
        state.lpad.y = -1000;
        state.lpad.touched = true;
        let events = [
            IpcEvent::State { id: 4, state },
            IpcEvent::Profile("desktop".into()),
            IpcEvent::Controllers(vec![IpcController {
                id: 4,
                path: "/dev/hidraw4".into(),
                battery: None,
                wireless: false,
                link: IpcLink::Puck,
                connected: true,
                lizard: Some(true),
                leaks: 0,
                mouse: false,
            }]),
            IpcEvent::Tuning(tuning()),
            IpcEvent::Config(Box::new(config())),
            IpcEvent::Keyboard(Some(IpcKeyboard::from(&Keyboard::default()))),
        ];
        let mut buf = Vec::new();
        write(&mut buf, &IpcRequest::Subscribe).unwrap();
        for event in &events {
            write(&mut buf, event).unwrap();
        }
        let mut reader = buf.as_slice();
        assert_eq!(
            read::<IpcRequest>(&mut reader).unwrap(),
            IpcRequest::Subscribe
        );
        for event in events {
            assert_eq!(read::<IpcEvent>(&mut reader).unwrap(), event);
        }
        assert!(read::<IpcEvent>(&mut reader).is_err());
    }
}
