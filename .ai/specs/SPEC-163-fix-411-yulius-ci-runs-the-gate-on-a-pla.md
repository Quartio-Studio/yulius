---
yulius-task: 74d4c263-d6cc-4bee-b745-f30209b8bad0
status: done
---

# SPEC-163: Yulius CI runs the gate on a playground's task heads and its integration branch

Status: done
Date: 2026-10-15
Owner: Yulius task #130 (part of task #118, SPEC-138)
Scope authorization: issue [#411](https://github.com/Quartio-Studio/yulius/issues/411), the
follow-up of [SPEC-099](SPEC-099-on-onboarding-we-want-a-mode-to-play-aro.md): "Yulius CI tests a
playground's integration head and, when `delivery_merge` is called, the task head merged into it,
with the same jobs as `[[ci.jobs]]`; `delivery_merge` waits for green as on GitHub (require_checks
of the profile)." Acceptance: a playground's integration head and a delivered task head get CI
runs; `delivery_merge` in a playground refuses or arms on failing or running checks like on
GitHub; no commit status is published to GitHub for a playground.

## Goal

A playground (a sandbox whose Git stays on the server, ADR-0081) gets the same Yulius CI as a
GitHub project: its integration head is tested once per commit, and a task's `delivery_merge`
squashes onto the integration branch only once the CI passed on the task head merged into it.
While that run goes, the merge is armed and Yulius lands it when it passes, as on GitHub. Nothing
of it reaches GitHub.

## Context

- **Today** `ci_sync_sandbox` returns early for a playground (SPEC-099 PLY-008), so its CI page
  stays empty, and `playground_merge` (PLY-007) squashes at once: the agent runs the gates itself.
- **CI on GitHub** (`ci/mod.rs`): each sync round fetches the clone, reads the CI in force on the
  integration branch (`ci_gate`: the profile's gate, else `[[ci.jobs]]` or `[commands] gate`),
  lists open pull requests and plans runs per reference (`pr/<n>`, `branch/<name>`, one active
  run per reference, superseded when it moves). A branch commit whose tree a passed run tested
  reuses that result (`ci_reuse_tree`). Each run's state is published as the commit status
  `yulius/ci`; a run's end decides the armed merges of its sandbox (`merge_armed`).
- **Merge trains** (SPEC-084) already test a commit that exists only in Yulius's clone: the
  integration branch with pull request heads merged in (`GitPort::build_train`, reference
  `train/<n>+<m>`, no commit status).
- **The agent's merge** (`tool_delivery_merge`, SPEC-011, DLV-203): checks passed → merge;
  failed or cancelled → refused ("merge on green only"); running, queued, errored or not known
  yet → armed, and Yulius merges that exact head when its checks pass (`yulius wait` says how it
  ended). A profile without required checks merges on "none".
- **The domain's merge gate** (`check_merge`) treats "no checks" as passable even with
  `require_checks`; GitHub's "none" only means "nothing reported yet", so the agent's path arms
  on it there.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| PCI-001 | A playground's CI sync calls no GitHub API and asks for no token: it fetches the playground's own `origin` (as delivery does) and tests the head of its integration branch with the CI in force there, once per commit, superseding a run whose head moved; a commit with a tree a run already passed reuses that result. Its production branch is not tested: only a promotion moves it, and a playground has none. Runs of playground tasks (PCI-002) are never superseded by the sync. | `playground::a_playgrounds_dev_is_tested_without_github` |
| PCI-002 | `delivery_merge` in a playground, after the task gates of PLY-007 (valid spec, steps resolved, clean worktree, reviewed head, commits to deliver), tests the task head merged into the integration branch as the integration branch is now: Yulius builds that merge in its clone (the repository's merge drivers apply; nothing is pushed) and queues a run of it, reference `task/<n>` ("Task #n"). The run takes the task's own jobs when the task changed them (CI-110), and the task's jobs when the integration branch has none yet (bootstrap). A run of that task that tested exactly this head merged into this integration head (the build's parents say so) is reused, not built again; a run of a later commit that contains the head never counts (a branch reset to an older commit is tested again). Once it passed, the squash lands exactly its tree (Git's own squash may order a `merge=union` file the other way), so the integration head after the merge reuses its result. A head that conflicts with the integration branch is refused naming the files and the remedy, as before. | `playground::delivery_merge_tests_the_head_merged_into_dev_first`, `a_union_merged_file_lands_the_tested_tree`, `a_reset_head_never_takes_a_later_heads_verdict` |
| PCI-003 | The verdict of that run decides, as on GitHub: passed → squashed now; failed or cancelled → refused ("merge on green only") and nothing lands; queued, running or errored → the merge is armed (an errored run says to run it again with `ci_rerun`), and when a run of the sandbox ends Yulius decides it: it squashes the reviewed head once its run passed, drops it with why when the run failed or the head conflicts (a failure of Git or the store keeps it armed for the next pass; a task merged meanwhile by a person ends it), and builds and tests a new merge when the integration branch moved meanwhile (the older run is cancelled as superseded). A head that moved since arming keeps the armed merge only through clean merges of the integration branch (SPEC-083). Without CI (none configured, or CI off on this instance) there is nothing to wait for and the squash happens at once, as before. Merge trains (SPEC-084) never form in a playground. | `playground::delivery_merge_tests_the_head_merged_into_dev_first`, `a_failing_head_is_refused_and_its_armed_merge_dropped`, `an_armed_merge_follows_a_moved_dev` |
| PCI-004 | The task's merge action (a person) applies the same verdict but never arms: while the run goes it answers 409 saying that Yulius CI is testing the merge, and a failed run is a 409 saying so. As on GitHub, a profile without `require_checks` lets a person merge without waiting. A merge the agent armed is over once a person merged. | `playground::a_persons_merge_waits_for_green`, `a_persons_merge_without_required_checks_lands_at_once` |
| PCI-005 | No commit status is published for a playground: a playground's run records "not published: a playground has no GitHub repository" and calls no GitHub API, retries included; the sandbox's browsers still hear of its change. | `playground::a_playgrounds_dev_is_tested_without_github` (the fake GitHub records no write) |
| PCI-006 | `yulius wait` and `ci_status` follow a playground task's run: the checks at the agent's head are those of the newest run of its task that tested exactly that head merged into the integration head as Yulius's clone has it, and `ci_status` lists the task's runs first (`this_task`). | `playground::delivery_merge_tests_the_head_merged_into_dev_first` (wait state) |
| PCI-007 | A playground's agents are told: the briefing, the request's definition of done and the workflow note say that Yulius CI tests the head merged into the integration branch before `delivery_merge` squashes it, list `ci_status` and `ci_rerun`, and say to wait with `yulius wait`. | `request::a_playground_done_definition_merges_locally`, `briefing::a_playgrounds_briefing_merges_locally` |
| PCI-008 | SPEC-099 (PLY-002, PLY-006, PLY-007, PLY-008), ADR-0134, `docs/architecture.md` and `docs/changes.md` describe it. | docs |

## Plan

- [x] Spec SPEC-163 written and validated
- [x] A playground's integration head gets Yulius CI runs, with no GitHub status
- [x] delivery_merge tests the head merged into dev and squashes only on green
- [x] Armed playground merges land, drop or follow a moved dev when a run ends
- [x] The person's merge action waits for green in a playground
- [x] yulius wait and ci_status follow a playground task's run
- [x] A playground's agents are told about its CI
- [x] SPEC-099, ADR-0134, architecture and changes describe playground CI
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Gates green, delivered and merged into dev

## Acceptance criteria

- A playground with `[[ci.jobs]]` on `dev` gets a run of `branch/dev` at each new `dev` head, with
  no GitHub call and no commit status.
- `delivery_merge` of a ready task queues a `task/<n>` run of its head merged into `dev` and arms;
  `dev` does not move until that run passes; then Yulius squashes it, and the following `dev` run
  reuses the result.
- A failing run refuses the next `delivery_merge` and drops the armed merge; nothing lands.
- A person's merge answers 409 while the run goes and merges once it passed.
- A playground without CI merges at once, as before (the PLY-007 tests still pass).

## Test plan

- Integration (`crates/yulius/tests/playground.rs`, real Git, real process host running the jobs,
  the fake GitHub recording writes): the integration head is tested without GitHub; the agent's
  merge is armed, lands on green and the `dev` run reuses the tree; a failing head is refused and
  its armed merge dropped; a moved `dev` rebuilds the merge and supersedes the older run; a
  person's merge waits for green. The PLY tests keep passing unchanged (templates have no CI).
- Unit (`yulius-domain` CI): the `task/<n>` reference's key, parse, label and coverage.
- Unit (`yulius-domain` request and briefing): the playground texts mention Yulius CI and the wait.

## Decisions

- **Test a merge built in Yulius's clone, not the task head alone** (ADR-0134). The issue asks for
  the head merged into the integration branch; it is what the squash lands, without touching the
  agent's worktree (GitHub's DLV-203 update merges into the branch and pushes it, which a
  playground's branch, never pushed, does not need). The merge train's builder makes it, with a
  merge-commit landing whose parents name what it tests (the integration head, then the head or
  Yulius's merge of the integration head into it): the head's run is matched exactly, never by
  ancestry, which let a branch reset to an older commit take a later commit's verdict (review
  round 1). Once it passed, the squash lands the tested commit's tree: Git's squash of the head
  orders a `merge=union` file the other way (review round 1). Testing the bare head would pass a
  branch that is behind and breaks once merged.
- **One reference per task (`task/<n>`)**, not per head: retention keeps the newest verdict of
  each reference, so a per-head key would never be pruned. The run of the current head is found
  by the build's parents.
- **No CI → squash at once.** In a playground Yulius knows there is no CI (nothing will ever
  report), so "none" cannot mean "not reported yet" as on GitHub; the domain's merge gate already
  treats no checks as passable.
- **Error keeps the merge armed, cancelled refuses it**, as the agent's GitHub path maps them
  (`error` arms, `failure` covers failed and cancelled). An errored run is not rebuilt by itself:
  the answer names the run to `ci_rerun`. No test makes a run error (a job timeout takes the
  configured job limit); the mapping is the same code path as the other states.
- **A person's merge honours `require_checks`** as GitHub's merge gate (`check_merge`) does; the
  agent's merge always waits for green, as on GitHub (review round 1).
- **An armed merge survives failures that are not refusals** (a fetch racing the CI sync's, a
  store error): it is decided again on the next pass, as the GitHub path keeps it.
- **Only the integration branch is tested.** The production branch of a playground moves only by
  promotion, which a playground does not have.
- **The candidate builds are serialized per sandbox** with the existing armed-merge pass lock, so
  two merges never share the scratch checkout at once.

## Evidence

Run on the branch after merging `dev` (2026-10-15):

- `scripts/dev ci` at 5df70105: exit 0. Repository contracts and script tests passed; nextest
  `1831 tests run: 1831 passed, 18 skipped`; fmt, clippy with `-D warnings`, doctests, cargo-deny,
  cargo-shear and rustdoc clean. An earlier run (before the review fixes) had one failure in
  `first_message::a_manual_switch_of_a_held_first_message_keeps_its_image` ("task state changed
  concurrently"), a timing-sensitive test this change does not touch; it passed 5 runs of 5 alone
  and in the next full run.
- `cargo test -p yulius --test main playground::`: 31 passed, among them the eight new ones
  (`a_playgrounds_dev_is_tested_without_github`, `delivery_merge_tests_the_head_merged_into_dev_first`,
  `a_failing_head_is_refused_and_its_armed_merge_dropped`, `an_armed_merge_follows_a_moved_dev`,
  `a_persons_merge_waits_for_green`, `a_union_merged_file_lands_the_tested_tree`,
  `a_reset_head_never_takes_a_later_heads_verdict`,
  `a_persons_merge_without_required_checks_lands_at_once`). Each failed before its change: the
  first because no run existed, the merge test because the squash landed at once, the review
  regressions with the untested union order, a reset head merged on a later head's verdict and a
  409 without required checks. The PLY tests pass unchanged (templates have no CI).
- `cargo test -p yulius-domain --lib`: 330 passed (`refs_round_trip` with `task/<n>`; the
  playground request and briefing texts).
- `scripts/dev security`: exit 2, not run: Vidoc rejected the key (HTTP 401), the known state of
  the agent secret since 2026-10-14 (ADR-0104: unrun, not passed).
- Independent review: two rounds in a fresh session. Round 1 asked for changes (two majors: the
  squash could land an untested `merge=union` order; a reset head could take a later head's
  verdict; and minors), all fixed test-first; round 2 approved 5df70105.
- Not tested here: a run that errors (a job timeout takes the job limit; same mapping as the other
  states), a person merging while the agent's merge is armed (racy with the merge Yulius makes at
  the run's end; the slot is cleared and the pass ends on a merged task), and a real agent's
  `yulius wait` in a playground (the wait endpoint is exercised).
