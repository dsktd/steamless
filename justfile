# Every cargo recipe runs in the flake's dev shell (nix develop), so nothing needs
# installing: `just` and `nix` are the whole toolchain. `just` alone lists the recipes.

# The daemon's control socket, shared by every client below. The dev shell inherits it.
export YAKIN_SOCKET := env("YAKIN_SOCKET", env("XDG_RUNTIME_DIR", "/tmp") / "yakin.sock")

# The runnable recipes use the `fast` profile (Cargo.toml)
cargo := "nix develop -c cargo"
run := cargo + " run --profile fast -p"

default:
    @just --list --unsorted

# --- build and check ---------------------------------------------------------

# Fetch Valve's STL and draw the controller and icons into ui/assets (gitignored; see NOTICE)
assets:
    nix build .#drawing --out-link ui/assets

# Fetch Valve's STL and convert the 3D view's shell into target/controller.glb (see NOTICE)
model:
    nix build .#model --out-link target/controller.glb

# Debug build of the daemon, cli and core (the window is built on demand by `just ui`)
build: assets
    {{cargo}} build

# Release build of everything but the window
release: assets
    {{cargo}} build --release

test: assets
    {{cargo}} test

fmt:
    {{cargo}} fmt

# What CI would ask: formatting, tests, and the window compiles
check: assets
    {{cargo}} fmt --check
    {{cargo}} test
    {{cargo}} test -p yakin-ui
    {{cargo}} clippy -p yakin-ui --all-targets -- -D warnings
    @just osd-smoke
    {{cargo}} test -p yakin-view
    {{cargo}} clippy -p yakin-view --all-targets -- -D warnings

# Rewrite the editor test's fixtures (Vocabulary, GetConfig for examples/) from the daemon's types;
# `just test` fails while they differ
ui-fixtures:
    YAKIN_FIXTURES=update {{cargo}} test -p yakin-daemon test_ui_fixtures_match

# Open the window under a headless Weston, no daemon: exits 0 once the page is up, 2 if it
# never comes up within the timeout
ui-smoke: assets
    #!/usr/bin/env bash
    set -euo pipefail
    nix develop -c cargo build -p yakin-ui
    runtime=$(mktemp -d) weston=
    trap 'kill $weston 2>/dev/null; rm -rf "$runtime"' EXIT
    export XDG_RUNTIME_DIR=$runtime WAYLAND_DISPLAY=yakin-smoke
    unset DISPLAY
    nix develop -c weston --backend=headless --socket=yakin-smoke --idle-time=0 \
        >"$runtime/weston.log" 2>&1 &
    weston=$!
    for _ in $(seq 50); do [ -S "$runtime/yakin-smoke" ] && break; sleep 0.1; done
    YAKIN_SMOKE=1 YAKIN_SOCKET=$runtime/none.sock \
        nix develop -c timeout 20 target/debug/yakin-ui || { echo "smoke: no page"; exit 2; }

# The 3D page under the same headless Weston, its target resized through several sizes on
# lavapipe; fails on any wgpu validation error in the log
ui-smoke-model: assets
    #!/usr/bin/env bash
    set -euo pipefail
    nix develop -c cargo build -p yakin-ui
    runtime=$(mktemp -d) weston=
    trap 'kill $weston 2>/dev/null; rm -rf "$runtime"' EXIT
    export XDG_RUNTIME_DIR=$runtime WAYLAND_DISPLAY=yakin-smoke
    unset DISPLAY
    nix develop -c weston --backend=headless --socket=yakin-smoke --idle-time=0 \
        >"$runtime/weston.log" 2>&1 &
    weston=$!
    for _ in $(seq 50); do [ -S "$runtime/yakin-smoke" ] && break; sleep 0.1; done
    YAKIN_SMOKE=1 YAKIN_SMOKE_RESIZE=1 YAKIN_SOCKET=$runtime/none.sock \
        nix develop -c timeout 120 target/debug/yakin-ui --page model 2>&1 | tee "$runtime/ui.log" \
        || { echo "smoke: the 3D page did not finish"; exit 2; }
    ! grep -E "Validation Error|Caught rendering error" "$runtime/ui.log"

