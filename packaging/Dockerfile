# Multi-stage build for the Mina Release Toolkit.
#
# Stage 1 ("builder") compiles the four in-tree Rust binaries
# against a rust toolchain image. Stage 2 ("runtime") is a slim
# Debian image with just the runtime deps each binary actually
# needs at execution time (dpkg-deb/debsigs/gpg for deb-toolkit,
# the AWS CLI for release-manager's S3 paths, ssh for
# buildkite-cache-manager's Hetzner backend).
#
# Build:
#     docker build -f packaging/Dockerfile -t mina-release-toolkit:dev .
#
# Run any of the bundled tools:
#     docker run --rm mina-release-toolkit:dev deb-toolkit --help
#     docker run --rm mina-release-toolkit:dev release-manager --help
#     docker run --rm mina-release-toolkit:dev mina-bench-upload --help
#     docker run --rm mina-release-toolkit:dev buildkite-cache-manager --help

# ----- 1. Builder ------------------------------------------------------
FROM rust:1.82-bookworm AS builder

WORKDIR /src

# Build deps that some crates need at compile time (openssl headers
# for crates without rustls; pkg-config for any -sys crate that
# probes via pkg-config). Cheap to install once and cached.
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        build-essential pkg-config libssl-dev && \
    rm -rf /var/lib/apt/lists/*

# Copy each crate's source. We copy the four crates individually
# rather than the whole repo so unrelated changes (READMEs,
# submodules we don't use here) don't bust this layer's cache.
COPY release-manager           release-manager/
COPY mina-bench-upload         mina-bench-upload/
COPY buildkite-cache-manager   buildkite-cache-manager/
COPY deb-toolkit               deb-toolkit/

# Each crate is its own standalone build (no workspace at the repo
# root), so we cargo build each in turn. `--locked` keeps the build
# reproducible against the committed Cargo.lock files.
RUN (cd release-manager         && cargo build --release --locked) && \
    (cd mina-bench-upload       && cargo build --release --locked) && \
    (cd buildkite-cache-manager && cargo build --release --locked) && \
    (cd deb-toolkit             && cargo build --release --locked)

# ----- 2. Runtime ------------------------------------------------------
FROM debian:bookworm-slim AS runtime

# Runtime deps grouped by which binary needs them. Anything not
# listed here was either statically linked into the Rust binary or
# is only needed at compile time.
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        ca-certificates \
        # deb-toolkit: build/sign/verify .debs
        fakeroot dpkg-dev debsigs debsig-verify gnupg \
        # release-manager: pushes to APT and S3
        awscli \
        # buildkite-cache-manager: SSH transport to Hetzner
        openssh-client rsync \
        # generally useful for entrypoint and debugging
        bash curl && \
    rm -rf /var/lib/apt/lists/*

# Copy the four release binaries into /usr/bin so they're on PATH.
COPY --from=builder /src/release-manager/target/release/release-manager           /usr/bin/release-manager
COPY --from=builder /src/mina-bench-upload/target/release/mina-bench-upload       /usr/bin/mina-bench-upload
COPY --from=builder /src/buildkite-cache-manager/target/release/buildkite-cache-manager /usr/bin/buildkite-cache-manager
COPY --from=builder /src/deb-toolkit/target/release/deb-toolkit                   /usr/bin/deb-toolkit

# OCI labels — picked up by GHCR / ghcr.io UI.
LABEL org.opencontainers.image.title="Mina Release Toolkit"
LABEL org.opencontainers.image.description="Bundle of deb-toolkit, release-manager, mina-bench-upload, and buildkite-cache-manager."
LABEL org.opencontainers.image.source="https://github.com/MinaProtocol/mina-release-toolkit"
LABEL org.opencontainers.image.licenses="Apache-2.0"

# No fixed ENTRYPOINT: users pick which tool by name on the
# command line. A bare `docker run` lands in bash.
CMD ["bash"]
