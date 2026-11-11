---
yulius-task: cce2923e-92b8-4c22-a781-939a1b340f42
status: done
---

# SPEC-452: Nothing is typed into the Editor before its file is there (#1087)

Status: done
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
  gone and the tab is never modified. Monaco's inline-suggestions model, started by the typing,
  then recomputes on the disposed model: the two uncaught `Error: Model is disposed!`
  (`_assertNotDisposed` ← `getVersionId` in its fetch, and ← `getLineLength` in a value derived from
  it), which are the "2 errors logged". They need the typing; without it nothing throws.
- **Who meets it.** A person at the keyboard, not only a test: from the clicked tree row two Tab
  presses (row → "Resize explorer" → "Editor content") put the keyboard in the placeholder while
  the file loads. The pointer cannot reach it: the "Open a file from the explorer." hint covers the
  host until the first file shows.
- **Why the tests missed it.** `editorShown` (`e2e/tests/support.ts`) waits only for
  `.ed-host .monaco-editor`, which existed from `createEditor` on; the other tests wait for the tab
  to be selected before typing, this one did not. On an idle machine the read and join resolve
  before the typing; on a loaded one the join can still be on its way.
- **Reproduction (probe, not load).** The mock's join answered 1.5 s late (or held with
  `__yuliusMock.live.holdJoins = true` and released after typing) fails the test's first check
  every time, with the same two errors.
- **What else the cause broke.** `places.e2e.ts` "the editor keeps open files and unsaved text
  across a sandbox switch, without feeding the other sandbox" types after `editorShown` in the same
  way: it failed in the Yulius CI runs e753bd7a (PR #1150) and ae55e33c (PR #1151) on 2026-11-11
  with the missing `README.md, modified` tab and the same two errors, frame for frame. The fix
  covers it without a change: with no model Monaco builds no view, so `editorShown` now also means
  the first file is in the editor. For the same reason a test that holds the file's read or join
  can no longer wait on `editorShown`: `places.e2e.ts` "a file opened just before a sandbox switch
  shows when the sandbox is back" waits on the held read instead.
- **A second race, in the test only.** With the first one gone the test goes on the moment the file
  is activated, while the activation's look at the disk (`refreshFromDisk`) is on its way. The mock
  answers every request 50-200 ms late, so the test's `repoFile` could move the disk before the
  first edit was answered; a look whose `POST /files/live/disk` then reached the server first found
  saved text, which follows the disk (SPEC-411 EDT-020): the typing landed on the disk's text and
  no conflict was ever asked (`README.md Saving… Ln 1, Col 9`, never "Changed on disk"; 2 of 16
  probe runs on a loaded machine). That is the app working as designed, and the order `conflictOn`
  already keeps since #422: the server has the typing before the disk moves. Reproduced every time
  by answering the first edit 3 s late.
- **Not the cause.** The `hold=` on `/files/roots`: it holds only that GET, and `loadRoots` does not
  block `loadFile`.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| FE-001 | Before the Editor's first file is in it, the editor has no model: no "Editor content" textbox to type into, and nothing Monaco owns that can be disposed under a pending request. | e2e "the editor offers nothing to type into until its first file is in it" |
| FE-002 | Once the file is there, typed text marks its tab modified, as before. | same test; the unskipped #1087 test |
| FE-003 | The #1087 test runs again and waits on the open tab, not on Monaco's node, before typing. | `editor.e2e.ts`, 10 runs green under the probe |
| FE-004 | The #1087 test moves the disk only once the server's document has the typed text. | 10 runs green with the first edit answered late; 3 of 3 fail without the wait |

## Plan

- [x] Spec names the cause: typing before the file lands goes into Monaco's placeholder model
- [x] Regression test: typing while the file's join is held loses nothing and logs no error
- [x] The editor starts without a model, so nothing can be typed before the file is set
- [x] The quarantined test waits for the open tab and runs again, 10 runs green under the probe
- [x] The #1087 test moves the disk once the server has the typing: green under a late first edit
- [x] Independent review answered and gates green

## Acceptance criteria

- With the live join held, the Editor offers no "Editor content" textbox; after release, typing
  marks `README.md, modified`, and no page error is logged.
- The #1087 test is unskipped and passes 10 runs in a row under each probe: the join answered
  late, the file's read answered late, the first edit answered late.
- The rest of `editor.e2e.ts` and `live-editor.e2e.ts` pass.

## Test plan

- New e2e in `e2e/tests/editor.e2e.ts`: hold the join, open README.md, wait until the join is
  asked (the editor is made, the file read), the textbox is absent; release, the tab is selected,
  type, the tab shows modified (`failOnPageErrors` fails on the two errors). Fails before the fix
  (the textbox is there).
- The #1087 test, unskipped, waits for `aria-selected="true"` on the README.md tab before typing,
  and for the typed text in the mock's document (`liveDoc`) before it moves the disk.
- The proof uses probes in `mock.js`, never committed: the join answered 1.5 s late, the file's
  read answered 1.5 s late, the first edit answered 3 s late; ten runs of both tests under each.
  The same probes with the fix or a wait taken out show each one is needed.
- The Editor-opening e2e files (`editor`, `live-editor`, `places`, `terminal`, `filedrop`) run
  under each probe, to find other tests that depended on the old timing.

## Decisions

- **No model rather than a hidden host.** `model: null` at `monaco.editor.create` removes the only
  thing that could take typing or be disposed; hiding the host until the first `activate` would
  still leave Monaco's placeholder model and its listeners. A later file opened after `closeAll`
  already shows the empty pane while it loads, so the first file now behaves like those.
- **Typing in the gap is not kept.** It has no place in a file that is not read yet; with no
  textbox there is nothing to type into, so nothing is lost silently.
- **The test waits on the tab**, the condition the sibling tests use, not on a sleep.
- **The second race is ordered in the test, not changed in the app.** Saved text following the disk
  while a person's first keystroke is on its way is EDT-020 as designed (the typing is kept, on the
  newer text); only the test needs the conflict, so it waits until the server has the typing, as
  `conflictOn` does.
