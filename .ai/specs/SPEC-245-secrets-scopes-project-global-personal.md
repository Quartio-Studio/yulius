---
yulius-task: 052449aa-0f5f-4c85-8429-61fe063ba484
status: in-progress
---

# SPEC-245: Agent secrets have three scopes: project, global, personal

Status: in-progress
Date: 2026-10-20
Owner: Yulius task #231
Scope authorization: task #231, from the feedback of an early external user who forked Yulius
("secrets are kind of pointlessly done imo, but it's hard to figure out how they should work
sensibly"). In the fork, the Secrets page reads "Secrets — keys and logins agents use without
seeing them" and has the tabs Project / Global / Personal. Decision:
[ADR-0186](../../docs/decisions/0186-agent-secrets-are-project-global-or-personal.md).

## Goal

Agent secrets (the keys and logins agents use without seeing them) get three scopes:

- **Project**: belongs to one project (sandbox). What every agent secret of a sandbox is today.
- **Global**: defined once for the instance by an administrator; each project switches each
  global secret on or off. Example: a token for pulling private packages that many repositories
  need.
- **Personal**: defined once by one person and switched on per project by that person. A task the
  person started gets it; tasks of other people never do, and other people never see it.

The guarantees stay: agents never see values, the egress proxy adds a secret only to requests for
its bound hosts, `secret_copy_to_env` asks a person once per secret, and the MCP tools
(`secrets_list`, `secret_request_hosts`, `secret_copy_to_env`, `secret_verify`,
`secret_create_from_file`, `browser_fill_secret`) keep their arguments and answers.

## Context

- **Vault** (ADR-0014, SPEC-002 ADR-20): `vault_secrets` rows, section `team` (people reveal them)
  or `agent` (agents use them). Payloads are sealed with the row id and section as associated
  data. `crates/yulius-store/src/vault.rs`, `crates/yulius-app/src/vault/`.
- **Scopes today** (SPEC-216, ADR-0171): `sandbox_id` set means one sandbox; `NULL` means every
  sandbox ("team-wide", "instance"; only instance administrators manage those agent secrets). An
  agent name must be unique wherever it applies (`ensure_agent_unique`). Every run of a sandbox
  gets every agent secret of the sandbox and every team-wide one (`vault_agent(Some(sandbox))`,
  `App::applied_agent_secrets`), plus the instance Vidoc key as a virtual `VIDOC_API_KEY` when the
  sandbox has none (SPEC-195).
- **Who reads the set**: the run environment (placeholders, `vault_run_setup`), the egress proxy
  (`EgressPolicy` for `EgressCaller::{Agent, Ci, Superagent}`), the Superagent
  (`vault_superagent_setup`), the MCP tools, `${secret:NAME}` MCP references
  (`agent_secret_value`), `.env` copies (`env_files.rs`), CD (`cd_secret_values`), CI grants
  (`ci/exec.rs`, ADR-0116), redaction (`agent_secret_matcher`, Bash wrapper fingerprints,
  terminals, transcripts, MCP results).
- **Who started a task**: `tasks.owner_id` (SPEC-070) is the person a task acts for; a hand-over
  changes it. An automation's task is owned by the automation's creator
  (`TaskOrigin::Automation`), and anyone who may configure the project can change an automation's
  prompt (`update_automation`, `Action::Configure`).
- **Shared surfaces**: all tasks of a sandbox share its file system (worktrees isolate changes,
  not capabilities: AGENTS.md) and its project browser (`browser_session(sandbox)`).
- **Team secrets** (people's logins, revealed with two-factor) keep their own section and scopes
  (SPEC-216); this change is about agent secrets.

## Requirements

### Scopes (SCP)

- SCP-001. An agent secret is exactly one of: project (`sandbox_id` set, no owner), global
  (neither), personal (`owner_id` set, no sandbox). The API names them `sandbox`, `instance` and
  `personal` (the first two as before).
- SCP-002. A name is unique within its scope: per project, among global secrets, per owner among
  their personal ones. A project secret, a global one and a personal one may share a name.
- SCP-003. A global or personal secret reaches a project only while it is switched on for it
  (`vault_enabled`). A new global or personal secret is on for the project it is added from and
  off everywhere else.

### Resolution (RES)

- RES-001. The secrets a run uses are resolved per name with precedence **project > personal >
  global**; the instance Vidoc key comes last (SPEC-195). A shadowed secret is not in the run's
  set: no placeholder, no proxy injection, no tool lists it.
- RES-002. A task's run gets the personal secrets of the task's owner that are on for its project,
  unless the task was started by an automation (`origin = automation`): those get none. The
  Superagent gets its person's.
- RES-003. Other people's personal secrets never reach a run: not in its environment, its proxy
  injection, its tool answers, its `.env` copies, its browser or its fingerprint file.
- RES-004. A CI job's `secrets = "NAME …"` grant resolves among the project's secrets and the
  global ones on for the project (project first); never personal ones, as a CI run belongs to no
  person. CD resolves the same way.
- RES-005. A personal secret is used only through the egress proxy: `secret_copy_to_env`,
  `${secret:NAME}` MCP references and `browser_fill_secret` refuse it (or skip it), saying why,
  as each would put its value where other people's tasks of the project read it. It is never
  allowed for production deploys.
- RES-006. Redaction of a project's text (transcripts, terminals, MCP results, CI and CD logs,
  proxy responses) covers every secret that can reach any of its runs, personal ones of every
  owner included.

### Management (MAN)

- MAN-001. Project secrets: as today (people with write access manage them; people who can view
  the project list them).
- MAN-002. Global secrets: instance administrators create, edit, delete, reveal them and switch
  them on or off per project. Everyone who can view a project sees the global secrets (name,
  description, hosts) and whether each is on there.
- MAN-003. Personal secrets: only their owner lists, creates, edits, deletes, reveals them,
  answers their requests and switches them on or off for a project the owner may work in. To
  everyone else, administrators included, they do not exist (absent from lists, 404 by id); their
  vault activity is shown to their owner only.
- MAN-004. `PUT /api/sandboxes/{sid}/agent-secrets/{id}/enabled` `{ "enabled": bool }` switches a
  global or personal secret for a project; audited `vault.agent.enable` / `vault.agent.disable`.
  A project secret answers `Invalid`.
- MAN-005. The list of a project marks each global or personal secret `enabled`, and every secret
  a run would not get because another scope's secret of the same name wins `shadowed_by`
  (`sandbox` or `personal`).

### Migration (MIG)

- MIG-001. Schema version 38 keeps every row: project secrets stay project secrets, and every
  instance-wide agent secret becomes a global secret switched on for every project that exists
  (not removed), so no run loses a secret it had. Team secrets are unchanged.

### Web UI (WUI)

- WUI-001. The Secrets page reads "keys and logins agents use without seeing them" and has the
  tabs Project, Global, Personal and Team secrets (people's logins). `?section=agents` (links
  Yulius prints) opens Project.
- WUI-002. Global and Personal list their secrets with an "On for this project" switch (Global:
  instance administrators may switch; others see the state). A shadowed secret says which secret
  wins. Adding from a tab creates a secret of that scope (Global: administrators only).

## Plan

- [x] Spec and ADR-0186 for the three scopes
- [ ] Schema 38: owners, project switches, names unique per scope; team-wide agent secrets become globals switched on everywhere
- [ ] A run's secrets resolve by scope and precedence, personal ones only for their owner's tasks
- [ ] Personal secrets stay in the proxy; redaction and fingerprints cover each scope
- [ ] Management API: scopes, permissions, switches and visibility
- [ ] Secrets page: Project, Global and Personal tabs with switches
- [ ] Documents, gates and delivery

## Acceptance criteria

- A personal secret of person A that is on for project P: a task A started in P gets its
  placeholder and the proxy adds it for its hosts; a task B started in P, an automation's task in
  P, a CI job of P and a CD run of P do not; B's list of P does not show it; B's request for its
  id is 404.
- A global secret off for P is not in P's runs; switched on by an administrator it is; a member
  without administration cannot switch it (403).
- With project, personal and global secrets of one name, a run gets the project one; without the
  project one, its owner's personal one; otherwise the global one; the list marks the others
  shadowed.
- After the upgrade, a database with a team-wide agent secret and two sandboxes has a global
  secret on for both, and both sandboxes' runs still get it.
- `secret_copy_to_env` and `browser_fill_secret` refuse a personal secret with a message that
  says why.

## Test plan

- Store (`crates/yulius-store/src/vault.rs` tests): the 0039 migration on a version-37 database
  (team-wide agent secret → global, on for live sandboxes; project secrets untouched); names
  unique per scope; switches by id and sandbox; removing a user or sandbox removes their
  personal secrets or switches.
- App unit tests (`vault/reach.rs`): precedence and shadowing, the automation rule, redaction set.
- Integration (`crates/yulius/tests/secrets_scopes.rs`): two people, one project — the personal
  secret reaches the owner's task through the proxy and not the other's, nor an automation's task
  or a CI job; visibility and 404; administrator-only global switches; precedence through the
  proxy; refusals of `secret_copy_to_env` and `browser_fill_secret`.
- e2e (`e2e/tests/secrets.e2e.ts`): the tabs, adding a personal secret, switching a global one.

## Decisions

See [ADR-0186](../../docs/decisions/0186-agent-secrets-are-project-global-or-personal.md):
precedence project > personal > global instead of an error; administrators switch global
secrets; personal secrets stay in the proxy; automations' tasks get no personal secrets;
team-wide secrets become globals switched on everywhere instead of copies per project.
