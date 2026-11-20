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
- [x] Keep unfinished plans, specs and goals live
- [x] Verify PR checks and unpublished repairs before parking
- [ ] Give parked PRs fair status reads and gate dependent tasks
- [ ] Deliver bounded Codex goal continuation to dev
- [ ] Recover screens from tasks parked before the upgrade

The owner supplied the ordering of #325. On November 20 UTC the last answer at 04:22:19 reported pending CI, review and mutation checks on `f86cbd49f`. Ordinary PR #1461 CI failed at 04:30:56. The digest parked the waiting session at 04:52:52. It resumed at 10:30:20 and resubmitted at 10:36:52. The process was absent for 337.46 minutes. Step 10 covers this ordering, including a lagging GitHub status, a newer local commit during the parking decision, and a published head that exists only in another clone.

Step 11 also covers the owner's November 20 observation from quartio-v2 #331. All four steps were complete and ordinary CI on open PR #1481 was green. The integration candidate at `e57c30b8c` failed at 15:08:43 UTC, four seconds before the daemon restarted. Run `dc945ada-86fc-42ba-a4d1-bbc8240130ce` reported 292.1 ms against a 200 ms search limit in shard 2. At 15:09:25 the board showed Done with the PR unarmed. Attempt 2 returned to Working at 15:09:31, rearmed at 15:09:35 and went idle at 15:09:41, before the operator message at 15:09:43. The observation establishes a transient display error. The restart fixture checks the waiting task, the parked view and the dependent task against the applicable candidate. Verify delivery of its recorded reason to the existing owner, including deduplication after rearm or earlier notification. A stale display alone triggers no wake or new work. Human decisions take precedence and superseded candidates are excluded.

## Acceptance criteria

- AC-1 A Codex shutdown which replaces the real screen with token usage and resume instructions leaves the page and task_screen showing the pre-stop answer and parking notice.
- AC-2 The next run uses the same rollout ID and consumes the owner's message.
- AC-3 Five completed steps, two open steps and a failing PR produce a continuation at Stop. Either unfinished condition alone suffices. Retries stop at the existing bound and human requests keep control.
- AC-4 Neither grace-period parking nor digest parking consumes an unfinished task. The sidebar and tools agree when an already parked task has open work.

## Test plan

Use the fake Codex executable, its rollout, real PTY shutdown and the application Stop hook. Exercise saved-screen and terminal readers, terminal restoration, step and pull combinations and historical statuses. Run scoped Rust and web tests, repository quick checks, the security command and Yulius CI.

## Decisions

Capture a separate screen at the park decision because exiting can clear the entire alternate screen. Changing shutdown signals would couple presentation to the harness and could interfere with reaping. A separate snapshot lets an unsuccessful park keep its real terminal. Reuse native Codex resume and the Stop hook's persisted three-block budget. Older runs can only recover content still present in their saved screen. A parked owner terminal fits the saved grid to the viewport so the first row, last row and notice survive a smaller window. If the PR is the only remaining work and the Stop lookup cannot finish, the continuation asks the agent to verify its checks. Parking reads the current head before acquiring the launch lock and supplies that signal to the final eligibility check.

A human request can take precedence over unfinished work in the task list. Both parking triggers therefore check the recorded plan, spec and native goal immediately before committing to shutdown. The task’s steps lock orders that decision with plan, spec and goal updates. An update in progress or a store read failure leaves the session live for a later pass.

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

- On dev at `3cc2b1969`, the one-open-step, in-progress-spec and failed-native-goal regressions each observed one park where zero was expected. Positive controls let the idle session park after the step or spec finishes. The step fixture exercises both the grace timer and the digest.

- The first local guard at `311e7e09b` passed 41 parking tests. Review found that a step could be committed after the final eligibility read and that a failed spec-status read could let digest parking proceed. Three deterministic regressions reproduced one incorrect park each on that head. They cover a step added before the park intent, an in-progress step writer, and a recorded spec made unreadable in SQLite. The spec fixture restores the status, verifies that it was in progress, and parks successfully after completion.

- Independent review approved the local parking guard at `dad583243`. All 44 scoped Rust tests passed, including grace and digest parking, concurrent updates and recovery after the stored spec becomes readable. Repository contracts and formatting passed.

