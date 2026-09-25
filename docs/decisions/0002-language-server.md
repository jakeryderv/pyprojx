# 0002: Add a language server as `pyprojx server`

- Status: Accepted
- Date: 2026-09-25

## Context

The vision promises a Ruff- and ty-style experience for `pyproject.toml`,
including editor integration. `pyprojx check` already produces what an editor
needs: diagnostics with exact spans, labels, help, and notes about the tool
versions checked against, and fixes marked safe or unsafe that edit ranges of
the file. A language server would show these as you type and offer fixes as
quick fixes.

Findings that inform the decision:

- Ruff and ty ship their language servers as a subcommand of the same binary
  (`ruff server`, `ty server`), built on `lsp-server` (0.10, the synchronous
  transport from rust-analyzer: a stdio connection over `crossbeam` channels)
  and `gen-lsp-types` (0.11, types generated from the protocol's official
  metamodel). Ruff moved to `gen-lsp-types` from `lsp-types`, whose last release
  (0.97) is from 2024.
- `tower-lsp` is unmaintained since 2023; its fork `tower-lsp-server` (0.23) is
  active. Both are asynchronous and require `tokio`.
- Editors pick servers per language, not per file name: a server registered for
  TOML also receives `Cargo.toml` and other TOML files, so the server must
  decide which files it checks.
- Editors that already understand TOML, such as VS Code with Even Better TOML
  (taplo), validate `pyproject.toml` against the SchemaStore schema, which
  includes the latest schemas of Ruff, uv, and ty. That validation is not
  version-aware and has no fixes, but it overlaps with pyprojx's diagnostics.
- The protocol counts positions in UTF-16 code units by default; clients may
  offer UTF-8 (`general.positionEncodings`). pyprojx's spans are byte offsets.
- Tool versions come from lock files on disk (`uv.lock`, `pylock.toml`), which
  `pyprojx check` finds next to the file or at the workspace root.

## Decision

Add a language server, started with `pyprojx server`, communicating over stdio.

- **Crates.** A library crate, `pyprojx_server`, holds the server; the
  `pyprojx` binary adds the subcommand. `pyprojx_core` stays free of protocol
  concerns. Finding lock files moves from the CLI into `pyprojx_core` (a
  `lock::find` function using `std::fs`), so the CLI and the server share it;
  checking itself keeps taking the lock as input.
- **Protocol libraries.** `lsp-server` and `gen-lsp-types`, as in Ruff and ty.
  Checking a file takes milliseconds, so the server handles messages on one
  thread without an async runtime.
- **Documents.** The server checks open documents named `pyproject.toml` and
  ignores other files. It uses the editor's text, not the file on disk, with the
  lock file found from the document's path, cached by modification time.
- **First version's features:**
  - Full text synchronization; diagnostics published on open and change, and
    cleared on close.
  - Diagnostics carry the rule name as their code, `pyprojx` as their source,
    labels as related information, and help and notes in the message.
  - Quick fixes (`quickfix` code actions) from each diagnostic's fix. Safe fixes
    are marked preferred. Unsafe fixes are offered too, since applying one in an
    editor is an explicit, visible choice.
  - A `source.fixAll.pyprojx` code action applying all safe fixes, so editors
    can fix on save.
  - UTF-16 positions, and UTF-8 when the client offers it.
- **Not in the first version:** hover documentation, completion, and
  formatting. Hover and completion need the options' descriptions from the
  tool schemas, which the data does not keep today and which would make it
  much larger; that is a separate decision.
- **Editors.** Document setup for editors that can run any language server:
  Neovim, Helix, and Emacs (Eglot). A VS Code extension, which needs its own
  repository, packaging of the binary, and publishing to the marketplace, is a
  separate decision once the server has settled.
- **Tests.** In-process tests drive the server over `lsp-server`'s in-memory
  connection and snapshot what it publishes and offers: diagnostics, positions
  with non-ASCII text, code actions, and fix-all.

## Alternatives considered

- **`tower-lsp-server`:** a mature async framework, but `tokio` and async
  handlers buy nothing for millisecond checks, and it differs from what Ruff
  and ty use, whose code is the best reference for this server.
- **`lsp-types`:** widely used, but unreleased since 2024 and behind the
  protocol's current version.
- **A separate `pyprojx-lsp` binary:** would need its own wheels and releases,
  and could disagree with the CLI's version. A subcommand keeps one install and
  one version, and adds little to the binary.
- **Contributing pyprojx's knowledge to SchemaStore's `pyproject.toml` schema:**
  would improve existing TOML tooling for everyone, but a JSON schema cannot
  express version-aware checks, references between fields, or fixes. It could
  complement the server later.
- **A VS Code extension first:** reaches the most users, but couples the first
  release to extension packaging and marketplace publishing before the server's
  behavior has settled.

## Consequences

- New dependencies for the binary: `lsp-server`, `gen-lsp-types`,
  `crossbeam-channel`, and `serde_json`, which the protocol needs.
- `gen-lsp-types` is maintained mostly by one person. Ruff and ty depend on it,
  which lowers the risk; if it stalls, the types can be vendored.
- In editors that also validate `pyproject.toml` against SchemaStore, some
  problems, such as unknown keys, are reported twice. pyprojx's diagnostics add
  versions and fixes; users can turn off schema validation for the file.
- Each change re-checks the whole document, which is fast at the sizes
  `pyproject.toml` files have; incremental checking is not needed.
- Fix-all through the editor applies the same safe fixes as `pyprojx check
  --fix`, so the safety policy in CONTRIBUTING applies unchanged.
