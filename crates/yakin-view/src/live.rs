//! The daemon's side: a thread follows its event stream and config, the app reads them as [`Live`].

use std::path::Path;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::anyhow;
use bevy::prelude::*;
use crossbeam_channel::Receiver;
use crossbeam_channel::Sender;
use yakin_core::profile::Config;
use yakin_core::profile::Target;
use yakin_core::state::Button;
use yakin_core::state::ControllerState;
use yakin_ipc::IpcConfig;
use yakin_ipc::IpcEvent;
use yakin_ipc::IpcRequest;
use yakin_ipc::IpcResponse;
use yakin_ipc::client::Backoff;
use yakin_view::layout;
use yakin_view::model::Input;

pub fn plugin(app: &mut App) {
    app.init_resource::<Live>();
    app.add_systems(Startup, listen);
    app.add_systems(PreUpdate, receive);
}

#[derive(Resource, Clone, Debug)]
pub struct Socket(pub PathBuf);

/// What the listener thread hands over.
#[derive(Clone, Debug)]
pub enum Feed {
    Event(IpcEvent),
    Config(Box<IpcConfig>),
    /// The connection is gone, or never came.
    Down(String),
}

#[derive(Resource)]
struct Inbox(Receiver<Feed>);

/// The daemon as last heard.
#[derive(Resource, Clone, Debug, Default)]
pub struct Live {
    pub state: ControllerState,
    /// The active profile; empty before the daemon names it.
    pub profile: String,
    pub controllers: usize,
    /// Why there is no connection, while there is none.
    pub down: Option<String>,
    config: Option<Config>,
}
impl Live {
    pub fn apply(
        &mut self,
        feed: Feed,
    ) {
        match feed {
            Feed::Config(config) => {
                self.down = None;
                self.config(*config);
            }
            Feed::Event(IpcEvent::State { state, .. }) => self.state = state,
            Feed::Event(IpcEvent::Profile(profile)) => self.profile = profile,
            Feed::Event(IpcEvent::Controllers(controllers)) => {
                self.controllers = controllers.len();
                if controllers.is_empty() {
                    self.state = ControllerState::default();
                }
            }
            Feed::Event(IpcEvent::Config(config)) => self.config(*config),
            Feed::Event(_) => {}
            Feed::Down(err) => {
                self.down = Some(err);
                self.state = ControllerState::default();
                self.controllers = 0;
            }
        }
    }

    fn config(
        &mut self,
        config: IpcConfig,
    ) {
        self.config = match Config::try_from(config.effective) {
            Ok(config) => Some(config),
            Err(err) => {
                warn!("Cannot read the daemon's config: {err}");
                None
            }
        };
    }

    /// The base binding of `button` in the active profile, one line per action, plus the
    /// axis it sits on. Chords and layers are not shown.
    pub fn binding(
        &self,
        button: Button,
    ) -> Vec<String> {
        let Some(config) = &self.config else {
            return vec!["no config yet".to_string()];
        };
        let name = match self.profile.is_empty() {
            true => config.active().unwrap_or_default(),
            false => self.profile.clone(),
        };
        let Ok(profile) = config.get(&name) else {
            return vec![format!("no profile {name}")];
        };
        let mut lines = Vec::new();
        if let Some(binding) = profile.buttons.get(&button) {
            let labels = ["press", "start", "release", "long", "double"];
            for (label, target) in labels.into_iter().zip(binding.targets()) {
                if *target != Target::None {
                    lines.push(format!("{label}: {target}"));
                }
            }
        }
        if let Some(target) = layout::axis(button).and_then(|axis| profile.axes.get(&axis)) {
            lines.push(format!("axis: {target}"));
        }
        if lines.is_empty() {
            lines.push("unbound".to_string());
        }
        lines
    }
}

fn listen(
    mut commands: Commands,
    socket: Res<Socket>,
) {
    let (tx_feed, rx_feed) = crossbeam_channel::unbounded();
    let socket = socket.0.clone();
    std::thread::spawn(move || follow(&socket, &tx_feed));
    commands.insert_resource(Inbox(rx_feed));
}

fn receive(
    inbox: Res<Inbox>,
    mut live: ResMut<Live>,
    mut input: ResMut<Input>,
) {
    for feed in inbox.0.try_iter() {
        live.apply(feed);
    }
    if input.0 != live.state {
        input.0 = live.state;
    }
}

