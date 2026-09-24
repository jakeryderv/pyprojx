"""Shared code for generating pyprojx's data about tool configuration across
releases, such as Ruff's `[tool.ruff]`, from each release's JSON schema.

Imported by the `update_*_data.py` scripts, which add what is specific to each
tool. See `crates/pyprojx_core/src/tool.rs` for the Rust side.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import urllib.error
import urllib.request
from collections.abc import Callable, Iterable
from dataclasses import dataclass, field
from pathlib import Path

from packaging.version import InvalidVersion, Version

ROOT = Path(__file__).resolve().parents[1]
CACHE = Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache")) / "pyprojx"


def releases(package: str, first: Version) -> list[Version]:
    """Final, unyanked releases of a PyPI package since `first`, oldest first."""
    with urllib.request.urlopen(f"https://pypi.org/pypi/{package}/json") as response:
        data = json.load(response)
    found = []
    for raw, files in data["releases"].items():
        try:
            version = Version(raw)
        except InvalidVersion:
            continue
        if version.is_prerelease or version.is_devrelease or version < first:
            continue
        if files and not all(file["yanked"] for file in files):
            found.append(version)
    return sorted(found)


def fetch(urls: Iterable[str], path: Path) -> bytes | None:
    """The first of `urls` that exists, cached at `path`; `None` if none does."""
    if not path.exists():
        for url in urls:
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
    return path.read_bytes()


@dataclass(frozen=True)
class Info:
    kind: str
    deprecated: bool
    selectors: bool
    # The canonical type of the values, from `value_type`; `None` for tables.
    value_type: tuple | None


def options(document: dict, named: dict[str, tuple] | None = None) -> dict[str, Info]:
    """Every option path, such as `lint.isort.known-first-party`, with its kind,
    whether it is deprecated, whether its values are Ruff rule selectors, and
    the type of its values. `named` gives the types of some definitions, such
    as Ruff's `RuleSelector`, by name."""
    definitions = document.get("definitions", {})
    named = named or {}

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

    def value_type(node: dict) -> tuple:
        """A canonical, hashable type, ignoring how the schema spells it."""
        while "$ref" in node:
            name = node["$ref"].rsplit("/", 1)[1]
            if name in named:
                return named[name]
            node = definitions[name]
        for key in ("anyOf", "oneOf", "allOf"):
            if key in node:
                members = [c for c in node[key] if c.get("type") != "null"]
                return combine([value_type(member) for member in members])
        if "enum" in node:
            variants = {str(v) for v in node["enum"] if v is not None}
            return ("enum", tuple(sorted(variants)))
        if "const" in node:
            return ("enum", (str(node["const"]),))
        types = node.get("type")
        types = [types] if isinstance(types, str) else list(types or [])
        members = []
        for name in types:
            if name == "boolean":
                members.append(("bool",))
            elif name == "integer":
                minimum = node.get("minimum")
                members.append(("int", None if minimum is None else int(minimum)))
            elif name == "number":
                members.append(("number",))
            elif name == "string":
                members.append(("str",))
            elif name == "array":
                members.append(("array", value_type(node.get("items", {}))))
            elif name != "null":
                members.append(("any",))
        return combine(members) if members else ("any",)

    def combine(members: list[tuple]) -> tuple:
        flat = []
        for member in members:
            flat.extend(member[1] if member[0] == "oneof" else [member])
        if not flat or ("any",) in flat:
            return ("any",)
        variants = tuple(v for m in flat if m[0] == "enum" for v in m[1])
        rest = [m for m in flat if m[0] != "enum"]
        # Any string covers the named ones.
        if variants and ("str",) not in rest:
            rest.append(("enum", tuple(sorted(set(variants)))))
        unique = list(dict.fromkeys(rest))
        return unique[0] if len(unique) == 1 else ("oneof", tuple(unique))

    found: dict[str, Info] = {}

    def selects_rules(node: dict) -> bool:
        items = resolve(node).get("items")
        return isinstance(items, dict) and items.get("$ref", "").endswith(
            "/RuleSelector"
        )

    def walk(node: dict, prefix: str) -> None:
        for key, prop in node.get("properties", {}).items():
            path = prefix + key
            target = resolve(prop)
            deprecated = bool(prop.get("deprecated") or target.get("deprecated"))
            values = target.get("additionalProperties")
            if isinstance(values, dict):
                # Keys are names the user chooses, such as file patterns, even
                # if the schema lists some.
                found[path] = Info(
                    "Map", deprecated, selects_rules(values), value_type(values)
                )
            elif "properties" in target:
                found[path] = Info("Table", deprecated, False, None)
                walk(target, path + ".")
            elif "properties" in (entry := resolve(target.get("items") or {})):
                # An array of tables, such as `[[tool.ty.overrides]]`.
                found[path] = Info("TableArray", deprecated, False, None)
                walk(entry, path + ".")
            else:
                found[path] = Info(
                    "Value", deprecated, selects_rules(target), value_type(prop)
                )

    walk(document, "")
    return found


