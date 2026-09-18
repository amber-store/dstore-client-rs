{
  description = "github.com/amber-store/dstore-client-rs";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    systems.url = "github:nix-systems/default";
  };

  outputs = { self, nixpkgs, systems, ... }:
    let
      lib = nixpkgs.lib;
      eachSystem = f:
        lib.genAttrs (import systems) (system: f system nixpkgs.legacyPackages.${system});

      cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);

      src = lib.fileset.toSource {
        root = ./.;
        # third_party: the patched iroh 1.2.0 ([patch.crates-io] in Cargo.toml).
        fileset = lib.fileset.unions [ ./Cargo.toml ./Cargo.lock ./src ./crates ./tests ./examples ./third_party ];
      };

      rustCommon = {
        inherit src;
        version = cargoToml.workspace.package.version;
        cargoLock = {
          lockFile = ./Cargo.lock;
          # github.com/amber-store/core-rs a85ffa1eb5ed363b9072ab224de179196cd0a046 (v0.3.0); bump with every rev.
          outputHashes."amber-store-core-0.3.0" = "sha256-ZnXXnztVqGXq5ILG29kkJIIRo9/TTW3XqULx1chbLXA=";
        };
      };
    in
    {
      formatter = eachSystem (system: pkgs: pkgs.writeShellApplication {
        name = "fmt";
        runtimeInputs = [ pkgs.cargo pkgs.rustfmt ];
        text = ''exec cargo fmt --all "$@"'';
      });

      packages = eachSystem (system: pkgs: rec {
        dstore = pkgs.rustPlatform.buildRustPackage (rustCommon // {
          pname = "dstore";
          cargoBuildFlags = [ "-p" "dstore-client-rs" "--bin" "dstore" ];
          # iroh tests bind UDP sockets, which the Darwin sandbox refuses (EPERM);
          # the socket-free suites run in checks.tests, everything runs in CI.
          doCheck = false;
          meta = {
            description = "dstore client library and CLI (Rust port of github.com/amber-store/dstore v0.1.9)";
            homepage = "https://github.com/amber-store/dstore-client-rs";
            license = lib.licenses.lgpl3Only;
            mainProgram = "dstore";
          };
        });
        default = dstore;
      });

      checks = eachSystem (system: pkgs: {
        dstore = self.packages.${system}.dstore;

        # cargo fmt would run cargo metadata and try to fetch the git dependency;
        # rustfmt on the files needs nothing.
        fmt = pkgs.runCommand "dstore-fmt-check" { nativeBuildInputs = [ pkgs.rustfmt ]; } ''
          cd ${src}
          find src crates tests examples -name '*.rs' -print0 | xargs -0 rustfmt --check --edition 2024
          touch $out
        '';

        clippy = pkgs.rustPlatform.buildRustPackage (rustCommon // {
          pname = "dstore-clippy";
          nativeBuildInputs = [ pkgs.clippy ];
          buildPhase = ''
            runHook preBuild
            cargo clippy --workspace --all-targets --offline -- -D warnings
            runHook postBuild
          '';
          doCheck = false;
          installPhase = "touch $out";
        });

        # Every test target that opens no socket: the unit tests of every crate; the root suites (golden
        # vectors, CLI snapshots, the CLI actions, the fake cluster over the in-memory transport); and the
        # crate test targets dstore-view all_slots and dstore-codec structs. iroh_loopback binds UDP
        # sockets, which the Darwin sandbox refuses; the CI rust job runs it.
        tests = pkgs.rustPlatform.buildRustPackage (rustCommon // {
          pname = "dstore-tests";
          doCheck = true;
          cargoTestFlags = [
            "--workspace"
            "--lib"
            "--test" "golden"
            "--test" "cli_snapshots"
            "--test" "cli_admin"
            "--test" "cli_client"
            "--test" "cli_wc"
            "--test" "fake_cluster"
            "--test" "fake_cluster_transfer"
            "--test" "fake_cluster_worktree"
            "--test" "all_slots" # dstore-view
            "--test" "structs" # dstore-codec
          ];
          TZ = "UTC";
          installPhase = "touch $out";
        });
      });

      devShells = eachSystem (system: pkgs: {
        default = pkgs.mkShell {
          hardeningDisable = [ "all" ];
          # go: tools/vectorgen, the CLI snapshot generator, the interop build of Go dstore
          packages = with pkgs; [ cargo rustc rustfmt clippy rust-analyzer go gopls ];
          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
          GOTOOLCHAIN = "local"; # go.mod says 1.26.5 = nixpkgs go; never download a toolchain
          CGO_ENABLED = "0"; # as the dstore Dockerfile; avoids the macOS Xcode cgo trap
        };
      });
    };
}
