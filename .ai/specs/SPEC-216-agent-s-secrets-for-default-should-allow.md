---
yulius-task: 338ed358-62f1-41df-a050-fdb829a22367
status: draft
---

# SPEC-216: Agent secrets set themselves up by default; secrets belong to a sandbox unless team-wide

Status: draft
Date: 2026-10-17
Owner: Yulius task #203
Scope authorization: task #203, requested by MrFishPL: "Agent's secrets for default should allow
the agent to set the url by itself and prod permission without confirming it by the user, but
while creating the secret, user can disable the option. Also, team and agent's secrets should be
split to per sandbox and teamwise. By default they should be per specific sandbox and currect
secrets should get assigned this way".

## Goal

Two changes to the vault, one request:

1. **Agents set a new agent secret up themselves.** By default an agent binds the hosts it works
   out for a secret (no person allows them), and production deploys get the secret (no person
   allows that either). The person who adds the secret can turn this off in the Add dialog (and
   later in Edit); it then behaves as today: a person allows each host and the production
   allowance.
2. **Every secret belongs to one sandbox unless it is team-wide.** Team secrets gain the scope
   agent secrets already have. A new team or agent secret belongs to the open sandbox unless the
   person picks team-wide. The secrets that exist today apply to every sandbox; each becomes a
   secret of every sandbox it applied to.

## Context

- **Agent secret hosts** (SPEC-121, ADR-0098): a secret starts bound to nothing; an agent calls
  `secret_request_hosts`, the hosts wait in `requested_hosts`, the task needs a person, and a
  person who manages the secret allows them (`answer_host_request`). `crates/yulius-app/src/vault/requests.rs`.
- **Production deploys** (SPEC-134, ADR-0109, SPEC-204): a production run gets a secret its
  `[cd.prod]` names only when `cd_prod` is set, which a person allows on the Secrets page
  (`answer_cd_prod`). `cd_prod` is exclusive: it is refused while agents can use the secret, and
  then agents, the Superagent, CI, `.env` copies and dev deploys never get it (`PRODUCTION_SECRET`,
  `EgressClient::may_use`, `allowed_for` in `vault/env_files.rs`). `[cd.prod]` is read only at
  the production branch's head, which only a person moves (ADR-0110, ADR-0114).
- **Scopes today.** `vault_secrets.sandbox_id` exists for both sections. Agent secrets use it:
  `scope` `sandbox` (the API default) or `instance` (every sandbox, instance administrators only;
  the Add dialog offers it to them under More options). Team secrets are always `NULL`: every
  member of the instance sees and changes every team secret (`vault/team.rs`, `/api/team-secrets`).
- **AAD.** A row's payload is sealed with its id and section as associated data, so a copy under
  a new id is re-sealed by the application, not copied in SQL.
