# Mina Protocol Release Manager (Rust)

A Rust implementation of the Mina Protocol release manager script, providing comprehensive release management functionality for build artifacts.

## Overview

This tool handles the complete lifecycle of build artifacts including publishing, promotion, verification, and maintenance of packages across different channels and platforms.

### Main Capabilities

- **PUBLISH**: Put already-built `.deb` files into a Debian repository, unchanged, then check each one is really there
- **PUBLISH-FROM-CACHE**: Legacy. Pull from the CI cache by Buildkite build id, re-versioning on the way
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

Put `.deb` files that are already at their final version into a Debian
repository, exactly as they are, and then confirm they arrived.

```bash
release-manager publish \
  --source-folder _debs \
  --debian-repo stable.apt.packages.minaprotocol.com \
  --channel stable \
  --debian-sign-key 386E9DAC378726A48ED5CE56ADB30D9ACE02F414 \
  --verify
```

`--source-folder` holds a `{codename}/*.deb` tree — the layout `pull` and
`reversion` write, and the layout the Buildkite cache stores. Every codename
subfolder found is published, unless `--codenames` names a subset.

**Publish rewrites nothing.** There is no `--source-version` /
`--target-version` pair and no `--buildkite-build-id`:

| Not an argument | Because |
| --- | --- |
| version | It is already in the package and in the file name. A version argument could only agree with the package or contradict it, and contradicting it is a re-version — which is `reversion`, a separate and visible step. |
| architecture | `deb-s3` reads `Architecture` from each package, so a folder holding both amd64 and arm64 publishes correctly in one call. |
| build id | Fetching from the CI cache is `pull`'s job. Keeping the two apart means `publish` can be tested without a Buildkite build to point at. |

Use it when the build already produced the final version and the final suite,
which is the direct shape: build, test, publish. Use `publish-from-cache` when
the artifact has to change on its way to the repository, and `promote` when it
is already published and moving between channels.

One `deb-s3 upload` call is made per codename rather than per package:
`deb-s3` takes the repository lock for a whole invocation, so uploading package
by package would take and release the lock, and rewrite the manifest, once per
package.

##### What is already there

Before uploading, each package is compared against what the repository already
holds at that name, version and architecture, using `deb-s3 show`:

| Repository | What happens |
| --- | --- |
| absent | uploaded |
| present, same SHA256, `.deb` really in the pool | skipped, and reported as already published |
| present, same SHA256, pool object missing or unconfirmed | uploaded again, saying why |
| present, different SHA256 | **the command fails**, naming both digests |

So a re-run over the same folder — the retry after a partial upload — converges
and exits 0 without needing a human, while a second, *different* build at the
same version is refused instead of quietly replacing what users install.

**A skip needs the pool object, not just the index.** deb-s3 writes the
`Packages` index, then `Release`, then releases the lock, and only then uploads
the `.deb` files themselves (`cli.rb:254-283`). A run killed in that tail — the
long part, the part no longer holding the lock — leaves an index advertising a
package, with the right SHA256, whose `.deb` is not there. So `show` reporting
a matching digest is not enough to skip: the `Filename:` it names is checked
with `aws s3api head-object` first, and anything short of "the object is there"
means upload. `--verify` cannot cover this gap for you — it asks `deb-s3
exist`, which reads the same manifest `show` does, so for a skipped package it
re-asserts what `show` already said and never touches the pool.

An `Architecture: all` package — the `mina-{network}-config` ones — is never
skipped. deb-s3 merges those into every architecture manifest that exists at
the time of the upload, so skipping one would leave an architecture that has
appeared since it was first published without it.

If every package in a codename is already published, the `deb-s3 upload` call is
skipped for it, but `--verify` and the CDN invalidation still run, because a
previous partial run may have left a stale index cached. Each codename is
checked before *any* codename is uploaded, so a package that would be refused
in the last codename fails the command before the first one is published.

