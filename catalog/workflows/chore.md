---
name: Chore
description: A small change without new behavior - a dependency bump, configuration, a rename, cleanup or tooling - kept green by the gates. No spec.
when_to_use: A small maintenance change with no behavior change - dependency or toolchain bump, configuration, rename, dead-code removal, formatting, a build or tooling tweak.
spec: optional
tests: required
order: 50
---

## Steps

1. Make the change with the smallest diff
2. Every gate green and the changes log updated

## Instructions

A chore is small and boring on purpose.

- One concern, the smallest diff. If it turns out to change behavior, stop and continue it as a
  Feature or Fix with a spec.
- Dependency and toolchain bumps: read the changelog for breaking changes, keep lockfiles
  consistent, and check licenses with the project's audit.
- Cleanup and dead-code removal: follow `dead-code-cleanup` (the language's tools, every
  candidate verified, one kind of removal per commit).
- Run every gate with `om-check-and-commit`; never weaken a test, a lint rule or an allowlist to
  get green.
- Commit and deliver following `git-hygiene` and `delivery-pipeline`: merge on green, deploy,
  verify, clean up.
