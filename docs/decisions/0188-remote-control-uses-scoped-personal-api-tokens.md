# 0188. Remote control uses scoped personal API tokens

## Status

Accepted. Specified in [SPEC-247](../../.ai/specs/SPEC-247-drive-yulius-fully-from-a-local-agent-cl.md);
task #233. Amends SPEC-051 OBD-002 (onboarding starts only in the browser) for `operate` tokens.

## Context

- An early external user called this the most important point: drive Yulius from the agent on
  their own computer ("make a task, restart it"), through MCP or a CLI. The operator asked for
  more: onboard a repository completely from a laptop, without the web UI.
- Yulius already serves MCP, but only on the loopback listener, to a task's agent (task token)
  and a person's Superagent (`ck_` token), each bound to one project.
- API tokens (`yul_…`, SHA-256 stored) existed only as the result of `yulius login`'s device
  approval. They had no scope, nobody could list or revoke one, and `last_used_at` was never
  written. Every bearer token could do through the REST API whatever its person could.
- SPEC-051 OBD-002 refuses every bearer token to start onboarding, because a person asked that
  adapting a repository "should require explicit action in the GUI … never via the CLI".

## Decision

- **Tokens.** A person makes tokens on Settings → Connect your local agent, each with a label and
  a scope: `operate` (everything the person may do) or `read` (what view access allows). It is
  shown once, with the exact `claude mcp add` and `yulius remote login` commands, and stored as
  its SHA-256 (`api_tokens.scope`, migration 0040). Only a browser session makes one: a token
  cannot mint tokens. At most 50 live tokens a person. The list shows scope, creation and last
  use (written at most once a minute); Revoke ends a token at once, from the person's browser
  or any of their tokens that may change things. Making and revoking are audited (`api_token.create`, `api_token.revoke`),
  with the label and scope, never the token.
- **Existing tokens are `cli`.** `yulius login` tokens and every token made before keep the REST
  API as it was and are refused by the remote MCP (403, naming Settings). SPEC-051's rule holds
  for them: they cannot start onboarding.
- **A token never grants more than its person.** Per project, the person's access level (from
  GitHub) decides as everywhere else; the scope can only narrow it. A `read` token is refused
  (403) by every REST request other than GET and HEAD, and by every remote tool needing more than
  view access; `tools/list` serves it only the tools that read.
- **An `operate` token acts for the person**, so it may start onboarding through the REST API and
  the remote MCP (`AuthContext::acts_for_person`): the person made it on purpose for their agent,
  and starting onboarding is still an explicit call (`onboarding_start`), never a side effect.
- **The remote MCP** is `POST /mcp` on the public listener (MCP streamable HTTP, JSON answers;
  `GET` and `DELETE` are 405). It reads only `Authorization: Bearer yul_…`: no cookie, so no
  browser session can be ridden; task and `ck_` tokens get 401 there and keep the loopback
  `/mcp`. A refused token is answered 401 (`WWW-Authenticate: Bearer`), 403 or 429, so MCP clients
  report it.
- **One tool layer.** The remote MCP is a third caller of the existing dispatch. The
  Superagent's tools (tasks, automations, workflows, sessions, secrets list) run as a
  `ProjectCaller` with the actor `remote:<login>`; the others call the functions behind the web
  UI's routes with the person's `AuthContext`. Every project tool takes `project` (owner/name,
  slug or id) and declares the action it needs, checked before it runs; a project the person
  cannot see is "not found".
- **Rate limit.** 120 requests a minute per token (in memory, the sign-in limiter's sliding
  window), then 429 with `Retry-After: 60`. Tools that create tasks keep their 20 an hour per
  project.
- **Audit.** Every remote call of a tool that changes something, refused or done, is audited
  `remote.tool` with the actor `remote:<login>`, the project, the tool, the token's id and whether
  it succeeded, never the arguments (they carry prompts and file contents). The tools' own audits
  name the same actor.
- **The CLI** is a client of the same endpoint (`yulius remote login|logout|status|tools|call`,
  `yulius project|task|ci …`, `yulius secrets list`), so each operation exists once. Its token is
  checked with `initialize` before it is stored, in `remote-credentials.json` (0600) beside
  `yulius login`'s file, which it never replaces.
- **Not remote:** production deploys (ADR-0110: a person on the CD page), secret values in either
  direction (they are set on the Secrets page or with `yulius secrets import`), token management.

## Alternatives

- **Reuse `yulius login` tokens for the remote MCP.** Rejected: every CLI token would silently
  become an agent's key with the power to start onboarding, against SPEC-051's request, and none
  could be told apart or revoked.
- **OAuth 2.1 authorization for MCP clients.** Deferred: Claude Code supports it, but it needs
  dynamic client registration, consent screens and refresh tokens; a pasted bearer token is one
  command today and works with every agent and script. The scopes map onto OAuth scopes later.
- **Fine-grained scopes per action** (tasks:write, ci:read, …). Rejected for now: two scopes cover
  "let my agent work" and "let a dashboard read"; the person's GitHub access already varies per
  project.
- **A separate REST client in the CLI.** Rejected: two clients of two surfaces drift; the MCP's
  tools are the contract both the agent and the CLI use.
- **A rate limit in the store.** Rejected: one process serves the instance; a restart resetting
  the window costs nothing.

## Consequences

- `claude mcp add --transport http yulius https://<instance>/mcp --header "Authorization: Bearer
  <token>"` gives a local Claude Code the whole project management surface; `yulius remote login`
  gives a shell the same.
- A leaked `operate` token is its person's power until revoked; the Settings list shows its last
  use, and revoking is immediate. Tokens do not expire by themselves.
- Disabling a person refuses all their tokens at once (as before).
- New remote tools must declare their action in `REMOTE_TOOLS`; a unit test fails when one is
  served without a descriptor, or a read token would be served a tool that changes things.
