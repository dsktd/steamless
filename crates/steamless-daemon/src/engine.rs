//! The daemon's one event loop: controller input in, outputs and IPC answers out.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crossbeam_channel::Sender;
use log::debug;
use log::info;
use log::warn;
use steamless_core::Result;
use steamless_core::device::ControllerId;
use steamless_core::device::Event as DeviceEvent;
use steamless_core::device::ibex::Wireless;
use steamless_core::output::Keys;
use steamless_core::output::Output;
use steamless_core::output::Pad;
use steamless_core::profile::Config;
use steamless_core::profile::Mapper;
use steamless_core::profile::Profile;
use steamless_core::state::ControllerState;
use steamless_ipc::IpcController;
use steamless_ipc::IpcRequest;
use steamless_ipc::IpcResponse;
use steamless_ipc::IpcStatus;

pub enum Event {
    Device(DeviceEvent),
    Ipc(IpcRequest, Sender<IpcResponse>),
}
impl From<DeviceEvent> for Event {
    fn from(event: DeviceEvent) -> Self {
        Event::Device(event)
    }
}
impl From<(IpcRequest, Sender<IpcResponse>)> for Event {
    fn from((request, tx_response): (IpcRequest, Sender<IpcResponse>)) -> Self {
        Event::Ipc(request, tx_response)
    }
}

/// An open hidraw node, whether or not it has sent input yet.
struct Device {
    path: PathBuf,
    battery: Option<u8>,
}

/// A controller that has sent input, with its virtual pad.
struct Controller {
    pad: Pad,
    mapper: Mapper,
    prev: ControllerState,
}

pub struct Engine {
    path: Option<PathBuf>,
    config: Config,
    active: String,
    profile: Profile,
    lizard_off: bool,
    keys: Keys,
    devices: BTreeMap<ControllerId, Device>,
    controllers: BTreeMap<ControllerId, Controller>,
}
impl Engine {
    pub fn new(
        path: Option<PathBuf>,
        config: Config,
        lizard_off: bool,
    ) -> Result<Self> {
        let active = config.active()?;
        let profile = config.get(&active)?;
        info!("Profile {active}");
        Ok(Self {
            path,
            keys: Keys::new(config.keys())?,
            config,
            active,
            profile,
            lizard_off,
            devices: BTreeMap::new(),
            controllers: BTreeMap::new(),
        })
    }

    pub fn handle(
        &mut self,
        event: Event,
    ) {
        match event {
            Event::Device(event) => self.device(event),
            Event::Ipc(request, tx_response) => {
                let response = match self.request(request) {
                    Ok(response) => response,
                    Err(err) => IpcResponse::Error(err.to_string()),
                };
                let _ = tx_response.send(response);
            }
        }
    }

    fn device(
        &mut self,
        event: DeviceEvent,
    ) {
        match event {
            DeviceEvent::Opened(id, path) => {
                debug!("{id} opened {}", path.display());
                self.devices.insert(
                    id,
                    Device {
                        path,
                        battery: None,
                    },
                );
            }
            DeviceEvent::State(id, state) => {
                if !self.controllers.contains_key(&id) {
                    let phys = self.devices.get(&id).map(|device| device.path.display());
                    let pad = match Pad::new(&phys.map_or(id.to_string(), |phys| phys.to_string()))
                    {
                        Ok(pad) => pad,
                        Err(err) => {
                            warn!("{id} pad: {err}");
                            return;
                        }
                    };
                    info!("{id} connected");
                    let controller = Controller {
                        pad,
                        mapper: Mapper::new(self.profile.clone()),
                        prev: ControllerState::default(),
                    };
                    self.controllers.insert(id, controller);
                }
                let controller = self.controllers.get_mut(&id).unwrap();
                let outputs = controller.mapper.map(&controller.prev, &state);
                controller.prev = state;
                send(id, controller, &mut self.keys, &outputs);
            }
            DeviceEvent::Battery(id, battery) => {
                debug!("{id} battery {battery:?}");
                if let Some(device) = self.devices.get_mut(&id) {
                    device.battery = Some(battery.level);
                }
            }
            DeviceEvent::Wireless(id, Wireless::Disconnect) => self.disconnect(id),
            DeviceEvent::Wireless(id, wireless) => debug!("{id} {wireless:?}"),
            DeviceEvent::Closed(id) => {
                debug!("{id} closed");
                self.devices.remove(&id);
                self.disconnect(id);
            }
        }
    }

    fn request(
        &mut self,
        request: IpcRequest,
    ) -> Result<IpcResponse> {
        match request {
            IpcRequest::Status => {
                let controllers = self
                    .devices
                    .iter()
                    .map(|(id, device)| IpcController {
                        id: u8::from(*id),
                        path: device.path.display().to_string(),
                        battery: device.battery,
                        lizard: !self.lizard_off,
                    })
                    .collect();
                Ok(IpcResponse::Status(IpcStatus {
                    profile: self.active.clone(),
                    controllers,
                }))
            }
            IpcRequest::Activate(name) => {
                let profile = self.config.get(&name)?;
                self.activate(name, profile);
                Ok(IpcResponse::Ok)
            }
            IpcRequest::Reload => {
                let config = crate::load(self.path.as_deref())?;
                let profile = config.get(&self.active)?;
                // The uinput key set is fixed at creation; build the new keyboard first so a
                // failure changes nothing, release through the old one, then replace it
                let mut keys = Keys::new(config.keys())?;
                self.config = config;
                self.activate(self.active.clone(), profile);
                std::mem::swap(&mut self.keys, &mut keys);
                Ok(IpcResponse::Ok)
            }
        }
    }

    /// Release what every controller holds under the old profile, then swap it in.
    fn activate(
        &mut self,
        name: String,
        profile: Profile,
    ) {
        for (id, controller) in self.controllers.iter_mut() {
            let outputs = controller.mapper.activate(profile.clone());
            send(*id, controller, &mut self.keys, &outputs);
        }
        info!("Profile {name}");
        self.active = name;
        self.profile = profile;
    }

    /// Drop the virtual pad, releasing any key it still holds.
    fn disconnect(
        &mut self,
        id: ControllerId,
    ) {
        let Some(mut controller) = self.controllers.remove(&id) else {
            return;
        };
        let _ = self.keys.send(&controller.mapper.release_all());
        info!("{id} disconnected");
    }
}

fn send(
    id: ControllerId,
    controller: &mut Controller,
    keys: &mut Keys,
    outputs: &[Output],
) {
    if let Err(err) = controller.pad.send(outputs) {
        warn!("{id} pad: {err}");
    }
    if let Err(err) = keys.send(outputs) {
        warn!("{id} keys: {err}");
    }
}
