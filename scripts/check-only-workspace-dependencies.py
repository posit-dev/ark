#!/usr/bin/env python3
"""
Checks that crate-level Cargo.toml files use workspace dependency inheritance
rather than specifying versions inline. Every dependency must be referenced with
`dep.workspace = true` or `dep = { workspace = true, ... }`.
"""

import sys
import tomllib
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
DEP_SECTIONS = ["dependencies", "dev-dependencies", "build-dependencies"]


def check_deps(toml_path: Path) -> list[str]:
    with open(toml_path, "rb") as f:
        data = tomllib.load(f)

    rel_path = toml_path.relative_to(REPO_ROOT)
    errors = []

    for section_name in DEP_SECTIONS:
        # Top-level dependency sections
        deps = data.get(section_name, {})
        errors.extend(check_section(rel_path, section_name, deps))

        # Target-specific dependency sections, e.g. [target.'cfg(unix)'.dependencies]
        for target_name, target_data in data.get("target", {}).items():
            deps = target_data.get(section_name, {})
            errors.extend(
                check_section(rel_path, f"target.{target_name}.{section_name}", deps)
            )

    return errors


def check_section(
    rel_path: Path, section_name: str, deps: dict
) -> list[str]:
    errors = []
    for dep_name, dep_value in deps.items():
        is_workspace = isinstance(dep_value, dict) and dep_value.get("workspace")
        if not is_workspace:
            errors.append(
                f"error: {rel_path} [{section_name}]: "
                f"'{dep_name}' must use workspace inheritance"
            )
    return errors


def main() -> int:
    crate_tomls = [
        p
        for p in REPO_ROOT.rglob("Cargo.toml")
        if p != REPO_ROOT / "Cargo.toml" and "target" not in p.parts
    ]

    errors = []

    for toml_path in sorted(crate_tomls):
        errors.extend(check_deps(toml_path))

    for error in errors:
        print(error)

    if errors:
        return 1

    print("All crate dependencies use workspace inheritance.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