- PR #1505 delivered local plan, spec and goal guards at dev commit `21810b880`, whose tree matched the passing merge train.
- PR #1510 delivered step 10 into dev at `a97de183b`. Its CI record `64e9886f-0234-4a51-8e43-3ce3f1a9ae38` cites the passing singleton train `9caebdf9-3d7d-4fba-b415-6a4bdab0db63`. Quality, both test shards and browser checks passed. Security was skipped.
- The reverse-order fixture initially passed against the existing PR verification candidate. Its ordinary failed CI run was durable before parking while GitHub still showed success. The human decision held the watch, then clearing it produced one actionable notice in the existing attempt. The second-clone fixture on that candidate reproduced one incorrect park. The task checkout did not have the upstream commit needed for the ancestry comparison.
- The integrated test `a_ci_failure_before_digest_parking_reaches_the_idle_author_once` runs a synthetic agent turn with ordinary CI pending, records a failed run before the digest, and leaves the remote status green. An open human decision holds both parking and the notification. Once cleared, the watcher sends the ordinary failure exactly once to the original attempt and ignores the obsolete train. The companion test `parked_task_parks_safely_during_ci_and_auto_wakes_on_actionable_check_results` covers failure after parking.
- The second-clone test keeps the session live through an unknown upstream SHA and through a new local commit at the final host probe. It permits parking once publication covers the checkout. On `ec068537d` two further regressions each observed one incorrect park, when GitHub required a base update and when an unwatched CI rerun started during the final probe. Both pass on the integrated change. Independent review approved `b64f4aea4`. The scoped parking and PR-watch run passed all 69 tests.
- The review-fix mutation campaign reported `mutants: 32 listed, 9 caught, 2 missed, 0 timed out, 21 unviable, 0 not reached (bound 30 min, took 1095 s)`. Two additional tests caught the survivors. A different active head takes over the watcher at the final host probe, and a recorded PR absent from branch discovery falls behind its base during the checks read. Each weakened condition produced one incorrect park in the corresponding test. The test-only changes preserve the independently reviewed product code.
- The final scoped nextest run passed all 72 parking, PR-watch and eligibility tests, including both mutation regressions. Clippy passed on the app and integration targets. Repository contracts and Rust formatting passed. Vidoc was disabled.
- Live verification still needs an instance running this Yulius release. On a disposable task, observe a failed ordinary run between Stop and the scheduled digest, then verify that the same live attempt receives one watcher prompt despite lagging remote status. Repository tests used real Git and synthetic agents. Quartio-v2 #325 remains under its owner's control.
- A test-only reproduction for step 11 reopened the database with four completed steps, a green ordinary head and a newer failed single-member candidate. The waiting task counted as Done. The live owner received one generic green-head rearm prompt that omitted the candidate's failure. This fixture does not establish which prompt reached the real #331 owner. Its attempt resumed work before the operator message.

- The step 11 cold-start fixture reopens the database, reattaches to the same live attempt and reads the persisted failed job. The owner receives its run ID, shard and measured latency in the existing PR notice after the human decision clears. The next watch sends nothing. A new published head clears the candidate observation. This case uses an attributable single-member train. Membership of the actual #331 train was not established.
- The parked terminal and sidebar initially disagreed on a same-head rerun. A separate browser fixture retained a failed candidate after a newer verified result. All 13 parking browser tests passed after both views refreshed the same task summary. A direct JavaScript check also reproduced an older Done summary masking a newer request for a person. The renderer now preserves that request.

## Delivery slices

PRs #1492 and #1493 delivered native Codex resume and the pre-stop screen for new parks. The next delivery keeps sessions with local unfinished work live. Subsequent deliveries verify current PR checks and unpublished work, classify historical parked tasks and gate their successors, continue bounded Codex goals, and recover pre-upgrade screens. The earlier combined-head evidence records prototype verification across these deliveries.

The native-resume regression checks the rollout in the native launch and reads the first and second user prompts from its on-disk history. It also exercises absent IDs and an older lineage entry. No paid provider is involved.

The terminal review of `50880f241` confirmed that reopening during account selection preserves the committed grid. It found that after a cold restart the viewer arriving between recording a spawning attempt and attaching its host could see an empty grid. The terminal readers select the preceding committed capture until the resumed host publishes its first snapshot. A separate capacity retry must keep the held prompt if it cannot read the provider session identity.
