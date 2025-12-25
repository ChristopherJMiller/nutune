{
  description = "nutune - Sync music from Subsonic to portable devices";

  inputs = {
    nixpkgs.url = "nixpkgs";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      flake-utils,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };

        # Automatically read rust-toolchain.toml
        rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
      in
      {
        devShells.default = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [
            rustToolchain
            pkg-config
          ];

          buildInputs = with pkgs; [
            openssl
            dbus.dev
          ];
        };

        packages = rec {
          nutune = pkgs.rustPlatform.buildRustPackage {
            pname = "nutune";
            version = "0.1.0";
            src = ./.;

            cargoLock = {
              lockFile = ./Cargo.lock;
            };

            nativeBuildInputs = with pkgs; [
              rustToolchain
              pkg-config
              installShellFiles
            ];

            buildInputs = with pkgs; [
              openssl
              dbus
            ];

            env = {
              OPENSSL_NO_VENDOR = "1";
            };

            postInstall = ''
              installShellCompletion --cmd nutune \
                --bash <($out/bin/nutune completion bash) \
                --fish <($out/bin/nutune completion fish) \
                --zsh <($out/bin/nutune completion zsh)
            '';

            meta = with pkgs.lib; {
              description = "Sync music from Subsonic to portable devices";
              homepage = "https://github.com/ChristopherJMiller/nutune";
              license = licenses.gpl3;
              maintainers = [ ];
              platforms = platforms.unix;

              longDescription = ''
                A CLI tool to sync your Subsonic music library to portable devices.
                Features device detection, interactive browsing, parallel downloads,
                and playlist support with M3U generation.
              '';
            };
          };

          default = nutune;
        };
      }
    );
}
