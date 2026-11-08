---
yulius-task: 73dd94db-e49b-4efb-8a86-41f06c0007e6
status: in-progress
---

# SPEC-440: Agents ask for a copy of a production secret

Status: in progress
Date: 2026-11-08
Owner: Yulius task #422
Scope authorization: task #422 ("agent should be able to request copying the token"), with a
screenshot of a task's **Needs you**: "Yulius refuses agents N3_DEV_CI_TOKEN ('a production secret:
add a separate secret for agents'). To do: Add agent secret N3_DEV_CI_TOKEN_AGENT (same value as
N3_DEV_CI_TOKEN), bound to host 165-245-200-246.sslip.io, on the Secrets page". Amends SPEC-134
(production secrets are exclusive) and SPEC-121 (agents ask, people allow); ADR-0284.

## Goal

An agent that needs the value of a production secret for agent work (a dev CI token that only
exists as a production secret) asks Yulius for a **copy** of it as a new agent secret, with the name
and hosts it needs. A person allows the copy on the Secrets page with one click (after their
two-factor code); Yulius creates the agent secret from the stored value. Nobody reveals, copies or
pastes the token by hand, and the production secret stays exclusive to production deploys.

## Context

- A production secret (`cd_prod`, SPEC-134 round 2 M-B, ADR-0109) is never used by agents:
  `secret_request_hosts`, `secret_copy_to_env` and `browser_fill_secret` refuse it with
  `PRODUCTION_SECRET` ("… Add a separate secret for agents"). The only way today is the screenshot's:
  the agent raises `needs_you`, a person reveals the production value (two-factor), adds a new agent
  secret, pastes the value and binds the hosts.
- Agents already ask people for things on secrets and wait: hosts (`secret_request_hosts`,
  SPEC-121, ADR-0098), `.env` copies (SPEC-129). A waiting request flags the task as needing a person
  (`flag_secret_wait`) and an answer lets it work on (`settle_secret_waits`, SPEC-213).
- Agents create agent secrets from a file (`secret_create_from_file`, SPEC-211, ADR-0167):
  `insert_agent_secret` stores a new project secret, audited `vault.agent.create` with an origin.
- Revealing a value needs the person's fresh two-factor unlock (`fresh_or_status`); API tokens
  cannot.

## Requirements

- **CPY-001 An agent asks for a copy.** MCP tool `secret_request_copy {secret, name, hosts,
  reason?}`, for task agents and the Superagent: `secret` names a production secret that applies to
  the project (`resolved_agent_secrets`), `name` a valid new NAME, `hosts` 1-16 hosts
  (`parse_hosts`). It stores a copy request for (secret, the caller's sandbox) and never a value or a
  binding. Refused: a secret that is not a production secret (`Invalid`, pointing to
  `secret_request_hosts` / `secret_copy_to_env`), an unknown secret, a name already applying to the
  project (`Conflict`, as `secret_create_from_file`), the reserved `VIDOC_API_KEY`, no hosts.
- **CPY-002 One request per secret and sandbox.** Asking again with the same name and hosts adds the
  asker (`#3, #7`); asking with another name or hosts replaces the request, asked by the new asker
  only. The result says what waits and gives the Secrets page link to print; the request is audited
  `vault.agent.copy_request` (`SOURCE → NAME on hosts (reason)`).
- **CPY-003 The task waits for a person.** A stored request flags the task as needing a person, as a
  host request does; allowing or dismissing it (or deleting the secret) lets the task work on.
- **CPY-004 A person allows or dismisses it.** `POST /api/sandboxes/{sid}/agent-secrets/{id}/copy
  {allow, name, hosts}`: `name` and `hosts` are what the person saw (409 unless they are exactly what
  waits). Needs manage rights on the production secret **and** on the sandbox's project secrets
  (write access). Allowing needs a fresh two-factor unlock (`reauth_required` otherwise, API tokens
  refused, as a reveal); it creates a project secret of the sandbox named `name`, with the production
  secret's value, user name and injection, bound to `hosts`, not set up by agents (`auto_allow`
  off), no `.env` allowance, described "Copy of SOURCE for agents (asked by …)", audited
  `vault.agent.create` (origin "copy of SOURCE, allowed by <login>") and
  `vault.agent.copy_allow`; dismissing audits `vault.agent.copy_dismiss`. The request is removed in
  the same transaction that adds the copy; a name taken meanwhile is a 409 and the request stays.
