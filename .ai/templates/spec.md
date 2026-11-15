---
yulius-task: <task id>
status: draft
---

# SPEC-NNN: title

<!-- The form `spec_check` validates (skill `om-spec-writing`): the frontmatter above, with
`status` one of draft, approved, in-progress, done or abandoned, and the seven sections below.
Omit nothing; a section that does not apply says why in one line. -->

## Goal

The user outcome in a few sentences: who gets what, and why now.

## Context

Current behavior and cited observations (code paths, transcripts, issues). Distinguish facts
from assumptions. In scope, explicit non-goals, and dependencies on unresolved decisions.

## Requirements

- **ABC-001** One observable behavior per requirement, with a stable ID. For each nonterminal
  state name its owner, its exit trigger and what happens when that trigger never arrives;
  boundaries, limits, cancellation, recovery and compatibility where they affect the change.

## Plan

- [ ] A slice that leaves the project working, named by what it delivers

## Acceptance criteria

- **AC-1** A concrete scenario tied to requirement IDs, with negative cases and expected outcomes.

## Test plan

The tests that prove each criterion (unit, integration, e2e), the fixtures, and manual evidence
where a person must look (UI).

## Decisions

Each choice with the alternatives and why; a lasting one is an ADR in `docs/decisions/`. Open
questions are decided with a reversible default and recorded here.

<!-- Before `status: done`: every Plan item ticked and an `## Evidence` section with the
commands run and their results. A skipped gate is not a passing gate; test names are not
executed results. -->
