---
yulius-task: cad237f1-6b2d-4d2b-b269-527b733985a5
status: done
---

# SPEC-386: ADR-0160's SPEC-201 amendment wraps like the rest of the file (#612)

Status: done
Date: 2026-11-02
Owner: Yulius task #368
Scope authorization: the open-issues automation (task #368): solve GitHub issue #612 in one pull
request into `dev` saying `Fixes #612`. Create no new GitHub issue; comment on #612 instead.

## Goal

The over-long lines of
`docs/decisions/0160-a-sandbox-s-engine-comes-from-its-own-system-not-the-host-s.md` are
rewrapped to the file's usual width (about 96 characters), with no text changed. Documentation
only; nothing behaves differently.

## Context

- Line 55, in the bullet "The install brings buildx and compose" (SPEC-201's amendment, #610), is
  141 characters: "...so they normally have them). Either install writes to the sandbox's own
  system, so". Review round 3 of task #186 left it open at the round limit (#612).
- Line 9 (in the Status section) is 143 characters: the amendment note for ADR-0162 was appended
  to the SPEC-201 note's line.
- Line 8 is 105 characters: "Amended on 2026-10-16 by" plus one Markdown link of 80 characters
  (its target alone is 68). A link target cannot be split, and line 7 already holds a link on its
  own, so line 8 stays as it is.

## Requirements

- **RWR-001** No line of the bullet "The install brings buildx and compose" is longer than 98
  characters; inline code spans stay on one line.
- **RWR-002** The Status section's ADR-0162 amendment starts its own line, so no Status line is
  longer than line 8's unsplittable link.
- **RWR-003** No word of the file changes: `git diff --word-diff=porcelain` shows no added or
  removed words.

## Plan

- [x] Spec the rewrap
- [x] ADR-0160's long lines rewrapped with no word changed
- [x] Independent review approved and findings answered
- [x] Gates green with evidence in the spec

## Acceptance criteria

- `awk 'length($0) > 98'` on the ADR prints only line 8 (105, an unsplittable link).
- `git diff --word-diff=porcelain origin/dev -- <ADR>` has no `+`/`-` word lines.
- `scripts/dev ci` is green.

## Test plan

Whitespace-only documentation change: no test can fail without it. The proof is the word diff and
the line-length check above, plus the full gate (docs checks included).

## Decisions

- Wrap greedily at 98 characters with code spans kept whole, matching the bullet's existing
  lines (92-98), rather than reflowing the whole bullet: only the bullet's lines 55-56 change.
- Leave line 8 at 105 characters: splitting it would mean a line holding only "Amended on
  2026-10-16 by", which reads worse than one long link line. No ADR: nothing lasting is decided.

## Evidence

- `git diff --word-diff=porcelain origin/dev -- docs/decisions/0160-*.md`: no added or removed
  words; the reviewer also found the old and new ADR identical with whitespace collapsed.
- `awk 'length($0) > 98'` on the ADR: only line 8 (105). Lines 55-56 (141 and 83) are now 94, 96
  and 35 characters; line 9 (143) is now 56 and 86. `git diff --check`: clean.
- `scripts/dev ci` at 6260e5d4 (after `python3 scripts/bootstrap.py` in this fresh worktree):
  exit 0, 2952 tests run, 2952 passed, 16 skipped.
- `yulius security-scan`: "Security scan disabled" (Vidoc is off for this project); nothing was
  scanned, so it is not a pass.
- Independent review (fresh read-only session, `om-code-review`) approved 6260e5d4 with one minor
  and three nit findings, all own code and all fixed in the next commit: the link's length (80,
  target 68), "Line 9 (in the Status section)", "the bullet's lines 55-56", and the changelog
  entry's wrap and brackets. Nothing pre-existing was found.
