# 0049. Management tools cover the task and automation lifecycle, never the caller's own task

## Status

Accepted. Specified in [SPEC-066](../../.ai/specs/SPEC-066-fix-275-293-cockpit-and-agents-manage-ta.md).
Extends [0029](0029-cockpit-manages-through-mcp.md) and [0034](0034-ready-made-automations-catalog.md).

## Context

Since 0029 the Cockpit and task agents create and list tasks and create, list and change
automations through the Yulius MCP server, acting for a person. Stopping, archiving or deleting
a task, deleting an automation, running one now and enabling a ready-made automation were still
UI-only (#275, #293), so "manage our whole system" from the Cockpit stopped halfway.

## Decision

- `task_stop`, `task_archive` and `task_delete` call the UI's own functions (`stop_task`,
  `archive_task`, `delete_task_as`) with an `AuthContext` of the person the caller acts for,
  after `manage_authority`. `automation_delete` and `automation_run_now` call the UI's functions
  the same way. Only the audit's actor and detail are parameters: each call is audited with the
  caller (`task:<n>` / `cockpit:<login>`).
- A task's agent cannot stop, archive or delete its own task through them: stopping or deleting
  itself would kill the session in the middle of its own call, and Yulius closes and archives a
  finished task by itself.
- `automations_list` also returns `ready_made`, the catalog without its prompts, and
  `automation_template_enable` enables one by id with the Automations page's semantics
  (`enable_template_as`, under the same lock), `created_via = agent`.
- The Cockpit is served every new tool and pre-allows none of them: Claude Code asks the person
  before each.

## Alternatives

- **Copy the rules into the tools**: they would drift from the UI (Done tasks closed first,
  successors blocked on deletion, the previous-active rule).
- **A `template` argument of `automation_create`**: it would make the required name, cron and
  prompt optional in its schema; a separate tool keeps both schemas exact.
- **Let a task stop or delete itself**: the call would end the session answering it; the
  existing finish path already covers a task that is done.
- **Workflow create and edit in the same change**: a separate surface (instance and project
  workflows, read-only defaults); left for later.

## Consequences

A person can manage a project's tasks and automations end to end from the Cockpit, approving each
changing call. Task agents (which run with the approval bypass) can stop or delete other tasks of
their creator's project: the same power the creator has in the UI, audited with the agent. Stop
and archive are audited for tool calls only; the UI does not audit them. `automation_run_now` is
not counted in the hourly bound of tool-created tasks: like Run now in the UI it is bounded by one
active task per automation.
