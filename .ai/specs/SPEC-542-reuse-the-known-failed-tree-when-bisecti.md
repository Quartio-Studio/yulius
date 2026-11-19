---
yulius-task: 216deccb-393d-4b5e-b15a-47e613e2d474
status: in-progress
---

# SPEC-542. Reuse the failed tree during merge candidate bisection

## Goal

Keep the failed candidate run as the verdict when bisection forms its tested tree again.

## Context

Issue #1384 reports that four members fail together, the first half lands, and the remaining
two get another run on the same Git tree. In `form_train`, the recent-run lookup accepts
only a pass. The three-member regression exercises a different path in `bisect`, which can
identify a single omitted member after the other two pass (SPEC-497 MQW-003).

## Requirements

- **FCT-001** A newly built candidate takes the original failed candidate run for its exact
  tree in the sandbox's CI history. This failure takes precedence over a passing result.
  The merge queue uses its existing failed-candidate handling, including its bounded wait
  for members' own runs, bisection, and notification.
- **FCT-002** Errored and cancelled runs supply no verdict. A failure on another tree or on
  a non-candidate reference cannot reject the candidate.

## Plan

- [x] Specify the failed candidate verdict
- [x] Reproduce the four-member bisection and cover inconclusive runs
- [x] Reuse the failed candidate run and verify the queue outcomes
- [ ] Independently review and deliver the fix on green CI

## Acceptance criteria

- **AC-1 (FCT-001)** With four members whose last two break together, the original red tree
  executes once. The first two land in order and the remaining pair is bisected on the original run ID.
- **AC-2 (FCT-002)** Earlier cancellations and infrastructure errors for the same tree
  leave a new candidate eligible to run. An unrelated failed pull request cannot reject it.

## Test plan

Use the merge-train integration fixture with real Git and local CI. Compare candidate
trees and run IDs, inspect the bisection audit and the recorded merges, and exercise
inconclusive candidate runs at the rebuilt tree. Run the existing merge-train tests.

## Decisions

Look for a failed candidate before consulting the existing passed-tree lookup, and attach
the original run so its job logs and failure remain available. Use the scheduler's existing
bounded history and sandbox scope. See ADR-0383.

## Evidence

- On the test-only commit, the four-member regression failed with
  `train/3+4 got a second run on the failed tree`. Git reported tree
  `e003ac10660d2355d232903af31b46f39c845084` for both runs. The same-tree
  error/cancellation control and the existing three-member bisection test passed.
- After the fix, nextest ran both new integration tests. Both passed. The four-member
  case observed the original failed run ID at the second bisection.
- `scripts/dev quick` passed the repository contracts and format check.
- On 9729061c, all seven targeted nextest cases passed, including passed-tree reuse,
  path-skipped jobs, bounded waits and cancelled-candidate handling. The independent
  reviewer ran all 47 merge-train integration tests successfully.
- Round one requested a test where a newer eligible pass competes with the original
  candidate failure. The strengthened four-member regression on a26f1cad passed.
- `scripts/dev security` reported that Vidoc is disabled for this project. No scan ran.
- In an isolated copy with the lookup order reversed, the strengthened regression failed
  on `train/3+4` for the same tree ID. The newer pass is an eligible source in that control.
- `scripts/dev mutants --base 9729061c` found no product mutant in the test-only review fix.
