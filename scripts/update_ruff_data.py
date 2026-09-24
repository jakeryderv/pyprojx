# /// script
# requires-python = ">=3.11"
# dependencies = ["packaging"]
# ///
"""Generate pyprojx's data about Ruff's configuration across releases.

For every Ruff release since 0.1.0, reads the JSON schema of its configuration
(`ruff.schema.json` at the release's tag) and records which releases accept
each option and rule selector, and which deprecate each option. From the latest
release, it records option deprecation messages (`ruff config`), each rule's
status (`ruff rule --all`), and the rule codes Ruff redirects to others.

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
import tempfile
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


@dataclass(frozen=True)
class Info:
    kind: str
    deprecated: bool
    selectors: bool
    # The canonical type of the values, from `value_type`; `None` for tables.
    value_type: tuple | None


def options(document: dict) -> dict[str, Info]:
    """Every option path, such as `lint.isort.known-first-party`, with its kind,
    whether it is deprecated, whether its values are rule selectors, and the
    type of its values."""
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

    def value_type(node: dict) -> tuple:
        """A canonical, hashable type, ignoring how the schema spells it."""
        while "$ref" in node:
            name = node["$ref"].rsplit("/", 1)[1]
            if name == "RuleSelector":
                return ("selector",)
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
            else:
                found[path] = Info(
                    "Value", deprecated, selects_rules(target), value_type(prop)
                )

    walk(document, "")
    return found


def selectors(document: dict) -> list[str]:
    """The rule selectors a release accepts, such as `E`, `E5`, and `E501`."""
    return document["definitions"]["RuleSelector"]["enum"]


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


def rule_statuses(latest: Version, versions: list[Version]) -> dict[str, str]:
    """Each rule's status in the latest release, as Rust `RuleStatus` values."""
    output = subprocess.run(
        [
            "uvx",
            "--quiet",
            f"ruff@{latest}",
            "rule",
            "--all",
            "--output-format",
            "json",
        ],
        capture_output=True,
        text=True,
        check=True,
    ).stdout

    def index(since: str) -> int:
        """The first release from `since` on; earlier versions map to the first."""
        since = Version(since.removeprefix("v"))
        return next(i for i, version in enumerate(versions) if version >= since)

    statuses = {}
    for rule in json.loads(output):
        [(kind, detail)] = rule["status"].items()
        if kind == "Preview":
            statuses[rule["code"]] = "RuleStatus::Preview"
        elif kind == "Stable":
            statuses[rule["code"]] = f"RuleStatus::Stable({index(detail['since'])})"
        elif kind == "Removed":
            statuses[rule["code"]] = f"RuleStatus::Removed({index(detail['since'])})"
        elif kind == "Deprecated":
            # Not seen yet; deprecated rules have so far been removed.
            raise RuntimeError(
                f"{rule['code']} is deprecated; record it like removed rules"
            )
        else:
            raise RuntimeError(f"unknown status {kind} for {rule['code']}")
    return statuses


def probe(version: Version, config: str) -> str:
    """Ruff's output when a release checks a file with the given configuration,
    cached across runs."""
    cache = CACHE / "probes.json"
    known = json.loads(cache.read_text()) if cache.exists() else {}
    key = f"{version}\n{config}"
    if key not in known:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "a.py").write_text("x = 1\n")
            (root / "pyproject.toml").write_text(config)
            process = subprocess.run(
                ["uvx", "--quiet", f"ruff@{version}", "check", "--no-cache", "a.py"],
                cwd=root,
                capture_output=True,
                text=True,
                check=False,
            )
        known[key] = process.stdout + process.stderr
        cache.write_text(json.dumps(known, indent=1, sort_keys=True))
    return known[key]


def warns_deprecated(version: Version, code: str) -> bool:
    """Whether a release warns that the rule is deprecated when it is selected."""
    output = probe(version, f'[tool.ruff.lint]\nselect = ["{code}"]\n')
    return f"`{code}` is deprecated" in output


def first_where(indexes: list[int], predicate) -> int | None:
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


def python_in_development(
    found: dict[str, Option], versions: list[Version]
) -> list[tuple[str, int, int]]:
    """For each `target-version` value, the releases that warn that support for
    it is under development, as (value, first, last) index ranges."""
    target = found["target-version"]
    windows = []
    variants = sorted(
        {v for t in target.types.values() if t[0] == "enum" for v in t[1]}
    )
    for variant in variants:
        present = [i for i, t in sorted(target.types.items()) if variant in t[1]]
        config = f'[tool.ruff]\ntarget-version = "{variant}"\n'

        def warns(index: int, config: str = config) -> bool:
            return "is under development" in probe(versions[index], config)

        if not warns(present[0]):
            continue
        stable = first_where(present, lambda index: not warns(index))
        last = present[-1] if stable is None else present[present.index(stable) - 1]
        windows.append((variant, present[0], last))
    return windows


# Values Ruff rejects although the schema still lists them, and the releases
# that deprecated them first, found with scripts/compare_with_ruff.py.
VALUE_OVERRIDES = [
    # (option, value, deprecated from, rejected from, message)
    ("output-format", "text", "0.2.0", "0.5.0", 'use "full" or "concise"'),
]


