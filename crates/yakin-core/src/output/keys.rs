//! Virtual keyboard for key bindings, one per daemon.
//!
//! Raw uinput ioctls instead of evdev's builder, which cannot set `EV_REP`.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::Write;
use std::os::fd::AsRawFd;

use anyhow::anyhow;
use evdev::KeyCode;
use nix::libc::input_event;
use nix::libc::uinput_setup;

use crate::Result;
use crate::output::Output;
use crate::output::UINPUT;

const NAME: &str = "yakin keys";
const BUS_VIRTUAL: u16 = 0x06;
const VENDOR: u16 = 0x28de;
const PRODUCT: u16 = 0x0001;

const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_REP: u16 = 0x14;
const REP_DELAY: u16 = 0x00;
const REP_PERIOD: u16 = 0x01;

/// Autorepeat of held keys, in ms: slow enough that a stick flick sends one arrow.
const DELAY: i32 = 400;
const PERIOD: i32 = 150;

/// Always enabled, on top of whatever the profiles bind.
pub const DEFAULT_KEYS: [KeyCode; 7] = [
    KeyCode::KEY_HOMEPAGE,
    KeyCode::KEY_HOME,
    KeyCode::KEY_VOLUMEUP,
    KeyCode::KEY_VOLUMEDOWN,
    KeyCode::KEY_MUTE,
    KeyCode::KEY_ESC,
    KeyCode::KEY_ENTER,
];

nix::ioctl_none!(ui_dev_create, b'U', 1);
nix::ioctl_write_ptr!(ui_dev_setup, b'U', 3, uinput_setup);
nix::ioctl_write_int!(ui_set_evbit, b'U', 100);
nix::ioctl_write_int!(ui_set_keybit, b'U', 101);

pub struct Keys {
    file: File,
    keys: BTreeSet<u16>,
}
impl Keys {
    /// A keyboard with [`DEFAULT_KEYS`] plus `extra`, with kernel autorepeat.
    pub fn new(extra: impl IntoIterator<Item = u16>) -> Result<Self> {
        crate::output::check_uinput()?;
        let mut keys: BTreeSet<u16> = DEFAULT_KEYS.iter().map(|key| key.0).collect();
        keys.extend(extra);
        let file = match File::options().write(true).open(UINPUT) {
            Ok(file) => file,
            Err(err) => return Err(anyhow!("Cannot open {UINPUT}: {err}")),
        };
        let fd = file.as_raw_fd();
        let mut setup: uinput_setup = unsafe { std::mem::zeroed() };
        setup.id.bustype = BUS_VIRTUAL;
        setup.id.vendor = VENDOR;
        setup.id.product = PRODUCT;
        for (index, byte) in NAME.bytes().enumerate() {
            setup.name[index] = byte as _;
        }
        unsafe { ui_set_evbit(fd, EV_KEY as _) }?;
        unsafe { ui_set_evbit(fd, EV_REP as _) }?;
        for key in &keys {
            unsafe { ui_set_keybit(fd, *key as _) }?;
        }
        unsafe { ui_dev_setup(fd, &setup) }?;
        unsafe { ui_dev_create(fd) }?;
        let mut keys = Self { file, keys };
        keys.write(EV_REP, REP_DELAY, DELAY)?;
        keys.write(EV_REP, REP_PERIOD, PERIOD)?;
        Ok(keys)
    }

    /// Emit the key outputs; pad outputs are skipped.
    pub fn send(
        &mut self,
        outputs: &[Output],
    ) -> Result<()> {
        for output in outputs {
            let Output::Key(code, pressed) = *output else {
                continue;
            };
            if !self.keys.contains(&code) {
                return Err(anyhow!("Key {code} is not enabled on {NAME}"));
            }
            // A report per key, so a shift wrapped around a key is seen pressed first
            self.write(EV_KEY, code, pressed as i32)?;
            self.write(EV_SYN, 0, 0)?;
        }
        Ok(())
    }

    fn write(
        &mut self,
        type_: u16,
        code: u16,
        value: i32,
    ) -> Result<()> {
        let mut event: input_event = unsafe { std::mem::zeroed() };
        event.type_ = type_;
        event.code = code;
        event.value = value;
        let bytes = unsafe {
            std::slice::from_raw_parts(
                &event as *const input_event as *const u8,
                size_of::<input_event>(),
            )
        };
        self.file.write_all(bytes)?;
        Ok(())
    }
}
