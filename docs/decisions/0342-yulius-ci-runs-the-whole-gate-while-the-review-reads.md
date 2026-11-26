# 0342. Yulius CI runs the whole gate while the review reads

## Status

Accepted (2026-11-15). Specified in
[SPEC-493](../../.ai/specs/SPEC-493-faster-from-start-to-merge-opus-writes-f.md) (CIR-001 to
CIR-004, fix 3); task #472. Amends step 4 of [the engineering workflow](../engineering/workflow.md)
("`scripts/dev ci` before handoff"), which no decision record had set.

## Context

A change waited for the whole gate three times: the author ran `scripts/dev ci` (about 3 to 4
minutes here) before the review, the reviewer ran it again, and Yulius CI ran it on the pushed
head, which was pushed only once every step was done or `delivery_merge` was called after the
approval. Its run then started at the next 60-second poll. Between review rounds the author ran
it again. Each copy costs minutes and, beside other cargo commands, breaks the gate's doctests
on a shared target directory. Nothing merges without Yulius CI's passed run on the exact head
anyway (CIR-003), so the local copies prove nothing a merge relies on.

## Decision

- **A review's start delivers the head under review.** When the agent starts `yulius-reviewer`,
  Yulius pushes the committed head (opening the pull request when there is none), with any
  step still open (`DeliveryAsk::Review`), and queues its Yulius CI run at once
  (`ci_sync_sandbox`), not at the next poll. A spec that is not valid, a missing plan or
  uncommitted changes leave the head where it is, and the agent is told why. The reviewer
  starts either way. A newer head's run supersedes the older one.
- **One rule for local checks, everywhere checks are described:** "While iterating, run the
  checks your change touches. The whole gate is Yulius CI's job: it tests the head while the
  reviewer reads. Do not run the whole gate locally before a review or between rounds, except to
  reproduce a CI failure or when CI cannot run (a playground without CI, an outage)." The
  workflows and the briefing, which have size budgets, say its short form ("the whole gate is
  Yulius CI's"). A test holds every place to it and refuses the old wordings. Since 2026-11-26
  (audit L10 F2) the full sentence is in `delivery-pipeline` only; the test refuses a copy.
- **The reviewer reads the head's Yulius CI run** and runs the tests it doubts; it does not run
  the whole gate either, except where CI cannot run.

## Alternatives

- **Keep the local gate and only start CI earlier.** Saves the poll, not the minutes the two
  local copies cost, nor their interference with each other.
- **Push at every commit.** The pull request would carry half-done slices, and every push would
  start a run that the next one cancels.
- **Let the reviewer start CI.** The reviewer is read-only by contract.

## Consequences

- A review round and the head's CI run go on at the same time; `delivery_merge` after the
  approval finds the run passed (it merges) or running (it arms), as before.
- Pull requests open earlier, before the review is answered, so people see work in review.
- A head that fails CI is learned while the reviewer still reads; the author answers both
  together.
- A playground without CI, and an outage, still need the local gate.
