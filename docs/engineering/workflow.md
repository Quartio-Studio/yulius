# Agentic engineering workflow

The goal is a verified user outcome, with enough durable context for the next
engineer or agent to continue. Use [AGENTS.md](../../AGENTS.md) as the entry point.
[Git delivery](git.md) defines dev/main branches, delivery through `delivery_merge` and post-merge
gates.

0. **Primitives:** skills, personas, instructions and hooks follow
   [the primitive standard](primitives.md); a primitive change is reviewed like code.
1. **Orient:** inspect Git state and the task's worktree, then the relevant code, specs and
   observed behavior. State the authorized outcome and unknowns. Load one matching skill, not
   the whole catalog.
2. **Specify:** a task that needs a spec writes it in the [spec workflow](specifications.md)'s
   form. Attach stable requirement IDs, input/output and lifecycle contracts, acceptance
   scenarios and affected boundaries. Record assumptions and unresolved product choices.
3. **Implement:** build a complete slice, preserving existing behavior. Keep an
   affected boundary's types, implementation and tests together. An ADR is only for a
   choice someone may want to reverse ([spec workflow](specifications.md)).
4. **Verify:** demonstrate the reported behavior and failure cases with the checks your change
   touches (`delivery-pipeline`, step 3). Documentation-only changes need the repository check,
   not redundant compilation unless tooling/contracts changed (ADR-0342).
5. **Review:** examine the diff against each requirement, security boundary and
   existing default behavior. Classify findings with concrete failure scenarios.
   Green tests do not establish untested product acceptance. The independent review in a fresh
   session and the answers to its findings follow the bundled skill `om-code-review`.
6. **Deliver and hand off:** follow the [Git sequence](git.md) to a verified merge into dev.
   Report changed behavior, exact evidence and unresolved limits. Update the spec state only
   when its scope is proven. Use the handoff template for sustained work; keep private logs in
   ignored `.ai/runs/`.

A failing check is either a product defect, test defect, infrastructure failure or
known nondeterminism. Name it, reproduce it, and fix the cause. Do not rerun until
green without explanation, disable the test, broaden licenses or silently omit a
missing tool (flaky tests: `test-driven-development`). If external infrastructure blocks one
check, finish independent work and state what remains unverified.

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

Findings, their severity, the own-code or pre-existing mark and what each verdict requires are
defined once, in the bundled skill `om-code-review`. Do not invent issues for balance. For
changes to existing mechanisms, identify the old guarantee and test it under the new default
configuration.
