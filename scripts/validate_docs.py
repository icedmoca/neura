#!/usr/bin/env python3
"""Validate source-backed documentation inventory for Kcode."""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

REQUIRED_DOCS = [
    "README.md",
    "docs/ARCHITECTURE.md",
    "docs/OPERATIONS.md",
    "docs/reference/implementation-inventory.md",
    "docs/reference/implementation-inventory.json",
]

REQUIRED_ANCHORS = [
    "src/adaptive_cognition.rs",
    "src/operational_repair_learning.rs",
    "src/local_model.rs",
    "src/provider/mod.rs",
    "src/tui/info_widget_usage.rs",
]


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8", errors="ignore")


def collect_inventory() -> dict:
    rs_files = list((ROOT / "src").rglob("*.rs")) + list((ROOT / "crates").rglob("*.rs"))

    modules = []
    slash_commands = []
    for path in rs_files:
        rel = path.relative_to(ROOT).as_posix()
        text = read(path)
        for match in re.finditer(r"^pub mod ([A-Za-z0-9_]+);", text, re.MULTILINE):
            modules.append({"file": rel, "module": match.group(1)})
        for match in re.finditer(
            r'RegisteredCommand::public\(\s*"([^"]+)"\s*,\s*(?:Some\()?"([^"]*)"\)?',
            text,
            re.DOTALL,
        ):
            slash_commands.append({"command": match.group(1), "description": match.group(2), "file": rel})

    bin_dir = ROOT / "src" / "bin"
    binaries = []
    if bin_dir.exists():
        for path in sorted(bin_dir.glob("*.rs")):
            binaries.append({"binary": path.stem.replace("_", "-"), "file": path.relative_to(ROOT).as_posix()})

    provider_dir = ROOT / "src" / "provider"
    provider_files = []
    if provider_dir.exists():
        for path in sorted(provider_dir.glob("*.rs")):
            provider_files.append({"name": path.stem, "provider_file": path.relative_to(ROOT).as_posix()})

    return {
        "modules": sorted(modules, key=lambda item: (item["file"], item["module"])),
        "slash_commands": sorted(slash_commands, key=lambda item: (item["command"], item["file"])),
        "binaries": sorted(binaries, key=lambda item: item["binary"]),
        "provider_files": sorted(provider_files, key=lambda item: item["name"]),
    }


def inventory_markdown(inventory: dict) -> str:
    lines = [
        "# Implementation inventory",
        "",
        "Generated from source with `scripts/validate_docs.py --write-inventory`.",
        "",
        "## Binaries",
        "",
    ]
    lines.extend(f"- `{item['binary']}`: `{item['file']}`" for item in inventory["binaries"])
    lines.extend(["", "## Public slash commands", ""])
    lines.extend(
        f"- `{item['command']}`: {item['description']} (`{item['file']}`)"
        for item in inventory["slash_commands"]
    )
    lines.extend(["", "## Provider implementation files", ""])
    lines.extend(f"- `{item['name']}`: `{item['provider_file']}`" for item in inventory["provider_files"])
    lines.extend(["", "## Public modules", ""])
    lines.extend(f"- `{item['module']}` from `{item['file']}`" for item in inventory["modules"])
    return "\n".join(lines) + "\n"


def write_inventory(inventory: dict) -> None:
    ref = ROOT / "docs" / "reference"
    ref.mkdir(parents=True, exist_ok=True)
    (ref / "implementation-inventory.json").write_text(json.dumps(inventory, indent=2) + "\n", encoding="utf-8")
    (ref / "implementation-inventory.md").write_text(inventory_markdown(inventory), encoding="utf-8")


def validate(inventory: dict) -> list[str]:
    errors: list[str] = []
    for doc in REQUIRED_DOCS:
        if not (ROOT / doc).exists():
            errors.append(f"missing required doc: {doc}")
    for anchor in REQUIRED_ANCHORS:
        if not (ROOT / anchor).exists():
            errors.append(f"missing required implementation anchor: {anchor}")

    json_path = ROOT / "docs" / "reference" / "implementation-inventory.json"
    md_path = ROOT / "docs" / "reference" / "implementation-inventory.md"
    if json_path.exists():
        try:
            existing = json.loads(read(json_path))
            if existing != inventory:
                errors.append("implementation inventory JSON is stale; run scripts/validate_docs.py --write-inventory")
        except json.JSONDecodeError as exc:
            errors.append(f"implementation inventory JSON is invalid: {exc}")
    if md_path.exists() and read(md_path) != inventory_markdown(inventory):
        errors.append("implementation inventory markdown is stale; run scripts/validate_docs.py --write-inventory")

    readme = read(ROOT / "README.md") if (ROOT / "README.md").exists() else ""
    for phrase in ["operational_repair_learning", "adaptive_cognition", "docs/ARCHITECTURE.md", "docs/INSTALL.md"]:
        if phrase not in readme:
            errors.append(f"README missing required phrase: {phrase}")

    if len(inventory["slash_commands"]) < 20:
        errors.append("unexpectedly few slash commands discovered")
    if len(inventory["provider_files"]) < 10:
        errors.append("unexpectedly few provider files discovered")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--write-inventory", action="store_true")
    args = parser.parse_args()

    inventory = collect_inventory()
    if args.write_inventory:
        write_inventory(inventory)

    errors = validate(inventory)
    if errors:
        for error in errors:
            print(f"docs validation error: {error}", file=sys.stderr)
        return 1
    print(
        "docs validation ok: "
        f"{len(inventory['binaries'])} binaries, "
        f"{len(inventory['slash_commands'])} slash commands, "
        f"{len(inventory['provider_files'])} provider files, "
        f"{len(inventory['modules'])} public modules"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
