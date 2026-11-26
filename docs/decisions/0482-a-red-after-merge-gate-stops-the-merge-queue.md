# 0482. A red after-merge gate stops the merge queue until the integration branch is green

## Status

Accepted. Specified in [SPEC-676](../../.ai/specs/SPEC-676-a-red-after-merge-gate-stops-the-project.md)
(RGS-001 to RGS-010); task #660. Adds a rule to
[0285](0285-armed-pull-requests-merge-through-a-queue-that-never-restarts-a-run.md)'s merge queue,
beside [0410](0410-the-full-suite-runs-on-the-merged-revision-beside-a-merge-first-staging-target.md)'s
merge-first staging. It reads the open failures of
[0450](0450-attribute-integration-test-failures-in-first-parent-order.md)'s first-parent
attribution.

## Context

With merge-first staging, the full suite runs on the merged revision, so pull requests merge
without it. Nothing kept them from merging onto an integration branch whose after-merge gate was
red. On quartio-v2 on 2026-11-25, merges kept landing on a red `dev`:

- later failures were attributed to the wrong task (task #414 got "inherited failures from #412",
  and task #415, a docs-only pull request, was shown "tests (4/4) failed");
- tasks repaired `dev` for one another;
- staging stood still.

The owner kept merge-first, since running the full suite before every merge takes too long, and
added one rule: a red after-merge gate stops the merge queue until the branch is green again.

## Decision

- **The setting.**
  - `[merge_queue] stop_on_red_gate` in `yulius.toml`, read at the integration head like `apart`.
  - Without the key it is on exactly when the project has a merge-first staging target.
- **Stopped** means the gate history at the integration head (settled verdicts since the last
  full pass) has an open failure that is not a quarantined flaky test of the project. The state
  is read at most every 15 s per project, and at once for a new integration head.
- **Held, not dropped.**
  - The armed-merge pass and `delivery_merge` merge no pull request that is not a repair. Each
    one stays armed in its place.
  - Its note names the failures, their revisions and tasks, and says that it merges by itself
    once a later verdict is green.
  - A candidate under test that holds a pull request that is not a repair is dissolved (why
    `gate_red`). A new candidate forms only from repairs.
- **A repair merges.** The task marks it with `delivery_merge`'s `repairs_gate: true`, or Yulius
  names its task as the merge owner of an open failure. Both are audited.
- **The breaker repairs first.**
  - The repair assignment prefers the task that brought in an open failure among the available
    workers. The incumbent keeps the repair.
  - While the queue is stopped, the assignment says that merges wait for the repair and how to
    mark it.
- **It resumes by itself** at the first pass after a later verdict, or a rerun, passes.
- The CI tab of the CI/CD page shows the stop: its failures, its run, and the waiting pull
  requests.

## Alternatives

- **Run the full suite before every merge** (no merge-first): the owner refused it because the
  suite takes too long for every pull request.
- **A database setting on the project page**: it would split how a project merges between its
  repository and the instance, and it could not be reviewed in a pull request. `[merge_queue]`
  already lives in the repository.
- **Off by default everywhere**: every other merge-first project would repeat quartio-v2's
  2026-11-25 until someone found the key. Without merge-first, the full gate checks each pull
  request, and the integration branch's verdicts stop nothing. That is why the default follows
  merge-first.
- **Drop armed merges while the queue is stopped**: agents would arm again and again, and the
  queue order would be lost. Holding them keeps their places.
- **Let a candidate under test finish and merge**: its run tests on top of the red branch and
  merges more onto it, which is what the rule forbids.
- **Only a person may declare the repair**: the stop would then wait for a person in every
  project. A wrong mark merges one pull request onto a red branch, which is the old behavior, and
  it is audited.

## Consequences

- On merge-first projects (quartio-v2, Yulius itself), a red after-merge gate holds every merge
  that is not a repair until the gate is green. A flake that is not quarantined holds them too,
  until it is rerun or quarantined: the CI tab offers the rerun.
- When no task repairs the gate, the queue stays stopped until a person reruns the run or starts
  a task. The CI tab says that nobody is working on the repair.
- A person's own Merge of a green pull request on the task page is not held. Their armed merge
  waits like any other.
