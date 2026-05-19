# Packaging

Single Docker image and single `.deb` that bundle all four toolkit
binaries (`deb-toolkit`, `release-manager`, `mina-bench-upload`,
`buildkite-cache-manager`) so a CI box can `apt install` once instead
of cloning four crates and running `cargo build` four times.

## What you get

```
mina-release-toolkit_<version>_amd64.deb
└── /usr/bin/
    ├── deb-toolkit
    ├── release-manager
    ├── mina-bench-upload
    └── buildkite-cache-manager
```

```
ghcr.io/minaprotocol/mina-release-toolkit:<tag>
├── /usr/bin/{deb-toolkit,release-manager,mina-bench-upload,buildkite-cache-manager}
└── runtime deps preinstalled (dpkg-dev, debsigs, gnupg, awscli, …)
```

## Building locally

### Docker

```bash
# From repo root.
docker build -f packaging/Dockerfile -t mina-release-toolkit:dev .
docker run --rm mina-release-toolkit:dev deb-toolkit --help
```

### Debian

```bash
# Build all four binaries first.
(cd release-manager         && cargo build --release)
(cd mina-bench-upload       && cargo build --release)
(cd buildkite-cache-manager && cargo build --release)
(cd deb-toolkit             && cargo build --release)

# Then assemble the .deb. Uses the just-built deb-toolkit binary to do
# the actual `dpkg-deb --build`, so dpkg-dev / fakeroot need to be on
# PATH locally.
packaging/build-deb.sh 0.1.0 ./out
# → ./out/mina-release-toolkit_0.1.0_amd64.deb

# Install and verify.
sudo apt install ./out/mina-release-toolkit_0.1.0_amd64.deb
deb-toolkit --help
```

## How CI publishes these

The `.github/workflows/package.yml` workflow builds both artifacts:

| Trigger | Docker | .deb |
| --- | --- | --- |
| PR / push to non-tag | builds, doesn't push | builds, uploads as a workflow artifact |
| Tag push `v*` | builds + pushes to `ghcr.io/minaprotocol/mina-release-toolkit:<tag>` and `:latest` | builds + attaches to the matching GitHub Release |

## Files in this directory

| File | What it is |
| --- | --- |
| [`Dockerfile`](Dockerfile) | Multi-stage build. Stage 1 compiles the four Rust crates against `rust:1.82-bookworm`. Stage 2 is `debian:bookworm-slim` with just the runtime deps. |
| [`build-deb.sh`](build-deb.sh) | Drives the .deb build using the just-built `deb-toolkit` binary (eats our own dogfood). |
| [`defaults/toolkit.json`](defaults/toolkit.json) | Defaults file consumed by `deb-toolkit build` — pins per-release-invariant fields (maintainer, description, runtime depends, etc.). |
