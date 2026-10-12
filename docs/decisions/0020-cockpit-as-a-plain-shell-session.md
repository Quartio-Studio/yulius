# 0020. The Cockpit is a plain Claude Code session on the shell runtime

## Status

Accepted. Specified in [SPEC-021](../../.ai/specs/SPEC-021-in-this-bar-we-need-to-add-cockpit-at-th.md).
Amended by [0029](0029-cockpit-manages-through-mcp.md): the Cockpit gets the Yulius MCP server with
the management tools; everything else below still holds.

## Context

Every user wants one permanent Claude Code session per sandbox, first in the sidebar, for quick
questions and small work that should not become a task. They can `/clear` it but never delete
it, and it must be "a plain session without any additional requirements, hooks etc". A task
always brings Yulius's hooks, status line, MCP server, briefing, discipline gates, a number, a
branch and a worktree; sandbox shells are already private per user and sandbox, run as the
user's sandbox identity, survive a server restart and carry no task token or hooks.

## Decision

The Cockpit is a **shell of kind `cockpit`**: the shell runtime (ownership, containment, the
terminal protocol, adoption after a restart, the idle reaper) running `claude [--resume <id>]`
instead of the login shell. `POST /api/sandboxes/{sid}/cockpit` returns the caller's running
Cockpit or starts one under a lock (one per user and sandbox). Each start takes the pool's best
subscription for an Opus run (`pool_account`) and passes its token in the secret environment;
the harness port plans the launch (`plan_cockpit`: no settings, MCP config or approval bypass;
the default model and effort of ADR-0024) and settles Claude Code's first-run screens and
folder trust in `~/.claude` of the user's own home. A start
resumes the newest conversation file for the repository there, so `/clear`, `/exit`, an idle
stop or a lost process never lose it. The Cockpit is never listed, renamed or closed as a
terminal and does not count against the terminal limit; files can be dropped onto it.

## Alternatives

- **A hidden task kind**: reuses the supervisor's account rotation and session lineage, but every
  task path (briefing, Stop gates, steps, spec check, delivery, numbering, branches, worktree
  pruning, lists, analytics) would need an exception, and the user asked for no hooks.
- **`claude --continue`**: exits with an error when there is no conversation yet; the newest
  conversation file is known and also follows `/clear`.
- **A separate Cockpit runtime**: a second copy of shell supervision and adoption.

## Consequences

The Cockpit has none of a task's coordination: no activity in the sidebar, no subscription
switch at a limit (Start again picks another), no session slot. Its conversations live in the
user's home, so they survive and stay private to the user. The token is visible to the user's
own session, as it already is to the agents of tasks they start.
An older binary (a rollback) ignores the kind and adopts a running Cockpit as a closable terminal
named "Cockpit"; nothing is lost.
