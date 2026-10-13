---
yulius-task: 5fcb2f23-f971-48d3-b9bb-fee39c950142
status: in-progress
---

# SPEC-067: Flaky tests wait on the condition they check, under load

Status: in progress
Date: 2026-10-13
Owner: Yulius task #56
Scope authorization: GitHub issues #243, #320 (its two e2e cases; the cockpit case is fixed by
SPEC-050 TRM-102), #322, #294 and #269, through task #56 (part of task #47, "fix all open
actionable GitHub issues"). No new GitHub issue is created.

## Goal

Each of the five tests passes on a loaded runner because it waits on the state it checks, or the
code it checks is deterministic. No sleep and no retry is added; `retries: 0` stays.

## Context

Reproduced on 2026-10-13 in a task sandbox (12 CPUs) with 24 to 32 CPU-bound busy loops next to
the runs (load average 26 to 61). Traces are under Evidence.

- **#322, `menus.e2e.ts` "the Markdown slash menu filters too".** 2 of 10 loaded runs failed:
  `expect.toBeFocused` on the slash menu's Filter. The trace shows the menu open with its Filter,
  and `textbox "Task prompt" [focused]`. The test injects a Markdown editor into `/s/studio`, whose
  page is the new-task composer. The composer focuses its prompt in `start()`, when the page is
  mounted after the app's first reads. Under load that happens after the injected editor opened
  its menu, and the composer takes the keyboard from it. The test raced the page's own autofocus.
