---
yulius-task: 1786ab2c-9cac-4831-983d-48f7f21bea33
status: done
---

# SPEC-534. Enforce automation ownership across edits and starts

## Goal

An automation creates tasks using the GitHub identity of the person who authorized its prompt and start.

## Context

Issue #1289 and static inspection of dev 8d05c25b show that HTTP and MCP allow any project writer to save configuration or start a run. Task creation always uses the stored creator. Existing tests cover readers and same-person tools, and one template test explicitly allowed another writer to re-enable it.

## Requirements

- AOW-001. The creator with configure access alone saves, toggles or manually starts an existing automation. A tool acts for its authenticated person. Project writers can still delete it. An already-enabled template can be returned idempotently.
- AOW-002. On upgrade, pause existing schedules until their creator reviews and enables them. Resolve old pending runs with an existing task by linking it, and skip the others. New schedules and crash recovery keep their usual behavior.
- AOW-003. Show who owns the automation in its editor and explain whose identity the tools use.

## Plan

- [x] Specify the automation owner and legacy behavior
- [x] Cover edits, re-enables and starts with two actors
- [x] Bind automation actions to the authorized identity
- [x] Explain execution ownership in the editor and tools
- [x] Verify and review the fix for delivery into dev

## Acceptance criteria

- AC-1. A synthetic second writer gets a refusal for edits, re-enabling (including templates), and Run now through HTTP and MCP. The stored configuration and run history survive these refusals intact. The creator can schedule or start a task with their identity.
- AC-2. An upgraded database retains configurations and task histories and creates no task from an unapproved legacy schedule or pending run.
- AC-3. The editor names the creator before saving, reflects access on its controls, and tool descriptions explain the rule.

## Test plan

Use the existing SQLite and fake GitHub fixtures, HTTP router, in-process MCP tools and controllable scheduler clock. Check upgrade behavior with a pre-upgrade database. Verify the UI with the mock browser suite. Run the repository quick checks, security command and Yulius CI.

## Decisions

[ADR-0381](../../docs/decisions/0381-automation-creators-authorize-edits-and-starts.md) extends task ownership to its scheduled source. A project writer can replace an absent creator's automation with their own.

## Evidence

- Before the fix, both synthetic authorization tests failed. The HTTP edit returned 200 when the fixture expected 403, and MCP accepted the other writer's update. The browser regression failed because the editor omitted the execution identity.
- With the UI change, all 15 tests in `e2e/tests/automations.e2e.ts` passed on Chromium. The SQLite upgrade fixture paused the schedule, linked the pre-existing task and skipped the slot with no task.
- `cargo test -p yulius --test main automations::` passed all 15 cases, including both two-person regressions. The management-tool integration group passed all 21 cases. `cargo test -p yulius-store --lib legacy_automation_schedules` passed. The automation and store commands also passed after merging the updated dev base.
- Round 1 asked for direct assertions on the list switch, Run now, Delete and the editor's read-only prompt. Those assertions passed. The mock's repeated template enable changed the next run time in the new regression. Preserving the schedule made it pass. Round 2 approved commit `25d1917bbc942c81815bb22333f24d614ae24768`.
- `scripts/dev quick` passed. `scripts/dev security` reported Vidoc disabled. Full local Chromium suite at the first reviewed head recorded 1,132 passes, one skip and two failures in the host-policy notice and task-terminal image paste. The failures and the separate shared-workflow authorization observation are recorded on issue #1289.
- The final `npx e2e run tests/automations.e2e.ts` passed all 16 Chromium cases, including the additional ownership assertions and repeated template enable.
- After integrating the upstream editor fix for typed names, all 19 automation browser cases passed and the independent reviewer approved integrated commit `87f2eb5642408554cebd0daa9c67a5450f447930`.
- `scripts/dev mutants` reported 30 listed, 1 caught, 0 missed, 0 timed out, 28 unviable and 1 not reached within the 30-minute bound. The tests caught the inverted creator comparison. The pending mutation replaced the whole MCP tool catalog with an empty list. The review follow-up command reported no changed Rust lines in that round.
