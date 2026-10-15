---
name: Fix
description: A defect fixed at its root cause, proven by a regression test that fails without the fix.
when_to_use: Something does not work as specified or as it used to - a bug, a regression, a crash, wrong data, a failing check or deployment.
spec: required
tests: required
order: 20
---

## Steps

1. Reproduce the defect and find its root cause
2. Write the fix spec and the regression test that fails
3. Fix the cause until the regression test and every gate pass
4. Independent review in a fresh session, findings fixed test-first

## Instructions

Follow `delivery-pipeline`; a fix is small, so keep it to one concern.

- Reproduce first and find the cause, not the symptom, with `om-root-cause` (read-only): the
  failing input, the code path, and why the existing tests missed it. Search `sessions_search` and
  earlier specs for how the area was built.
- Once the defect is confirmed, report it: `issue_create` with what you reproduced, unless an open
  issue already tracks it (`gh issue list --state open --search "<words>"`); then reference that
  issue and say `Fixes #<number>` in the pull request. A task told not to create issues never does;
  it comments on an existing issue or says it in its summary instead. A defect you cannot reproduce is reported back with the evidence,
  not filed.
- The spec can be short: symptom, root cause, the regression test, the fix, and what else the
  cause could break.
- Fix with `om-fix`: commit the regression test first and show it fails, then the smallest fix
  that makes it pass. No unrelated refactoring; record other pre-existing problems you notice with
  `issue_create`.
- A flaky test is a defect too: reproduce it under load and fix the cause (wait on conditions,
  inject the clock, isolate data); never add retries.
- Review in a fresh session (`om-code-review`). The reviewer classifies each finding as own code or
  pre-existing, and the pull request answers every one. A finding in code or tests this change added
  or modified is fixed here, test-first, and never becomes a follow-up issue; the round limit (3)
  does not excuse it: keep fixing, split the change or ask a person (`needs_you`), and deferring it
  makes the verdict request changes. Only a pre-existing bug, a flaky test the change did not cause,
  or a cosmetic nit left at the round limit becomes an issue, saying why it is outside the change
  (for example "present on the integration branch before this change").
- Then `om-check-and-commit` and deliver: merge on green, deploy and verify, promote, verify
  production, clean up (`git-hygiene`).
