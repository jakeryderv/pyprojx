## `pyprojx`: Intelligence for pyproject.toml

### Identity

> **Version-aware intelligence for `pyproject.toml`—check, explain, and safely evolve your Python configuration.**

pyprojx aims to be a **toolkit for understanding and safely maintaining
`pyproject.toml`**, with analysis as its foundation. It should understand both
official Python standards and tool-specific configuration, explain problems in
context, and help people make deliberate improvements.

The boundary is the configuration file, not the entire Python project. Helping
express dependency requirements correctly does not mean resolving dependencies;
configuring build tools does not mean becoming a build system.

This document describes the intended direction, not implemented functionality.
Version 0.0.1 is an importable development stub; there is no analyzer or CLI yet.

Think:

> **Ruff/ty-style developer experience, but for `pyproject.toml` itself.**

### Why / Inspiration

Modern Python increasingly centralizes configuration in `pyproject.toml`, but the file combines multiple independently evolving specifications:

```text
pyproject.toml
├── Python/PyPA standards
│   ├── [project]
│   ├── [build-system]
│   └── [dependency-groups]
│
└── Tool-specific configuration
    ├── [tool.uv]
    ├── [tool.ruff]
    ├── [tool.ty]
    ├── [tool.pytest]
    └── ...
```

Knowing whether a configuration is valid often requires consulting multiple documentation sites and accounting for **different tool versions**.

The inspiration is the experience provided by tools like Ruff and ty: fast diagnostics, useful errors, autofixes, and strong editor integration.

### Progression: understand → improve → author

1. **Understand:** parse, validate, explain, and diagnose configuration in the
   context of the project's tool versions.
2. **Improve:** offer formatting, useful fixes, and eventually migrations where
   the intended change can be established safely.
3. **Author:** explore guided editing and configuration generation using the same
   knowledge, when concrete workflows justify them.

This is a direction, not a commitment to implement every authoring feature.
Generation requires explicit choices and defaults; schemas alone cannot determine
what a project should configure.

The first slice remains deliberately narrow: `pyprojx check` parses one file,
reports TOML syntax errors with useful locations, and returns clear exit codes.
Schemas, version detection, formatting, edits, and LSP support come later.

### Analysis-first foundation

The tool would:

* Parse and validate TOML syntax.
* Validate standardized Python/PyPA sections.
* Validate `[tool.*]` sections against tool-specific schemas.
* Detect the project's actual/locked tool versions when possible.
* Perform **version-aware validation**.
* Detect unknown/misspelled settings and suggest corrections.
* Validate types, enums, required properties, and incompatible settings.
* Lint redundant, deprecated, suspicious, or conflicting configuration.
* Format `pyproject.toml` consistently.
* Provide an LSP for autocomplete, hover documentation, diagnostics, and quick fixes.
* Expose a CLI suitable for local development and CI.

Planned CLI example (not implemented yet):

```text
$ pyprojx check

tool.ruff.line-lenght
                 ^^^^^
Unknown setting `line-lenght`.
Did you mean `line-length`?

Configured Ruff: 0.x
```

### Planned scope

```text
✓ TOML parsing
✓ formatting
✓ Python/PyPA schema validation
✓ [tool.*] schema validation
✓ version-aware configuration
✓ configuration linting
✓ deprecation detection
✓ helpful diagnostics/autofixes
✓ CLI
✓ language server / editor integration
✓ CI usage
```

### Possible later directions

- Guided configuration edits with validation and explanations.
- Migrations away from deprecated settings or between supported tool versions.
- Generating configuration from explicit project requirements and selected tools.

These are opportunities enabled by the analysis core, not promised features.
Their scope should follow actual user needs rather than expand into general
Python project management.

### What safe maintenance means

- Analysis is read-only; changes require an explicit editing or formatting action.
- Changes are reviewable, with a clear explanation of their purpose.
- Targeted edits preserve comments and unrelated settings, and keep diffs small.
- Formatting preserves meaning and comments; it is a distinct, explicit operation.
- Ambiguity is surfaced for a decision, not silently resolved by guessing.
- Version-specific advice distinguishes detected facts, assumptions, and unknowns.

Understanding configuration is not enough to edit it safely. Editing features
will need an appropriate comment-preserving representation and validation of the
proposed result; the initial parser choice does not settle that architecture.

### Out of scope

```text
✗ Python source linting → Ruff
✗ Python type checking → ty/Pyright
✗ dependency resolution → uv/pip
✗ environment management → uv
✗ package building/publishing → uv/build tools
✗ replacing the tools being configured
```

The goal is **version-aware intelligence for understanding and safely maintaining
`pyproject.toml`**, delivered through analysis, a CLI, and editor tooling. It
complements tools like uv, Ruff, and ty rather than replacing them.