- **CLI import** (SPEC-205): `yulius secrets import --to team` posts to `/api/team-secrets`;
  `--to agent` resolves a sandbox (`--sandbox` or the checkout's remote) and takes `--instance`.

## Requirements

### Agents set a secret up (SSA)

- SSA-001. An agent secret has `auto_allow`. A new one has it on unless its creator turns it off:
  the Add dialog has the option "Let agents set it up", checked; the API field `auto_allow`
  defaults to `true` when omitted. Secrets that exist keep it off (their people chose to
  confirm).
- SSA-002. With `auto_allow`, `secret_request_hosts` binds the hosts it asks for at once (same
  parsing, at most 16 hosts in all), audits `vault.agent.auto_allow` with the asker and the
  reason, does not mark the task as needing a person, and answers that they are bound (`bound`).
  A host it asks for that already waited for a person is bound too; other waiting hosts stay.
- SSA-003. With `auto_allow`, production deploys get the secret (as `cd_prod` would) without a
  person allowing it. Dev deploys, `.env` copies, CI and the Superagent are unchanged.
- SSA-004. `auto_allow` and the production-only allowance (`cd_prod`) exclude each other: turning
  `auto_allow` on for a production secret is refused, and so is allowing `cd_prod` for a secret
  with `auto_allow` (production already gets it).
- SSA-005. People who manage a secret turn `auto_allow` on or off in Edit (`PATCH auto_allow`),
  audited `vault.agent.auto_allow_on` / `vault.agent.auto_allow_off`. Turning it off keeps the
  hosts already bound (Edit removes them) and stops production deploys getting it.
- SSA-006. The Secrets page marks such a secret "Agents set it up", and its production line says
  that production deploys get it because agents set it up (no Allow/Revoke button there).
  `secrets_list` gives `auto_allow` per secret and its guidance says such hosts bind at once; the
  `secret_request_hosts` description says the same.

### Per sandbox or team-wide (SCO)

- SCO-001. A team secret has a scope: `sandbox` (one sandbox) or `instance` (team-wide).
  `GET /api/sandboxes/{sid}/team-secrets` lists the sandbox's and the team-wide ones;
  `POST /api/sandboxes/{sid}/team-secrets` creates one, `scope` defaulting to `sandbox`.
  `GET /api/team-secrets` lists the team-wide ones and `POST /api/team-secrets` creates a
  team-wide one, as before. Each view carries `scope`.
- SCO-002. A sandbox's team secret is listed, revealed, changed and deleted only by people who can
  view that sandbox; others get 404, as for an invisible sandbox. Its activity entries are shown
  only to them. Team-wide ones stay as today (every member).
- SCO-003. On the Secrets page both tabs work in the open sandbox. The Add dialogs offer Scope:
  "This sandbox" (default) or "Team-wide (every sandbox)". For agent secrets the team-wide choice
  stays limited to instance administrators. The cards badge "This sandbox" or "Team-wide".
  Without an open sandbox the Team tab shows and adds team-wide secrets.
- SCO-004. Existing secrets are assigned to sandboxes once, at the first start after the update:
  every team secret and every agent secret with no sandbox at that moment is copied into each
  sandbox that exists (re-sealed under the copy's id, every other column kept), and the original
  is removed, in one transaction per secret. Each copy is audited `vault.team.assign` or
  `vault.agent.assign` by `yulius`. Secrets created later as team-wide are never assigned. With no
  sandbox, a secret stays team-wide. A failure is logged and retried at the next start.
- SCO-005. `yulius secrets import --to team` imports into a sandbox (`--sandbox` or the
  checkout's remote) by default, and `--instance` imports team-wide, as for agent secrets.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Agents bind the hosts of a self-set-up agent secret, opt-out at creation and in Edit
- [x] Production deploys get a self-set-up agent secret without a person's allowance
- [ ] Team secrets belong to a sandbox or are team-wide, with sandbox access
- [ ] Existing team-wide secrets are assigned to every sandbox once
- [ ] The Secrets page offers the option and the scopes, the mock and e2e follow
- [ ] The CLI imports team secrets into a sandbox or team-wide
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-1. A secret added with no `auto_allow` field: an agent's `secret_request_hosts` binds the
  host, the egress proxy intercepts it at once, the task does not need a person, and the audit
  has `vault.agent.auto_allow`.
- AC-2. A secret added with `auto_allow: false` keeps today's flow (waits, a person allows).
- AC-3. A production CD run gets a self-set-up secret its `[cd.prod]` names; with the option off
  it is refused until a person allows it.
- AC-4. `cd_prod` cannot be allowed while `auto_allow` is on, and `auto_allow` cannot be turned on
  for a `cd_prod` secret.
- AC-5. A team secret created in sandbox s1 is listed in s1 with `scope: sandbox`, not in s2 nor
  in `/api/team-secrets`; a person who cannot view s1 gets 404 on it; a team-wide one is listed
  everywhere.
- AC-6. After the assignment, a team secret and an instance-wide agent secret that existed before
  are one secret per sandbox, each revealing the original value; the originals are gone; a
  second run changes nothing.
- AC-7. On the Secrets page, the Add agent secret dialog has "Let agents set it up" checked, and
  both Add dialogs default to "This sandbox".

## Test plan

- `crates/yulius/tests/secrets.rs`: AC-1, AC-2, AC-4, AC-5 and the activity filter (SCO-002) over
  the API and the MCP tools; existing tests that rely on a person allowing hosts send
  `auto_allow: false`.
- `crates/yulius/tests/cd.rs`: AC-3 (production values with and without the option).
- Store unit test (`crates/yulius-store/src/vault.rs`): the assignment transaction replaces the
  original by its copies and refuses a stale original. App test for AC-6 in `secrets.rs`.
- `crates/yulius-import` tests: team import posts to the sandbox route by default and to
  `/api/team-secrets` with `--instance`.
- `e2e/tests/secrets.e2e.ts`: AC-7, the scope badges and the "Agents set it up" production line,
  against the mock.
- Gates: `scripts/dev ci`, the e2e suite, `scripts/dev security`.

## Decisions

- **One option covers hosts and production** (ADR-0171). The request names "the option": one
  switch, "Let agents set it up", reads as one decision about trusting agents with this secret.
  Two switches would let a person build combinations nobody asked for.
- **Default on for new secrets only.** Existing secrets were added under the promise that a person
  confirms; widening what they reach without a person is not what "for default" asks, and is not
  reversible once a value has left. People turn it on in Edit.
- **Production stays exclusive for `cd_prod`.** A person who wants a production-only credential
  turns the option off and allows production as today (ADR-0109 unchanged for those). A
  self-set-up secret is reachable by agents and production alike: the person chose that at
  creation; `[cd.prod]` is still read only from the branch a person promotes.
- **Copy into every sandbox rather than pick one.** Nothing records which sandbox a team secret is
  for; a copy per sandbox keeps every sandbox working as before and makes each per sandbox, as
  asked. The cost (a rotation per copy) is said on the page through the scope badge; people
  delete copies they do not need.
- **A pending list fixed by the migration** (`vault_scope_pending`), not "every row without a
  sandbox": team-wide secrets created after the update must stay team-wide, and a crash midway
  resumes where it stopped.
- **Sandbox team secrets need only view access**, like team secrets today need only membership:
  stricter rules for people's own shared passwords would be a separate change.
