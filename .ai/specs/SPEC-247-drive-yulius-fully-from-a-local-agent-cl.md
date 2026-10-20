---
yulius-task: 0a8d30be-c585-4637-9904-82f44ae9e1fe
status: in-progress
---

# SPEC-247: Drive Yulius fully from a local agent (remote MCP and CLI)

Status: in progress
Date: 2026-10-20
Owner: task #233
Scope authorization: task #233. Feedback from an early external user, which they called the most
important point: "I'd focus on making it as convenient as possible to use with an agent, a local
one, so you can just say: make a task, restart it, etc." ("an MCP, or just some CLI"). The operator
asked for more: from a laptop, with a local Claude Code or any agent, onboard a repository into a
remote Yulius completely without the web UI, and manage its tasks, CI/CD, files, automations and
secrets (names and bindings only).

## Goal

A person with only the instance URL and a personal API token can, from their laptop:

- add Yulius to their local Claude Code with one command,
  `claude mcp add --transport http yulius https://<instance>/mcp --header "Authorization: Bearer <token>"`,
  and ask it to import a repository, start its onboarding, set up its CI, create, message,
  restart, stop and archive tasks, and watch their status, transcripts, pull requests and CI;
- do the same from a shell or script with the `yulius` CLI (`yulius remote login <url>`,
  `yulius task create|list|get|message|restart|stop`, `yulius project import`, `yulius ci …`,
  `yulius secret …`), which calls the same server tools with the same token.

## Context

Confirmed against `dev` at 06b58375.

- **The MCP server exists, but only inside the instance.** `App::mcp_handle`
  (`crates/yulius-app/src/mcp.rs`) serves JSON-RPC `initialize`, `tools/list` and `tools/call` at
  `POST /mcp` on the loopback listener only (`routes::internal`). Two callers: a task's agent (task
  token, every tool) and a person's Superagent/Cockpit (`ck_` token, `COCKPIT_TOOLS`: tasks,
  automations, workflows, sessions, agent secrets, `task_message`). Both are bound to one project:
  the Cockpit acts as `ProjectCaller { sandbox, user, actor: "cockpit:<login>" }`.
- **API tokens exist, unscoped and unmanaged.** `api_tokens` (`yul_…`, SHA-256 stored, `label`,
  `last_used_at` never written, `revoked_at`) are issued only by the CLI device approval
  (`yulius login`, label "yulius CLI"). `authenticate_bearer` accepts any unrevoked token for the
  whole REST API with the user's permissions. Nobody can list or revoke one.
- **The CLI only uploads.** `yulius login` (device flow), `yulius onboard` (upload a checkout and
  sessions) and `yulius secrets import` (values from a dotenv file or the terminal, never shown).
  The token store is `~/.config/yulius/…` per server (`yulius_import::TokenStore`).
