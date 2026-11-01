# steamless: design

Steam Input-style features for the Steam Controller family without Steam
running. A daemon reads the controller over hidraw (or the kernel evdev
device as a fallback), applies a profile, and presents a standard virtual
gamepad plus a virtual keyboard through uinput. An optional GUI edits the
profile and talks to the daemon over a local socket. Rust.

Prior art for the raw protocol (cited, not copied):

- Linux `hid-steam` 7.3 (`hid-steam-v7.3.c` in this repo): Ibex report
  layout, feature-report command path, lizard mode, haptics, `client_opened`.
- SDL3 HIDAPI Steam driver (`SDL_HINT_JOYSTICK_HIDAPI_STEAM`,
  https://wiki.libsdl.org/SDL3/SDL_HINT_JOYSTICK_HIDAPI_STEAM): userspace
  take-over of the controller, off by default, Bluetooth path.
- C0rn3j/sc-controller (https://github.com/C0rn3j/sc-controller) and the
  Patola/sc-controller-cc fork, whose `docs/steam-controller-v2-protocol.md`
  documents the 2026 IMU layout (quaternion vs angular rate split):
  userspace lizard toggling, haptics, gyro modes, profile engine shape.

## 1. Goals / non-goals

Goals

- Work with no Steam client, on any kernel that exposes hidraw. Kernel >= 7.3
  `hid-steam` is a fallback and compat path, not a requirement.
- 2026 Steam Controller: USB puck `28de:1304`, wired `28de:1305`, Bluetooth
  `28de:1303`. Device layer shaped so 2015 (`1102`/`1142`) and Steam Deck
  (`1205`) can be added later; not specified here.
- One virtual gamepad per controller with a layout Chromium and SDL map
  natively (Xbox-like), and a virtual keyboard for chords (Home, volume, mute).
- Profiles in TOML. Per-app/per-window activation as a later milestone.
- First deployment: the NixOS kiosk "settop" (Cage, Chromium kiosk launcher,
  Dolphin, PCSX2). Replaces the JS gamepad-index hacks and the Python evdev
  watcher (`retrotv-bridge.py`, `home.js`).
- Nix flake with a NixOS module: systemd service, udev rule, nothing as root.

Non-goals

- Reimplementing Steam Input's full action-set model, touch menus, radial
  menus, mouse emulation from trackpads (lizard mode already does the latter).
- Firmware update, pairing UI, audio upload, calibration commands.
- Windows/macOS. Rumble passthrough from games (FF) is a maybe after M2.
- Multi-user desktops. One daemon, one user, up to 4 controllers on a puck.

## 2. Architecture

Components

- `device`: opens one controller, decodes reports into `ControllerState`,
  sends commands (lizard, IMU mode, haptics). Two backends: `hidraw` (primary)
  and `evdev` (kernel `hid-steam` gamepad + motion sensors, fallback).
- `state`: `ControllerState` snapshot (buttons bitset, sticks, pads, triggers,
  IMU, battery) with edge detection against the previous snapshot.
- `profile`: TOML-loaded `Profile`; the engine maps a `ControllerState` to
  `Output` events, handling chords and mode shifts.
- `output`: `Pad` (uinput gamepad) and `Keys` (uinput keyboard).
- `ipc`: Unix socket server in the daemon; client used by the GUI and a CLI.
- `gui`: separate optional binary, edits the TOML, asks the daemon to reload.

Data flow

```
 hidraw node(s)              evdev node (fallback)
 /dev/hidraw*  28de:1304/5/3  /dev/input/event* "Wireless Steam Controller"
      |                             |
      v                             v
 +---------------------------------------------+
 | device: Backend::{Hidraw, Evdev}            |  one thread per controller
 |   decode 0x42/0x45 -> ControllerState       |  tx_state.send(state)
 |   commands: lizard, imu, haptic (hidraw)    |  <- rx_command
 +---------------------------------------------+
      | ControllerState (crossbeam channel)
      v
 +---------------------------------------------+
 | engine: Profile + Mapper                    |  main thread
 |   edges, chords, modeshift, deadzones       |
 |   -> Vec<Output>                            |
 +---------------------------------------------+
      |                     |
      v                     v
 +-------------+     +-------------+     +------------------+
 | output::Pad |     | output::Keys|     | ipc::Server      |
 | uinput pad  |     | uinput kbd  |     | runtime_dir/     |
 | "Xbox 360"  |     | KEY_HOME..  |     | steamless.sock   |
 +-------------+     +-------------+     +------------------+
                                                 ^
                                         steamless-gui / steamless-cli
```

Threads, not async: one reader thread per controller, one IPC accept thread
plus one per connection, the engine on the main thread draining a single
`crossbeam_channel` of `Event` (`State(id, ControllerState)`,
`Hotplug(..)`, `Ipc(request, tx_response)`). Hotplug via a udev monitor
thread (`udev` crate) or a 2 s rescan of `/sys/class/hidraw`; rescan first,
udev monitor if it bothers anyone.

## 3. Device protocol notes (2026 controller, "Ibex")

Known from `hid-steam-v7.3.c`

- Interfaces: wired = 1 unified HID interface; puck = 7 interfaces, 2-5 are
  controller slots, 6 is the pogo-pin interface (collection usage
  `0xFF000002`, ignore it). BLE = 1 interface.
- Input report `0x42`, 54 bytes; `0x45`, 46 bytes (same layout, no
  quaternion). Offsets (byte 0 = report id):
  - 2-5 buttons, bit `byte.bit`: 2.0 A, 2.1 B, 2.2 X, 2.3 Y, 2.4 quick access
    (`BTN_BASE`), 2.5 R-stick click, 2.6 menu/start, 2.7 right top grip,
    3.0 right bottom grip, 3.1 RB, 3.2 D-down, 3.3 D-right, 3.4 D-left,
    3.5 D-up, 3.6 view/select, 3.7 L-stick click, 4.0 Steam logo
    (`BTN_MODE`), 4.1 left top grip, 4.2 left bottom grip, 4.3 LB,
    4.4 R-stick touched, 4.5 right pad touched, 4.6 right pad pressed,
    4.7 RT full, 5.0 L-stick touched, 5.1 left pad touched, 5.2 left pad
    pressed, 5.3 LT full, 5.4/5.5 grip touch.
  - 6-7 left trigger, 8-9 right trigger (u16, 0..32767).
  - 10-11 LX, 12-13 LY, 14-15 RX, 16-17 RY (s16, Y inverted by the kernel).
  - 18-19/20-21 left pad X/Y, 22-23 pressure; 24-27 right pad X/Y, 28-29
    pressure. Pad values only valid while the touched bit is set.
  - 30-33 u32 IMU timestamp; 34-45 accel X,Z,Y then gyro X,Z,Y (s16, note the
    kernel's axis swap); 46-53 quaternion W,X,Y,Z (0x42 only).
- Battery report `0x43`, 15 bytes: charge_state (1 discharging, 2 charging,
  4 done), level %, voltages/currents/temperature as le16.
- Wireless event `0x79`, 2 bytes: 1 disconnect, 2 connect, 3 pair.
- Commands go as HID **feature report id 1**, 64 bytes:
  `[0x01, cmd, len, payload..]`. Replies are read back with GET_REPORT on the
  same id; the kernel retries on EPIPE with 20 ms sleeps (wireless).
  - Lizard off: `0x81` (`ID_CLEAR_DIGITAL_MAPPINGS`, no payload) then
    `0x87` (`ID_SET_SETTINGS_VALUES`) with
    `[SETTING_LIZARD_MODE, 0, 0, SETTING_STEAM_WATCHDOG_ENABLE, 0, 0]`
    (len 6; each setting is `reg, lo, hi`).
  - Lizard on: `0x85` (`ID_SET_DEFAULT_DIGITAL_MAPPINGS`) then `0x8E`
    (`ID_LOAD_DEFAULT_SETTINGS`).
  - IMU on: `0x87` with `SETTING_IMU_MODE = SEND_RAW_ACCEL|SEND_RAW_GYRO`
    (bits 3|4; bit 2 adds orientation). Needed on puck/BLE; wired streams
    IMU without it.
- Haptics are HID **output reports** (write to hidraw, not feature):
  `0x81` pulse: `[0x81, side, on_us lo/hi, off_us lo/hi, repeat lo/hi]`, 8
  bytes, side 0 left / 1 right / 2 both, **left and right swapped** vs the
  2015 convention (kernel XORs). `0x80` rumble: `[0x80, type, intensity le16,
left speed le16, left gain, right speed le16, right gain]`, 10 bytes;
  Steam throttles to 20 Hz. `0x82`-`0x85` (command, LFO tone, log sweep,
  script) exist, undocumented.
- Mode switch in the kernel: hold menu (2.6) 0.45 s toggles gamepad mode;
  the kernel plays a pulse on toggle. The firmware's own right-menu hold
  (observed in NOTES: 0.5 s) does the same without the driver.
- `client_opened`: when any process opens the controller's hidraw node, the
  driver forwards every raw report to that client and unregisters its own
  evdev devices (`steam_work_connect_cb`). Consequence: while steamless holds
  hidraw, no `Wireless Steam Controller` evdev node exists, so there is
  nothing to grab; conversely Chromium holding it starves the kernel path.
  This is why the udev rule in section 8 exists.

Verified on a live 2026 controller + puck (2026-11-01, `steamless-cli decode`,
each control pressed by name): A, B, X, Y, D-pad x4, Steam, view, menu, quick
access, LB, RB, LT/RT (0..32767 and the full-pull bits), L3, R3, all four
grips, both pad clicks, both sticks (full range, sign), the touch flags for
sticks, pads and grips, battery report.

Unverified (must capture before relying on)

- Setting numbers: `SETTING_LIZARD_MODE` and `SETTING_STEAM_WATCHDOG_ENABLE`
  are enum positions in the C file; copy the numeric values from a build of
  the driver, not by counting.
- Whether `0x87` needs the feature report padded to 64 bytes over hidraw
  (`HIDIOCSFEATURE` sends what you give it; the kernel pads to report len).
- Trigger range on 0x45 vs 0x42, pad pressure units, stick deadzone.
- BLE (`1303`): report ids and whether the feature path works over GATT.
- Does the controller re-enter lizard mode on its own (watchdog) if the
  daemon dies? The watchdog setting suggests yes; measure the timeout.
- Puck slot 2-5 to hidraw node mapping and whether a controller moves slots.

Capture plan

1. `steamless-cli dump --hidraw /dev/hidrawN` prints raw reports with id,
   length and a hexdump diff against the previous report; press each control
   and record the byte.bit. Run on settop with the udev rule in place.
2. `echo "module hid_steam +p" > /sys/kernel/debug/dynamic_debug/control`
   on a 7.3 kernel, open the evdev device, read `dmesg` for the exact
   `Sending report ..` bytes of the lizard toggle. Replay them with
   `steamless-cli send --feature 01 81 00` and compare.
3. For haptics, send the kernel's mode-switch pulses (`0x190`, `0x14D`,
   `0x2D`) and confirm which side buzzes.
4. Save captures under `captures/` as text (`id len hex`); they become test
   vectors for `test_decode_*`.

## 4. Profile model

`~/.config/steamless/config.toml` (or `/etc/steamless/config.toml` from the
NixOS module). One file, several profiles, one active.

Default (Cal, 2026-11-01): with no config file, or a profile that sets
nothing, the daemon presents the controller as a plain Xbox-style pad, like
Steam Input's default: A/B/X/Y, bumpers, triggers, sticks, D-pad, view/menu,
Steam = guide. Chords and key bindings are user configuration and ship only
as an example (`examples/settop.toml`, the kiosk's volume chord), never as
built-in behaviour.

```toml
[daemon]
active = "settop"
lizard = "off"            # "off" | "on" | "auto" (off while a pad client exists)
imu = false

[profile.settop]
layout = "xbox"           # pad layout preset; only "xbox" at first
deadzone.stick = 0.10
deadzone.trigger = 0.05

[profile.settop.buttons]
a = "pad:south"
b = "pad:east"
x = "pad:west"
y = "pad:north"
lb = "pad:lb"
rb = "pad:rb"
lt_full = "pad:lt_click"  # digital full-pull, optional
menu = "pad:start"
view = "pad:select"
steam = "key:KEY_HOMEPAGE"
quick = "pad:guide"       # or "none"
grip_l1 = "none"
dpad_up = "pad:dpad_up"   # dpad_* default to pad hat

[profile.settop.axes]
lstick = "pad:left"
rstick = "pad:right"
lpad = "none"             # "pad:right" | "mouse" later | "none"
rpad = "none"
lt = "pad:lt"
rt = "pad:rt"

[[profile.settop.chord]]
hold = "view"             # modifier; its own output suppressed while held
dpad_up = "key:KEY_VOLUMEUP"
dpad_down = "key:KEY_VOLUMEDOWN"
dpad_left = "key:KEY_MUTE"
```

Semantics

- `Source` enum: every button bit and axis of the Ibex report by name.
- `Target` enum: `Pad(PadButton | PadAxis)`, `Key(u16 evdev code)`, `None`.
- `Chord`: `hold` is a source; while held, listed sources map to the chord
  targets and the hold source emits nothing. On release of `hold` with no
  chord member pressed since press, the hold source's own target is emitted
  as a press and release (a tap still works as the plain button; the cost is
  that its press arrives on release). Chords are the only mode shift in M1.
- Repeat: `key:` targets get kernel autorepeat (uinput keyboard
  `EV_REP`), so held volume keeps going. Pad buttons do not repeat.
- Flicks: `lstick = "keys:KEY_UP,KEY_DOWN,KEY_LEFT,KEY_RIGHT"` holds the
  key of the dominant direction once the stick passes `deadzone.stick`,
  releases it below 0.8 of that; the keyboard repeats at 400/150 ms.
- Profiles are switched at run time with `IpcRequest::Activate`; `pad` is
  the built-in Xbox layout unless the file defines it.
- Lizard policy: `off` (the default) sends the lizard-off sequence
  immediately on open and on every reconnect (`0x79` connect, puck slot
  re-open). `on` leaves firmware defaults (arrow keys/mouse) and still
  decodes reports. `auto` later.
- Per-app switching (M4): `[profile.x.match] title = "Dolphin"` or
  `unit = "chromium-kiosk.service"`; a watcher thread posts
  `Event::Activate(name)`.

## 5. Outputs

Virtual gamepad (`output::Pad`, one per controller)

- `name = "Microsoft X-Box 360 pad"`, `bustype = BUS_USB`, `vendor 0x045e`,
  `product 0x028e`, `version 0x0114`. Chromium's Linux gamepad backend and
  SDL's `GameControllerDB` both carry a mapping for this id, so the browser
  reports `mapping: "standard"` with no per-site hacks.
- Keys: `BTN_SOUTH/EAST/NORTH/WEST, BTN_TL, BTN_TR, BTN_SELECT, BTN_START,
BTN_MODE, BTN_THUMBL, BTN_THUMBR`. Abs: `ABS_X/Y/RX/RY` -32768..32767
  (flat 128, fuzz 16), `ABS_Z/ABS_RZ` 0..255 (triggers), `ABS_HAT0X/Y`
  -1..1. Exactly the set the real 360 pad exposes; no extra buttons, or
  Chromium falls back to a generic mapping.
- `uniq`: the controller serial if `ID_GET_STRING_ATTRIBUTE` works, else the
  hidraw path, so two controllers keep stable indices.

Virtual keyboard (`output::Keys`, one per daemon)

- `name = "steamless keys"`, bustype `BUS_VIRTUAL`, vendor `0x28de`, product
  `0x0001`. Enables only the key codes referenced by any profile plus
  `KEY_HOMEPAGE, KEY_HOME, KEY_VOLUMEUP, KEY_VOLUMEDOWN, KEY_MUTE, KEY_ESC,
KEY_ENTER`. `EV_REP` on. Cage/wlroots sees it as a keyboard and delivers
  to the focused Chromium surface; `home.js` listens for `keydown` and the
  volume keys go to the compositor's media-key handling or a tiny
  `wpctl` binding in the kiosk service.

Exclusive grab of the source

- Evdev backend: `EVIOCGRAB` on the kernel gamepad and the motion-sensors
  node so Chromium/SDL do not also see the raw `Wireless Steam Controller`.
- Hidraw backend: nothing to grab; the kernel removes its evdev devices
  while the node is open (`client_opened`). The udev rule keeps the node away
  from Chromium, which otherwise opens every uaccess hidraw it can.
- The daemon never grabs its own uinput devices.

## 6. IPC and GUI boundary

Socket: `dirs::runtime_dir()/steamless.sock` (`$XDG_RUNTIME_DIR`), mode
0600, created by the daemon, stale file removed on start. Framing: u32
big-endian length + `bincode` (`bincode-next`, `Encode`/`Decode` derives),
one thread per connection, request/response only, no subscriptions in M3.

```rust
pub enum IpcRequest {
    Status,                       // daemon + controllers
    Reload,                       // re-read config.toml, re-apply
    Activate(String),             // profile name
    Lizard(bool),                 // override until next Reload
    Haptic { controller: u8, side: IpcSide, on_us: u16, off_us: u16, count: u16 },
}
pub enum IpcResponse {
    Ok,
    Status(IpcStatus),            // active profile, Vec<IpcController{ id, path, battery, lizard, backend }>
    Error(String),
}
```

The GUI edits `config.toml` directly (it owns the file format via
`steamless-core::profile`), then sends `Reload`. It reads `Status` for the
controller list and battery, and `Haptic` for a "test" button. It never
streams input; a "press a button to bind" feature would need
`IpcRequest::Capture` and is deferred.

GUI toolkit candidates (no choice yet)

- egui/eframe: immediate mode, one binary, no web stack; plain looks.
- iced: Elm-style, native widgets look better, more boilerplate, slower
  compile; good fit for a form-shaped editor.
- Tauri: web front end, heavy toolchain and WebKitGTK on the kiosk; only
  worth it if the editor must also run remotely in a browser.

## 7. Crate layout and dependencies

```
Cargo.toml                workspace, resolver = "3", [workspace.package] edition 2024
rustfmt.toml              group_imports, imports_granularity = "Item", fn_params_layout
flake.nix                 package + nixosModules.default
crates/
  steamless-core/         lib: device, state, profile, output
    src/lib.rs            pub mod ...; pub use ...; pub type Result<T> = anyhow::Result<T>;
    src/device/mod.rs     Backend enum, open(), Event, Command
    src/device/hidraw.rs  ioctls, feature/output report writers, read loop
    src/device/ibex.rs    decode 0x42/0x45/0x43/0x79, consts, test vectors
    src/device/evdev.rs   kernel fallback, EVIOCGRAB
    src/state.rs          ControllerState, Button bitset, edges
    src/profile.rs        TOML types, load(), Mapper
    src/output/mod.rs     Output enum
    src/output/pad.rs     uinput gamepad
    src/output/keys.rs    uinput keyboard
  steamless-ipc/          message.rs (Ipc* types), client.rs, server.rs, path.rs
  steamless-daemon/       main.rs < 120 lines: env_logger, clap, start()
  steamless-cli/          dump / send / status / reload / haptic
  steamless-gui/          later (M3)
```

Dependencies

- `anyhow`, `log`, `env_logger`, `clap` (derive), `crossbeam-channel`,
  `dirs`, `bincode-next`: as in the style study.
- `evdev` (crate `evdev`): both the fallback reader and the uinput writer
  (`evdev::uinput::VirtualDeviceBuilder`). One crate for both sides, exposes
  `grab()`, abs setup, and key codes as constants; avoids a second uinput
  crate. Alternative `uinput` crate is unmaintained.
- hidraw: direct `nix` ioctls (`HIDIOCGRAWINFO`, `HIDIOCSFEATURE`,
  `HIDIOCGFEATURE`) over `std::fs::File` with `read()` for input and
  `write()` for output reports. Not `hidapi`: it links libusb/hidraw C,
  enumerates for us but hides the fd, and the Bluetooth path needs no extra.
- `toml` + `serde` (derive) for the config. This is the one departure from
  the no-serde habit; hand-parsing TOML is not worth it (open question 2).
- `udev` crate only if the rescan proves inadequate.
- No tokio/async: four blocking threads and one channel cover it; the
  hottest path is a 54-byte read at ~250 Hz.

## 8. Security / permissions

- Daemon runs as the kiosk user (`settop` session user), `systemd --user`
  unit or a system unit with `User=` set; never root. uinput access via
  `KERNEL=="uinput", GROUP="input", MODE="0660"` + the user in `input`
  (NixOS: `hardware.uinput.enable` or the module adds the rule).
- hidraw: `SUBSYSTEM=="hidraw", ATTRS{idVendor}=="28de",
ATTRS{idProduct}=="130[345]", MODE="0600", OWNER="<daemon user>",
TAG-="uaccess"`. Strips the logind uaccess ACL so Chromium cannot open the
  node (NOTES 2026-11-01: ACL re-applied each session, so chmod is not
  enough; `TAG-="uaccess"` in a rule after `70-uaccess.rules` is).
- Evdev fallback: the daemon needs read on the kernel gamepad node; same
  rule style on `SUBSYSTEM=="input"`.
- Socket 0600 in the user's runtime dir; no auth beyond filesystem.
- NixOS module options: `services.steamless.enable`, `.user`, `.config`
  (attrset rendered to TOML), `.package`. It sets `boot.extraModprobeConfig
= "options hid_steam lizard_mode=0"` only when the evdev fallback is on.

## 9. Milestones

M1: daemon reads 0x42, virtual pad, default Xbox profile

- hidraw open of `1304` slots, decode 0x42/0x45, `ControllerState`, pad
  output, keys output, TOML profile, the built-in Xbox default with no
  config, and `examples/settop.toml` (Steam = `KEY_HOMEPAGE`, view + D-pad
  up/down = volume, + left = mute) loaded by path for the kiosk.
- Accept: `evtest` on the virtual pad shows A -> `BTN_SOUTH`, sticks
  -32768..32767; Chromium `navigator.getGamepads()[0].mapping ===
"standard"`; `home.js` stops a game on Steam press without gamepad-index
  code; Dolphin's SDL input sees "Xbox 360 Controller"; `retrotv-bridge.py`
  removed from the kiosk config; daemon survives puck unplug/replug.

M2: lizard toggle, haptics, gyro

- Feature-report writer with retry, lizard off on connect, `0x80/0x81`
  output reports, IMU enable, IMU in `ControllerState`; evdev fallback
  backend with `EVIOCGRAB`.
- Accept: with `lizard_mode=1` kernel default and no `hid_steam` option,
  no arrow keys reach Cage while the daemon runs; `steamless-cli haptic`
  buzzes the named side; `steamless-cli dump` shows gyro changing when the
  controller tilts; killing the daemon returns lizard mode within the
  watchdog timeout (measured, documented).

M3: IPC + GUI

- `steamless-ipc` server/client, `steamless-cli status|reload|activate`,
  GUI skeleton with profile list, button table, test-haptic.
- Accept: edit TOML in GUI -> `Reload` -> new chord works without restart;
  `status` lists controllers with battery %.

M4: per-app profiles

- Watcher for the Cage foreign-toplevel title or systemd unit; `[match]`
  tables; `Activate` on change.
- Accept: launching Dolphin switches to a profile where Steam = `pad:guide`
  and back to `settop` when the launcher regains focus, visible in
  `status`.

## 10. Decisions and open questions

Decided by Cal (2026-11-01):

1. The virtual pad is visible to the whole seat (uaccess on the uinput
   device, like a USB pad).
2. Config: `toml` + `serde`, the one flagged serde departure.
3. Lizard mode is switched off immediately at daemon start.
4. No built-in chords: the default profile is a plain Xbox pad; the kiosk's
   volume chord is an example. A chord modifier tapped alone emits its own
   button on release.

Open:

5. M4 app matching: Wayland toplevel title under Cage, or the systemd unit
   that launched the app?
6. Haptic pulse on chord activation, or keep the pad silent on the settop?

## 11. Conventions

From the owner's Rust style study (`style-study/rust-STYLE-STUDY.md`,
checklist items 1-22, anti-patterns list):

- Workspace with `[workspace.package]` first, `edition = "2024"`,
  `resolver = "3"`, crates `steamless-<role>` under `crates/`, versions as
  `cargo add` prints them, `Cargo.lock` committed, dep groups under `#`
  headers with upstream URLs. Ship the 3-line `rustfmt.toml`; nightly fmt.
- One `use` per line, std / external / `crate::` groups; every 2+-param fn
  vertical. Module names one lowercase word (`hidraw`, `ibex`, `keys`).
- `anyhow` only: `pub type Result<T> = anyhow::Result<T>;` in core,
  `anyhow!("Short sentence {value}")`, `?`, no `thiserror`/`context`/
  `map_err`. Errors cross the socket as `String`.
- `unwrap()` on locks and channel sends, never `expect`; `unreachable!()`
  bare; non-fatal failures `warn!` and `continue`.
- `let Some(x) = .. else { continue; };` as primary control flow;
  exhaustive `match` with `=> {}` arms.
- Threads + `crossbeam-channel`, reply channel inside the request
  (`Event::Ipc(req, tx_response)`), `State(Arc<Mutex<StateInner>>)` with
  lock-per-method; no tokio, no async, no `RwLock`.
- Newtypes: `ControllerId(pub u8)`, `Button` bitset with `const fn`,
  hand-written `Display`, `From` both ways; `Ipc*` prefix for protocol
  mirrors; `Event` per module aliased on import (`DeviceEvent`).
- `log` + `env_logger::init()` first in `main`; `debug!` happy path,
  `info!` connect/disconnect/profile switch, `warn!` skipped data,
  `trace!` raw report dumps.
- Tests only on pure decoders (`test_decode_0x42_buttons`) in
  `#[cfg(test)] mod test` fed from `captures/`; no CI, README, LICENSE
  unless asked.
- Commits: lowercase subject-only fragments, `fmt` for whitespace, linear
  history.
- Departures, each flagged in code with `// TODO` where it hurts: `serde`
  for TOML (question 2); `nix` ioctl calls need `unsafe` blocks (no
  `// SAFETY` comments, inline).
