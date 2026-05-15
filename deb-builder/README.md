# deb-builder (Rust)

Rust port of the OCaml `deb_builder` utility. Builds, signs and verifies Debian
packages used by the Mina release pipeline.

## Status

Phase 1 — scaffold and CLI parity. Phases 2-5 in progress.

## CLI

```
deb-builder build         --build-dir … --output-dir … --package-name … --version … \
                          --suite … --codename …   [+ optional metadata]
deb-builder sign          --deb … --key …
deb-builder verify content --deb …  [+ optional metadata]
deb-builder verify signature <deb> [--key <path|url>]
deb-builder lookup sign-key <deb>
```

All flag names match the original OCaml CLI 1:1 (e.g. `--build-dir`,
`--package-name`, `--installed-size`, `--description`/`--package-description`).

## Tools shelled out to

`fakeroot dpkg-deb`, `debsigs`, `debsig-verify`, `gpg` (tests), `curl`.

## Differences from the OCaml original

* `verify content` fixes four latent bugs in `content_verifier.ml`; details in
  the module header. Behavioural change is intentional.
* The control-file template currently mirrors the OCaml behaviour of *not*
  emitting `Depends`/`Suggests`/`Vendor`/`Authors`/etc. into the resulting
  `DEBIAN/control` (see `format_control_file` for the note). Looks like a
  pre-existing OCaml bug; not addressed in this port.
