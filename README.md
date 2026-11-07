# yakin

Steam Input-style control for the Steam Controller family, without Steam running.

A daemon reads the controller over hidraw, applies a profile and presents a virtual
gamepad, mouse and keyboard through uinput. Around it:

- `yakin-cli`: status, profiles, tuning and the quick menu from a shell
- `yakin-ui`: a configuration window (Dioxus desktop)
- `yakin-osd`: a Wayland overlay for the quick menu, the trackpad keyboard and toasts
- `yakin-tray`: a system tray icon
- `yakin-import`: Steam Input VDF configs to yakin profiles

Wayland only. Desktop actions go through a small backend trait; Niri is the first backend.

## Build and run

```sh
nix build            # the daemon and cli; .#yakin-ui, .#yakin-osd, .#yakin-tray for the rest
just check           # fmt, tests, clippy, smoke runs
just daemon          # run the daemon against examples/settop.toml
```

NixOS: import `nixosModules.default` and set `services.yakin.enable = true`
(`ui`, `osd` and `tray` add the rest).

[DESIGN.md](DESIGN.md) has the protocol notes, config format and architecture.

## License

Public domain, under the [Unlicense](UNLICENSE).

Modern AI has made software free to produce and worthless to own. Quality code
still takes weeks to write, but few care to write it or use it when slop is
instant. That makes software licensing pointless, so yakin doesn't bother: copy
it, change it, sell it, train on it, steal it, use it for nefarious purposes.
Whatever. No strings.

The controller artwork isn't software, and it isn't ours. It's generated at build time from
Valve's Steam Controller CAD, which Valve licenses CC BY-NC-SA 4.0, and the generated files
carry that licence. The repository contains only the code that generates them.
