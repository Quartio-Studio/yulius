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
- [ ] Deliver the parked screen and native resume to dev
- [ ] Verify chain eligibility and refresh stale PR observations
- [ ] Deliver goal continuation and parking eligibility to dev

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

## Delivery slices

The third review requested separate corrections to historical screen recognition and PR eligibility. Deliver the terminal snapshot and native resume first (PKR-001 and PKR-002). Deliver the Stop gate, parking eligibility and Done classification next (PKR-003 and PKR-004). The earlier combined-head evidence describes development verification for both slices. Each delivered head needs independent approval and its own green CI.
- First delivery slice addresses the third review's terminal finding. The regression demonstrated that a legitimate final answer containing `codex resume --last` lost the command. Legacy cleanup now requires the adjacent instruction heading when a command follows earlier content. A clipped command at the top must name a valid provider session ID. The three PR eligibility findings belong to the second delivery slice and remain open until that slice's tests pass.
- Review of the first slice at `185467e7d` found a clipped phone viewport, a stop overtaking parking and a second false positive in legacy footer recognition. Chromium reproduced the clipping with a 220 × 90 grid on a 390 × 844 phone. The parked grid now scales below the normal live font floor when needed. All five parked-page tests passed, including pixel bounds for the last row. Both Rust regressions failed on the reviewed code. The cold screen readers now check the committed task transition when a stop overtakes a park, and both resume instruction forms require surrounding footer text when earlier content is visible.
- The second terminal review of `bb9de3dd5` identified queued resumes whose previous attempt recorded a loss or parked during a switch, and a legitimate standalone resume suggestion with room left in the viewport. The switch test and the standalone-answer test failed on that head. The supervisor now renames the captured screen after the Parked transition commits. Both cold readers recognize the committed capture regardless of how the attempt ended. The legacy reader requires corroborating footer lines or visible evidence that a standalone suffix could have scrolled to the top.
