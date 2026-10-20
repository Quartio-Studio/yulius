---
yulius-task: 052449aa-0f5f-4c85-8429-61fe063ba484
status: done
---

# SPEC-245: Agent secrets have three scopes: project, global, personal

Status: done
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
  unless the task was started by an automation (`origin = automation`): those get none, and so do
  the tasks such a task creates (they are automation tasks too). The Superagent gets its person's.
  After a hand-over (SPEC-070) the run's next request resolves the new owner's, not the previous
  owner's (cached proxy credentials of the task are dropped).
- RES-007. Only its owner messages a task that holds personal secrets (`task_message`): another
  person's Superagent would steer an agent holding their credential.
- RES-003. Other people's personal secrets never reach a run: not in its environment, its proxy
  injection, its tool answers, its `.env` copies, its browser or its fingerprint file.
- RES-004. A CI job's `secrets = "NAME …"` grant resolves among the project's secrets and the
  global ones on for the project (project first); never personal ones, as a CI run belongs to no
  person. CD resolves the same way.
- RES-005. A personal secret is used only through the egress proxy: `secret_copy_to_env` and
  `browser_fill_secret` refuse it, saying why, and `${secret:NAME}` MCP references resolve, as CI
  does, among the project's and the switched-on global secrets only: each would put its value
  where other people's tasks of the project read it. It is never allowed for production deploys.
  An agent's `secret_create_from_file` refuses a name that already reaches its own runs in any
  scope (SPEC-211: an existing name is never replaced). It cannot see other people's personal
  secrets, so a project secret it adds may shadow one; that person's card says so.
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
  vault activity is shown to their owner only (once deleted, like any deleted secret's, to
  instance administrators).
- MAN-004. `PUT /api/sandboxes/{sid}/agent-secrets/{id}/enabled` `{ "enabled": bool }` switches a
  global or personal secret for a project; audited `vault.agent.enable` / `vault.agent.disable`
  (a global secret's switches shown to administrators only, as they name projects). A project
  secret answers `Invalid`; switching on past 200 secrets reaching the project answers
  `Capacity`. A secret added as global or personal is inserted and switched on in one
  transaction.
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
- [x] Schema 38: owners, project switches, names unique per scope; team-wide agent secrets become globals switched on everywhere
- [x] A run's secrets resolve by scope and precedence, personal ones only for their owner's tasks
- [x] Personal secrets stay in the proxy; redaction and fingerprints cover each scope
- [x] Management API: scopes, permissions, switches and visibility
- [x] Secrets page: Project, Global and Personal tabs with switches
- [x] Documents, gates and delivery
- [x] Review round 1 fixed: automation children, task messages, hand-over cache, switch limit
- [x] Gates green and an independent review of the delivered head

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

## Evidence

Observed on the branch (2026-10-20), head `8ab9c910` plus this spec's own text:

- `scripts/dev ci`: exit 0 (check, fmt, clippy, tests, docs, deps, macOS targets); nextest
  2283 passed, 17 skipped. Earlier runs on the way stopped at `cargo fmt --check` and at a
  private intra-doc link (`cargo doc`), both fixed.
- `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^secrets_scopes::/)'`: 9
  passed — personal secrets reach only their owner's task through the proxy (not another
  person's task, an automation's task, a CI job; no forwarded header or path carries the value),
  are listed and reachable by id for no one else (404, administrators included), are absent from
  others' activity and fingerprint files but redacted from the project's text; global secrets
  reach only projects an administrator switched them on for (403 for a writer, 422 for a project
  secret); precedence project > personal > global through the proxy and `shadowed_by`; `.env`
  copies, browser filling, MCP references and production refuse personal secrets; readers cannot
  add or switch one on. Review round 1 added: a task an automation's task creates holds no
  personal secret; only its owner messages a task holding them; a hand-over ends the previous
  owner's secrets at once; switching on respects the 200-secret limit. A mutation of the owner
  rule (`Holder::may_hold` returning true for personal secrets) turned 3 of the first 5 red.
- Store tests (`yulius-store` `vault::tests`): migration from version 37 (team-wide agent secret
  switched on for live sandboxes only, project and team secrets untouched), names unique per
  scope, switches, insert-and-switch in one transaction, cascades. App unit tests
  (`vault::reach`): precedence, shadowing, owner-only, automation holder.
- e2e (`npx e2e run tests/secrets.e2e.ts tests/quality.e2e.ts tests/phone.e2e.ts`): 45 passed;
  also `cd`, `responsive` suites passed (67). One full run of `secrets.e2e.ts` failed "a failed
  copy shows no check…" on axe target-size of a Close button; it passed alone 3/3 and on the next
  full run: the known flake #640, outside this change.
- `scripts/dev security` (Vidoc vidoc-1): exit 0, no critical, high or in-change medium finding.
  Five medium/low findings in lines this branch did not change are left to CI's issue filing
  (`ci/exec.rs:219`, `vault/verify.rs:186`, `layout.js:149`, `ci/mod.rs:1255`,
  `routes/secrets.rs:226` — the last is already clamped to 500 in `vault_activity`).
- Not tested: a real browser against a running server (the UI ran against the mock), a real CI
  job's run with a personal-secret grant beyond the in-process proxy client, and an upgrade of a
  live instance's database (the migration test runs on a synthetic version-37 file).
- Size: about 1400 changed lines outside tests and docs, above the 400-line guideline, kept as one
  change because the resolution of every call site (proxy, run environment, CI, CD, MCP, tools,
  redaction) must change together: a partial merge would either leak personal secrets or ship an
  API without its page.
