# mina-ops

A read-only view across the systems a Mina release touches.

Buildkite knows about builds. The Debian repositories know about packages. The
Docker registries know about images. None of them knows about the others, and
the question that comes up in an incident always crosses all three:

> For commit `b3762e1`: which builds ran, which packages exist in which
> repository and architecture, and which images were pushed?

Answering that by hand takes six queries in three tools. `mina-ops` answers it
in one command, and prints JSON when a script or an agent is asking.

It reads only. Publishing, promotion and repair stay in
[`release-manager`](../release-manager/), and the package-naming conventions
are imported from that crate rather than copied, so the two cannot disagree
about what "published" means.

## Install

```bash
cd mina-ops
cargo build --release
# ./target/release/mina-ops
```

## Credentials

`mina-ops` borrows the credentials already on your machine. It stores nothing.

| System | Source | Needed for |
| --- | --- | --- |
| Buildkite | `BUILDKITE_API_TOKEN` or `BUILDKITE_API_ACCESS_TOKEN`, else `~/.config/mina-ops/buildkite-token` | build lookup; scopes `read_builds`, `read_artifacts` |
| Debian repositories | whatever `deb-s3` and `aws` already use | package listings |
| Docker registries | whatever `docker` already uses | image presence |
| GitHub | whatever `gh` already uses (`gh auth login`) | the pull request behind a commit |
| CI cache | a mounted path, or ssh — see "The CI cache" below | packages a build produced |

A missing tool or credential is reported as `unknown`, never as `missing`. See
"Honesty" below.

## Use

Everything about one commit. A short hash is expanded with a local checkout:

```bash
mina-ops artifacts --commit 8c0c2e6 --repo-path ~/work/minaprotocol/mina \
  --channel stable --codenames noble
```

```
Project: mina
Commit:  8c0c2e63c26bf9c7be8b4ae6b0452471dd6d4d6b
Version: 3.3.0-8c0c2e6 (recovered from the Debian repository)
Channel: stable · network mainnet

Buildkite
  mina-stable              #1661    failed     master     1 artifacts
  ...

Debian packages
  packages.o1test.net · noble
    [ok]      mina-mainnet                     amd64
    [ok]      mina-archive-mainnet             arm64
  ...

Summary
  Debian: 20/20 present
```

Just the builds:

```bash
mina-ops builds --commit 143b9cc0ae1d33bd16d0d1623f8b636d49646b11
```

A version you already know, with no Buildkite call:

```bash
mina-ops artifacts --version 3.2.0-49b523c --channel stable --skip-buildkite
```

JSON for scripts:

```bash
mina-ops --json artifacts --commit 8c0c2e6 --repo-path ../../mina
```

## Nightly triage

A failing nightly tells you what is red. It does not tell you what is *newly*
red, which is where triage actually starts.

```bash
mina-ops nightly --last 3
```

```
Builds
  #1587    failed       5 failing of 99   adfabd713 2026-08-12
      #19201 restore ci-single-me and add a deploy makefile (merged, dkijania)
  #1585    failed       6 failing of 101  6616118b6 2026-08-11

New in #1587 (1)
  [new]       libp2p unit-tests

Already failing before #1587
  [2 builds] hard fork test - legacy mode
  [3 builds] Perf: Archive (soft failure, does not fail the build)

Fixed since the previous build
  [fixed] Hardfork: Package Conversion
```

Three rules keep the labels honest:

- **Retried attempts do not count.** A job that failed and passed on retry is
  not a failure; the retry's outcome is the one that counts.
- **Soft failures are reported and marked.** They do not turn the build red,
  but they are still regressions, so they are neither hidden nor mixed in with
  the failures that broke the build.
- **One build alone yields `unknown`, not `new`.** With nothing to compare
  against, no failure can honestly be called new.

Jobs are matched across builds by step key rather than by label, because
labels carry emoji and wording that change between builds — matching on those
would make a long-standing failure look new.

Without `--branch`, release branches are mixed into the comparison and
failures appear to come and go. The project default is `develop`.

## The console

```bash
mina-ops serve            # prints http://127.0.0.1:7777/?t=<token>
```

One page over the same API: look up a commit, compare nightlies, and fill in
the hardfork package-generation parameters as a form rather than pasting a
block of environment variables into Buildkite.

The hardfork tab checks every field before anything is created — codenames and
network against the values the pipeline's Dhall accepts, the timestamp for
UTC, the config URL for existence, and the referenced build UUID for packages
still in the CI cache. It then shows the exact environment block that would be
sent. "Copy as env block" gives you that text if you would rather start the
build from the Buildkite UI.

Creating a build requires an explicit confirmation, and the server re-runs
every check on the confirmed request rather than trusting the browser's copy.
Afterwards it hands you Buildkite's URL and sends you there: the console
renders no build state.

### Why a page on localhost needs guarding

Any page open in the same browser can send requests to a localhost port.
Three rules close that off:

1. The listener binds `127.0.0.1`, never `0.0.0.0`.
2. Every request must carry a token generated at startup and changed on every
   start. The page reads it from the URL it was opened with and sends it in a
   header, which a cross-origin page cannot set without a preflight this
   server never grants.
3. The `Host` header must be a loopback address, which is what stops DNS
   rebinding.

No CORS headers are sent, deliberately.

## The cache tab

The CI cache is where Mina's packages actually live, and it grows by roughly
200 GB a day. The tab lists every entry with its size and date — the whole
cache in about a second, because `du --max-depth=1` and `ls -lt` each walk only
the top level — and expanding a build shows its packages and versions.

