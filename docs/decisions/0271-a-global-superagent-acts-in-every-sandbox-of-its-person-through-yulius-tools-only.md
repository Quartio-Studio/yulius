# 0271. A Global Superagent acts in every sandbox of its person, through Yulius tools only

## Status

Accepted. Specified in [SPEC-427](../../.ai/specs/SPEC-427-there-should-be-2-superagens-global-and.md);
task #409 ("there should be 2 superagents: global and sandbox-wise. Global has access to all
sandboxes and can synchronise them"). Adds a second Superagent beside the one of
[0020](0020-cockpit-as-a-plain-shell-session.md) and
[0029](0029-cockpit-manages-through-mcp.md); leaves
[0227](0227-the-superagent-starts-tasks-in-other-sandboxes-only-on-its-persons-approval.md) (the
sandbox Superagent's approved cross-sandbox requests) as it is, and reuses the tools of
[0188](0188-remote-control-uses-scoped-personal-api-tokens.md) (the remote MCP).

## Context

Each person has a Superagent per sandbox: Claude Code in that sandbox's repository, contained in
it, managing that project. People with several repositories (a service and its clients, a website
and its API) want one agent that sees all of them and keeps them in sync, without switching
sandboxes or approving each step. The sandbox Superagent may reach another sandbox only on the
person's click per request (ADR-0227), because it reads repository files and runs shell commands
without approvals: text it reads could otherwise start work everywhere.

The remote MCP already serves a person's own agent every project they can see, each tool checked
against the person's access at the call, changes audited, merges only on green CI. Every run
Yulius starts is contained in a sandbox (ADR-0003), and the person's home is writable by all
their task runs.

## Decision

- **One Global Superagent per person**, `POST /api/superagent`, beside the sandbox Superagents.
  It is a Cockpit shell marked `global`: start lock, usage-limit moves, update refreshes, adoption
  after a restart and the terminal protocol are the sandbox Superagent's.
- **It acts only through Yulius.** Claude Code runs with no built-in tools (`--tools ""`) and only
  the Yulius MCP (`--strict-mcp-config`): no shell, no file reads or writes, no web fetches, no
  other MCP server, no agent secrets and no redaction hook (nothing it runs could print one).
- **Its tools are the remote MCP's for its person, minus the person's decisions.** Its `gs_`
  bearer resolves to a caller acting as an operate token of its person would, with the actor
  `superagent:<login>`, the Superagent channel ("through their Superagent") and `remote.tool`
  audits marked `global-superagent`. It is not served the vault and variable tools, `cd_deploy`,
  `project_import`, `onboarding_start` or `onboarding_skip`. A `ck_` bearer never reaches these
  tools, and a `gs_` bearer never the sandbox Superagent's.
- **No approval per action.** The person asked for an agent with access to all their sandboxes;
  its reach is exactly theirs, checked per call, and what it changes goes through tasks, pull
  requests Yulius CI tests, or task messages.
- **Hosted by a sandbox, contained in its own root.** Shells are registered per sandbox (private
  terminal, idle end, removal). The Global Superagent is registered under the oldest live sandbox
  its person can work in; the process itself runs in its own folder `users/<id>/superagent` (its
  working folder and Claude Code config), on a contained server in a bubblewrap namespace whose
  only writable root is that folder, without a sandbox holder, repository or home. A removed or
  lost host ends it, and the next open starts it under another sandbox, resuming the conversation
  kept in its folder.
- **The web UI opens it outside a sandbox.** On All sandboxes, Settings and the other pages without
  a sandbox, the rail's Superagent opens the Global Superagent; in a sandbox, that sandbox's.

## Alternatives

- **Give the sandbox Superagent a `sandbox` argument on every tool.** One agent, but it would run
  shell commands in one repository while acting in all of them, the risk ADR-0227 refused; with
  per-action approval it would not be the hands-off coordinator asked for.
- **Approve every cross-sandbox action of the Global Superagent too.** Safe but not what was asked
  ("has access to all sandboxes and can synchronise them"); its lack of shell and file tools and
  the per-call access checks carry the risk instead.
- **A sandbox-less shell kind with its own registry and routes.** Cleaner in the model, but it
  would duplicate the terminal, adoption, idle and limit machinery the Cockpit already has; a host
  sandbox costs one lookup.
- **Running it in its host sandbox's holder.** It would see that repository, its network and its
  processes, and its reach would depend on which sandbox is the host.
- **Its config in the person's home.** Every task run of the person can write there (Claude Code
  settings with hooks), and the Global Superagent runs outside every sandbox.

## Consequences

- "Which of my sandboxes have failing CI?", "the API changed in acme/api: make its clients follow"
  and "tell every running task to rebase on dev" work from one panel, as tasks, pull requests and
  messages in each sandbox, audited.
- It cannot read a file except through `project_file_read` and cannot write one except through a
  `project_change` pull request; code work stays in tasks.
- An injected instruction in something it reads can act with the person's reach over Yulius's
  tools (start tasks, open and merge green pull requests, message tasks), never deploy, touch the
  vault or add projects; every change is audited under the person's name.
- Contained without a holder, it runs the host's Claude Code installation, not the newer one
  isolated runs bind (ADR-0266), until the host's is updated.
- A person who can work in no sandbox (read-only access, a guest without one) has no Global
  Superagent.