- **CPY-005 The Secrets page shows it.** A production secret with a copy request for the open
  sandbox shows "Task #N asks for a copy for agents: NAME, sent to HOSTS" (and the reason) with
  **Allow copy** (only for people who may answer) and **Dismiss**; Allow runs the two-factor unlock
  when needed. The list updates live (`SecretsUpdated`).
- **CPY-006 Agents learn the way.** The production-secret refusal agents get names
  `secret_request_copy`; `secrets_list` and the tool descriptions say it.
- **CPY-007 Nothing else changes for production.** The production secret keeps no hosts and no
  `.env` allowance; the copy is a separate secret (rotating one does not rotate the other); deleting
  the production secret or the sandbox removes its requests.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Copy requests are stored per production secret and sandbox
- [x] Agents and the Superagent ask for a copy with secret_request_copy and wait for a person
- [x] A person allows or dismisses a copy through the API, creating the agent secret
- [x] The Secrets page shows a copy request with Allow copy and Dismiss
- [x] ADR, architecture, API and product docs describe copy requests
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- A task asking `secret_request_copy` for a production secret gets "waiting" and the link, its task
  shows as needing a person, and the source secret's row is unchanged (no hosts, no `.env`).
- Allowing it (unlocked) creates `NAME` bound to the hosts, whose proxied value equals the
  production secret's; the request is gone and the task works on; without an unlock the answer is
  `reauth_required` and nothing is created.
- Answers naming another name or other hosts than wait are 409; dismissing removes the request and
  creates nothing.
- A non-production secret, an existing name or an unknown secret are refused with messages that say
  what to do instead.
- The Secrets page (mock) shows the request on the production secret and Allow copy creates the
  copy.

## Test plan

- Store unit tests (`crates/yulius-store/src/vault.rs`): put/merge/replace a request, answer only as
  seen, the copy inserted with the request removed in one transaction, a taken name keeps the
  request, cascade on secret delete.
- Integration tests (`crates/yulius/tests/secrets_copy.rs`): the tool's refusals and waiting result,
  task flag and settle, allow without unlock (reauth), allow with unlock (copy's metadata, hosts,
  value through `agent_secret_value`-style resolution), dismiss, mismatched answers, permission
  (reader 403), the Superagent asks too.
- MCP tool lists (`cockpit.rs`) include `secret_request_copy`.
- e2e (`e2e/tests/secrets*.spec.*`, mock): the request shows and Allow copy creates the secret.

## Decisions

- **A request answered by a person, not an agent-made copy.** An agent copying a production value
  into an agent secret on its own would undo SPEC-134's exclusivity; the person's Allow is the same
  decision they make today by hand, without the value passing through a screen or clipboard.
- **Two-factor for Allow.** Making a production value usable by agents is as sensitive as revealing
  it (the manual path needs a reveal), so it takes the same fresh unlock.
- **Hosts allowed with the copy.** The person sees the name and hosts together; a copy with no hosts
  would need a second answer for nothing. `auto_allow` stays off so further hosts are asked for.
- **Own table, keyed by (secret, sandbox).** A global production secret may be asked for by several
  projects; a column on `vault_secrets` would hold one. Rows cascade with the secret and the
  sandbox.
- **Production secrets only.** Other secrets get hosts or `.env` copies directly; copying them would
  just duplicate values.
- **Rejected:** letting agents read production values for dev use (breaks ADR-0109); a copy that
  follows the source's rotations (a link between secrets nothing else has; a rotation of the copy is
  a person's, as for any agent secret).
