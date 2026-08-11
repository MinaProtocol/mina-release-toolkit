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

JSON for scripts and agents:

```bash
mina-ops --json artifacts --commit 8c0c2e6 --repo-path ../../mina
```

## How a commit becomes a version

Mina package versions embed the 7-character short commit, for example
`3.3.0-8c0c2e6`. The version is resolved in this order:

1. `--version`, when given.
2. The `.deb` artifacts of the commit's Buildkite builds. **Rarely fires for
   Mina**: its pipelines upload logs and tools to Buildkite and put packages in
   the CI cache, so most builds carry no `.deb` at all.
3. A scan of the Debian repositories for a version ending in `-<short commit>`.
   This is the normal path, and it keeps working long after Buildkite has
   dropped the build.

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
