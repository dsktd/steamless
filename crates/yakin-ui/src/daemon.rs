//! The daemon's socket: one request per call, and a thread that stays subscribed and passes
//! every event to the page.

use std::path::Path;
use std::path::PathBuf;
use std::time::Instant;

use log::warn;
use tokio::sync::mpsc::UnboundedSender;
use yakin_core::profile::ConfigToml;
use yakin_core::profile::Vocabulary;
use yakin_ipc::IpcConfig;
use yakin_ipc::IpcEvent;
use yakin_ipc::IpcRequest;
use yakin_ipc::IpcResponse;
use yakin_ipc::IpcStatus;
use yakin_ipc::IpcTuning;
use yakin_ipc::client::Backoff;

/// What the subscription thread sends the page.
#[derive(Clone, Debug)]
pub enum Event {
    Connection(bool),
    /// Why a subscription could not start.
    Failed(String),
    Ipc(IpcEvent),
}

#[derive(Clone, Debug)]
pub struct Daemon(PathBuf);
impl Daemon {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self(socket.into())
    }

    pub fn activate(
        &self,
        name: &str,
    ) -> Result<(), String> {
        self.request(IpcRequest::Activate(name.to_string()))
    }

    pub fn tune(
        &self,
        tuning: IpcTuning,
    ) -> Result<(), String> {
        self.request(IpcRequest::Tune(tuning))
    }

    pub fn reload(&self) -> Result<(), String> {
        self.request(IpcRequest::Reload)
    }

    pub fn status(&self) -> Result<IpcStatus, String> {
        match self.ask(IpcRequest::Status)? {
            IpcResponse::Status(status) => Ok(status),
            response => Err(format!("Unexpected response {response:?}")),
        }
    }

    pub fn config(&self) -> Result<IpcConfig, String> {
        match self.ask(IpcRequest::GetConfig)? {
            IpcResponse::Config(config) => Ok(*config),
            response => Err(format!("Unexpected response {response:?}")),
        }
    }

    pub fn set_config(
        &self,
        toml: ConfigToml,
    ) -> Result<(), String> {
        self.request(IpcRequest::SetConfig(toml))
    }

    pub fn vocabulary(&self) -> Result<Vocabulary, String> {
        match self.ask(IpcRequest::Vocabulary)? {
            IpcResponse::Vocabulary(vocabulary) => Ok(vocabulary),
            response => Err(format!("Unexpected response {response:?}")),
        }
    }

    fn request(
        &self,
        request: IpcRequest,
    ) -> Result<(), String> {
        match self.ask(request)? {
            IpcResponse::Ok => Ok(()),
            response => Err(format!("Unexpected response {response:?}")),
        }
    }

    /// The daemon's answer, its `Error` turned into the page's error.
    fn ask(
        &self,
        request: IpcRequest,
    ) -> Result<IpcResponse, String> {
        match yakin_ipc::client::request(&self.0, request) {
            Ok(IpcResponse::Error(err)) => Err(err),
            Ok(response) => Ok(response),
            Err(err) => Err(err.to_string()),
        }
    }

    /// Stay subscribed on a thread, reconnecting with backoff whenever the daemon goes away,
    /// until the page drops its end.
    pub fn follow(
        &self,
        tx: UnboundedSender<Event>,
    ) {
        let socket = self.0.clone();
        let _handle = std::thread::spawn(move || follow(&socket, &tx));
    }
}

fn follow(
    socket: &Path,
    tx: &UnboundedSender<Event>,
) {
    let mut backoff = Backoff::default();
    loop {
        let start = Instant::now();
        match yakin_ipc::client::subscribe(socket) {
            Ok(events) => {
                if tx.send(Event::Connection(true)).is_err() {
                    return;
                }
                let mut end = "end of stream".to_string();
                for event in events {
                    match event {
                        Ok(event) => {
                            if tx.send(Event::Ipc(event)).is_err() {
                                return;
                            }
                        }
                        Err(err) => end = err.to_string(),
                    }
                }
                let ms = start.elapsed().as_millis();
                warn!("subscription ended after {ms} ms: {end}");
                if tx.send(Event::Connection(false)).is_err() {
                    return;
                }
            }
            Err(err) => {
                warn!("subscribe: {err}");
                if tx.send(Event::Failed(err.to_string())).is_err() {
                    return;
                }
            }
        }
        std::thread::sleep(backoff.delay(start.elapsed()));
    }
}
