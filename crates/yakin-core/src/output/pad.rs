//! Virtual Xbox 360 pad, one per controller, with its games' rumble passed back to it.

use std::os::fd::AsFd;
use std::os::fd::OwnedFd;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use anyhow::anyhow;
use evdev::AbsInfo;
use evdev::AbsoluteAxisCode;
use evdev::AttributeSet;
use evdev::BusType;
use evdev::EventSummary;
use evdev::EventType;
use evdev::FFEffectCode;
use evdev::FFEffectKind;
use evdev::InputEvent;
use evdev::InputId;
use evdev::KeyCode;
use evdev::UInputCode;
use evdev::UinputAbsSetup;
use evdev::uinput::VirtualDevice;
use log::debug;
use log::warn;
use nix::poll::PollFd;
use nix::poll::PollFlags;
use nix::poll::PollTimeout;

use crate::Result;
use crate::device::haptics::Effect;
use crate::device::haptics::Haptics;
use crate::device::haptics::Rumble;
use crate::output::Output;
use crate::output::PadButton;
use crate::output::PadStick;
use crate::output::PadTrigger;
use crate::output::UINPUT;

const NAME: &str = "Microsoft X-Box 360 pad";
const VENDOR: u16 = 0x045e;
const PRODUCT: u16 = 0x028e;
const VERSION: u16 = 0x0114;

/// Rumble effects a game may upload at once.
const EFFECTS_MAX: u32 = 16;

/// How often the rumble thread looks whether its pad is gone.
const RUMBLE_POLL: Duration = Duration::from_millis(100);

/// How long dropping a pad waits for its rumble thread to stop the motors: a poll and a write.
const RUMBLE_STOP: Duration = Duration::from_millis(500);

/// A thread that runs until told to stop, and whose end can be waited for.
struct Worker {
    /// Set to end the thread.
    gone: Arc<AtomicBool>,
    /// Hung up when the thread returns.
    done: crossbeam_channel::Receiver<()>,
}
impl Worker {
    fn spawn(work: impl FnOnce(&AtomicBool) + Send + 'static) -> Self {
        let gone = Arc::new(AtomicBool::new(false));
        let (done_tx, done) = crossbeam_channel::bounded(0);
        let gone_ = gone.clone();
        let _handle = std::thread::spawn(move || {
            let _done = done_tx;
            work(&gone_);
        });
        Self { gone, done }
    }
}
impl Drop for Worker {
    /// Stop the thread and wait up to [`RUMBLE_STOP`] for it to return, so what it does on the
    /// way out, the rumble stop, happens before the process exits.
    fn drop(&mut self) {
        self.gone.store(true, Ordering::Relaxed);
        let _ = self.done.recv_timeout(RUMBLE_STOP);
    }
}

