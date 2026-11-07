use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::os::fd::AsFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use crossbeam_channel::Sender;
use log::debug;
use log::warn;
use nix::poll::PollFd;
use nix::poll::PollFlags;
use nix::poll::PollTimeout;
use yakin_core::state::ControllerState;

use crate::IpcEvent;
use crate::IpcRequest;
use crate::IpcResponse;
use crate::Result;

/// A subscriber gets at most one state per controller per tick.
const TICK: Duration = Duration::from_micros(16_667);

/// Profile and controller events kept for a subscriber that is not reading; older ones drop.
const BACKLOG: usize = 16;

/// How long a quiet subscriber waits before it checks whether its client is still there.
const WATCH: Duration = Duration::from_millis(250);

/// Pause after a failed accept, doubling up to `ACCEPT_MAX` while it keeps failing.
const ACCEPT_MIN: Duration = Duration::from_millis(100);
const ACCEPT_MAX: Duration = Duration::from_secs(2);

/// The same accept error is logged at most this often.
const LOG_EVERY: Duration = Duration::from_secs(10);

/// Bind `socket` (mode 0600, a stale file removed first) and accept on a thread. Each request is
/// sent as `(request, tx_response)` and the connection waits for the answer; `Subscribe` is sent
/// as a `Subscriber` and the connection then streams what the receiver feeds it.
pub fn start<E>(
    socket: &Path,
    tx_event: Sender<E>,
) -> Result<()>
where
    E: From<(IpcRequest, Sender<IpcResponse>)> + From<Subscriber> + Send + 'static,
{
    let _ = std::fs::remove_file(socket);
    let listener = UnixListener::bind(socket)?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
    let _handle = std::thread::spawn(move || {
        let mut delay = ACCEPT_MIN;
        let mut log = Throttle::default();
        for stream in listener.incoming() {
            let stream = match stream {
                Ok(stream) => {
                    delay = ACCEPT_MIN;
                    stream
                }
                Err(err) => {
                    // Out of fds or memory fails again at once; wait for some to free up
                    log.warn(&format!("ipc accept: {err}"));
                    std::thread::sleep(delay);
                    delay = (delay * 2).min(ACCEPT_MAX);
                    continue;
                }
            };
            let tx_event_ = tx_event.clone();
            let _handle = std::thread::spawn(move || serve(stream, tx_event_));
        }
    });
    Ok(())
}

