# 0357. A merge candidate is decided by its own run

## Status

Accepted. Specified in [SPEC-497](../../.ai/specs/SPEC-497-an-armed-pull-request-merges-in-about-on.md)
slice C (task #479), MQW-001 to MQW-004. Amends SPEC-493 MQC-001 and MQC-002 (a member merges
only once its own run passed too; a member whose own run fails, errors or is cancelled leaves).
[ADR-0285](0285-armed-pull-requests-merge-through-a-queue-that-never-restarts-a-run.md) stays in
force: a moved base restarts no pull request's own run, and Yulius merges the base into no pull
request before its own run passed, with one exception. The member of a passed candidate is brought
up to date to merge it, with the candidate's verdict recorded at the pushed head first, while its
own run may still go.

## Context

On 16 Nov 2026 quartio-v2 formed 31 merge candidates; 10 merged and 21 were dissolved. Since
12:00 UTC, 10 candidate runs were cancelled ("the merge train was dissolved: #N's own …",
"`dev` moved"). Under SPEC-493:

- a passed candidate waited for each member's own run before merging it, so the candidate's
  verdict on the exact merged result sat unused for up to the rest of an own run;
- a member whose own run errored (the infrastructure, a time limit) or was cancelled left the
  candidate, and the candidate's run was cancelled, though neither says anything about the
  change;
- two candidates failed on how their members combine: a formatting check after the merge, and a
  test that lists every default file with its version when two pull requests changed the same
  one. A red candidate is bisected: its first half is tested again, merges, and the last member
  then fails a candidate of its own on top of it before its agent hears why;
- why a candidate was dissolved was counted by five kinds, and only as counts.

The rules that stay: nothing merges without a passed Yulius CI run on the exact head or the exact
merged result, and a failing test is never run again until it passes.

## Decision

- When open PRs share a commit, prefer the ordinary run for the member’s PR number. If it has none, the queue and `yulius wait` use the scheduler’s shared verdict for that commit. A fresh admission after integration CI was repaired supersedes an earlier configuration refusal. [SPEC-558](../../.ai/specs/SPEC-558-parked-codex-tasks-no-codex-exit-screen.md)

- **A passed candidate merges at once** (MQW-002). Its run tested the integration branch with
  every member merged: that is the exact merged result, which the merge rule asks for. A member's
  own run that still goes is not waited for. When Yulius brings the member up to date for its
  merge, that run is superseded like any run of an old head.
- **Only a failed own run makes a member leave** (MQW-001). A failed own run is the change's
  verdict, and a failing test is never run again until it passes, so the candidate cannot decide
  that member. The run read is the one at the member's armed head now: one its agent's clean merge
  of the integration branch moved with the same arming, else the head it joined with. It is read
  at each pass's start and again before each member's merge. An errored or cancelled own run
  says nothing about the change, and the candidate's run, which goes on, decides it. A run whose tree holds a member that failed decides
  no one, since that tree can never land, so that run is still cancelled. A pull request left out
  of the candidate for a conflict is not in its tree and never touches its run.
- **Results are reused by tree** (MQW-001). A candidate formed again with the same members on
  the same base (after it waited too long for a busy member, for example) has the same tree as
  before. It takes that tree's passed run, as an up-to-date pull request's squash merge already
  does on the integration branch, and makes no new run. A source that skipped a job for its
  `paths` is not reused: the same tree on another base may change what `paths` decides, and a
  run does not record its base. A candidate's own run skips such jobs too, so a project with
  `paths` jobs gets this reuse only from runs that ran every job. A train's verdict recorded at a
  pull request head (`yulius:merge-train`) is never a source.
