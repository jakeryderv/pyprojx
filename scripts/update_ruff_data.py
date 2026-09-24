# /// script
# requires-python = ">=3.11"
# dependencies = ["packaging"]
# ///
"""Generate pyprojx's data about Ruff's configuration across releases.

For every Ruff release since 0.1.0, reads the JSON schema of its configuration
(`ruff.schema.json` at the release's tag) and records which releases accept
each option and which deprecate it. Deprecation messages come from the latest
release (`ruff config --output-format json`).

Schemas are cached, so later runs only download new releases. The script needs
the network, so it runs by hand rather than in CI:

    uv run scripts/update_ruff_data.py
"""

from __future__ import annotations

import argparse
import concurrent.futures
import json
import os
import re
import subprocess
import sys
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path

from packaging.version import InvalidVersion, Version

OUTPUT = Path(__file__).resolve().parents[1] / "crates/pyprojx_core/src/ruff_data.rs"
FIRST = Version("0.1.0")
CACHE = Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache")) / "pyprojx/ruff"


def releases() -> list[Version]:
    """Final, unyanked releases since `FIRST`, oldest first."""
    with urllib.request.urlopen("https://pypi.org/pypi/ruff/json") as response:
        data = json.load(response)
    found = []
    for raw, files in data["releases"].items():
        try:
            version = Version(raw)
        except InvalidVersion:
            continue
        if version.is_prerelease or version.is_devrelease or version < FIRST:
            continue
        if files and not all(file["yanked"] for file in files):
            found.append(version)
    return sorted(found)


def schema(version: Version) -> dict | None:
    """The configuration schema at the release's tag, which some releases prefix
    with `v`, or `None` if the release has no tag."""
    path = CACHE / f"{version}.schema.json"
    if not path.exists():
        for tag in (str(version), f"v{version}"):
            url = f"https://raw.githubusercontent.com/astral-sh/ruff/{tag}/ruff.schema.json"
            try:
                with urllib.request.urlopen(url) as response:
                    content = response.read()
                break
            except urllib.error.HTTPError as error:
                if error.code != 404:
                    raise
        else:
            return None
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
    return json.loads(path.read_text(encoding="utf-8"))


def options(document: dict) -> dict[str, tuple[str, bool]]:
    """Every option path, such as `lint.isort.known-first-party`, with its kind and
    whether it is deprecated."""
    definitions = document.get("definitions", {})

    def resolve(node: dict) -> dict:
        while True:
            if "$ref" in node:
                node = definitions[node["$ref"].rsplit("/", 1)[1]]
                continue
            choices = node.get("anyOf") or node.get("oneOf") or node.get("allOf")
            if choices:
                non_null = [c for c in choices if c.get("type") != "null"]
                if len(non_null) == 1:
                    node = non_null[0]
                    continue
            return node

    found: dict[str, tuple[str, bool]] = {}

    def walk(node: dict, prefix: str) -> None:
        for key, prop in node.get("properties", {}).items():
            path = prefix + key
            target = resolve(prop)
            deprecated = bool(prop.get("deprecated") or target.get("deprecated"))
            if isinstance(target.get("additionalProperties"), dict):
                # Keys are names the user chooses, such as file patterns, even
                # if the schema lists some.
                found[path] = ("Map", deprecated)
            elif "properties" in target:
                found[path] = ("Table", deprecated)
                walk(target, path + ".")
            else:
                found[path] = ("Value", deprecated)

    walk(document, "")
    return found


def ranges(indexes: list[int]) -> list[tuple[int, int]]:
    """Consecutive runs of release indexes, as inclusive (first, last) pairs."""
    runs: list[tuple[int, int]] = []
    for index in indexes:
        if runs and runs[-1][1] == index - 1:
            runs[-1] = (runs[-1][0], index)
        else:
            runs.append((index, index))
    return runs


def ruff_config(version: Version) -> dict:
    """The options a release reports with `ruff config` (from Ruff 0.5)."""
    output = subprocess.run(
        ["uvx", "--quiet", f"ruff@{version}", "config", "--output-format", "json"],
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    return json.loads(output)


def same_options(a: Version, b: Version) -> bool:
    """Whether two releases report the same options and deprecations."""

    def summary(version: Version) -> dict[str, bool]:
        return {
            path: bool(o.get("deprecated")) for path, o in ruff_config(version).items()
        }

    return summary(a) == summary(b)


def deprecation_messages(latest: Version) -> dict[str, str]:
    """Deprecation messages of the latest release's options, without Markdown links."""
    messages = {}
    for path, option in ruff_config(latest).items():
        deprecated = option.get("deprecated")
        if deprecated and deprecated.get("message"):
            message = re.sub(r"\[([^\]]+)\]\([^)]*\)", r"\1", deprecated["message"])
            messages[path] = " ".join(message.split())
    return messages


@dataclass
class Option:
    kind: str
    present: list[int] = field(default_factory=list)
    deprecated: list[int] = field(default_factory=list)


def rust_string(value: str) -> str:
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def rust_ranges(runs: list[tuple[int, int]]) -> str:
    return "&[" + ", ".join(f"({a}, {b})" for a, b in runs) + "]"


def generate(
    versions: list[Version], found: dict[str, Option], messages: dict[str, str]
) -> str:
    lines = [
        "// @generated by scripts/update_ruff_data.py; do not edit.",
        "",
        "use crate::ruff::{OptionData, OptionKind};",
        "",
        "/// Ruff releases, oldest first. Other data refers to them by index.",
        "pub const RELEASES: &[&str] = &[",
    ]
    lines += [f"    {rust_string(str(version))}," for version in versions]
    lines += [
        "];",
        "",
        "/// Options by path, sorted.",
        "pub const OPTIONS: &[OptionData] = &[",
    ]
    for path in sorted(found):
        option = found[path]
        message = f"Some({rust_string(messages[path])})" if path in messages else "None"
        lines.append(
            f"    OptionData {{ path: {rust_string(path)}, kind: OptionKind::{option.kind}, "
            f"present: {rust_ranges(ranges(option.present))}, "
            f"deprecated: {rust_ranges(ranges(option.deprecated))}, message: {message} }},"
        )
    lines += ["];", ""]
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.parse_args()
    versions = releases()
    with concurrent.futures.ThreadPoolExecutor(8) as pool:
        schemas = list(pool.map(schema, versions))
    # A few releases have no tag. Use the previous release's schema if the two
    # report the same options.
    for index, document in enumerate(schemas):
        if document is None:
            previous = versions[index - 1]
            if index == 0 or not same_options(previous, versions[index]):
                print(f"no schema for Ruff {versions[index]}", file=sys.stderr)
                return 1
            print(f"Ruff {versions[index]} has no tag; using {previous}'s schema")
            schemas[index] = schemas[index - 1]

    found: dict[str, Option] = {}
    for index, document in enumerate(schemas):
        for path, (kind, deprecated) in options(document).items():
            option = found.setdefault(path, Option(kind))
            if option.kind != kind:
                print(
                    f"{path} changes kind in {versions[index]}; using {kind}",
                    file=sys.stderr,
                )
                option.kind = kind
            option.present.append(index)
            if deprecated:
                option.deprecated.append(index)

    messages = deprecation_messages(versions[-1])
    OUTPUT.write_text(generate(versions, found, messages), encoding="utf-8")
    print(f"wrote {len(found)} options across {len(versions)} releases to {OUTPUT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