fn serve<E>(
    stream: UnixStream,
    tx_event: Sender<E>,
) where
    E: From<(IpcRequest, Sender<IpcResponse>)> + From<Subscriber>,
{
    loop {
        let Ok(request) = crate::message::read::<IpcRequest>(&stream) else {
            break;
        };
        debug!("ipc {request:?}");
        if request == IpcRequest::Subscribe {
            let subscriber = Subscriber::default();
            if crate::message::write(&stream, &IpcResponse::Ok).is_err() {
                break;
            }
            if tx_event.send(subscriber.clone().into()).is_err() {
                break;
            }
            stream_to(&stream, &subscriber);
            break;
        }
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

/// Write what the subscriber collected, one batch per tick, until the client goes away. A quiet
/// stream still notices a gone client within `WATCH`.
fn stream_to(
    stream: &UnixStream,
    subscriber: &Subscriber,
) {
    let mut sent: BTreeMap<u8, ControllerState> = BTreeMap::new();
    loop {
        let start = Instant::now();
        let Some((events, states)) = subscriber.take(WATCH) else {
            if hung_up(stream) {
                subscriber.close();
                return;
            }
            continue;
        };
        for event in events {
            if crate::message::write(stream, &event).is_err() {
                subscriber.close();
                return;
            }
        }
        for (id, state) in states {
            // seq counts every report, so it alone does not make a state new
            let same = sent.get(&id).is_some_and(|prev| {
                ControllerState {
                    seq: prev.seq,
                    ..state
                } == *prev
            });
            if same {
                continue;
            }
            if crate::message::write(stream, &IpcEvent::State { id, state }).is_err() {
                subscriber.close();
                return;
            }
            sent.insert(id, state);
        }
        std::thread::sleep(TICK.saturating_sub(start.elapsed()));
    }
}

/// Logs a repeated message once per `LOG_EVERY`, with how many it held back.
#[derive(Default)]
struct Throttle {
    last: Option<(String, Instant)>,
    suppressed: u32,
}
impl Throttle {
    fn warn(
        &mut self,
        message: &str,
    ) {
        if let Some((last, at)) = &self.last
            && last == message
            && at.elapsed() < LOG_EVERY
        {
            self.suppressed += 1;
            return;
        }
        match self.suppressed {
            0 => warn!("{message}"),
            n => warn!("{message} ({n} more suppressed)"),
        }
        self.last = Some((message.to_string(), Instant::now()));
        self.suppressed = 0;
    }
}

/// A subscribed client sends nothing more, so anything readable is its EOF (or a protocol error,
/// just as final), and a hangup or socket error means it is gone.
fn hung_up(stream: &UnixStream) -> bool {
    let mut fds = [PollFd::new(stream.as_fd(), PollFlags::POLLIN)];
    match nix::poll::poll(&mut fds, PollTimeout::ZERO) {
        Ok(0) => false,
        Ok(_) => true,
        Err(err) => {
            debug!("ipc poll: {err}");
            true
        }
    }
}

#[derive(Default)]
struct Feed {
    events: VecDeque<IpcEvent>,
    states: BTreeMap<u8, ControllerState>,
    closed: bool,
}

/// The receiving end of one `Subscribe` connection. Feeding it never blocks on the client: states
/// overwrite the one not yet sent and the event backlog is capped.
#[derive(Clone, Default)]
pub struct Subscriber(Arc<(Mutex<Feed>, Condvar)>);
impl Subscriber {
    pub fn state(
        &self,
        id: u8,
        state: ControllerState,
    ) {
        let (feed, wake) = &*self.0;
        feed.lock().unwrap().states.insert(id, state);
        wake.notify_one();
    }

    pub fn event(
        &self,
        event: IpcEvent,
    ) {
        let (feed, wake) = &*self.0;
        let mut feed = feed.lock().unwrap();
        if feed.events.len() == BACKLOG {
            feed.events.pop_front();
        }
        feed.events.push_back(event);
        wake.notify_one();
    }

    /// The client has gone; drop this subscriber.
    pub fn is_closed(&self) -> bool {
        self.0.0.lock().unwrap().closed
    }

    fn close(&self) {
        self.0.0.lock().unwrap().closed = true;
    }

    /// Wait up to `timeout` until something is pending, then take it all.
    fn take(
        &self,
        timeout: Duration,
    ) -> Option<(VecDeque<IpcEvent>, BTreeMap<u8, ControllerState>)> {
        let (feed, wake) = &*self.0;
        let (mut feed, wait) = wake
            .wait_timeout_while(feed.lock().unwrap(), timeout, |feed| {
                feed.events.is_empty() && feed.states.is_empty()
            })
            .unwrap();
        if wait.timed_out() {
            return None;
        }
        Some((
            std::mem::take(&mut feed.events),
            std::mem::take(&mut feed.states),
        ))
    }
}

#[cfg(test)]
mod test {
    use super::*;

    enum Event {
        Request(IpcRequest, Sender<IpcResponse>),
        Subscribe(Subscriber),
    }
    impl From<(IpcRequest, Sender<IpcResponse>)> for Event {
        fn from((request, tx_response): (IpcRequest, Sender<IpcResponse>)) -> Self {
            Event::Request(request, tx_response)
        }
    }
    impl From<Subscriber> for Event {
        fn from(subscriber: Subscriber) -> Self {
            Event::Subscribe(subscriber)
        }
    }

    #[test]
    fn test_throttle() {
        let mut log = Throttle::default();
        log.warn("a");
        log.warn("a");
        log.warn("a");
        assert_eq!(log.suppressed, 2);
        // Another error is logged at once
        log.warn("b");
        assert_eq!(log.suppressed, 0);
        log.last = Some(("b".into(), Instant::now() - LOG_EVERY));
        log.warn("b");
        assert_eq!(log.suppressed, 0);
    }

    #[test]
    fn test_subscribe_coalesces() {
        let socket = std::env::temp_dir().join(format!("yakin-test-{}.sock", std::process::id()));
        let (tx_event, rx_event) = crossbeam_channel::unbounded();
        start::<Event>(&socket, tx_event).unwrap();
        let mut events = crate::client::subscribe(&socket).unwrap();
        let Ok(Event::Subscribe(subscriber)) = rx_event.recv() else {
            panic!("no subscriber");
        };
        subscriber.event(IpcEvent::Profile("pad".into()));
        // Many reports within one tick arrive as a few frames, the last one among them
        for lt in 0..100 {
            let state = ControllerState {
                seq: lt as u8,
                lt,
                ..Default::default()
            };
            subscriber.state(2, state);
        }
        assert_eq!(
            events.next().unwrap().unwrap(),
            IpcEvent::Profile("pad".into())
        );
        // The first batch may go out before the loop ends, the rest waits for the tick
        let mut frames = 0;
        loop {
            let IpcEvent::State { id, state } = events.next().unwrap().unwrap() else {
                panic!("no state");
            };
            assert_eq!(id, 2);
            frames += 1;
            if state.lt == 99 {
                break;
            }
        }
        assert!(frames < 10, "{frames} frames");
        drop(events);
        // A request on its own connection while the engine answers it
        let tuning = crate::IpcTuning {
            profile: "pad".into(),
            mouse_speed: 2.0,
            mouse_accel: 0.5,
            mouse_release_guard: 0,
            mouse_smoothing: 0.0,
            mouse_stick_speed: 1500.0,
            mouse_stick_curve: 2.0,
            mouse_flick_degrees: 3000.0,
            scroll_speed: 1.0,
            scroll_natural: true,
            scroll_stick_speed: 5.0,
            deadzone_stick: 0.2,
            deadzone_trigger: 0.0,
            trigger_click: 0.3,
            haptics_click: 0.7,
            haptics_pad_ticks: 0.1,
            haptics_menu: 0.0,
        };
        let request = IpcRequest::Tune(tuning.clone());
        let socket_ = socket.clone();
        let client = std::thread::spawn(move || crate::client::request(&socket_, request));
        let Ok(Event::Request(IpcRequest::Tune(got), tx_response)) = rx_event.recv() else {
            panic!("no tune");
        };
        assert_eq!(got, tuning);
        tx_response.send(IpcResponse::Ok).unwrap();
        assert_eq!(client.join().unwrap().unwrap(), IpcResponse::Ok);
        let _ = std::fs::remove_file(&socket);
    }
}
