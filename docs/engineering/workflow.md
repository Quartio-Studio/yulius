# Agentic engineering workflow

The goal is a verified user outcome, with enough durable context for the next
engineer or agent to continue. Use [AGENTS.md](../../AGENTS.md) as the entry point.
[Git delivery](git.md) defines dev/main branches, author self-merges and post-merge gates.

0. **Primitives:** skills, personas, instructions and hooks follow
   [the primitive standard](primitives.md); a primitive change is reviewed like code.
1. **Orient:** inspect Git and branch/worktree assignments; each local branch has
   its own worktree. Create the task branch and worktree together from dev, then
   inspect relevant code/specs and observed behavior. State the
   authorized outcome and unknowns. Load one matching skill, not the whole catalog.
2. **Specify:** features and fixes get a spec; research, questions, docs and chores
   do not (SPEC-001 DSC-013). Attach stable requirement IDs, input/output and lifecycle
   contracts, acceptance scenarios and affected boundaries. Record assumptions and
   unresolved product choices; use the [spec template](../../.ai/templates/spec.md).
   Plan steps of similar effort, each a slice that leaves the code working, and finish
   them one at a time (STP-006/007).
3. **Implement:** build a complete slice, preserving existing behavior. Keep an
   affected boundary's types, implementation and tests together. New architecture
   deserves a short ADR with alternatives and consequences.
4. **Verify:** demonstrate the reported behavior and failure cases. Run targeted
   checks during iteration, then `scripts/dev ci` before handoff of Rust/tooling
   changes. Documentation-only changes need the repository check, not redundant
   compilation unless tooling/contracts changed.
5. **Review:** examine the diff against each requirement, security boundary and
   existing default behavior. Classify findings with concrete failure scenarios.
   Green tests do not establish untested product acceptance. Get an independent
   review in a fresh session too: a reviewer with no shared context (a subagent given
   only the diff range and the spec), read-only; fix each finding in the change's own
   code test-first, never deferring it ([Review output](#review-output)).
6. **Deliver and hand off:** for authorized delivery, follow the Git sequence through
   verified dev and main merges, without waiting for reviewer approval. Report
   changed behavior, exact evidence and unresolved limits. Update
   the spec state only when its scope is proven. Use the handoff template for
   sustained work; keep private logs in ignored `.ai/runs/`.

A failing check is either a product defect, test defect, infrastructure failure or
known nondeterminism. Name it, reproduce it, and fix the cause. Do not rerun until
green without explanation, disable the test, broaden licenses or silently omit a
missing tool. A flaky test is reproduced and fixed (wait on conditions, not sleeps;
inject the clock; isolate data), or quarantined with an issue; never retried. If external infrastructure blocks one check, finish independent
work and state what remains unverified.

## Definition of ready

An authorized outcome, enough input/output detail to test, named affected boundaries
and no unresolved decision that would materially change implementation. A routine
bug may already meet this via its reproduction and existing contract. Reversible
implementation details do not need fresh permission.

## Definition of done

All acceptance items have direct evidence; tests exercise failure/recovery where
applicable; required gates pass; diff and generated artifacts are inspected; specs
and rules reflect the result. A UI flow also needs browser verification. An external
publish/merge/deploy requires its own user authorization and is not implied by this
workflow. Record unrun hosted checks separately from local equivalents.

## Review output

For each finding: severity, path/location, concrete trigger, impact, recommended
change, and whether it is **own code** (code or tests the change added or modified)
or **pre-existing** (on dev before the change). Report no findings if none were
discovered, plus validation scope and gaps.

Findings in the change's own code are fixed, never deferred
([ADR-0141](../decisions/0141-review-findings-in-the-change-s-own-code-are-fixed-before-merge.md)):

1. An own-code finding is fixed in the same pull request, test-first, and never becomes a
   follow-up issue, whatever its severity: a test that cannot fail, a weak or missing
   assertion, a race, a missing regression test. The round limit (3) does not excuse it:
   keep fixing, split the change, or stop and ask a person (`needs_you`). Never merge
   with a known defect in the change's own code.
2. Only these may become a new issue: a pre-existing bug found on the way, a flaky test
   the change did not cause, and cosmetic nits (naming, wording) left at the round limit.
   Each says why it is outside the change, with the evidence (for example "present on
   dev before this change").
3. The reviewer classifies each finding as own code or pre-existing, and the pull request
   answers every finding. Deferring an own-code finding makes the verdict request changes.
Do not invent issues for balance. For changes to existing mechanisms, identify the
old guarantee and test it under the new default configuration.
