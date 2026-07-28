import subprocess
import tempfile
import unittest
from pathlib import Path
from typing import Sequence

from scripts.add_morph import AddMorphError, add_morph, append_code


SOURCE = """\
const AUTO_STATES: &[&str] = &[
    "n1_first",
    // add-morph inserts new entries immediately above this line.
];
"""


class FakeRunner:
    def __init__(self, fail_on: tuple[str, ...] | None = None) -> None:
        self.commands: list[tuple[str, ...]] = []
        self.fail_on = fail_on

    def __call__(
        self, command: Sequence[str], root: Path, check: bool
    ) -> subprocess.CompletedProcess[str]:
        del root
        recorded = tuple(command)
        self.commands.append(recorded)
        if self.fail_on is not None and recorded[: len(self.fail_on)] == self.fail_on:
            if check:
                raise subprocess.CalledProcessError(1, command)
            return subprocess.CompletedProcess(command, 1)
        return subprocess.CompletedProcess(command, 0)


class AddMorphTests(unittest.TestCase):
    def test_append_uses_the_scoped_marker(self) -> None:
        source = SOURCE + "\nconst OTHER: &[&str] = &[];\n"

        updated = append_code(source, "n1_second")

        self.assertIn('"n1_first",\n    "n1_second",', updated)
        self.assertTrue(updated.endswith("\nconst OTHER: &[&str] = &[];\n"))

    def test_duplicate_is_rejected(self) -> None:
        with self.assertRaisesRegex(AddMorphError, "already in AUTO_STATES"):
            append_code(SOURCE, "n1_first")

    def test_missing_or_repeated_marker_is_rejected(self) -> None:
        with self.assertRaisesRegex(AddMorphError, "insertion marker"):
            source = SOURCE.replace(
                "    // add-morph inserts new entries immediately above this line.\n",
                "",
            )
            append_code(source, "n1_second")
        with self.assertRaisesRegex(AddMorphError, "insertion marker"):
            append_code(SOURCE + SOURCE, "n1_second")

    def test_failed_validation_restores_the_source(self) -> None:
        runner = FakeRunner(("cargo", "test"))
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            auto_rs = root / "auto.rs"
            auto_rs.write_text(SOURCE)

            with self.assertRaises(subprocess.CalledProcessError):
                add_morph("n1_second", root=root, auto_rs=auto_rs, run=runner)

            self.assertEqual(auto_rs.read_text(), SOURCE)

    def test_commit_is_limited_to_auto_rs(self) -> None:
        runner = FakeRunner()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            auto_rs = root / "auto.rs"
            auto_rs.write_text(SOURCE)

            add_morph("n1_second", root=root, auto_rs=auto_rs, run=runner)

        self.assertEqual(
            runner.commands[-1],
            (
                "git",
                "commit",
                "--only",
                "-m",
                "feat: add auto-morph state",
                "--",
                "src/fluid/auto.rs",
            ),
        )

    def test_dirty_target_is_rejected_before_writing(self) -> None:
        runner = FakeRunner(("git", "diff", "--quiet"))
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            auto_rs = root / "auto.rs"
            auto_rs.write_text(SOURCE)

            with self.assertRaisesRegex(AddMorphError, "uncommitted changes"):
                add_morph("n1_second", root=root, auto_rs=auto_rs, run=runner)

            self.assertEqual(auto_rs.read_text(), SOURCE)


if __name__ == "__main__":
    unittest.main()
