use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::os::unix::net::UnixStream;
use std::path::Path;

use crossbeam_channel::Sender;
use log::debug;
use log::warn;

use crate::IpcRequest;
use crate::IpcResponse;
use crate::Result;

/// Bind `socket` (mode 0600, a stale file removed first) and accept on a thread. Each request is
/// sent as `(request, tx_response)` and the connection waits for the answer.
pub fn start<E: From<(IpcRequest, Sender<IpcResponse>)> + Send + 'static>(
    socket: &Path,
    tx_event: Sender<E>,
) -> Result<()> {
    let _ = std::fs::remove_file(socket);
    let listener = UnixListener::bind(socket)?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
    let _handle = std::thread::spawn(move || {
        for stream in listener.incoming() {
            let stream = match stream {
                Ok(stream) => stream,
                Err(err) => {
                    warn!("ipc accept: {err}");
                    continue;
                }
            };
            let tx_event_ = tx_event.clone();
            let _handle = std::thread::spawn(move || serve(stream, tx_event_));
        }
    });
    Ok(())
}

fn serve<E: From<(IpcRequest, Sender<IpcResponse>)>>(
    stream: UnixStream,
    tx_event: Sender<E>,
) {
    loop {
        let Ok(request) = crate::message::read::<IpcRequest>(&stream) else {
            break;
        };
        debug!("ipc {request:?}");
        let (tx_response, rx_response) = crossbeam_channel::bounded(1);
        if tx_event.send((request, tx_response).into()).is_err() {
            break;
        }
        let Ok(response) = rx_response.recv() else {
            break;
        };
        if let Err(err) = crate::message::write(&stream, &response) {
            debug!("ipc write: {err}");
            break;
        }
    }
}
