---
yulius-task: cbb818cd-cebd-4a67-8f92-ad8f0dc73341
status: in-progress
---

# SPEC-308: A production secret's Edit dialog offers no Hosts the server refuses (#622)

Status: in-progress
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
- R2. The Edit dialog of a production secret that still has hosts (from before it became one)
  shows them as text with one choice, "Remove them", and says why; checked, Save sends
  `hosts: []` with `expect_hosts` (the hosts shown, SPEC-216); unchecked, it sends no hosts.
- R3. The Edit dialog of any other secret keeps its Hosts input unchanged.
- R4. The mock's `PATCH /api/agent-secrets/{id}` answers 409 `conflict` with the server's
  `PRODUCTION_SECRET` message for non-empty `hosts` on a production secret, before its
  `expect_hosts` check, as `update_agent_secret` does; an empty list passes.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Spec for a production secret's Hosts in the Edit dialog
- [ ] Regression e2e test fails against the dialog and the mock
- [ ] Mock PATCH refuses hosts on a production secret
- [ ] Edit dialog says why a production secret has no hosts and only clears old ones
- [ ] Changes entry and SPEC-204 open item closed
- [ ] Independent review answered and gates green

## Acceptance criteria

- AC1 (R1, R3). Editing `PROD_API_TOKEN` shows no Hosts textbox and the note; editing
  `STRIPE_KEY` still has its Hosts textbox.
- AC2 (R2). With `PROD_API_TOKEN` carrying an old host, its dialog shows it, offers "Remove
  them", and Save with it checked leaves the secret sent nowhere.
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
  "Remove them" choice (rather than dropping the field) keeps clearing old hosts possible, which
  the server allows. No ADR: this follows SPEC-204's rule that the page offers neither side of a
  production secret where the server refuses.
- The mock's refusal is exercised through the page with a dialog opened before the secret became
  a production secret, a real race the server also refuses (`update_refused`), rather than with
  a request the page can no longer send.
