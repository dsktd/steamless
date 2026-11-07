{
  description = "Steam Controller without Steam";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  inputs.crane.url = "github:ipetkov/crane";

  outputs =
    { self, nixpkgs, crane }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
      osdFonts = pkgs: pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };

      # Dependencies build once per Cargo.lock in three sets: a plain one (daemon, cli, import, osd),
      # the window's (Blitz and Bevy) and the 3D view's (Bevy); a source change rebuilds only our crates
      build =
        pkgs:
        let
          inherit (pkgs) lib;
          craneLib = crane.mkLib pkgs;
          cad = import ./cad { inherit pkgs; };
          # Only what cargo reads: editing DESIGN.md or the justfile rebuilds nothing
          src = lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./rustfmt.toml
              ./crates
              ./examples
              ./vendor
            ];
          };
          common = {
            inherit src;
            pname = "yakin";
            version = "0.1.0";
            strictDeps = true;
            # wayland-sys's build script asks pkg-config for libwayland (the osd and the
            # tray link it; Bevy's winit pulls the same crate into the lock)
            nativeBuildInputs = [ pkgs.pkg-config ];
            buildInputs = [ pkgs.wayland ];
            # The drawing and icons are made from Valve's CAD, never committed (NOTICE)
            postPatch = ''
              mkdir -p ui
              ln -s ${cad.drawing} ui/assets
            '';
          };
          # Every package that carries the drawing or icon carries their licence too
          installLegal = ''
            install -Dm644 -t $out/share/licenses/yakin ${cad.legal}/*
          '';
          plainArtifacts = craneLib.buildDepsOnly common;
          # The 3D view (Bevy) and the window (Blitz), Wayland only; winit and wgpu dlopen these
          # at run time
          viewLibs = with pkgs; [
            vulkan-loader
            wayland
            libxkbcommon
          ];
          uiLibs = viewLibs ++ [ pkgs.fontconfig ];
          uiCommon = common // {
            pname = "yakin-ui";
            cargoExtraArgs = "-p yakin-ui";
            doCheck = false;
            nativeBuildInputs = with pkgs; [
              pkg-config
              makeWrapper
              # stylo (Blitz's CSS engine) generates its properties with Python at build time
              python3
            ];
            buildInputs = uiLibs ++ [ pkgs.openssl ];
          };
          uiArtifacts = craneLib.buildDepsOnly uiCommon;
          viewCommon = common // {
            pname = "yakin-view";
            cargoExtraArgs = "-p yakin-view";
            nativeBuildInputs = [
              pkgs.pkg-config
              pkgs.makeWrapper
            ];
            buildInputs = viewLibs;
          };
          viewArtifacts = craneLib.buildDepsOnly viewCommon;
        in
        rec {
          packages = {
            inherit (cad) drawing model;

            default = craneLib.buildPackage (
              common
              // {
                cargoArtifacts = plainArtifacts;
                cargoExtraArgs = "-p yakin-daemon -p yakin-cli";
                # cargo test over the default members, as before
                cargoTestExtraArgs = "--workspace --exclude yakin-ui";
                meta.mainProgram = "yakin-daemon";
              }
            );

            # The configuration window (Dioxus on Blitz), apart so the daemon's build needs no renderer
            yakin-ui = craneLib.buildPackage (
              uiCommon
              // {
                cargoArtifacts = uiArtifacts;
                desktopItem = pkgs.makeDesktopItem {
                  name = "dev.dsktd.yakin";
                  desktopName = "Yakin";
                  comment = "Steam Controller profiles and bindings";
                  exec = "yakin-ui";
                  icon = "dev.dsktd.yakin";
                  categories = [
                    "Settings"
                    "Utility"
                  ];
                  startupWMClass = "dev.dsktd.yakin";
                };
                # The desktop file's name is the window's Wayland app_id (APP_ID in main.rs); the
                # 3D page reads the shell from share/yakin, as yakin-view does
                postInstall = ''
                  wrapProgram $out/bin/yakin-ui --prefix LD_LIBRARY_PATH : ${lib.makeLibraryPath uiLibs}
                  install -Dm644 ${cad.model} $out/share/yakin/controller.glb
                  install -Dm644 $desktopItem/share/applications/dev.dsktd.yakin.desktop -t $out/share/applications
                  for size in 32 128 256 512; do
                    install -Dm644 ${cad.drawing}/icons/$size.png \
                      $out/share/icons/hicolor/''${size}x''${size}/apps/dev.dsktd.yakin.png
                  done
                  install -Dm644 ${cad.drawing}/icon.svg $out/share/icons/hicolor/scalable/apps/dev.dsktd.yakin.svg
                  install -Dm644 ${cad.drawing}/icon-symbolic.svg \
                    $out/share/icons/hicolor/symbolic/apps/dev.dsktd.yakin-symbolic.svg
                ''
                + installLegal;
                meta.mainProgram = "yakin-ui";
              }
            );

            # The on-screen display; pure Rust Wayland, so it only needs a font. FONTCONFIG_FILE points
            # resvg's fontdb at DejaVu Sans, so a NixOS box without fonts.packages still draws text
            yakin-osd = craneLib.buildPackage (
              common
              // {
                pname = "yakin-osd";
                cargoArtifacts = plainArtifacts;
                cargoExtraArgs = "-p yakin-osd";
                nativeBuildInputs = [ pkgs.makeWrapper ];
                postInstall = ''
                  wrapProgram $out/bin/yakin-osd --set FONTCONFIG_FILE ${osdFonts pkgs}
                ''
                + installLegal;
                meta.mainProgram = "yakin-osd";
              }
            );

            # The tray icon; pure Rust D-Bus, no native libraries
            yakin-tray = craneLib.buildPackage (
              common
              // {
                pname = "yakin-tray";
                cargoArtifacts = plainArtifacts;
                cargoExtraArgs = "-p yakin-tray";
                postInstall = installLegal;
                meta.mainProgram = "yakin-tray";
              }
            );

            # The 3D controller with live input; it reads the shell from share/yakin
            yakin-view = craneLib.buildPackage (
              viewCommon
              // {
                cargoArtifacts = viewArtifacts;
                postInstall = ''
                  wrapProgram $out/bin/yakin-view --prefix LD_LIBRARY_PATH : ${lib.makeLibraryPath viewLibs}
                  install -Dm644 ${cad.model} $out/share/yakin/controller.glb
                ''
                + installLegal;
                meta.mainProgram = "yakin-view";
              }
            );
          };

          checks = {
            yakin = packages.default;
            inherit (packages) yakin-ui yakin-osd yakin-tray yakin-view;
            clippy = craneLib.cargoClippy (
              common
              // {
                cargoArtifacts = plainArtifacts;
                cargoClippyExtraArgs = "--all-targets -- -D warnings";
              }
            );
            clippy-ui = craneLib.cargoClippy (
              uiCommon
              // {
                cargoArtifacts = uiArtifacts;
                cargoClippyExtraArgs = "-p yakin-ui --all-targets -- -D warnings";
              }
            );
            clippy-view = craneLib.cargoClippy (
              viewCommon
              // {
                cargoArtifacts = viewArtifacts;
                cargoClippyExtraArgs = "-p yakin-view --all-targets -- -D warnings";
              }
            );
            fmt = craneLib.cargoFmt { inherit src; pname = "yakin"; version = "0.1.0"; };
          };
        };
    in
    {
      packages = forAll (pkgs: (build pkgs).packages);
      checks = forAll (pkgs: (build pkgs).checks);

      # Stable rustfmt: the nightly-only rustfmt.toml options are skipped with a warning
      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          # just osd-smoke draws text with the packaged font
          FONTCONFIG_FILE = osdFonts pkgs;
          # mold links the window about a third faster than GNU ld; dev shell only, the
          # Nix packages link with the stdenv default
          RUSTFLAGS = "-C link-arg=-fuse-ld=mold";
          # just ui, just view: winit and wgpu dlopen the Wayland and Vulkan libraries
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (
            with pkgs;
            [
              vulkan-loader
              wayland
              libxkbcommon
              fontconfig
            ]
          );
          # On a distribution other than NixOS the host's GPU drivers cannot load into a Nix-built
          # process (Nix's dynamic loader never searches /usr/lib, and LD_LIBRARY_PATH would put the
          # host's libc over Nix's), so wgpu gets no backend and the window dies creating its
          # surface. Nix's own Mesa serves instead, as nixGL does: libEGL through glvnd and every
          # Mesa Vulkan driver (AMD, Intel, ...; a hardware GPU is taken before lavapipe)
          shellHook = ''
            if [ ! -e /run/opengl-driver ]; then
              export LD_LIBRARY_PATH="''${LD_LIBRARY_PATH:+$LD_LIBRARY_PATH:}${pkgs.libglvnd}/lib:${pkgs.mesa}/lib"
              export __EGL_VENDOR_LIBRARY_FILENAMES=${pkgs.mesa}/share/glvnd/egl_vendor.d/50_mesa.json
              export VK_DRIVER_FILES=$(echo ${pkgs.mesa}/share/vulkan/icd.d/*.json | tr ' ' ':')
            fi
          '';
          packages = with pkgs; [
            cargo
            rustc
            rustfmt
            clippy
            rust-analyzer
            pkg-config
            mold
            # yakin-ui (Dioxus on Blitz): stylo's build script
            python3
            openssl
            # pkg-config: yakin-ui (fontconfig), yakin-view (Wayland, xkbcommon)
            fontconfig
            wayland
            libxkbcommon
            # just ui-smoke
            weston
            # just osd-smoke (Weston has no layer-shell)
            sway
          ];
        };
      });

      nixosModules.default =
        {
          config,
          lib,
          pkgs,
          ...
        }:
        let
          cfg = config.services.yakin;
          toml = pkgs.formats.toml { };
        in
        {
          options.services.yakin = {
            enable = lib.mkEnableOption "yakin, the Steam Controller daemon";
            user = lib.mkOption {
              type = lib.types.str;
              description = "User the daemon runs as; owns the controller's hidraw nodes.";
            };
            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
            };
            ui = lib.mkEnableOption "the configuration window (yakin-ui) with its desktop entry";
            uiPackage = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.yakin-ui;
            };
            osd = lib.mkEnableOption "the on-screen display (yakin-osd), a user service in every graphical session";
            osdPackage = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.yakin-osd;
            };
            tray = lib.mkEnableOption "the tray icon (yakin-tray), a user service in every graphical session";
            trayPackage = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.yakin-tray;
            };
            config = lib.mkOption {
              type = lib.types.either lib.types.path toml.type;
              default = { };
              description = "Config file, or an attrset rendered to TOML; empty runs the Xbox layout.";
            };
            configDir = lib.mkOption {
              type = lib.types.str;
              default = "/var/lib/yakin/config.d";
              description = "Drop-in directory: every *.toml in it applies over `config` by file name; the editor writes 90-editor.toml.";
            };
          };

          config = lib.mkIf cfg.enable {
            boot.kernelModules = [ "uinput" ];
            users.users.${cfg.user}.extraGroups = [ "input" ];

            # After 73-seat-late (which applies the uaccess ACL) so the strip sticks; Chromium and SDL then cannot open the node
            services.udev.packages = [
              (pkgs.writeTextDir "lib/udev/rules.d/99-yakin.rules" ''
                SUBSYSTEM=="hidraw", ATTRS{idVendor}=="28de", ATTRS{idProduct}=="130[345]", MODE="0600", OWNER="${cfg.user}", TAG-="uaccess"
                SUBSYSTEM=="hidraw", KERNELS=="0005:28DE:1303.*", MODE="0600", OWNER="${cfg.user}", TAG-="uaccess"
                KERNEL=="uinput", SUBSYSTEM=="misc", MODE="0660", GROUP="input", OPTIONS+="static_node=uinput"
              '')
            ];

            environment.etc."yakin/config.toml".source =
              if builtins.isAttrs cfg.config then toml.generate "config.toml" cfg.config else cfg.config;

            # Clients reach the daemon with YAKIN_SOCKET=/run/yakin/yakin.sock;
            # the cli is on every PATH for `yakin-cli status|activate|reload`
            environment.variables.YAKIN_SOCKET = "/run/yakin/yakin.sock";
            environment.variables.YAKIN_CONFIG_DIR = cfg.configDir;
            systemd.tmpfiles.rules = [ "d ${cfg.configDir} 0750 ${cfg.user} - - -" ];
            environment.systemPackages = [
              cfg.package
            ]
            ++ lib.optional cfg.ui cfg.uiPackage
            ++ lib.optional cfg.osd cfg.osdPackage
            ++ lib.optional cfg.tray cfg.trayPackage;

            # niri.service (niri-session) binds graphical-session.target, so this starts with it
            systemd.user.services.yakin-osd = lib.mkIf cfg.osd {
              description = "Yakin on-screen display";
              wantedBy = [ "graphical-session.target" ];
              partOf = [ "graphical-session.target" ];
              after = [ "graphical-session.target" ];
              environment.YAKIN_SOCKET = "/run/yakin/yakin.sock";
              serviceConfig = {
                ExecStart = lib.getExe cfg.osdPackage;
                Restart = "on-failure";
                RestartSec = 2;
              };
            };

            systemd.user.services.yakin-tray = lib.mkIf cfg.tray {
              description = "Yakin tray icon";
              wantedBy = [ "graphical-session.target" ];
              partOf = [ "graphical-session.target" ];
              after = [ "graphical-session.target" ];
              environment.YAKIN_SOCKET = "/run/yakin/yakin.sock";
              # yakin-ui for "Open configuration"
              path = lib.optional cfg.ui cfg.uiPackage;
              serviceConfig = {
                ExecStart = lib.getExe cfg.trayPackage;
                Restart = "on-failure";
                RestartSec = 2;
              };
            };

            systemd.services.yakin = {
              description = "Steam Controller daemon";
              wantedBy = [ "multi-user.target" ];
              after = [ "systemd-udev-settle.service" ];
              environment.XDG_RUNTIME_DIR = "/run/yakin";
              # bluetoothctl drops a controller's link on power-down
              path = [ pkgs.bluez ];
              # a new baseline is a new daemon; drop-in changes go through `yakin-cli reload`
              restartTriggers = [ config.environment.etc."yakin/config.toml".source ];
              serviceConfig = {
                User = cfg.user;
                RuntimeDirectory = "yakin";
                ExecStart = "${lib.getExe' cfg.package "yakin-daemon"} --config /etc/yakin/config.toml --config-dir ${cfg.configDir}";
                Restart = "always";
                RestartSec = 2;
                # headroom over the default 1024 for client connections
                LimitNOFILE = 4096;
              };
            };
          };
        };
    };
}
