# Contributing to pyprojx

pyprojx is in early development. Start with the
[project vision](docs/vision.md) for its intended scope and
[decision 0001](docs/decisions/0001-rust-core.md) for why it is written in Rust.
`pyprojx check` currently reports TOML syntax, encoding, and TOML 1.1 compatibility
problems; most planned
features are not yet implemented.

## Proposing changes

For substantial features or design changes, open an
[issue](https://github.com/jakeryderv/pyprojx/issues) to discuss the problem and
approach before implementation. Small documentation fixes can go straight to a
pull request.

Keep changes focused, describe what changed and why, and include the checks you
ran in your pull request. Add meaningful tests when introducing executable
behavior, and distinguish implemented features from plans in documentation.

## Development setup

Requires [rustup](https://rustup.rs/) and
[uv](https://docs.astral.sh/uv/getting-started/installation/). `rust-toolchain.toml`
pins the Rust toolchain (with rustfmt and clippy), and rustup installs it on first
use. Workflows use uv 0.12.18, which manages the Python development tools.

```sh
git clone https://github.com/jakeryderv/pyprojx.git
cd pyprojx
rustup toolchain install
uv sync --locked
uv run --locked pre-commit install
```

The code is a Cargo workspace:

- `crates/pyprojx_core/`: the analysis library. Keep CLI concerns such as
  argument parsing and output rendering out of it.
- `crates/pyprojx/`: the `pyprojx` command-line binary, a thin layer over the
  core. CLI integration tests live in `crates/pyprojx/tests/`.

`pyproject.toml` configures the maturin build that packages the binary as Python
wheels. `uv sync` installs only development tools (pre-commit); it does not build
pyprojx. Development tools are locked in `uv.lock` and Rust dependencies in
`Cargo.lock`; do not install separate tool versions to work on this repository.

## Local checks

Run these from the repository root:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
uv lock --check
uv run --locked pre-commit validate-config
```

Use `cargo run -- <args>` to try the CLI, for example `cargo run -- --version`.
Use `cargo add` for intentional Rust dependency changes and `uv add --dev <tool>`
for development tools, and commit the updated lockfiles.

Pre-commit runs only `cargo fmt` when Rust files are staged. If it changes files,
review and stage the fixes before committing again. To run it on all files:

```sh
uv run --locked pre-commit run --all-files
```

Clippy and tests are deliberately not commit hooks; run them locally before
pushing.

### Snapshot tests

CLI tests in `crates/pyprojx/tests/` compare command output with
[insta](https://insta.rs/) snapshots in `crates/pyprojx/tests/snapshots/`. After
an intentional output change, regenerate and review them:

```sh
INSTA_UPDATE=always cargo test --workspace
git diff crates/pyprojx/tests/snapshots/
```

[`cargo-insta`](https://insta.rs/docs/cli/) offers an interactive alternative
(`cargo insta review`). Commit reviewed snapshots with the change. CI never
writes snapshots, so a mismatch fails the tests.

### Real-world corpus

`crates/pyprojx_core/tests/corpus/` holds unmodified `pyproject.toml` files from
well-known projects, listed with their source commits in `SOURCES.md`. A snapshot
records every diagnostic pyprojx reports on them, and errors fail the test: these
projects build, so an error is almost certainly a false positive. Review changes
to that snapshot carefully when adding or changing checks.

### Check built distributions

pyprojx is distributed as platform-specific wheels containing the binary, plus a
source distribution that builds it with Rust. Start with a clean `dist/`
directory, then build, check, and smoke-test both for your platform (POSIX
shell):

```sh
uvx --from maturin==1.15.0 maturin build --release --locked --out dist
uvx --from maturin==1.15.0 maturin sdist --out dist
uvx --from twine==7.0.0 twine check --strict dist/*
for artifact in dist/*.whl dist/*.tar.gz; do
  uv tool run --isolated --no-cache --from "./$artifact" pyprojx --version
done
```

Local wheels are tagged for your machine's C library and are not suitable for
publishing; CI builds the portable wheels that are released.

CI runs the checks and tests on Linux, macOS, and Windows, and builds and
smoke-tests wheels for Linux (x86_64, aarch64), macOS (x86_64, arm64), and
Windows (x86_64), plus the source distribution. Its stable `CI` status requires
all of them to pass. Build output, virtual environments, credentials, and local
agent state must not be committed.

## Pull requests and commits

Use a [Conventional Commit](https://www.conventionalcommits.org/) PR title:

- `feat: validate project metadata`
- `fix: handle an empty configuration`
- `docs: clarify configuration support`
- `chore(deps): update development dependencies`

Allowed types are `feat`, `fix`, `perf`, `refactor`, `docs`, `style`, `test`,
`build`, `ci`, `chore`, and `revert`. An optional scope and `!` for a breaking
change are supported, for example `feat(cli)!: change diagnostic output`.
Explain breaking changes in the PR description as well.

PRs are squash-merged using the PR title as the commit subject. Individual
work-in-progress commits do not need conventional titles. Before merging, the
`CI` and `PR title` checks must pass; no additional reviewer is required for the
solo-maintainer workflow. Admin bypass is disabled. Keep the branch up to date
with `main` before merging.

Dependabot checks Cargo dependencies, uv development dependencies, and GitHub
Actions weekly, grouping each ecosystem's updates, with a seven-day cooldown for
new versions. Review its changes and let CI validate them rather than automatically
merging them.

## Releases

Release Please proposes version and changelog updates; merging a reviewed release
PR authorizes publication. See the [publishing guide](docs/publishing.md) for the
versioning policy, GitHub App setup, and failure recovery.
