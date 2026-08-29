{
  description = "AIVCS — agent version control system and OCI images";

  nixConfig = {
    extra-substituters = "https://cache.aivcs.io";
    extra-trusted-public-keys = "cache.aivcs.io:RZvvJ2Hx0EKj4V+J9dHKkfJ5L5YmP0C+WJ8K8J5J8pY=";
  };

  # A single release tarball keeps flake evaluation independent of Git
  # transport and authenticated registry helper endpoints.
  inputs.nixpkgs.url =
    "tarball+https://registry.aivcs.io/api/v1/crates/nixpkgs-source/26.11pre1055335-e5bdc4a41d4c/download";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;

      mkSystemPackages = system:
        let
          pkgs = import nixpkgs { inherit system; config.allowUnfree = true; };
          cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
          version = cargoToml.workspace.package.version;

          cargoSrc = pkgs.lib.cleanSourceWith {
            src = ./.;
            filter = path: type:
              let base = baseNameOf (toString path);
              in !(type == "directory" && builtins.elem base [
                ".aivcs" "node_modules" "result" "target"
              ]);
          };

          buildRustPackage = { pname }:
            pkgs.rustPlatform.buildRustPackage {
              inherit pname version;
              src = cargoSrc;
              cargoLock.lockFile = ./Cargo.lock;
              cargoBuildFlags = [ "-p" pname ];
              cargoTestFlags = [ "-p" pname ];
              strictDeps = true;

              # openssl-sys and aws-lc-sys both compile in this workspace.
              nativeBuildInputs = [ pkgs.pkg-config pkgs.cmake ];
              buildInputs = [ pkgs.openssl ]
                ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin [ pkgs.libiconv ];
              OPENSSL_NO_VENDOR = 1;
            };

          aivcs = buildRustPackage { pname = "aivcs-cli"; };
          aivcsd = buildRustPackage { pname = "aivcsd"; };
          aivcs-repo = buildRustPackage { pname = "aivcs-repo"; };
          aivcs-backup-agent = buildRustPackage { pname = "aivcs-backup-agent"; };
          aivcs-restore-agent = buildRustPackage { pname = "aivcs-restore-agent"; };
          aivcs-backup-validator = buildRustPackage { pname = "aivcs-backup-validator"; };

          mkImage = { name, package, binary, port ? null }:
            pkgs.dockerTools.buildLayeredImage {
              inherit name;
              tag = version;
              contents = [ package pkgs.cacert pkgs.dockerTools.fakeNss ];
              config = {
                Entrypoint = [ "${package}/bin/${binary}" ];
                User = "65532:65532";
                Env = [
                  "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
                  "RUST_LOG=info"
                ];
                ExposedPorts = if port == null then { } else { "${toString port}/tcp" = { }; };
                Labels = {
                  "org.opencontainers.image.source" = "aivcs://aivcs/aivcs";
                  "org.opencontainers.image.title" = name;
                  "org.opencontainers.image.version" = version;
                  "aivcs.io/managed-by" = "aivcs-propel";
                };
              };
            };

          aivcs-cli-image = mkImage {
            name = "aivcs-cli-image";
            package = aivcs;
            binary = "aivcs";
          };
          aivcsd-image = mkImage {
            name = "aivcsd-image";
            package = aivcsd;
            binary = "aivcsd";
            port = 8080;
          };
          aivcs-repo-image = mkImage {
            name = "aivcs-repo-image";
            package = aivcs-repo;
            binary = "aivcs-repo";
            port = 8080;
          };
          aivcs-backup-agent-image = mkImage {
            name = "aivcs-backup-agent";
            package = aivcs-backup-agent;
            binary = "aivcs_backup_agent";
          };
          aivcs-restore-agent-image = mkImage {
            name = "aivcs-restore-agent";
            package = aivcs-restore-agent;
            binary = "aivcs_restore_agent";
          };
          aivcs-backup-validator-image = mkImage {
            name = "aivcs-backup-validator";
            package = aivcs-backup-validator;
            binary = "aivcs_backup_validator";
          };
        in
        {
          inherit pkgs version
            aivcs aivcsd aivcs-repo
            aivcs-backup-agent aivcs-restore-agent aivcs-backup-validator
            aivcs-cli-image aivcsd-image aivcs-repo-image
            aivcs-backup-agent-image aivcs-restore-agent-image
            aivcs-backup-validator-image;
        };
    in
    {
      packages = forAllSystems (system:
        let built = mkSystemPackages system;
        in {
          default = built.aivcs;
          inherit (built)
            aivcs aivcsd aivcs-repo
            aivcs-backup-agent aivcs-restore-agent aivcs-backup-validator;
        } // built.pkgs.lib.optionalAttrs built.pkgs.stdenv.hostPlatform.isLinux {
          inherit (built)
            aivcs-cli-image aivcsd-image aivcs-repo-image
            aivcs-backup-agent-image aivcs-restore-agent-image
            aivcs-backup-validator-image;
        });

      # Package derivations run their own Cargo tests. Linux additionally checks
      # every OCI output referenced by the central matrix.
      checks = forAllSystems (system:
        let built = mkSystemPackages system;
        in {
          inherit (built)
            aivcs aivcsd aivcs-repo
            aivcs-backup-agent aivcs-restore-agent aivcs-backup-validator;
        } // built.pkgs.lib.optionalAttrs built.pkgs.stdenv.hostPlatform.isLinux {
          inherit (built)
            aivcs-cli-image aivcsd-image aivcs-repo-image
            aivcs-backup-agent-image aivcs-restore-agent-image
            aivcs-backup-validator-image;
        });

      devShells = forAllSystems (system:
        let pkgs = (mkSystemPackages system).pkgs;
        in {
          default = pkgs.mkShell {
            packages = with pkgs; [
              cargo rustc clippy rustfmt cargo-watch pkg-config cmake openssl just
            ];
            RUST_BACKTRACE = "1";
          };
        });
    };
}
