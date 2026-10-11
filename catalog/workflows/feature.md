---
name: Feature
description: New or changed behavior, from spec to verified production, through the autonomous delivery pipeline.
when_to_use: The task adds or changes behavior that users or other code rely on - a new capability, a changed flow or screen, a new API or integration.
spec: required
tests: required
order: 10
---

## Steps

1. Read the product context and related work, then write and validate the spec
2. Build the first planned slice test-first
3. Independent review in a fresh session, findings fixed test-first
4. Every gate green, docs and the changes log updated

## Instructions

Follow `delivery-pipeline` end to end; nobody accepts the result for you.

- Before designing, read the docs index, search earlier specs, `tasks_list` and `sessions_search`
  for the area, and extend what exists. One feature, one concern: split independent capabilities
  into chained tasks.
- Write the spec with `om-spec-writing` (`om-brainstorm` first when the request is vague, and
  `om-ux-shape` for user-facing flows). Its Plan replaces these steps: keep the first step and the
  last two, and put 3-10 slices of similar effort between them, each named by what it delivers and
  leaving the project working.
- Build each slice with `test-driven-development`: a test that fails without the change, then the
  change. One step in progress at a time, marked done as soon as it is finished.
- Review: a fresh reviewer with no shared context runs `om-code-review` on the diff and the spec.
  Fix every finding test-first; at most 3 rounds.
- Finish with `om-check-and-commit` and `verification-before-completion`, then deliver: merge on
  green, deploy and verify, promote, verify production, clean up (`git-hygiene`).