- **Onboarding is browser-only by an earlier request.** SPEC-051 OBD-002: Start onboarding (and
  continuing a pending sandbox's onboarding task) refuses every bearer token, because the person
  asked that adapting the repository "should require explicit action in the GUI … never via the
  CLI". This task's operator asks for the opposite for a person's own remote agent.
- **Every UI action is an `App` method taking an `AuthContext`** (`create_sandbox`,
  `continue_task`, `pause_task_for_person`, `ci_overview`, `ci_rerun_run`, `cd_overview`,
  `agent_secrets`, `update_agent_secret`, …), which check the person's access level
  (`Action::{View, Work, Configure, Administer}` from their GitHub permission).
- **Yulius CI tests every open pull request** whose base is the integration branch
  (`ci_sync_sandbox`), and `ci_gate_result` tells whether the gate passed at a commit.
- **Reading files**: the Git port has `show_file` and `list_dir` at a revision of the sandbox's
  clone. Writing: `empty_repo.rs` already commits in a scratch clone and pushes with the
  installation token (SPEC-241).

## Requirements

### Tokens

- **RMT-001** A person creates API tokens on Settings → "Connect your local agent": a label and a
  scope, `operate` (everything they may do) or `read` (only what view access allows). The token is
  shown once, with the exact `claude mcp add …` and `yulius remote login …` commands filled in, and
  only its SHA-256 is stored. The list shows each token's label, scope, creation and last use
  (to the minute), and Revoke. A revoked token is refused at once. `GET/POST /api/me/tokens`,
  `DELETE /api/me/tokens/{id}` (own tokens only; another person's id is 404). At most 50 live
  tokens per person.
- **RMT-002** Tokens from `yulius login` (device approval) and all existing tokens have the scope
  `cli`: the REST API as before (`yulius onboard`, `yulius secrets import`), refused by the remote
  MCP with a message naming the Settings page. A `read` token is refused (403) by every REST
  request other than GET/HEAD. A disabled person's tokens are refused.
- **RMT-003** Creating and revoking a token is audited (`api_token.create`, `api_token.revoke`,
  label and scope, never the token). `last_used_at` is written at most once a minute per token.

### Remote MCP

- **RMT-010** The public listener serves `POST /mcp` (MCP streamable HTTP, JSON responses; `GET`
  and `DELETE` answer 405) to `Authorization: Bearer yul_…` only. A cookie session, a task token
  and a `ck_` token are refused there (401) and keep working on the loopback `/mcp` unchanged.
  `initialize` answers with instructions for a person's local agent.
- **RMT-011** `tools/list` serves the remote tools: the instance tools and, with a required
  `project` argument (`owner/name`, the slug or the id), the project tools. A `read` token is
  served only the tools that need view access.
- **RMT-012** Every project tool requires the person's access to that project for its action
  (view, work, configure or administer, as the same UI action), checked before the tool runs; the
  shared implementations check again. A project the person cannot view is "not found", the same
  as one that does not exist. A `read` token calling a tool that needs more is refused naming the
  scope.
- **RMT-013** The tasks, automations, workflows and sessions tools are the Superagent's own
  implementations (`tasks_list`, `task_get`, `task_create`, `task_message`, `task_stop`,
  `task_archive`, `task_delete`, `automation_*`, `workflows_list`, `sessions_search`,
  `session_read`), run as `ProjectCaller { actor: "remote:<login>" }`, so they behave, render and
  audit as for the Superagent.
- **RMT-014** At most 120 requests a minute per token; more are answered with a JSON-RPC error
  saying to wait. Tools that create tasks keep their 20 an hour per project.
- **RMT-015** Every remote `tools/call` of a tool that changes something is audited
  `remote.tool` with the actor `remote:<login>`, the project, the tool, the token's id and whether
  it succeeded; never the arguments. Results are redacted of the project's agent secrets as every
  MCP result is.

### Remote operations beyond the Superagent's

- **RMT-020** Projects: `projects_list` (projects the person can view, with onboarding state and
  access), `repositories_list` (GitHub repositories they may import), `project_import {repo}`
  (as Import in the UI; it stays `pending` until onboarding starts), `project_get` (the project,
  its onboarding state, what is pending and the onboarding plan).
- **RMT-021** Onboarding: `onboarding_start` (work access) starts a `pending` project's
  onboarding as Start onboarding does; `onboarding_skip` (administer). An `operate` token is the
  person's explicit delegation, so it may start onboarding through the REST API and the remote
  MCP; `cli` tokens are still refused (OBD-002 stays for them; ADR-0188).
- **RMT-022** Task lifecycle: `task_restart` (Continue in the UI: a resting task starts again; a
  running one is refused with "stop it first"), `task_pause`, `task_unpause`, `task_start_now`
  and `task_delivery` (pull request, checks and merge state).
- **RMT-023** CI/CD: `ci_runs` (the jobs in force and the latest runs), `ci_run {run_id, job?}`
  (a run's jobs, failing output), `ci_rerun`, `ci_cancel`, `cd_status`, `cd_deploy` (dev only;
  production is a person's action on the CD page, ADR-0110).
- **RMT-024** Files: `project_files {path?, ref?}` and `project_file_read {path, ref?}` read the
  integration branch (or a branch or commit) of the project's clone after a fetch; at most 256 KiB
  of text per file.
- **RMT-025** File changes (ADR-0189): `project_change {title, body?, files[{path, content} |
  {path, delete: true}]}` commits the given whole files as one commit, authored by the person, on a
  new branch `yulius/change-<id>` from the integration branch head, pushes it and opens a pull
  request into the integration branch; Yulius CI tests it as any pull request. Nothing is written
  into the sandbox's clone or any task's worktree. Paths are relative, without `..`, `.git` or
  absolute parts; at most 50 files and 256 KiB per file. Work access.
- **RMT-026** `change_merge {number}` squash-merges an open pull request of a `yulius/change-`
  branch once Yulius CI passed at its exact head (refused with the CI state otherwise), and
  deletes the branch. Work access; audited.
- **RMT-027** Secrets, names and bindings only: `secrets_list` (names, descriptions, bound and
  requested hosts, allowances; never values), `secret_update {secret, hosts?, expect_hosts?,
  description?}`, `secret_hosts_answer {secret, allow, hosts}`. Values enter only through
  `yulius secrets import` (unchanged) or the Secrets page.

### CLI

- **RMT-030** `yulius remote login <url>` reads a token (hidden prompt, or `--token-stdin`),
  checks it against the server's `/mcp` and stores it in the CLI token store (0600) for that
  server; `yulius remote logout`, `yulius remote status` (server, person, scope), `yulius remote
  tools` and `yulius remote call <tool> [JSON]`.
- **RMT-031** Shortcuts over the same tools: `yulius project list|import|get|onboard`,
  `yulius task list|get|create|message|restart|stop|archive|delete`, `yulius ci runs|run|rerun`,
  `yulius secret list`. `--project` defaults to the GitHub `origin` of the current checkout.
  Output is the tool's text; `--json` prints its record. Exit 1 on a tool error, 2 on a transport
  or authentication failure.

### Web UI and docs

- **RMT-040** Settings has the "Connect your local agent" section of RMT-001.
- **RMT-041** `docs/remote-control.md` walks through a laptop-only onboarding of a new repository
  (token, `claude mcp add`, import, onboarding, CI through `project_change`, tasks) and the CLI;
  `docs/api.md`, `docs/architecture.md` and `docs/product.md` describe the remote surface.

## Plan

- [ ] Scoped personal API tokens with list, create, revoke and last use
- [ ] Remote MCP endpoint with token scopes, project permissions, rate limit and audit
- [ ] Remote project import and onboarding tools
- [ ] Remote task lifecycle, CI and CD tools
- [ ] Remote file reads and change pull requests with merge on green
- [ ] Remote secret names and bindings tools
- [ ] yulius remote CLI client and shortcuts
- [ ] Connect your local agent section in Settings
- [ ] Laptop-only onboarding end-to-end test over the remote surface
- [ ] Remote control guide, ADRs, API, architecture and product docs
- [ ] Independent review approved and findings fixed
- [ ] Repository gate and security scan pass

## Acceptance criteria

- A token created through `POST /api/me/tokens` is returned once, stored hashed, listed with its
  scope and last use, and refused after `DELETE`; another person cannot list or revoke it.
- `POST /mcp` on the public router: no bearer → 401; a cookie, task or `ck_` token → 401; a `cli`
  token → refused naming Settings; `read` token → `tools/list` without write tools and a write
  tool refused; a person with read access to a project → `task_create` refused, `tasks_list` works;
  a project of another owner the person cannot see → not found.
- The 121st request within a minute from one token is refused.
- An integration test on real Git remotes, using only `POST /mcp` with one `operate` token:
  imports a repository, starts its onboarding, reads `yulius.toml`, sets up CI with
  `project_change` (a pull request with `[[ci.jobs]]`), sees the run, merges it with
  `change_merge` once green, and creates a task that the task list then shows.
- The CLI's `remote login`, `task list` and `remote call` work against a served instance.
- The Settings section creates, shows once and revokes a token (e2e).

## Test plan

- `crates/yulius/tests/remote_mcp.rs`: token lifecycle, auth refusals, scopes, per-project
  permissions, rate limit, audit, Superagent tools as remote tools, task lifecycle, CI/CD and
  secrets tools, file reads, `project_change`/`change_merge`, and the laptop-only end-to-end flow
  (`support::repos::World`).
- `crates/yulius/tests/cli.rs` (or a new file): `yulius remote` against a served router.
- Unit tests: path validation of `project_change`, scope parsing, tool tables (every remote tool
  has an action; `read` tools are view-only).
- e2e `e2e/tests/api-tokens.e2e.ts`: create, copy snippet, revoke.
- Gates: `scripts/dev ci`, `scripts/dev security`, the e2e suite.

## Decisions

- **One tool layer, two transports.** The remote MCP reuses `mcp_handle`'s dispatch with a third
  caller kind; the CLI is an MCP client of the same endpoint rather than a second REST client, so
  every operation exists once and the CLI never lags behind the tools.
- **Scopes `cli`, `read`, `operate`** (ADR-0188): existing tokens become `cli` so nothing that
  works changes and SPEC-051's rule holds for them; remote control needs a token made on purpose
  in Settings. Per-project power always comes from the person's access level, never the token.
- **File changes are pull requests** (ADR-0189): a remote agent never writes into a live sandbox
  or a task's worktree; exact edits (CI config) go through a change branch tested by Yulius CI;
  work that needs judgment is a task (`task_create`).
- **Production deploys stay a person's action in the browser** (ADR-0110 unchanged).
- **This is one pull request** larger than the usual 400 lines: the tokens, the endpoint and the
  CLI are only usable together; the tools are thin wrappers over existing `App` methods.
