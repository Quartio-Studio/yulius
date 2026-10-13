# SPEC-084: armed pull requests merge in trains, tested once

Status: implemented
Date: 2026-10-13
Owner: engineer T (merge throughput round)
Scope authorization: the person wants many agents at once without merges being rejected or
waiting forever; a merge train in Yulius's armed-merge flow (SPEC-011, SPEC-050 DLV-203/205),
generic for every onboarded repository. ADR:
[0067](../../docs/decisions/0067-armed-pull-requests-merge-in-trains.md).

## Goal

When several agents have armed their merges, Yulius tests their pull requests together once and
merges them one after another on that verdict, instead of one per CI run. Nothing about merging
gets worse: anything unexpected falls back to the one-by-one flow as it was.

## Context

- **The treadmill.** GitHub's ruleset on `dev` requires the `yulius/ci` status and strict "up to
  date" branches.
  - Only one pull request can merge per CI run (about 12 minutes).
  - Each merge puts every other armed pull request behind its base.
  - Yulius then merges `dev` into it (DLV-203), and its CI runs again.
- **Tree reuse today (SPEC-020/SPEC-050).** `ci_reuse_tree` records a passed run for a branch
  head (`dev`, `main`) whose tree a passed run already tested: the squash merge of an
  up-to-date pull request. It never applied to pull request heads.
- **Squash merges keep the tree.** A squash merge of a pull request whose head contains its base
  makes a commit with exactly the head's tree. A merge commit does too. A rebase merge does not
  promise it.
- **Union files.** `docs/changes.md merge=union` resolves the shared log when Yulius merges
  locally (DLV-203). A union merge depends on which side is "ours", so a train must merge
  exactly as DLV-203 does: the integration branch into the head.
- **Data, 2026-10-12/13 (UTC), production `audit_log` and `ci_runs`.**
  - Yulius performed 50 armed merges (2 more were merged at once).
  - From the agent's last arm to the merge: median 14 min, mean 18, p90 32, max 60.
  - 21 of the 50 (42%) were brought up to date by DLV-203 after that last arm: 33 updates.
    They waited a median 23.6 and a mean 26 minutes from their last arm. In total that is 443
    minutes (7.4 h) from their first update to their merge: median 15.3, mean 21.1, max 59.
  - DLV-203 made 96 updates over the two days, counting earlier arms.
  - At each merge into `dev`, other pull requests were armed and unmerged (armed within 6 h):
    mean 2.25, up to 7. At most 4 armed final heads waited at once.
  - CI, the 81 runs still kept, all on 2026-10-13:
    - a passed run took median 12.1 min, p90 15.4;
    - 32 runs were at DLV-203 heads: 22 passed, 7 failed, 3 cancelled;
    - they took 273 of 620 run-minutes (44%);
    - an update's run passed a median 14.3 min after the update;
    - 16 runs were cancelled after they started (113 run-minutes), mostly superseded heads.
  - The 7 failures at DLV-203 heads were two parallel pull requests that broke together: three
    took one ADR number (`check_repository.py`), two did not compile together.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| TRN-001 | A train forms only under all of these conditions. **Pull requests:** two or more armed pull requests of a sandbox wait. Each agent is running and idle, nothing runs in its background, nothing is uncommitted, its branch is at the armed head, its pull request is open and its own run did not fail. **Others:** no armed pull request can merge right now (up to date and passed): that one goes first, on its own. **Project:** it has Yulius CI, merges by squash or merge commit, and does not promote automatically. Members go in arm order (then task number), at most 8. A member of a train that did not pass joins none until its agent arms again. | `merge_train::tests::candidates_go_in_arm_order`; `merge_train::armed_pull_requests_merge_together_on_one_run` |