This check is what makes the guarantee hold. `deb-s3 --fail-if-exists` is still
passed, but it does not do this job: it raises only when the same name and
version is in the manifest under a *different* pool file name, so a re-publish
of the same file name falls through and replaces the pool object, reporting
success. Verified against the pinned fork and a real S3 — publishing different
bytes at a published version exits 0 and changes the `SHA256:` the repository
serves. The flag is kept as a zero-cost backstop for the cases it does cover.

The digest of the local file is only computed once `show` says the package is
present, so a first publish pays for one manifest read per package and nothing
else. An output that cannot be read — a `show` that fails for any reason other
than `No such package found.`, or a stanza with no usable `SHA256:` — fails the
command rather than being assumed to mean "absent", for the same reason
`--verify` refuses to read "no verdict" as "found". The pool check is the one
place where an unanswered question is not fatal: uploading bytes that are
already there is safe and idempotent, so a `head-object` that cannot run (no
`aws` on PATH, no credentials) uploads instead of skipping.

Two consequences worth stating plainly. The pre-flight needs a package's name,
version and architecture, so a `.deb` whose file name is not
`{name}_{version}_{arch}.deb` now fails the publish even without `--verify` —
before, only `--verify` was that strict. And the pre-flight reads the version
from the file name, while deb-s3 matches on the control file's `Version:`; the
two agree for every version Mina builds, but a package carrying an epoch
(`1:3.0.0-…`) would read as absent forever and the check would silently not
engage. `--dry-run` stays offline and does not run the pre-flight at all.

##### Verification

`--verify` asks the repository, package by package, whether it is now listed at
the version and architecture it was built with, and fails the command if any is
not. It is `deb-s3 exist`, not `deb-s3 verify`: the latter checks that the
manifest is internally consistent, which it can be while saying nothing about
the packages you just pushed.

Three details are worth knowing, because each one is a way this check could
have passed without checking anything:

- **The exit status is unusable.** The pinned `deb-s3` fork exits 0 whether a
  package is there or not and states the answer on stdout, so the output is
  parsed.
- **Two `deb-s3` builds are in use and word it differently.** The fork prints
  `name : Found`; the Debian `ruby-deb-s3` gem prints `>> name version arch:
  Found`. They also disagree about the subcommand — `exist` taking one
  space-joined argument versus `exists` taking separate ones — so exactly one
  package is asked about per call, the only shape both accept.
- **"No verdict" is not "found".** A package the output never mentions fails the
  command with `cannot tell whether …`, and is not retried. Treating an
  unreadable answer as success is how a publish reports a package it never
  checked.

`--verify-attempts` (default 10) and `--verify-interval-secs` (default 30)
control the retry. The retry is not padding: the bucket sits behind a CDN and
the index is rewritten as a whole object, so a read straight after a write can
legitimately still serve the previous manifest. Only packages that have not
turned up yet are re-asked about.

##### Other flags

- `--force` skips the pre-flight check above and drops `--fail-if-exists`, so an
  existing package at that version is overwritten. It is the way to say the
  published copy is the wrong one. Off by default.
- `--skip-cache-invalidation` leaves the CloudFront cache alone. By default the
  `dists/{codename}/*` prefix is invalidated, so readers are not served a stale
  `Packages` index.
- `--dry-run` lists what would be published and stops.
- `--s3-endpoint` and `--s3-force-path-style` point `deb-s3` at an
  S3-compatible server (a mirror, or a local MinIO) instead of AWS. Credentials
  are deliberately not options — `deb-s3` reads them from the environment,
  which is where CI keeps them and keeps them out of the process list.

An empty codename folder, or a source folder with no codename folders in it, is
an error rather than a silent success: a pipeline that publishes nothing must
not report that it published.

#### Publish from cache (legacy)

Publish build artifacts from cache to repositories and registries.

```bash
release-manager publish-from-cache \
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