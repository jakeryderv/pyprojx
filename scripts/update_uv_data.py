# /// script
# requires-python = ">=3.11"
# dependencies = ["packaging"]
# ///
"""Generate pyprojx's data about uv's configuration across releases.

For every uv release since 0.1.34, the first with a configuration schema, reads
`uv.schema.json` at the release's tag and records which releases accept and
deprecate each option. Where uv behaves differently from its schema, the script
runs uv to find out: which releases still accept options the schema dropped,
which options uv warns are deprecated, and what uv does with an unknown key or
an invalid value in each table and option. uv fails on some, but for most it
only warns and ignores the file's other settings too, or silently ignores them.

Schemas and measurements are cached, so later runs only fetch new releases.
The script needs the network, so it runs on a schedule
(.github/workflows/tool-data.yml) rather than in CI, or by hand:

    uv run scripts/update_uv_data.py
"""

from __future__ import annotations

import argparse
import concurrent.futures
import json
import re
import sys

from packaging.version import Version
from schema_history import (
    CACHE,
    ROOT,
    Option,
    Probe,
    add_aliases,
    collect,
    example,
    fetch,
    fill_gaps,
    first_where,
    generate_options,
    probe,
    releases,
    rust_string,
    setting,
)

OUTPUT = ROOT / "crates/pyprojx_core/src/uv_data.rs"
FIRST = Version("0.1.34")
# The first `uv_build` release; see crates/pyprojx_core/src/backend_data.rs.
FIRST_UV_BUILD = Version("0.6.6")
UV_CACHE = CACHE / "uv"
# The first release with `uv lock`, which reads the project fields of
# `[tool.uv]` as well as its settings.
LOCK = Version("0.3.0")
# Definitions whose schema differs from what uv accepts, found by running uv.
NAMED = {
    # `pip.group` entries are strings such as `dev` or `path:dev`.
    "PipGroupName": ("str",),
}
HEADER = '[project]\nname = "demo"\nversion = "1"\nrequires-python = ">=3.8"\n'
# A value no option accepts.
INVALID = "1979-05-27T07:32:00Z"


def schema(version: Version) -> dict | None:
    url = f"https://raw.githubusercontent.com/astral-sh/uv/{version}/uv.schema.json"
    content = fetch([url], UV_CACHE / f"{version}.schema.json")
    return None if content is None else json.loads(content)


def run(version: Version, config: str, command: list[str]) -> Probe:
    """uv's output when a release runs `command` in a project with `config`."""
    return probe(
        UV_CACHE / "probes.json",
        ["uvx", "--quiet", "--from", f"uv=={version}", "uv", *command],
        {"pyproject.toml": HEADER + config, "requirements.in": ""},
    )


def compile_settings(version: Version, config: str) -> Probe:
    """Reads the settings with `uv pip compile`, which every release has."""
    return run(
        version,
        config,
        ["pip", "compile", "requirements.in", "--offline", "--no-cache"],
    )


def lock(version: Version, config: str) -> Probe:
    """Reads the settings and the project with `uv lock`."""
    return run(version, config, ["lock", "--offline", "--no-cache"])


def build(version: Version, config: str) -> Probe:
    """Builds a wheel with the `uv_build` release, which reads the
    `[tool.uv.build-backend]` settings, through its PEP 517 interface."""
    backend = f'[build-system]\nrequires = ["uv_build=={version}"]\nbuild-backend = "uv_build"\n'
    return probe(
        UV_CACHE / "probes.json",
        ["uv", "build", "--force-pep517", "--wheel", "--no-cache", "--quiet"],
        {"pyproject.toml": HEADER + backend + config, "src/demo/__init__.py": ""},
    )


def entry(found: dict[str, Option], path: str, extra: str = "") -> str:
    """An inline table for an entry of the array of tables at `path`, with its
    required options and `extra`."""
    fields = [
        f"{other.rpartition('.')[2]} = {value(found, other)}"
        for other, option in found.items()
        if other.rpartition(".")[0] == path and option.required
    ]
    return "{ " + ", ".join([*fields, *filter(None, [extra])]) + " }"


def value(found: dict[str, Option], path: str) -> str:
    """A valid value for the option at `path`."""
    option = found[path]
    name = path.rpartition(".")[2]
    if name == "url" or name.endswith("-url"):
        return '"https://example.com/simple"'
    if name == "version":
        return '"1.0"'
    if option.kind == "TableArray":
        return f"[{entry(found, path)}]"
    return example(option.types.get(option.present[-1]), option.kind)


def treatment(outcome: Probe) -> str:
    """What uv did with a setting it cannot read."""
    if outcome.status != 0:
        return "Rejects"
    if "during settings discovery" in outcome.output:
        return "Warns"
    return "Ignores"


def backfill_build_backend(found: dict[str, Option], versions: list[Version]) -> None:
    """Records the `uv_build` releases that read each `build-backend` option
    before uv's schema describes it, measured by building with an invalid
    value, which a release that reads the option rejects."""
    for path, option in found.items():
        if not path.startswith("build-backend.") or option.kind != "Value":
            continue
        first = option.present[0]
        earlier = [i for i in range(first) if versions[i] >= FIRST_UV_BUILD]
        config = setting("tool.uv", found, path, INVALID)
        since = first_where(
            earlier, lambda i, config=config: build(versions[i], config).status != 0
        )
        if since is None:
            continue
        print(
            f"uv_build reads {path} from {versions[since]}, before uv's schema has it"
        )
        added = range(since, first)
        option.present = sorted(set(option.present) | set(added))
        for index in added:
            option.types[index] = option.types[first]
    # A table is present where any option in it is.
    for path, option in found.items():
        if path.startswith("build-backend") and option.kind == "Table":
            inside = [o for p, o in found.items() if p.startswith(path + ".")]
            option.present = sorted(
                set().union(*(o.present for o in inside), option.present)
            )


