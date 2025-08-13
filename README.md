# Mina Protocol Release Toolkit

A comprehensive monorepo containing all subprojects, tools, and scripts for managing the Mina Protocol release process. This toolkit provides end-to-end automation for building, packaging, publishing, and maintaining Mina Protocol releases across different channels and platforms.

## Overview

The mina-release-toolkit is designed to handle the complete lifecycle of Mina Protocol releases, from building packages to deploying them across various distribution channels. It supports multiple artifacts (mina-daemon, mina-archive, mina-rosetta, mina-logproc), networks (devnet, mainnet), platforms (Debian, Docker), and storage backends (Google Cloud Storage, Hetzner, local filesystem).

## Components

### 1. **deb-builder** (OCaml/Dune Submodule)
A robust OCaml utility for building Debian packages with comprehensive configuration options and GPG signing capabilities.

**Key Features:**
- **Template-based package generation** with extensive metadata support
- **GPG signature verification** and content verification for package integrity  
- **Flexible configuration system** supporting defaults files and CLI overrides
- **Multi-platform support** for different Debian codenames (bullseye, focal)
- **CI/CD integration** with Docker-based building and testing

**Architecture:**
- `src/lib/builder.ml` - Core package building logic with ~70 configuration parameters
- `src/lib/templates.ml` - Jingoo-based template system for Debian control files
- `src/lib/signer.ml` - GPG signing functionality for package authentication
- `src/lib/content_verifier.ml` - Package content validation
- `ci/scripts/` - Build automation with Docker support

**Usage:**
```bash
cd deb-builder
make dependencies && make build
./target/deb_builder.exe --build-dir ./build --package-name mina-daemon --version 1.0.0
```

### 2. **deb-s3** (Ruby Gem Submodule)  
A specialized Ruby utility for managing APT repositories on Amazon S3, forked from the original krobertson/deb-s3 project.

**Key Features:**
- **Direct S3 APT repository management** without local file tree maintenance
- **Package manifest management** with automatic Packages/Release file updates
- **Multi-architecture support** (amd64, i386, all)
- **GPG signing integration** for repository authentication
- **Verification and repair tools** for repository integrity

**Core Operations:**
- **Upload**: Add .deb packages to S3-hosted APT repositories
- **Delete**: Remove specific package versions from repositories  
- **Verify**: Check repository integrity and fix manifest issues
- **Component/Codename management** for organized package distribution

**Usage:**
```bash
deb-s3 upload --bucket my-bucket --codename stable my-package.deb
deb-s3 verify --bucket my-bucket --fix-manifests
```

### 3. **dhall-buildkite** (Dhall Configuration Submodule)
A foundational Dhall library providing type-safe, composable configurations for Buildkite CI/CD pipelines.

**Key Features:**
- **Type-safe pipeline configurations** with compile-time validation
- **Modular command system** supporting Docker, plugins, and custom commands
- **Monorepo diff filtering** for selective CI execution
- **Reusable pipeline components** and templates
- **S3-hosted package distribution** with versioned releases

**Architecture:**
- `src/Pipeline/Type.dhall` - Core pipeline type definitions and builders
- `src/Command/` - Buildkite command abstractions (size, retry, dependencies)
- `src/Lib/` - Utility functions for file selection and command composition
- `examples/` - Working examples from hello-world to complex monorepo filtering

**Usage:**
```bash
cd dhall-buildkite
make all_checks  # Validate syntax, lint, and format
make build_package && make release
```

### 4. **release-manager** (Rust Application)
A high-performance Rust implementation providing comprehensive release management functionality for build artifacts across multiple platforms and storage backends.

**Key Features:**
- **Multi-platform publishing** to Debian repositories and Docker registries
- **Channel promotion** (unstable → alpha → beta → stable)
- **Artifact verification** across all target platforms
- **Repository repair tools** for Debian manifest fixes
- **Long-term archival** to multiple storage backends
- **Cache management** with pull/persist operations

**Supported Configurations:**
- **Artifacts**: mina-daemon, mina-archive, mina-rosetta, mina-logproc
- **Networks**: devnet, mainnet  
- **Platforms**: Debian (bullseye, focal), Docker (GCR, docker.io)
- **Channels**: unstable, alpha, beta, stable
- **Storage**: Google Cloud Storage, Hetzner, local filesystem

