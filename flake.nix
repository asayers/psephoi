{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs =
    { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
      # Must match the `wasm-bindgen` version in Cargo.lock exactly
      wasm-bindgen-cli = pkgs.wasm-bindgen-cli_0_2_129;
    in
    {
      formatter.x86_64-linux = nixpkgs.legacyPackages.${system}.nixfmt-tree;

      devShells.${system}.default = pkgs.mkShell {
        packages = with pkgs; [
          binaryen
          cargo
          clippy
          lld
          rust-analyzer
          rustc
          rustfmt
          wasm-bindgen-cli
        ];
        # env.PSEPHOI_CLIENT_DIR = self.packages.${system}.psephoiClient;
        env.PSEPHOI_CLIENT_DIR = "/tmp";
      };

      packages.${system} = {
        psephoiClient = pkgs.stdenv.mkDerivation {
          pname = "psephoi-client";
          version = "0.1.0";
          src = ./.;
          cargoDeps = pkgs.rustPlatform.importCargoLock { lockFile = ./Cargo.lock; };
          nativeBuildInputs = with pkgs; [
            binaryen
            cargo
            lld
            rustc
            rustPlatform.cargoSetupHook
            wasm-bindgen-cli
          ];
          buildPhase = ''
            runHook preBuild
            cargo build --release --offline -p client --target wasm32-unknown-unknown
            wasm-bindgen --target web --out-dir $out \
              target/wasm32-unknown-unknown/release/client.wasm
            wasm-opt -Os $out/client_bg.wasm -o $out/client_bg.wasm
            runHook postBuild
          '';
          dontInstall = true;
        };

        psephoiServer = pkgs.rustPlatform.buildRustPackage {
          pname = "psephoi-server";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          cargoBuildFlags = [
            "-p"
            "server"
          ];
          cargoTestFlags = [
            "-p"
            "server"
            "-p"
            "psephoi"
          ];
          meta.mainProgram = "server";
          env.PSEPHOI_CLIENT_DIR = self.packages.${system}.psephoiClient;
        };

        default = self.packages.${system}.psephoiServer;
      };

      # In the server flake:
      # services.psephoi.enable = true;
      # services.caddy.virtualHosts."vote.example.com".extraConfig = ''
      #   reverse_proxy unix/${config.services.psephoi.socketPath}
      # '';

      nixosModules.default = { config, lib, pkgs, ... }: {
        options.services.psephoi = {
          enable = lib.mkEnableOption "psephoi";
          socketPath = lib.mkOption {
            type = lib.types.str;
            default = "/run/psephoi/http.sock";
          };
          socketGroup = lib.mkOption {
            type = lib.types.str;
            default = "caddy";
          };
        };
        config = lib.mkIf config.services.psephoi.enable {
          systemd.sockets.psephoi = {
            wantedBy = [ "sockets.target" ];
            listenStreams = [ config.services.psephoi.socketPath ];
            socketConfig = {
              SocketGroup = config.services.psephoi.socketGroup;
              SocketMode = "0660";
            };
          };
          systemd.services.psephoi = {
            serviceConfig = {
              ExecStart = "${self.packages.${pkgs.stdenv.hostPlatform.system}.psephoiServer}/bin/server";
              StateDirectory = "psephoi";
              Restart = "always";

              # Resource limits
              MemoryMax = "64M";
              CPUQuota = "50%";
              TasksMax = 64;
              LimitNOFILE = 4096; # 1 WAL file per candidate per election stays open
              # StateDirectoryQuota=200M # systemd v258+, needs prjquota on ext4/xfs

              # Sandboxing
              DynamicUser = true;
              PrivateNetwork = true;
              RestrictAddressFamilies = "AF_UNIX";
              ProtectSystem = "strict";
              ProtectHome = true;
              PrivateTmp = true;
              PrivateDevices = true;
              PrivateUsers = true;
              ProtectKernelTunables = true;
              ProtectKernelModules = true;
              ProtectKernelLogs = true;
              ProtectControlGroups = true;
              ProtectClock = true;
              ProtectHostname = true;
              ProtectProc = "invisible";
              ProcSubset = "pid";
              RestrictNamespaces = true;
              RestrictRealtime = true;
              RestrictSUIDSGID = true;
              LockPersonality = true;
              MemoryDenyWriteExecute = true;
              NoNewPrivileges = true;
              CapabilityBoundingSet = "";
              SystemCallFilter = [ "@system-service" "~@privileged @resources" ];
              SystemCallArchitectures = "native";
              UMask = "0077";
            };
            environment.RUST_LOG = "info";
          };
        };
      };
    };
}
