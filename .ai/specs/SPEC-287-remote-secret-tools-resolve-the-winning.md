---
yulius-task: 609d3144-d973-40d9-9865-368e586b573b
status: in-progress
---

# SPEC-287: Remote secret tools resolve the winning secret by name

Status: in-progress
Date: 2026-10-24
Owner: task #273
Scope authorization: task #273 (requested by MrFishPL): solve issue #787 in one pull request into
`dev`. Applies ADR-0186 (agent secrets are project, global or personal) and SPEC-245's resolution
to the remote MCP of SPEC-247 (RMT-027); no new decision record.

## Goal

The remote MCP's `secret_update` and `secret_hosts_answer` (and the CLI commands built on them)
act on the secret `secrets_list` showed the person for that name: the one that wins for this
project and this person, project > the person's personal > a global switched on for the project.

## Context

- Symptom (#787): with a global `NPM_TOKEN` created before the project's own `NPM_TOKEN`,
  `secret_update {secret: "NPM_TOKEN"}` from a person's laptop agent acts on the global one: a
  project writer who is not an instance administrator gets 403 (managing a global secret), an
  administrator silently rebinds the global secret's hosts.
- Root cause: `remote.rs` looks the name up in `App::agent_secrets` (the Secrets page's list) and
  takes the first match. That list holds the project's secrets, every global secret (switched on
  for the project or not) and the caller's personal ones (switched on or not), ordered
  `name, created_at` by `vault_agent_all`, so the oldest secret of the name wins.
  `secrets_list` instead lists `applied_agent_secrets` → `resolved_agent_secrets` with the
  caller as holder (`reach::resolve`: one per name by precedence, switched-on only, another
  person's personal secret never).
- Why the tests missed it: `secrets_are_managed_by_name_and_binding_only` has one secret per
  name; no remote test had a name in two scopes.
- Already holds: another person's personal secret is never reached (`agent_secrets` keeps only
  the caller's own); the fix keeps that (`Holder::may_hold`).
- `secret_delete` is not affected: it picks the project's own secret of the name and refuses the
  others by scope (SPEC-274 PLW-004).

## Requirements

- RSR-001: `secret_update` and `secret_hosts_answer` look `secret` up among
  `resolved_agent_secrets(project, Holder::Person(caller))` and act on that record's id;
  `update_agent_secret` / `answer_host_request` still check the person may manage it.
- RSR-002: a name that resolves to nothing (no such secret, only a global or personal secret not
  switched on for the project, only another person's personal secret) is the existing error
  "the project has no agent secret NAME".
- RSR-003: secrets of the name that lose (shadowed) are never changed by these tools.

## Plan

- [x] Spec SPEC-287 written and validated
- [ ] Regression tests for name resolution fail on dev
- [ ] Remote secret tools resolve the winning secret
- [ ] Docs and changes entry true, gates green
- [ ] Independent review answered
- [ ] Merged into dev on green CI

## Acceptance criteria

- A global `NPM_TOKEN` switched on and created first, then the project's `NPM_TOKEN`: a project
  writer who is not an administrator updates the hosts with remote `secret_update`; the project's
  secret changes, the global one does not.
- The same for `secret_hosts_answer` on a host request of the project's secret.
- The caller's personal secret switched on for the project and shadowing a global one is the one
  `secret_update` changes; another person's personal secret of the name is never reached.
- A global secret not switched on for the project is not reachable by name: "no agent secret".

## Test plan

- `crates/yulius/tests/remote_mcp.rs`: `remote_secret_tools_act_on_the_secret_that_wins`, written
  first and shown to fail on `dev`, covering the four acceptance criteria through `/mcp` with an
  operate token, checking the stored records with `vault_get`.
- The existing `secrets_are_managed_by_name_and_binding_only` and the `secrets_scopes` suite keep
  passing.

## Decisions

- Resolve with `resolved_agent_secrets`, not `applied_agent_secrets`: the instance Vidoc key
  `applied` adds is virtual (no record to update), and `update_agent_secret` would answer
  not-found for it. A `VIDOC_API_KEY` name without a stored secret is "no agent secret".
- A personal secret of the caller not switched on for the project is no longer reachable by name
  through these tools (it was before, as the Secrets page lists it). This matches `secrets_list`,
  which does not show it either; the person manages it on the Secrets page. Reversible.
- No ADR: the change applies ADR-0186's precedence to two tools that bypassed it.
