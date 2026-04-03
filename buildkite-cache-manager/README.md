# buildkite-cache-manager

CLI tool for managing Buildkite CI cache on Hetzner shared storage. Replaces the original `buildkite-cache-manager.sh` bash script with a Rust implementation, adding `list` and `prune` commands.

## Cache Structure

```
/var/storagebox/                       # CACHE_BASE_URL (configurable)
├── <buildkite-build-id>/              # UUID-formatted Buildkite build IDs
│   ├── debians/
│   │   ├── <codename>/                # bullseye, focal, noble, jammy, bookworm
│   │   │   ├── amd64/
│   │   │   │   └── *.deb
│   │   │   ├── arm64/
│   │   │   │   └── *.deb
│   │   │   └── all/
│   │   │       └── *.deb
│   │   └── ...
│   └── <other-artifacts>/
├── legacy/                            # Legacy artifacts
│   └── ...
└── ...
```

## Commands

### read

Copy cached artifacts from Hetzner storage to a local path. Requires `BUILDKITE_BUILD_ID`.

```bash
buildkite-cache-manager read debians/mina-devnet*.deb /workdir
buildkite-cache-manager read --root custom-root artifacts/build.tar.gz /output
buildkite-cache-manager read --override --skip-dirs-create file.txt /existing-dir
```

### write

Copy local files to the cache. Requires `BUILDKITE_BUILD_ID`.

```bash
buildkite-cache-manager write mina-devnet*.deb debians/
buildkite-cache-manager write --override build.tar.gz artifacts/
buildkite-cache-manager write --root custom-root file.txt uploads/
```

### list

List files and folders in the cache. Does not require `BUILDKITE_BUILD_ID`.

```bash
# List all top-level cache folders (build IDs, legacy, etc.)
buildkite-cache-manager list

# List contents of a specific build folder
buildkite-cache-manager list a1b2c3d4-e5f6-7890-abcd-ef1234567890

# List debian packages with codename/architecture awareness
buildkite-cache-manager list a1b2c3d4-e5f6-7890-abcd-ef1234567890 --debians

# Output as JSON (useful for scripting and LLM integration)
buildkite-cache-manager list --format json

# Plain text output (tab-separated, one entry per line)
buildkite-cache-manager list --format plain
```

The `--debians` flag walks the expected debian structure (`debians/<codename>/<arch>/*.deb`) and displays packages grouped by codename and architecture. It also handles flat `.deb` files and detects architecture from filenames (e.g., `mina-devnet_1.0.0_amd64.deb`).

### prune

Remove cache folders based on age, version, or timestamp.

```bash
# Remove build folders older than 30 days
buildkite-cache-manager prune --older-than 30d

# Keep only 5 latest builds by modification time
buildkite-cache-manager prune --keep-latest-timestamp 5

# Keep only 3 latest by version number
buildkite-cache-manager prune --keep-latest-versions 3

# Prune only legacy folders
buildkite-cache-manager prune --older-than 7d --folder-type legacy

# Prune all folder types
buildkite-cache-manager prune --keep-latest-timestamp 10 --folder-type all

# Preview what would be deleted (safe!)
buildkite-cache-manager prune --older-than 14d --dry-run
```

Duration formats: `12h` (hours), `30d` (days), `2w` (weeks), `3m` (months, ~30 days each).

## Hetzner Cache Setup

The tool operates on a locally-mounted filesystem. In CI (Buildkite), the Hetzner storage
is already mounted at `/var/storagebox`. For local use, you need to mount it via sshfs first.

### One-time mount (persists until unmount or reboot)

```bash
# Install sshfs if needed (Ubuntu/Debian)
sudo apt install sshfs

# Create mount point
sudo mkdir -p /var/storagebox

# Mount the Hetzner storage box
sudo sshfs -o ssh_command="ssh -p 23 -i ~/work/secrets/storagebox.key",allow_other \
  u434410@u434410-sub2.your-storagebox.de:/home/o1labs-generic/pvc-4d294645-6466-4260-b933-1b909ff9c3a1 \
  /var/storagebox
```

Or mount to a custom path and set `CACHE_BASE_URL`:

```bash
mkdir -p /tmp/hetzner-cache

sshfs -o ssh_command="ssh -p 23 -i ~/work/secrets/storagebox.key" \
  u434410@u434410-sub2.your-storagebox.de:/home/o1labs-generic/pvc-4d294645-6466-4260-b933-1b909ff9c3a1 \
  /tmp/hetzner-cache

export CACHE_BASE_URL=/tmp/hetzner-cache
```

The mount persists for the duration of your session. You only need to remount if:
- The machine reboots
- You manually unmount (`fusermount -u /tmp/hetzner-cache`)
- The network connection to Hetzner drops

### Unmount when done

```bash
fusermount -u /tmp/hetzner-cache
# or, if mounted with sudo:
sudo umount /var/storagebox
```

### Persistent mount (survives reboots)

Add to `/etc/fstab`:

```
u434410@u434410-sub2.your-storagebox.de:/home/o1labs-generic/pvc-4d294645-6466-4260-b933-1b909ff9c3a1 /var/storagebox fuse.sshfs port=23,IdentityFile=/home/<user>/work/secrets/storagebox.key,_netdev,reconnect,ServerAliveInterval=15 0 0
```

## Environment Variables

| Variable | Required | Default | Description |
|---|---|---|---|
| `BUILDKITE_BUILD_ID` | For read/write | - | Buildkite build ID, used as default cache root |
| `CACHE_BASE_URL` | No | `/var/storagebox` | Cache mount point base path |

## LLM Integration

All commands support `--format json` for structured output, making it easy to pipe into LLM-based workflows:

```bash
# Feed cache listing to an LLM for analysis
buildkite-cache-manager list --format json | llm "Which builds are older than a week?"

# Get debian package inventory as JSON
buildkite-cache-manager list <build-id> --debians --format json
```

The `--dry-run` flag on prune makes it safe to let an LLM suggest cleanup operations:

```bash
# LLM suggests, human reviews
buildkite-cache-manager prune --older-than 30d --dry-run
```

## Building

```bash
cargo build --release
```

## Testing

Tests use an in-memory mock of the Hetzner cache filesystem (`MockBackend`), so no real storage is needed:

```bash
cargo test
```

The mock backend (`src/mock.rs`) implements the `CacheBackend` trait with a `HashMap`-backed filesystem, supporting all operations: list, copy, remove, glob, and directory creation.
