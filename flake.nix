{
  description = "Steam Controller without Steam";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAll (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "steamless";
          version = "0.1.0";
          src = self;
          cargoLock.lockFile = ./Cargo.lock;
          cargoBuildFlags = [
            "-p"
            "steamless-daemon"
            "-p"
            "steamless-cli"
          ];
          meta.mainProgram = "steamless-daemon";
        };
      });

      # Stable rustfmt: the nightly-only rustfmt.toml options are skipped with a warning
      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            rustfmt
            clippy
            rust-analyzer
            pkg-config
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
          cfg = config.services.steamless;
          toml = pkgs.formats.toml { };
        in
        {
          options.services.steamless = {
            enable = lib.mkEnableOption "steamless, the Steam Controller daemon";
            user = lib.mkOption {
              type = lib.types.str;
              description = "User the daemon runs as; owns the controller's hidraw nodes.";
            };
            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
            };
            config = lib.mkOption {
              type = toml.type;
              default = { };
              description = "Rendered to /etc/steamless/config.toml; empty runs the Xbox layout.";
            };
          };

          config = lib.mkIf cfg.enable {
            # uinput module, /dev/uinput 0660 group uinput
            hardware.uinput.enable = true;
            users.users.${cfg.user}.extraGroups = [ "uinput" ];

            # After 70-uaccess and before 73-seat-late applies it, so Chromium cannot open the node
            services.udev.packages = [
              (pkgs.writeTextDir "lib/udev/rules.d/71-steamless.rules" ''
                SUBSYSTEM=="hidraw", ATTRS{idVendor}=="28de", ATTRS{idProduct}=="130[345]", MODE="0600", OWNER="${cfg.user}", TAG-="uaccess"
              '')
            ];

            environment.etc."steamless/config.toml".source = toml.generate "config.toml" cfg.config;

            systemd.services.steamless = {
              description = "Steam Controller daemon";
              wantedBy = [ "multi-user.target" ];
              after = [ "systemd-udev-settle.service" ];
              serviceConfig = {
                User = cfg.user;
                ExecStart = "${lib.getExe cfg.package} --config /etc/steamless/config.toml";
                Restart = "always";
                RestartSec = 2;
              };
            };
          };
        };
    };
}
