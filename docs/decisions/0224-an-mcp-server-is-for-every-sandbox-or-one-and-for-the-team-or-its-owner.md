# 0224. An MCP server is for every sandbox or one, and for the team or its owner

## Status

Accepted. Specified in [SPEC-332](../../.ai/specs/SPEC-332-mcp-should-have-2-config-fields-per-sand.md);
task #316. Extends [0186](0186-agent-secrets-are-project-global-or-personal.md) (whose holder rule
it reuses) to MCP servers.

## Context

An MCP server was the instance's (instance administrators) or one sandbox's (its writers), chosen
once when it was added, and every run of its scope got it. A founder who wanted their own admin
MCP server (founder credentials, fleet costs) was told by an agent not to add it to Yulius at all:
"every server added on Yulius's MCP page is attached to every task run in every agent". The request
asks for two settings, every sandbox or one, and personal or team-wise, four combinations in all,
defaulting to global and team-wise, and for configuring them to be frictionless.

Constraints: a run's holder (whose personal credentials it may hold) is already defined for agent
secrets: a task's owner unless an automation started it, the Superagent's person, nobody for CI
and CD (`Holder`, ADR-0186). A task's MCP servers are fixed when its run starts. Tasks of one
sandbox share its file system and its sandbox user.

## Decision

- **Two settings, one column each.** `sandbox_id` (NULL: every sandbox) as before, and a new
  `owner_id` (NULL: the team), removed with the user's row. The API shows `scope` and `audience`.
  Names are unique per (sandbox, owner); at most 50 servers each.
- **A personal server reaches its holder alone.** `App::mcp_attachments(sandbox, holder)` adds
  only the holder's personal servers; an automation's task, CI and CD hold nobody. To everyone but
  its owner, administrators included, a personal server does not exist (not listed, 404).
- **Precedence: reach, then personal.** Per name: this sandbox's personal > its team > every
  sandbox's personal > its team, also when the winner is disabled, so a person can turn a team
  server off for themselves. A sandbox's team decision still beats one person's every-sandbox
  server there, as a project secret beats a personal one (ADR-0186).
- **Who sets what.** Every sandbox + team: instance administrators. One sandbox + team: its
  writers. Personal: anyone who may work in the sandbox they add it from. An update may change
  either setting with the right to manage the server where it goes; making it personal makes the
  caller its owner.
- **Defaults.** The editor starts on every sandbox + whole team for an administrator (the
  request's default) and this sandbox + whole team for anyone else, who may still choose only me
  in every sandbox. The API default stays `sandbox` + `team` so existing clients keep working.
  Pasting `claude mcp add --scope user` picks only me, `--scope project` the whole team.
- **Directed by its owner.** `task_message` from anyone but the owner is refused for a task whose
  runs get the owner's personal MCP server, as for personal secrets.

## Alternatives

- **A per-person on/off switch for team servers** (as global secrets are switched on per
  project): keeps one owner for every server, but a person still could not add a server holding
  their own credential without every run of the scope getting it.
- **Personal over team regardless of reach** (every-sandbox personal beats one sandbox's team):
  lets one person silently replace what a project agreed on for its tasks of theirs; rejected for
  consistency with ADR-0186.
- **Changing the API default to every sandbox + team**: every non-administrator client that sends
  no scope would start failing with 403.
- **A fourth "global personal" table or a separate personal MCP page**: two places to look for
  the same thing; the MCP page lists both with an "Only you" badge.

## Consequences

- An agent can now honestly say a personal server reaches only its owner's own tasks and
  Superagent; the editor says so under the settings.
- A personal server's values reach its owner's run environment (`YULIUS_MCP_<SERVER>_<VAR>`).
  Personal limits whose runs get the server; it is not a containment boundary between tasks that
  share a sandbox's file system and user, so a credential that must never be readable by a
  teammate's concurrent task there belongs in a sandbox only its owner works in.
- A live run keeps the servers it started with; after a hand-over the task's next run gets the new
  owner's personal servers, not the previous owner's.
- `${secret:NAME}` references in any MCP server still never resolve personal secrets (ADR-0186).
- Schema version 44 adds `mcp_servers.owner_id`; existing servers are team servers.
