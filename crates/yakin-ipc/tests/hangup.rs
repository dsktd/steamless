//! Its own process: the open fd count is the whole process's.

use std::time::Duration;
use std::time::Instant;

use crossbeam_channel::Sender;
use yakin_ipc::IpcRequest;
use yakin_ipc::IpcResponse;
use yakin_ipc::server::Subscriber;

enum Event {
    Request,
    Subscribe(Subscriber),
}
impl From<(IpcRequest, Sender<IpcResponse>)> for Event {
    fn from(_: (IpcRequest, Sender<IpcResponse>)) -> Self {
        Event::Request
    }
}
impl From<Subscriber> for Event {
    fn from(subscriber: Subscriber) -> Self {
        Event::Subscribe(subscriber)
    }
}

fn open_fds() -> usize {
    std::fs::read_dir("/proc/self/fd").unwrap().count()
}

#[test]
fn test_gone_subscribers_close() {
    let socket = std::env::temp_dir().join(format!("yakin-hangup-{}.sock", std::process::id()));
    let (tx_event, rx_event) = crossbeam_channel::unbounded();
    yakin_ipc::server::start::<Event>(&socket, tx_event).unwrap();
    let before = open_fds();
    let mut subscribers = Vec::new();
    for _ in 0..200 {
        let events = yakin_ipc::client::subscribe(&socket).unwrap();
        drop(events);
        let Ok(Event::Subscribe(subscriber)) = rx_event.recv() else {
            panic!("no subscriber");
        };
        subscribers.push(subscriber);
    }
    let start = Instant::now();
    loop {
        // What the engine does on every tick
        subscribers.retain(|subscriber| !subscriber.is_closed());
        let after = open_fds();
        if after <= before && subscribers.is_empty() {
            eprintln!("fds {before} before, {after} after {:?}", start.elapsed());
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "fds {before} before, {after} after, {} subscribers",
            subscribers.len()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = std::fs::remove_file(&socket);
}