/// Stays subscribed, reconnecting with backoff, until the app is gone.
fn follow(
    socket: &Path,
    tx_feed: &Sender<Feed>,
) {
    let mut backoff = Backoff::default();
    loop {
        let started = Instant::now();
        let err = match session(socket, tx_feed) {
            Ok(()) => return,
            Err(err) => err,
        };
        if tx_feed.send(Feed::Down(err.to_string())).is_err() {
            return;
        }
        std::thread::sleep(backoff.delay(started.elapsed()));
    }
}

/// One connection: the config, then events until the stream ends. `Ok` once the app is gone.
fn session(
    socket: &Path,
    tx_feed: &Sender<Feed>,
) -> anyhow::Result<()> {
    let config = match yakin_ipc::client::request(socket, IpcRequest::GetConfig)? {
        IpcResponse::Config(config) => config,
        IpcResponse::Error(err) => return Err(anyhow!(err)),
        response => return Err(anyhow!("Unexpected response {response:?}")),
    };
    if tx_feed.send(Feed::Config(config)).is_err() {
        return Ok(());
    }
    for event in yakin_ipc::client::subscribe(socket)? {
        if tx_feed.send(Feed::Event(event?)).is_err() {
            return Ok(());
        }
    }
    Err(anyhow!("The daemon closed the stream"))
}

#[cfg(test)]
mod test {
    use yakin_core::profile::ConfigToml;
    use yakin_ipc::IpcController;

    use super::*;

    const CONFIG: &str = r#"
[daemon]
active = "pad"

[profile.pad]
layout = "xbox"

[profile.pad.buttons]
steam = "key:KEY_HOMEPAGE"

[profile.desk]
[profile.desk.buttons]
a = "key:KEY_ENTER"
[profile.desk.axes]
lstick = "keys:KEY_UP,KEY_DOWN,KEY_LEFT,KEY_RIGHT"
"#;

    fn config() -> Feed {
        Feed::Config(Box::new(IpcConfig {
            baseline: None,
            dropins: Vec::new(),
            effective: ConfigToml::parse(CONFIG).unwrap(),
            editor_path: String::new(),
            writable: false,
        }))
    }

    #[test]
    fn test_binding() {
        let mut live = Live::default();
        assert_eq!(live.binding(Button::A), ["no config yet"]);
        live.apply(config());
        // No profile event yet: daemon.active
        assert_eq!(live.binding(Button::Steam), ["press: key:KEY_HOMEPAGE"]);
        live.apply(Feed::Event(IpcEvent::Profile("desk".to_string())));
        assert_eq!(live.binding(Button::A), ["press: key:KEY_ENTER"]);
        assert_eq!(live.binding(Button::GripL2), ["unbound"]);
        let stick = live.binding(Button::LStickClick);
        assert_eq!(
            stick.last().unwrap(),
            "axis: keys:KEY_UP,KEY_DOWN,KEY_LEFT,KEY_RIGHT"
        );
        live.apply(Feed::Event(IpcEvent::Profile("gone".to_string())));
        assert_eq!(live.binding(Button::A), ["no profile gone"]);
    }

    #[test]
    fn test_apply_state() {
        let mut live = Live::default();
        let state = ControllerState {
            lt: 7,
            ..Default::default()
        };
        live.apply(Feed::Event(IpcEvent::State { id: 0, state }));
        assert_eq!(live.state.lt, 7);
        // The last controller leaving resets the state
        live.apply(Feed::Event(IpcEvent::Controllers(
            Vec::<IpcController>::new(),
        )));
        assert_eq!(live.state, ControllerState::default());
    }

    #[test]
    fn test_apply_down() {
        let mut live = Live::default();
        live.apply(Feed::Event(IpcEvent::State {
            id: 0,
            state: ControllerState {
                rt: 9,
                ..Default::default()
            },
        }));
        live.apply(Feed::Down("gone".to_string()));
        assert_eq!(live.down.as_deref(), Some("gone"));
        assert_eq!(live.state, ControllerState::default());
        // The next connection clears it
        live.apply(config());
        assert_eq!(live.down, None);
    }
}
