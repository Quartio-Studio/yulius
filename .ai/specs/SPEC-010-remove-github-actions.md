---
yulius-task: 03b375b6-03e3-42f4-965a-366b746a07ff
status: done
---

# SPEC-010: Remove GitHub Actions from this repository

Status: done
Date: 2026-10-12
Owner: Yulius task #17
Scope authorization: user request (issue #184, step 4; F 2026-10-10: "deleting GitHub Actions completely")

## Goal

Yulius is the only CI of this repository: no GitHub Actions workflow remains, and the rulesets
require only the `yulius/ci` commit status the Yulius GitHub App publishes.

## Context

- Outside the repository (2026-10-12) the App got Commit statuses, Workflows and Issues read and
  write, and the live rulesets `yulius-dev` and `yulius-main` already require only `yulius/ci`
  (integration_id 5225204).
- `[[ci.jobs]]` in `yulius.toml` already holds `quality` and `e2e`.
- The branch policy (`scripts/check_git_policy.py`) moves into Yulius's merge gate in #223; it is
  not implemented here.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| GHA-001 | `.github/workflows` (ci.yml, e2e.yml, policy.yml) and `.github/actionlint.yaml` no longer exist. | git diff |
| GHA-002 | `.github/rulesets/dev.json` and `main.json` require exactly `yulius/ci` with integration_id 5225204 and are otherwise unchanged. | git diff |
| GHA-003 | AGENTS.md and the engineering docs say Yulius CI is the only CI, macOS is checked with `scripts/dev ci` on a Mac before promotion, and the branch policy is checked by hand until #223. | docs diff |
| GHA-004 | Nothing outside history (changes log, specs' evidence) and the onboarding of other projects points at `.github/workflows`. | grep |

## Design

Deletion and documentation only; no product code changes. `scripts/check_git_policy.py` and its
tests stay as they are: no gate runs the script, and its `ci.yml` query only matters when run by
hand (to be replaced in #223).

## Failure and compatibility

Until #223 nothing enforces the branch policy automatically. Open pull requests need no action:
the live rulesets were already switched.

## Acceptance criteria

1. `scripts/dev ci` passes.
2. `grep -r "\.github/workflows"` finds only history, specs and other-project onboarding.

## Test plan

No behavior changes, so no new tests; the existing repository check and `scripts/dev ci` prove
nothing depended on the removed files.

## Plan

- [x] Workflows, actionlint config and ruleset checks removed
- [x] Docs, SPEC-002 and changes log updated; `scripts/dev ci` green

## Decisions

- Also delete `.github/actionlint.yaml`: it only configured linting of the deleted workflows.
- Leave `scripts/check_git_policy.py` untouched (instruction), noting its stale `ci.yml` query.
