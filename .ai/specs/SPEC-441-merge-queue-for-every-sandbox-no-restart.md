---
yulius-task: 50457a7b-0346-4616-bc3e-70c37de8b10b
status: in-progress
---

# SPEC-441: A merge queue for every project: no restarts when the base moves

Status: in progress
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
| MQ-003 | **One merge candidate when the base moved.** Armed pull requests whose run passed but whose base moved since go into one merge candidate, a train (SPEC-084) of 1 to 8 members in arm order: the integration branch with each merged in, tested by one run. Every member waits for it; none is brought up to date or merged alone meanwhile. On green they merge in order with no further run. A member whose agent is busy at its turn waits (bounded, TRN-006), instead of ending the candidate. | `merge_train::three_green_pull_requests_land_in_order_without_a_restart` (3 pre-checks + 2 candidates, nothing cancelled); `merge_train::armed_pull_requests_merge_together_on_one_run` |
| MQ-004 | **A red candidate is bisected.** A candidate of n ≥ 2 members whose run failed is formed again from its first ⌈n/2⌉ members; the limit ends with a candidate that passes. A failed candidate of one drops that armed merge, in plain words: it passed alone but fails on top of the current integration branch, which run says why, and what to do. A candidate that errored or was cancelled sends its members one by one (DLV-203 after their run passed), as before. | `merge_train::a_red_candidate_is_bisected_and_the_breaking_pull_request_told` |
| MQ-005 | **When no candidate can be made, one by one.** A project without Yulius CI, with rebase merges or automatic promotion, a candidate that cannot be built, and a pull request that conflicts with the integration branch alone go one by one: Yulius brings the branch up to date (DLV-203) once its own run passed, its agent idle, and that head's run decides; a conflict is reported as before. | `merge_train::a_pull_request_that_conflicts_in_the_train_is_left_out`; `delivery` suite (DLV-203) |
| MQ-006 | **The armed merge follows the pull request.** When Yulius merges the integration branch into an armed pull request (DLV-203 or a candidate's merge), the merge stays armed on the new head. `delivery_merge` called again for the armed head, or for a head reached from it only through clean merges of the integration branch, keeps the armed merge and its place in the queue (its `armed_at`). It does not arm it anew. | `merge_train::a_yulius_base_merge_keeps_the_merge_armed`; `merge_train::arming_the_same_head_again_keeps_its_place` |
| MQ-007 | **Every armed pull request says where it stands.** Each pass records one plain sentence per armed pull request: waiting for its own run, in candidate X at place k, waiting for the next candidate, waiting for its agent's turn to end (or uncommitted changes), or brought up to date and waiting for that run. `yulius wait` shows it (`merge_train`, informative, it never ends a wait), and so do `delivery_status` and the task page (delivery notes). | `merge_train::every_armed_pull_request_says_where_it_stands`; `agent_wait::tests` |
| MQ-008 | **Armed merges fire within a minute.** Besides the end of every CI run and the 3-minute delivery round, a fast pass runs every 20 seconds for sandboxes with an armed merge that could be decided now (its head's run passed, or a candidate that passed). It costs nothing for sandboxes with none. | `merge_train::a_green_current_pull_request_merges_on_the_next_pass`; `background::tests::the_fast_armed_pass_runs_well_within_a_minute` |
| MQ-009 | **A merge on a green head counts, whoever made it.** When an armed or delivered pull request is found merged, by Yulius or anyone else, Yulius names the merged head and whether its own Yulius CI run passed there. If it passed, the outcome says the merge counts as merged on green Yulius CI. If not, it says so. `yulius wait`'s outcome and `delivery_merge`'s answer say it. The definition of done reads "merged … on green Yulius CI (by `delivery_merge`, or by anyone at a head whose own Yulius CI run passed)". | `merge_train::a_pinned_hand_merge_on_a_green_head_counts`; `request::tests` |
| MQ-010 | **Nothing project-specific.** The queue is on for every sandbox, with no setting. The integration branch, merge method, merge drivers and jobs come from the project. A second, minimal project (integration branch `trunk`, merge commits, no changes log, one job) gives the same results. | `merge_train::three_green_pull_requests_land_in_order_without_a_restart` run for both projects |
| MQ-011 | **Task guidance.** The briefing and the bundled delivery skills tell every task: arm the merge with `delivery_merge` as soon as the review approved the head, and never hold a green pull request back for another task's run. Do not merge the integration branch only because it moved: that restarts your own run, and the queue brings it up to date. Merge it only for a conflict Yulius names. Do not call `delivery_merge` again for an armed head. | `briefing::tests`; `request::tests` |

## Plan

- [x] Write the spec and the ADR for the merge queue
- [ ] Pre-checks finish: no base merge into an armed pull request before its run passed
- [ ] Merge candidates of one or more green pull requests, every member held, busy members waited for
- [ ] A red candidate is bisected and a lone breaking pull request told why
- [ ] An armed merge keeps its place when armed again and follows Yulius's base merges
- [ ] Every armed pull request says where it stands in wait, delivery_status and the task page
- [ ] A fast armed pass merges a green current pull request within a minute
- [ ] A merge on a green head counts for the goal, whoever made it
- [ ] The same queue results for a second, minimal project
- [ ] Task guidance in the briefing and bundled skills
- [ ] Architecture, product and changes documents updated, gates green, independent review answered

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
- Unit: the fast pass's period, the hold of every member, the definition of done and the briefing
  wording.

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
  reset the queue position and dissolve trains.
- **A fast pass, not a shorter delivery round.** The 3-minute round reconciles every delivery with
  GitHub. The 20-second pass only looks at sandboxes with a decidable armed merge (local records),
  so GitHub calls grow with the merges that can happen, not with the open pull requests.
- **Green at the merged head is the fact that counts.** Who pressed merge does not change what
  was tested. Yulius reports the merged head and its run, and the goal text says such a merge
  counts.