@dataclass
class Option:
    kind: str
    selectors: bool = False
    present: list[int] = field(default_factory=list)
    deprecated: list[int] = field(default_factory=list)
    # The value type in each release that accepts the option.
    types: dict[int, tuple] = field(default_factory=dict)


def collect(
    versions: list[Version],
    schemas: list[dict],
    named: dict[str, tuple] | None = None,
) -> dict[str, Option]:
    """Each option in any release's schema, with the releases that accept and
    deprecate it and its value type in each."""
    found: dict[str, Option] = {}
    for index, document in enumerate(schemas):
        for path, info in options(document, named).items():
            kind, deprecated = info.kind, info.deprecated
            option = found.setdefault(path, Option(kind))
            option.selectors = option.selectors or info.selectors
            if info.value_type is not None:
                option.types[index] = info.value_type
            if option.kind != kind:
                print(
                    f"{path} changes kind in {versions[index]}; using {kind}",
                    file=sys.stderr,
                )
                option.kind = kind
            option.present.append(index)
            if deprecated:
                option.deprecated.append(index)
    merge_case_changes(found)
    return found


def merge_case_changes(found: dict[str, Option]) -> None:
    """Keeps old spellings of values whose case changed, which tools accept."""
    for option in found.values():
        previous = None
        for index in sorted(option.types):
            current = option.types[index]
            if (
                previous is not None
                and current[0] == previous[0] == "enum"
                and {v.lower() for v in current[1]} == {v.lower() for v in previous[1]}
            ):
                current = ("enum", tuple(sorted(set(current[1]) | set(previous[1]))))
                option.types[index] = current
            previous = current


def example(value_type: tuple | None, kind: str) -> str:
    """A TOML value an option of this type and kind accepts."""
    if kind in ("Table", "Map"):
        return "{}"
    if kind == "TableArray":
        return "[]"
    name, *rest = value_type or ("any",)
    if name == "bool":
        return "true"
    if name == "int":
        return str(rest[0] or 1)
    if name == "number":
        return "1"
    if name == "enum":
        return json.dumps(rest[0][0])
    if name == "array":
        return f"[{example(rest[0], 'Value')}]"
    if name == "oneof":
        return example(rest[0][0], "Value")
    if name == "selector":
        return '"E"'
    return '"x"'


def setting(table: str, found: dict[str, Option], path: str, value: str) -> str:
    """TOML setting the option at `path` in `table`, such as `tool.ty`, to
    `value`, inside an inline table for options in arrays of tables."""
    parts = path.split(".")
    for end in range(len(parts) - 1, 0, -1):
        parent = ".".join(parts[:end])
        if found.get(parent) and found[parent].kind == "TableArray":
            inner = setting(table, found, ".".join(parts[end:]), value).split("\n", 1)[
                1
            ]
            return setting(table, found, parent, f"[{{ {inner.strip()} }}]")
    return f"[{table}]\n{path} = {value}\n"


def add_aliases(
    found: dict[str, Option],
    versions: list[Version],
    accepts: Callable[[int, str], bool],
) -> dict[str, str]:
    """Measures which releases still accept options after they leave the
    schema, as tools do for renamed options, and records those releases as
    accepting and deprecating them. `accepts(index, path)` tells whether a
    release accepts the option at `path`. Returns the new names of renamed
    options, as deprecation messages."""
    latest = len(versions) - 1
    messages = {}
    for path, option in found.items():
        last = option.present[-1]
        if last == latest:
            continue
        later = list(range(last + 1, latest + 1))
        rejecting = first_where(
            later, lambda index, path=path: not accepts(index, path)
        )
        accepting = later if rejecting is None else later[: later.index(rejecting)]
        if not accepting:
            continue
        print(
            f"{path} is accepted until {versions[accepting[-1]]} after leaving the schema"
        )
        last_type = option.types.get(last)
        for index in accepting:
            option.present.append(index)
            option.deprecated.append(index)
            if last_type is not None:
                option.types[index] = last_type
        parent, _, _ = path.rpartition(".")
        renamed = [
            other
            for other, data in found.items()
            if other.rpartition(".")[0] == parent and data.present[0] == last + 1
        ]
        if len(renamed) == 1:
            name = renamed[0].rpartition(".")[2]
            messages[path] = f"use `{name}`, the option's new name"
    return messages


