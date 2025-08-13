{
  description = "Mina Protocol Release Manager - Development Environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };
        
        # Define the Rust toolchain
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" ];
        };

        # System dependencies needed by the release manager
        systemDeps = with pkgs; [
          # Core build tools
          pkg-config
          openssl
          
          # Version control
          git
          
          # Docker tools (needed for docker promotion features)
          docker
          docker-compose
          
          # Network tools
          curl
          wget
          
          # File transfer tools
          rsync
          openssh
          
          # Archive tools
          gzip
          unzip
          
          # Package management tools
          dpkg
          
          # Cloud tools (for GCS integration)
          google-cloud-sdk
          
          # General utilities
          jq
          
          # SSL certificates
          cacert
        ];

      in
      {
        devShells.default = pkgs.mkShell {
          buildInputs = [
            rustToolchain
          ] ++ systemDeps;

          # Environment variables
          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
          
          # Set up SSL certificates for HTTPS requests
          SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
          
          # Docker-related environment setup
          DOCKER_HOST = "unix:///var/run/docker.sock";

          shellHook = ''
            echo "🦀 Mina Protocol Release Manager Development Environment"
            echo "   Rust version: $(rustc --version)"
            echo "   Cargo version: $(cargo --version)"
            echo ""
            echo "Available tools:"
            echo "  • cargo (Rust package manager)"
            echo "  • docker (Container management)"
            echo "  • git (Version control)"
            echo "  • rsync (File synchronization)"
            echo "  • gcloud (Google Cloud SDK)"
            echo "  • jq (JSON processor)"
            echo "  • dpkg (Debian package tools)"
            echo ""
            echo "Quick start:"
            echo "  cargo build    # Build the project"
            echo "  cargo test     # Run tests" 
            echo "  cargo run -- --help  # Show help"
            echo ""
            
            # Check if Docker daemon is running
            if ! docker info > /dev/null 2>&1; then
              echo "⚠️  Docker daemon is not running or not accessible"
              echo "   Make sure Docker is installed and running on your system"
              echo ""
            fi
          '';
        };

        # Package definition for the release manager binary
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "release-manager";
          version = "1.0.0";

          src = ./.;

          cargoLock = {
            lockFile = ./Cargo.lock;
          };

          nativeBuildInputs = [
            pkg-config
          ];

          buildInputs = [
            openssl
          ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
            pkgs.darwin.apple_sdk.frameworks.Security
            pkgs.darwin.apple_sdk.frameworks.SystemConfiguration
          ];

          # Skip tests during build (they may require Docker or network access)
          doCheck = false;

          meta = with pkgs.lib; {
            description = "Mina Protocol Release Manager - Comprehensive release management tool";
            homepage = "https://github.com/minaprotocol/mina-release-toolkit";
            license = licenses.asl20;
            maintainers = [ "Mina Protocol Team" ];
            platforms = platforms.unix;
          };
        };

        # App definition for easy running
        apps.default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/release-manager";
        };
      });
}