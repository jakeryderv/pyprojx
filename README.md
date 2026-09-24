# pyprojx

Version-aware intelligence for `pyproject.toml`—check, explain, and safely evolve
your Python configuration.

Python project configuration brings together packaging standards and settings for
tools that evolve independently. pyprojx aims to help you understand and safely
maintain that configuration for the versions your project actually uses, with
analysis as the foundation for useful explanations and reviewable improvements.

## Planned capabilities

- Validate Python/PyPA sections and tool-specific configuration, starting with
  uv, Ruff, and ty.
- Explain configuration issues, including unknown settings, deprecated options,
  and conflicts, with useful diagnostics and suggested fixes.
- Provide formatting, a `pyprojx` CLI, and language-server features such as
  completion, hover documentation, and quick fixes.

Guided editing, configuration migrations, and generation are possible future
directions built on that foundation—not committed features or current capabilities.

The focus is `pyproject.toml`, not general project management. pyprojx is intended
to complement tools like uv, Ruff, and ty—not replace package management, Python
source linting, or type checking.

## Current status

**pyprojx is in early development.** `pyprojx check` reports TOML syntax errors,
invalid UTF-8, and byte order marks in `pyproject.toml`, warns about TOML 1.1
syntax that `tomllib` in Python 3.14 and earlier rejects, and validates
`[build-system]`; the other capabilities above are not implemented yet. The
[0.0.1 release](https://pypi.org/project/pyprojx/0.0.1/) on PyPI is a
placeholder that predates the command.

```text
$ pyprojx check
error[invalid-toml]: missing value
 --> pyproject.toml:2:14
  |
2 | line-length =
  |              ^

Found 1 error.
```

`pyprojx check` checks the nearest `pyproject.toml` in the current directory or
its parents, or the file or directory you pass. It exits with 0 when there are no
errors (warnings do not fail the check), 1 when there are, and 2 when the file
cannot be checked.

pyprojx is written in Rust and will be distributed on PyPI as prebuilt binaries,
like Ruff and uv, so no Rust toolchain is needed to install it. Once released,
run it with `uvx pyprojx` or install it with `uv tool install pyprojx`.

## Learn more and contribute

- [Project vision](https://github.com/jakeryderv/pyprojx/blob/main/docs/vision.md):
  intended capabilities, inspiration, and scope.
- [Contributing](https://github.com/jakeryderv/pyprojx/blob/main/CONTRIBUTING.md):
  development setup, validation, and proposing changes.
- [Publishing guide](https://github.com/jakeryderv/pyprojx/blob/main/docs/publishing.md):
  maintainer release instructions and current automation limits.

## License

[MIT](https://github.com/jakeryderv/pyprojx/blob/main/LICENSE), copyright 2026
Jake Van Slyke.
