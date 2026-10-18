---
yulius-task: 338ed358-62f1-41df-a050-fdb829a22367
status: done
---

# SPEC-216: Agent secrets set themselves up by default; secrets belong to a sandbox unless team-wide

Status: done
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
   person picks team-wide. The team secrets that exist today apply to every sandbox; each becomes
   a secret of every sandbox (agent secrets already were a sandbox's, unless an administrator made
   them team-wide).

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
  confirm). A secret an agent creates (`secret_create_from_file`, SPEC-211) has it off: whether
  agents set a secret up is a person's choice.
- SSA-002. With `auto_allow`, `secret_request_hosts` binds the hosts it asks for at once (same
  parsing, at most 16 hosts in all), audits `vault.agent.auto_allow` with the asker and the
  reason, does not mark the task as needing a person, and answers that they are bound (`bound`).
  A host it asks for that already waited for a person is bound too; other waiting hosts stay.
  Once nothing a task asked for waits, it no longer needs a person (SPEC-213).
- SSA-003. With `auto_allow`, production deploys get the secret (as `cd_prod` would) without a
  person allowing it. Dev deploys, `.env` copies, CI and the Superagent are unchanged.
- SSA-004. `auto_allow` and the production-only allowance (`cd_prod`) exclude each other: turning
  `auto_allow` on for a production secret is refused, and so is allowing `cd_prod` for a secret
  with `auto_allow` (production already gets it).
- SSA-005. People who manage a secret turn `auto_allow` on or off in Edit (`PATCH auto_allow`),
  audited `vault.agent.auto_allow_on` / `vault.agent.auto_allow_off`. Turning it off keeps the
  hosts already bound (Edit removes them) and stops production deploys getting it. An edit read
  before a host was bound or the switch changed is refused (409, reload), never undoing it: the
  Edit dialog sends hosts only when changed, with `expect_hosts`, the hosts it showed.
- SSA-006. The Secrets page marks such a secret "Agents set it up", and its production line says
  that production deploys get it because agents set it up (no Allow/Revoke button there).
  `secrets_list` gives `auto_allow` per secret and its guidance says such hosts bind at once; the
  `secret_request_hosts` description says the same. A binding is announced to open Secrets pages
  (`secrets.updated`, SPEC-209).

### Per sandbox or team-wide (SCO)

- SCO-001. A team secret has a scope: `sandbox` (one sandbox) or `instance` (team-wide).
  `GET /api/sandboxes/{sid}/team-secrets` lists the sandbox's and the team-wide ones;
  `POST /api/sandboxes/{sid}/team-secrets` creates one, `scope` defaulting to `sandbox`.
  `GET /api/team-secrets` lists the team-wide ones and `POST /api/team-secrets` creates a
  team-wide one, as before. Each view carries `scope`.
- SCO-002. A sandbox's team secret is listed and revealed only by people who can view that
  sandbox (others get 404, as for an invisible sandbox), and added, changed and deleted only by
  those who write to it (403 for readers). Its activity entries are shown only to the people who
  can view it. Team-wide ones stay as today (every member). Entries of a secret that no longer
  exists (deleted, or replaced by its copies in SCO-004) are shown to instance administrators
  only, as for agent secrets.
- SCO-003. On the Secrets page both tabs work in the open sandbox. The Add dialogs offer Scope:
  "This sandbox" (default) or "Team-wide (every sandbox)". For agent secrets the team-wide choice
  stays limited to instance administrators. The cards badge "This sandbox" or "Team-wide".
  Without an open sandbox the Team tab shows and adds team-wide secrets.
