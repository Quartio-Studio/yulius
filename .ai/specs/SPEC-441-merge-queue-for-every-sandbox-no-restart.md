---
yulius-task: 50457a7b-0346-4616-bc3e-70c37de8b10b
status: done
---

# SPEC-441: A merge queue for every project: no restarts when the base moves

Status: implemented
Date: 2026-11-08
Owner: Yulius task #423
Scope authorization: the owner, via quartio-v2's main session: "prepare a task to yulius … it
should be general, so apply to every sandbox yulius powers." Amends SPEC-050 DLV-203 (when Yulius
brings an armed pull request up to date) and SPEC-084 (merge trains become the merge candidates of
one or more pull requests). ADR:
[0285](../../docs/decisions/0285-armed-pull-requests-merge-through-a-queue-that-never-restarts-a-run.md).

## Goal

Every project Yulius runs merges its armed pull requests through one queue: a pull request's own
CI run (its pre-check) always finishes, green pull requests merge as soon as they can, a pull
request whose base moved since its run is tested once on top of the current integration branch (alone
or with the others waiting, as a merge candidate), and every armed pull request says in plain
words where it stands. A merge made by anyone on a head whose own Yulius CI run passed counts as
merged on green.

## Context

- **Evidence, quartio-v2, 2026-11-05 to 11-07** (the request): 45 to 60 minute runs, 6 to 9 open
  pull requests, one lane per job.
  - Every merge into `dev` restarted the other armed pull requests' runs. One lost a nearly
    finished run three times in a night. Throughput fell to about one merge an hour, with 6 to 8
    hour queues.
  - Tasks with green pull requests held their merge so as not to cancel another run (one sat
    green for 1 h 45 min).
  - Green, clean armed pull requests stayed unmerged for 15 to 40 minutes (six cases); armed
    merges stayed on old heads after Yulius moved them (about ten nudges); two sat "Queued" on
    stale heads for 2 to 3 hours.
  - A task merged by hand, pinned to its green head (`gh pr merge --match-head-commit`), and its
    `/goal` was refused forever ("Yulius didn't perform the merge").
- **Why runs restart (code, `agent_wait.rs`).** `merge_armed_task` called `keep_armed_current`
  (DLV-203) before it looked at the checks: an armed pull request behind its base got `dev` merged
  in and pushed while its own run was still going. The CI loop then superseded that run
  (`ci/mod.rs`, `plan.supersede`, "superseded by …") and queued one for the new head. Every merge
  into `dev` did this to every armed pull request behind it.
- **Why green ones wait (code, `merge_train.rs`).** A train formed from armed pull requests whose
  own runs had not failed, running or not, and held every member after the first while its run
  went (TRN-004), up to a whole run (45 to 60 minutes here). The first member stayed in the
  one-by-one flow, so DLV-203 could still move its head under the train. A red train sent its
  members one by one (DLV-203 each, a run each).
- **Why the armed merge moves badly.** `delivery_merge` called again for the same head re-armed
  it: a new `armed_at` took it out of its train ("#n was armed again"), the train dissolved and its
  run was cancelled. Tasks told to "call `delivery_merge` again" nudged their own merges back.
- **Why merges are slow to fire.** A pass runs at the end of every CI run and every 3 minutes
  after the delivery reconcile (`background.rs`). A train waiting for GitHub to show a pushed head,
  or a merge GitHub could not take yet, waited up to the next 3-minute round, behind the
  reconcile of every delivery.
- **Why the hand merge failed `/goal`.** Claude Code's goal evaluator reads the transcript. After a
  hand merge the armed merge pass answered "The merge you armed was refused: …" (merging a merged
  pull request is refused), and `delivery_merge` said only "already merged … nothing to deliver".
  Nothing said the merged head had passed Yulius CI, and the definition of done reads "merged …
  with `delivery_merge` on green Yulius CI".
- **What stays.** GitHub's ruleset needs `yulius/ci` at success on the exact head and the head up
  to date with its base (strict); SPEC-084's train machinery already builds the integration branch
  plus heads in a scratch worktree, runs one CI run on it (`train/…`, never superseded by the CI
  loop), brings each member up to date with the repository's merge drivers and records the
  train's verdict at the pushed head so GitHub accepts the merge without another run.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| MQ-001 | **A pre-check is never restarted by the base.** Yulius does not bring an armed pull request up to date (DLV-203) or put it in a merge candidate until its own Yulius CI run at the armed head passed. A moving integration branch therefore never supersedes a pull request's run in progress. Only the agent's own new commits supersede it. | `merge_train::three_green_pull_requests_land_in_order_without_a_restart`; `merge_train::a_moving_base_never_restarts_a_pre_check` |
