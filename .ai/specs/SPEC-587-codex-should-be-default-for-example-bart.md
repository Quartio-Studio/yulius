---
yulius-task: 39af12db-48d1-46b9-880b-767572f2e552
status: in-progress
---

# SPEC-587 — Codex is the initial model choice for a person's new tasks

## Goal

Opening Settings or the new-task composer in a browser with no saved choice shows Codex. Starting a task uses the Codex launch defaults.

## Context

The two pages share `modelPreferences()`. On an empty store it returns a null model, the same value written when a person picks Random. The composer submits an empty set of model fields, triggering the server's random draw. Earlier browser tests expected this behavior under [SPEC-520](SPEC-520-i-dont-know-why-but-i-dont-see-random-ha.md). The CLI and Superagents already default to Codex under [SPEC-583](SPEC-583-cli-nad-superagents-should-spawn-tasks-i.md). Fixes #1592.

## Requirements

- **CDF-001** A missing, unreadable or unrecognized browser preference selects Codex default in Settings and the composer. Task creation sends the Codex harness and inherits the model and effort unless a person sets an override.
- **CDF-002** Honor saved models, compatible effort levels and an explicitly stored Random choice across visits.
- **CDF-003** If a default start needs a Codex subscription, name Codex. When the composer has no usable accounts, offer the connection for its selected agent.

## Plan

- [x] Trace the default in the composer and Settings
- [x] Capture the missing-preference regression in the browser
- [x] Make Codex the personal task default and preserve explicit choices
- [ ] Verify and independently review the delivery head

## Acceptance criteria

- With empty storage, a person sees Codex default in both pages. An untouched task is recorded as chosen Codex and inherits Astra at extra high effort.
- A removed model or invalid preference recovers to Codex. Selecting Random persists across reopening the page, and its task still draws from eligible harnesses. Selecting a listed model restores that model and its compatible effort.
- With no connected account the composer offers Connect Codex for the default. A refused default start directs an administrator to connect Codex.

## Test plan

Exercise Settings, the composer, the submitted request, task details and missing-account guidance in Chromium using synthetic subscriptions. Exercise Random draws and restoration of saved choices. Run repository quick checks and the browser suite, then submit the reviewed head to Yulius CI.

## Decisions

Use the existing Codex default model option. [ADR-0426](../../docs/decisions/0426-the-new-task-composer-defaults-to-codex.md) The server owns the inherited launch settings. Explicit Random is stored as JSON null, while an untouched browser has no model key. Automation defaults and calls to the task API have their own selection contracts.

## Evidence

- On the original preference reader, Chromium failed all three new regressions in `tests/model-choices.e2e.ts`. The observed values were Random agent in fresh Settings and after an invalid preference, and Connect Claude in the empty-account composer.
- With the fix, all seven model-choice tests passed in Chromium. They checked the actual submitted fields and the resulting chosen Codex task with inherited Astra and xhigh. All eleven Antigravity tests, sixteen Codex tests and seven Claude connection tests also passed. Explicit Random survived navigation and still drew across providers.
- A direct JavaScript check exercised missing, malformed, unrecognized and inaccessible browser storage, JSON null, and a saved model for each provider.
