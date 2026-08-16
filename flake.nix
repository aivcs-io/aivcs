{
  description = "AIVCS - AI Version Control System";

  nixConfig = {
    extra-substituters = [ "https://nix-cache.lornu.ai/lornu" ];
    extra-trusted-public-keys = [
      "lornu-1:BdTDSJYXTq/AC/Z2S2MtluX2mjuN5Ew2IZCWQoeTyww="
    ];
  };

  # NOTE: All inputs from aivcs registry only (pure aivcs ecosystem).
  # Pinned via flake.lock (committed to aivcs).
  inputs = {
    nixpkgs.url = "tarball+https://registry.aivcs.io/nixpkgs/nixos-unstable";
    flake-utils.url = "tarball+https://registry.aivcs.io/flake-utils";

    rust-overlay = {
      url = "tarball+https://registry.aivcs.io/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };

    nixos-wsl = {
      url = "tarball+https://registry.aivcs.io/nixos-wsl";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = inputs@{ self, nixpkgs, flake-utils, rust-overlay, nixos-wsl, ... }:
    let
      mkSystemPackages = system:
        let
          overlays = [ (import rust-overlay) ];
          pkgs = import nixpkgs { inherit system overlays; config.allowUnfree = true; };

          rustToolchain = pkgs.rust-bin.stable.latest.default.override {
            extensions = [ "rust-src" "rust-analyzer" "rustfmt" "clippy" ];
          };

          # Minimal source filter (include only build-relevant files)
          cargoSrc = pkgs.lib.cleanSourceWith {
            src = ./.;
            filter = path: type:
              let p = toString path; in
              pkgs.lib.any (suffix: pkgs.lib.hasSuffix suffix p) [
                ".rs" ".toml" ".lock" ".surql" ".pem"
              ] ||
              (type == "directory" && pkgs.lib.any (suffix: pkgs.lib.hasSuffix suffix p) [
                "/src" "/crates" "/keys" "/.cargo"
              ]);
          };

          # Nix build for a Rust binary: calls cargo directly
          buildRustPackage = { pname, cargoExtraArgs ? "", RUSTFLAGS ? "", preBuild ? "" }:
            pkgs.stdenv.mkDerivation {
              inherit pname preBuild;
              name = "${pname}-rust";
              version = "0.4.0";
              src = cargoSrc;

              nativeBuildInputs = with pkgs; [
                rustToolchain
                pkg-config
              ];

              buildInputs = with pkgs; [
                openssl
              ];

              buildPhase = ''
                export CARGO_HOME=$PWD/.cargo
                export RUSTFLAGS="${RUSTFLAGS} -C target-feature=+crt-static"
                export SOURCE_DATE_EPOCH=0
                ${preBuild}
                cargo build -p ${pname} --release ${cargoExtraArgs}
              '';

              installPhase = ''
                mkdir -p $out/bin
                find target/release -maxdepth 1 -type f -executable ! -name "*.d" -exec cp {} $out/bin/ \;
              '';

              dontFixup = true;
            };

          aivcs = buildRustPackage {
            pname = "aivcs-cli";
            cargoExtraArgs = "";
          };

          aivcsd = buildRustPackage {
            pname = "aivcsd";
          };

          aivcs-repo = buildRustPackage {
            pname = "aivcs-repo";
          };

          aivcs-backup-agent = buildRustPackage {
            pname = "aivcs-backup-agent";
          };

          aivcs-restore-agent = buildRustPackage {
            pname = "aivcs-restore-agent";
          };

          aivcs-backup-validator = buildRustPackage {
            pname = "aivcs-backup-validator";
          };

          pkgVersion = "0.4.0";

          aivcs-cli-image = pkgs.dockerTools.buildLayeredImage {
            name = "aivcs";
            tag = pkgVersion;
            contents = [ aivcs pkgs.cacert pkgs.coreutils ];
            config = {
              Cmd = [ "${aivcs}/bin/aivcs-cli" ];
              User = "65532:65532";
              Env = [
                "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
                "RUST_LOG=info"
              ];
              Labels = {
                "org.opencontainers.image.source" = "aivcs://aivcs/aivcs";
                "org.opencontainers.image.title" = "aivcs";
                "org.opencontainers.image.version" = pkgVersion;
                "lornu.ai/managed-by" = "dockworker";
                "lornu.ai/runtime" = "rust";
                "lornu.ai/component" = "aivcs-cli";
              };
            };
          };

          aivcsd-image = pkgs.dockerTools.buildLayeredImage {
            name = "aivcsd";
            tag = pkgVersion;
            contents = [ aivcsd pkgs.cacert ];
            config = {
              Cmd = [ "${aivcsd}/bin/aivcsd" ];
              User = "65532:65532";
              ExposedPorts = { "8080/tcp" = { }; };
              Env = [
                "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
                "RUST_LOG=info"
              ];
              Labels = {
                "org.opencontainers.image.source" = "aivcs://aivcs/aivcs";
                "org.opencontainers.image.title" = "aivcsd";
                "org.opencontainers.image.version" = pkgVersion;
                "lornu.ai/managed-by" = "dockworker";
                "lornu.ai/runtime" = "rust";
                "lornu.ai/component" = "aivcsd";
              };
            };
          };

          aivcs-repo-image = pkgs.dockerTools.buildLayeredImage {
            name = "aivcs-repo";
            tag = pkgVersion;
            contents = [ aivcs-repo pkgs.cacert ];
            config = {
              Cmd = [ "${aivcs-repo}/bin/aivcs-repo" ];
              User = "65532:65532";
              ExposedPorts = { "8080/tcp" = { }; };
              Env = [
                "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
                "RUST_LOG=info"
              ];
              Labels = {
                "org.opencontainers.image.source" = "aivcs://aivcs/aivcs";
                "org.opencontainers.image.title" = "aivcs-repo";
                "org.opencontainers.image.version" = pkgVersion;
                "lornu.ai/managed-by" = "dockworker";
                "lornu.ai/runtime" = "rust";
                "lornu.ai/component" = "aivcs-repo";
              };
            };
          };

          aivcs-backup-agent-image = pkgs.dockerTools.buildLayeredImage {
            name = "aivcs-backup-agent";
            tag = pkgVersion;
            contents = [ aivcs-backup-agent pkgs.cacert ];
            config = {
              Cmd = [ "${aivcs-backup-agent}/bin/aivcs_backup_agent" ];
              User = "65532:65532";
              Env = [
                "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
                "RUST_LOG=info"
              ];
              Labels = {
                "org.opencontainers.image.source" = "aivcs://aivcs/aivcs";
                "org.opencontainers.image.title" = "aivcs-backup-agent";
                "org.opencontainers.image.version" = pkgVersion;
                "lornu.ai/managed-by" = "dockworker";
                "lornu.ai/runtime" = "rust";
                "lornu.ai/component" = "aivcs-backup-agent";
              };
            };
          };

          aivcs-restore-agent-image = pkgs.dockerTools.buildLayeredImage {
            name = "aivcs-restore-agent";
            tag = pkgVersion;
            contents = [ aivcs-restore-agent pkgs.cacert ];
            config = {
              Cmd = [ "${aivcs-restore-agent}/bin/aivcs_restore_agent" ];
              User = "65532:65532";
              Env = [
                "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
                "RUST_LOG=info"
              ];
              Labels = {
                "org.opencontainers.image.source" = "aivcs://aivcs/aivcs";
                "org.opencontainers.image.title" = "aivcs-restore-agent";
                "org.opencontainers.image.version" = pkgVersion;
                "lornu.ai/managed-by" = "dockworker";
                "lornu.ai/runtime" = "rust";
                "lornu.ai/component" = "aivcs-restore-agent";
              };
            };
          };

          aivcs-backup-validator-image = pkgs.dockerTools.buildLayeredImage {
            name = "aivcs-backup-validator";
            tag = pkgVersion;
            contents = [ aivcs-backup-validator pkgs.cacert ];
            config = {
              Cmd = [ "${aivcs-backup-validator}/bin/aivcs_backup_validator" ];
              User = "65532:65532";
              Env = [
                "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
                "RUST_LOG=info"
              ];
              Labels = {
                "org.opencontainers.image.source" = "aivcs://aivcs/aivcs";
                "org.opencontainers.image.title" = "aivcs-backup-validator";
                "org.opencontainers.image.version" = pkgVersion;
                "lornu.ai/managed-by" = "dockworker";
                "lornu.ai/runtime" = "rust";
                "lornu.ai/component" = "aivcs-backup-validator";
              };
            };
          };
        in
        {
          inherit pkgs aivcs aivcsd aivcs-repo aivcs-backup-agent aivcs-restore-agent aivcs-backup-validator
            aivcs-cli-image aivcsd-image aivcs-repo-image aivcs-backup-agent-image aivcs-restore-agent-image aivcs-backup-validator-image;
        };

      linuxPackages = mkSystemPackages "x86_64-linux";
    in
    flake-utils.lib.eachDefaultSystem (system:
      let
        inherit (mkSystemPackages system) pkgs aivcs aivcsd aivcs-repo aivcs-backup-agent aivcs-restore-agent aivcs-backup-validator
          aivcs-cli-image aivcsd-image aivcs-repo-image aivcs-backup-agent-image aivcs-restore-agent-image aivcs-backup-validator-image;
        wslChecks =
          if system == "x86_64-linux" then {
            aivcs-wsl = self.nixosConfigurations.aivcs-wsl.config.system.build.toplevel;
            aivcs-wsl-e2e = import ./nix/tests/aivcs-wsl-e2e.nix {
              inherit pkgs;
              aivcsPackage = aivcs;
              aivcsdPackage = aivcsd;
            };
          } else { };
      in
      {
        checks = {
          clippy = pkgs.stdenv.mkDerivation {
            name = "clippy-check";
            src = ./.;
            nativeBuildInputs = with pkgs; [
              (pkgs.rust-bin.stable.latest.default.override {
                extensions = [ "clippy" ];
              })
            ];
            buildPhase = ''
              cargo clippy --all-targets -- -D warnings
            '';
            installPhase = "mkdir -p $out && echo 'passed' > $out/result";
          };

          fmt = pkgs.stdenv.mkDerivation {
            name = "fmt-check";
            src = ./.;
            nativeBuildInputs = with pkgs; [
              (pkgs.rust-bin.stable.latest.default.override {
                extensions = [ "rustfmt" ];
              })
            ];
            buildPhase = ''
              cargo fmt -- --check
            '';
            installPhase = "mkdir -p $out && echo 'passed' > $out/result";
          };

          tests = pkgs.stdenv.mkDerivation {
            name = "cargo-tests";
            src = ./.;
            nativeBuildInputs = with pkgs; [
              pkgs.rust-bin.stable.latest.default
              pkg-config
            ];
            buildInputs = with pkgs; [ openssl ];
            buildPhase = ''
              cargo test --workspace --release
            '';
            installPhase = "mkdir -p $out && echo 'passed' > $out/result";
          };
        } // wslChecks;

        packages = {
          default = aivcs;
          inherit aivcs aivcsd aivcs-repo aivcs-backup-agent aivcs-restore-agent aivcs-backup-validator;
        } // pkgs.lib.optionalAttrs (system == "x86_64-linux") {
          inherit aivcs-cli-image aivcsd-image aivcs-repo-image aivcs-backup-agent-image aivcs-restore-agent-image aivcs-backup-validator-image;
        };

        devShells.default = pkgs.mkShell {
          name = "aivcs-dev";
          nativeBuildInputs = with pkgs; [
            (pkgs.rust-bin.stable.latest.default.override {
              extensions = [ "rust-src" "rust-analyzer" "rustfmt" "clippy" ];
            })
            cargo-watch
            pkg-config
            surrealdb
            just
          ];

          buildInputs = with pkgs; [ openssl ];

          RUST_BACKTRACE = "1";

          shellHook = ''
            echo "AIVCS Development Environment"
            echo ""
            echo "Commands:"
            echo "  cargo test --workspace        # Run all tests"
            echo "  cargo run -p aivcs-cli        # Run CLI"
            echo "  surreal start memory           # Start SurrealDB (in-memory)"
            echo "  just --list                    # Show available justfile recipes"
            echo ""
          '';
        };
      }
    )
    // {
      nixosConfigurations.aivcs-wsl = nixpkgs.lib.nixosSystem {
        system = "x86_64-linux";
        specialArgs = {
          inherit inputs;
          aivcsPackage = linuxPackages.aivcs;
          aivcsdPackage = linuxPackages.aivcsd;
        };
        modules = [
          nixos-wsl.nixosModules.default
          ./nix/nixos/aivcs-wsl.nix
        ];
      };
    };
}
