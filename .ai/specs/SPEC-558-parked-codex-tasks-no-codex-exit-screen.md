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
- [x] Gate parked successors on declared work, owner decisions and current branch PRs
- [x] Preserve parked PR observations across overlapping Git overviews
- [ ] Give parked PRs fair status reads in task lists and Home
- [ ] Refresh parked page and sidebar status independently of the Git overview
- [ ] Keep failed integration candidates with their original owner
- [ ] Deliver bounded Codex goal continuation to dev
- [ ] Recover screens from tasks parked before the upgrade

The owner supplied the ordering of #325. On November 20 UTC the last answer at 04:22:19 reported pending CI, review and mutation checks on `f86cbd49f`. Ordinary PR #1461 CI failed at 04:30:56. The digest parked the waiting session at 04:52:52. It resumed at 10:30:20 and resubmitted at 10:36:52. The process was absent for 337.46 minutes. Step 10 covers this ordering, including a lagging GitHub status, a newer local commit during the parking decision, and a published head that exists only in another clone.

The failed-integration-candidate step covers the owner's November 20 observation from quartio-v2 #331. All four steps were complete and ordinary CI on open PR #1481 was green. The integration candidate at `e57c30b8c` failed at 15:08:43 UTC, four seconds before the daemon restarted. Run `dc945ada-86fc-42ba-a4d1-bbc8240130ce` reported 292.1 ms against a 200 ms search limit in shard 2. At 15:09:25 the board showed Done with the PR unarmed. Attempt 2 returned to Working at 15:09:31, rearmed at 15:09:35 and went idle at 15:09:41, before the operator message at 15:09:43. The observation establishes a transient display error. The restart fixture checks the waiting task, the parked view and the dependent task against the applicable candidate. Verify delivery of its recorded reason to the existing owner, including deduplication after rearm or earlier notification. A stale display alone triggers no wake or new work. Human decisions take precedence and superseded candidates are excluded.

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

- Three review rounds of the combined status prototype identified separate delivery risks for integration candidates. The dependency delivery checks the current branch and recorded PR against declared work and human decisions. The task-list delivery gives parked PRs fair status reads. Browser polling has its own delivery. The later candidate delivery covers persisted candidate failures, deduplication with the merge outcome and revalidation before automatic wake or parking.
- The ordinary status regressions reproduced cases where an unreadable or changing head, unknown mergeability, or a plan reopened during a check could release a successor. Browser coverage checks the parked task page and sidebar in both directions after a same-head rerun.
- Live restart verification of the #331 sequence requires an instance running the candidate fix. Only a disposable task should be used.

- The ordinary status review reproduced a same-head failure while GitHub confirmed the PR. On the prior implementation, the dependent task moved from Chained to Queued. A separate achieved-goal fixture showed Done when a pending GitHub report masked the PR's failed ordinary run. A single-batch deadline mutation made the healthy ninth row show Needs you.
- Three browser review rounds reproduced stale parked groups after a failed overview, a partial overview, and a second CI update while the same overview remained held. The server summary carries the recorded PR identity and its verified group. A separate browser delivery will refresh that summary on each watched change independently of the overview. Its prototype passed 13 parked browser tests before the final held-overview finding.

- On the server-only head, replacing the historical goal directory with an unreadable path changed the parked summary to Done. Temporarily hiding the durable owner-decision column released a successor to Queued. A limited MCP request started reading a parked PR after its first page was already full. All three regressions failed before the prototype repairs. The list delivery will retain ordered reads, early exit and the Git worker cap. The dependency delivery reuses the parking supervisor’s verification of both recorded and branch PRs, including replacements published externally.

- Independent review approved step 11 at `72388f86a`. The scoped nextest run passed all 200 parking, PR watcher, chain and task completion tests. App and integration Clippy, formatting and repository contracts passed. Vidoc was disabled. The replacement-PR fixture queued its successor on the earlier combined prototype while the replacement was red. Two more fixtures queued it on `3e806b7d1` when the historical run directory disappeared or the owner resumed during the final checkout read. A separate regression held the final browser lookup while the owner resumed. It reproduced the incorrect release on `04082357c` and kept the successor chained on the approved head.
- The parked successor checks both the recorded and current ordinary branch PRs, including a replacement opened while the recorded PR was being reconciled. A pending human decision, unreadable historical goal, unpublished checkout or active ordinary CI holds the successor. Once the evidence settles, normal chain evaluation releases it. An explicit `#N merged` rule continues to use its recorded merge.

- The first dependency mutation run reported `mutants: 65 listed, 35 caught, 2 missed, 0 timed out, 28 unviable, 0 not reached (bound 30 min, took 1123 s)`. Both survivors concerned the tracked PR allowed to park during an active run. Inverting the purpose released a successor while CI was running in the updated chain fixture. Inverting the PR identity prevented the tracked author from parking in the supervisor’s real-host fixture. Both unmodified tests passed in the scoped run.

- The checkout and historical-goal review fix reported `mutants: 40 listed, 10 caught, 2 missed, 0 timed out, 28 unviable, 0 not reached (bound 30 min, took 1071 s)`. The publication regression caught a final parking check weakened to accept either the checkout or CI. The historical chain fixture now supplies a readable run directory whose marker exceeds the OS path limit. It caught treating every marker error as file absence and released the successor after a readable attempt was recorded.

