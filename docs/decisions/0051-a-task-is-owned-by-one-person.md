# 0051. A task is owned by one person, whose GitHub credentials it uses; control passes on request

## Status

Accepted. Specified in [SPEC-070](../../.ai/specs/SPEC-070-tasks-should-be-owned-by-a-specific-pers.md).
Extends [0007](0007-github-app-authority.md).

## Context

Since 2026-10-11 a task acts on GitHub with its creator's user-to-server token (0007, GIT-003):
the agent's Git and `gh`, Yulius's pushes and pull requests, merges, promotions and issues. But
anyone with write access to the project could type into the task's terminal, send it messages,
continue, stop, archive or delete it and merge its pull request. They drove an agent that holds
someone else's GitHub token (task #66).

## Decision

- Every task has an owner (`tasks.owner_id`, schema version 17), its creator when created.
  `created_by` stays who asked; everything that acts on GitHub or for a person (the token, the
  worktree's commit identity, the agent's management and delivery authority) follows the owner.
- Only the owner changes a task. `App::load_task` refuses every action beyond viewing to anyone
  else (403 naming the owner), so every task route gets it; the terminal opens read-only and
  rechecks ownership on each input. Writing into the task's worktree through the editor's file
  API or an upload, and opening a shell in it, are the owner's too (its agent commits and pushes
  what is there as them). There is no exception for administrators or the superadmin.
- Anyone else with write access to the project asks for control. One request waits per task,
  stored on the task. The owner hands over or declines, and the asker may withdraw. A hand-over is
  one conditional update (the owner and the request the decision saw), then the commit identity,
  a terminal notice, an audit entry and the published summary. The next GitHub call of the task,
  also in a run that goes on, uses the new owner's token.
- When the owner can no longer act (disabled, deleted, or without write access to the project), a
  request passes control at once, whatever request waits, so a task is never stuck with an absent
  owner. A waiting asker who can no longer work gives way to the next person who asks, and a
  hand-over to them is refused; both drop the request, audited (`task.control_dropped`).

## Alternatives

- **Rewrite `created_by` on a hand-over**: loses who asked, and the creator's draft attachments
  are found by `created_by`.
- **Check ownership in each route**: every new write path would have to remember it; the one gate
  covers them all.
- **Leave stop, archive and delete open to any writer**: they are the owner's work too, and the
  absent-owner rule covers a task nobody can reach.
- **Let administrators take control without asking**: the point is protecting a person's
  credentials; an administrator asks like anyone else.
- **A request table with a queue**: nothing to queue; one waiting request is enough and is
  cleared by the same update that passes control.

## Consequences

Two people cannot both drive one task; a person who wants to help asks first. What this does not
prevent: a person's shell in the sandbox repository runs with the sandbox's files, so it can
reach another task's worktree on disk; containment isolates sandboxes, not tasks within one. A non-owner's
terminal is read-only, also on another person's tasks they could type into before. Tests that had
a second person write to a task now use the owner on another device or expect the refusal.
GitHub shows the new owner as the author of what the task does after a hand-over; commits already
made keep the earlier owner's identity.
