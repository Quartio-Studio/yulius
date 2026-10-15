# 0141. Review findings in the change's own code are fixed before merge

## Status

Accepted. Specified in [SPEC-174](../../.ai/specs/SPEC-174-review-findings-in-the-change-s-own-code.md);
task #164. Tightens the review step of the delivery pipeline (SPEC-001 DLV-008).

## Context

An audit of this repository's issues (2026-10-15) found about 190 filed by agents, about 23 of
them review leftovers. Most were legitimate: bugs the reviewer found in older code the change did
not touch (#348, #502, #406), or nits left after the 3-round limit (#431, #331). Some were
weaknesses in the task's own new code, pushed to an issue to close the review loop: two checks in a
new e2e test that could not fail (#524), a finding "recorded rather than fixed to close the review
loop after five rounds" (#478), a leftover of round 4 (#513) and of code the task had just written
(#477, #436). The rules allowed it: the delivery pipeline let follow-ups become issues and ended the
review with "after 3 rounds, decide and record why", and `om-code-review` let a major through with
"a recorded decision with its reason and a follow-up issue".

## Decision

One rule, stated the same way wherever the review or follow-ups are described (the
`delivery-pipeline` and `om-code-review` skills, the Feature and Fix workflows, this repository's
`yulius-review`, `yulius-verify` and `yulius-git` primitives, [the engineering
workflow](../engineering/workflow.md#review-output), the briefing and the `issue_create` tool):

1. A finding in code or tests the change added or modified is fixed in the same pull request,
   test-first, and never becomes a follow-up issue, whatever its severity: a test that cannot
   fail, a weak or missing assertion, a race, a missing regression test. The 3-round limit does not
   excuse it: the task keeps fixing, splits the change, or stops and asks a person (`needs_you`).
   It never merges with a known defect in its own code. A review that cannot converge is
   something only a person can resolve (the scope, or how to split the change), so it is a valid
   `needs_you` case, not a decision the agent takes alone ([ADR-0111](0111-agents-ask-a-person-with-needs-you.md)):
   "decide, record why and carry on" never means merging past an own-code finding.
2. Only these may become a new issue: a bug in code the change did not introduce, a flaky test the
   change did not cause, and cosmetic nits (naming, wording) left at the round limit. Each says why
   it is outside the change, with the evidence ("present on dev before this change"). A
   pre-existing major filed that way does not block the verdict; any blocker, pre-existing or not
   (a failing gate included), still does.
3. The reviewer classifies each finding as own code or pre-existing, and the pull request answers
   every finding. Deferring an own-code finding makes the verdict request changes.

A unit test (`skills::tests::review_findings_in_own_code_are_never_deferred`) checks that every
location states the rule and none keeps the old deferral wording; the briefing's tests check its
text.

## Alternatives

- **Keep the recorded-decision escape for majors.** Rejected: it is exactly what produced #478 and
  #524; a recorded reason does not make a test that cannot fail any less wrong.
- **Raise the round limit.** Rejected: more rounds do not decide what may be deferred, and a
  change that needs many rounds is usually too big; splitting it is the better answer.
- **Enforce it in `delivery_merge`** (refuse a merge whose pull request links a new issue).
  Rejected for now: Yulius cannot tell an own-code issue from a legitimate pre-existing one
  without the reviewer's classification, which is prose. The reviewer's verdict already gates the
  merge.

## Consequences

- Reviews take as many rounds as the change's own defects need; a task that cannot converge
  splits or asks a person instead of merging.
- Issues filed by agents are only pre-existing bugs, unrelated flaky tests and leftover nits, each
  with the evidence that it is outside the change.
- Existing issues (#524, #478, #513, #477, #436) stay as they are.
