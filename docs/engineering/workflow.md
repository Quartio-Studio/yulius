# Agentic engineering workflow

The goal is a verified user outcome, with enough durable context for the next
engineer or agent to continue. Use [AGENTS.md](../../AGENTS.md) as the entry point.
[Git delivery](git.md) defines dev/main branches, delivery through `delivery_merge` and post-merge
gates.

0. **Primitives:** skills, personas, instructions and hooks follow
   [the primitive standard](primitives.md); a primitive change is reviewed like code.
1. **Orient:** inspect Git and branch/worktree assignments; each local branch has
   its own worktree. Create the task branch and worktree together from dev, then
   inspect relevant code/specs and observed behavior. State the
   authorized outcome and unknowns. Load one matching skill, not the whole catalog.
2. **Specify:** features and fixes get a spec; research, questions, docs and chores
   do not (SPEC-001 DSC-013). Attach stable requirement IDs, input/output and lifecycle
   contracts, acceptance scenarios and affected boundaries. Record assumptions and
   unresolved product choices; start from the [spec template](../../.ai/templates/spec.md).
   Plan all the known work as steps of similar effort, each a slice that leaves the code
   working, and finish them one at a time (STP-006/007, STP-009).
3. **Implement:** build a complete slice, preserving existing behavior. Keep an
   affected boundary's types, implementation and tests together. An ADR is only for a
   choice someone may want to reverse ([spec workflow](specifications.md)).
4. **Verify:** demonstrate the reported behavior and failure cases. While iterating,
   run the checks your change touches. The whole gate is Yulius CI's job: it tests the
   head while the reviewer reads. Do not run the whole gate locally before a review or
   between rounds, except to reproduce a CI failure or when CI cannot run (a playground
   without CI, an outage). Documentation-only changes need the repository check, not
   redundant compilation unless tooling/contracts changed (ADR-0342).
5. **Review:** examine the diff against each requirement, security boundary and
   existing default behavior. Classify findings with concrete failure scenarios.
   Green tests do not establish untested product acceptance. Get an independent review in a
   fresh session too: a reviewer with no shared context (a subagent given only the diff
   range and the spec), read-only. Follow the task briefing for the reviewer and model of the active harness. Then fix each finding in the change's own
   code test-first, never deferring it ([Review output](#review-output)).
   A reviewer does not ask for changes because of CI alone: `delivery_merge` merges only a
   head whose own Yulius CI run passed. When the review finds nothing to fix and the
   head's run is still going, or failed in a way the change does not cause, the verdict is
   approve and the report says what CI still has to do. A failure the change causes (its
   own test, format or lint, or a contract its files break) is a blocker.
6. **Deliver and hand off:** for authorized delivery, follow the [Git sequence](git.md) to a
   verified merge into dev (`delivery_merge` with the reviewed head); promotion to main is a
   person's. Report changed behavior, exact evidence and unresolved limits. Update the spec
   state only when its scope is proven. Use the handoff template for
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

After an approval, fix the remaining nits within 20 product lines and merge without
another round: `delivery_merge` with the approved head counts them (tests and documents do
not count). More than that, or a file leaving the change, needs a new round. A usage limit can cut a reviewer off before its report: resume that same reviewer by its agent id, never start a new one for the same round. A reviewer runs at high effort, never max.

Findings in the change's own code are fixed, never deferred
([ADR-0141](../decisions/0141-review-findings-in-the-change-s-own-code-are-fixed-before-merge.md),
[ADR-0427](../decisions/0427-task-authorization-covers-independent-re-review.md)):

1. An own-code finding is fixed in the same pull request, test-first, and never becomes a
   follow-up issue, whatever its severity: a test that cannot fail, a weak or missing
   assertion, a race, a missing regression test. Re-review until approval under the task's
   existing authorization. If findings persist, the author keeps fixing or splits the work
   within scope. Use `needs_you` only for a concrete action that requires a person.
2. Only these may become a new issue: a pre-existing bug found on the way, a flaky test
   the change did not cause, and cosmetic nits (naming, wording) left after 3 rounds.
   Each says why it is outside the change, with the evidence (for example "present on
   dev before this change").
3. The reviewer classifies each finding as own code or pre-existing, and the pull request
   answers every finding. Deferring an own-code finding makes the verdict request changes.

Before asking for another round, answer each finding of the last report with the test that fails
without its fix (or why no test can) and check every finding of that report against the new head.

Do not invent issues for balance. For changes to existing mechanisms, identify the
old guarantee and test it under the new default configuration.