def ranges(indexes: list[int]) -> list[tuple[int, int]]:
    """Consecutive runs of release indexes, as inclusive (first, last) pairs."""
    runs: list[tuple[int, int]] = []
    for index in indexes:
        if runs and runs[-1][1] == index - 1:
            runs[-1] = (runs[-1][0], index)
        else:
            runs.append((index, index))
    return runs


def first_where(indexes: list[int], predicate: Callable[[int], bool]) -> int | None:
    """The first index for which `predicate` holds, assuming it holds from some
    index on, found by bisecting; `None` if it does not hold for the last."""
    if not indexes or not predicate(indexes[-1]):
        return None
    low, high = -1, len(indexes) - 1  # `predicate` holds at `high`, not at `low`
    while high - low > 1:
        middle = (low + high) // 2
        if predicate(indexes[middle]):
            high = middle
        else:
            low = middle
    return indexes[high]


def probe(cache: Path, command: list[str], files: dict[str, str]) -> str:
    """The output of `command` run in a directory with `files`, cached across
    runs in `cache`."""
    known = json.loads(cache.read_text()) if cache.exists() else {}
    key = " ".join(command) + "\n" + "\n".join(f"{n}:\n{c}" for n, c in files.items())
    if key not in known:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name, content in files.items():
                (root / name).write_text(content)
            process = subprocess.run(
                command, cwd=root, capture_output=True, text=True, check=False
            )
        known[key] = process.stdout + process.stderr
        cache.parent.mkdir(parents=True, exist_ok=True)
        cache.write_text(json.dumps(known, indent=1, sort_keys=True))
    return known[key]


def rust_type(value_type: tuple) -> str:
    """A Rust `ValueType` expression."""
    kind, *rest = value_type
    simple = {
        "any": "Any",
        "bool": "Bool",
        "number": "Number",
        "str": "Str",
        "selector": "Selector",
    }
    if kind in simple:
        return f"ValueType::{simple[kind]}"
    if kind == "int":
        minimum = "None" if rest[0] is None else f"Some({rest[0]})"
        return f"ValueType::Int {{ min: {minimum} }}"
    if kind == "enum":
        return "ValueType::Enum(&[" + ", ".join(map(rust_string, rest[0])) + "])"
    if kind == "array":
        return f"ValueType::Array(&{rust_type(rest[0])})"
    if kind == "oneof":
        return "ValueType::OneOf(&[" + ", ".join(map(rust_type, rest[0])) + "])"
    raise ValueError(value_type)


def type_runs(types: dict[int, tuple]) -> list[tuple[int, int, tuple]]:
    """Runs of consecutive releases with the same value type."""
    runs: list[tuple[int, int, tuple]] = []
    for index in sorted(types):
        if runs and runs[-1][1] == index - 1 and runs[-1][2] == types[index]:
            runs[-1] = (runs[-1][0], index, types[index])
        else:
            runs.append((index, index, types[index]))
    return runs


def rust_string(value: str) -> str:
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def rust_ranges(runs: list[tuple[int, int]]) -> str:
    return "&[" + ", ".join(f"({a}, {b})" for a, b in runs) + "]"


def generate_options(
    tool: str,
    versions: list[Version],
    found: dict[str, Option],
    messages: dict[str, str],
) -> list[str]:
    """Rust lines defining `RELEASES`, the value types, and `OPTIONS`."""
    lines = [
        f"/// {tool} releases, oldest first. Other data refers to them by index.",
        "pub const RELEASES: &[&str] = &[",
    ]
    lines += [f"    {rust_string(str(version))}," for version in versions]
    lines += [
        "];",
        "",
    ]
    # Value types, shared between options.
    names: dict[tuple, str] = {}
    for option in found.values():
        for value_type in option.types.values():
            names.setdefault(value_type, f"T{len(names)}")
    lines += [
        f"const {name}: ValueType = {rust_type(value_type)};"
        for value_type, name in names.items()
    ]
    lines += [
        "",
        "/// Options by path, sorted.",
        "pub const OPTIONS: &[OptionData] = &[",
    ]
    for path in sorted(found):
        option = found[path]
        types = ", ".join(
            f"({first}, {last}, &{names[value_type]})"
            for first, last, value_type in type_runs(option.types)
        )
        message = f"Some({rust_string(messages[path])})" if path in messages else "None"
        lines.append(
            f"    OptionData {{ path: {rust_string(path)}, kind: OptionKind::{option.kind}, "
            f"selectors: {str(option.selectors).lower()}, types: &[{types}], "
            f"present: {rust_ranges(ranges(option.present))}, "
            f"deprecated: {rust_ranges(ranges(option.deprecated))}, message: {message} }},"
        )
    lines += ["];", ""]
    return lines