| MQ-002 | **Green and current merges at once.** An armed pull request whose run passed at its head, and whose head contains the integration branch, merges on the next pass, also while a merge candidate of others is under test. That candidate's base moved, so it is dissolved and formed again. | `merge_train::a_green_current_pull_request_merges_on_the_next_pass` |
| MQ-003 | **One merge candidate when the base moved.** Armed pull requests whose run passed but whose base moved since go into one merge candidate, a train (SPEC-084) of 1 to 8 members in arm order: the integration branch with each merged in, tested by one run. Every member waits for it; none is brought up to date or merged alone meanwhile. On green they merge in order with no further run. A member whose agent is busy at its turn is waited for, up to 15 minutes (`WAIT_LIMIT_MS`), instead of ending the candidate. A member that cannot merge for a reason of its own (a conflict after all, a refused merge, an owner who can no longer act) goes one by one, where it is told why, instead of the same candidate being run again. | `merge_train::three_green_pull_requests_land_in_order_without_a_restart` (3 pre-checks + 2 candidates, nothing cancelled); `merge_train::armed_pull_requests_merge_together_on_one_run`; `merge_train::a_busy_member_of_a_passed_candidate_is_waited_for`; `merge_train::a_member_that_cannot_merge_is_told_instead_of_tested_again` |
| MQ-004 | **A red candidate is bisected.** A candidate of n ≥ 2 members whose run failed is formed again from its first ⌈n/2⌉ members; the limit ends with a candidate that passes. A failed candidate of one drops that armed merge, in plain words: it passed alone but fails on top of the current integration branch, which run says why, and what to do. A candidate that errored or was cancelled sends its members one by one (DLV-203 after their run passed), as before. | `merge_train::a_red_candidate_is_bisected_and_the_breaking_pull_request_told` |
| MQ-005 | **When no candidate can be made, one by one.** A project without Yulius CI, with rebase merges or automatic promotion, a candidate that cannot be built, and a pull request that conflicts with the integration branch alone go one by one: Yulius brings the branch up to date (DLV-203) once its own run passed, its agent idle, and that head's run decides; a conflict is reported as before. | `merge_train::a_pull_request_that_conflicts_in_the_train_is_left_out`; `merge_train::a_cancelled_candidate_sends_its_members_one_by_one`; `delivery` suite (DLV-203) |
| MQ-006 | **The armed merge follows the pull request.** When Yulius merges the integration branch into an armed pull request (DLV-203 or a candidate's merge), the merge stays armed on the new head. `delivery_merge` called again for the armed head, or for a head reached from it only through clean merges of the integration branch, keeps the armed merge and its place in the queue (its `armed_at`). It does not arm it anew. | `merge_train::a_yulius_base_merge_keeps_the_merge_armed`; `merge_train::arming_the_same_head_again_keeps_its_place`; `merge_train::an_armed_merge_follows_the_agents_own_clean_merge_of_dev` |
| MQ-007 | **Every armed pull request says where it stands.** Each pass records one plain sentence per armed pull request: waiting for its own run, in candidate X at place k, waiting for the next candidate, waiting for its agent's turn to end (or uncommitted changes), or brought up to date and waiting for that run. `yulius wait` shows it (`merge_train`, informative, it never ends a wait), and so do `delivery_status` and the task page (delivery notes). | `merge_train::every_armed_pull_request_says_where_it_stands`; `merge_train::an_errored_run_says_to_rerun_it`; `agent_wait::tests::a_decision_leaves_where_the_merge_stands` |
| MQ-008 | **Armed merges fire within a minute.** Besides the end of every CI run and the 3-minute delivery round, a fast pass runs every 20 seconds for sandboxes with an armed merge that could be decided now: a candidate whose run ended, or an armed head whose own run passed and that is up to date, or, with no candidate under test, whose agent is idle. A green head waiting for a candidate under test or for its busy agent does not count, so the pass asks GitHub only about merges that can happen. | `merge_train::a_green_current_pull_request_merges_on_the_next_pass`; `merge_train::every_armed_pull_request_says_where_it_stands` (nothing decidable while a candidate runs); the compile-time bound on `DECIDABLE_EVERY` in `background.rs` |
| MQ-009 | **A merge on a green head counts, whoever made it.** When an armed or delivered pull request is found merged, by Yulius or anyone else, Yulius names the merged head and whether its own Yulius CI run passed there. If it passed, the outcome says the merge counts as merged on green Yulius CI. If not, it says so. `yulius wait`'s outcome and `delivery_merge`'s answer say it. The definition of done reads "merged … on green Yulius CI (by `delivery_merge`, or by anyone at a head whose own Yulius CI run passed)". | `merge_train::a_pinned_hand_merge_on_a_green_head_counts`; `request::tests` |
| MQ-010 | **Nothing project-specific.** The queue is on for every sandbox, with no setting. The integration branch, merge method, merge drivers and jobs come from the project. A second, minimal project (integration branch `trunk`, merge commits, no changes log, one job) gives the same results. | `merge_train::three_green_pull_requests_land_in_order_without_a_restart` run for both projects |
| MQ-011 | **Task guidance.** The briefing (within its byte budget) tells every task to merge the integration branch only for a conflict and never to hold a green pull request back, since the queue merges in turn; the definition of done and the bundled `delivery-pipeline` and `git-hygiene` skills say it in full: arm the merge as soon as the review approved the head, the queue orders the merges and never restarts a run, merging the integration branch only because it moved restarts your own run, and an armed head is not armed again. | `briefing::tests`; `request::tests` |

## Plan

- [x] Write the spec and the ADR for the merge queue
- [x] Pre-checks finish: no base merge into an armed pull request before its run passed
- [x] Merge candidates of one or more green pull requests, every member held, busy members waited for
- [x] A red candidate is bisected and a lone breaking pull request told why
- [x] An armed merge keeps its place when armed again and follows Yulius's base merges
- [x] Every armed pull request says where it stands in wait, delivery_status and the task page
- [x] A fast armed pass merges a green current pull request within a minute
- [x] A merge on a green head counts for the goal, whoever made it
- [x] The same queue results for a second, minimal project
- [x] Task guidance in the briefing and bundled skills
- [x] Architecture, product and changes documents updated
- [x] Independent review findings answered test-first
- [x] Gates green and the evidence recorded

## Acceptance criteria

- Three pull requests turn green in sequence while the base moves after each merge: no run is
  cancelled, and all three land after 3 pre-checks and at most 2 candidate runs, in both test
  projects.
- A Yulius base merge into an armed pull request keeps it armed on the new head.
- An armed, green, up-to-date pull request merges on the next pass (at most 20 seconds), also
  while a candidate of others is under test.
- A hand merge pinned to a green head ends the armed merge with an outcome saying it counts as
  merged on green Yulius CI.
- `scripts/dev ci` passes.

## Test plan

- Integration (`crates/yulius/tests/merge_train.rs`), with real repositories, the production Git
  adapter, real Yulius CI runs and the fake GitHub enforcing `yulius/ci` and strict up-to-date
  merges:
  - three pull requests green in sequence, the base moving after each merge, for the standard
    project and for a minimal one (`trunk`, merge commits, no changes log), counting executed and
    cancelled runs;
  - a moving base during a pre-check;
  - a green current pull request merging while a candidate of others is under test;
  - a red candidate of three bisected down to the breaking pull request, which is told why;
  - a Yulius base merge keeping the merge armed, and arming the same head again keeping its place;
  - every armed pull request's note;
  - a hand merge pinned to a green head.
- The existing SPEC-084 train tests, updated where the queue changes them (members need a passed
  pre-check; a red train is bisected).
- Unit: the hold of every member, the armed slot's note, what a merge counts as, the definition
  of done and the briefing wording; the fast pass's period is bounded when compiling.

## Decisions

- **Pre-check first, then a candidate.** Moving the head of a pull request whose run is going
  costs that run, and with 45 to 60 minute runs most runs were thrown away. Waiting for the
  pre-check costs nothing: the pull request could not merge before it passed anyway. See ADR-0285
  for the alternatives (testing every head merged into the base, GitHub's merge queue, keeping the
  old run alive beside the new one).
- **A candidate is a train of one or more.** SPEC-084's machinery is the candidate: built in
  scratch, one run that the CI loop never supersedes, the verdict recorded at the pushed head. A
  candidate of one still never moves the head before it passed.
- **Green and current merges at once, even mid-candidate.** The request asks that a green,
  mergeable pull request never wait for other runs. A candidate under test then has a moved base
  and is formed again. That costs at most the candidate run, and only when a pull request's own run
  already had the newest base, which the task guidance makes rare.
- **Bisect by halves, drop only a proven breaker.** A red candidate of one is proof: the pull
  request passed alone and fails on the current base. Its agent must change something, so the armed
  merge ends and says why. Larger red candidates halve until one passes or one is proven.
- **Same head, same place.** Re-arming the same head is a no-op that keeps `armed_at`. It used to
  reset the queue position and dissolve trains. One window is left: while a pass is deciding the
  merge (it is taken out of the slot for seconds), `delivery_merge` for the same head arms it
  anew, as before (WAI-002), with a new `armed_at`. That takes it out of a candidate it is in.
  Closing it means keeping the head being decided in the slot. That is not worth the
  complexity while a decision lasts seconds and agents are told not to arm an armed head again.
- **A fast pass, not a shorter delivery round.** The 3-minute round reconciles every delivery with
  GitHub. The 20-second pass only looks at sandboxes with a decidable armed merge (local records),
  so GitHub calls grow with the merges that can happen, not with the open pull requests.
- **Green at the merged head is the fact that counts.** Who pressed merge does not change what
  was tested. Yulius reports the merged head and its run, and the goal text says such a merge
  counts.

## Evidence

- **Red first.** Each change was switched off in turn on committed code, its test was seen to
  fail, and the change was restored with `git checkout`:
  - DLV-203 before the checks again:
    - `a_moving_base_never_restarts_a_pre_check` fails ("untouched": the head moved);
    - `three_green_pull_requests_land_in_order_without_a_restart` fails (a pre-check superseded
      and cancelled).
  - Candidates admitting a pull request whose own run had not passed:
    `a_moving_base_never_restarts_a_pre_check` fails ("no merge candidate before its pre-check
    passed"). The three-PR test alone did not catch this, because its pre-checks happened to
    finish first, so that assertion was added.
  - The first member left free: `merge_train::tests::a_train_holds_every_member` fails.
  - Re-arming the same head arming anew: `arming_the_same_head_again_keeps_its_place` fails.
  - No check of a merged pull request in the pass: `a_pinned_hand_merge_on_a_green_head_counts`
    fails (the merged pull request was held in a candidate, still armed).
  - Deciding in store order instead of arm order: `armed_pull_requests_merge_together_on_one_run`
    fails (#3 merged before #1).
  - The old fast-pass predicate: `every_armed_pull_request_says_where_it_stands` fails (decidable
    while a candidate runs), and `uncommitted_changes_leave_nothing_to_decide` was red before its
    fix.
  - `Step::Stop` instead of `Step::OneByOne`: `a_member_that_cannot_be_updated_goes_one_by_one`
    fails ("not tested again": a second candidate run).
  - `a_member_that_cannot_merge_is_told_instead_of_tested_again` passes either way, because the
    same pass's one-by-one decision already tells the agent. It documents the outcome only.
- **Gate.** `scripts/dev ci` passed at 54c5397e (exit 0):
  - format, lint, the repository contracts (16 crates, 7 skills, links, unique decision numbers);
  - 3,228 of 3,228 tests, 16 skipped;
  - docs.

  The run before it, at e456d60a, failed one test of 3,226:
  `local_ci::the_security_job_and_the_others_never_cancel_each_other`. Its jobs did not reach
  "running" within 60 s at a load average of about 40. That test involves no armed merge. It
  passed alone three times (16 to 18 s), and the open issue #1079 tracks this flake. It passed in
  the run at 54c5397e.
- **Suites run on the way:**
  - the 22 `merge_train` integration tests, across both test projects;
  - `delivery`, `wait_cli`, `playground`, `local_ci`, `github_checks`, `agent_tools` and
    `follow_up`: 168 passed. Four `delivery` DLV-203 tests were updated: the head's own checks
    pass first, and the wait shows the queue note;
  - the `yulius-app` and `yulius-domain` unit tests.
- **Independent review.** Three rounds, by a fresh read-only subagent:
  - round 1: 1 blocker (format), 2 majors (the fast pass's GitHub cost, missing tests), 7 minors;
  - round 2: 1 major (no test failing without `OneByOne`), 3 minors;
  - round 3: approved 54c5397e, conditional on this gate.

  Every finding was fixed test-first, or documented: the re-arm window during a decision, under
  Decisions.
- **Security scan.** Vidoc is switched off for this project in Yulius: `yulius security-scan`
  printed "Security scan disabled". Disabled, not passed.
- **Could not be tested here:**
  - Throughput on quartio-v2. Its repository is outside this sandbox's GitHub access, and the
    change is not deployed yet: a person deploys Yulius. The request's figures are the baseline:
    about one merge an hour and 6 to 8 hour queues, 2026-11-05 to 11-07. The figure after
    needs quartio-v2's next busy day on the deployed build.
  - The goal evaluator itself (Claude Code's `/goal`), which reads the transcript. The tests prove
    the words a task now sees after a hand merge on a green head, and the definition of done
    that names such a merge.

