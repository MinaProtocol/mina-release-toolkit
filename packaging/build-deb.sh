#!/usr/bin/env bash
# Build a single .deb that bundles all four toolkit binaries.
#
# Eats our own dogfood: invokes `deb-toolkit` (one of the bundled
# binaries) to produce the .deb. The CI workflow at
# .github/workflows/package.yml drives this; nothing stops you
# running it locally too.
#
# Usage:
#   packaging/build-deb.sh <version> <output_dir>
#
# Expects the four release binaries to already exist at:
#   <crate>/target/release/<crate-name>
# (Run `cargo build --release` in each crate first, or use the
# `packaging/build-binaries.sh` helper.)
#
# Output:
#   <output_dir>/mina-release-toolkit_<version>_amd64.deb

set -euo pipefail

if [[ $# -ne 2 ]]; then
    echo "Usage: $0 <version> <output_dir>" >&2
    exit 2
fi

VERSION="$1"
OUTPUT_DIR="$2"
ARCH="${ARCH:-amd64}"

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEB_TOOLKIT_BIN="${REPO_ROOT}/deb-toolkit/target/release/deb-toolkit"

if [[ ! -x "${DEB_TOOLKIT_BIN}" ]]; then
    echo "deb-toolkit release binary not found at ${DEB_TOOLKIT_BIN}." >&2
    echo "Run 'cargo build --release' in deb-toolkit/ first." >&2
    exit 1
fi

# Stage the binaries into a build directory laid out the way
# `deb-toolkit build` expects: every file's relative path under the
# build dir becomes its installed path inside the package.
STAGING="$(mktemp -d)"
trap 'rm -rf "${STAGING}"' EXIT

install -D -m 0755 "${REPO_ROOT}/release-manager/target/release/release-manager"                  "${STAGING}/usr/bin/release-manager"
install -D -m 0755 "${REPO_ROOT}/mina-bench-upload/target/release/mina-bench-upload"              "${STAGING}/usr/bin/mina-bench-upload"
install -D -m 0755 "${REPO_ROOT}/buildkite-cache-manager/target/release/buildkite-cache-manager"  "${STAGING}/usr/bin/buildkite-cache-manager"
install -D -m 0755 "${DEB_TOOLKIT_BIN}"                                                            "${STAGING}/usr/bin/deb-toolkit"

mkdir -p "${OUTPUT_DIR}"

# Drive deb-toolkit. The defaults file in packaging/defaults/toolkit.json
# pins fields that don't change per release (maintainer, description,
# section, runtime depends). The CLI flags supply the per-build bits
# (version, codename).
"${DEB_TOOLKIT_BIN}" build \
    --build-dir   "${STAGING}" \
    --output-dir  "${OUTPUT_DIR}" \
    --package-name mina-release-toolkit \
    --version     "${VERSION}" \
    --suite       stable \
    --codename    bookworm \
    --architecture "${ARCH}" \
    --defaults-file "${REPO_ROOT}/packaging/defaults/toolkit.json"

echo
echo "Built: ${OUTPUT_DIR}/mina-release-toolkit_${VERSION}_${ARCH}.deb"
