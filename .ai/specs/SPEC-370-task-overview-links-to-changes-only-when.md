---
yulius-task: b88061bb-2253-4c04-a490-249793c064fc
status: done
---

# SPEC-370: the task overview links to changes only when the task has a worktree

Status: done
Date: 2026-11-01
Owner: Yulius task #353
Scope authorization: task #353, Fixes #945 (found by the review of #943, SPEC-343).

## Goal

The task overview offers the changes view ("Review changes" in its head, "N files · review" in its
Changes tile) only when the task's worktree is on disk, and otherwise says why ("No worktree yet",
"Worktree gone"), as the Git board does since SPEC-343, instead of a link that can only answer
"Couldn't load the changes".

## Context

- Symptom: on `/s/<slug>/tasks/<N>` of a task not started yet, or whose worktree was removed, both
  links open `/s/<slug>/git/t/<N>`, whose `GET /api/tasks/{tid}/changes` answers 409.
- Root cause (`pages/taskinfo.js`): the head's button is built once when the page mounts, whenever
  `task.branch` is set, and `renderHead()` never redraws it; the Changes tile links whenever the
  Git overview row has `files_changed > 0`. Neither reads the row's `worktree` (`present`,
  `not_created`, `gone`; SPEC-343 WTG-001), which the page already has through
  `ctx.store.gitRowOf` and redraws on every `pulls` event. SPEC-343 left these two links to #945;
  no test opened the overview of a task without a worktree.
- What else the cause could break: nothing else links to `/git/t/<N>` from a task: the Git board
  card already follows the rule (SPEC-343 WTG-002).

## Requirements

- **TOW-001** The head's "Review changes" button shows only when the task's Git overview row is
  read and its `worktree` is `present` (or absent, as from an older server, which SPEC-343 reads as
  `present`). While the overview is still loading, when the task has no row (no branch, or archived:
  the overview lists live tasks only) and when the worktree is `not_created` or `gone`, there is no
  button. It follows each new read.
- **TOW-002** The Changes tile links "N files · review" only under the same condition. With a
  `not_created` or `gone` worktree its line reads "N files · No worktree yet" / "N files · Worktree
  gone" (or the reason alone with no changed files), as plain text; its diffstat stays.
- **TOW-003** The wording is the Git board's (`WORKTREE_TEXT` in `pages/git.js`), shared, not
  copied.

## Plan

- [x] Spec the fix and reproduce it
- [x] e2e for a gone and a not-created worktree on the task overview (red)
- [x] The overview links to changes only with a worktree (green)
- [x] Independent review approved and findings answered
- [x] Gates green with evidence in the spec
- [x] Full e2e suite green and dev merged before the final review

## Acceptance criteria

- The overview of a task whose worktree is `gone` has no link to its changes and its Changes tile
  says "Worktree gone"; of a task `not_created`, "No worktree yet" (and not "gone").
- While the Git overview is held, the head has no "Review changes" button.
- A task whose worktree is present still offers both links to `/git/t/<N>`.

## Test plan

- e2e (`e2e/tests/git.e2e.ts`) against the mock (`ambient=0`), with the Git overview held by
  `__yuliusHoldGit` so the page is checked before the read lands: for `gone` and `not_created`
  (set on `__yuliusMock.tasks` while held), no "Review changes", no link to `/git/t/12`, the
  reason in the Changes tile; for an untouched task, both links. Written first, failing on dev.

## Decisions

- The button is built once, hidden, and shown or hidden by `renderHead()` (which every `pulls` and
  `task` event reaches, also before the task's detail is read), from the same row as the tile:
  one place decides. `[hidden]` is `display: none !important`, so a hidden button is out of the
  tab order and the accessibility tree.
- No row means no button: a link that may answer 409 is worse than a button that appears when the
  overview lands (usually within the first read). The tile already said "No branch changes yet"
  without a row; it keeps that.
- An archived task has no row (the Git overview lists live tasks), so its overview no longer offers
  "Review changes" (review round 1). Accepted: whether its worktree is still there is unknown to
  the page (repository cleanup removes it later), and a link that may answer 409 is what #945
  removes; "Go to task" still reaches it, and its tile linked nothing before either. Adding
  archived tasks to the overview, or a worktree state to the task detail, would be a separate
  change.
- `WORKTREE_TEXT` moves behind an exported `worktreeMissing(row)` in `pages/git.js`, which
  `app.js` already loads, rather than a new module for one constant (a new asset would also need
  the preload list).
- The tile keeps the diffstat of a branch without a worktree: the numbers come from the branch in the
  sandbox's repository (`delivery.rs`), not the worktree; only the link to the worktree's view goes.

## Evidence

- Red first: `npx e2e run tests/git.e2e.ts` at d3ebdbc1 (test only, dev's page) failed with
  `expected ["Review changes"] to equal []` while the Git overview was held; the corrected test
  (092ad133: a reload per case, visible links only) failed the same way against dev 50f1faf5's
  `taskinfo.js`. With only the tile's change reverted it failed on `expected: text containing
  "Worktree gone"`, observed `16 files · review`.
- After the fix: `npx e2e run tests/git.e2e.ts` 6/6 passed; full suite `npx e2e run` at 7d6a54e0:
  795 passed, 1 skipped (70 files, 1 skipped file), exit 0.
- `scripts/dev ci` at c3565d86 (after `python3 scripts/bootstrap.py`): exit 0; 2929 tests passed,
  16 skipped; repository check, fmt, clippy (Linux and macOS targets), doctests, deny, shear, doc.
  Later commits touch only this spec, `docs/changes.md` and the e2e test.
- `yulius security-scan`: "Security scan disabled: Vidoc is turned off in Yulius"; nothing was
  scanned. Disabled, not passed.
- Independent review in a fresh session: round 1 approved conditionally on the gate, with two
  minor own-code findings (archived tasks lose "Review changes": accepted and documented above;
  the Decisions bullet contradicting the code: reworded) and one nit (axe on the present case:
  added); round 2 approved 7d6a54e0 with no new findings.
- `origin/dev` merged before the final review: already up to date at 50f1faf5.
- Not tested: the real instance's task overview against a removed worktree in a browser (the
  server's `worktree` field is covered by SPEC-343's Rust test; the page by the mock e2e).
