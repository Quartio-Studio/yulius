---
yulius-task: a205ca56-48bc-46a4-b2dd-7a26a444c69d
status: done
---

# SPEC-148: The e2e suite holds under host load; notes, archive, editor and Monaco races fixed

Status: done
Date: 2026-10-15
Owner: Yulius task #134 (part of task #118, SPEC-138 lanes)
Scope authorization: the person's request (task #134). Fix GitHub issues #354, #349, #356 and #394:
make the e2e suite reliable under host load without retries or blanket timeout increases, and
fix the notes, archive, editor and Monaco races found on the way, each test-first. Reproduce next
to CPU-bound processes, then show each file passing 10 consecutive runs under load and the full
suite passing twice in a row under load. Create no new GitHub issue.

## Goal

A red e2e run on a loaded shared VM means a defect, not a busy machine: first loads finish within
the tests' waits because the test server keeps up with a page's burst of requests, the waits that
cover Monaco's cold load are conditions with their own budget, and the product races these tests
met (a second editor made from a half-loaded Monaco) are fixed.

## Context

Reproduced in task #134's sandbox (12 CPUs shared with other tasks) with 24 or 48 CPU-bound busy
loops (`sh -c 'while :; do :; done'`) beside the runs, the 1-minute load average sampled every
30 s and the kernel's `TcpExtListenOverflows` counter read before and after (Evidence).