| TRN-002 | The train is built in Yulius's clone (a scratch worktree under the CI directory) as the merges will happen. The integration branch so far is merged into each head with `git merge`, as DLV-203 does, so the head's `.gitattributes` and the repository's merge drivers apply. Each result then lands as the profile's method lands it: a squash, or a merge commit. A head that conflicts is left out. Nothing is pushed. | `git_ops::a_merge_train_predicts_the_trees_of_merging_one_by_one` (union log, a conflict left out, both landings) |
| TRN-003 | Yulius CI runs once on the train's last commit, with reference `train/<n>+<m>` and label "Merge train #n + #m". It runs the integration branch's jobs, or the members' own when they changed them (CI-110). It is queued ahead of the sandbox's queued runs, since it decides every member's merge. No commit status is published for it: the run notes why. The CI loop neither queues nor supersedes it; the train cancels it when it dissolves. It cannot be run again. | `ci::tests::refs_round_trip`; `merge_train::armed_pull_requests_merge_together_on_one_run` (one train run, one per head) |
| TRN-004 | While the run is queued or running, the first member stays in the one-by-one flow and may merge on its own run. The other members wait for it: not brought up to date (that would only start runs the train makes useless), not merged on their own. A pull request left out waits until the train is over, so the ones armed before it go first. Once the run passed, the train alone decides its members. | `merge_train::tests::a_train_holds_the_members_after_the_first_while_it_runs`; `merge_train::a_pull_request_that_conflicts_in_the_train_is_left_out` |
| TRN-005 | When the run passed, the members merge in order. **Before each:** the integration branch must have the tree the members before it leave, and the member is brought up to date as DLV-203 does. **The head:** it must have the tree the train predicted. The train's verdict is recorded there before the push and published as `yulius/ci`, so GitHub's rules hold without another run. **The merge:** Yulius waits, bounded, until GitHub shows that head, then merges through the normal merge path with all its gates. **After:** the integration branch has the tested tree, and its own CI reuses that result. A verdict recorded for a member before the last is never a source for reusing a result. | `merge_train::armed_pull_requests_merge_together_on_one_run`; `merge_train::tests::each_step_expects_the_tree_of_the_ones_before_it` |
| TRN-006 | A run that fails, errors or is cancelled dissolves the train. Its members merge one by one as before, until armed again. Anything unexpected also dissolves the train (its run cancelled while it runs): the integration branch moving another way, a member re-armed, dropped, busy, moved, conflicting, refused or with another tree, a push failing, or 15 minutes waiting for GitHub or a member's own run. Every member goes back to the one-by-one flow, which decides it with today's messages. | `merge_train::a_red_train_falls_back_to_one_by_one_merges` |
| TRN-007 | `yulius wait` shows where the train has the pull request: its place and the run, or that the members are merging. A pull request left out is told which files conflict and that it merges after the train. The note never ends the wait. A merge made by a train says so in its outcome. | `merge_train::tests::the_wait_says_where_a_pull_request_stands`; `wait_cli::tests::a_merge_train_note_shows_but_never_ends_the_wait`; `merge_train::armed_pull_requests_merge_together_on_one_run`; `merge_train::a_pull_request_that_conflicts_in_the_train_is_left_out` |
| TRN-008 | A train survives a restart. It lives in `<data>/merge-trains.json` (written whole, then renamed into place), its run in the store and its commits in the clone, and the armed merges come back through DLV-205. A train that has not run yet runs. One halfway through its merges continues where it stopped, without another run. | `merge_train::a_merge_train_survives_a_restart` |
| TRN-009 | Nothing assumes this repository. The integration branch, merge method and promotion come from the sandbox's profile, the jobs from `yulius.toml`, the merge drivers from the repository. GitHub checks its own rules (required checks, reviews, up to date) at every merge, and a refusal falls back. Projects without Yulius CI, with rebase merges or with automatic promotion have no trains. | the four `merge_train` integration tests (a repository with its own jobs, union driver and a strict ruleset in the fake GitHub) |
| TRN-010 | Passes that decide a sandbox's armed merges (and its train) take turns. Every run that ends starts one, so two often ran at once. One read a pull request as up to date while the other merged into its base, so GitHub refused the merge as behind, and the armed merge was dropped ("The merge you armed was refused: … not up to date"). A later pass now finds it behind and brings it up to date. | `merge_train::passes_at_the_same_time_take_turns` (fails without the turn-taking with exactly that refusal) |
| TRN-011 | An armed pull request still behind its base is not merged. Its branch lacks the integration branch as the clone last fetched it because its agent was busy, so DLV-203 did not bring it up to date. GitHub would refuse it as behind, and the armed merge was dropped (2 cases on 2026-10-12/13, found by engineer B). It waits, and is brought up to date once its agent is idle. | `merge_train::a_busy_agent_behind_its_base_waits_instead_of_being_refused` |

