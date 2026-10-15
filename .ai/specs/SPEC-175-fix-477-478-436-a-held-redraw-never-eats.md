---
yulius-task: 852f7d16-0305-4c22-a61f-84329970160c
status: done
---

# SPEC-175: A held redraw never eats a click or a drag; one toast per onboarding start

Status: done
Date: 2026-10-15
Owner: Yulius task #136, issues #477, #478, #436 (part of task #118, SPEC-138)
Scope authorization: task #136: "Fix GitHub issues #477, #478 and #436 of Quartio-Studio/yulius
[…] All three are web UI events whose redraw or remount arrives in the middle of a person's
gesture."

## Goal

Three web UI defects, each a redraw or remount landing in the middle of a gesture, are gone:

- A plain click on an Editor tab while a strip redraw is held back activates the tab in every
  browser (#477, SPEC-127 EDT-011). The review's test gaps of SPEC-127 round 2 are covered.
- A drag from the new-task prompt that a redraw interrupted no longer drops as a copy of itself
  (#478, SPEC-124).
- Start onboarding shows one toast per start, also when the person left the sandbox's page and came
  back while it ran (#436, SPEC-111).

## Context

- **#477.** `assets/tabdrag.js` holds the Editor strip's redraws while a tab is pressed
  (`dragging`). A press that ends without moving calls `cancelled()` at `pointerup`, and the
  Editor's `cancelled` redraws the strip if a redraw was held (`tabsStale`). `pointerup` comes
  before `click`, so the tab element that got `mousedown` is replaced before the click. The issue
  saw Firefox drop that click; Chromium drops it too when the redraw moves another tab under the
  pointer (the e2e test below recorded no click on any tab), so the tab is not activated. The
  release after a drag already defers its flag with `setTimeout(…, 0)` (`dragged`). Notes passes no
  `cancelled`. The round-2 review also listed missing tests: a leave naming a link to a folder
  (`symlink_metadata` in `App::file_presence`), `createPresence().destroy()` with an update on its
  way, and `retryable()` refusing 403/404/409/422 while retrying 429.
- **#478.** `assets/promptinput.js` `draw()` forgets a drag started in the prompt (`moving`): its
  offsets may be stale and its source node is replaced. The drag still carries the `text/plain` set
  at `dragstart`, so a drop inside the box afterwards takes the "text from elsewhere" path and
  inserts a copy: the dragged text or file is duplicated. A thumbnail arriving in the draft redraws
  through `prompt.refresh()` (`taskview.js` `draftChanged`); the existing test covers only an edit.
  SPEC-124's Evidence says such a drag "must be started again", which the code did not do.
- **#436.** `taskview.js` `approveOnboarding` toasts "Onboarding started." unless its own composer
  is opening the task (`opening`, `sentToTask`). Left and mounted again meanwhile, the first
  composer is destroyed and its flags are false, while the new composer opens the task and toasts
  "Onboarding started: opened #N.": two toasts. Starts in flight are already module-level
  (`onboardingStarts`); the "a page opens it" knowledge is not.

## Requirements

- **HRD-001 (#477).** A press on an Editor tab that ends without a drag runs the redraw held back
  during the press after the browser's `click` for that release (a task queued at `pointerup`), so
  the element pressed is the one clicked and the tab is activated. A press released off its tab
  (no click) still redraws then. A click on a tab closed during the press chooses nothing (review
  round 1). A cancelled press (pointercancel, Escape) is unchanged.
- **HRD-002 (#477).** Tests cover: a leave (`selection: null`) naming a link to a folder is relayed
  (the link is not followed); a presence destroyed with an update on its way sends its leave once
  that update is answered and arms no timer; refusals 403, 404, 409 and 422 are not sent again while
  429 (and a lost request) is. The wording in `tabdrag.js` and SPEC-127 says "during a press".
- **HRD-003 (#478).** `dragstart` in the prompt marks the drag with a data type of its own (one per
  prompt box). Over that box, a drag carrying its own mark that it no longer tracks (a redraw forgot
  it) is refused: `dropEffect = 'none'` on `dragover`, and a `drop` changes nothing. Text dragged in
  from elsewhere, another prompt included, is still inserted as a copy; a drag still tracked still
  moves.
- **HRD-004 (#436).** A module-level, per-sandbox marker says that a page is opening the sandbox's
  onboarding task and will say so ("Onboarding started: opened #N."). `approveOnboarding` toasts
  "Onboarding started." only when no page set it during the start; the marker is cleared when a
  start begins and ends, and when the page that set it gives up opening. One start, one toast.
- **HRD-005 (#436).** The comment above `gateNews` says what the live region now announces
  (onboarding finished), and `openNewTask` reads the sandbox from the store
  (`store.sandboxById(sandbox.id)`), not the object it was handed.

## Plan

- [x] Spec, scope and issue checks
- [x] An Editor tab clicked during a held redraw is activated (#477)
- [x] Presence test gaps covered: folder link leave, destroy, refusals (#477)
- [x] A prompt drag interrupted by a redraw is refused on drop (#478)
- [x] One toast per onboarding start, and the app.js/comment nits (#436)
- [x] Docs, specs and changes updated
- [x] Gates green and independent review passed

## Acceptance criteria

- `editor.e2e.ts`: a tab pressed while another tab closes (a held redraw) and released in place is
  the element that receives the click, is still connected then, and is activated. Fails without
  HRD-001 (no click reaches a tab). A press released below the strip still redraws it, and a
  tab closed while pressed is not chosen by the release (review round 1).
- `files.rs`: a leave naming `src-link` (a link to `src/`) answers 204 and is relayed.
- `editor.e2e.ts` (the page's own `presence.js`): `destroy()` with an update held sends exactly one
  leave after the answer and arms no timer; 403/404/409/422 arm no retry, 429 and a lost request
  arm one.
- `inline-images.e2e.ts`: a selection dragged in the prompt, a thumbnail arriving in the draft
  mid-drag (`prompt.refresh()`), then dropped in the box with the same drag data: `dropEffect` is
  `none` on `dragover` and the text is unchanged. Fails without HRD-003 (the text is duplicated).
- `onboarding.e2e.ts` "leaving and coming back while Start onboarding runs …" counts exactly one
  toast, "Onboarding started: opened #1.", once the start has ended (its task read held, then
  seen). Fails without HRD-004 (two toasts).

## Test plan

- e2e (mock server): the cases above, each run red before its fix and green after; then the touched
  files and the full suite.
- Rust: `files::presence_is_refused_when_malformed` extended with the link to a folder.
- Gates: `scripts/dev check`, `scripts/dev lint`, `scripts/dev test`, `scripts/dev security`.

## Decisions

- **#477: defer the held redraw with `setTimeout(cancelled, 0)`.** `click` follows `pointerup` in
  the same input task in every engine, so a task queued at `pointerup` runs after it. Keeping
  `dragging` true until then would need a second state for the release; listening for `click`
  would miss a release outside the tab (no click fires). The press's `drag` state ends at
  `pointerup` as before, so a redraw asked for between release and click (none today) draws at once.
- **#478: refuse, not copy.** The issue allows either. A copy duplicates what the person meant to
  move; a refused drop leaves the text as it was, and the person drags again. A per-box type
  (`application/x-yulius-prompt-<random>`) is used because `dragover` can read types but not data:
  a drag from another prompt (another tab of the app) is still text from elsewhere.
- **#436: a per-sandbox marker set by the opener, read by the starter.** The opener sets it only
  when it will toast (`offered`); a page that opens the task silently (mounted after the sandbox
  was preparing) leaves the starter's "Onboarding started." as the one toast. It replaces
  `sentToTask` and the starter's `opening` check, which only knew their own mount.
- No ADR: these are local corrections of SPEC-124, SPEC-127 and SPEC-111 behavior, recorded here.

## Evidence

- **Issues on `dev` first:** none of #477, #478, #436 was fixed (`tabdrag.js` still called
  `cancelled()` at `pointerup`, `promptinput.js` had no own drag type, `approveOnboarding` checked
  `opening`/`sentToTask`).
- **Red before each fix** (the new test against the unfixed code, Chromium):
  - `editor.e2e.ts` "a tab clicked while a redraw is held back …": `expected [] to equal
    [{"same":true,"connected":true,"path":"AGENTS.md"}]` (no click reached a tab).
  - `inline-images.e2e.ts` "a drag from the prompt that a thumbnail interrupted …": `expected
    "alpha beta gammaalpha[shot.png]" to be "alpha beta gamma[shot.png]"`.
  - `onboarding.e2e.ts` "leaving and coming back …": the toasts were `["Onboarding started: opened
    #1.", "Onboarding started."]`.
  - Review round 1: "a press released off its tab …" failed without the `tabs.includes(tab)` guard
    (the closed tab was selected and a page error logged) and with the deferred redraw removed
    (the closed tab stayed in the strip).
- **Coverage tests, mutation-checked:** the in-page presence test fails when 403 is retried or a
  destroyed text's leave is retried (a timer left armed); `files::presence_is_refused_when_malformed`
  fails when `symlink_metadata` becomes `metadata` (`src-link: … presence names a file`); the drag
  test fails on `effect: "copy"` when only `dragover` loses the refusal; the toast test fails with
  the starter always toasting.
- **Flaky test met on the way:** `scripts/test_vidoc_scan.py` `RealRunner` failed the gate twice
  under load with `ProcessLookupError` in `alive()`: a child reaped between opening and reading its
  `/proc/<pid>/stat`. Reproduced every run with a 0.5 s delay between the open and the read; with
  the fix (`ProcessLookupError` is "not alive") the same probe passes.
- **Gates:** `scripts/dev ci` exit 0 (after the flake fix); full e2e suite
  `E2E_TELEMETRY_DISABLED=1 npx e2e run`: 513 passed, 1 skipped (56 files) before the review fixes;
  the review's new and changed tests then passed 4 times in a row with the touched files.
  `scripts/dev security`: **not run**, exit 2 (Vidoc rejected the key, HTTP 401, #470); unrun under
  ADR-0104, not passed.
- **Independent review** (fresh session, round 1): APPROVE with four minors, all fixed test-first:
  a tab closed during a press could be chosen by the release (guard in `editorview.js`); the
  deferred redraw without a click untested; `dropEffect` unasserted; the toast count could be read
  before the start ended.
- **Known limit, not changed here:** when the start cannot read the tasks, it says "Onboarding
  started." and a page that opens the task later says "opened #N" (two toasts, as before this
  change; noted on #436).
- Not testable here: Firefox and Safari (the e2e suite runs Chromium only).