pub struct Pad {
    /// Shared with the rumble thread, which answers the pad's force feedback requests.
    device: Arc<Mutex<VirtualDevice>>,
    /// Up, down, left, right; folded into the hat.
    dpad: [bool; 4],
    /// The rumble thread, stopped on drop; it holds its own reference to the device.
    _rumble: Option<Worker>,
}
impl Pad {
    /// `phys` tells two pads apart, e.g. the hidraw path. With `rumble`, the pad advertises
    /// `FF_RUMBLE` and plays its games' effects on those motors.
    pub fn new(
        phys: &str,
        rumble: Option<Haptics>,
    ) -> Result<Self> {
        crate::output::check_uinput()?;
        let keys = AttributeSet::from_iter([
            KeyCode::BTN_SOUTH,
            KeyCode::BTN_EAST,
            KeyCode::BTN_NORTH,
            KeyCode::BTN_WEST,
            KeyCode::BTN_TL,
            KeyCode::BTN_TR,
            KeyCode::BTN_SELECT,
            KeyCode::BTN_START,
            KeyCode::BTN_MODE,
            KeyCode::BTN_THUMBL,
            KeyCode::BTN_THUMBR,
        ]);
        let stick = AbsInfo::new(0, i16::MIN as i32, i16::MAX as i32, 16, 128, 0);
        let trigger = AbsInfo::new(0, 0, 255, 0, 0, 0);
        let hat = AbsInfo::new(0, -1, 1, 0, 0, 0);
        let _ = phys; // TODO uniq/phys: UI_SET_PHYS returns EINVAL here, investigate
        let builder = match VirtualDevice::builder() {
            Ok(builder) => builder,
            Err(err) => return Err(anyhow!("Cannot open {UINPUT}: {err}")),
        };
        let builder = builder
            .name(NAME)
            .input_id(InputId::new(BusType::BUS_USB, VENDOR, PRODUCT, VERSION))
            .with_keys(&keys)?
            .with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisCode::ABS_X, stick))?
            .with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisCode::ABS_Y, stick))?
            .with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisCode::ABS_Z, trigger))?
            .with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisCode::ABS_RX, stick))?
            .with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisCode::ABS_RY, stick))?
            .with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisCode::ABS_RZ, trigger))?
            .with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisCode::ABS_HAT0X, hat))?
            .with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisCode::ABS_HAT0Y, hat))?;
        let builder = match rumble {
            Some(_) => builder
                .with_ff(&AttributeSet::from_iter([FFEffectCode::FF_RUMBLE]))?
                .with_ff_effects_max(EFFECTS_MAX),
            None => builder,
        };
        let device = Arc::new(Mutex::new(builder.build()?));
        let rumble = match rumble {
            Some(haptics) => {
                let fd = device.lock().unwrap().as_fd().try_clone_to_owned()?;
                let device_ = device.clone();
                Some(Worker::spawn(move |gone| {
                    feedback(fd, &device_, gone, haptics)
                }))
            }
            None => None,
        };
        Ok(Self {
            device,
            dpad: [false; 4],
            _rumble: rumble,
        })
    }

    /// Emit the pad outputs as one report; keyboard outputs are skipped.
    pub fn send(
        &mut self,
        outputs: &[Output],
    ) -> Result<()> {
        let mut events = Vec::new();
        for output in outputs {
            match *output {
                Output::Button(button, pressed) => {
                    let code = match button {
                        PadButton::South => KeyCode::BTN_SOUTH,
                        PadButton::East => KeyCode::BTN_EAST,
                        PadButton::West => KeyCode::BTN_WEST,
                        PadButton::North => KeyCode::BTN_NORTH,
                        PadButton::Lb => KeyCode::BTN_TL,
                        PadButton::Rb => KeyCode::BTN_TR,
                        PadButton::Select => KeyCode::BTN_SELECT,
                        PadButton::Start => KeyCode::BTN_START,
                        PadButton::Guide => KeyCode::BTN_MODE,
                        PadButton::ThumbL => KeyCode::BTN_THUMBL,
                        PadButton::ThumbR => KeyCode::BTN_THUMBR,
                        PadButton::DpadUp
                        | PadButton::DpadDown
                        | PadButton::DpadLeft
                        | PadButton::DpadRight => {
                            self.hat(button, pressed, &mut events);
                            continue;
                        }
                    };
                    events.push(InputEvent::new(EventType::KEY.0, code.0, pressed as i32));
                }
                Output::Stick(stick, x, y) => {
                    let (code_x, code_y) = match stick {
                        PadStick::Left => (AbsoluteAxisCode::ABS_X, AbsoluteAxisCode::ABS_Y),
                        PadStick::Right => (AbsoluteAxisCode::ABS_RX, AbsoluteAxisCode::ABS_RY),
                    };
                    events.push(abs(code_x, x as i32));
                    events.push(abs(code_y, y as i32));
                }
                Output::Trigger(trigger, value) => {
                    let code = match trigger {
                        PadTrigger::Lt => AbsoluteAxisCode::ABS_Z,
                        PadTrigger::Rt => AbsoluteAxisCode::ABS_RZ,
                    };
                    events.push(abs(code, value as i32));
                }
                Output::Key(..)
                | Output::MouseMove(..)
                | Output::Scroll(..)
                | Output::MouseButton(..)
                | Output::Desktop(_)
                | Output::Profile(_)
                | Output::Layer(..)
                | Output::Menu(_)
                | Output::Keyboard(_)
                | Output::Lizard(..)
                | Output::TvOff
                | Output::Haptic { .. }
                | Output::Tone { .. } => {}
            }
        }
        if events.is_empty() {
            return Ok(());
        }
        self.device.lock().unwrap().emit(&events)?;
        Ok(())
    }

    fn hat(
        &mut self,
        button: PadButton,
        pressed: bool,
        events: &mut Vec<InputEvent>,
    ) {
        let index = match button {
            PadButton::DpadUp => 0,
            PadButton::DpadDown => 1,
            PadButton::DpadLeft => 2,
            PadButton::DpadRight => 3,
            _ => unreachable!(),
        };
        self.dpad[index] = pressed;
        let [up, down, left, right] = self.dpad.map(|held| held as i32);
        events.push(abs(AbsoluteAxisCode::ABS_HAT0X, right - left));
        events.push(abs(AbsoluteAxisCode::ABS_HAT0Y, down - up));
    }
}

