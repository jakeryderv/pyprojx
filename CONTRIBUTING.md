# Contributing to pyprojx

pyprojx is at the development-stub stage. Start with the
[project vision](docs/vision.md) for its intended scope; planned features are not
yet implemented. The planned top-level command is `pyprojx`; there is no CLI yet.

## Proposing changes

For substantial features or design changes, open an
[issue](https://github.com/jakeryderv/pyprojx/issues) to discuss the problem and
approach before implementation. Small documentation fixes can go straight to a
pull request.

Keep changes focused, describe what changed and why, and include the checks you
ran in your pull request. Add meaningful tests when introducing executable
behavior, and distinguish implemented features from plans in documentation.

## Development setup

Requires Python 3.11+ and [uv](https://docs.astral.sh/uv/getting-started/installation/).
Workflows use uv 0.12.18. `.python-version` selects Python 3.11 for development;
CI tests Python 3.11, 3.12, 3.13, and 3.14.

```sh
git clone https://github.com/jakeryderv/pyprojx.git
cd pyprojx
uv sync --locked
uv run --locked pre-commit install
```

The package lives in `src/pyprojx/`, and tests live in `tests/`. Development
tools are in the `dev` dependency group and locked in `uv.lock`; do not install
separate tool versions to work on this repository.

## Local checks

Run these from the repository root:

```sh
uv run --locked ruff check .
uv run --locked ruff format --check .
uv run --locked ty check
uv run --locked pytest
uv run --locked pre-commit validate-config
uv build --no-sources
uvx --from twine==7.0.0 twine check --strict dist/*
```

`uv sync --locked` fails if `pyproject.toml` and `uv.lock` disagree. Use
`uv add --dev <tool>` for intentional development dependency changes and commit
both files. Run `uv lock` after other changes that affect dependency resolution.

Pre-commit runs only Ruff lint/fixes and formatting on staged Python files,
using the uv-locked versions. If it changes files, review and stage the fixes
before committing again. To check all tracked files:

```sh
uv run --locked pre-commit run --all-files
```

Type checks and tests are deliberately not commit hooks; run them locally before
pushing. The current tests cover installed package metadata and the typing marker,
not the planned analyzer or CLI.

### Check built distributions

Start with a clean `dist/` directory so old releases are not tested accidentally.
After building, smoke-test both the wheel and source distribution in isolated
environments (POSIX shell):

```sh
EXPECTED_VERSION="$(uv version --short)"
export EXPECTED_VERSION
for artifact in dist/*.whl dist/*.tar.gz; do
  uv run --isolated --no-project --with "$artifact" \
    python -I -c \
    'import os, pyprojx; from importlib.metadata import version; from importlib.resources import files; assert version("pyprojx") == os.environ["EXPECTED_VERSION"]; assert files(pyprojx).joinpath("py.typed").is_file(); print(pyprojx.__file__)'
done
```

CI runs the same checks on pull requests and pushes to `main`. Its stable `CI`
status requires quality checks, all supported Python test jobs, and distribution
validation to pass. Generated artifacts, virtual environments, credentials, and
local agent state must not be committed.

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

Dependabot checks uv dependencies and GitHub Actions weekly, grouping development
dependencies and action updates separately, with a seven-day cooldown for new
versions. Review its changes and let CI validate them rather than automatically
merging them.

## Releases

Release Please proposes version and changelog updates; merging a reviewed release
PR authorizes publication. See the [publishing guide](docs/publishing.md) for the
versioning policy, GitHub App setup, and failure recovery.
