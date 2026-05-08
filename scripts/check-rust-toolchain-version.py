#!/usr/bin/env python3
"""
Checks that the workspace `rust-version` matches the versioned toolchain pinned
in `rust-toolchain.toml`.
"""

import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
SECTION_RE = re.compile(r"^\[(.+)\]$")
KEY_VALUE_RE = re.compile(r'^([A-Za-z0-9_-]+)\s*=\s*"([^"]+)"\s*$')
VERSION_RE = re.compile(r"^\d+(?:\.\d+){1,2}$")


def find_string_value(path: Path, section_name: str, key_name: str) -> str | None:
    current_section = None

    with open(path, encoding="utf-8") as f:
        for raw_line in f:
            line = raw_line.strip()
            if not line or line.startswith("#"):
                continue

            section_match = SECTION_RE.fullmatch(line)
            if section_match:
                current_section = section_match.group(1)
                continue

            if current_section != section_name:
                continue

            key_value_match = KEY_VALUE_RE.fullmatch(line)
            if key_value_match and key_value_match.group(1) == key_name:
                return key_value_match.group(2)

    return None


def normalize_version(version: str) -> str:
    parts = version.split(".")
    while len(parts) > 2 and parts[-1] == "0":
        parts.pop()
    return ".".join(parts)


def main() -> int:
    rust_version = find_string_value(
        REPO_ROOT / "Cargo.toml", "workspace.package", "rust-version"
    )
    channel = find_string_value(
        REPO_ROOT / "rust-toolchain.toml", "toolchain", "channel"
    )

    if rust_version is None:
        print("error: Cargo.toml is missing `workspace.package.rust-version`")
        return 1

    if channel is None:
        print("error: rust-toolchain.toml is missing `toolchain.channel`")
        return 1

    if not VERSION_RE.fullmatch(channel):
        print(
            "error: rust-toolchain.toml `toolchain.channel` must be a versioned toolchain "
            f"to compare with `workspace.package.rust-version`, got '{channel}'"
        )
        return 1

    if normalize_version(rust_version) != normalize_version(channel):
        print(
            "error: Cargo.toml `workspace.package.rust-version` "
            f"({rust_version}) does not match rust-toolchain.toml `toolchain.channel` ({channel})"
        )
        return 1

    print(
        "Cargo.toml `workspace.package.rust-version` matches rust-toolchain.toml `toolchain.channel`."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
