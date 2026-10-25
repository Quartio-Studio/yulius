---
yulius-task: cbb818cd-cebd-4a67-8f92-ad8f0dc73341
status: done
---

# SPEC-308: A production secret's Edit dialog offers no Hosts the server refuses (#622)

Status: done
Date: 2026-10-25
Owner: Yulius task #294
Scope authorization: task #294: "Solve GitHub issue #622 […] in one pull request whose description
says `Fixes #622`." The server needs no change.

## Goal

The Edit dialog of a production secret (`cd_prod`, SPEC-134) no longer lets a person enter hosts
the server refuses, and says why. The web mock refuses those hosts as the server does, so the e2e
suite sees the refusal.

## Context

- Symptom (#622): on Secrets → agent secrets, a person edits a production secret, adds a host,
  presses Save and gets a 409 toast ("this is a production secret: …").
- Code path: `agentForm` (`crates/yulius-server/web/assets/pages/secrets.js`) adds the Hosts input
  for every edited secret; its production check only hides "Let agents set it up". `form.values()`
  sends the edited hosts. `update_agent_secret` (`crates/yulius-app/src/vault/agents.rs`) refuses
  non-empty hosts on a `cd_prod` secret with `Conflict(PRODUCTION_SECRET)`, and so does
  `update_refused` when the secret became one meanwhile. An empty list passes: an older
  production secret may still carry hosts and can be cleared (unit test in `agents.rs`).
- Why the tests missed it: the mock's `PATCH /api/agent-secrets/{id}` refuses only
  `auto_allow: true` on a production secret, never `hosts`, and no e2e test edits one.
- SPEC-204 (task #191) listed it as left open.

## Requirements

- R1. The Edit dialog of a production secret without hosts has no Hosts input. In its place,
  under the label Hosts, it says: "None. This is a production secret: agents never get it, so it
  is sent to no host. Add a separate secret for agents to send to a host."
- R2. The Edit dialog of a production secret that still has hosts (kept from before production
  secrets became exclusive) shows them as text with one choice, "Remove these hosts", and says
  that agents' requests to them still get it (the egress proxy lets agent runs use a secret on
  its bound hosts whatever `cd_prod` is, `EgressClient::may_use`); checked, Save sends
  `hosts: []` with `expect_hosts` (the hosts shown, SPEC-216); unchecked, it sends no hosts.
- R3. The Edit dialog of any other secret keeps its Hosts input unchanged.
- R4. The mock's `PATCH /api/agent-secrets/{id}` answers 409 `conflict` with the server's
  `PRODUCTION_SECRET` message for non-empty `hosts` on a production secret, before its
  `expect_hosts` check, as `update_agent_secret` does; an empty list passes.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Spec for a production secret's Hosts in the Edit dialog
- [x] Regression e2e test fails against the dialog and the mock
- [x] Mock PATCH refuses hosts on a production secret
- [x] Edit dialog says why a production secret has no hosts and only clears old ones
- [x] Changes entry and SPEC-204 open item closed
- [x] Independent review answered and gates green

## Acceptance criteria

- AC1 (R1, R3). Editing `PROD_API_TOKEN` shows no Hosts textbox and the note; editing
  `STRIPE_KEY` still has its Hosts textbox.
- AC2 (R2). With `PROD_API_TOKEN` carrying an old host, its dialog shows it and says agents'
  requests to it still get the secret; Save unchecked keeps it, Save with "Remove these hosts"
  checked after a host was bound meanwhile is refused (hosts changed), and checked on a fresh
  dialog it leaves the secret bound to no host.
- AC3 (R4). A secret that becomes a production secret while its Edit dialog is open (another
  person allowed it for production meanwhile): adding a host and saving shows the server's
  production-secret message and keeps the secret without hosts.

## Test plan

- e2e (`e2e/tests/secrets.e2e.ts`), one new test for AC1-AC3 against the mock; it fails before
  the change (Hosts textbox present on `PROD_API_TOKEN`, the mock accepts the host).
- Server behavior is covered by the existing `agents.rs` unit tests (refusal and clearing).

## Decisions

- Replace the input with a note rather than keep it and refuse on Save: the issue asks that the
  dialog not let a person enter what the server refuses. Keeping a read-only list with one
  "Remove these hosts" choice (rather than dropping the field) keeps clearing old hosts
  possible, which the server allows. No ADR: this follows SPEC-204's rule that the page offers neither side of a
  production secret where the server refuses.
- The mock's refusal is exercised through the page with a dialog opened before the secret became
  a production secret, a real race the server also refuses (`update_refused`), rather than with
  a request the page can no longer send.

## Evidence

- Regression first: `5b3566e6` (test only) fails at the VIDOC_API_KEY Save (no alert: the mock
  accepted the host); with the mock fix `91d68d24` it fails on the Hosts textbox shown for
  PROD_API_TOKEN; with the dialog fix it passes. The round-1 test `a3ea949f` fails before
  `81bd4374` (the old "sent to no host" hint). Mutation: dropping the checkbox's
  `aria-describedby` fails the round-2 assertion.
- `cd e2e && E2E_TELEMETRY_DISABLED=1 npx e2e run tests/secrets.e2e.ts`: 26 passed (at
  `81bd4374`; the new test alone also at `50f6103c`).
- `python3 scripts/bootstrap.py && scripts/dev ci` at `8e77b445`: exit 0, nextest 2611 passed,
  16 skipped. Later commits change only the page, the test and docs; `scripts/dev check` at
  `50f6103c`: exit 0.
- Security: `yulius security-scan` at `50f6103c` did not run (exit 2: Vidoc HTTP 429, the key has
  used its 50,000,000 tokens), which is unrun, not passed (ADR-0133, ADR-0157); the PR's CI
  `security` job is the live scan.
- Independent review (fresh sessions): round 1 requested changes on `8e77b445` (major: the hint
  said "sent to no host" beside hosts agent runs still use, `EgressClient::may_use`; minors: test
  gaps for unchecked Save and `expect_hosts`, the checkbox's name and style; nits), all fixed in
  `a3ea949f` and `81bd4374`; round 2 approved `81bd4374` with two nits, fixed in `50f6103c`; round 3
  approved `50f6103c`.
- Pre-existing, outside this change, reported on #622 (this task creates no issues): agent runs
  still use a production secret on hosts it kept (`egress.rs:81`, from `e593dc71`), which also
  makes the production card's "agents never do" untrue for such a secret; the mock checks
  `auto_allow` before `expect_hosts` and lacks the "send expect_hosts" refusal.
