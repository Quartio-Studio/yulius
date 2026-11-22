# 0460. A task reads its neighbours' status and never steers them

## Status

Accepted on 2026-11-22 for task #609. The owner approved a read view of the other tasks on
22 Nov 2026 and rejected messaging between tasks ("messaging makes too much chaos"). Specified in
[SPEC-625](../../.ai/specs/SPEC-625-task-agents-can-read-other-tasks-status.md).

## Context

Parallel tasks of one project duplicate and collide when they cannot see each other: tasks #538
and #539 delivered parts of #494's work and each asked the owner, with a Needs you, to relay a
message to #494. Task agents already read `tasks_list` and `task_get` (status, plan, waiting
texts, request) and `scopes_list`, but not a task's owner, pull request, delivery stage, its own
claims, what Yulius says it waits for, or its screen. Messaging between tasks was removed
(SPEC-012, SPEC-028) and `task_message` is a person's tool (ADR-0178). A task's agent acts for its
owner (SPEC-070); the owner's personal agent secrets and personal MCP servers reach only tasks
that person holds (SPEC-245, SPEC-332), and what those tools return can show in the task's plan,
screen and transcript.

## Decision

- `task_get` answers, for every caller, the task's `owner`, `delivery` (stage, PR, armed merge),
  `claims`, `waits_for` (Yulius's own sentences) and `screen` (its last 20 lines, redacted as the
  page's terminal is). The read writes nothing. No tool or argument that messages, starts, stops
  or changes a task is added.
- A task's agent reads its own project only and only while its owner may view it (enabled, read
  access), the rule a person's view follows (ADR-0331).
- When the peer reaches its owner's personal secrets or MCP servers and the reading task does not
  hold the same person's, every free text (request, plan titles and notes, waiting texts, claim
  notes, screen, transcript) is withheld from `task_get`, `tasks_list`, `session_read` and
  `sessions_search`, and the answer says so. Structure stays: number, title, status, branch, PR
  and stage, step counts, owner, claim patterns, `waits_for`.
- The agents' instructions say to read the neighbours before overlapping work and never to ask a
  person to relay a status they can read; only a person, through the Superagent, sends a task a
  message.

## Alternatives

- A new `task_status` tool beside `task_get`: a second version of the same view, which agents
  would have to learn to pick between; rejected.
- Serving the Superagent's `task_screen` and `task_delivery` to task agents: three calls for one
  question, and the person tools carry person-only parts (actions, merge controls); rejected.
- Withholding by reader's owner only (another person's task): an automation's task holds no
  person's secrets yet is owned by its creator, so it would read the creator's personal task in
  full; rejected for the holder rule.
- Hiding personal tasks entirely: the neighbours would not see that the task exists, its branch
  or its PR, which is what prevents collisions; rejected.
- Messaging between tasks: rejected by the owner.

## Consequences

- An agent sees a neighbour's PR, stage, claims and screen in one call and has no reason to ask a
  person to relay them.
- A task that holds its owner's personal secrets shows the other people's tasks only its shape;
  their agents read its plan through its owner.
- Each `task_get` reads the task's summary and screen: heavier than before, bounded to one task.
- The existing tidy tools of task agents (`task_stop`, `task_archive`, `task_delete`,
  `task_update`, SPEC-066, SPEC-396) are unchanged.
