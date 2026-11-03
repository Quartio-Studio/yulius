---
yulius-task: cb65828b-58a2-4d3e-a62e-a5a2336cd079
status: done
---

# SPEC-414: The explorer keeps a drag's drop target across a redraw (#1029)

Status: done
Date: 2026-11-03
Owner: Yulius task #396
Scope authorization: task #396 (fix the flaky e2e test "files dropped on a folder of the explorer
are added to it"; fix the app if the race is there). Fixes #1029.

## Goal

The e2e test "files dropped on a folder of the explorer are added to it" passes every time. The
explorer row under a file drag keeps its `ed-drop-target` mark while the drag lasts, also when
the explorer redraws its rows during the drag.

## Context

- `filedrop.js` `attachEditorDrop` marks the folder row under a file drag. On each `dragover`,
  `mark(row)` adds `ed-drop-target` to that row element and keeps a reference to it. It clears
  the mark when the drag leaves or drops. The overlay label is set in the same handler, so the
  class is never set later than the overlay's text (#1029's first guess).
- `editorview.js` `renderTree()` builds every `.ed-row` again and replaces the tree's children.
  When the Editor is shown, `show()` starts `loadDir('')` and `loadRoots()` together. When the
  `files/roots` answer arrives after the listing, `applyAccess()` calls `renderTree()`. The new
  `src` row has no mark, and `mark` still holds the detached old row. `refreshTree`, `loadDir` of
  another folder and `activate` (a tab chosen) redraw the tree in the same way.
- The test dispatches one `dragenter` and one `dragover`, waits for the overlay text and then
  reads the class once. When the roots answer lands between the `dragover` and that read (under
  full-suite load), the read sees the new row, which has no class. A poll in the test would not
  help: without another `dragover` the new row never gets the mark. In a real browser,
  `dragover` repeats about every 50 ms, so the mark only flickers, but the state is still wrong.
- The mock's `hold=<path>` keeps the GET requests of a path waiting until
  `__yuliusMock.release()`, and `heldWaiting` counts the requests waiting at the hold (SPEC-092).
  With this, a test can make the roots answer land after the `dragover`, every time.
- The shared first-load wait helper (task #394 for #354, #525 and #510) has not merged yet. This
  test waits for a treeitem that the mock lists without a delay, so it does not need the helper.

## Requirements

- FDT-001. While a file drag is over a folder row of the explorer, that folder's row carries
  `ed-drop-target`, also after the explorer redraws its rows during the drag. The mark is kept by
  the folder's path, not by a row element.
- FDT-002. When the drag leaves the explorer or drops, no row carries `ed-drop-target`, and a
  later redraw does not bring the mark back.
- FDT-003. The e2e test waits for the mark (`toHaveClass` or a poll), and does not read it once.

## Plan

- [x] Spec with the root cause of #1029
- [x] Regression test: a roots answer landing after the dragover keeps the mark
- [x] Explorer keeps the drop mark by path across redraws
- [x] Filedrop test waits for the mark; 30 repeats and the full suite pass
- [x] Docs and evidence: changes, e2e README, spec
- [x] Independent review answered

## Acceptance criteria

- The regression test holds `files/roots`, drags files over `src`, releases the roots read and
  waits until the folder picker lists the roots. It then expects `src` to be marked. It fails on
  `dev` (fc492692) and passes with the fix.
- After the drop (or a leave), no `.ed-drop-target` row remains, also after a redraw.
- `npx e2e run tests/filedrop.e2e.ts --repeat-each 30` passes, and the full e2e suite passes.

## Test plan

- e2e (`filedrop.e2e.ts`): the existing explorer drop test, with the mark awaited. A new test
  makes a redraw land during the drag (the roots read held, then released), then checks the mark,
  the drop, and that no mark remains after a further redraw. The new test is run against the
  unfixed app first, to see it fail.
- Repeat: the filedrop file with `--repeat-each 30`, and the full e2e suite once.

## Decisions

- The fix is in the app, not only in the test. A poll in the test cannot see a mark that the app
  never puts back on the redrawn row. The explorer owns its rows, so it draws the mark the way it
  draws `active`: `renderTree()` adds `ed-drop-target` to the row whose path is the drop target.
  The drop zone tells the explorer that path (`markTarget(path | null)`) and no longer keeps a
  row element. Alternatives: a `MutationObserver` in `filedrop.js` that marks the new row again
  (more moving parts, and a frame late), and leaving the app as it is with only a test poll
  (still fails when the redraw lands after the `dragover`). Both were rejected. This is a local
  choice, recorded here and not as an ADR.

## Evidence

- Reproduced first: `npx e2e run tests/filedrop.e2e.ts --grep 'stays marked when the explorer
  redraws'` against the unfixed app (dev fc492692 plus the new test) failed at
  `filedrop.e2e.ts:82`. The roots read had been released, the `src` row was a new element
  (line 81 passed), and its class was `"ed-row"` only. With the fix, the whole file passes
  (13 of 13).
- `npx e2e run tests/filedrop.e2e.ts --repeat-each 30`: 390 of 390 passed, and all 13 tests
  passed all 30 runs, both explorer drag tests included (20 min, `YULIUS_CPUS=4`).
- Full e2e suite (`npx e2e run`): 800 passed, 1 skipped, 1 failed. The failure was
  `start-change.e2e.ts:67` ("a prepared task changes to run after a task…"): `.strip-status`
  did not show "Scheduled · tomorrow" within 5 s under full-suite load. This change touches
  nothing in the task strip or the start menu. That file passed 15 of 15 alone with
  `--repeat-each 5`. It is outside this change, and the task may not open issues, so it is
  reported here and in the pull request.
- `scripts/dev ci` (after `python3 scripts/bootstrap.py`): exit 0. nextest: 3023 tests run,
  3023 passed, 16 skipped.
- `yulius security-scan`: disabled ("Vidoc is turned off in Yulius"), nothing scanned. This is
  not a pass.
- The shared first-load helper (task #394) has not merged. This test waits for a row the mock
  lists at once, so it does not need the helper.
- Independent review (fresh session) of 4ea977fc: approved. Its minor finding: a redraw that
  removes the row under the pointer may skew the drop zone's enter/leave count, so a drag that
  leaves the window might not clear the mark. That count is the drop zone's pre-existing leave
  handling (`filedrop.js` `dragleave`), and the overlay then stays up as well, naming the same
  folder, so it is left as it is. Its nit (no default for `markTarget`) is declined: `reveal`,
  `refresh` and `open` are required in the same way.