## Plan

- [x] Spec and ADR
- [x] The train built in a scratch worktree, as DLV-203 and the host merge (`yulius-git`)
- [x] A `train/…` CI reference, kept out of the CI loop and of status publishing
- [x] Formation, hold, merge in order, fallback, restart (`merge_train.rs`)
- [x] `yulius wait` note and outcome
- [x] Integration tests: green, red with fallback, conflict left out, restart halfway
- [x] Passes deciding armed merges take turns per sandbox

## Acceptance criteria

- With two or more armed pull requests waiting, one CI run decides them, and they merge one after
  another under a strict ruleset with no further run.
- A red train, a conflict or a restart never leaves a pull request unmerged that the one-by-one
  flow would have merged.
- `scripts/dev ci` passes.

## Test plan

- `yulius-git`, real repositories: a train with a union-merged log and a conflicting head
  predicts the trees that merging one by one gives; merge-commit landing.
- Unit: hold rules, step trees, the wait note, arm order, the file.
- Integration, in `merge_train`, with real repositories, the production Git adapter, real Yulius
  CI runs and a fake GitHub that enforces `yulius/ci` and strict up-to-date branches: the green
  train, the red train with fallback, a conflicting pull request left out, and restarts before the
  run and halfway through the merges.

## Decisions

- **One run, its verdict for every member.** Only the train's last commit is tested. A member
  before the last gets the train's verdict for a tree no run tested by itself. The integration
  branch passes through those trees for seconds, between merges, on the way to the tested one.
  Such a verdict is never reused for another commit. When a train stops halfway, the integration
  branch's own CI tests where it stopped. See ADR-0067 for the alternatives (testing every
  prefix, GitHub's merge queue).
- **A verdict at the exact head, not tree reuse for pull requests.** The train records its
  verdict at the head it pushes, before the push. Generic tree reuse stays for branch heads.
- **Fall back instead of bisecting.** A red train costs one run. Its members then merge one by one
  as before, each tested on its own, which finds the one that breaks. No train re-forms from them
  until their agents arm again.

## Evidence

- **Data:** see Context. Queries over production `audit_log` (`delivery.agent.merge_armed`,
  `delivery.branch_updated`, `delivery.agent.merge`, `delivery.merge`) and `ci_runs`, read only.
- **Expected gain**, from the same two days:
  - The 33 updates after a last arm each cost about one run before the merge (median 14.3 min
    from update to pass). A train merges those pull requests on the run that tested them with
    the one ahead. That is about 7 hours less waiting over two days.
  - Mean wait from the last arm drops from 18 minutes to about 10.
  - About 30 fewer runs at updated heads. Those runs took 44% of the CI time kept on 2026-10-13;
    a train costs one run per group instead.
  - With 8 to 12 agents, where pull requests pile up behind each other, a group of n waits about
    one run instead of n.
  - Limits: two pull requests that break each other (the 7 failures above) make a train red, at
    the cost of one run before the one-by-one flow. A pull request that can merge at once still
    goes alone.
- **Passes at once (TRN-010).** The test shows the refusal without the turn-taking. In
  production, 13 of the 69 run ends kept for 2026-10-12/13 came within a minute of the one
  before, each starting its own pass. Eight arms read "checks behind". Yulius records no
  refusal text, so how often it happened cannot be counted.
- **Red first.** With trains switched off (`run_merge_train` returning at once), the four train
  tests fail: no train run. Without the turn-taking, `passes_at_the_same_time_take_turns` fails
  with "The merge you armed was refused: … Head branch is not up to date".
- **Tests:** listed in the table. Run on this Mac:
  - `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^merge_train::/)'`: 5
    passed, in parallel.
  - The `yulius-git` train test and the `yulius-app` unit tests passed.
