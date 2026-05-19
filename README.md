# Mina Release Toolkit

The tools the Mina Protocol release pipeline uses to build, sign, ship,
and verify packages. Six self-contained components live side-by-side
in this repo; this README is a map and pointers — each component's
own `README.md` is the detailed reference.

## At a glance

| Component | Language | What it does | Detail |
| --- | --- | --- | --- |
| [`deb-toolkit/`](deb-toolkit/) (submodule) | Rust | Build, sign, verify, and transactionally edit `.deb` packages. Includes the `session` subsystem for hardfork-style mutations. | [README](deb-toolkit/README.md) |
| [`release-manager/`](release-manager/) | Rust | Publish artifacts to Debian repos / Docker registries; promote between channels; verify; archive. | [README](release-manager/README.md) |
| [`mina-bench-upload/`](mina-bench-upload/) | Rust | Parse benchmark output (7 formats) and upload to InfluxDB with regression checks. | [README](mina-bench-upload/README.md) |
| [`buildkite-cache-manager/`](buildkite-cache-manager/) | Rust | Read/write/list/prune Buildkite CI cache on Hetzner shared storage. | [README](buildkite-cache-manager/README.md) |
| [`deb-s3/`](deb-s3/) (submodule) | Ruby | Manage APT repositories on S3 (upload, delete, verify, repair). Forked from `krobertson/deb-s3`. | [README](deb-s3/README.md) |
| [`dhall-buildkite/`](dhall-buildkite/) (submodule) | Dhall | Type-safe, composable Buildkite pipeline configs. | [README](dhall-buildkite/README.md) |
| [`debian/`](debian/) | HTML + bash | Static welcome pages for the Debian repositories + a Docker-based test harness. | — |

## How the pieces fit

```
                    ┌──────────────────────────┐
                    │       dhall-buildkite    │ defines CI pipelines
                    └─────────────┬────────────┘
                                  │ triggers
                                  ▼
        ┌─────────────────┐  ┌─────────────────┐  ┌──────────────────┐
        │  buildkite-     │  │   deb-toolkit   │  │  mina-bench-     │
        │  cache-manager  │◀─│   builds .deb   │  │  upload          │
        │  caches in/out  │  │   signs, mutates│  │  reports to      │
        └─────────────────┘  └────────┬────────┘  │  InfluxDB        │
                                      │           └──────────────────┘
                                      ▼
                          ┌──────────────────────┐
                          │   release-manager    │ promote / publish
                          └────────┬─────────────┘
                                   ▼
                        ┌────────────────────┐
                        │   APT repo on S3   │ (managed by deb-s3)
                        └────────────────────┘
```

## Quick start

```bash
# Clone with submodules (deb-toolkit, deb-s3, dhall-buildkite).
git clone --recurse-submodules https://github.com/MinaProtocol/mina-release-toolkit.git
cd mina-release-toolkit

# Or update submodules in an existing clone.
git submodule update --init --recursive

# Build the three local Rust tools.
(cd release-manager       && cargo build --release)
(cd mina-bench-upload     && cargo build --release)
(cd buildkite-cache-manager && cargo build --release)

# deb-toolkit ships as a submodule; build it in-place.
(cd deb-toolkit && cargo build --release)

# Run all the Rust test suites.
for crate in release-manager mina-bench-upload buildkite-cache-manager deb-toolkit; do
    (cd "$crate" && cargo test) || break
done
```

## Prerequisites

| Need | For |
| --- | --- |
| Rust 1.70+ | `deb-toolkit`, `release-manager`, `mina-bench-upload`, `buildkite-cache-manager` |
| Ruby 2.7+ | `deb-s3` |
| Dhall 1.40+ | `dhall-buildkite` |
| `dpkg-deb`, `fakeroot`, `debsigs`, `debsig-verify`, `gpg` | `deb-toolkit` integration tests |
| Docker | repository welcome-page tests under `debian/` |
| `gsutil`, AWS CLI | `release-manager` GCS / S3 operations |

## CI

Per-component GitHub Actions workflows live in `.github/workflows/`.
Each is path-filtered to its own directory:

| Workflow | Triggers on changes to |
| --- | --- |
| `release-manager.yml` | `release-manager/**` |
| `mina-bench-upload.yml` | `mina-bench-upload/**` |
| `debian-repositories.yml`, `test-debian-repositories.yml`, `test-html-pages.yml` | `debian/**` |

The `deb-toolkit` submodule has its own CI in its own repo.

## Storage backends

Several components touch shared storage. The conventions are:

- **Google Cloud Storage** (`gs://...`) — CI cache and primary artifact
  staging. Requires authenticated `gsutil`.
- **Hetzner storage box** (SSH+SFTP) — long-term archival and CI cache
  fallback. Configured via `HETZNER_USER`, `HETZNER_HOST`, `HETZNER_KEY`.
- **Local filesystem** (`/var/storagebox/` by default) — local dev /
  testing backend.

`buildkite-cache-manager/README.md` documents the cache layout.

## Contributing

Each component is independently testable. For changes inside a
submodule (`deb-toolkit`, `deb-s3`, `dhall-buildkite`):

1. Land the change in the submodule's own repo via a PR there.
2. Bump the submodule pointer in this repo:
   `cd <submodule> && git checkout <new SHA> && cd .. && git add <submodule>`.
3. Open a PR here with the pointer bump.

For in-tree crates (`release-manager`, `mina-bench-upload`,
`buildkite-cache-manager`), normal PR workflow applies — the per-crate
GitHub Actions runs `cargo fmt`, `cargo clippy`, and `cargo test` on
every push.

## License

Apache-2.0. See [LICENSE](LICENSE).
