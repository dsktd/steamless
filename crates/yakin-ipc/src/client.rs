use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use anyhow::anyhow;

use crate::IpcEvent;
use crate::IpcRequest;
use crate::IpcResponse;
use crate::Result;

/// Send one request and wait for its response.
pub fn request(
    socket: &Path,
    request: IpcRequest,
) -> Result<IpcResponse> {
    let stream = connect(socket)?;
    crate::message::write(&stream, &request)?;
    crate::message::read(&stream)
}

/// Subscribe and yield events until the daemon closes the connection; the first error ends it.
pub fn subscribe(socket: &Path) -> Result<impl Iterator<Item = Result<IpcEvent>>> {
    let stream = connect(socket)?;
    crate::message::write(&stream, &IpcRequest::Subscribe)?;
    match crate::message::read(&stream)? {
        IpcResponse::Ok => {}
        IpcResponse::Error(err) => return Err(anyhow!(err)),
        response => return Err(anyhow!("Unexpected response {response:?}")),
    }
    let mut done = false;
    Ok(std::iter::from_fn(move || {
        if done {
            return None;
        }
        let event = crate::message::read(&stream);
        done = event.is_err();
        Some(event)
    }))
}

/// Retry delays while the daemon is gone, doubling up to the last.
const BACKOFF: [u64; 5] = [250, 500, 1000, 2000, 4000];

/// A subscription that lasted this long was a success; a shorter one counts as a failure.
const STEADY: Duration = Duration::from_secs(5);

/// The pauses of a client that stays subscribed and reconnects.
#[derive(Default)]
pub struct Backoff {
    attempt: usize,
}
impl Backoff {
    /// How long to wait after a subscription that lasted `lasted` (zero when it failed).
    pub fn delay(
        &mut self,
        lasted: Duration,
    ) -> Duration {
        if lasted >= STEADY {
            self.attempt = 0;
        }
        let delay = Duration::from_millis(BACKOFF[self.attempt]);
        self.attempt = (self.attempt + 1).min(BACKOFF.len() - 1);
        delay
    }
}

fn connect(socket: &Path) -> Result<UnixStream> {
    UnixStream::connect(socket)
        .map_err(|err| anyhow!("Cannot connect to {}: {err}", socket.display()))
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_backoff() {
        let mut backoff = Backoff::default();
        let ms = |delay: Duration| delay.as_millis();
        assert_eq!(ms(backoff.delay(Duration::ZERO)), 250);
        // Ending at once is no success
        assert_eq!(ms(backoff.delay(Duration::from_millis(10))), 500);
        assert_eq!(ms(backoff.delay(Duration::ZERO)), 1000);
        assert_eq!(ms(backoff.delay(Duration::ZERO)), 2000);
        assert_eq!(ms(backoff.delay(Duration::ZERO)), 4000);
        assert_eq!(ms(backoff.delay(Duration::ZERO)), 4000);
        assert_eq!(ms(backoff.delay(STEADY)), 250);
    }
}