def apply_overrides(
    found: dict[str, Option], versions: list[Version]
) -> list[tuple[str, str, int, int, str]]:
    """Removes values Ruff rejects from their options' types, and returns the
    deprecation windows of those values."""
    deprecations = []
    for path, value, deprecated, rejected, message in VALUE_OVERRIDES:
        rejected_index = versions.index(Version(rejected))
        option = found[path]
        for index, value_type in option.types.items():
            if index >= rejected_index and value_type[0] == "enum":
                variants = tuple(v for v in value_type[1] if v != value)
                option.types[index] = ("enum", variants)
        first = versions.index(Version(deprecated))
        deprecations.append((path, value, first, rejected_index - 1, message))
    return deprecations


def merge_case_changes(found: dict[str, Option]) -> None:
    """Keeps old spellings of values whose case changed, which Ruff accepts."""
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


def deprecated_since(
    code: str, present: list[int], versions: list[Version]
) -> int | None:
    """The first release that deprecates a removed rule, found by bisecting the
    releases that accept it; `None` if none did."""
    return first_where(present, lambda index: warns_deprecated(versions[index], code))


def redirects(latest: Version) -> dict[str, str]:
    """Rule codes Ruff accepts and redirects to others, from its source."""
    url = f"https://raw.githubusercontent.com/astral-sh/ruff/{latest}/crates/ruff_linter/src/rule_redirects.rs"
    with urllib.request.urlopen(url) as response:
        source = response.read().decode()
    table = source[source.index("HashMap::from_iter([") : source.index("])")]
    pairs = dict(re.findall(r'\("([A-Z0-9]+)",\s*"([A-Z0-9]+)"\)', table))
    if not pairs:
        raise RuntimeError("no redirects found; has rule_redirects.rs changed?")
    return pairs


@dataclass
class Option:
    kind: str
    selectors: bool = False
    present: list[int] = field(default_factory=list)
    deprecated: list[int] = field(default_factory=list)
    # The value type in each release that accepts the option.
    types: dict[int, tuple] = field(default_factory=dict)


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


def generate(
    versions: list[Version],
    found: dict[str, Option],
    messages: dict[str, str],
    rules: dict[str, list[int]],
    statuses: dict[str, str],
    redirected: dict[str, str],
    development: list[tuple[str, int, int]],
    deprecated_values: list[tuple[str, str, int, int, str]],
) -> str:
    lines = [
        "// @generated by scripts/update_ruff_data.py; do not edit.",
        "",
        "use crate::ruff::{OptionData, OptionKind, RuleStatus, SelectorData, ValueType};",
        "",
        "/// Ruff releases, oldest first. Other data refers to them by index.",
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
    lines += [
        "];",
        "",
        "/// Rule selectors (codes, prefixes of codes, and rule names), sorted.",
        "pub const SELECTORS: &[SelectorData] = &[",
    ]
    for selector in sorted(rules):
        status = statuses.get(selector)
        status = f"Some({status})" if status else "None"
        lines.append(
            f"    SelectorData {{ selector: {rust_string(selector)}, "
            f"present: {rust_ranges(ranges(rules[selector]))}, status: {status} }},"
        )
    lines += [
        "];",
        "",
        "/// Rule codes Ruff redirects to others, sorted.",
        "pub const REDIRECTS: &[(&str, &str)] = &[",
    ]
    lines += [
        f"    ({rust_string(old)}, {rust_string(new)}),"
        for old, new in sorted(redirected.items())
    ]
    lines += [
        "];",
        "",
        "/// `target-version` values that releases support only in preview, with",
        "/// inclusive ranges of those releases.",
        "pub const PYTHON_IN_DEVELOPMENT: &[(&str, u16, u16)] = &[",
    ]
    lines += [
        f"    ({rust_string(value)}, {first}, {last}),"
        for value, first, last in development
    ]
    lines += [
        "];",
        "",
        "/// Option values that releases deprecate: option, value, inclusive range of",
        "/// releases, and what to use instead.",
        "pub const DEPRECATED_VALUES: &[(&str, &str, u16, u16, &str)] = &[",
    ]
    lines += [
        f"    ({rust_string(path)}, {rust_string(value)}, {first}, {last}, {rust_string(message)}),"
        for path, value, first, last, message in deprecated_values
    ]
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
    rules: dict[str, list[int]] = {}
    for index, document in enumerate(schemas):
        for selector in selectors(document):
            rules.setdefault(selector, []).append(index)
        for path, info in options(document).items():
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

    latest = versions[-1]
    statuses = rule_statuses(latest, versions)
    for code, status in statuses.items():
        if status.startswith("RuleStatus::Removed"):
            # Removed rules no known release accepts are recorded for their messages.
            present = rules.setdefault(code, [])
            since = deprecated_since(code, present, versions)
            removed = status.removeprefix("RuleStatus::Removed(").removesuffix(")")
            deprecated = "None" if since is None else f"Some({since})"
            statuses[code] = (
                f"RuleStatus::Removed {{ deprecated: {deprecated}, removed: {removed} }}"
            )
    merge_case_changes(found)
    deprecated_values = apply_overrides(found, versions)
    content = generate(
        versions,
        found,
        deprecation_messages(latest),
        rules,
        statuses,
        redirects(latest),
        python_in_development(found, versions),
        deprecated_values,
    )
    OUTPUT.write_text(content, encoding="utf-8")
    print(
        f"wrote {len(found)} options and {len(rules)} rule selectors "
        f"across {len(versions)} releases to {OUTPUT}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
