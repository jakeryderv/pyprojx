# Publishing pyprojx

This guide is for maintainers. See [Contributing](../CONTRIBUTING.md) for local
checks and distribution validation.

## Release flow

1. Merge a feature or fix PR into `main` with a Conventional Commit title.
2. Release Please opens or updates a release PR containing the proposed version,
   `CHANGELOG.md`, and `.release-please-manifest.json` updates. It updates the
   workspace version in `Cargo.toml` and the pyprojx crate versions in
   `Cargo.lock` together; the wheel version is read from `Cargo.toml`.
3. Review the release PR, including the changelog and version. Required CI must
   pass before merging. Check that the README accurately describes the new version.
4. Squash-merge the release PR. Release Please creates the version tag and GitHub
   Release on its next run on `main`.
5. The published GitHub Release triggers `publish.yml`, which validates the released
   commit and publishes its tested distributions to PyPI.

**Merging a release PR authorizes publication.** There is no additional `pypi`
environment approval. Tags and GitHub Releases are automated once the GitHub App
below is configured; ordinary tag pushes no longer trigger PyPI publication.

## Versioning policy

The manifest starts at the existing `0.0.1` release, which was published from the
`v0.0.1` tag before this automation existed and has no GitHub Release. The bootstrap
commit is that tag's commit, so earlier history is not included in the next release
notes.

While the project is below `1.0.0`:

- `fix` and `perf` changes produce patch releases, such as `0.0.1` → `0.0.2`.
- `feat` changes produce minor releases, such as `0.0.1` → `0.1.0`.
- Breaking changes marked with `!` or a `BREAKING CHANGE` footer produce minor
  releases rather than `1.0.0` automatically.
- Documentation, CI, tests, and tooling-only commits do not independently trigger
  a release unless marked as breaking changes. Use `chore(deps)` for development
  dependency updates; user-facing dependency fixes should use `fix(deps)`.
  Cargo dependencies are compiled into the binary, so retitle a Dependabot Cargo
  update to `fix(deps)` when it fixes something users would notice.

Moving to `1.0.0` is an explicit maintainer decision. Configure a deliberate
release override only after agreeing that milestone; remove the override after
use. Do not hand-edit the version on ordinary feature PRs.

`release-please-config.json` defines this policy. It uses the `simple` release
type, because Release Please's `rust` type does not support a virtual workspace
whose members inherit `workspace.package.version`. Two TOML extra-file updaters
change only `workspace.package.version` in `Cargo.toml` and the `pyprojx` and
`pyprojx_core` versions in `Cargo.lock`, not dependency versions. The
`Cargo.lock` JSONPath handles both plain names and the tagged `name.value`
representation used by the pinned Release Please TOML parser. Recheck the
updaters when upgrading Release Please or adding a crate; a simulated version
bump should change only those version lines and still pass `cargo check --locked`.
CI's `--locked` Cargo commands check that the updated lockfile remains consistent.

## One-time GitHub App setup

Release Please uses a GitHub App rather than the built-in `GITHUB_TOKEN` so its
PRs and releases trigger other workflows normally. The App is only for GitHub
operations; PyPI authentication remains OIDC.

1. Create a GitHub App under your account's **Settings → Developer settings →
   GitHub Apps**, or reuse an existing App configured as below. Choose a unique
   name, disable webhooks, and restrict installation to your account. No callback
   URL or user authorization flow is needed.
2. Grant repository permissions **Contents: Read and write** and **Pull requests:
   Read and write**. Metadata read access is implicit; no organization or account
   permissions are needed.
3. Install it on selected repositories only, including **`jakeryderv/pyprojx`**.
   The workflow's token is scoped to this repository regardless of the
   installation's other repositories.
4. Generate a private key. Add its PEM contents as the repository Actions secret
   **`RELEASE_PLEASE_APP_PRIVATE_KEY`** using GitHub's secret UI or secure stdin.
   Never commit it or paste it into an issue/chat.
5. Add the App's **Client ID** (not its numeric App ID) as the repository Actions
   variable **`RELEASE_PLEASE_APP_CLIENT_ID`**. Set this last: its presence enables
   the workflow.
6. Run **Release Please → Run workflow** on `main`, or let the next merge trigger
   it. With only documentation/tooling commits, no release PR is expected.

Until the Client ID variable exists, Release Please deliberately skips mutations and
writes a setup reminder in the workflow summary. Once enabled, missing or invalid
credentials fail visibly; there is no fallback to `GITHUB_TOKEN`.

The installation token is short-lived, scoped to this repository and those two
permissions, and revoked by the token action after the job. The App does not need
branch-protection bypass: it opens release PRs, which the maintainer merges after
checks pass. No automatic PR merging is configured.

## PyPI trusted publishing

The existing trusted-publisher identity stays unchanged:

- Project: [`pyprojx`](https://pypi.org/project/pyprojx/)
- GitHub repository: [`jakeryderv/pyprojx`](https://github.com/jakeryderv/pyprojx)
- Workflow filename: `publish.yml`
- GitHub environment: `pypi`

`publish.yml` responds to published, non-prerelease GitHub Releases. It calls the
same `ci.yml` used for pull requests, using the release event's commit SHA rather
than the current branch tip. It requires:

- A stable `vMAJOR.MINOR.PATCH` tag matching the package metadata.
- The released commit to be in `main`'s history.
- Locked dependencies, `cargo fmt`, clippy, and tests on Linux, macOS, and
  Windows to pass.
- Wheels for every supported platform and the sdist to build, pass strict
  metadata checks, and report the expected `pyprojx --version` when installed in
  isolation.

Wheels are built natively on each platform's runner by maturin: Linux x86_64
and aarch64 (manylinux), macOS x86_64 and arm64, and Windows x86_64. Other
platforms, such as musl Linux or Windows on Arm, fall back to the sdist and need
a Rust toolchain until wheels are added for them.

Only after validation does a separate job download the same distribution
artifacts and run `uv publish --trusted-publishing always`. That job alone has
`id-token: write`; no stored PyPI token or source checkout is used there. Ordinary
CI has read-only repository permissions and no publishing credentials.

GitHub Actions are pinned to commit SHAs. Dependabot proposes action updates;
the uv executable, maturin, and the release metadata checker are explicitly
versioned in workflows, and the Rust toolchain in `rust-toolchain.toml`; review
those separately when updating tooling.

## Verification and recovery

Check the [Actions run](https://github.com/jakeryderv/pyprojx/actions) and the
[PyPI project](https://pypi.org/project/pyprojx/) for the expected version, wheel,
and source distribution. Compare published artifact hashes with the validated
artifacts and test installation from PyPI in an isolated environment.

- **Release PR checks fail:** fix the PR/configuration before merging. Do not
  bypass checks or relax permissions to make a release proceed.
- **GitHub Release exists but publishing failed before upload:** diagnose the run,
  then rerun it once the external issue is fixed. A code fix needs a new reviewed
  commit and release; rerunning an old run does not change its source revision.
- **Upload outcome is ambiguous or partial:** inspect PyPI's existing filenames
  and hashes before retrying. uv checks existing files on retry; do not bypass
  mismatched-hash errors. Never overwrite a published version with new contents,
  delete and recreate a release to retry, or move its tag.
- **Release Please has no credentials:** complete the App setup above. Keep PyPI
  token secrets out of this process.

Version `0.0.1` is already published. Setting up this automation does not require
another release. A stub publication does not guarantee permanent name ownership.
