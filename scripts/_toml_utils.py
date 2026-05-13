"""
Tiny TOML helpers shared by `scripts/check-rust-toolchain-version.py` and
`scripts/print-rust-version.py`.

The leading underscore marks this as an internal module: don't run it directly,
import from it.
"""

import re
from pathlib import Path

SECTION_RE = re.compile(r"^\[(.+)\]$")
KEY_VALUE_RE = re.compile(r'^([A-Za-z0-9_-]+)\s*=\s*"([^"]+)"\s*$')


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
