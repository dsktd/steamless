use std::os::unix::net::UnixStream;
use std::path::Path;

use anyhow::anyhow;

use crate::IpcRequest;
use crate::IpcResponse;
use crate::Result;

/// Send one request and wait for its response.
pub fn request(
    socket: &Path,
    request: IpcRequest,
) -> Result<IpcResponse> {
    let stream = match UnixStream::connect(socket) {
        Ok(stream) => stream,
        Err(err) => return Err(anyhow!("Cannot connect to {}: {err}", socket.display())),
    };
    crate::message::write(&stream, &request)?;
    crate::message::read(&stream)
}
