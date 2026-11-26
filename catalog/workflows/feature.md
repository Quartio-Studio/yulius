---
name: Feature
description: New or changed behavior, from spec to a merge on green, through the autonomous delivery pipeline.
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

Follow `delivery-pipeline`; nobody accepts the result for you; the whole gate is Yulius CI's.

- One feature, one concern: split independent capabilities into chained tasks.
- Write the spec with `om-spec-writing` (`om-brainstorm` first when the request is vague, and
  `om-ux-shape` for user-facing flows). Its Plan replaces these steps: keep the first step and the
  last two, and put between them all the work you know of as slices of similar effort (3-12 steps
  per pull request, at most 60; add one later only for work found later), each named by what it
  delivers and leaving the project working.
- Build each slice with `test-driven-development`: a test that fails without the change, then the
  change. One step in progress at a time, done as soon as it is finished.
- Review: the reviewer from the task briefing runs `om-code-review` on the diff and the spec;
  answer each finding as that skill says.
- Finish with `om-check-and-commit` and `verification-before-completion`, then deliver: merge on
  green, clean up (`git-hygiene`), deploy dev only if asked (`cd_deploy`); production is a person's.
