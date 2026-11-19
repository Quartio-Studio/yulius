---
yulius-task: 1786ab2c-9cac-4831-983d-48f7f21bea33
status: in-progress
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
- [ ] Cover edits, re-enables and starts with two actors
- [ ] Bind automation actions to the authorized identity
- [ ] Explain execution ownership in the editor and tools
- [ ] Verify and review the fix for delivery into dev

## Acceptance criteria

- AC-1. A synthetic second writer gets a refusal for edits, re-enabling (including templates), and Run now through HTTP and MCP. The stored configuration and run history survive these refusals intact. The creator can schedule or start a task with their identity.
- AC-2. An upgraded database retains configurations and task histories and creates no task from an unapproved legacy schedule or pending run.
- AC-3. The editor names the creator before saving, reflects access on its controls, and tool descriptions explain the rule.

## Test plan

Use the existing SQLite and fake GitHub fixtures, HTTP router, in-process MCP tools and controllable scheduler clock. Check upgrade behavior with a pre-upgrade database. Verify the UI with the mock browser suite. Run the repository quick checks, security command and Yulius CI.

## Decisions

[ADR-0381](../../docs/decisions/0381-automation-creators-authorize-edits-and-starts.md) extends task ownership to its scheduled source. A project writer can replace an absent creator's automation with their own.
