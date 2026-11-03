---
yulius-task: 5a102cb9-9349-4aac-b6a3-3ee049a49bf0
status: done
---

# SPEC-412: e2e: one named, longer wait for first loads, and the races behind #354, #525 and #510

Status: done
Date: 2026-11-03
Owner: Yulius task #394
Scope authorization: the person's goal (task #394). Fix GitHub issues #354, #525 and #510 in one
pull request into `dev` (`Fixes #354`, `Fixes #525`, `Fixes #510`). The person decided #354's open
question on 2026-11-03: option (a) of its latest "Not actionable yet" comment, one named, longer
wait for first loads only; option (b), accepting about 1 failure in 360 runs, is rejected because
flaky required checks block merges. Every other wait keeps its budget and `retries: 0` stays.
Create no new GitHub issue.

## Goal

A test's first wait for the page it opened has a budget that a starved runner can meet, named
once in `e2e/tests/support.ts` beside `MONACO_LOAD_MS` and `TERMINAL_SHOW_MS`, and the races the
three issues found behind other timeouts are fixed, so these tests fail only for a defect.

## Context

- **#354 (open since SPEC-148).** After `e2e/serve.py` (ADR-0122), about 1 run in 360 at a fifth
  of a CPU per thread still failed a default 5 s wait on a starved first load or read: archive's
  quick Archive (its file's first test), terminal first loads, ownership, editor presence. Later
  comments add `pause.e2e.ts` "the Tasks list row menu…" at load 56 and 190. SPEC-148 and
  ADR-0122 left the remedy, a longer budget for first loads, to a person; this task has that
  decision.
- **What already has a named budget.** `MONACO_LOAD_MS` (60 s, `editorShown`) and, since
  ADR-0251, `TERMINAL_SHOW_MS` (30 s, `terminalShown`), the terminal's first screen. What has
  none: the mock's install, which every `app.open` and `browser.reload` waits for
  (`waitForTheMockOnEveryLoad`, 5 s), and each test's own first look at the page (5 s).
- **#525.** Four tests at load ~45:
  - `rail.e2e.ts` "the tools rail keeps its items inside it…": the mock's sandbox browser asks for
    a sign-in 5 s after the start (`browser.action_requested`), which renames the rail's Browser
    item "Browser, an agent needs your action". A run slower than 5 s reads the new name. A race,
    not a budget: `&browserask=0` leaves the request out (as `notes.e2e.ts` does, E2L-006).
  - `terminal.e2e.ts` "a click beside another viewer's grid…": the rows were empty at the first
    `toHaveText` (5 s): the terminal's first screen, which `terminalShown` covers. With 30 rows,
    the awaited line scrolls out about 21 s after the hello, so the test opens the mock with a
    transcript that stays still (`still=1`, FLD-009).
  - `queued-messages.e2e.ts` "a waiting task takes a message…": the first wait after `app.open`.
  - `onboarding.e2e.ts`: the Add sandbox dialog's repository radio did not show within the
    action's 30 s, at load 15 too (task #136). No race was found by task #297, nor by this task
    (Evidence): its wait is already 30 s, so the first-load budget does not apply to it.
- **#510.** At load ~78:
  - `consistency.e2e.ts`, `places.e2e.ts` (three tests) and `pipeline.e2e.ts` (three tests):
    first waits after `app.open` (a heading, a closed-task banner, the Superagent's first screen,
    the Dashboard's first state).
  - `notes.e2e.ts`: the races reproduced below; "the tabs show a grip…" and "Team comes first…"
    were fixed by #523 and #703 (`browserask=0`, `ambient=0`).
- **The notes races, reproduced with probe delays** (a 9 s pause in the page where a starved
  runner would be slow, just before the test turns the mock's failure off; 4 workers, no load):
  - "a notepad is read-only until it is loaded…" failed (`toHaveValue` saw ""): a failed first
    load is retried after 1, 2, 4, 8… s (`notes.js` `retryLater`, doubling up to 30 s), so after
    a long failing stretch the next try comes after the test's 5 s.
  - "a save made while a refused one is looked into…" failed (`noteLoads` 3, expected 4): the
    test meant its own blur to be the save refused after the lost answer, but the pad's own retry
    (1 s after the lost answer) was refused first, looked at the server unheld and settled, so
    the blur's save was a plain save and nothing was looked into.
  - "an undo after a lost answer is not Saved…" failed (`noteLoads` never rose): the same retry
    settled the lost save before the test undid it, so the undo was a plain save and the server
    was never checked.
  - "undoing a save whose answer was lost…", "a save whose answer was lost…" and "text kept over
    a reload…" passed with the probe. "a failed reload after a teammate's edit backs off" waits
    for the pad's backed-off reload (its next try at 7 s, then 15 s) within 5 s of turning loads
    back on, which a starved runner can miss the same way.
  - None is a product defect: retrying with growing pauses is the pad's contract (SPEC-041).
- **#354's `pause.e2e.ts` failure.** The row's actions show while the row is hovered; the mock's
  ambient activity regroups the Tasks list under a still pointer, so the hover sticks to another
  row and the action stays hidden for the click's 30 s (the cause #703 found for notes). The
  test does not need the ambient activity: `&ambient=0`.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| FLD-001 | `e2e/tests/support.ts` names one budget for a page's first load, `FIRST_LOAD_MS` (30 s), and one helper, `firstShown(locator, text?)`, which waits within it until the locator shows (with `text`, until it has that text). It is for the first look at a page after `app.open` or a reload only; every later wait keeps its budget and `retries: 0` stays. | `support.ts`; `mock-ready.e2e.ts` |
| FLD-002 | `app.open` and `browser.reload` wait for the mock's install within `FIRST_LOAD_MS`, not 5 s. | `support.ts` |
| FLD-003 | A regression test holds a page's first content past 5 s: `firstShown` passes and a default wait for the same thing fails. | `mock-ready.e2e.ts` |
| FLD-004 | The tests the three issues list wait for their first screen with `firstShown`, or with `terminalShown` where it is a task terminal's first screen. | the listed files |
| FLD-005 | The rail test opens the mock without the sandbox browser's sign-in request (`browserask=0`); the Tasks list pause test without ambient activity (`ambient=0`). | `rail.e2e.ts`, `pause.e2e.ts` |
| FLD-006 | The notes tests that turn a failure off then trigger the next load at once (a tab switch, which loads), instead of waiting out a back-off a starved run stretches; tests that need the test's own save to be the refused one hold the server's answer to the pad's look from before the lost answer, so the pad's own retry cannot settle it first. Each passes with the probe delay that failed it before, and the back-off test still proves the pad retries by itself. | `notes.e2e.ts` (removing the retry after a teammate's edit fails "…backs off") |
| FLD-007 | A running subject's waterfall, drawn again at every poll, shows the tip of the row under a still pointer again after the redraw (`stages.js` `loadWaterfall`). | `pipeline.e2e.ts` "the hover tip of a running run stays through the waterfall's redraw" (fails without it: the tip hidden) |
| FLD-008 | Terminal tests that look at a task terminal's rows right after `app.open` wait for its first screen first (`terminalShown`). | `terminal.e2e.ts` |
| FLD-009 | Terminal tests that read or point at the transcript's lines open the mock with `still=1`: the mock agent prints no more of its script after the first screen (`mock.js` `TermSession.ensureTicking`; typed input, its echo and canned replies still print), instead of being stopped after the runner's next read. Stopped late, a starved run's transcript had scrolled on, so a classic run had scrollback and its first turn up offered no whole session. | `terminal.e2e.ts` (#911's tests: a 15 s probe right after `app.open` fails 2 of 2 with a hold after the first screen, passes 2 of 2 with `still=1`) |

## Plan

- [x] Spec, and the notes races reproduced with probe delays
- [x] A named first-load wait with its regression test
- [x] Notes back-off races fixed
- [x] Rail, pause and terminal races fixed
- [x] #354's listed tests wait for their first load
- [x] #510's and #525's listed tests wait for their first load
- [x] ADR, e2e README, architecture and changes
- [x] Affected files repeated under load, numbers recorded
- [x] Load-run findings fixed: waterfall tip, terminal first screens
- [x] Changed files repeated under load again
- [x] The whole-session offer holds under load (transcript held in the page)
- [x] Independent review findings resolved
- [x] Gates green

## Acceptance criteria

- The regression test of FLD-003 fails with a 5 s wait and passes with `firstShown`.
- Each notes test of FLD-006 fails with its probe delay before the change and passes with it after.
- The affected files pass repeated runs (`--repeat-each`) next to CPU-bound load holding one
  thread at about 0.2 of a CPU, with the numbers in Evidence and the pull request; no retry, sleep
  or general budget increase.
- `scripts/dev ci` passes; the e2e suite passes in Yulius CI.

## Test plan

- Probe delays (a pause inside the page at the await point) for each race, before and after.
- `npx e2e run <files> --repeat-each N --workers 4` with and without busy loops held so that a
  CPU-bound probe gets about 0.2 of a CPU (SPEC-148's method), recording passes, failures and
  seconds per test.
- `scripts/dev ci`, `scripts/dev security`.

## Decisions

- **One named budget, 30 s, for first loads (the person's decision on #354).** Like the terminal's
  `TERMINAL_SHOW_MS` and the runner's action budget, not Monaco's 60 s: a page's first load is
  modules and in-page mock reads, lighter than Monaco's parse. Recorded as ADR-0262, amending
  ADR-0122.
- **A helper the test calls, not a longer `app.open`.** No single sign says a page's first screen
  is done for every route (some tests hold reads on purpose, `hold=`), so the test names what it
  waits for and the budget covers that one wait. `app.open` itself only gets the budget for the
  mock's install, which is part of every first load.
- **Races are fixed, not covered by the budget.** The rail, pause, terminal margin and notes
  failures have causes a longer wait would hide or not help; each is removed in the test.
- **Notes: trigger, don't wait out the back-off.** A product change (no back-off after a failure
  is cleared) is not possible: the pad cannot know the server is back. A tab switch is what a
  person does and loads at once (E2L-004's approach).

## Evidence

All in task #394's sandbox (12 CPUs shared with other tasks, `YULIUS_CPUS=4`), 2026-11-03,
`CI=true`, the config's `retries: 0`, 4 workers. "Probe delay" means a pause inside the page at an
await point, where a starved runner is slow; "held load" means busy loops that a controller sets
so that a 2 s CPU-bound probe gets about 0.2 of a CPU (SPEC-148's method), logged every 6–20 s.

**Red, then green (each change).**
- FLD-002/003: with `FIRST_LOAD_MS = 5_000`, "opening a page waits for a mock that loads later than
  5 s" failed (`expect.poll` timed out after 5000 ms) and "the first look at a page waits…" failed
  (`toBeVisible`, no node); with 30 s, 3 of 3 pass.
- Notes (FLD-006), a 9 s probe before each `setMock(…, false)`, unchanged tests: "read-only until
  loaded" failed (`toHaveValue` saw ""), "a save made while a refused one…" failed (`noteLoads` 3,
  not 4), "an undo after a lost answer…" failed (`noteLoads` never rose); "…backs off" with a 5 s
  probe failed 2 of 3. Changed tests: the ten probed tests 10 of 10; `notes.e2e.ts` 36 of 36.
  Removing `retryLater(load)` after a teammate's edit fails "…backs off" (review M1).
- Rail (FLD-005), a 7 s pause after the rail shows: unchanged, failed with #525's exact list
  (`"Browser, an agent needs your action"`); with `browserask=0`, 3 of 3.
- Pause (FLD-005), a 9 s pause between the hover and the click on #8's row: unchanged, 1 of 2
  failed with #354's exact "element is not visible: .tl-row[data-task-id="tsk_studio_8"] …"; with
  `ambient=0`, 3 of 3.
- Terminal first screens, the hello held 7 s (`__yuliusHoldTerminal`): unchanged 0 of 4 (the
  margin, wheel, other viewer's size and read-only mouse tests); with the waits, 4 of 4.
- Waterfall tip (FLD-007): the new test failed 2 of 2 (`.st-tip` hidden after the redraw); with
  the `stages.js` change, 3 of 3; a 6 s probe before "a busy CI run…"'s hover failed 2 of 2 before
  and passes 2 of 2 after (`browserask=0` did not help, so it was not kept).
- Whole-session offer (FLD-009): a 15 s probe before the old `holdStill` in `classicTask12` failed
  "the attention bar's Reply…" 2 of 2 ("tap did not become actionable", the offer never shown).
  An in-page hold after the first screen (round 2 of the review) still failed with a 15 s probe
  right after `app.open` ("the whole session is read page by page…" and "…Reply…", 2 of 2): the
  hello starts the printing before any hold from the runner. With `still=1`, 2 of 2 pass with that
  probe, and `terminal.e2e.ts` 30 of 30.

**Loaded run 1** (head `291a79fd`, 14:09–15:10): the 15 affected files `--repeat-each 5`; probe
median 0.181 (10th–90th percentile 0.09–0.36), load 13–77. **1251 of 1260 passed**, 3661 s.
Seconds per test (a file's reported time over its runs): archive 7.4 (8.7 in SPEC-148's
reproduction), notes 9.8 (8.8), terminal 11.3 (7.1), so at least as starved as there.
- Every file the issues list passed every run but two: archive 45/45, consistency 55/55,
  menus 145/145, onboarding 70/70, ownership 25/25, agent-panel 165/165, pause 20/20, rail 15/15,
  places 105/105, queued-messages 105/105, notes 180/180, task-done 95/95, mock-ready 15/15.
- pipeline 67/70: "a busy CI run…" failed 3 of 5, with an empty hover tip: FLD-007, fixed.
- terminal 144/150: four were first waits on the rows right after `app.open` (FLD-008, fixed);
  "a read-only viewer's clipboard…" (the runner's read of the rows after a print timed out) and
  "the way on sits below a held card…" (the offer hidden) were later waits.

**Loaded run 2** (head `81297787`, 15:25–16:10): terminal, pipeline and notes `--repeat-each 5`;
probe median 0.175 (0.07–0.37), load 37–82, seconds per test pipeline 14.0, terminal 13.4, notes
15.0. **399 of 405 passed**: pipeline 75/75, notes 180/180, terminal 144/150. Five of the six were
#911's whole-session tests (the offer never shown: FLD-009, fixed); the sixth, "file paths in the
task terminal…" (1 of 5), waited 5 s for xterm to report a hovered OSC 8 link, a later wait.

**Loaded run 3** (head `25450ec5`, `still=1`; 16:30–16:52): terminal and pipeline
`--repeat-each 5`; probe median 0.156 (0.07–0.43), load 17–79. **225 of 225 passed**: terminal
150/150 (8.9 s a test), pipeline 75/75 (9.2 s a test). Then the two read-only tests that still
read a ticking transcript got `still=1` too (review round 3): terminal and pipeline 45 of 45
without load.

So the affected files ran 1881 times under the held load: 1251 + 399 + 225 passed, 15 failed in
runs 1 and 2, each traced to a cause fixed afterwards (FLD-007, FLD-008, FLD-009) or a later wait
named below; run 3 of the files those fixes touched had no failure.

**Review** (fresh sessions, read-only, no test runs while the load ran).
- Round 1 (head `291a79fd`): request changes. M1, the back-off test no longer proved a retry:
  fixed (lower bound; the mutation fails it). M2, onboarding and editor presence unaddressed:
  onboarding's no-op wait removed and both explained below and in the changes entry. Lows: the
  ADR's wording for an `app.open` to another route and for its 5 s claim, the Team pad's load
  held by mistake (now waited for), the architecture's wording: all fixed.
- Round 2 (head `82ae706c`): request changes. M-A, the in-page hold still came after the hello:
  replaced by `still=1` (red/green above). L-B, the redraw wait got the poll's budget
  (`RUN_POLL_MS + 5_000`). Unused types removed; the changes entry names what is not covered.
- Round 3 (head `25450ec5`): approve. Minor: two read-only tests still read a ticking transcript:
  `still=1` added. Nits (wording of `still=1`, a stale spec line, a comment's place, a long line):
  fixed.

**Not covered, and why.**
- `editor.e2e.ts` "two people in one file…" (#354's last item from SPEC-148): Bob still shown 5 s
  after he left, a later wait on a starved runner; no ordering fault was found (SPEC-148), and the
  person's decision gives first loads only a longer budget. Not run here.
- `onboarding.e2e.ts`'s repository radio (#525): not reproduced by probe delays after the page's
  load, after the mock flag and after the click, nor under held load (70 of 70); the home renders
  no Import button before its sandboxes load, so no early click lands on a moved button. Its wait
  is already the action's 30 s, so `firstShown` would change nothing and is not used there.
- Later waits that a starved runner still misses (the clipboard read, the held card, the OSC 8
  hover): kept at their budgets as decided.

**Gates** (head `34480c37`: this branch with `dev` merged, `fcb85e94`; after `scripts/bootstrap.py`).
- `scripts/dev ci`: exit 0. nextest 3029 passed, 16 skipped; `unittest discover -s scripts` 152
  tests OK; doctests ok; `cargo deny` advisories, bans, licenses and sources ok; `cargo shear` clean.
- `cd e2e && npx e2e run` (the whole suite, `CI=true`, the config's 2 workers, host load 20–45):
  804 passed, 1 skipped, in 1406 s.
- `yulius security-scan`: "Security scan disabled: Vidoc is turned off in Yulius": disabled, not
  passed; nothing was scanned.
