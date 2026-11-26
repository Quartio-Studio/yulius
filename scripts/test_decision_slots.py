"""The repository's Markdown layout is a contract with Git's ordinary union merge."""
from functools import partial
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import unittest

from check_repository import ROOT, decision_errors
from decision_slots import extend, slot_errors


def fixture_git(repo, *args):
    return subprocess.check_output(
        ["git", "-c", "user.name=Test", "-c", "user.email=test@example.invalid",
         "-c", "commit.gpgSign=false", "-c", "core.hooksPath=/dev/null", *args],
        cwd=repo, text=True, stderr=subprocess.PIPE,
    ).strip()


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

    def test_authors_must_share_the_new_block_before_filling_neighbors(self):
        # Dev has since acquired the empty block. Both branches still fork from the old
        # base, so comparing to dev's current file alone would accept the broken pair.
        base = extend("# Decisions\n")
        extended = extend(base, through=465)
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            git = partial(fixture_git, repo)

            git("init", "-q")
            (repo / "scripts").mkdir()
            shutil.copyfile(ROOT / "scripts/decision_slots.py", repo / "scripts/decision_slots.py")
            index = repo / "docs/decisions/README.md"
            index.parent.mkdir(parents=True)
            index.write_text(base)
            git("add", ".")
            git("commit", "-qm", "initial slots")
            shared = git("rev-parse", "HEAD")
            index.write_text(extended)
            git("commit", "-qam", "reserve next block")
            dev = git("rev-parse", "HEAD")
            git("update-ref", "refs/remotes/origin/dev", dev)
            for number in (465, 466):
                git("checkout", "-q", "--detach", shared)
                point = f"<!-- ADR {number:04} -->"
                author = extended.replace(point, f"- [{number:04}]({number:04}-choice.md) Entry.\n" + point)
                index.write_text(author)
                git("commit", "-qam", "fill a slot beyond the fork point")
                self.assertEqual(slot_errors(author), [])
                refused = subprocess.run([sys.executable, "scripts/decision_slots.py"], cwd=repo,
                                         capture_output=True, text=True)
                self.assertNotEqual(refused.returncode, 0)
                self.assertIn(str(number), refused.stderr)
                self.assertEqual(index.read_text(), author)
                git("checkout", "-q", "--detach", dev)
                index.write_text(author)
                subprocess.run([sys.executable, "scripts/decision_slots.py"], cwd=repo, check=True)
                self.assertEqual(index.read_text(), author)
                git("restore", "--", "docs/decisions/README.md")

    def test_replenishing_an_unterminated_marker_keeps_it_on_its_own_line(self):
        base = extend("# Decisions\n").rstrip("\n")
        self.assertEqual(extend(base), base)
        self.assertEqual(extend(base, through=465), extend(base + "\n", through=465))

    def test_repeated_legacy_number_with_different_valid_links_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            for n in (399, 400):
                (directory / f"{n:04}-choice.md").write_text(f"# {n:04}. Choice\n")
            (directory / "README.md").write_text(
                "- [0399](0399-choice.md) A\n- [0400](0400-choice.md) B\n- [0400](0399-choice.md) C\n"
            )
            self.assertTrue(any("0400" in e and "more than once" in e for e in decision_errors(directory)))

    def test_protected_checker_reuses_identical_trees_with_different_parents(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp) / "fixture"
            subprocess.run(["git", "clone", "-q", "--shared", "--no-checkout", str(ROOT), str(repo)], check=True)
            git = partial(fixture_git, repo)

            head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
            git("checkout", "-q", "--detach", head)
            for script in ("check_repository.py", "decision_slots.py"):
                shutil.copyfile(ROOT / "scripts" / script, repo / "scripts" / script)
            decisions = repo / "docs/decisions"
            index = decisions / "README.md"
            layout = index.read_text()
            filled = set(re.findall(r"^- \[(\d{4})\]\(", layout, re.MULTILINE))
            number = next(n for n in re.findall(r"^<!-- ADR (\d{4}) -->$", layout, re.MULTILINE)
                          if int(n) > 403 and n not in filled)
            marker = f"<!-- ADR {number} -->\n"
            index.write_text(layout.replace(marker, ""))
            git("add", ".")
            git("commit", "-qm", "old base without the insertion point")
            old = git("rev-parse", "HEAD")
            index.write_text(layout)
            git("commit", "-qam", "dev shares the insertion point")
            shared = git("rev-parse", "HEAD")
            git("update-ref", "refs/remotes/origin/dev", shared)
            name = f"{number}-tree-evidence.md"
            (decisions / name).write_text(f"# {number}. Tree evidence\n\n## Alternatives\n\n- One: no.\n- Two: no.\n")
            index.write_text(extend(layout.replace(marker, f"- [{number}]({name}) Tree evidence.\n" + marker)))
            git("add", ".")
            tree = git("write-tree")
            verdicts = []
            author_verdicts = []
            for parent in (old, shared):
                commit = git("commit-tree", tree, "-p", parent, "-m", "identical complete tree")
                git("checkout", "-q", "--detach", commit)
                self.assertEqual(git("rev-parse", "HEAD^{tree}"), tree)
                checked = subprocess.run([sys.executable, "scripts/check_repository.py"], cwd=repo,
                                         capture_output=True, text=True)
                verdicts.append((checked.returncode, checked.stdout + checked.stderr))
                authored = subprocess.run([sys.executable, "scripts/decision_slots.py"], cwd=repo,
                                          capture_output=True, text=True)
                author_verdicts.append((authored.returncode, authored.stderr))
                self.assertEqual(git("diff", "--name-only", "HEAD"), "")
            self.assertEqual([code for code, _ in verdicts], [0, 0], verdicts)
            self.assertEqual([code for code, _ in author_verdicts], [1, 0], author_verdicts)
            self.assertIn(f"ADR {number} needs its empty slot on dev", author_verdicts[0][1])
