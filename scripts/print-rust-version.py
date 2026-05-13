#!/usr/bin/env python3
"""
Prints the workspace `rust-version` from `Cargo.toml`.

Used by CI to pin the MSRV toolchain. Exits non-zero (and prints nothing on
stdout) if the value is missing, so `$(...)` substitution doesn't silently
produce an empty toolchain name.
"""

import sys
from pathlib import Path

from _toml_utils import find_string_value

REPO_ROOT = Path(__file__).resolve().parent.parent


def main() -> int:
    rust_version = find_string_value(
        REPO_ROOT / "Cargo.toml", "workspace.package", "rust-version"
    )

    if rust_version is None:
        print(
            "error: Cargo.toml is missing `workspace.package.rust-version`",
            file=sys.stderr,
        )
        return 1

    print(rust_version)
    return 0


if __name__ == "__main__":
    sys.exit(main())