- **#320, `new-task.e2e.ts` "Auto is the default…".** 1 of 10 failed: `.intake-status` never had
  "Choosing mode and workflow…". The status is transient. It shows 150 ms after Start and goes
  when the creation answers (the mock's Auto choice takes about a second). Under load the fill and
  the click took 2.3 s and 1.3 s, and the page was already on `#/s/studio/t/15` when the assertion
  first read. The test asserted a state that can be over before it looks.
- **#294, `task-done.e2e.ts` "…can be archived and is closed".** 1 of 10 failed: `.strip-status`
  read "Your turn" for the full 5 s instead of "Done · PR #221". This is a product bug. The task
  view's strip derives Done from the sandbox's pull requests (`store.pullsOf`, filled by the
  `/git` read), but `taskview.js` never listens to the store's `pulls` event. The sidebar
  (`layout.js`) and the Tasks page do. When the `/git` read lands after the task view drew its
  strip (and after its detail read), the strip keeps the task's own state until something else
  redraws it. In the mock that is its one browser request, about 5 s after the start, which is why
  the test usually passed and lost the race only under load. In use, a task opened right after a
  reload keeps "Your turn" until its next task event, while the sidebar says Done.
- **#269, `drafts.e2e.ts` "a stored draft drops attachments…".** 1 of 10 failed: the prompt stayed
  empty after the reload. The test writes a stored draft into `localStorage` while the composer is
  open, then reloads. At mount, the composer copies its default choices into its draft
  (`renderChips`: `mode` null → `auto`) and schedules a save 300 ms later (`SAVE_DELAY_MS`). When
  the test writes inside that window (a busy runner stretches it), the pending save, or the
  `pagehide` flush on reload, stores the composer's empty draft. That removes the test's draft.
  The product behaviour is right: the page's draft is the page's. The test raced it.
- **#320, `terminal.e2e.ts` "file paths in the task terminal open the editor…".** It passed 10
  of 10 and then 30 of 30 runs under load. Its sibling "file names a shell prints open the editor",
  which shares its helpers and its ending, failed 1 of 30 (load 48) and, after the first fix
  below, 1 of 20. Both failures ended the same way: the editor tab missing after 5 s, with the
  Editor button `[pressed]`, so the click had opened the Editor. The tab came after the
  assertion gave up (it is in the screen at failure). The assertion had managed only 5 to 8 reads
  in those 5 s. Opening the Editor the first time loads Monaco (many files), and on a starved runner
  that takes longer than the 5 s default. That matches the reported failures (`toHaveAttribute` on
  the `checkout.rs` tab, and a timeout waiting for it). `filedrop.e2e.ts` already waits for that
  load as a condition.
  A second race sits in the same helpers. `hoverText` rests the pointer for a fixed 500 ms
  (`settle()`), then reads once whether xterm shows the pointer cursor. A file link is known only
  after the provider's directory listing answers, so a slow answer reads as "not a link". A click
  made before xterm knows the link lands on plain text.
- **#243, `task_done::a_goal_the_tui_dropped_is_submitted_again`.** In 7 runs under load
  average 18 to 61 (CPU loops, then fsync writers), GOAL SET came after 59.5 to 59.7 s and Done
  0.2 to 1.1 s later. The 60 s is fixed idling, not load. The fake agent sends no SessionStart
  hook, so `wait_input_ready` waits out its hookless fallback (`GOAL_READY_TIMEOUT / 2`, 45 s).
  Then the dropped submit costs the 5 s and 8 s submit waits before the line is typed again
  (`GOAL_SUBMIT_WAITS`). CPU load does not stretch "GOAL SET → Done". The CI failure (90.3 s =
  60 + the 30 s window) happened while the sandbox was 51 % over its disk quota and other runs
  errored for that reason. A write that failed (the fake agent's transcript record of the achieved
  goal, or the store's activity) would leave Done never coming. A longer window would not have
  helped that run. The run's log has expired (`ci_status`: not found).
- **#243, a product race found on the way.** With the SessionStart hook, 3 of 20 loaded runs still
  took 61 s. `start_watcher` cleared `session_started`, and it runs at the end of `attach_run`,
  after the agent process was spawned. Hooks are accepted for any attempt not yet ended. A
  SessionStart answered before the run was attached and watched was wiped. The goal then waited
  out the 45 s hookless fallback. In use that is a fast TUI on a busy host: the task sits with its
  goal untyped for 45 s. The opening turn's flag is already set before the spawn for the same
  reason.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| FLK-001 | The slash menu test injects its editor only once the page's composer has the keyboard, so no later autofocus of the page takes it from the menu (#322). | `menus.e2e.ts` "the Markdown slash menu filters too" |
| FLK-002 | The Auto test holds the task creation (`__yuliusMock.holdCreate`) while it reads "Choosing mode and workflow…", then releases it. The status is checked while it must show (#320). | `new-task.e2e.ts` "Auto is the default…" |
| FLK-003 | The task view redraws its strip when the sandbox's pull requests are read. A task whose pull request is tested reads Done whenever the `/git` read lands, also after the task's detail. The mock can hold that read (`window.__yuliusHoldGit`, set by an init script, since the first read starts at boot) to show it (#294). | `task-done.e2e.ts` "a task view reads Done once its pull request is read late"; "…can be archived and is closed" |
| FLK-004 | The stored-draft test stores its draft before the page opens (an init script), so no open composer can store its own draft over it (#269). | `drafts.e2e.ts` "a stored draft drops attachments…" |
| FLK-005 | The terminal link tests wait until xterm shows a link under the pointer before they assert it or click it, and after the click until the Editor has loaded (Monaco, bounded at 60 s as in `filedrop.e2e.ts`) before they check its tab. A plain-text check keeps a bounded rest: a late answer there can only make it pass, never fail (#320). | `terminal.e2e.ts` "file paths in the task terminal…", "file names a shell prints…" |
| FLK-006 | The dropped-goal test runs the fake agent with its SessionStart hook, as Claude Code sends it, so the goal is typed when the TUI is ready instead of after the 45 s hookless fallback. The test still exercises the dropped submit and the retype. The goal must start within 60 s (16 s measured) and Done follow within 45 s (1.1 s or less measured under load average 61), inside nextest's 120 s limit (#243). | `task_done::a_goal_the_tui_dropped_is_submitted_again` |
| FLK-007 | A new attempt forgets the previous session's SessionStart before its agent is spawned, not when its run is watched. A SessionStart hook answered before the supervisor attached the run still counts, so the goal is typed when the TUI is ready (#243). | `task_done::a_session_start_before_the_run_is_watched_still_types_the_goal_at_once` |

## Acceptance criteria

- AC-1: each e2e case passes 20 consecutive runs (`--repeat-each 20`, `retries: 0`) next to 24 or
  more CPU-bound processes, with the load average recorded.
- AC-2: `task_done::a_goal_the_tui_dropped_is_submitted_again` passes 20 consecutive runs under
  `cargo nextest run --profile ci` next to CPU-bound processes.
- AC-3: FLK-003's new test fails without the `taskview.js` change and passes with it. FLK-007's
  new test fails without the `supervisor.rs` change and passes with it.
- AC-4: the full e2e suite and `scripts/dev ci` pass; Yulius CI is green at the merged head.

## Plan

- [x] Spec: the five flakes reproduced under load, each root cause named
- [x] Slash menu test (#322) types once the page's composer has focus
- [x] New-task Auto test (#320) holds the creation while it reads the status
- [x] Task view strip follows a late pull request read (#294), test-first
- [x] Stored-draft test (#269) seeds the draft before the page opens
- [x] Terminal file-link tests (#320) wait for the link, not a fixed rest
- [x] Dropped-goal test (#243) types after SessionStart, Done bound fits a loaded runner
- [x] Loaded repeat runs pass: 20x each e2e case, 20x nextest ci
- [x] A SessionStart answered before the run is watched still counts (#243), test-first
- [x] Terminal link tests wait for the Editor's first load (#320); loaded reruns pass
- [ ] Docs, changes log, gates green and independent review

## Test plan

- The FLK-003 regression test holds the mock's `/git` read and opens task #11. It waits until the
  detail drew the strip (its steps show) and the mock's one browser request redrew it too, then
  releases the read and expects "Done · PR #221". Without the listener nothing else redraws the
  strip, and it stays "Your turn". This is deterministic, with no load needed.
- Every changed e2e test: `npx e2e run --repeat-each 20` with 24+ busy loops beside it, no retries.
- The Rust test: 20 consecutive `cargo nextest run --profile ci` runs with busy loops beside it.
- FLK-007's regression test makes the race certain rather than likely. A test session host
  attaches a run only once a router layer saw the SessionStart hook answered (bounded at 30 s). The
  goal must then start within 30 s, under the 45 s fallback.
- The full e2e suite and `scripts/dev ci` once at the end.

## Decisions

- **Fix the app where the flake is a product race, the test where it is the test's.** #294 is a
  real stale strip, so the view listens to `pulls`. #243 hid a real lost SessionStart, so the reset
  moves before the spawn. #322, #320 and #269 raced states the product
  is right to have (an autofocus, a transient status, the page's own draft). Their tests now wait
  on the condition, or set up state before the page exists.
- **Hold the mock rather than poll a transient state.** Polling faster can still miss a status that
  lasts less than one poll. Holding the creation, as `drafts.e2e.ts` already does with
  `holdCreate`, makes the status last until the test has seen it. `__yuliusHoldGit` follows the
  terminal's `__yuliusHoldTerminal` pattern for the `/git` read.
- **The Editor's first load gets a condition, not a bigger default.** Raising the e2e config's
  assertion budget for every test would slow every real failure. Only the first opening of the
  Editor loads Monaco, so only those two checks wait for it, as `filedrop.e2e.ts` does.
- **Negative link checks keep a bounded rest.** No signal tells "the provider answered: not a
  link" apart from "not answered yet". A late answer turns a check for plain text into a pass,
  never a failure. Positive checks and clicks wait for the pointer cursor.
- **#243: the SessionStart hook, not a shorter fallback.** Making `GOAL_READY_TIMEOUT` configurable
  only for tests would add a production knob to save test time. Claude Code sends SessionStart, so
  the fake agent sending it is the realistic case. The hookless fallback stays covered by the tests
  that run without it.
- **`hooks_seen` stays as it is here (follow-up).** `start_watcher` also clears `hooks_seen`, which
  an early SessionStart sets. Unlike `session_started`, every later hook (UserPromptSubmit, Stop)
  sets it again, so a wiped value repairs itself at the next hook. Until then a turn end read from
  the transcript is handled as for a hookless run. Moving that reset before the spawn too is a
  separate slice with its own test. It is noted on #243, since this task creates no new issue.
- **No ADR.** Nothing here sets a lasting architectural direction. These are test fixes and one UI
  listener.

## Evidence

To be completed.
