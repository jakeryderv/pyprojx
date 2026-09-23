# 0001: Implement pyprojx in Rust and distribute it through PyPI

- Status: Accepted
- Date: 2026-09-23

## Context

The first slice needs source locations for valid keys and values, not only for
syntax errors. Later work needs more: targeted fixes, structural edits such as
moving settings between tables, formatting, and a language server that keeps
working in the presence of errors. Those require a lossless, comment-preserving
representation of `pyproject.toml`.

Findings that informed the decision:

- `tomllib` exposes no source positions. `tomlkit` 0.15.1 reports positions only
  for syntax errors, not for parsed items.
- `tomllib` on Python 3.11–3.14 rejects TOML 1.1 syntax (for example, multi-line
  inline tables); Python 3.15 and tomli 2.4 accept it.
- A tomli fork recording key, value, and header spans was prototyped in about 16
  changed lines and passed tomli's test suite, but it stops at the first error
  and cannot support structural edits.
- tree-sitter-toml provides spans and error recovery, but reports errors as bare
  `ERROR` nodes, parses only TOML 1.0, and misses semantic errors such as
  duplicate keys.
- The toml-rs crates are what uv and Ruff use to parse TOML (`toml`,
  `toml_parser`, and in uv `toml_edit`). `toml_edit` is format-preserving and
  exposes spans on keys, items, tables, and arrays; `toml_parser` supports error
  recovery; both support TOML 1.1.

## Decision

Implement pyprojx as a Rust tool, following the model of Ruff, ty, and uv:

- A Cargo workspace with a library crate, `pyprojx_core`, holding parsing,
  diagnostics, schemas, and version detection, and a binary crate, `pyprojx`,
  holding the CLI (and later the language server) as a thin layer over the core.
- Build on the toml-rs crates rather than writing or forking a TOML parser.
- Distribute the binary on PyPI as platform wheels built by maturin, so
  `uvx pyprojx`, `uv tool install pyprojx`, and `uv add --dev pyprojx` work
  without a Rust toolchain.
- Declare `requires-python = ">=3.8"`, like uv and ty. The wheels contain no
  Python code; a low floor lets projects that support older Python versions add
  pyprojx as a development dependency. This is separate from which Python
  versions pyprojx can analyze.

Python bindings are not planned. Keeping CLI concerns such as argument parsing
and output rendering out of `pyprojx_core` keeps them possible: a future
`pyprojx_python` crate could expose the core with typed Python stubs without
restructuring.

## Alternatives considered

- **Python with a tomli fork:** fastest start and pure Python, but a dead end for
  structural edits and error recovery, so a second parser would be needed later.
- **Python with a custom lossless parser:** full control, but pyprojx would own a
  parser and its conformance testing.
- **Python with a Rust parser extension:** keeps Python for checks and the CLI,
  but pays the Rust build and release cost while keeping Python's startup and
  performance, and adds a Python–Rust boundary to design.
- **tree-sitter:** see the findings above.

## Consequences

- TOML handling follows the same parser family as uv and Ruff. Exact behavior
  still depends on crate versions: at the time of writing, uv uses `toml` 1.1
  and Ruff uses `toml` 1.0. Version-aware checks must map tool releases to the
  parser versions they shipped.
- The build moves from `uv_build` to maturin, with a wheel per platform. CI,
  release, and publishing workflows grow accordingly, and contributors need a
  Rust toolchain.
- Rust tooling (`cargo fmt`, `clippy`, `cargo test`) replaces Ruff, ty, and
  pytest for project code.
- pyprojx is not importable from Python unless bindings are added later.
