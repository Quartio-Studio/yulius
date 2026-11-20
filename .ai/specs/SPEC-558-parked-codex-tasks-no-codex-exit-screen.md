---
yulius-task: 09d643c4-9df9-4abe-9c9c-d0cfe7fdf205
status: in-progress
---

# SPEC-558 Parked terminals and unfinished Codex goals

## Goal

An owner can see the last turn of a parked session, write its next prompt and trust the Done label. Codex continues a goal that has repairable work left.

## Context

Task #517 displayed Codex's exit instructions. The host drained shutdown output into the terminal hub and the supervisor saved it as the final screen. In quartio-v2 #325 the owner observed five of seven steps complete, a failing open PR and an idle parked goal. SPEC-512 introduced parking. SPEC-503 supplies native Codex resume and its blocking Stop hook.

## Requirements

- PKR-001 Preserve the redacted pre-stop screen across parking and server restart for every harness. Add one Yulius line with the idle duration, reason and how to resume.
- PKR-002 A message resumes the recorded Codex rollout with the message as its next prompt, including when capacity delays the launch.
- PKR-003 On Codex goal Stop, continue open steps or failing checks through the bounded discipline gate. Respect a real request for a person and autonomous delivery waits. On exhaustion flag the task.
- PKR-004 Parking requires finished agent work. Historical parked tasks with unfinished work show Needs you. Completed tasks follow the existing Done rule.

## Plan

- [x] Preserve the pre-stop terminal and explain parking
- [x] Prove parked messages resume the recorded Codex rollout
- [x] Continue unfinished Codex goals within the Stop retry bound
- [x] Keep unfinished tasks live and classify parked work accurately
- [x] Document and verify the parking contract
- [x] Deliver native Codex resume to dev
- [x] Preserve the parked grid until the resumed host attaches
- [x] Verify chain eligibility and refresh stale PR observations
- [ ] Deliver goal continuation and parking eligibility to dev
- [ ] Recover screens from tasks parked before the upgrade

## Acceptance criteria

- AC-1 A Codex shutdown which replaces the real screen with token usage and resume instructions leaves the page and task_screen showing the pre-stop answer and parking notice.
- AC-2 The next run uses the same rollout ID and consumes the owner's message.
- AC-3 Five completed steps, two open steps and a failing PR produce a continuation at Stop. Either unfinished condition alone suffices. Retries stop at the existing bound and human requests keep control.
- AC-4 Neither grace-period parking nor digest parking consumes an unfinished task. The sidebar and tools agree when an already parked task has open work.

## Test plan

Use the fake Codex executable, its rollout, real PTY shutdown and the application Stop hook. Exercise saved-screen and terminal readers, terminal restoration, step and pull combinations and historical statuses. Run scoped Rust and web tests, repository quick checks, the security command and Yulius CI.

## Decisions

Capture a separate screen at the park decision because exiting can clear the entire alternate screen. Changing shutdown signals would couple presentation to the harness and could interfere with reaping. A separate snapshot lets an unsuccessful park keep its real terminal. Reuse native Codex resume and the Stop hook's persisted three-block budget. Older runs can only recover content still present in their saved screen. A parked owner terminal fits the saved grid to the viewport so the first row, last row and notice survive a smaller window. If the PR is the only remaining work and the Stop lookup cannot finish, the continuation asks the agent to verify its checks. Parking reads the current head before acquiring the launch lock and supplies that signal to the final eligibility check.

## Evidence

