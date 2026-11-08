# 0285. Armed pull requests merge through a queue that never restarts a run

## Status

Accepted. Specified in [SPEC-441](../../.ai/specs/SPEC-441-merge-queue-for-every-sandbox-no-restart.md);
task #423. Amends SPEC-050 DLV-203 and [ADR-0067](0067-armed-pull-requests-merge-in-trains.md)
(merge trains).

## Context

GitHub's ruleset on the integration branch needs `yulius/ci` at success on the exact head and the
head up to date with its base. Yulius used to bring an armed pull request up to date as soon as it
fell behind, while its own run was still going. Each merge into the integration branch therefore
restarted the run of every armed pull request behind it. With 45 to 60 minute runs and 6 to 9 open
pull requests (quartio-v2, 2026-11-05 to 11-07), most runs were thrown away: about one merge an
hour, 6 to 8 hour queues. Trains (ADR-0067) formed from pull requests whose runs were still going
and held the green ones behind the first for a whole run. Tasks held green pull requests back by
hand, and re-armed merges, which reset their place and dissolved trains.

## Decision

- **Pre-check, then candidate.** A pull request's own run on its own head (the pre-check) always
  finishes: Yulius neither merges the integration branch into an armed pull request nor puts it
  in a candidate before that run passed.
- **Green and current merges at once.** A green pull request whose head contains the integration
  branch merges on the next pass, even while a candidate of others is under test (that candidate
  is formed again on the new base).
- **One candidate when the base moved.** Green pull requests whose base moved go, in arm order (at
  most 8), into one merge candidate: an SPEC-084 train, now of one or more members. One run tests
  the integration branch with all of them merged in. Every member waits for it, and on green they
  merge in order with the verdict recorded at each pushed head.
- **Bisect a red candidate.** A failed candidate of n ≥ 2 is formed again from its first ⌈n/2⌉
  members. A failed candidate of one ends that armed merge, with the reason and the run. An
  errored or cancelled candidate sends its members one by one (DLV-203 after the pre-check).
- **The armed merge follows the pull request.** Yulius's base merges keep it armed on the new head,
  and arming the same head again keeps its place.
- **Every armed pull request says where it stands**, in `yulius wait`, `delivery_status` and the
  task page.
- **A fast pass** every 20 seconds decides the armed merges that can be decided now (a passed run
  at the head, or a candidate that passed).
- **A merge counts by its head, not its author.** A merge at a head whose own Yulius CI run passed
  counts as merged on green, whoever made it; Yulius says so.
- On for every sandbox, with no setting. Projects with rebase merges, automatic promotion or no
  Yulius CI go one by one, as before.

## Alternatives

- **Test every pull request merged into the current base, rerunning on every base move** (what
  happened in effect): correct, but quadratic in runs. With runs that take longer than the gap
  between merges, almost no run finishes.
- **GitHub's merge queue**: it needs GitHub Actions or a required check on `merge_group` events.
  Yulius CI is the only CI and runs on Yulius's machines, and GitHub's queue does not apply the
  repository's merge drivers (the union-merged changes log).
- **Keep the old run going beside the new head's run**: the old head's verdict cannot merge (GitHub
  needs the exact head up to date), so it only spends a lane.
- **Hold everything while a candidate is under test**, a strict first-come queue: it saves the rare
  candidate run that a newly current pull request makes stale. But it makes a green, mergeable pull
  request wait a whole run, which the request rules out.
- **Drop a whole red train's armed merges, or send them one by one** (ADR-0067's fallback): one by
  one costs a run each and finds the breaker last; bisection costs about log₂ n runs and tells the
  breaker exactly.

## Consequences

- A merge costs at most one run beyond the pre-check (its candidate), shared by every pull request
  waiting at that time. With three pull requests turning green one after another, the base moving
  after each merge: 3 pre-checks and at most 2 candidates, nothing cancelled.
- A pull request's run is only superseded by its own new commits.
- A pull request that passes alone but breaks on the current base is told so after its candidate,
  not by a rerun of its own.
- A candidate can still be made stale by a pull request that became green and current during its
  run. It is cancelled and formed again.
- Agents no longer need to merge the integration branch only because it moved, nor hold or re-arm
  merges. The briefing and the bundled skills say so.
