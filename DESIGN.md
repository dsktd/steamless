# yakin: design

Formerly named steamless.

Steam Input-style features for the Steam Controller family without Steam
running. A daemon reads the controller over hidraw (or the kernel evdev
device as a fallback), applies a profile, and presents a standard virtual
gamepad plus a virtual keyboard through uinput. An optional GUI edits the
profile and talks to the daemon over a local socket. Rust.

Prior art for the raw protocol (cited, not copied):

- Linux `hid-steam` 7.3 (`drivers/hid/hid-steam.c` at tag v7.3,
  https://github.com/torvalds/linux/blob/v7.3/drivers/hid/hid-steam.c; GPL-2.0,
  read as reference, not copied here): Ibex report layout, feature-report
  command path, lizard mode, haptics, `client_opened`.
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
 | "Xbox 360"  |     | KEY_HOME..  |     | yakin.sock   |
 +-------------+     +-------------+     +------------------+
                                                 ^
                                         yakin-gui / yakin-cli
```

Threads, not async: one reader thread per controller, one IPC accept thread
plus one per connection, the engine on the main thread draining a single
`crossbeam_channel` of `Event` (`State(id, ControllerState)`,
`Hotplug(..)`, `Ipc(request, tx_response)`). Hotplug via a udev monitor
thread (`udev` crate) or a 2 s rescan of `/sys/class/hidraw`; rescan first,
udev monitor if it bothers anyone.

### Steam

Observed with Steam and the daemon both running as the same user: Steam
holds every hidraw node of the controller open and adds its own virtual pad,
and also wraps our virtual pad as a second controller ("pad 1"), so games
see three pads. `[daemon] steam = "yield"` (default) makes the engine scan
`/proc/*/fd` every 2 s, and on each controller add, for other processes
holding its nodes. On a holder it releases everything, drops the pads, keys
and mouse, and pauses the readers, which close the nodes; dropping only the
readers would leave our pad for Steam to wrap. Once no holder is left it
recreates keys and mouse and unpauses: the nodes reopen, lizard mode goes
off again and the pads return with the active profile. `Activate` and
`Tune` while yielded apply then; `yakin-cli reclaim` forces an attempt.
Limit: only same-user holders are visible. `"ignore"` coexists.

### Profiles follow focus

`[auto] enabled = true` makes the profile follow the focused window. A thread
runs the desktop's `watch` (see Desktop backends), sending `Focus` events
(app id, title) to the engine; when it ends it watches again with backoff
(1 s to 30 s), as the daemon starts before the desktop. Niri's watch
connects to its socket (`$NIRI_SOCKET`, else the newest
`/run/user/<uid>/niri*.sock`), sends `"EventStream"` and tracks windows from
`WindowsChanged`, `WindowOpenedOrChanged`, `WindowClosed` and
`WindowFocusChanged`.
`[[auto.rule]] { app_id, title?, profile }` are regexes tried in order, the
first match wins, else `auto.default`, else the profile stays. A manual
switch (chord, menu, `Activate`) holds until focus moves to another app id;
title changes within the app keep it. While yielded to Steam nothing
switches; reclaim and reload apply the rules again.
The window edits `[auto]` too: enabled, default profile ("leave as is"), ordered rules, and
the focused app id from `Status.auto` with an "add rule for this app" button (it refreshes on
profile events). Regex errors (`auto.rule.N.field`) show on their row.

## 3. Device protocol notes (2026 controller, "Ibex")

Known from `hid-steam.c` (v7.3)

- Interfaces: wired = 1 unified HID interface; puck = 7 interfaces, 2-5 are
  controller slots, 6 is the pogo-pin interface (collection usage
  `0xFF000002`, ignore it). BLE (`1303`, bustype `0005`, kernel name
  `0005:28DE:1303.NNNN` under uhid, `HID_NAME` "Steam Ctrl (BT) <serial>") =
  1 interface: mouse `0x40`, keyboard `0x41`, then the vendor collection with
  the same report ids as USB (inputs `0x42` 53, `0x44` 5, `0x79` 1, `0x43`
  14, `0x7b` 12, `0x45` 45 bytes after the id; outputs `0x80`-`0x89`;
  features `0x01` and `0x02`, 63 bytes).
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
- Wireless event `0x79`, 2 bytes: 1 disconnect, 2 connect, 3 pair. Over BLE
  the hidraw node itself comes and goes with the link, and a `0x79` connect
  arrives when it comes up.
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
  evdev devices (`steam_work_connect_cb`). Consequence: while yakin holds
  hidraw, no `Wireless Steam Controller` evdev node exists, so there is
  nothing to grab; conversely Chromium holding it starves the kernel path.
  This is why the udev rule in section 8 exists. The firmware's own keyboard
  and mouse (the lizard device, one per link, `kbd`+`mouse` handlers, the
  same vendor and product as the hidraw node) is not among the devices the
  driver removes: it is plain hidinput on the real HID device and stays
  while yakin holds the node. `steam_register` (every connect, on any
  bus) sends lizard on when `lizard_mode` is 1, the module default; for the
  2026 controller (`STEAM_QUIRK_IBEX`) input open and close do nothing to
  it. The hidraw node is the driver's client HID device (`1303.0011`
  next to the real `1303.0010`), so the two are matched by `HID_PHYS` /
  `phys`, not by parent.

Verified on a live 2026 controller + puck (2026-11-01, `yakin-cli decode`,
each control pressed by name): A, B, X, Y, D-pad x4, Steam, view, menu, quick
access, LB, RB, LT/RT (0..32767 and the full-pull bits), L3, R3, all four
grips, both pad clicks, both sticks (full range, sign), the touch flags for
sticks, pads and grips, battery report.

Verified over Bluetooth LE (2026-11-04, settop, kernel 7.3.1 with
`hid-steam` bound, `cat` on the hidraw node for 20 s): the controller
streams `0x45` (46 bytes) at ~133 Hz, accel and gyro filled in, no
quaternion; `0x43` (15 bytes) every ~3 s with level and charge state; no
`0x42`; `0x79` connect only when the link comes up. The feature path is the
same as USB: feature report id 1, `HIDIOCSFEATURE` with 64 (or 63, 32, 3)
bytes all succeed over the BT HID control channel; the kernel driver makes no
difference between the busses either. For a moment after the `0x79` connect
the writes answer EIO (seen on both lizard-off and IMU-on), so the daemon
retries EIO 50 x 20 ms like the kernel does for EPIPE; EPIPE itself is what an
empty puck slot answers and is not retried. When EIO outlasts that, the reader
retries lizard off and IMU on every second for 30 s instead of giving up. A
reconnect makes a new uhid device whose hidraw node often reuses the old name,
so the rescan compares the node's inode with the one each reader holds and
stops a reader whose node was replaced; EIO, ENODEV or end of file on a read
closes the node too. The udev rule needs a second line
for BLE: there is no USB parent, so match `KERNELS=="0005:28DE:1303.*"`.

Unverified (must capture before relying on)

- Setting numbers: `SETTING_LIZARD_MODE` and `SETTING_STEAM_WATCHDOG_ENABLE`
  are enum positions in the driver source; copy the numeric values from a build of
  the driver, not by counting.
- Whether `0x87` needs the feature report padded to 64 bytes over hidraw
  (`HIDIOCSFEATURE` sends what you give it; the kernel pads to report len).
- Trigger range on 0x45 vs 0x42, pad pressure units, stick deadzone.
- BLE (`1303`): haptics (output reports `0x80`/`0x81`) over BLE, and whether
  the IMU keeps streaming without `SETTING_IMU_MODE` once nothing else has
  opened it (it was streaming on first contact).
- Does the controller re-enter lizard mode on its own (watchdog) if the
  daemon dies? The watchdog setting suggests yes; measure the timeout.
- Puck slot 2-5 to hidraw node mapping and whether a controller moves slots.

Capture plan

1. `yakin-cli dump --hidraw /dev/hidrawN` prints raw reports with id,
   length and a hexdump diff against the previous report; press each control
   and record the byte.bit. Run on settop with the udev rule in place.
2. `echo "module hid_steam +p" > /sys/kernel/debug/dynamic_debug/control`
   on a 7.3 kernel, open the evdev device, read `dmesg` for the exact
   `Sending report ..` bytes of the lizard toggle. Replay them with
   `yakin-cli send --feature 01 81 00` and compare.
3. For haptics, send the kernel's mode-switch pulses (`0x190`, `0x14D`,
   `0x2D`) and confirm which side buzzes.
4. Save captures under `captures/` as text (`id len hex`); they become test
   vectors for `test_decode_*`.

## 4. Profile model

The config is layered, like systemd's `*.conf.d`: a baseline `--config`
(default `~/.config/yakin/config.toml`), then every `*.toml` in
`--config-dir` (`$YAKIN_CONFIG_DIR`, default `~/.config/yakin/config.d`)
in file name order. Layers merge as TOML values: tables key by key, anything
else (a chord array, `daemon.active`) replaces, so a drop-in can change one
binding or add a profile (`examples/config.d/50-example.toml`). `Reload` re-reads
every layer; a file that fails to parse is named and the old config stays.
The editor writes only `90-editor.toml`. On NixOS the baseline is the
immutable `/etc/yakin/config.toml` and `services.yakin.configDir`
(default `/var/lib/yakin/config.d`, owned by the daemon's user) holds the
drop-ins. Several profiles, one active.

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
imu = false               # true: stream raw accel/gyro (SETTING_IMU_MODE 48); the puck needs it for gyro
steam = "yield"           # "yield" | "ignore": step aside while Steam holds the controller

[profile.settop]
layout = "xbox"           # pad layout preset; only "xbox" at first
deadzone.stick = 0.10
deadzone.trigger = 0.05
stick.touch = true        # a stick counts only while its cap is touched (default)

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
- Stick directions: `lstick_up`/`_down`/`_left`/`_right` and `rstick_*` are
  virtual buttons, pressed while the stick (as the profile sees it, so after
  `stick_touch` gating) points that way past half deflection and released
  below 0.4, the menu's thresholds. They bind like any button, in `[buttons]`
  and chords (as members or `hold`), with activators such as `turbo_ms`; the
  stick's own axis binding keeps running alongside.
- Activators: a button (or chord member) binding may be a table,
  `a = { press = "pad:south", long = "key:KEY_ESC", double = "mouse:middle", start = "key:KEY_F",
release = "key:KEY_G", long_ms = 400, double_ms = 250, turbo_ms = 100 }`; the string form is
  `{ press = ... }`, and a table without `press` keeps the layout's. Steam's semantics: `start`
  taps on every press edge, `release` on every release edge; `press` holds until release;
  `long` holds once the button is down `long_ms` (default 400), and then `press` only taps on a
  shorter release; `warn_ms` (below `long_ms`, off by default) warns that long before `long`
  fires, with a long buzz on the right pad and a 1 kHz beep from the left pad's tone generator
  (report 0x83, SDL's `MsgHapticLfoTone`), so a release can still cancel it; `double` holds from a second press within `double_ms` (default 250) of the
  first. Tap latency: with `double` set, a single press's `press` waits out the window (taps on
  release after it, or holds once held past it). `turbo_ms` toggles `press` every half period
  while held. Timers run on reports (`Mapper::map_at(.., now)`), so they fire within ~4 ms.
- Repeat: `key:` targets get kernel autorepeat (uinput keyboard
  `EV_REP`), so held volume keeps going. Pad buttons do not repeat.
- Stick touch: the 2026 sticks have capacitive caps (buttons 4.4 and 5.0).
  With `stick.touch = true` (the default) a stick whose cap is not touched
  reads as centred for everything that reads it: pad axes, flicks, cursor,
  scroll, flick stick, the menu and the keyboard. The touch bit may drop out
  for `TOUCH_DEBOUNCE` (150 ms) before the stick lets go, then a held flick
  key releases and motion stops at once; touch counts at once. A stick click
  without touch still clicks. `false` reads the sticks raw, as before.
  Measured on settop over BLE (2026-11-05, 133 Hz capture): a thumb settling
  on the cap flickers the bit in OFF runs of 13-77 reports (0.1-0.6 s) for
  its first ~3 s, hence the long release side; rubbing the stick on couch
  fabric with no skin contact sets the bit for 94% of the moved reports, so
  the caps report fabric as touch and the gate cannot tell them apart.
- Flicks: `lstick = "keys:KEY_UP,KEY_DOWN,KEY_LEFT,KEY_RIGHT"` holds the
  key of the dominant direction once the stick passes `deadzone.stick`,
  releases it below 0.8 of that; the keyboard repeats at 400/150 ms.
- Profiles are switched at run time with `IpcRequest::Activate`; `pad` is
  the built-in Xbox layout unless the file defines it.
- Profile targets: `profile:<name>`, `profile:next`, `profile:prev` on
  buttons and chord members (not axes). They fire on release, so the press
  never reaches the new profile; the engine then switches like Activate
  (`next`/`prev` cycle the sorted names, the active one is a no-op). A name
  that is no profile fails config load, naming the binding.
- Action layers (Steam's name): `[profile.x.layer.aim]` holds `buttons` and `axes` tables that
  replace only the profile's bindings they name while the layer is active (a table without
  `press` keeps the profile's). Targets `layer:hold:aim` (active while the binding is held) and
  `layer:toggle:aim` (flipped on each press) on buttons and chord members; the name must be a
  layer of the same profile. Several layers stack, the newest on top. A button keeps the binding
  it was pressed with until release; an axis whose target a layer replaces is released and
  centred first. Switching profile drops every layer. Layers have no chords or knobs of their
  own. Not to be confused with config file layers above.
- Lizard policy: `off` (the default) sends the lizard-off sequence
  immediately on open and on every reconnect (`0x79` connect, puck slot
  re-open). `on` leaves firmware defaults (arrow keys/mouse) and still
  decodes reports. `auto` later.
- TV off: `tv:off` runs `daemon.tv_off` (a shell command, e.g. CEC Standby)
  on press, releases everything held, and the engine's `Tv` gate then drops
  every output until a button (not a touch) is held for `daemon.tv_wake_ms`
  (default 1000), which runs `daemon.tv_on` once; short presses do nothing,
  and the waking press is swallowed down to its release (with any other
  button held meanwhile). yakin tracks only what it did itself, so a TV woken
  by its remote idles yakin until that hold. A controller connecting while
  idle leaves it idle (a Bluetooth controller reconnects by itself after a
  power-down); connecting while on runs `daemon.tv_on` (active source). Chords stack,
  so `hold = "grip_r_touch"` with `menu = { press = ..., long = "tv:off",
  long_ms = 3000 }` leaves the other chords working while the grip is held.
- Lizard by button: `lizard:hold` (lizard on while the binding is held, off
  on release) and `lizard:toggle` (flipped on each press) on buttons and
  chord members, puck and BLE alike. They use the same command pairs as the
  policy (`0x85`+`0x8E` on, `0x81`+settings off, then IMU on again as
  `0x8E` resets it). While on, the engine's `Lizard` gate drops every output
  of that controller (pad, keys, mouse, desktop actions, menu); on the way in
  it releases everything yakin held, on the way out the mapper forgets what
  was pressed meanwhile, so nothing sticks. `press = "desktop:overview",
  long = "lizard:hold", long_ms = 300` gives tap-overview, hold-lizard on one
  button; lizard engages once the hold is recognised. While yielded to Steam
  no report is mapped, so the bindings are moot. With policy `on` the
  release leaves lizard on. A release never seen (the link went away while
  held) is made up for on disconnect: lizard off is sent to the node right
  away, and if the link was already gone, on the node's next report. The
  reader also reports how its setup ended (`Event::Lizard`), so the status
  shows the firmware's state as last commanded, `?` until then.
- Lizard grab: the lizard device of every link whose hidraw node yakin
  holds is grabbed (`EVIOCGRAB`, `device::grab`), idle links included, so
  whatever the firmware types on its own never reaches the desktop next to
  yakin's keys. Released only for a link handed over by `lizard:hold` or
  `lizard:toggle`, and for all while yielded. Links are rescanned on every
  node event and tick; a node that comes back under the same name (new
  inode) is grabbed again. Grab and release are logged at info.
- Lizard leaks: the grabbed devices are read on every tick (never
  forwarded). Keys or motion there while lizard mode should be off on that
  link mean the firmware turned it back on: one warning per burst, a count
  per node (`leaks N` in the status) and lizard off sent again.
- Status: every open node is reported with its link (puck slot, Bluetooth,
  wired, from `HID_ID`) and whether a controller is on it (a puck slot from
  its `0x79` connect or first input until its disconnect). `yakin-cli
  status`, the UI's status strip and the tray list only those that carry one.
- Per-app switching (M4): `[profile.x.match] title = "Dolphin"` or
  `unit = "chromium-kiosk.service"`; a watcher thread posts
  `Event::Activate(name)`.

Mouse mode (M2)

- Targets: pads and sticks `= "cursor" | "scroll"`, sticks `"flick"`, buttons
  `"mouse:left" | "mouse:right" | "mouse:middle"`. `examples/settop.toml`
  has a `desktop` profile for GNOME on the TV.
- Cursor: while a pad is touched, the finger's delta since the last report,
  times `CURSOR_GAIN` (1440 / 65534 px per pad unit: one full-width swipe
  crosses three quarters of a 1920 px screen) times `mouse.speed` (default 1.0) times
  `1 + mouse.accel * min(|delta| / 2000, 1)` (accel default 1.3). The first
  touched report and lifted pads emit nothing, so a finger landing elsewhere
  does not jump; the sub-pixel remainder carries over between reports.
- Release guard: each delta waits `mouse.release_guard` reports (default 3,
  max 20) in a per-pad queue before it moves the cursor or scroll; lifting the
  finger discards the queue, so the twitch of the last reports never shows.
- Smoothing: before the delta, each pad coordinate goes through a 1€ filter
  (Casiez, Roussel, Vogel, CHI 2012) at an assumed 100 Hz; `mouse.smoothing`
  (0..1, default 0.5, 0 = off) sets cutoff = (1 Hz + 0.002 * |speed|) / smoothing.
- Trigger clicks: `lt`/`rt` may bind a button target (`mouse:*`, `key:*`,
  `pad:<button>`); it presses past `trigger.click` of travel (default 0.3)
  and releases below 0.8 of that.
- Scroll: the same delta, times `SCROLL_GAIN` (a full swipe = 10 notches of
  120 hi-res units) times `scroll.speed`; `scroll.natural` (default true)
  makes the content follow the finger. Per touch, the scroll stays still until
  the finger is 1/50 of the pad from where it landed (a resting thumb never
  creeps), then locks to the dominant axis, switching only when the other
  axis's smoothed speed is 2.5 times higher (no sideways scroll from a wobbly
  swipe), and eases its output with a 15 Hz exponential filter.
- Sticks: `cursor`/`scroll` by velocity: deflection past the deadzone, rescaled
  to 0..1 and raised to `mouse.stick_curve` (1..3, default 1.6), gives
  `mouse.stick_speed` px/s (1000) or `scroll.stick_speed` notches/s (10) at full
  tilt, per report at the assumed 100 Hz with whole-pixel carry; no guard or 1€.
- Flick stick (`flick`, sticks only): leaving the deadzone turns by the stick's
  angle, eased over `mouse.flick_ms` (100); held, the wrapped angle delta turns
  as is. `mouse.flick_degrees` = px per 360° (2400). Import: `joystick_mouse` →
  `cursor`, stick_speed = sensitivity / 100 * 1000 (rough); `flickstick` → `flick`.
- Gyro (`axes.gyro = "cursor"`, the only target of the `gyro` source; unbound by default): the
  IMU's yaw and pitch rate (report bytes 44 and 40, hid-steam `steam_ibex_imu_mappings`,
  `STEAM_GYRO_RES_PER_DPS` = 16 units per °/s) over one report at the assumed 250 Hz, times
  `GYRO_GAIN` (40 px per degree, a 1920 px screen is 48°) times `gyro.speed` (default 1.0), with
  whole-pixel carry; rates under 1 °/s count as still. `gyro.button = "<button>"` enables it only
  while that button is held (its own binding still fires); unset, always on. Needs
  `daemon.imu = true` on the puck (sends `SETTING_IMU_MODE` raw accel|gyro on open and on every
  reconnect, like hid-steam's `steam_sensor_open`, line 948). Unverified on hardware: the setting
  number (counted, 48), the signs (turn right = right, tilt top away = up) and the gain.

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

- `name = "yakin keys"`, bustype `BUS_VIRTUAL`, vendor `0x28de`, product
  `0x0001`. Enables only the key codes referenced by any profile plus
  `KEY_HOMEPAGE, KEY_HOME, KEY_VOLUMEUP, KEY_VOLUMEDOWN, KEY_MUTE, KEY_ESC,
KEY_ENTER`. `EV_REP` on. Cage/wlroots sees it as a keyboard and delivers
  to the focused Chromium surface; `home.js` listens for `keydown` and the
  volume keys go to the compositor's media-key handling or a tiny
  `wpctl` binding in the kiosk service.

Virtual mouse (`output::Mouse`, one per daemon)

- `name = "yakin mouse"`, bustype `BUS_VIRTUAL`, vendor `0x28de`, product
  `0x0002`. `REL_X/Y`, `REL_WHEEL/HWHEEL` and their `_HI_RES` twins,
  `BTN_LEFT/RIGHT/MIDDLE`. Scroll is sent hi-res, with a whole-notch
  `REL_WHEEL` whenever 120 units have built up, like a real wheel.

Desktop control (Wayland only; see Desktop backends below)

- `key:LEFTMETA+D`: a combo, any `KEY_*` names joined by `+` (`KEY_` optional), valid wherever
  `key:` is; pressed in order, released in reverse. Imported from several `key_press` bindings
  on one Steam activator. The keyboard enables every key of every combo.
- `desktop:<action>`: a common action (table below), run on press by the selected backend.
  `desktop:workspace:<n>` (1-255) and `desktop:spawn:<cmd>` (split on whitespace) take an
  argument. Failed and unsupported actions are logged at most once a minute.
- `niri:<action>`: niri's escape hatch, for what no common action covers (`desktop::Action::Raw`,
  run by the niri backend only): any of niri 25.11's actions by its kebab-case name (`desktop::niri::NIRI_ACTIONS`,
  the list of `niri msg action --help`), sent as its PascalCase variant with no fields,
  `{"Action":{"FocusColumnLeft":{}}}`: niri-ipc lets every `Option` field be left out. The few
  with required flags get niri's CLI defaults (`quit` with its confirm dialog, `screenshot*`,
  `move-*-to-workspace-up/down` focusing). Arguments as `niri:<name>:<arg>` in niri's CLI syntax
  for `focus-workspace:3` (index, else name), `move-column-to-workspace:2`,
  `move-window-to-workspace:2`, `set-column-width:+10%`, `set-window-width:640`,
  `set-window-height:50%` and `spawn-sh:<shell line>`; `niri:spawn:<cmd>` splits on whitespace.
  Actions with other required fields (`focus-monitor`, `switch-layout`, ...) are refused. Sent
  on press over a fresh connection to `$NIRI_SOCKET`; unset (the system service), the newest
  `/run/user/<own uid>/niri*.sock`, as the service runs as the session user. Failures are logged
  at most once a minute. Unknown names and bad arguments fail at config load.
- `flicks:<up>,<down>,<left>,<right>`: like `keys:` but each slot is any button-like target
  (a bare name is a key), so a stick flick can focus windows.

Desktop backends (`yakin-core/src/desktop/`)

A backend implements two methods; everything else is shared: backend selection, watching
again with backoff (`yakin-daemon/src/desktop.rs`), rate-limited logging of failed and
unsupported actions, focus rules and profile switching.

```rust
pub trait Desktop: Send {
    /// Run one action; Ok(false) when this desktop has no such action.
    fn act(&mut self, action: &Action) -> Result<bool>;
    /// Report every focus change (None = nothing focused) until the connection ends; blocks.
    fn watch(&mut self, focus: &mut dyn FnMut(Option<Window>)) -> Result<()>;
    fn name(&self) -> &'static str { "desktop" }
}
pub struct Window { pub app_id: String, pub title: String }
```

| `desktop:`          | niri                                                        |
| ------------------- | ----------------------------------------------------------- |
| `overview`          | `toggle-overview`                                           |
| `close-window`      | `close-window`                                              |
| `fullscreen`        | `fullscreen-window`                                         |
| `maximize`          | `maximize-column`                                           |
| `focus-left/right`  | `focus-column-left/right`                                   |
| `focus-up/down`     | `focus-window-up/down`                                      |
| `move-left/right`   | `move-column-left/right`                                    |
| `move-up/down`      | `move-window-up/down` (within the column, as focus-up/down) |
| `workspace-up/down` | `focus-workspace-up/down`                                   |
| `workspace:<n>`     | `focus-workspace:<n>`                                       |
| `spawn:<cmd>`       | `spawn:<cmd>`                                               |
| `screen-off`        | `power-off-monitors`                                        |
| `screenshot`        | `screenshot`                                                |
| `niri:<raw>`        | `<raw>` as is                                               |

Selection: `[desktop] backend = "auto" | "niri" | "none"`, default `auto`. `auto` takes the
first backend detected (niri: its socket is found), detected again on every action and every
watch attempt, so a session that starts after the daemon is found. `none` runs no action
(each is unsupported) and never reports focus, so `[auto]` rules idle. `Status.desktop` and
`yakin-cli status` name the backend in use. A reload applies a new backend to actions; the
focus watch keeps the one it started with.

Adding a backend: a type implementing `act` (map each `Action` it can, `Ok(false)` for the
rest, `Raw` included) and `watch`, its own `name`, a `Backend` variant and one detection line
in `Selected::detect`. Each call may open its own connection, as niri's does; the backend
may keep state if it wants.

Quick menu (`menu.rs`, drawn by yakin-osd)

- `menu:toggle`, `menu:open`, `menu:close` on any button; settop binds `quick` (the "..." button).
- `[menu]`: `columns` (1-8, default 4) and `items = [{ label, icon, target }]`, any button target
  but `menu:`; icons are built-in stroked glyphs (`menu::ICONS`). Absent, a 4x3 default: launcher,
  keyboard (`keyboard:toggle`), overview, fullscreen, maximize, close, volume, pad/desktop
  profiles, screen off. `examples/settop.toml` writes it out.
- Opening releases everything the profile holds (`Mapper::activate`, sticks centred). While open
  each controller's reports go to `MenuNav`, not the mapper: D-pad and left stick (repeat after
  400 ms, every 150 ms) move the focus, wrapping; A, the right pad click or RT fire the item as a
  press and release under the profile, then close; B or a `menu:toggle`/`close` button closes.
- `IpcEvent::Menu(Option<IpcMenu>)` on open, each focus change, close, and in the subscribe
  snapshot. The OSD draws it on its own full-screen overlay surface, no input: dimmed screen, a
  centred panel inside 5 % safe margins, square tiles of 120-160 logical px (22-18 px labels), the
  focused one ringed (4 px) and scaled by 1.08. Toasts keep their own surface above it.
- The window edits `[menu]` (label, icon from `Vocabulary.menu_icons`, target, order, columns 2-6,
  labels-only preview grid) next to the bindings; see the editor section.

Keyboard (`keyboard.rs`, drawn by yakin-osd)

- `keyboard:toggle|open|close` on any button or menu item; settop's quick menu and the desktop
  Steam chord's `menu` toggle it (wvkbd stays an option in a comment). Opening closes the menu
  and releases what the profile holds; while open each report goes to `Keyboard::read`, not the
  mapper or `MenuNav`, and types through the keys device: no input-method protocol.
- Steam-style split QWERTY: digits, three letter rows, then shift, 123 (symbols and Esc, Tab,
  Home, End, PgUp/PgDn, arrows), space, backspace, enter, close; ten columns, each trackpad
  points absolutely at its half (row by Y, column by X) and its click or trigger types the key.
- D-pad + A move and type a focus; View backspace, B closes it (like Esc), Y space, X shift (one-shot, twice caps lock),
  LB/RB arrow left/right, menu enter, Steam or a `keyboard:` toggle/close button closes.
- A shifted character presses LEFTSHIFT around its key, one SYN report per key event; the keys
  device advertises every key of both layers (`keyboard::keys`).
- `IpcEvent::Keyboard(Option<IpcKeyboard>)` with the labelled rows. The OSD docks it on a
  bottom-anchored, full-width, no-input surface 40 % of the screen high: translucent panel in 5 %
  side margins, the split marked by a faint line, left finger blue, right orange, focus ringed.

Exclusive grab of the source

- Evdev backend: `EVIOCGRAB` on the kernel gamepad and the motion-sensors
  node so Chromium/SDL do not also see the raw `Wireless Steam Controller`.
- Hidraw backend: the kernel removes its gamepad and sensor devices while
  the node is open (`client_opened`); the lizard device stays and is grabbed
  (section 4). The udev rule keeps the node away from Chromium, which
  otherwise opens every uaccess hidraw it can.
- The daemon never grabs its own uinput devices.

### Haptics

Both are HID output reports written to the controller's hidraw node (hid-steam 7.3, Ibex branch;
ids at lines 329-334). `crates/yakin-core/src/device/haptics.rs` has the byte layouts and their tests.
- Pulse `0x81` (`steam_haptic_pulse`, line 751; `struct steam_ibex_haptic_pulse`, line 377):
  `[0x81, side, on_us, off_us, repeat]`, le16, 8 bytes, left 1 / right 0 (the driver XORs). The
  Ibex report has no gain, so strength is a duty cycle of a 2 ms period (a guess, tune by feel).
- Rumble `0x80` (`steam_haptic_rumble`, line 793; `struct steam_ibex_haptic_rumble`, line 367):
  10 bytes, type 0, intensity 0, left = strong motor with gain 2, right = weak with gain 0, as
  `steam_play_effect` (line 860) sends; 20 Hz, repeated while running (`steam_coalesce_rumble_cb`, line 844).
- `Output::Haptic { side, strength, ms }`: `haptics.click` on a button press that clicks or keys
  (side of the button), `haptics.pad_ticks` per 1/32 pad width of cursor/scroll travel,
  `haptics.menu` on quick menu and keyboard focus moves and activation. The engine sends them on the
  controller the input came from, at most one per side per 15 ms.
- Rumble passthrough (`daemon.rumble`, default on): the virtual pad advertises `FF_RUMBLE`; a thread
  answers `UI_FF_UPLOAD`/`UI_FF_ERASE` and plays the loudest running effect per motor.
- Lizard cue (`daemon.lizard_cue`, default on): a `lizard:` binding taking over is two 30 ms
  full pulses 60 ms apart on the right trackpad, handing back one, played from a thread; the OSD
  toasts "Mouse mode" / "Mouse mode off" from the status's `mouse` flag. `yakin-cli cue on|off
  [--hidraw]` plays it directly on the node, to feel it without a binding.
- Unverified until felt on hardware: side swap, the duty-cycle mapping, gains, motor timeout.

## 6. IPC and GUI boundary

Socket: `dirs::runtime_dir()/yakin.sock` (`$XDG_RUNTIME_DIR`), mode
0600, created by the daemon, stale file removed on start. Framing: u32
big-endian length + `bincode` (`bincode-next`, `Encode`/`Decode` derives),
one thread per connection; `Subscribe` turns a connection into an event stream (see UI).
A quiet subscriber checks its socket every 250 ms and closes on hangup or EOF; the engine drops closed slots on every tick.
A failed accept (out of fds or memory) waits 100 ms doubling to 2 s, and the same error is logged at most once per 10 s.
Clients count a subscription shorter than 5 s as a failure for their backoff and log why it ended.

```rust
pub enum IpcRequest {
    Status,                       // daemon + controllers
    Reload,                       // re-read every config layer, re-apply
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

The GUI edits through `GetConfig`/`SetConfig` (see UI), never the files. It reads `Status` for the
controller list and battery, and `Haptic` for a "test" button. It never
streams input; a "press a button to bind" feature would need
`IpcRequest::Capture` and is deferred.

GUI toolkit candidates (no choice yet)

- egui/eframe: immediate mode, one binary, no web stack; plain looks.
- iced: Elm-style, native widgets look better, more boilerplate, slower
  compile; good fit for a form-shaped editor.
- Tauri: web front end, heavy toolchain and WebKitGTK on the kiosk; only
  worth it if the editor must also run remotely in a browser.

### UI

`IpcRequest::Subscribe` is answered `Ok`, then the connection streams
`IpcEvent` frames (same framing) until the client closes it: `State { id,
state }`, `Profile(name)` on subscribe/activate/reload, `Controllers(..)` on
subscribe and any device change. The engine only overwrites a per-subscriber
slot (latest state per controller, 16 profile/controller events), so a slow
client loses frames and never slows input; its thread sends at most one state
per controller per 1/60 s tick, skipping states equal but for `seq`.
`crates/yakin-ui` is a Dioxus app drawn by Blitz (`dioxus-native`, left out of
`default-members` for its build time): one native Rust process, no webview and no
JavaScript, Wayland only. A thread stays subscribed and feeds the stream into signals;
every daemon call is Rust with the IPC types end to end. The widgets are the shadcn-style
`dioxus-primitives` (not on crates.io; pinned by revision and ported to Dioxus 0.8 by hand
in `vendor/dioxus-primitives`, see its VENDOR) with the styled wrappers copied into
`src/components/` (their licence next to them); the stylesheets go in a `<style>` at the top
of the page. Blitz makes no click out of Enter or Space on a button and reports no `change`
event, so the wrappers (and the vendored select and tab triggers) take those keys
themselves and `Input` fires `onchange` on blur or Enter. The drawing is `ui/assets/controller.svg` at the repo root, linked in, made from
Valve's STL by `cad/drawing.py` (`just assets`, see NOTICE), shown as is: what is pressed
goes on its wrapper as data tokens that CSS matches to element ids, the stick, pad and
trigger positions on an overlay in the same coordinate space. Run `just ui`;
`yakin-cli watch` prints the same stream.

The 3D page shows `crates/yakin-view`'s scene inside the window: Valve's shell (decimated
to glTF by `cad/convert.py`, `just model`, see NOTICE; read from `$YAKIN_MODEL` or the
package's `share/yakin/controller.glb`) with a flat overlay on every control placed by hand
in CAD millimetres (`layout.rs`). Bevy runs on a thread without a window, drawing into a
texture on Blitz's own wgpu device (the `RenderPlugin` is handed Blitz's `DeviceHandle`,
and the page asks wgpu for `TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES` at launch); a custom
widget (`view.rs`, an `<object>` element) paints that texture and sends the pointer over a
channel: a drag orbits, the wheel zooms, a press that travels under 4 px is a click, which
`scene.rs` ray-casts to the overlay under it (or the nearest one to the point hit on the
shell). The camera swings round to the picked control, the overlays light with the
`Subscribe` stream (pressed cyan, touched faint, triggers sink, sticks lean, a dot on a
touched pad), and the panel beside the view shows the control's binding rows (the axis it
sits on too) from the same draft the Edit page edits, with Save and Revert. With the controller alone (the `desktop` profile's arrows, Enter and
Escape): the panel lists every control, focused when the tab opens; the D-pad walks it and
the camera swings to each as a click does, A focuses the binding's kind select (the select
takes the D-pad and A itself), B backs out to the list. The right stick turns and the
triggers' travel zooms, read from the `Subscribe` state the scene already has, while the tab
draws. `yakin-view`
is the same scene in a window of its own (`just view`), behind its `window` feature.

`IpcRequest::Tune(IpcTuning)` sets a profile's mouse, scroll and deadzone
knobs in the daemon's memory only; the active profile's mappers take it on the
next report, nothing is released, and `IpcEvent::Tuning` tells other windows.
The file is never written: `Reload` reverts every tuning. `IpcStatus` lists
every profile (`pad` included) with its tuning. The UI sidebar drives this with
profile buttons, throttled sliders (~10/s) and a Reload button.

The Edit view edits bindings on top of `GetConfig`, which returns every layer
as written (`IpcConfig { baseline, dropins, effective, editor_path, writable }`,
each a `ConfigToml`: names and target strings, not resolved enums). Each row
is tagged with the layer that sets it (`baseline`, a drop-in's file name, or
`yours`) and can be reset to the layers under the editor's. `SetConfig(ConfigToml)`
takes the effective config and writes to `<config-dir>/90-editor.toml` only
what differs from the other layers merged (equal keys and equal profiles left
out, a new profile whole), after validating the merge through the same
`TryFrom` as loading (errors name the binding, e.g. `Profile pad: buttons.a: ...`,
shown next to that row). It writes a temp file and renames it (the leading
comment block stays), creates the directory 0750 if needed, applies it like
`Reload` and sends `IpcEvent::Config`. A directory that cannot be written is
refused up front with `config is read-only: <dir>`. A binding can be changed
but not removed below the editor's layer, and only profiles the editor's
drop-in alone defines can be deleted. `Vocabulary` lists sources, layouts, target
kinds and what each source accepts, derived from the validation rules, and the
`menu_icons` names. Below the bindings sit the Quick menu and Focus rules sections (see
those chapters); their arrays replace whole, so each header carries one layer tag.

Two checks guard the window: `cargo test -p yakin-ui` (in `just check`) drives the editor's
model (`src/draft.rs`, no UI) against `tests/fixtures/*.json`, the daemon's own Vocabulary and
GetConfig for `examples/` (a daemon test fails when stale; `just ui-fixtures` rewrites them):
targets, activators, layers, profiles, the saved diff and error placement; `just ui-smoke`
opens the window under headless Weston with `YAKIN_SMOKE=1`, no daemon, and exits 0 once the
page has rendered, 2 if it has not within 20 s.

Icon: `ui/assets/icon.svg`, `icon-symbolic.svg` and `icons/<size>.png`, the CAD outline simplified, made with the drawing.
Desktop entry `dev.dsktd.yakin.desktop` matches the window's Wayland app_id (`glib::set_prgname`).
`nix build .#yakin-ui` installs both; the module adds it with `services.yakin.ui = true`.

### OSD

`yakin-osd` shows short toasts on a wlr-layer-shell `overlay` surface (smithay-client-toolkit,
resvg; Wayland only). Not desktop notifications: settop runs no notification daemon, and a toast
must not stack, linger in a history or take focus. The surface exists only while a toast is up,
has an empty input region and no keyboard interactivity, and fades out over 200 ms.
It subscribes like the UI and toasts profile switches, Steam taking and returning the controller,
controllers connecting or leaving, and a battery under 15 % (once, until above 20 %). The choice
is the pure `toast(prev, event)`, unit-tested; a fresh subscription's opening events are state,
not changes, and stay silent, as does the daemon going away. `just osd-smoke` runs it against a
headless sway (Weston has no layer-shell) and exits at the first frame callback, once for a
toast, once for the default quick menu (`--menu-demo --once`) and once for a sample keyboard
(`--keyboard-demo --once`); `--menu-svg`/`--keyboard-svg <file>` write them over 1080p at scale
2, which the recipe renders to `$YAKIN_MENU_PNG`/`$YAKIN_KEYBOARD_PNG` for review. The menu
is the pure `menu_svg(menu, screen)`; a daemon that goes away takes its menu down.
`services.yakin.osd` adds a systemd user service wanted by `graphical-session.target`, which
niri's `niri.service` binds (`niri-session`); its package sets FONTCONFIG_FILE to DejaVu Sans.

## 7. Crate layout and dependencies

```
Cargo.toml                workspace, resolver = "3", [workspace.package] edition 2024
rustfmt.toml              group_imports, imports_granularity = "Item", fn_params_layout
flake.nix                 package + nixosModules.default
crates/
  yakin-core/         lib: device, state, profile, output
    src/lib.rs            pub mod ...; pub use ...; pub type Result<T> = anyhow::Result<T>;
    src/device/mod.rs     Backend enum, open(), Event, Command
    src/device/hidraw.rs  ioctls, feature/output report writers, read loop
    src/device/ibex.rs    decode 0x42/0x45/0x43/0x79, consts, test vectors
    src/device/evdev.rs   kernel fallback, EVIOCGRAB
    src/state.rs          ControllerState, Button bitset, edges
    src/profile.rs        TOML types, load(), Mapper
    src/desktop/mod.rs    Desktop trait, Action, Backend selection
    src/desktop/niri.rs   niri backend: actions and event stream
    src/output/mod.rs     Output enum
    src/output/pad.rs     uinput gamepad
    src/output/keys.rs    uinput keyboard
  yakin-ipc/          message.rs (Ipc* types), client.rs, server.rs, path.rs
  yakin-daemon/       main.rs < 120 lines: env_logger, clap, start()
  yakin-cli/          dump / send / status / reload / haptic / import
  yakin-import/       Steam Input .vdf to ProfileToml, coverage tally (cli only, §12)
  yakin-gui/          later (M3)
  yakin-osd/          toast.rs (event to toast), draw.rs (SVG to shm), overlay.rs (layer surface)
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
- `regex` for `[[auto.rule]]`, `serde_json` for niri's event stream (the
  daemon only; actions are written by hand, the stream is read).
- `keyvalues-parser` for Steam's .vdf files (import only, never the daemon):
  keeps repeated keys like `group` in order, handles escapes, `//` and `#base`.
- `smithay-client-toolkit` (calloop, pure-Rust wayland-client) and `resvg`
  for yakin-osd only: layer-shell and shm without a toolkit or C libraries.
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
  enough; `TAG-="uaccess"` in a rule after `70-uaccess.rules` is). A second
  line `SUBSYSTEM=="hidraw", KERNELS=="0005:28DE:1303.*"` covers Bluetooth,
  whose hid device has no USB parent with `idVendor`/`idProduct`.
- Evdev: the lizard grab opens `/dev/input/event*` (`root:input 0660`), so
  the daemon's user is in `input`, which the uinput rule asks for anyway;
  `User=` gives the service the user's groups.
- Socket 0600 in the user's runtime dir; no auth beyond filesystem.
- NixOS module options: `services.yakin.enable`, `.user`, `.config`
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
  no arrow keys reach Cage while the daemon runs; `yakin-cli haptic`
  buzzes the named side; `yakin-cli dump` shows gyro changing when the
  controller tilts; killing the daemon returns lizard mode within the
  watchdog timeout (measured, documented).

M3: IPC + GUI

- `yakin-ipc` server/client, `yakin-cli status|reload|activate`,
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
  `resolver = "3"`, crates `yakin-<role>` under `crates/`, versions as
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

## 12. Steam Input import

`yakin-cli import <file.vdf> [--profile NAME]` prints a profile; `import --report <dir>...`
tallies every .vdf below. Format notes from Steam's `controller_base/` (structure only):

- KeyValues: quoted `"key" "value"` or `"key" { ... }`, keys repeat (`group`, `binding`), case
  varies (`Full_Press`/`full_press`, `button_B`), `//` comments, some files start with a BOM.
  The corpus has no `#base` lines.
- Root `controller_mappings`: `version`, `title`, many `group`, one or more `preset`, plus
  `actions`/`action_layers`/`localization` in game configs.
- `group`: `id`, `mode`, `bindings` (old flat `input "verb args"`) or `inputs` → input →
  `activators` → `Full_Press|Long_Press|Double_Press|Start_Press|Soft_Press|release` →
  `bindings` (repeated `binding`) + `settings`; group `settings` (sensitivity, output_trigger, ...).
- `preset`: `name`, `group_source_bindings` `"<group id>" "<source> active|inactive [modeshift]"`;
  old files have `switch_bindings` instead of a switches group. Presets after the first are
  action sets. Sources: button_diamond, dpad, switch, joystick, right_joystick, left_trackpad,
  right_trackpad, center_trackpad, left_trigger, right_trigger, gyro.
- Inputs: button_a/b/x/y, dpad_north/south/east/west, click, edge, touch, scroll_clockwise,
  left/right_bumper, button_escape (right, our menu), button_menu (left, our view),
  button_back_left/right(_upper), button_capture. `chord_triton.vdf` (the 2026 controller) uses
  exactly these plus both trackpads and sticks.
- Verbs: `key_press NAME`, `xinput_button NAME`, `mouse_button LEFT|RIGHT|MIDDLE|BACK|FORWARD`,
  `mouse_wheel`, `controller_action`, `game_action`, `mode_shift <source> <group id>`; anything
  after a comma is a label.
- Mapped: four_buttons, switches, dpad (on dpad; on a stick → `keys:` when all four are keys),
  joystick_move → `pad:left|right`, trigger (`output_trigger` ≠ 0 → `pad:lt|rt`, `click` →
  `lt_full`), absolute_mouse → cursor (`mouse.speed = sensitivity / 100`, rough), scrollwheel →
  scroll, mode_shift from a button onto a buttons group → chord. First binding only; activators
  Full/Long/Double/Start_Press and release → press/long/double/start/release, `long_press_time` →
  `long_ms`, `doubetap_max_duration` → `double_ms`, `hold_repeats` + `repeat_rate` →
  `turbo_ms = 1000 / repeat_rate` (press only); Soft_Press is skipped (an analog threshold).
  Back grips: `_upper` → grip_*1, lower → grip_*2; capture → quick (guesses until checked).
- `haptic_intensity` (an activator setting: 815 in the corpus, values 0/1/2 only) → `haptics.click`
  0 / 0.2 / 0.4 / 0.7 for off/low/medium/high, the strongest of the executed activators.
- Action sets: every preset converts. One set keeps the profile name; several become
  `<name>-<set>` (set lowercased, odd characters `-`). A preset named in `action_layers` is a layer
  (`[profile.<name>-<parent_set_name>.layer.<layer>]`, its buttons and axes only; its chords and
  knobs are a skip line). `controller_action CHANGE_PRESET <n>` (`n` counts presets from 1) →
  `profile:<that set>`; `hold_layer <n>` → `layer:hold:`, `add_layer <n>` → `layer:toggle:`
  (guessed from Steam's UI, not in the corpus; `remove_layer` is skipped).
- Gyro: `gyro_to_mouse` or `absolute_mouse` on source `gyro` → `axes.gyro = "cursor"`,
  `sensitivity` → `gyro.speed = sensitivity / 100`. `gyro_button` (1 in the corpus, value 12) is an
  undocumented enum: skipped, so imported gyro is always on.
- Corpus (2026-11-02, 79 files): 5 import several action sets, 4 have layers (WebBrowserChat in
  the basicui files), 7 bind the gyro; `gyro_to_mouse` 7/7 run, `controller_action` 2/190
  (the two CHANGE_PRESET in desktop_neptune; the rest are Steam UI actions), 242 groups skipped as
  inactive.
- Everything else is a `# skipped:` line in the output and a skip in the tally.