def reader(path: str):
    """How to find what the reader of the option at `path` does with it."""
    return build if path.split(".")[0] == "build-backend" else lock


def treatments(
    found: dict[str, Option], latest: Version, index: int
) -> tuple[dict[str, str], dict[str, str]]:
    """What the latest release does with an unknown key in each table, and with
    an invalid value for each option, other than rejecting it. uv ignores the
    `build-backend` settings, which `uv_build` reads, so those are measured by
    building with `uv_build`."""
    current = {p: o for p, o in found.items() if index in o.present}
    tables = [""] + [p for p, o in current.items() if o.kind in ("Table", "TableArray")]
    unknown, invalid = {}, {}
    for path in tables:
        key = "pyprojx-probe = 1"
        if path == "":
            config = f"[tool.uv]\n{key}\n"
        elif current[path].kind == "TableArray":
            config = setting("tool.uv", found, path, f"[{entry(found, path, key)}]")
        else:
            config = setting("tool.uv", found, path, f"{{ {key} }}")
        unknown[path] = treatment(reader(path)(latest, config))
    for path in current:
        config = setting("tool.uv", found, path, INVALID)
        invalid[path] = treatment(reader(path)(latest, config))
    return (
        {p: t for p, t in unknown.items() if t != "Rejects"},
        {p: t for p, t in invalid.items() if t != "Rejects"},
    )


def deprecations(found: dict[str, Option], versions: list[Version]) -> dict[str, str]:
    """Options the latest release warns are deprecated, recorded as deprecated
    from the first release that warns, with uv's advice as the message."""
    latest = len(versions) - 1
    messages = {}
    for path, option in found.items():
        if latest not in option.present or option.kind != "Value":
            continue

        def warning(index: int, path: str = path) -> str | None:
            outcome = lock(
                versions[index], setting("tool.uv", found, path, value(found, path))
            )
            name = re.escape(path.rpartition(".")[2])
            for line in outcome.output.splitlines():
                if re.search(rf"`[^`]*{name}`.*deprecated", line):
                    return line
            return None

        line = warning(latest)
        if line is None:
            continue
        candidates = [i for i in option.present if versions[i] >= LOCK]
        since = first_where(candidates, lambda index: warning(index) is not None)
        print(f"{path} is deprecated since {versions[since]}: {line}")
        option.deprecated = sorted(
            set(option.deprecated) | set(range(since, latest + 1))
        )
        advice = line.rpartition("; ")[2] if "; " in line else line
        messages[path] = advice.removeprefix("warning: ").strip()
    return messages


def source_kinds(document: dict) -> list[tuple[str, list[str]]]:
    """The kinds of `tool.uv.sources` entries, by the key each requires, such
    as `git`, with the keys each allows."""
    kinds = []
    for variant in document["definitions"]["Source"]["anyOf"]:
        [key] = variant["required"]
        if variant.get("additionalProperties") is not False:
            raise RuntimeError(f"`{key}` sources allow other keys; check them by hand")
        kinds.append((key, sorted(variant["properties"])))
    return kinds


def generate(versions, found, messages, unknown, invalid, sources) -> str:
    lines = [
        "// @generated by scripts/update_uv_data.py; do not edit.",
        "",
        "use crate::tool::{OptionData, OptionKind, Treatment, ValueType};",
        "",
    ]
    lines += generate_options("uv", versions, found, messages)
    for name, doc, entries in (
        (
            "UNKNOWN_KEYS",
            'an unknown key in each table (`""` for `[tool.uv]`)',
            unknown,
        ),
        ("INVALID_VALUES", "an invalid value for each option", invalid),
    ):
        lines += [
            f"/// What the latest release does with {doc}, where it does",
            "/// not reject it, sorted.",
            f"pub const {name}: &[(&str, Treatment)] = &[",
        ]
        lines += [
            f"    ({rust_string(path)}, Treatment::{kind}),"
            for path, kind in sorted(entries.items())
        ]
        lines += ["];", ""]
    lines += [
        "/// The kinds of `tool.uv.sources` entries in the latest release, by the key",
        "/// each requires, with the keys each allows.",
        "pub const SOURCE_KINDS: &[(&str, &[&str])] = &[",
    ]
    lines += [
        f"    ({rust_string(key)}, &[{', '.join(map(rust_string, keys))}]),"
        for key, keys in sources
    ]
    lines += ["];", ""]
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.parse_args()
    versions = releases("uv", FIRST)
    with concurrent.futures.ThreadPoolExecutor(8) as pool:
        schemas = list(pool.map(schema, versions))
    fill_gaps(versions, schemas, NAMED)
    found = collect(versions, schemas, NAMED)
    latest = len(versions) - 1

    def accepts(index: int, path: str) -> bool:
        config = setting("tool.uv", found, path, value(found, path))
        return "unknown field" not in compile_settings(versions[index], config).output

    messages = add_aliases(found, versions, accepts)
    backfill_build_backend(found, versions)
    messages |= deprecations(found, versions)
    unknown, invalid = treatments(found, versions[latest], latest)
    OUTPUT.write_text(
        generate(
            versions, found, messages, unknown, invalid, source_kinds(schemas[latest])
        ),
        encoding="utf-8",
    )
    print(f"wrote {len(found)} options across {len(versions)} releases to {OUTPUT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
