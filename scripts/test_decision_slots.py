"""The repository's Markdown layout is a contract with Git's ordinary union merge."""
from pathlib import Path
import subprocess
import tempfile
import unittest

from check_repository import ROOT, decision_errors
from decision_slots import extend, slot_errors


class DecisionSlots(unittest.TestCase):
    def test_distinct_entries_merge_in_order_and_competing_descriptions_fail(self):
        source = (ROOT / "docs/decisions/README.md").read_text()
        # Replay neighboring author edits against their actual markers, even after both
        # numbers are occupied on dev. The legacy author operation appended at EOF.
        first, second = 401, 402
        source = "".join(line for line in source.splitlines(keepends=True)
                         if not line.startswith(("- [0401](", "- [0402](")))

        def add(index, number, description):
            marker = f"<!-- ADR {number:04} -->\n"
            entry = f"- [{number:04}]({number:04}-choice.md) {description}\n"
            return index.replace(marker, entry + marker) if marker in index else index + entry

        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            def merge(ours, theirs):
                for name, content in (("base", source), ("ours", ours), ("theirs", theirs)):
                    (directory / name).write_text(content)
                return subprocess.check_output(
                    ["git", "merge-file", "-p", "--union", "ours", "base", "theirs"],
                    cwd=directory, text=True,
                )

            lower = add(source, first, "An author's description.")
            higher = add(source, second, "Another description with *Markdown*.")
            expected = add(lower, second, "Another description with *Markdown*.")
            self.assertEqual(merge(lower, higher), expected)
            self.assertEqual(merge(higher, lower), expected)

            decisions = directory / "decisions"
            decisions.mkdir()
            (decisions / f"{first:04}-choice.md").write_text(f"# {first:04}. Choice\n")
            conflict = merge(lower, add(source, first, "Competing description."))
            self.assertIn("An author's description.", conflict)
            self.assertIn("Competing description.", conflict)
            (decisions / "README.md").write_text(conflict)
            self.assertTrue(any("more than once" in e for e in decision_errors(decisions)))

    def test_an_appended_row_cannot_pass_even_when_the_numbers_are_sorted(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            (directory / "0401-choice.md").write_text("# 0401. Choice\n")
            markers = "".join(f"<!-- ADR {n:04} -->\n" for n in range(401, 465))
            (directory / "README.md").write_text(markers + "- [0401](0401-choice.md) An intact row.\n")
            self.assertTrue(any("immediately before" in e for e in decision_errors(directory)))

    def test_two_branches_replenish_the_same_block_at_the_shared_boundary(self):
        base = extend("# Decisions\n")
        lower = base.replace("<!-- ADR 0433 -->", "- [0433](0433-one.md) First in block.\n<!-- ADR 0433 -->")
        higher = base.replace("<!-- ADR 0464 -->", "- [0464](0464-two.md) Last in block.\n<!-- ADR 0464 -->")
        expected = extend(lower.replace("<!-- ADR 0464 -->", "- [0464](0464-two.md) Last in block.\n<!-- ADR 0464 -->"))
        self.assertTrue(any("replenish" in e for e in slot_errors(lower)))
        self.assertEqual(slot_errors(expected), [])
        self.assertEqual(extend(expected), expected)
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            for name, content in (("base", base), ("lower", extend(lower)), ("higher", extend(higher))):
                (directory / name).write_text(content)
            for ours, theirs in (("higher", "lower"), ("lower", "higher")):
                merged = subprocess.check_output(
                    ["git", "merge-file", "-p", "--union", ours, "base", theirs], cwd=directory, text=True,
                )
                self.assertEqual(merged, expected)

    def test_missing_or_repeated_insertion_points_fail_before_any_entry_uses_them(self):
        index = extend("# Decisions\n")
        marker = "<!-- ADR 0410 -->\n"
        for invalid in (index.replace(marker, ""), index.replace(marker, marker + marker)):
            self.assertTrue(slot_errors(invalid))
        self.assertFalse(slot_errors(index))
