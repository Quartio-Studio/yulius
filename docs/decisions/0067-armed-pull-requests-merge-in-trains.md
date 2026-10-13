# 0067. Armed pull requests merge in trains: one run on their combined result decides them all

## Status

Accepted. Extends SPEC-011 (armed merges), SPEC-050 DLV-203/205 and
[0048](0048-armed-merge-state-under-one-lock.md); specified in
[SPEC-084](../../.ai/specs/SPEC-084-merge-trains.md).

## Context

Under a strict "up to date" ruleset, one pull request merges per CI run:

- each merge puts every other armed pull request behind;
- Yulius merges the integration branch into it (DLV-203);
- its CI runs again.

On 2026-10-12/13, 42% of the armed merges waited for at least one such update after their
agent's last arm. They waited 7.4 hours in total, and runs at updated heads took 44% of the CI
time kept. With more agents at once, the wait grows with the number of pull requests in line.

GitHub itself only checks a status on the pull request's head and that the head contains its
base. Yulius CI is the only CI of the projects that have it, and the integration branch's
post-merge run already reuses a result by tree.

## Decision

1. **Train.** When two or more armed pull requests wait and none can merge at once, Yulius builds
   their combined result in its clone, in arm order:
   - It merges the integration branch so far into each head, as DLV-203 will (the repository's
     merge drivers apply), and lands it as the profile's squash or merge commit lands it.
   - A head that conflicts is left out and waits until the train is over.
   - Yulius CI runs once on the last commit. That commit is never pushed and gets no commit
     status.
2. **Hold.** While that run runs, the members after the first are not brought up to date: that
   would only start runs the train makes useless.
3. **One verdict for all.** When the run passed, each member in turn:
   - is brought up to date as DLV-203 does;
   - must have exactly the tree the train predicted;
   - gets the train's verdict recorded at that head, then published as `yulius/ci`;
   - merges through the normal merge path once GitHub shows that head.
   Before each step the integration branch must have the tree the steps before it leave. It ends
   with the tested tree, so its own run reuses that result.
4. **Verdict scope.** A verdict given to a member before the last covers a tree no run tested by
   itself. Such a verdict is marked (`requested_by` `yulius:merge-train`) and never reused for
   another commit. A train stopped halfway leaves the integration branch at a state its own CI
   tests for real.
5. **Fallback.** A red, errored or cancelled run dissolves the train. Its members then merge one
   by one until their agents arm again. Anything unexpected also dissolves it, and every member
   goes back to the one-by-one flow, which decides it with today's messages. The train only adds
   verdicts and a hold.
6. **Not for every project.** Projects without Yulius CI, with rebase merges, or that promote
   automatically after each merge have no trains: the last would ship the untested states in
   between.

## Alternatives

- **Test every prefix** (GitHub's merge queue tests each pull request with the ones before it).
  Every merged state would have its own run, but a train of n costs n runs. Two CI lanes on a
  machine shared with the agents run them about n/2 runs deep. One run for all, which the person
  asked for, ends the wait for the whole group after one run. The states in between last seconds,
  on the way to the tested one.
- **GitHub's merge queue.** GitHub's own runners would test the merge group. Yulius CI runs only
  on Yulius's machine, so the queue would require Actions or an extra integration, and it skips
  the repository's merge drivers.
- **Bisect a red train.** It saves a run when one member breaks. The one-by-one fallback finds
  that member anyway with each member's own run, and costs no new state.
- **Reuse a verdict by tree for any pull request head.** A train's intermediate trees are not
  tested trees, so generic reuse could not cover them honestly. Recording the verdict at the exact
  head, with its scope marked, keeps tree reuse to tested trees.

## Consequences

- A group of armed pull requests waits about one run instead of one run per pull request ahead
  of it.
- Two pull requests that break each other make a train red, at the cost of one run before the
  one-by-one flow.
- A pull request that can merge at once still goes alone; a train forms after it.
- The CI page shows train runs as "Merge train #n + #m". `ci_status` lists them for their
  members. Their jobs see `YULIUS_CI_REF=train/n+m`.
- `<data>/merge-trains.json` holds at most one train per sandbox. A lost file at worst cancels a
  train's run and puts its members back in the one-by-one flow.