**Command Structure:**
```bash
# Publish artifacts from cache to repositories
release-manager publish --buildkite-build-id 12345 --source-version 1.0.0 --target-version 1.0.1 --channel stable

# Promote between channels  
release-manager promote --source-version 1.0.0 --target-version 1.0.1 --source-channel alpha --target-channel beta

# Verify published artifacts
release-manager verify --version 1.0.1 --channel stable --artifacts mina-daemon,mina-archive

# Repair repository manifests
release-manager fix --codenames bullseye,focal --channel stable

# Archive to long-term storage
release-manager persist --backend hetzner --buildkite-build-id 12345 --target /archive/2024
```

### 5. **buildkite-cache-manager.sh** (Bash Script)
A lightweight cache management utility for Buildkite CI environments, providing efficient file transfer to/from shared storage.

**Key Features:**
- **Buildkite integration** with automatic build ID detection
- **Bidirectional operations** (read from cache, write to cache)
- **Wildcard support** for batch file operations
- **Override protection** with configurable file conflict handling
- **Flexible root path management** for organized cache structure

**Operations:**
- **Read**: Copy artifacts from cache to local workspace
- **Write**: Upload build artifacts to shared cache
- **Directory management** with automatic creation
- **Error handling** with detailed failure reporting

**Usage:**
```bash
# Write artifacts to cache
./buildkite-cache-manager.sh write mina-daemon*.deb debians/

# Read artifacts from cache  
./buildkite-cache-manager.sh read debians/mina-devnet*.deb /workdir

# Override existing files
./buildkite-cache-manager.sh read --override debians/* /workdir
```

### 6. **debian/repositories** (Repository Testing)
Testing infrastructure and HTML repository pages for Mina Protocol's Debian package repositories.

**Components:**
- **Repository pages**: stable, nightly, unstable package listings
- **Automated testing**: Docker-based validation of installation instructions
- **Multi-distribution support**: Testing across different Debian/Ubuntu versions

## Workflow Integration

The toolkit components work together in a coordinated release pipeline:

1. **dhall-buildkite** defines CI/CD pipelines that trigger builds
2. **buildkite-cache-manager.sh** handles artifact caching during builds  
3. **deb-builder** creates Debian packages from build outputs
4. **release-manager** publishes packages to repositories and registries
5. **deb-s3** manages APT repository manifests on S3
6. **debian/repositories** validates published packages

## Development Environment

### Prerequisites
- **Rust** (1.70+) for release-manager
- **OCaml/Dune** (4.14+) for deb-builder  
- **Ruby** (2.7+) for deb-s3
- **Dhall** (1.40+) for dhall-buildkite
- **Docker** for containerized builds and testing
- **gsutil** for Google Cloud Storage operations

### Quick Start
```bash
# Clone with submodules
git clone --recurse-submodules https://github.com/MinaProtocol/mina-release-toolkit.git
cd mina-release-toolkit

# Build all components
cd deb-builder && make build
cd ../release-manager && cargo build --release  
cd ../dhall-buildkite && make all_checks
cd ../deb-s3 && bundle install

# Run tests
cd deb-builder && make test
cd ../release-manager && cargo test
cd ../dhall-buildkite && make check_examples
```

## Storage Backends

### Google Cloud Storage (gs)
- Primary backend for CI/CD cache and artifact storage
- Requires authenticated gsutil installation
- Used for build artifact caching and distribution

### Hetzner Storage  
- Secondary backend for long-term archival
- SSH-based file transfer with key authentication
- Configurable via environment variables (HETZNER_USER, HETZNER_HOST, HETZNER_KEY)

### Local Filesystem
- Development and testing backend
- Uses `/var/storagebox/` as default mount point
- Useful for local development and debugging

## Contributing

This monorepo follows standard Git submodule practices. When making changes:

1. Work in individual submodule directories for component-specific changes
2. Update the main repository to reference new submodule commits
3. Test the entire release pipeline with integration tests
4. Follow each component's specific coding standards (OCaml, Rust, Ruby, Dhall)

## License

This project follows the same license as the Mina Protocol project (Apache 2.0).
