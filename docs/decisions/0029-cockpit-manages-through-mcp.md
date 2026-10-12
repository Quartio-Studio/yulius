# 0029. The Cockpit and task agents manage the project through the Yulius MCP server

## Status

Accepted. Specified in [SPEC-037](../../.ai/specs/SPEC-037-i-got-something-like-this-tasks-and-cock.md).
Supersedes the "no MCP server" part of [0020](0020-cockpit-as-a-plain-shell-session.md); the
Cockpit still has no hooks, settings, briefing, gates or approval bypass.

## Context

People want to tell the Cockpit "create a task from that" or "every day at 4am align the docs to
the latest changes", and agents want to split off follow-up work. Task agents could already
create automations but no tool created a task. The Cockpit had no Yulius tools at all, and the
API needs a person's login. Its environment held only a workspace token for Git credentials. That
token lives in memory and changes on every server restart, while a Cockpit outlives the restart.

## Decision

- A `task_create` tool creates a task through the same function as New task
  (`App::create_task_as`): workflows, Jev's auto choice, chains, start rules and the onboarding
  gate behave the same. Origin is `agent`; the creator is the person the caller acts for. Tools
  create at most 20 tasks per project in a rolling hour (in memory).
- A management tool acts for a person: a task's agent for the task's creator, a Cockpit for its
  owner (`coordination::ProjectCaller`). Write access is rechecked at every
  call that creates or changes something, and at every call of a Cockpit.
- The Cockpit gets the `yulius` MCP server as inline `--mcp-config` JSON whose header names
  `${YULIUS_COCKPIT_TOKEN}`. The bearer goes in the secret environment only. It is
  `ck_` plus the SHA-256 of a fixed label and the Cockpit shell's sealed host token, so it
  survives a restart with the Cockpit and ends with it. It is served only the management tools:
  tasks, automations, workflows and sessions. Read-only tools are pre-allowed (`--allowedTools`);
  creating or changing something asks the person first. The config is not strict, so the
  person's own MCP servers keep working.

## Alternatives

- **A `yulius task create` command with the workspace token**: a second protocol beside MCP, and
  the token is lost on a server restart while the Cockpit keeps running.
- **REST with the person's session**: the Cockpit has no cookie; giving it one grants the whole UI
  API.
- **A random token per Cockpit launch, or one stored in the database**: a random one is lost on a
  restart, and a stored one needs a migration and its own revocation. The derived one needs
  neither.
- **The full task tool set for the Cockpit**: steps, specs, scopes and delivery belong to a task;
  the person asked for managing tasks and automations.

## Consequences

The Cockpit can create and change tasks and automations for its owner after one approval in the
TUI. Anyone who can read the Cockpit's environment can act as its owner. That is only the owner's
own sandbox identity, which already holds their Git credential. A Cockpit started before the
update has no tools until it starts again. The hourly bound is per server process: a restart
resets it.
