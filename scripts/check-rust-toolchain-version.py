#!/usr/bin/env python3
"""
Checks that the workspace `rust-version` is not newer than the versioned
toolchain pinned in `rust-toolchain.toml`.
"""

import re
import sys
from pathlib import Path

from _toml_utils import find_string_value

REPO_ROOT = Path(__file__).resolve().parent.parent
VERSION_RE = re.compile(r"^\d+(?:\.\d+){1,2}$")


def parse_version(version: str) -> tuple[int, int, int]:
    parts = [int(p) for p in version.split(".")]
    while len(parts) < 3:
        parts.append(0)
    return (parts[0], parts[1], parts[2])


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

    if parse_version(rust_version) > parse_version(channel):
        print(
            "error: Cargo.toml `workspace.package.rust-version` "
            f"({rust_version}) is newer than rust-toolchain.toml `toolchain.channel` ({channel}). "
            "The MSRV must not exceed the pinned toolchain."
        )
        return 1

    print(
        f"Cargo.toml `workspace.package.rust-version` ({rust_version}) "
        f"<= rust-toolchain.toml `toolchain.channel` ({channel})."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
