---
yulius-task: cce2923e-92b8-4c22-a781-939a1b340f42
status: in-progress
---

# SPEC-452: Nothing is typed into the Editor before its file is there (#1087)

Status: in progress
Date: 2026-11-11
Owner: Yulius task #434
Scope authorization: task #430 working through open issues: "Solve GitHub issue #1087 … finds the
cause, fixes it and lifts the quarantine." A fix of the Editor (SPEC-181, SPEC-411); no new
requirement, a regression case for the existing one that typed text is never lost.

## Goal

The first file opened in the Editor shows no editable text until that file is in the editor, so
nothing a person types in that gap is lost, Monaco throws nothing, and the e2e test "a folder chosen
while the worktrees are read switches to it once the question is answered" runs again (unskipped)
and passes under load.

## Context

- **Symptom (#1087).** Seen three times in loaded Yulius CI runs (PRs #1086, #1095, #1135): after
  `editorShown`, `unsaved ` was typed, and the `README.md, modified` tab never showed; "app 2
  errors logged". Quarantined in #1135 with `{ skip: 'flaky under CI load: #1087' }`.
- **Root cause.** `loadFile` (`web/assets/editorview.js`) first makes the editor (`ensureEditor` →
  `createEditor` → `monaco.editor.create(host, {…})` with no `model`), then awaits the file's
  read and its live join (`tab.live.join()`), and only then `attach`es the tab's model and
  `activate` → `editor.setModel(tab.model)`. Without a `model` option Monaco makes its own empty
  placeholder model, so for the length of those awaits a visible, focusable, editable "Editor
  content" textbox exists (the host is not hidden before the first file). Text typed there goes into
  the placeholder; `setModel` replaces it and Monaco disposes the placeholder it owns, so the text is
  gone and the tab is never modified. The word-based suggestions Monaco started for the typed word
  then read the disposed model: the two uncaught `Error: Model is disposed!`
  (`_assertNotDisposed` ← `getVersionId` ← the suggest fetch), which are the "2 errors logged".
- **Why the tests missed it.** `editorShown` (`e2e/tests/support.ts`) waits only for
  `.ed-host .monaco-editor`, which exists from `createEditor` on; the other tests wait for the tab
  to be selected before typing, this one did not. On an idle machine the read and join resolve
  before the typing; on a loaded one the join can still be on its way.
- **Reproduction (probe, not load).** Holding the live join in the mock
  (`__yuliusMock.live.holdJoins = true`, released after typing) fails the test's first check every
  time, with the same two errors.
- **Not the cause.** The `hold=` on `/files/roots`: it holds only that GET, and `loadRoots` does not
  block `loadFile`.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| FE-001 | Before the Editor's first file is in it, the editor has no model: no "Editor content" textbox to type into, and nothing Monaco owns that can be disposed under a pending request. | e2e "typing before the first file is in the editor goes nowhere and loses nothing" |
| FE-002 | Once the file is there, typed text marks its tab modified, as before. | same test; the unskipped #1087 test |
| FE-003 | The #1087 test runs again and waits on the open tab, not on Monaco's node, before typing. | `editor.e2e.ts`, 10 runs green under the probe |

## Plan

- [x] Spec names the cause: typing before the file lands goes into Monaco's placeholder model
- [ ] Regression test: typing while the file's join is held loses nothing and logs no error
- [ ] The editor starts without a model, so nothing can be typed before the file is set
- [ ] The quarantined test waits for the open tab and runs again, 10 runs green under the probe
- [ ] Independent review answered and gates green

## Acceptance criteria

- With the live join held, the Editor offers no "Editor content" textbox; after release, typing
  marks `README.md, modified`, and no page error is logged.
- The #1087 test is unskipped and passes 10 runs in a row with the join held at the typing point.
- The rest of `editor.e2e.ts` and `live-editor.e2e.ts` pass.

## Test plan

- New e2e in `e2e/tests/editor.e2e.ts`: hold the join, open README.md, Monaco shows, the textbox
  is absent; release, the tab is selected, type, the tab shows modified (`failOnPageErrors` fails
  on the two errors). Fails before the fix (the textbox is there; the typing is lost).
- The #1087 test, unskipped, waits for `aria-selected="true"` on the README.md tab before typing.
- Its 10-run proof uses the probe (join held across the typing point, released by the test).

## Decisions

- **No model rather than a hidden host.** `model: null` at `monaco.editor.create` removes the only
  thing that could take typing or be disposed; hiding the host until the first `activate` would
  still leave Monaco's placeholder model and its listeners. A later file opened after `closeAll`
  already shows the empty pane while it loads, so the first file now behaves like those.
- **Typing in the gap is not kept.** It has no place in a file that is not read yet; with no
  textbox there is nothing to type into, so nothing is lost silently.
- **The test waits on the tab**, the condition the sibling tests use, not on a sleep.

## Evidence

(filled in when done)