Removal is the one destructive operation in this tool, and it acts on the
cache Buildkite reads from. Four guards run **on the server**, in this order,
and all four must pass:

| Guard | Refuses |
| --- | --- |
| is a build folder | anything that is not a build UUID, so `legacy`, `docker-cache`, `debs` and `test_data` can never be removed |
| confirmation matches | a request that does not type the UUID back |
| not in use by Buildkite | a build Buildkite is running, scheduling or creating — and also a request where that could not be checked, because not knowing is not permission |
| exists in the cache | a folder that is already gone |

A request with no `dry_run` field is a dry run. Deletions are appended to
`~/.local/state/mina-ops/deletions.log`.

Bulk pruning is deliberately absent. `buildkite-cache-manager prune` does that
against a mounted cache, where a mistake is easier to notice than in a browser.

## As an MCP server

`mina-ops mcp` serves the same queries over MCP on stdin and stdout, so an
agent can ask what exists for a commit during an incident instead of shelling
out and parsing text. It inherits the credentials of whoever starts it, so it
reaches exactly what that person reaches, and it only reads.

```bash
claude mcp add mina-ops -- /path/to/mina-ops mcp
```

Two tools are exposed:

| Tool | Answers |
| --- | --- |
| `mina_artifacts` | which packages and images exist for a commit or version |
| `mina_builds` | which Buildkite builds ran for a commit |
| `mina_nightly` | what broke in the newest nightly, and what was already broken |

Both return the same JSON the CLI prints, `warnings` included, so the agent
sees what limited the answer.

Set `BUILDKITE_API_TOKEN` in the environment the server starts in, or write it
to `~/.config/mina-ops/buildkite-token`. Without it the Debian and Docker
checks still work and the missing token is reported as a warning.

## The CI cache

Mina's pipelines do not upload `.deb` files to Buildkite. They put them in the
shared cache on the Hetzner storage box, keyed by the Buildkite **build UUID**
— which is what `USE_ARTIFACTS_FROM_BUILDKITE_BUILD` takes:

```text
<root>/<build-uuid>/debians/<codename>/<package>_<version>_<arch>.deb
```

The architecture is part of the filename, not a directory. (`buildkite-cache-manager`'s
README documents an extra `<arch>/` level; the cache observed in August 2026
has no such level. Both shapes are accepted.)

Nothing about the cache is committed here, because this repository is public.
Configure it in `~/.config/mina-ops/projects.yaml` under `cache.root` or
`cache.ssh`, or through the environment:

```bash
export MINA_OPS_CACHE_ROOT=/var/storagebox          # a mounted path, as CI has
# or
export MINA_OPS_CACHE_SSH_HOST=... MINA_OPS_CACHE_SSH_USER=... \
       MINA_OPS_CACHE_SSH_ROOT=... MINA_OPS_CACHE_SSH_PORT=23 \
       MINA_OPS_CACHE_SSH_KEY=~/.ssh/storagebox.key
```

Two behaviours are deliberate:

- **An empty cache root reports `unknown`, not "nothing cached".** An
  unmounted share looks exactly like an empty one, and reporting a miss would
  send somebody to rebuild packages that are sitting on the storage box.
- **A configured mount is used only when it holds something**, otherwise ssh
  is tried. That is what makes the same configuration work in CI and on a
  workstation where the share is not mounted.

The storage box runs a restricted shell: no `cd`, no `&&`, no glob expansion.
The lookup is therefore a single `ls -R` per build.

## How a commit becomes a version

Mina package versions embed the 7-character short commit, for example
`3.3.0-8c0c2e6`. The version is resolved in this order:

1. `--version`, when given.
2. The `.deb` artifacts of the commit's Buildkite builds. **Rarely fires for
   Mina**: its pipelines upload logs and tools to Buildkite and put packages in
   the CI cache, so most builds carry no `.deb` at all.
3. The packages the build left in the CI cache. This answers for a commit
   whose packages have not been published anywhere yet — the state a fresh
   build is in.
4. A scan of the Debian repositories for a version ending in `-<short commit>`.
   This keeps working long after Buildkite has dropped the build.

When no version can be resolved, the tool says so and checks nothing, rather
than reporting every package as missing.

## Honesty

Every check has three outcomes, not two:

- `present` — the system was asked and it has the artifact.
- `missing` — the system was asked and it does not have the artifact.
- `unknown` — the check could not run, with the reason attached.

`unknown` exists because the alternative causes real waste. A registry
authentication failure reported as `missing` sends somebody to rebuild an image
that was there all along. Anything that limited the answer — a skipped check, a
tool that is not installed, a truncated query — is listed under "Limits and
warnings" and in the `warnings` field of the JSON.

## Configuration

A project is a configuration entry, not a code path. The registry lives in
[`projects.example.yaml`](projects.example.yaml), which is also compiled into
the binary as the default, so the tool runs with no setup.

Resolution order: `--config`, then `MINA_OPS_CONFIG`, then
`~/.config/mina-ops/projects.yaml`, then the built-in default. The path in use
is printed after each report.

This repository is public. Keep internal hosts out of the committed file and
put them in your own copy.

## Rate limits

The Buildkite REST API allows 200 requests per minute per token. One request
finds the builds for a commit, and one more is spent per build whose artifacts
are listed. `--max-builds` bounds that, and the report says when the limit
truncated the answer.

## Not in scope

`mina-ops` does not render what another tool renders better. No build logs, no
pull-request review, no metric charts. It reports what crosses systems and
links out to Buildkite, GitHub and Grafana for the detail.
