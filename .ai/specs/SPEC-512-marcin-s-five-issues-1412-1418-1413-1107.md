---
yulius-task: c2fc38ad-443d-48d0-ab7b-549eb0d1e4b0
status: in-progress
---

# SPEC-512: Marcin's five issues: parked sessions, answers from a second person, the digest, Telegram intake, handoffs

Status: in-progress
Date: 2026-11-19
Owner: Yulius task #494
Scope authorization: the request of task #494 (by MrFishPL for the owner, Michal Karp,
2026-11-19): "Fix the five open issues Marcin (`studenciak00owl`) filed on this repository:
#1412, #1418, #1413, #1107 and #1106. Each issue is its own piece of work with its own pull
request or pull requests, delivered in the order below, and each is closed by the pull request
that completes its acceptance criteria." Each issue's recommended option is followed; everything
that widens who can do what stays off by default; the owner rule (GIT-004) stays.

This spec is delivered in five parts, one per issue, each by its own pull request. A part's
requirements carry its issue's prefix: PRK (#1412), ANS (#1418), DIG (#1413), TGI (#1107), HOF
(#1106). Decisions: [ADR-0367](../../docs/decisions/0367-an-idle-session-is-parked-and-resumed-by-a-message.md)
(parking) and the records the later parts add.

## Goal

1. **#1412.** A task whose turn ended and that waits for a person no longer holds a live session:
   after a grace period its session is parked (the process ends, the task stays where the list
   showed it, with its transcript, `needs_you` and delivery) and it resumes in the same session
   when someone writes to it. Only working sessions count toward `max_sessions`; a queued task
   says what it waits for; the Capacity card shows working, parked and queued sessions apart.
2. **#1418.** A `needs_you` can name who it waits for; a named person with write access answers
   it while it is open, on the web and with `task_message`, without taking the task.
3. **#1413.** An optional end-of-day digest, off by default: what waits for the person, the
   actions left in last messages, what was parked, what still works; to the browser and Telegram.
4. **#1107.** The personal Telegram channel's intake, off by default per person: a reply to a
   notification reaches its task, a new message becomes a task, `/status` and `/task`.
5. **#1106.** Tasks hand work to another project of the same instance behind an instance switch
   (off, ask, automatic; off by default) with a per-project opt-out, provenance and audit.

## Context

### #1412 today (read in the code on 2026-11-19)

- **What counts.** `App::sessions_in_use` (`crates/yulius-app/src/supervisor.rs`) counts live
  attempts except those of Done tasks (SPEC-050 SES-101). A task that waits for a person (`needs
  you`, "Your turn", an answered agent that is not Done) holds a slot, and every idle session,
  Done or not, keeps its Claude Code process: about 250 to 300 MB each (#1412's comment: 37
  processes, 9 GB of 11 GB).
- **What ends a session.** A person's close (`close_task_now`, `EndIntent::Close`) ends the
  session gracefully and the task is `closed`: the list shows it under Ended, its `needs_you` is
  hidden (ADR-0111: an ended task waits for nobody), and it continues only by Continue. That is
  more than parking: the task leaves where it was.
- **What resumes a session.** `RunStart::Resume` launches Claude Code (or Codex, Antigravity)
  with the task's recorded provider session (`--resume`), as Continue and the capacity loop do
  after a usage limit; a goal-mode task gets its `/goal` typed again (Claude's goal is per
  session).
- **The queue.** `start_run_if` queues a start when `sessions_in_use() >= max_live_sessions()`,
  an update holds starts, or a supertask holds the task; only the supertask hold is told on the
  task (`held_by`). The status panel and the Capacity card (`GET /api/capacity`, the Dashboard and
  Settings) do not show the sessions at all; the status panel says "live of limit, queued".
- **The tick.** `start_background` runs a 60-second tick that drains the queue, advances chains
  and releases prepared tasks.

### Later parts

Each later part adds its own context here when it starts, from `dev` as it is then: #1418 after
task #492 (SPEC-510, "take control") merged; #1107 against what pull request #1362 (ADR-0351)
merged; #1106 against ADR-0227.

## Requirements

### Part 1: parked sessions (#1412)

- **PRK-001 A parked status.** A task can be `parked`: its session was ended by Yulius to free
  its slot and memory, and it resumes in that session. Not live (no process, no slot), not
  resting: it keeps its worktree, its claims, its `needs_you`, its delivery and its place in the
  list. Transitions: `running → parked` (the parking run's end), `parked → starting` (a resume),
  `parked → queued` (a resume that waits for a slot), `parked → closed` (archive or close),
  `parked → stopped(cancelled)` (a stop).
- **PRK-002 Who is parked.** A `running` task is parked once all of these hold for the grace
  period: its activity is `waiting` or `done` (its turn ended; `needs_attention`, a question in
  its terminal, is never parked), the list shows it under Needs you or Done, it has no background
  shell or subagent, does not wait in `yulius wait`, has no merge armed and no pull request news
  due, no held prompt, no pending browser request, no person's pause, no possible draft in its
  prompt box and no keystroke within the grace period, nothing waiting in Claude Code's queue,
  and it is not the onboarding task (whose run's end decides the onboarding). Its background work
  is looked at afresh before the session ends; a look that gives no answer counts as work. No
  park starts in the first three minutes after a server start, so the tasks that went idle while
  it was down are woken first (SPEC-358 MSB-007). The grace period is `YULIUS_PARK_AFTER_MINUTES` (default 30; 0 turns parking off),
  counted from the last change of its status or activity (`state_at`).
- **PRK-003 How it parks.** The session ends as a close does (graceful exit, then escalation),
  and the run's end records `parked` instead of `closed`; the task keeps its activity. Parking
  is decided under the launch lock against the task as stored, so a turn, a message or a start
  that came meanwhile wins. Audited `task.parked` (actor `yulius`). The terminal shows that the
  task is parked and how it resumes.
- **PRK-004 It shows as it did.** A parked task is in the list's group it was in, labelled
  "· parked", never Working: nothing of it goes on by itself, so checks still running, a conflict
  with its base or a CI run that could not run make it Needs you (an achieved goal is Done
  whatever its checks still do), and otherwise it is Needs you or Done by the idle rule. Its
  `needs_you` stays open and is shown; `tasks_list` and `task_get` say `parked`. For chains and
  start rules, a parked task that is Done is done; its scope claims still count, and the
  briefing lists it among the parallel tasks.
- **PRK-005 Writing resumes it.** `task_message` to a parked task resumes it with the message as
  its first prompt (labelled as today) and ends its `needs_you`, delivered `"resumed"`; the web's
  Resume (the Continue of a parked task) resumes it with nothing typed, its `needs_you` kept
  until the person writes. Either way it is the same provider session, with its history; at the
  session limit the resume waits in the queue with its prompt (SPEC-483 QST-001). The owner rule
  is unchanged: only who may steer the task resumes it. `task_restart` of a parked task is its
  resume; `task_merge` tells to resume it to arm a merge.
- **PRK-006 What counts.** Only live sessions of tasks that are not Done count toward
  `max_sessions` (SES-101 stays); parked tasks hold no session and no subscription.
- **PRK-007 A queued task says why.** A queued task's summary carries `queue_reason`, one line:
  "Waiting for a free session: 8 of 8 in use", "Waiting for the Yulius update to finish", or the
  supertask's line (as today), and the list and the task page show it.
- **PRK-008 The Capacity card.** `GET /api/capacity` adds `sessions {working, limit, parked,
  idle, queued, park_after_ms}`; the Capacity card shows a Sessions tile: working of limit,
  with parked, idle (live but not counted: Done) and queued apart, and when idle sessions park.
  Parked sessions use no memory.

### Parts 2 to 5

Written when each part starts (see Context): ANS (#1418), DIG (#1413), TGI (#1107), HOF (#1106),
each from its issue's acceptance criteria.

## Plan

- [x] Spec SPEC-512 and ADR-0367 for parked sessions written
- [x] A parked status: transitions, store, list group on the server and the web
- [x] Idle sessions park after the grace period, decided under the launch lock
- [x] Writing to a parked task resumes it: task_message and the web's Resume
- [x] A queued task says what it waits for
- [x] The Capacity card shows working, parked, idle and queued sessions
- [x] Docs for parking: product, architecture, api, operations, changes
- [ ] Review of the #1412 pull request answered test-first
- [ ] needs_you names who it waits for, refused for logins without write access
- [ ] A named person answers an open needs_you on the web and with task_message
- [ ] The owner sees who may answer and withdraws it; answers attributed in transcript and audit
- [ ] Owner-only refusals tested for the named person; ADR and docs for answers
- [ ] Review of the #1418 pull request answered test-first
- [ ] Digest setting: on or off, time and idle threshold, off by default
- [ ] The digest is built from task state and last messages
- [ ] The digest parks idle tasks past the threshold and goes to the browser and Telegram
- [ ] ADR and docs for the digest
- [ ] Review of the #1413 pull request answered test-first
- [ ] Telegram intake switch and default project per person, off by default
- [ ] A reply to a Telegram notification reaches its task under the owner rule
- [ ] A new Telegram message or file becomes a task with attachments
- [ ] Telegram commands /status and /task; ADR and docs for intake
- [ ] Pull request #1130 answered and closed as superseded
- [ ] Review of the #1107 pull request answered test-first
- [ ] Handoff setting off, ask or automatic and a per-project opt-out
- [ ] The sandbox_handoff tool with its checks and bounds
- [ ] Ask mode: a stored request the owner approves in a browser session
- [ ] Provenance on the target, the outcome sent back, task.handoff audited on both
- [ ] ADR amending ADR-0227 and docs for handoffs
- [ ] Review of the #1106 pull request answered test-first

## Acceptance criteria

### #1412

- [x] A task with no turn running and no background work does not count toward `max_sessions`
  after the grace period (PRK-002, PRK-003, PRK-006).
- [x] Writing to such a task resumes it in its own session, with its history (PRK-005).
- [x] A queued task shows why it has not started and what it waits for (PRK-007).
- [x] The Capacity card shows working sessions and parked sessions separately (PRK-008).

## Test plan

### #1412

- Domain: `TaskStatus` transitions of `parked` (every allowed and refused event), its wire name,
  that it is neither live nor resting, and the group rule for a parked task (Needs you, Done,
  never Working by a live overlay).
- App, with the fake agent (`crates/yulius/tests`): an idle task parks after the grace period
  and its slot frees a queued task; each exclusion of PRK-002 keeps a task live (background
  shells, `yulius wait`, armed merge, needs_attention, a keystroke, a held prompt, grace 0); a
  turn that starts between the decision and the end wins; `task_message` resumes a parked task
  in the same provider session (`--resume` with the recorded session id) and ends its
  `needs_you`; the web's continue resumes it and keeps `needs_you`; at the limit the resume
  queues with its prompt; archive closes a parked task; a stop stops it.
- `queue_reason` at the session limit and during an update; the Capacity view's `sessions`.
- Web unit tests (`status.js`): label and group of a parked task, `taskActions` offers Resume,
  the queued line; an e2e test for the Sessions tile and a parked task's row.

## Decisions

- **A new status, not a flag on `closed` nor a `running` task without a process.** `closed` means
  a person ended the task (ADR-0111 hides its request, the list shows it Ended, chains read it as
  ended); a `running` task without a run breaks the restart reconciliation (ADR-0050), which
  takes a running task with no live run for a lost one. `parked` says what happened and keeps
  both rules true. ADR-0367.
- **Parked tasks are neither "live" nor "resting".** `is_resting` means ended in most of its
  callers (CI hygiene of worktrees, `needs_you`, chains); `is_live` means a process holds a slot.
  A parked task's claims count (`TaskStatus::holds_claims`), it is Done for chains when its idle
  run would be, and the few places that continue or archive a resting task accept `parked` by
  name. A supertask does not wait for a parked task, as it does not wait for an idle Done one.
- **A park looks at the session afresh.** The list's shell and `yulius wait` counters are a
  probe's last answer; before ending a session the park probes it again and treats no answer as
  work running, as a move does (SPEC-483 SWT-001), and asks Claude Code's queue (SPEC-471). A
  park whose session cannot be ended gives up and the session goes on.
- **Known limits.** The park's intent lives in the server's memory: a restart between the
  decision and the session's end records the end as a clean exit (`closed`) or a crash. A person
  who only reads the terminal does not hold a park; a keystroke or draft does. On the web,
  "writing to it" is Resume followed by typing; `task_message` resumes it with the message.
- **The grace period is a unit setting, default 30 minutes.** Long enough that a person who reads
  the answer and replies within the half hour finds the live session; short enough that a night
  of finished tasks frees its memory. Settings → limits is for host-sized numbers; this one is not
  sized from the host. 0 turns parking off.
- **A web Resume types nothing and keeps `needs_you`.** The person who presses Resume writes into
  the terminal next; their keystrokes end the request as today. A `task_message` is the answer
  itself, so it ends the request as a typed message does (TMS-006).
- **Yulius's own news does not wake a parked task.** A parked task has no armed merge and no
  pending news by PRK-002; news that arrives later waits in its watch like any news for a
  session that takes none, and is told when the task resumes.

## Evidence

### Part 1 (#1412)

| Acceptance criterion | Held by |
| --- | --- |
| A task with no turn and no background work stops counting after the grace period | `parking::an_answered_task_parks_and_its_slot_starts_a_queued_task` (limit 1: the parked task's slot starts the queued one; activity kept, run ended `parked`, audited `task.parked` by `yulius`); `yulius_app::parking::tests::*` (every exclusion of PRK-002); `parking::a_task_parks_only_after_its_grace_period_and_never_with_parking_off` (grace 0, a long grace, a keystroke) |
| Writing to it resumes it in its own session with its history | `parking::a_message_resumes_a_parked_task_in_its_own_session` (`task_message` → `delivered: "resumed"`, second run `LaunchKind::Resume` with the same provider session id, `needs_you` ended); `parking::resume_keeps_the_request_and_archive_or_stop_end_a_parked_task` (the web's Resume keeps the request; archive → `closed`; Stop → `stopped`); `parking::a_message_during_a_park_is_refused_and_resumes_the_task_once_parked` (race point `ParkDecided`) |
| A queued task says what it waits for | `parking::an_answered_task_parks_and_its_slot_starts_a_queued_task` (`queue_reason` "Waiting for a free session: 1 of 1 in use"); `yulius_app::parking::tests::a_queued_task_says_what_it_waits_for`; e2e `parked.e2e.ts` "a queued task says what it waits for" |
| The Capacity card shows working and parked sessions apart | `parking::an_answered_task_parks_and_its_slot_starts_a_queued_task` (`sessions` = working 1, limit 1, parked 1); e2e `parked.e2e.ts` "the Capacity card shows working and parked sessions apart" |

The status itself: `yulius_domain::task::tests::a_parked_task_resumes_closes_or_stops` and
`a_parked_task_stays_in_its_group`; the web rule: e2e `parked.e2e.ts` "a parked task reads as it
did…" (labels, groups, actions, the ended terminal's Resume).

Commands (2026-11-19, after merging `origin/dev` at 4b9045b4c):

- `cargo test -p yulius --test main parking::` → 5 passed (`target/evidence/parking-9.log`).
- `cargo test -p yulius-domain --lib` → 573 passed; `cargo test -p yulius-app --lib` → 614 passed.
- `npx e2e run tests/parked.e2e.ts` → 3 passed; `subscriptions`, `loading-shapes`, `task-done`,
  `attention`, `home`, `archive`, `pause` → 112 passed.
- `scripts/dev quick` → passed; `scripts/dev lint` → passed.

What a person sees. Before: a finished task stays "Running · Your turn" or "Done" with a live
process; a queued task reads "Queued" and nothing else; the Capacity card has no sessions. After:
the same row reads "Your turn · parked" (or "Done · parked"), its second line "Parked to free its
session · resumes when you write to it", its terminal "Yulius parked this idle session to free its
place and memory. Resume it, or write to it, and it goes on in the same session with its history."
with a Resume button; a queued row reads "Waiting for a free session: 8 of 8 in use"; the Capacity
card's Sessions tile reads "8 / 8 working" over "13 parked · 2 idle · 3 queued" and "Idle sessions
park after 30 min".

Review round 1 (Fable 5.2, request changes at 74c6b7076), answered test-first:

| Finding | Answer | Test that fails without it |
| --- | --- | --- |
| B1 `cargo doc`: private intra-doc links in `parking.rs` | plain backticks | `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps` passes |
| M1 a stop during the park's end left the task `stopping` | `settle_parked` rests a stopped or paused task as its run's end does | `parking::a_stop_during_the_parks_end_stops_the_task` (race point `RunEndTaken`) |
| M2 a parked task listed Working by its pull request | `AttentionFacts::parked_group`; `status.js` mirrors it | `a_parked_task_stays_in_its_group` (every `PullSignal`, plan open or not); e2e `parked.e2e.ts` |
| M3 a parked Done task never Done for chains and rules | `task_is_done` counts an idle parked task | `parking::a_task_chained_after_a_parked_done_task_starts` |
| M4 a parked task's claims ignored | `TaskStatus::holds_claims` in the claim readers, overlaps and the briefing | `parking::a_parked_task_keeps_its_claims` |
| M5 park trusted cached counters | fresh `background_or_wait_now` (no answer = work) and Claude Code's queue | `parking::a_look_that_gives_no_answer_keeps_the_session` (gated host) |
| M6 test gaps | added the tests in this table and below | `parking::a_question_in_the_terminal_or_a_pause_keeps_the_session`, `parking::a_resume_at_the_session_limit_waits_with_its_message` |
| F10 a park overtaking a switch closed the task | it parks; the notice says "the idle session was parked" (`SwitchEnd::Parked`) | covered by the fallback order of M1; no race test of its own |
| F11 a park whose session cannot end blocked messages | the park gives up and says so | `parking::a_park_that_cannot_end_its_session_gives_up` |
| F12 park intent in memory only | recorded as a known limit (Decisions) | — |
| F4 `task_restart`/`task_merge` on a parked task | resume through `continue_task`; merge advice says resume | the same `continue_task` as `resume_keeps_the_request…` |
| F7 Move offered, Stop not | no Move, Stop offered | e2e `parked.e2e.ts` |
| m7 park before the outage wake | no park in the first 3 minutes after a start (asserted at compile time > `WAKE_GRACE`) | — |
| m8 reason only on hover | the strip and the empty terminal show it | e2e "a parked task shows where it was…" |
| m9 analytics | parked + Done counts `done` | `analytics::tests` |
| m10, nits | rollback note in operations, the third `queue_reason` line documented, PRK-002 and ADR-0367 name the onboarding task | — |
| `queue_reason_of` per summary | kept: only queued tasks compute it, at most `queue_max` (32) | — |
| needs_you shared cause (pre-existing condition) | unchanged: answering a parked task resumes it; the others are answered by their own messages | — |

Commands after round 1: `cargo test -p yulius --test main parking::` → 12 passed;
`scope_overlaps:: task_message::` → 26 passed; `cargo test -p yulius-domain -p yulius-app --lib`
→ 573 + 614 passed; `npx e2e run tests/parked.e2e.ts` → 4 passed, four runs in a row;
`scripts/dev quick`, `scripts/dev lint`, `cargo doc` → passed.
