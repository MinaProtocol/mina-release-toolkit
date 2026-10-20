# Published artifact and infrastructure checks

These standalone Bash scripts test what Mina operators can download and install
from the public Debian repositories and Docker registry. They do not build Mina,
check out the Mina source tree, or require Buildkite, Dhall, or credentials.

The default distributions are Noble, Bookworm, and Jammy on amd64. Devnet
packages come from `alpha`; mainnet packages come from `stable`.

## Run locally

Requirements: Linux, Bash, curl, gzip, GNU coreutils/findutils, and Docker for
package/image checks. These tools are available on GitHub-hosted Ubuntu runners.

```bash
# Check non-empty apt indexes and Docker repository availability.
./tests/infra/run-verification.sh

# Install current packages and automode metapackages for both networks.
VERIFY_MODE=artifacts ./tests/infra/run-verification.sh

# Test one distribution, with fewer concurrent containers.
VERIFY_MODE=artifacts CODENAMES=noble JOBS=2 ./tests/infra/run-verification.sh

# Override the default package set; versions and per-package channels are optional.
VERIFY_MODE=artifacts \
PACKAGES=mina-devnet,mina-mainnet@stable \
AUTOMODE=mina-devnet-automode,mina-mainnet-automode@stable \
./tests/infra/run-verification.sh
```

`run-verification.sh` documents all environment variables. The underlying
`check-infra.sh` and `test-release.sh` scripts also accept command-line options.

Docker images require an explicit version in `IMAGES`; there is no implicit
Docker tag that resolves to the current Debian channel. Image checks are
therefore opt-in. The default artifact set tests Debian packages and unpinned
automode metapackages, including both pre-fork and post-fork runtimes.

Artifact results are written to `verification-results/results.tsv`, with
per-container logs below `verification-results/logs/`. `WARN` records known
conditions without failing a run; `FAIL` produces a nonzero exit status.
The Rosetta check currently adds known undeclared runtime libraries, and records
these prerequisites in the report: a pass does not prove its dependency metadata
is complete.

## GitHub Actions

`.github/workflows/verify-artifacts.yml` runs every night at **02:17 UTC**, can be
started manually, and runs on PRs or main-branch changes to these scripts or the
workflow. Scheduled runs begin once the workflow is merged into `main`.

Infrastructure checks run separately from package installation checks. The
artifact job uses one runner per distribution, with `fail-fast: false`, so a
failure on one distribution does not cancel the others. Each runner uses two
concurrent containers. Both job types upload reports and logs even on failure,
with 14-day retention. No publishing credentials are needed.

GitHub schedules can be delayed. For public repositories, scheduled workflows
are disabled after 60 days without repository activity. This is periodic release
verification, rather than an availability alerting service.

## Origin

Ported from MinaProtocol/mina's `dkijania/artifact-verification-tooling` branch
(rebased tip `567bccbec47116d06f2f67856cdb58098461071d`), including the follow-up
changes limiting the default distributions to Noble, Bookworm, and Jammy.