- The PTY regression failed on the original supervisor with the #517 token-usage and resume text. With the fix it preserves the first answer and one parking notice after process exit and server restart. The remote `task_screen` shows the same screen. The next `task_message` starts a native resume for the parked attempt's rollout ID and the synthetic rollout contains both prompts.
- The five-of-seven-step fixture with open PR #1461 and failed checks originally received no continuation. The HTTP Codex Stop tests now cover that combination, open steps alone, failed checks alone, the fourth Stop raising attention, and an explicit request for the owner. The historical parked terminal fixture removes the #517 footer and recovers its 35-minute idle interval.
- Scoped nextest across the app, domain and integration binary passed 101 tests for parking, Codex, Stop hooks, digest and task-screen behavior. `scripts/dev quick` passed repository contracts and formatting. `cd e2e && npx e2e run` passed 1,163 browser tests with one optional test skipped. The parked task page regression failed on the original sidebar. These tests used a fake provider and actual subprocess supervision. No paid session was needed.
- Independent review of `50a9313ba` requested three corrections. Holding the GitHub checks response reproduced a five-second hook timeout. Codex now gets its local plan continuation within the client deadline. An idle task with an unreadable recorded PR formerly parked on a cold cache. Both the grace-period and digest fixtures now keep its run. The attached PTY viewer formerly received the parked snapshot before its state, and Chromium showed 39 of 55 saved rows. The supervisor now sends the dimensions first and the owner’s page fits the whole grid. The notice names the page’s Resume action and sending a message. All 102 scoped Rust tests and all five parked-page browser tests passed with these fixes.
- `scripts/dev security` reported that Vidoc was disabled for the project. It scanned no files.
- Review of `19b7770a0` found three edge cases. With every step done and the remote response held, the old hook ended the turn. It now asks Codex to verify the recorded PR within the retry budget. A pending recorded head with an empty overview cache previously parked under the digest. The current head now feeds the same final grouping decision for both parking triggers. A three-row historical screen previously retained the Codex resume suffix when token usage scrolled off. The reader now identifies those trailing instructions across wrapped rows. All three regression tests failed on the previous head. The browser suite passed 1,164 tests with one optional test skipped.
- The initial mutation baseline passed. The author interrupted its workers when compilation logs showed that they had copied files from different revisions during review fixes. Mutation runs started subsequently use detached, immutable checkouts.

- [PR #1492](https://github.com/Quartio-Studio/yulius/pull/1492) delivered PKR-002 into `dev` on green Yulius CI at `ae7f8fa73`. The Clippy integration target and the native rollout regression passed locally on that revision.
- With the new attempt held at `RunRecorded`, the original cold reader returned an empty string in place of the first answer and parking notice. With unreadable lineage during a subscription retry, the original capacity round left the task in `Failed(SpawnError)` instead of `WaitingCapacity`. Each regression failed on the corresponding uncorrected path.
- Eight Chromium parking tests passed, including all 90 rows of a saved desktop grid on a phone. The full browser suite on the same web assets passed 1,167 tests with one optional skip.

- The first screen delivery candidate passed 76 scoped Rust tests and Clippy on the app, server and integration targets. Its reviewer identified an account removed after selection but before spawn. The real supervisor recorded the discarded attempt. With no replacement subscription, the prior reader then returned an empty string. The lookup now follows that resume past its discarded attempts. The regression exercises the cold tool and page while capacity waits and again while the replacement launch is held at `RunRecorded`.

- [PR #1493](https://github.com/Quartio-Studio/yulius/pull/1493) delivered PKR-001 for new parks into `dev` on green CI at `215463547`. The final head passed 76 scoped Rust tests, including both cold readers across a discarded launch; Clippy on the affected targets passed.
- The goal and delivery regression fixtures separately cover a current green head after an older failing observation, incomplete check sources, and whether a chained task can start after a historical parked predecessor.

- Chromium reproduced a stale green PR in the overview overriding a parked task’s current Needs you classification from the server. With the server result taking precedence over a Done label, the stale and missing overview cases both passed in the eight parked-page tests.

## Delivery slices

Deliver native Codex resume independently (PKR-002). The terminal delivery (PKR-001) includes the original pre-stop snapshot, phone geometry, and recovery through the resumed host's first snapshot. Subsequent deliveries cover goal continuation and parking eligibility (PKR-003 and PKR-004), then recovery of pre-upgrade screens. The earlier combined-head evidence describes development verification across these slices.

The native-resume regression checks the rollout in the native launch and reads the first and second user prompts from its on-disk history. It also exercises absent IDs and an older lineage entry. No paid provider is involved.

The terminal review of `50880f241` confirmed that reopening during account selection preserves the committed grid. It found that after a cold restart the viewer arriving between recording a spawning attempt and attaching its host could see an empty grid. The terminal readers select the preceding committed capture until the resumed host publishes its first snapshot. A separate capacity retry must keep the held prompt if it cannot read the provider session identity.
