# Mina Protocol Release Manager (Rust)

A Rust implementation of the Mina Protocol release manager script, providing comprehensive release management functionality for build artifacts.

## Overview

This tool handles the complete lifecycle of build artifacts including publishing, promotion, verification, and maintenance of packages across different channels and platforms.

### Main Capabilities

- **PUBLISH**: Publish build artifacts from cache to Debian repositories and Docker registries
- **PROMOTE**: Promote artifacts from one channel/registry to another (e.g., unstable -> stable)
- **VERIFY**: Verify that artifacts are correctly published in target channels/registries
- **FIX**: Repair Debian repository manifests when needed
- **PERSIST**: Archive artifacts to long-term storage backends
- **PULL**: Download artifacts from cache to local directory

### Supported Configurations

- **Artifacts**: mina-daemon, mina-archive, mina-rosetta, mina-logproc
- **Networks**: devnet, mainnet
- **Platforms**: Debian (bullseye, focal), Docker (GCR, Docker.io)
- **Channels**: unstable, alpha, beta, stable
- **Storage Backends**: Google Cloud Storage (gs), Hetzner, local filesystem

## Installation

### Prerequisites

Make sure you have Rust installed. If not, install it from [rustup.rs](https://rustup.rs/).

Additional tools required depending on operations:
- `gsutil` (for Google Cloud Storage operations)
- `docker` (for Docker operations and verification)
- `deb-s3` (for Debian repository fixes)
- SSH access and keys (for Hetzner operations)

### Building

```bash
cd buildkite/scripts/release/release-manager
cargo build --release
```

The binary will be available at `target/release/release-manager`.

### Environment Variables

The tool respects the following environment variables:

- `DEBIAN_CACHE_FOLDER`: Directory for caching Debian packages (default: `~/.release/debian/cache`)
- `HETZNER_USER`: Hetzner storage user (default: `u434410`)
- `HETZNER_HOST`: Hetzner storage host (default: `u434410-sub2.your-storagebox.de`)
- `HETZNER_KEY`: Path to Hetzner SSH key (default: `~/.ssh/id_rsa`)
- `RUST_LOG`: Log level (default: `info`)

## Usage

### Basic Command Structure

```bash
release-manager <COMMAND> [OPTIONS]
```

### Commands

#### Publish

Publish build artifacts from cache to repositories and registries.

```bash
release-manager publish \
  --buildkite-build-id 12345 \
  --source-version 1.0.0 \
  --target-version 1.0.1 \
  --channel stable \
  --artifacts mina-daemon,mina-archive \
  --networks devnet,mainnet \
  --codenames bullseye,focal \
  --verify
```

**Required options:**
- `--buildkite-build-id`: Buildkite build ID
- `--source-version`: Source version
- `--target-version`: Target version
- `--channel`: Target channel

**Optional options:**
- `--artifacts`: Comma-separated artifact list (default: all)
- `--networks`: Comma-separated network list (default: devnet,mainnet)
- `--codenames`: Comma-separated codename list (default: bullseye,focal)
- `--publish-to-docker-io`: Publish to docker.io instead of gcr.io
- `--only-dockers`: Publish only Docker images
- `--only-debians`: Publish only Debian packages
- `--verify`: Verify published packages
- `--dry-run`: Show what would be done without executing
- `--backend`: Storage backend (gs/hetzner/local, default: gs)
- `--debian-repo`: Debian repository (default: packages.o1test.net)
- `--debian-sign-key`: Signing key for Debian packages
- `--strip-network-from-archive`: Remove network suffix from archive packages

#### Promote

Promote artifacts from one channel/registry to another.

```bash
release-manager promote \
  --source-version 1.0.0 \
  --target-version 1.0.1 \
  --source-channel alpha \
  --target-channel beta \
  --artifacts mina-daemon,mina-archive \
  --verify
```

**Required options:**
- `--source-version`: Source version
- `--target-version`: Target version
- `--source-channel`: Source channel (required unless --only-dockers)
- `--target-channel`: Target channel (required unless --only-dockers)

#### Verify

Verify that artifacts are correctly published.

```bash
release-manager verify \
  --version 1.0.1 \
  --channel stable \
  --artifacts mina-daemon,mina-archive \
  --networks devnet,mainnet
```

**Required options:**
- `--version`: Version to verify

#### Fix

Repair Debian repository manifests.

```bash
release-manager fix \
  --codenames bullseye,focal \
  --channel stable
```

**Required options:**
- `--channel`: Channel to fix

#### Persist

Archive artifacts to long-term storage.

```bash
release-manager persist \
  --backend hetzner \
  --buildkite-build-id 12345 \
  --target /archive/2024 \
  --codename bullseye \
  --artifacts mina-daemon
```

**Required options:**
- `--buildkite-build-id`: Build ID to persist
- `--target`: Target storage location
- `--codename`: Codename to persist

#### Pull

Download artifacts from cache to local directory.

```bash
release-manager pull \
  --backend gs \
  --buildkite-build-id 12345 \
  --target ./downloads \
  --artifacts mina-daemon,mina-archive
```

**Required options:**
- `--buildkite-build-id`: Build ID to pull

## Configuration

### Storage Backends

#### Google Cloud Storage (gs)
- Requires `gsutil` to be installed and configured
- Uses `gs://buildkite_k8s/coda/shared` as root path

#### Hetzner
- Requires SSH access with key authentication
- Configure via environment variables:
  ```bash
  export HETZNER_USER=your-user
  export HETZNER_HOST=your-host
  export HETZNER_KEY=/path/to/key
  ```

#### Local
- Uses local filesystem at `/var/storagebox/`
- Useful for testing and development

### Logging

Set log level with `RUST_LOG` environment variable:
```bash
export RUST_LOG=debug  # trace, debug, info, warn, error
```

## Development

### Project Structure

```
src/
├── main.rs          # Main entry point
├── cli.rs           # Command-line argument definitions
├── errors.rs        # Error types and handling
├── storage.rs       # Storage backend abstraction
├── artifacts.rs     # Artifact handling functions
├── utils.rs         # Utility functions
└── commands/        # Command implementations
    ├── mod.rs
    ├── publish.rs
    ├── promote.rs
    ├── verify.rs
    ├── fix.rs
    ├── persist.rs
    └── pull.rs
```

### Running Tests

```bash
cargo test
```

### Building for Production

```bash
cargo build --release
strip target/release/release-manager  # Optional: reduce binary size
```

## Migration from Bash Script

This Rust implementation preserves all functionality from the original bash script while providing:

- Better error handling and validation
- Improved performance and reliability
- Type safety and compile-time checks
- Better maintainability and testability
- Structured logging
- Cross-platform compatibility

All command-line options and behavior remain compatible with the original script.

## License

This project follows the same license as the Mina Protocol project.