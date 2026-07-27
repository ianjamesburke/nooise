#!/usr/bin/env python3
"""Append a song code to AUTO_STATES in src/fluid/auto.rs, in place."""

import re
import sys
from pathlib import Path

AUTO_RS = Path(__file__).resolve().parent.parent / "src/fluid/auto.rs"


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: add_morph.py <n1_...song_code>", file=sys.stderr)
        return 1
    code = sys.argv[1]
    if not code.startswith("n1_"):
        print(f"error: not a song code (must start with n1_): {code!r}", file=sys.stderr)
        return 1

    text = AUTO_RS.read_text()
    entries = re.findall(r'// (\d+)\. New morph target\.', text)
    next_index = int(entries[-1]) + 1 if entries else 1

    marker = "\n];"
    if marker not in text:
        print("error: could not find AUTO_STATES closing `];` in auto.rs", file=sys.stderr)
        return 1

    insertion = f'    // {next_index}. New morph target.\n    "{code}",\n];'
    text = text.replace(marker, insertion, 1)
    AUTO_RS.write_text(text)
    print(f"appended as entry {next_index}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
