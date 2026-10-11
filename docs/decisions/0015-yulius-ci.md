# 0015. Yulius is the projects' only CI

## Status

Accepted. Recorded retroactively from SPEC-002 (ADR-21); the full analysis is in
[SPEC-002 section 2](../../.ai/specs/SPEC-002-yulius-architecture.md).

## Context

All tests and CI/CD must be local with zero paid GitHub features and no GitHub Actions.

## Decision

Yulius runs each sandbox's `[[ci.jobs]]` (or `[commands] gate`) for every open pull request head and every integration or production head, in parallel, inside the sandbox containment without secrets, and reports the `yulius/ci` commit status. The merge gate uses that run.

## Alternatives

GitHub Actions on hosted or Blacksmith runners; a self-hosted runner; a separate CI server.

## Consequences

One CI on infrastructure we control. Needs the App's Commit statuses and Workflows write permissions.
