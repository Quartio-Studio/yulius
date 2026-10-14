---
yulius-task: 5913c266-de69-48a1-a8cf-6d2ee4e9b31b
status: in-progress
---

# SPEC-089: the repository check catches committed conflict markers

Status: in progress
Date: 2026-10-14
Owner: task #74
Scope authorization: issue #364, "The repository check does not catch committed merge conflict
markers", requested through task #74.

## Goal

A merge that commits unresolved conflict markers fails `scripts/dev check` (and so Yulius CI),
naming each file and line, instead of passing every gate until a reviewer happens to read it.

## Context

- Task #66 merged `dev` into its branch (commit c9548da) and committed unresolved markers in
  `docs/api.md` (a `HEAD` side, a separator and an `origin/dev` side). `scripts/check_repository.py`
  and the rest of `scripts/dev ci` passed; an independent review found them later (#364).
- In Rust the markers break compilation, but Markdown, JavaScript, CSS, SQL, TOML and Python files
  under `scripts/` have no gate that reads them as a whole.
- `check()` in `scripts/check_repository.py` already lists every tracked and untracked, not ignored
  file (`git ls-files --cached --others --exclude-standard`) for the Markdown link check; the new
  check reads the same list.
- `docs/changes.md` and `docs/decisions/README.md` merge with Git's union driver
  (`.gitattributes`), which never writes markers; every other file can receive them.
- `crates/yulius-server/web/assets/vendor/` holds the vendored Monaco build: third-party, minified,
  not edited here. Today no tracked file, vendored or not, has a line that looks like a marker.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| CMK-001 | The repository check fails on a listed file with a line that is a Git conflict marker: seven `<`, `\|` or `>` followed by a space or the end of the line, or exactly seven `=`. A trailing carriage return is ignored. Each error names the file and the 1-based line. | `ConflictMarkerTests.test_each_marker_fails_with_file_and_line`, `test_crlf_marker_fails` |
| CMK-002 | Lines that only resemble markers pass: a marker not at the start of the line, a run of eight or more of the character, seven `=` followed by more text. | `ConflictMarkerTests.test_lookalikes_pass` |
| CMK-003 | Vendored files (`crates/yulius-server/web/assets/vendor/`) and binary files (any NUL byte) are not read for markers; a file listed but gone from the disk is skipped. | `ConflictMarkerTests.test_vendored_binary_and_missing_files_are_skipped` |
| CMK-004 | The shipped repository passes the check. | `python3 scripts/check_repository.py` |

## Plan

- [x] Spec SPEC-089 written and valid
- [x] Repository check fails on committed conflict markers, with regression tests
- [x] Docs and changes log describe the new check; full gate green
- [ ] Independent review of the exact head approved, findings fixed

## Acceptance criteria

- A tracked file with a conflict marker line makes `python3 scripts/check_repository.py` exit
  non-zero with `<path>:<line>: unresolved merge conflict marker ...`.
- The same file without the markers, a vendored file with them and a binary file pass.
- `scripts/dev check` passes on the branch; the checker's unit tests cover marker and clean files.
- `docs/engineering/development.md` lists the check under `scripts/dev check`.

## Test plan

- Unit tests in `scripts/test_check_repository.py` (`ConflictMarkerTests`) on temporary files:
  every marker kind (start, diff3 base, separator, end) reported with its line; CRLF; look-alikes;
  vendored, binary and missing files skipped. Marker text in the tests is built (`"<" * 7`) so the
  test file itself never holds a marker line.
- Manual negative run: commit-free, write a marker into a tracked file, run the checker, see it
  fail with the file and line, restore the file.
- Full gate: `scripts/dev check`, `scripts/dev lint`.

## Decisions

- **Exact markers, not a looser pattern.** Git writes seven characters and a label (or none for
  the separator); `conflict-marker-size` is not set in this repository. Matching exactly seven
  keeps Markdown rules, setext underlines and ASCII art of other lengths legal. A Markdown setext
  heading underlined with exactly seven `=` would fail; it is rare and the fix (any other length) is
  trivial, while missing a real separator is the defect being fixed.
- **The diff3 base marker counts too.** `merge.conflictStyle = diff3`/`zdiff3` adds a seven-`|`
  line; agents and people may use either style.
- **Exclude only the vendored directory.** Generated copies (`.agents/skills`, `.claude/skills`,
  `Cargo.lock`, `apm.lock.yaml`) can receive markers in a merge just like sources, so they are
  checked. The vendored Monaco build is third-party output we never merge by hand.
- **In the repository check, not a pre-commit hook.** Hooks are local and skippable; the check runs
  in Yulius CI on every pull request head, which is what blocks a merge. No ADR: this extends an
  existing gate without a new contract.