- **#354, the test server.** The suite served the UI with `python3 -m http.server`. Its listen
  queue holds 5 connections (`socketserver.TCPServer.request_queue_size`), and it speaks HTTP/1.0,
  which closes the connection after every answer. A page loads about 75 modules (`index.html`
  preloads them) and Monaco about 120 more files, six connections at a time per page, each file on
  a new connection, and every worker does so at once. On a loaded machine the server's accept loop
  is not scheduled for a while, the queue overflows, and the kernel drops the SYNs beyond it: the
  browser tries again after 1 s, then 3 s. During a full suite at load 35–53 that passed, the
  sandbox's network namespace counted 491 listen-queue overflows (`TcpExtListenOverflows` counts
  every listener there, other tasks' servers too, so it shows the scale, not this server alone). A benchmark
  of the UI's 201 scripts and stylesheets, three clients of six connections each (like Chromium's
  per-host limit), at load 36: the stock server's slowest request took 1.06–1.51 s in every round
  (a dropped SYN's retransmission) and a round 3.4–4.8 s; with keep-alive and a 1024-deep queue
  the slowest took 0.22–0.50 s and a round 1.8–2.4 s. The same server under load also failed a
  Monaco worker script once (task #114's session: `importScripts … editor.worker … failed to load`).
- **#394, Monaco "depends on UNKNOWN service ICodeLensCache".** Monaco's main chunk
  (`editor-BdtEMBbM.js`) sets `window.monaco` as soon as it runs, long before `editor.main` and
  the chunk that registers five services (`toggleHighContrast-*.js`: ICodeLensCache,
  IInlayHintsCache, treeViewsDndService, ISuggestMemories, actionWidgetService) have run. The first
  call into the API (`StandaloneServices.get`, behind `monaco.editor.*`) fixes the services every
  editor gets. Two callers made that call too early:
  - the product: `loadMonaco()` returned `window.monaco` whenever it existed. A second file opened
    while Monaco loaded (or any second `ensureEditor`) got the half-loaded Monaco and made a second
    editor in the same host before the services existed. Reproduced deterministically by holding
    the services chunk (`browser.route`): two `.monaco-editor` in one host, "Element already has
    context attribute: ed-host", and the five "depends on UNKNOWN service" errors;
  - the tests: `terminal.e2e.ts` (`editorLoaded`), `editor.e2e.ts` (the presence tests' first
    `inEditor` poll and `bobOpens`) polled `window.monaco?.editor.getEditors()` while Monaco loaded.
    With the services chunk held 15 s, that poll alone produced the reported errors; without the
    poll the same page was clean. A busy machine stretches the gap between the two chunks, hence a
    flake under load. Task #114 ruled out the language chunks running first; this is consistent
    with that (they make no API call as they load).
- **#356, `editor.e2e.ts` "a file from the tree opens in an editor tab".** Opening the first file
  loads Monaco (3.8 MB of script to parse); the tab appears only after that, and on a starved
  runner later than the assertion's 5 s. `terminal.e2e.ts` and `filedrop.e2e.ts` already waited
  for that cold load as a condition (SPEC-067 FLK-005), each in its own way.
- **#349, `notes.e2e.ts` "text kept over a reload survives hides and tab switches while the notes
  cannot load".** The kept text was "" because the notepad had not loaded yet, not because it was
  lost: until its first load succeeds a notepad is read-only and empty, and it then restores the
  kept text (`first()`). Every failed load doubles the pause before the next (1, 2, 4, 8… s, at most
  30 s), and the test's own tab switch adds a failed load. Under load the steps before the test
  lets loads succeed take seconds, so the next retry came after the assertion's 5 s. Nothing in the
  code path removes the kept text before a load succeeds (it is read in `first()` and dropped only
  after it is in the box).
- **#349 "a save whose answer was lost…" and #356 archive "quick Archive":** first-load waits
  (`getByRole("status", "Save state")`, `getByRole("heading")`) that timed out, as in #354.
- **Found under load after the first fixes** (the four files 10 times each, 4 workers, load 23–93):
  - `notes.e2e.ts` "the tabs show a grip…" failed 10 of 10 (it passed alone). The pointer rested on
    the "Me" tab, but `document.elementFromPoint` there was a toast: the mock's sandbox browser asks
    for an AWS sign-in 5 s after the start, its toast moves to the top edge while the notepad has
    the keyboard (WCAG 2.4.11, `dom.js` `avoidFocus`) and pauses while pointed at. A fast run
    hovered before it came. The drag and Escape tests point at the same tabs.
  - The "two people in one file" failures of the reproduction at line 313 (Bob closes his Editor):
    one was the page not answering an evaluate within 5 s at load 74, the other Bob still shown
    after 5 s. With the server and Monaco changes the test passed 10 of 10 (in 100 `editor.e2e.ts`
    runs at load 23–93); no ordering fault was found in `presence.js` (a leave waits for the update
    on its way, then goes).
  - `terminal.e2e.ts` "a read-only viewer cannot paste" failed 1 of 10 waiting for the page's
    "Read-only" banner at load about 90; the test then pasted into xterm without waiting for the
    terminal, which comes after that banner.
- **#356 archive "the keyboard stays on its control when the sidebar list is rebuilt"** (the
  Stopped toggle read `aria-expanded="false"` after its second Enter): not reproduced in 25 runs on
  the unchanged code at load 53–73 (5 in the four-file run, then 20 of this test alone). The
  toggle's state is the sidebar's `endedOpen`, changed only by its click and redrawn as it is by
  every rebuild (`layout.js` `groupHead`); a rebuild restores the keyboard to the new toggle
  (`sameControl`). Nothing in the list's rebuild can fold or unfold it, so the failure means the
  second Enter did not reach the toggle; no code path that takes the keyboard after the terminal
  shows was found. A regression test pins the invariant: it rebuilds the list between the two
  presses (and checks it was rebuilt) and fails if a rebuild changes the toggle's state.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| E2L-001 | The e2e runner serves the UI with `e2e/serve.py`: Python's static file handler over HTTP/1.1 keep-alive, with a listen queue of 1024 connections. A connection made while the server is busy waits in the queue instead of having its SYN dropped, and one connection carries several requests. | `scripts/test_e2e_serve.py`; loaded runs (Evidence) |
| E2L-002 | `loadMonaco()` resolves only when all of `editor.main` has run, whatever `window.monaco` says; `ensureEditor()` makes one editor per view even when files are opened while Monaco loads. | `editor.e2e.ts` "a second file opened while the editor loads waits for all of Monaco: one editor, no errors" |
| E2L-003 | Tests touch `window.monaco.editor` only after the Editor shows a Monaco editor (`editorShown` in `support.ts`, the DOM's `.ed-host .monaco-editor`). The Editor's first opening in a test waits for it with the cold load's budget (`MONACO_LOAD_MS`, 60 s, as before in `terminal.e2e.ts` and `filedrop.e2e.ts`); every other wait keeps its default budget. | `editor.e2e.ts`, `terminal.e2e.ts`, `filedrop.e2e.ts`, `places.e2e.ts` |
| E2L-004 | The notes test that keeps text over a reload while loads fail reads the text only after a load that can succeed: once loads may succeed again it switches tabs, which loads the notepad at once, instead of waiting out a backoff that a busy machine stretches past 5 s. | `notes.e2e.ts` "text kept over a reload survives…" |
| E2L-005 | The Stopped toggle keeps its state and the keyboard when the sidebar list is rebuilt while it is folded and focused, and Enter still unfolds it. | `archive.e2e.ts` "the Stopped toggle keeps its state and the keyboard when the list is rebuilt between presses" (fails with a rebuild that resets the toggle) |
| E2L-006 | `notes.e2e.ts` opens every mock page with `browserask=0`, which leaves out the sandbox browser's sign-in request 5 s after the start. Its toast moves to the top while a notepad has the keyboard (WCAG 2.4.11), covers the Notes tabs and stays while pointed at, so a slow run hovered or clicked the toast, not the tab. | `notes.e2e.ts` ("the tabs show a grip…" failed 10 of 10 under load before) |
| E2L-007 | "a read-only viewer cannot paste" pastes only once the terminal shows (`terminalShown`): the page's read-only banner is the sandbox's and comes before the terminal and its first state. | `terminal.e2e.ts` "a read-only viewer cannot paste" |
| E2L-008 | A view whose editor creation failed (Monaco did not load, or making the editor threw) makes it again at the next file opened, as before the creation was shared. | `editor.e2e.ts` "a file opened after the editor could not be made makes it then, once" (fails without it: `editorShown` times out) |
| E2L-009 | An editor whose setup threw after Monaco made it (the selection layer, its commands) is disposed, so the next file opened makes the one editor of the view again instead of a second one in the same host. | `editor.e2e.ts` "a file opened after the made editor could not be set up makes it then, once" (fails without it: "expected 2 to be 1") |

## Plan

- [x] Spec and loaded reproduction of #354, #349, #356, #394
- [x] The e2e server answers a page's burst of requests under load (#354)
- [x] Notes tests wait on the retried load and the save (#349)
- [x] Archive's Stopped toggle and quick Archive hold under load (#356)
- [x] Editor opens a file from the tree under load (#356)
- [x] Monaco opens without UNKNOWN service errors (#394)
- [x] Each file 10 loaded runs (667 of 670 passed; the 3 failures and what they need are in Evidence and #354)
- [x] Full suite twice in a row under load (454 of 454, then 453 of 454: Evidence)
- [x] ADR-0122, architecture, e2e README and changes
- [x] Independent review findings resolved
- [x] Merged into dev on green CI; #349, #356 and #394 closed, #354 left open with what it needs

## Acceptance criteria

- `scripts/test_e2e_serve.py` fails against `python3 -m http.server`'s settings (a burst of 64
  connections to a server that is not accepting yet times out; answers are HTTP/1.0) and passes
  with `serve.py`.
- The #394 regression test fails without the `editorview.js` change (two editors, five "UNKNOWN
  service" errors) and passes with it.
- `notes.e2e.ts`, `archive.e2e.ts`, `editor.e2e.ts` and `terminal.e2e.ts` each pass 10 consecutive
  runs next to CPU-bound processes that leave one CPU-bound thread about a fifth of a CPU, with
  each file's seconds per test at least the reproduction's (where the unchanged code failed),
  `retries: 0` and no sleeps.
- The full suite passes twice in a row under the same load.
- No retry, sleep or blanket timeout increase is added; the one longer budget is Monaco's cold
  load, which existed before and is now one named constant.
- After the merge into `dev`, #349, #356 and #394 are closed with a comment naming the PR; #354
  is closed too, or left open with a comment saying exactly what is still needed.

Outcome: the two loaded-run criteria were not met in full. At the target load 3 of 670 file runs,
1 of 908 full-suite runs and 1 of 230 runs of the final `editor`/`filedrop` failed, each a default 5 s wait that ran out while the page or the
runner's own reads were starved (Evidence). No product defect was found behind them, and removing
them needs a longer budget for first loads, which this task ruled out; #354 stays open for that
decision.

## Test plan

- `python3 -m unittest discover -s scripts -p 'test_*.py'` (part of `scripts/dev check`).
- The #394 regression test, run once without the `editorview.js` change and once with it.
- Reproduction on an untouched copy of `HEAD` (`git archive`), then the same files on this branch:
  `npx e2e run <files> --repeat-each 10` next to busy loops, load average recorded.
- The final proof (the files 10 times, then the full suite twice) next to busy loops whose number
  a controller sets every 20 s so that a 2 s CPU-bound probe gets 0.20 of a CPU (median of the
  last three probes); the probe's share, the loops and the load average are logged, and each
  file's seconds per test are compared with the reproduction's.
- `scripts/dev ci` and `scripts/dev security`.

## Decisions

- **Fix the test server, not the budgets.** Longer budgets or retries hide a stall; the stall was
  the server dropping connections. A queue of 1024 and keep-alive are what any production server
  does; `yulius serve` (axum) already answers HTTP/1.1 with keep-alive. Recorded as
  [ADR-0122](../../docs/decisions/0122-the-e2e-server-keeps-up-with-a-page-under-load.md).
- **No CPU priority for the suite in agent sessions.** Raising priority needs `CAP_SYS_NICE`,
  which a task sandbox has not, and the CI job already runs in the weighted `ci` cgroup leaf
  (ADR-0039). Workers stay bounded by the session's CPU budget (SPEC-082).
- **#394 is fixed in the product and in the tests, not filtered.** It is not vendor-only: our
  loader handed out a half-loaded Monaco, and our tests called its API early. Monaco itself is
  unchanged.
- **Monaco's cold load keeps one named budget.** Parsing 3.8 MB of script on a starved CPU is real
  work the server cannot speed up; it is waited for as a condition (`editorShown`), as two files
  already did, with the same 60 s, now in one place.

- **The proof's load is the starvation of one thread, not the load average.** The load average
  is the host's; a sandbox runs under its own CPU budget, so the same number of busy loops starved
  the suite twice as much when other tasks were busy (see Evidence: "a run starved beyond the
  target"). A controller holds a CPU-bound probe at 0.20 of a CPU instead. The reproduction did not
  measure that share; 0.20 is inferred from its tests' speed, so the proof also compares each
  file's seconds per test with the reproduction's, and does not count where it ran faster.

## Evidence

All in task #134's sandbox (12 CPUs, shared with other tasks), on 2026-10-15. Load came from
busy loops (`sh -c 'while :; do :; done'`) beside the runs, on top of the other tasks' work; the
1-minute load average was sampled every 30 s and `TcpExtListenOverflows` read before and after.
Every run used `CI=true` and the config's `retries: 0`.

**Reproduction on the unchanged code** (an archive of `HEAD` before the changes, `d6489013`).
- Full suite, 2 workers, 24 loops, load 8–53: 444 passed, 1 skipped, in 1134 s. The listen queue
  overflowed 491 times during the run (141426 → 141917).
- `notes`, `archive`, `editor` and `terminal` `--repeat-each 5 --workers 4`, 48 loops, load 33–74:
  316 passed, 4 failed, in 1556 s; 660 listen overflows (141917 → 142577).
  - `editor` "two people in one file…": 3 of 5 failed. One was #394 exactly (afterEach:
    "Ws depends on UNKNOWN service ICodeLensCache" and four more); the other two at the step after
    Bob closes his Editor (line 313): one with "no read completed" (the page did not answer an
    evaluate within 5 s), one still showing Bob after 5 s.
  - `editor` "the task terminal showing late…": 1 of 5 failed, `Editor content` not attached
    after 5 s: Monaco's cold load (#356's editor failure).
  - `archive` (35 runs) and `notes` (175 runs) passed at that load.
  - `archive` "the keyboard stays on its control…" alone, `--repeat-each 20`, 2 workers, load 56:
    20 of 20 passed.
- #394 without load: holding the services chunk 15 s (`browser.route`) while the test polled
  `monaco.editor.getEditors()` gave the five errors; holding it 20 s without the poll gave none,
  and `window.monaco` was set at 5.1 s by `editor-BdtEMBbM.js`, at 24.8 s by `editor.main.js`.
- #349 without load: a copy of the kept-text test that lets the backoff grow (until the mock
  counted 8 note loads) before loads succeed: the text appeared 5.4 s after; the old ending failed
  (`toHaveValue`, observed `""`), the new ending (a tab switch) passed.
- Benchmark of the server alone (201 scripts and stylesheets, 3 clients × 6 connections, load
  36): `http.server` 3.4–4.8 s a round, slowest request 1.06–1.51 s; `serve.py`'s settings
  1.8–2.4 s, slowest 0.22–0.50 s.

**Tests that fail without their change.**
- `scripts/test_e2e_serve.py` with the stock settings: the burst test timed out connecting and
  the keep-alive test read HTTP/1.0 (`10 != 11`); both pass with `serve.py`.
- `editor.e2e.ts` "a second file opened while the editor loads…" without the `editorview.js`
  change: "expected 2 to be 1" (two editors), "Element already has context attribute: ed-host" and
  the five "UNKNOWN service" errors; passes with it.
- `archive.e2e.ts` "the Stopped toggle keeps its state…" with a rebuild that resets the toggle
  (mutation in `renderTasks`, reverted): `toHaveAttribute` failed; passes on the real code.

**A run starved beyond the target** (head `d8896dca`, before the final merge of `dev`). The four
files `--repeat-each 10 --workers 4` next to 40 busy loops, load 61–84, 10:47–12:04: archive 20
of 80 failed, terminal 54 of 130, editor 13 of 110, notes about 10 of the 200 or so that ran
before it was stopped. Not counted as the proof, and not retried as one:
- the same code passed these files at 48 loops in the morning (archive 80/80, editor 100/100,
  terminal 129/130); here each test took 2–2.5 times as long (archive 22.6 s a test against
  8.7 s in the reproduction, terminal 16.8 s against 7.1 s), and a busy loop got 0.11 of a CPU:
  another sandbox was running its e2e suite at the same time (a second `http.server` with a queue
  of 5 was listening, and the namespace-wide overflow counter rose from it), and the sandbox's
  CPU budget, not the load average, decided our share;
- the failures were the runner's reads starving, not the page: e.g. "a read-only viewer's mouse
  selects text", rerun alone at that load, failed with `getByText("Read-only (GitHub: read)")`
  after "2 reads in 5.0 s" while the screen at failure showed that banner;
- `serve.py`'s queue (1024) held nothing (`ss`: Recv-Q 0); its log had only client resets and
  broken pipes (pages closing mid-answer), as often as in the morning's passing run;
- one `editor` run (1 of 10, "a file closed, a folder switched or a page gone…") reported
  Monaco's worker failing `importScripts` with a NetworkError. Holding the worker script 8 s at
  the server while the file was closed or the folder switched gave no error, so its cause is not
  found; it is named in #354's closing comment.

**The proof at the target load** (head `3c0e73f4`: this branch with `dev` merged; same code as
`2f257d5a`, which the review approved). `loaded2.sh`: busy loops that a controller sets every
20 s so that a 2 s CPU-bound probe gets 0.20 of a CPU (median of the last three probes); probe,
loops and load logged every 20 s. `CI=true`, `retries: 0`.
- `notes`, `archive`, `editor`, `terminal` `--repeat-each 10 --workers 4`, 12:22–13:23, 3–162
  loops, load 35–120, probe median 0.19 (10th–90th percentile 0.11–0.33): **667 of 670 passed**
  in 3685 s. Seconds per test, against the reproduction's: archive 10.1 (8.7), terminal 8.0
  (7.1), editor 16.9 (16.1), notes 10.4 (8.8): every file at least as starved as there.
  - `editor` 110 of 110 (#394, #356's editor) and `notes` 350 of 350 (#349).
  - `archive` "a row's quick Archive button…" (repeat 0, the file's first test): "Task #13
    archived." not seen within 5 s (the toast lasts 10 s).
  - `terminal` "another viewer's size fills this box…" (repeat 0): `.taskview .xterm-rows` not
    attached within 30 s; "the wheel scrolls the program…" (repeat 3): the rows still empty after
    5 s. Both are the terminal's first load.
- Full suite (the config's 2 workers), 13:23–13:58, load 47–76, probe median 0.19: **454 passed,
  1 skipped**, in 2109 s.
- Full suite again, 13:58–14:26, load 43–76, probe median 0.19: **453 passed, 1 failed, 1
  skipped**, in 1690 s. `ownership.e2e.ts` "the owner is asked for control…", line 56: "expect
  gave up after 0 reads in 5.0 s" — the runner did not finish one read of the page in 5 s, at a
  probe of 0.17–0.19.
- `TcpExtListenOverflows` rose 1366, 932 and 629 during the three runs; the counter is the
  network namespace's, which another task's suite (`http.server`, queue 5) shared.

So 4 of 1578 runs failed at the target load, against 4 of 320 on the unchanged code at about the
same load (the reproduction), and none of them was one of the failures the issues named, except
#356's quick Archive (1 of 10). Each was a default wait that ran out on a starved first load or
read, with no product defect found; a longer budget for those waits would remove them, and that
budget is the decision #354 is left open for (ADR-0122, Consequences).

**The final code under the same load** (head `cac33f8c`: E2L-009 and `dev` with #512 merged,
which touched `filedrop.e2e.ts`). `editor` and `filedrop` `--repeat-each 10 --workers 4`,
14:32–15:09, 7–44 loops, load 24–66, probe median 0.205 (0.12–0.30): **229 of 230 passed**;
`filedrop` 110 of 110. `editor` "two people in one file…" (repeat 6), line 406: this page still
showed Bob 5 s after he closed his Editor; one of the poll's reads took 4.6 s (trace step 64), so
it looked at most twice, the first time before Bob's leaving could have arrived. Same kind as the
four above: 5 of 1808 runs at the target load.

**Review round 2** (fresh session, head `2f257d5a`): APPROVE with five low findings, resolved:
the 0.20 share is said to be inferred and checked against the reproduction's speed (above); ADR-0122
states the load the suite holds at; #356's comment says the Stopped toggle did not reproduce;
E2L-009 (a half set-up editor is disposed) fixed test-first; E2L-008's red run recorded: without
`ensureEditor`'s reset both "…makes it then, once" tests timed out in `editorShown` (60 s), and
without E2L-009's disposal the set-up one failed "expected 2 to be 1".
