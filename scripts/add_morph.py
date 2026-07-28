#!/usr/bin/env python3
"""Append, verify, and commit one built-in auto-morph song code."""

import os
import re
import stat
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Callable, Sequence

ROOT = Path(__file__).resolve().parent.parent
AUTO_RS = ROOT / "src/fluid/auto.rs"
AUTO_RS_PATH = Path("src/fluid/auto.rs")
ARRAY_START = "const AUTO_STATES: &[&str] = &[\n"
INSERTION_MARKER = "    // add-morph inserts new entries immediately above this line.\n];"
SONG_CODE = re.compile(r"n1_[A-Za-z0-9_-]+")
CommandRunner = Callable[[Sequence[str], Path, bool], subprocess.CompletedProcess[str]]


class AddMorphError(Exception):
    """A user-correctable add-morph failure."""


def run_command(
    command: Sequence[str], root: Path, check: bool = True
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(command, cwd=root, check=check, text=True)


def require_clean_target(root: Path, run: CommandRunner) -> None:
    for command in (
        ["git", "diff", "--quiet", "--", str(AUTO_RS_PATH)],
        ["git", "diff", "--cached", "--quiet", "--", str(AUTO_RS_PATH)],
    ):
        result = run(command, root, False)
        if result.returncode == 1:
            raise AddMorphError(
                f"{AUTO_RS_PATH} has uncommitted changes; commit or stash them first"
            )
        if result.returncode != 0:
            raise subprocess.CalledProcessError(result.returncode, command)


def append_code(source: str, code: str) -> str:
    if SONG_CODE.fullmatch(code) is None:
        raise AddMorphError("song code must start with n1_ and contain base64url characters")
    if source.count(ARRAY_START) != 1 or source.count(INSERTION_MARKER) != 1:
        raise AddMorphError("could not find the AUTO_STATES insertion marker")

    before_array, array_and_after = source.split(ARRAY_START, 1)
    entries, after_entries = array_and_after.split(INSERTION_MARKER, 1)
    existing_codes = SONG_CODE.findall(entries)
    if code in existing_codes:
        raise AddMorphError("that song code is already in AUTO_STATES")

    return (
        before_array
        + ARRAY_START
        + entries
        + f'    "{code}",\n'
        + INSERTION_MARKER
        + after_entries
    )


def write_atomic(path: Path, text: str) -> None:
    mode = stat.S_IMODE(path.stat().st_mode)
    temp_path: Path | None = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="w",
            dir=path.parent,
            prefix=f".{path.name}.add-morph.",
            delete=False,
        ) as temp:
            temp.write(text)
            temp_path = Path(temp.name)
        os.chmod(temp_path, mode)
        temp_path.replace(path)
        temp_path = None
    finally:
        if temp_path is not None:
            temp_path.unlink(missing_ok=True)


def add_morph(
    code: str,
    *,
    root: Path = ROOT,
    auto_rs: Path = AUTO_RS,
    run: CommandRunner = run_command,
) -> None:
    require_clean_target(root, run)
    original = auto_rs.read_text()
    updated = append_code(original, code)
    committed = False
    write_atomic(auto_rs, updated)

    try:
        run(["cargo", "fmt", "--check"], root, True)
        run(["cargo", "test", "auto::", "--quiet"], root, True)
        run(
            [
                "git",
                "commit",
                "--only",
                "-m",
                "feat: add auto-morph state",
                "--",
                str(AUTO_RS_PATH),
            ],
            root,
            True,
        )
        committed = True
    finally:
        if not committed:
            write_atomic(auto_rs, original)
            print(f"restored {AUTO_RS_PATH} after add-morph failed", file=sys.stderr)


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: add_morph.py <n1_...song_code>", file=sys.stderr)
        return 1

    try:
        add_morph(sys.argv[1])
    except AddMorphError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    except subprocess.CalledProcessError as error:
        print(
            f"error: {error.cmd[0]} failed with exit code {error.returncode}",
            file=sys.stderr,
        )
        return error.returncode

    print("added and committed auto-morph state")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