/// Answer the pad's force feedback uploads and erases and play its rumble on the controller,
/// until the pad is dropped. `fd` is the pad's, to wait on without holding the lock.
fn feedback(
    fd: OwnedFd,
    device: &Mutex<VirtualDevice>,
    gone: &AtomicBool,
    mut haptics: Haptics,
) {
    let mut rumble = Rumble::default();
    while !gone.load(Ordering::Relaxed) {
        let wait = rumble.wait(Instant::now(), RUMBLE_POLL);
        let mut fds = [PollFd::new(fd.as_fd(), PollFlags::POLLIN)];
        let timeout = PollTimeout::try_from(wait).unwrap_or(PollTimeout::ZERO);
        match nix::poll::poll(&mut fds, timeout) {
            Ok(0) => {}
            Ok(_) => {
                if let Err(err) = requests(device, &mut rumble) {
                    debug!("rumble: {err}");
                    break;
                }
            }
            Err(err) => {
                debug!("rumble poll: {err}");
                break;
            }
        }
        if let Some((strong, weak)) = rumble.due(Instant::now())
            && let Err(err) = haptics.rumble(strong, weak)
        {
            warn!("rumble: {err}");
        }
    }
    let _ = haptics.rumble(0, 0);
}

/// Read what the kernel asks of the pad: effect uploads and erases, and plays.
fn requests(
    device: &Mutex<VirtualDevice>,
    rumble: &mut Rumble,
) -> Result<()> {
    let mut device = device.lock().unwrap();
    let events: Vec<InputEvent> = device.fetch_events()?.collect();
    let now = Instant::now();
    for event in events {
        match event.destructure() {
            EventSummary::UInput(event, UInputCode::UI_FF_UPLOAD, _) => {
                let upload = device.process_ff_upload(event)?;
                let data = upload.effect();
                if let FFEffectKind::Rumble {
                    strong_magnitude,
                    weak_magnitude,
                } = data.kind
                {
                    let effect = Effect {
                        strong: strong_magnitude,
                        weak: weak_magnitude,
                        length: Duration::from_millis(data.replay.length as u64),
                    };
                    rumble.upload(upload.effect_id(), effect);
                }
            }
            EventSummary::UInput(event, UInputCode::UI_FF_ERASE, _) => {
                let erase = device.process_ff_erase(event)?;
                rumble.erase(erase.effect_id() as i16);
            }
            EventSummary::ForceFeedback(_, code, count) if code != FFEffectCode::FF_GAIN => {
                rumble.play(code.0 as i16, count, now);
            }
            _ => {}
        }
    }
    Ok(())
}

fn abs(
    code: AbsoluteAxisCode,
    value: i32,
) -> InputEvent {
    InputEvent::new(EventType::ABSOLUTE.0, code.0, value)
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::device::haptics::rumble_report;

    /// Dropping the worker waits for the thread's way out: the rumble stop is written by then.
    #[test]
    fn test_worker_drop_waits_for_rumble_stop() {
        let path = std::env::temp_dir().join(format!("yakin-rumble-{}", std::process::id()));
        std::fs::write(&path, b"").unwrap();
        let mut haptics = Haptics::open(&path).unwrap();
        let worker = Worker::spawn(move |gone| {
            while !gone.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
            }
            // A slow way out, as a poll timeout would be
            std::thread::sleep(Duration::from_millis(50));
            let _ = haptics.rumble(0, 0);
        });
        drop(worker);
        let written = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(written, rumble_report(0, 0));
    }
}
