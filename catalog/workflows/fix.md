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
  failing input, the code path, and why the existing tests missed it.
- Once the defect is confirmed, report it: `issue_create` with what you reproduced, unless an open
  issue already tracks it; then reference that issue and say `Fixes #<number>` in the pull request.
  A task told not to create issues never does; it comments on an existing issue or says it in its
  summary instead. A defect you cannot reproduce is reported back with the evidence, not filed.
- The spec can be short: symptom, root cause, the regression test, the fix, and what else the
  cause could break.
- Fix with `om-fix`: commit the regression test first and show it fails, then the smallest fix
  that makes it pass. No unrelated refactoring.
- Review in a fresh session (`om-code-review`); it marks findings own code or pre-existing; the PR
  answers each. Own code is fixed test-first whatever the round limit, never as a follow-up issue
  (that is request changes); only pre-existing bugs, flaky tests the change did not cause and nits
  left at the round limit become issues, saying why they are outside the change. Then
  `om-check-and-commit` and deliver: merge on green, clean up (`git-hygiene`), deploy dev only if
  asked (`cd_deploy`); production is a person's.