- SCO-004. Existing team secrets are assigned to sandboxes once, at the first start after the
  update: every team secret (all had no sandbox) is copied into each sandbox that exists
  (re-sealed under the copy's id, every other column kept), and the original is removed, in one
  transaction per secret. Each copy is audited `vault.team.assign` by `yulius`. Secrets created later as team-wide are never assigned. With no
  sandbox, a secret stays team-wide. A failure is logged and retried at the next start. Agent
  secrets were already a sandbox's by default; those an instance administrator made team-wide
  stay team-wide (review round 3): copies would let every sandbox's writers reveal, rebind and
  hand them to agents, which only administrators could.
- SCO-005. `yulius secrets import --to team` imports into a sandbox (`--sandbox` or the
  checkout's remote) by default, and `--instance` imports team-wide, as for agent secrets.
  `--to agent` says before `yes` that agents set the secrets up, and `--confirm-hosts` turns that
  off (SSA-001 for the CLI). Fields left at their default are not sent, so an older server still
  takes a team-wide or an agent import (a sandbox's team secrets need a server with SPEC-216).

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Agents bind the hosts of a self-set-up agent secret, opt-out at creation and in Edit
- [x] Production deploys get a self-set-up agent secret without a person's allowance
- [x] Team secrets belong to a sandbox or are team-wide, with sandbox access
- [x] Existing team-wide secrets are assigned to every sandbox once
- [x] The Secrets page offers the option and the scopes, the mock and e2e follow
- [x] The CLI imports team secrets into a sandbox or team-wide
- [x] Review round 1 findings fixed test-first, dev merged
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

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
- AC-6. After the assignment, a team secret that existed before is one secret per sandbox, each
  revealing the original value; the original is gone; an instance-wide agent secret is unchanged;
  a second run changes nothing.
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
- **Team-wide agent secrets are not copied** (review round 3). Only instance administrators
  manage an agent secret of every sandbox; a sandbox's copy is managed by its writers, so the
  copy would widen who reveals it and who lets agents bind it. They already are the "team-wide"
  half of the split; team secrets, which had no scope at all, are the ones assigned.
- **Copy into every sandbox rather than pick one.** Nothing records which sandbox a team secret is
  for; a copy per sandbox keeps every sandbox working as before and makes each per sandbox, as
  asked. The cost (a rotation per copy) is said on the page through the scope badge; people
  delete copies they do not need.
- **A pending list fixed by the migration** (`vault_scope_pending`), not "every row without a
  sandbox": team-wide secrets created after the update must stay team-wide, and a crash midway
  resumes where it stopped.
- **Sandbox team secrets: read with view access, change with write access.** The first version
  let anyone who could view the sandbox change them, as any member changes team-wide ones; the
  Vidoc scan flagged it (high) and a reader replacing a credential others use is a real risk, so
  changes follow the agent secrets' rule. Team-wide ones keep their membership rule (SPEC-040).
- **Self-set-up stays the CLI default too** (review M1), said in the list before `yes`, with
  `--confirm-hosts` to turn it off: the request asks for one default everywhere, with the choice
  at creation.

## Evidence

- **Tests that failed first.** Each new test was run before its change and failed for the
  intended reason. `agents_bind_the_hosts_of_a_secret_they_set_up` failed because `auto_allow`
  was null. `production_gets_a_secret_agents_set_up` failed with state `not_allowed`.
  `team_secrets_belong_to_a_sandbox_unless_team_wide` got 404 (no route). The e2e test failed
  because there was no "Let agents set it up" switch. The review round 2 stale-hosts PATCH got
  422 (unknown `expect_hosts`).
- **Tests that did not compile before their change.** `existing_secrets_are_assigned_to_every_sandbox_once`
  needed `assign_vault_scopes`. The store guard test needed `vault_update(record, seen)`. The CLI
  `--confirm-hosts` test needed the new target field.
- **Existing tests that relied on a person allowing hosts** now opt out with `auto_allow: false`:
  `cd.rs`, `cockpit.rs`, `local_ci.rs` and `secrets.rs`, including SPEC-209's tests merged from
  dev.
- **`scripts/dev ci`**, exit 0:
  - at 1d1c2624: 2138 tests;
  - at a6ced0ff (review round 1 fixes): 2142 tests;
  - at cc98e45a (review round 2 fix): 2142 tests;
  - at afa1579f, after merging dev #644 (bootstrap rerun first): 2144 tests passed, 18 skipped,
    then docs.
  - at 279a1fbf (review round 3 fix, dev #641 and #648 merged, bootstrap rerun): 2153 tests
    passed, 18 skipped, then docs.
- **`cd e2e && npx e2e run`:**
  - secrets, cd and phone suites, 35/35 at a6ced0ff;
  - secrets suite, 20/20 at cc98e45a.
- **`scripts/dev security` (Vidoc vidoc-1).** At 1d1c2624 it failed on one high finding: a
  sandbox's team secret could be changed with view access. That is fixed in a6ced0ff (write
  access is required). At a6ced0ff it exits 0 with no open critical or high finding. Its medium
  and low findings are in code this change does not touch:
  - `runtime.rs` egress cache and header building;
  - the two-factor reset route and the internal routes;
  - the `otpauth` link.
- **Review, round 1** (fresh session, 1d1c2624): request changes. Own-code findings, all fixed:
  - M1: the CLI now says that agents set the secrets up, and takes `--confirm-hosts`.
  - M2: edits compare the hosts and `auto_allow` they read.
  - m1/m2: store guard tests, and the waiting-host, `auto_allow_on` and removed-sandbox tests.
  - m3: SCO-002 reconciled.
  - m4: `docs/api.md` updated.
  - m5: absent fields are not serialized.
  - nits: assignment audits are best-effort; the update detail wording.
  - Integration: `auto_bind` announces `secrets.updated`.
  - Not changed: a sandbox's team import treats a team-wide title as new, which is correct, since
    the scopes differ.
- **Review, round 2** (fresh session, 1c0290f8): request changes on one major: an open Edit
  dialog still dropped a host bound since it opened. Fixed test-first in cc98e45a: hosts are sent
  only when changed, with `expect_hosts`, and the server answers 409. Also fixed:
  - the empty-state Add button for readers;
  - the SCO-005 wording;
  - the ADR wrap.

  Left as is: a sandbox reader adds team-wide team secrets from `/secrets`, not from the sandbox
  page. That is consistent with the scope default; a nit.
- **Review, round 3** (fresh session, 280ba635): the round 2 fix holds. One major, own code:
  copying "every sandbox" agent secrets into each sandbox would let every sandbox writer reveal,
  rebind and hand to agents a key only instance administrators managed. Fixed test-first: the
  upgrade test failed with `["team", "wide"]`; schema 35 now lists team secrets only, and the
  app test checks that the agent secret stays team-wide. Also fixed:
  - m1: `changes.md` says earlier team-secret activity goes to administrators;
  - n2: the mock compares `expect_hosts` as a set;
  - n3: the ADR index order.

  Not changed: n1, the CLI's note against a server without SPEC-216 (stricter, the safe
  direction).
- **Review, round 4** (fresh session, 3eafb8e4): approve, no blocker or major. Its minors and
  nits are own code, so they were fixed rather than filed:
  - Minor 1: outside a checkout, `--to team` now names `--instance` beside `--sandbox`.
    Test-first: the new CLI test failed on the old message.
  - Minor 2: changing the hosts of a secret agents set up now needs `expect_hosts` (409 without
    it). Test-first: the PATCH answered 200 and dropped the bound host.
  - Nits: the `vault.agent.assign` leftovers are gone; the Goal and the upgrade test name say
    "team secrets"; the mock checks view access for a sandbox's team secret.
- **Review, round 5** (fresh session, 00d42d0a): approve, no blocker, major or minor. Its nits
  are in the test-only mock (it does not apply the `expect_hosts` rule, and answers 403 where the
  server answers 404), left after five rounds.
- **Merge of dev #642** (`secret_create_from_file`, SPEC-211): the shared `insert_agent_secret`
  now takes `auto_allow`. A person's create passes the API default; an agent's create passes
  `false`. `secrets_create.rs` asserts that, and the secrets, cd, cockpit, local CI, CLI and
  manage-tools suites passed: 140 tests, plus 515 in the import, store and domain crates.
- **Merge of dev #646** (SPEC-213, a task stops needing a person once nothing it asked for
  waits): binding at once a host that waited now settles the waits as well. Test-first: the task
  stayed in Needs attention after its last waiting host was bound.
- **What could not be tested here:** the assignment against this instance's real vault and
  sandboxes (it runs at the first start after the update), and the live Secrets page against a
  running server (the page was tested against the mock).