- The final browser-order fix reported `mutants: 33 listed, 13 caught, 6 missed, 0 timed out, 14 unviable, 0 not reached (bound 30 min, took 1031 s)`. The historical owner-decision fixture caught all five altered conjunctions in the work and decision check, including its three survivors. The final CI and browser-read fixtures caught all six altered combinations in the last predecessor check, including its three survivors. The browser fixture covers an owner who resumes and parks again after a new turn. With these test additions, the final scoped run passed all 200 tests, and Clippy passed.
- PR #1524’s E2E job failed on `f84a6eb0b` in run `9f6f533e-7f2d-48a7-a7f1-0b4a6d4fd0cc`. Four of eight local Chromium runs reproduced the empty input. The failure trace showed the test tapping the panel while it still displayed the loading overlay. The two browser restart tests now wait for the first displayed frame before sending input to the replacement session. Both tests passed eight of eight runs with retries off.

- PR #1524 delivered the parked predecessor check into dev at `4b3497496`. The exact PR head `87515a163` passed Yulius CI run `171d5cc9-b354-4d52-afeb-019d742abca3`, including quality, both test shards and E2E. Security was skipped.
- The independent reviews of the server list prototype isolated a prerequisite for an installation that cannot verify an owner-observed PR. A failed or truncated Git overview could erase a known red PR, including when it completed after another overview established the current state. The observation cache has its own delivery. Fair list deadlines and verified per-task summaries follow it.

- On dev head `4b3497496` the held partial overview overwrote a newer complete response. MCP omitted the parked task with failing external PR #1462. On `719deb087` a newer partial closure of #1 suppressed the earlier complete failure of #2 when it published first. Both are covered in the ordered overview fixtures. A separate fork assertion checks the task row, and the list boundary fixture distinguishes 31 open PRs from a complete result of exactly 30.

- The overview admission regression failed on `5129e2ab0` when cancelled requests retained capacity and an excess request waited indefinitely. The repaired cache releases cancelled slots immediately and returns HTTP 429 on saturation. The HTTP regression failed on `396d34a3d` with 429 after cancelling a request in the final Git log. Its replacement received 200 on `ad638e4e8` while the oldest request remained pending.
- Independent review approved `ad638e4e8` and reran the HTTP regression, three cache tests and the overlap integration fixture. The final scoped nextest run passed all 11 tests, including parked successor, Home, MCP, fork identity and cancellation checks. Clippy passed for the app and integration targets with warnings denied. Repository contracts, Rust formatting and spec validation passed. Vidoc was disabled.
- The first overview campaign reported `mutants: 32 listed, 22 caught, 5 missed, 0 timed out, 5 unviable, 0 not reached (bound 30 min, took 991 s)`. The ordering repair removed the per-row revision guard and its generation arithmetic. For the other three survivors, applying each generated mutation to the branch identity check made the integration fixture fail its assertion that the external task's own PR supplied its conflict flag. The unmodified test passed.
- The ordered publication campaign reported `mutants: 28 listed, 15 caught, 0 missed, 0 timed out, 13 unviable, 0 not reached (bound 30 min, took 913 s)`. The admission repair reported `mutants: 21 listed, 9 caught, 0 missed, 0 timed out, 12 unviable, 0 not reached (bound 30 min, took 629 s)`. Moving publication past the final Git log reported `mutants: 4 listed, 0 caught, 0 missed, 0 timed out, 4 unviable, 0 not reached (bound 30 min, took 330 s)`. Its HTTP red/green regression directly verified the changed lifetime.

- PR #1527 delivered the ordered Git overview cache at `a1f42f8eb`. The exact PR head `a7ea115e3` passed Yulius CI run `e80317f4-3320-40d4-bab2-66799b1f5cdf` through train `d44d77bc-4b53-4281-a829-fe9cea66b5c1`. Security was skipped.
- Fair parked summary reads share the parking verifier and its branch-scoped search. Batches reserve lookup time for subsequent rows, and native Git workers keep their capacity until completion. [ADR-0409](../../docs/decisions/0409-share-parked-pr-read-time-across-task-lists.md)

- The ninth-row fixture verifies both Home and MCP under a held earlier PR. An open local step originally hid owner-observed PR #1461 even though the row showed Needs you. The direct HTTP regression returned its identity after the repair. The initial scoped run then passed 96 tests.
- On `e0390b2cf`, a PR merging while its checks were held left the row naming closed #1461 after the same request discovered open #1462. The shared rule scan for `after_all_tasks` also exhausted its four-second test deadline. Rule construction now shares the Home read budget. The test uses the rule-size limit to isolate that scan from the subsequent chain eligibility check. The notifier and digest continue to read local state.
- On `138f511b3`, the two-base fixture named the newer healthy PR instead of the older blocking PR. A separate final-confirmation fixture returned an external PR as open after it disappeared from the branch, despite the previously recorded merge. Both regressions assert the identity and group returned by the same request.

## Delivery slices

PRs #1492 and #1493 delivered native Codex resume and the pre-stop screen for new parks. PR #1505 kept sessions with local unfinished work live. PR #1510 verified current ordinary PR checks and unpublished work before parking. PR #1524 delivered the parked predecessor gate. PR #1527 preserved known PR observations across overlapping Git overviews. Remaining deliveries cover fair parked task-list reads, page refresh, persisted integration failures, bounded Codex goals and pre-upgrade screens.

The native-resume regression checks the rollout in the native launch and reads the first and second user prompts from its on-disk history. It also exercises absent IDs and an older lineage entry. No paid provider is involved.

The terminal review of `50880f241` confirmed that reopening during account selection preserves the committed grid. It found that after a cold restart the viewer arriving between recording a spawning attempt and attaching its host could see an empty grid. The terminal readers select the preceding committed capture until the resumed host publishes its first snapshot. A separate capacity retry must keep the held prompt if it cannot read the provider session identity.
