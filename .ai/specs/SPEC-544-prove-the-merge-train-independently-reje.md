---
yulius-task: 6345a8d8-4a73-477e-9625-871179420507
status: done
---

# SPEC-544 Prove the merge train rejects an ineligible member

## Goal

Give issue #1154 a focused regression test for a passed merge candidate whose member's owner can no longer act.

## Context

PR #1150 fixed the four timing failures reported on November 11. The remaining gap concerns the one-by-one fallback in SPEC-441 MQ-003 and MQ-005. In the same merge pass, `train_member` checks the owner, then `merge_armed_task` can independently drop the armed merge. The latter produces the message the existing test asserts, even if the train's own decision regresses.

## Requirements

- **TRG-001** A passed train encountering a disabled owner records why it dissolves and sets that member's armed merge aside for one-by-one handling.
- **TRG-002** The pull request stays open, the agent learns why its armed merge ended, and another pass creates no replacement candidate.

## Plan

- [x] Trace the overlapping merge decisions
- [x] Exercise an ineligible member at its turn
- [x] Verify that the train decision controls the outcome
- [x] Secure independent review of the regression

## Acceptance criteria

- The integration test observes the train's own rejection and one-by-one disposition for this arm (TRG-001).
- The test also verifies the final delivery outcome and candidate count (TRG-002).
- Bypassing the train's owner check or ending the train without holding the member back makes the strengthened test fail.

## Test plan

Use the existing single-member Git fixture and decide a passed candidate in a controlled merge pass. Check its dissolution audit and saved fallback state, then the pull request, agent outcome and number of train runs. Exercise both counterexamples locally, restore the production source, and run the issue's test filter on the restored tree.

## Decisions

Keep the disabled-owner scenario so the test exercises the same two decision paths that masked the regression. The saved fallback entry identifies the task and the specific arm. The audit gives the train's reason for dissolving.

## Evidence

- Replaced the disabled-owner branch's one-by-one result locally with a plain stop carrying the same reason. Nextest ran the original and strengthened tests together. The original passed. The strengthened test failed because the saved queue state lacked the held-back entry for `t1` at its arm timestamp.
- Removed only the train's disabled-owner check locally. The original test again passed. The strengthened test failed on the train's recorded reason, which now reported a refused merge. Both probes used isolated fixture repositories. The production source was restored before the positive run.
- On the restored tree, `cargo nextest run --locked -p yulius --test main --test-threads 3 --no-fail-fast -E 'binary_id(yulius::main) & (test(=merge_train::a_member_that_cannot_merge_is_told_instead_of_tested_again) | test(/^first_message::/))'` passed all 23 selected tests in the task sandbox. `scripts/dev quick` passed.
- After merging current `dev`, the focused nextest passed again at `ad61ffb64`. Independent review approved that head with no findings.
- `scripts/dev mutants` reported “No mutant in the Rust lines changed against origin/dev.” The two targeted probes above exercised the existing production branches. `scripts/dev security` reported that Vidoc was disabled and scanned nothing.