# Show one toast, then the default quick menu, under a headless sway (Weston has no
# layer-shell), no daemon: each exits 0 at the compositor's first frame, 1 on error, 2 if no
# frame comes. The menu is also rendered to $YAKIN_MENU_PNG (default target/menu.png) for review
osd-smoke: assets
    #!/usr/bin/env bash
    set -euo pipefail
    nix develop -c cargo build -p yakin-osd
    runtime=$(mktemp -d) sway=
    trap 'kill $sway 2>/dev/null; rm -rf "$runtime"' EXIT
    chmod 700 "$runtime"
    printf 'output HEADLESS-1 scale 2\n' >"$runtime/config"
    export XDG_RUNTIME_DIR=$runtime WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=pixman
    unset DISPLAY WAYLAND_DISPLAY
    nix develop -c sway --config "$runtime/config" >"$runtime/sway.log" 2>&1 &
    sway=$!
    display=
    for _ in $(seq 100); do
        display=$(ls "$runtime" | grep -x 'wayland-[0-9]*' || true)
        [ -n "$display" ] && break
        sleep 0.1
    done
    WAYLAND_DISPLAY=$display nix develop -c target/debug/yakin-osd --once
    WAYLAND_DISPLAY=$display nix develop -c target/debug/yakin-osd --menu-demo --once
    png=${YAKIN_MENU_PNG:-target/menu.png}
    nix develop -c target/debug/yakin-osd --menu-svg "$runtime/menu.svg"
    nix develop -c nix shell nixpkgs#resvg -c resvg --zoom 2 "$runtime/menu.svg" "$png"
    echo "menu: $png"
    WAYLAND_DISPLAY=$display nix develop -c target/debug/yakin-osd --keyboard-demo --once
    png=${YAKIN_KEYBOARD_PNG:-target/keyboard.png}
    nix develop -c target/debug/yakin-osd --keyboard-svg "$runtime/keyboard.svg"
    nix develop -c nix shell nixpkgs#resvg -c resvg --zoom 2 "$runtime/keyboard.svg" "$png"
    echo "keyboard: $png"

clean:
    {{cargo}} clean

# --- nix ----------------------------------------------------------------------

# The package as the NixOS module installs it (result/bin/yakin-daemon, -cli)
nix-build:
    nix build .#default

# The window with its desktop entry and icons (result/bin/yakin-ui, result/share)
nix-build-ui:
    nix build .#yakin-ui

# The 3D view (result/bin/yakin-view)
nix-build-view:
    nix build .#yakin-view

# Evaluate the flake and build its checks: packages with tests, clippy, fmt
nix-check:
    nix flake check

# A shell with the toolchain and the window's and view's run-time libraries
shell:
    nix develop

# --- run ----------------------------------------------------------------------

# The daemon with debug logging; extra arguments go to it (default config: examples/settop.toml)
daemon *args="--config examples/settop.toml":
    RUST_LOG=debug {{run}} yakin-daemon -- --socket "$YAKIN_SOCKET" {{args}}

# The configuration window: live input, profiles, tuning, the binding editor and the 3D controller
ui *args: assets model
    YAKIN_MODEL=target/controller.glb RUST_LOG=warn {{run}} yakin-ui -- --socket "$YAKIN_SOCKET" {{args}}

# The 3D controller with live input from the running daemon; click a control for its binding
view *args: model
    YAKIN_MODEL=target/controller.glb RUST_LOG=warn {{run}} yakin-view -- --socket "$YAKIN_SOCKET" {{args}}

# The on-screen overlay against the running daemon; `just osd --show "hello"` needs no daemon
osd *args: assets
    {{cargo}} run --profile fast -p yakin-osd -- --socket "$YAKIN_SOCKET" {{args}}

# The tray icon against the running daemon
tray *args: assets
    {{cargo}} run --profile fast -p yakin-tray -- --socket "$YAKIN_SOCKET" {{args}}

# Any cli subcommand, e.g. `just cli dump`
cli *args:
    {{cargo}} run --profile fast -q -p yakin-cli -- --socket "$YAKIN_SOCKET" {{args}}

status:
    @just cli status

# Switch every controller to a profile, e.g. `just activate desktop`
activate profile:
    @just cli activate {{profile}}

# List the profiles, the active one starred
profiles:
    @just cli profiles

# Print the daemon's event stream (states, profile changes, controllers)
watch:
    @just cli watch

# Tune a profile in memory, e.g. `just tune desktop mouse_speed=0.8`
tune profile *pairs:
    @just cli tune {{profile}} {{pairs}}

# Re-read the config file and drop unsaved tuning
reload:
    @just cli reload

# Print the config as the daemon has it
config:
    @just cli config get

# Validate a config file without a daemon, e.g. `just config-check examples/settop.toml`
config-check file:
    @just cli config check {{file}}

# Raw HID reports from the controller, no daemon needed (needs access to /dev/hidraw*)
dump *args:
    @just cli dump {{args}}

# Decoded controller state from raw HID, no daemon needed
decode *args:
    @just cli decode {{args}}