- **`editorShown` keeps its name and gains a meaning.** It now returns only once the first file is
  in the editor; its comment and `e2e/README.md` say so, and say what a test that holds the read or
  join waits on instead.

## Evidence

All e2e runs from `e2e/` with `E2E_TELEMETRY_DISABLED=1 npx e2e run …`, on the shared machine at a
load average of 18-60. Probes are uncommitted edits of `web/assets/mock.js`, restored after each
run.

| What | Result |
| --- | --- |
| Both #1087 tests (`--grep "a folder chosen while the worktrees are read\|the editor offers nothing to type into"`), head 9defdf9e, join answered 1.5 s late | 10 of 10 runs: 2 passed |
| The same, the file's read answered 1.5 s late | 10 of 10 runs: 2 passed |
| The same, the first edit answered 3 s late | 10 of 10 runs: 2 passed |
| Dev's code and test (skip lifted), join late | 3 of 3 runs fail: the `README.md, modified` tab is missing, "app 2 errors logged" (the issue's failure) |
| `model: null` taken out, join late | the regression test fails 3 of 3 (the "Editor content" textbox is there) |
| The test without the `liveDoc` wait (d1abcc25), first edit 3 s late | 3 of 3 runs fail at the "Changed on disk" check with `README.md Saving… Ln 1, Col 9` |
| `editor`, `live-editor`, `places`, `terminal`, `filedrop` (111 tests), head 9defdf9e, no probe | 111 passed |
| The same five files, join late | 111 passed |
| The same, first edit late | 111 passed |
| The same, read late | 110 passed, 1 failed: `terminal.e2e.ts` "file names a shell prints open the editor", at the Shell's `toBeFocused`, before any file is read (see below) |
| The same five files of dev, join late | 106 passed, 1 skipped, 3 failed: the places test named in Context (the #1087 cause; passes on this branch) and the two below |
| Whole e2e suite, at an earlier head of this branch (before 508f11fa) | 903 passed, 1 skipped (`agent.e2e.ts`), 1 failed: `notes.e2e.ts` "a conflict on Me is always with the person's own other tab, even under an older login"; passed on a re-run, no Editor in it |
| `yulius security-scan` | "Security scan disabled": not run, so not passed |
| `scripts/dev ci` at d9becda2, in the task sandbox | doctor, check, lint, macos passed; nextest: 3281 passed, 3 failed, the `first_message` tests that fail in every task sandbox and pass in Yulius CI (#1154; this change has no Rust); doctests, deps and docs did not run after that. Yulius CI's `quality` job runs the whole gate on the delivered head |

Two failures met that this change does not cause (neither opens the Editor before the failing
line; both also fail on dev's copy). They are reported on #1087, since this task opens no issues:

- `terminal.e2e.ts` "file names a shell prints open the editor": the Shell's textarea is not
  focused in time (once on this branch with the read late, once on dev's copy with the join late;
  5 of 5 pass with no probe).
- `places.e2e.ts` "each sandbox comes back with its own tool panel and the same Superagent
  session": the "Tool panel" region is still visible after the Superagent button closes it (once on
  dev's copy, once in 5 runs on this branch with no probe).

Review: round 1 at 52383b1c asked for changes (a places test waited on `editorShown` while holding
the read: fixed in 508f11fa; the "redundant `aria-selected` wait" nit is kept, it is the condition
the test types on). The second round approves the delivered head and is named in the merge.