- **Derived files a project names are changed one candidate at a time** (MQW-003, serialised).
  `[merge_queue] apart = [globs]` in the integration branch's `yulius.toml` names them (a
  generated list, a table of shipped files, `Cargo.lock`). When the queue forms a candidate,
  an armed pull request that changes a file under one of those globs, which an earlier member
  in arm order also changes, is held out (`merge_queue::held_apart`). It waits for the next
  candidate, built on top of the first's merge: no other member loses a run to their
  combination, and a textual conflict is told without a run. The held one's own candidate still
  tests the combination and can fail on it (a file to regenerate that merges cleanly). The candidate's audit entry names who was held apart and
  why. A pull request whose changed files cannot be told is not held. Serialising, not redoing
  the deterministic step: redoing it means running the project's formatter or generator where
  Yulius builds candidates (outside the sandbox) and landing a commit the members' trees do not
  predict; holding one back costs that pull request one candidate's wait, and only when both
  touch the same derived files. Its wait note stays the generic "next candidate"; the reason is
  in the formed candidate's audit entry. Holding happens after the 8 oldest are taken, so a
  candidate with held ones has fewer members.
- **How members combine is told without a candidate of its own** (MQW-003, for what no `apart`
  names). A red candidate whose
  bisection leaves out exactly one member (a candidate of two or three) remembers that member and
  the others' heads. If exactly those heads then pass without it on the same base, it failed with
  them and they pass without it: often a combination in a derived file (formatting, a generated
  list), or a test that fails only there. Its armed merge ends at once, saying what was seen
  (both runs and its own run's state), and it never fails a candidate of its own on top of them.
  One candidate run is saved, and its agent hears why one cycle earlier. A suspect armed again
  since is kept. Everything else bisects as before.
- **Why each candidate was dissolved is counted and shown** (MQW-004).
  - A member whose own run failed has its own kind (`member_failed`), apart from one that was
    armed again, is no longer armed or moved on (`member_left`).
  - `GET /api/analytics/merge-queue` lists the last ten dissolved candidates in the range, newest
    first, each with its sandbox, kind and whole reason (`dissolved_recent`).
  - The Pipeline tab's Merge queue card shows that list under the counts.
  - A combination told at once is audited as `delivery.train.combined`.

## Alternatives

- **Keep waiting for the members' own runs**: these runs add nothing the candidate's run did not
  test, and they kept merges waiting up to a whole own run.
- **Merge the members before a failed one on the candidate's verdict**: their merged tree was
  never tested. The integration branch would end at a state no run passed.
- **A run per prefix of the candidate (base+A, base+A+B, …)**: each member's merged result
  would be tested exactly, and a failed member would cost only the runs after it, but every
  candidate would cost as many runs as members. CI's budget (ADR-0350, ADR-0353) is what the
  queue waits on.
- **Hold every pull request touching a common file apart**, without the project naming derived
  files: a changes log or an index every pull request touches would leave candidates of one.
- **Regenerate derived files in the candidate** (run the project's formatter or generator on the
  merged result and land that commit): it would need the project's commands run where Yulius
  builds candidates. Yulius would have to run them outside the sandbox, or merge a job's commit
  into the members' updates, which their own trees would not predict. A project whose derived
  files conflict textually can merge them by union, as the changes log does.
- **Blame the last member as soon as the candidate fails**: the first ones may fail alone on the
  new base, and the last one would be dropped for nothing. It is blamed only once the others
  passed without it.

## Consequences

- A candidate that passes merges its members on the next pass. Their own runs at the old heads
  end superseded, and their minutes count as cancelled.
- A member whose own run errored is merged on the candidate's run. Its own error stays unread
  unless the agent reads it.
- A red candidate of two or three costs one run less when its last member breaks the
  combination. A larger one bisects as before.
- The `member_failed` kind starts with this change. Earlier entries keep `member_left`.
- When the train cannot record its verdict at a member's pushed head (the tree differs from the
  prediction, or the store fails), it pushes the head without one, as before. A member whose own
  run still goes then gets a new run at that head: the restart ADR-0285 avoids, in a case that
  needs a broken prediction.
- An own run that fails between a pass's start and that member's merge is caught by the second
  read before the merge (a pause point there, `TrainMemberMerging`, lets a test fail it exactly
  then).
