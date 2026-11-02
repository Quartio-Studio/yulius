---
yulius-task: e51a6bac-83ec-4617-82b5-809f74b2b193
status: in-progress
---

# SPEC-392: Vidoc triage of the auth and token findings (#724 #725 #726 #734 #837)

Status: in-progress
Date: 2026-11-02
Owner: Yulius task #374
Scope authorization: task #374 (a person asked that each of the five issues end closed). Fixes
#725; #724, #726, #734 and #837 are dismissed as false positives.

## Goal

Each of the five Vidoc findings about sign-in, API tokens and credentials is checked against `dev`
(f416b400) and either fixed test-first or dismissed with file:line evidence and a narrow
`[[ignore]]` entry for its wording, so a repeat scan does not reopen it.

## Context

- API tokens are stored hashed (`api_tokens.token_hash`); their plaintext is shown once. A device
  grant (`device_grants`) holds the CLI's token sealed until the CLI polls it once (SPEC-247,
  ADR-0047).
- `App::device_approve` (`crates/yulius-app/src/auth.rs`) read the grant, checked only its expiry,
  inserted a new `cli` API token, then resolved the grant with `UPDATE … WHERE status = 'pending'`.
- The store runs every job on one database thread (`SqliteStore::call`), so one closure with a
  transaction is atomic against every other store call.
- `[[ignore]]` matches the path exactly and the title as a case-insensitive substring; one narrow
  entry per wording (SPEC-262, ADR-0202).

## Requirements

- VAT-001 (#725, real). Approving a device grant that is no longer pending (already approved or
  denied, by the same person or another, one after the other or at the same time) answers
  `Conflict` and leaves no new API token: the grant is claimed and its token stored in one store
  transaction (`SessionStore::approve_device_grant`), which inserts the token only when the claim
  changed a pending grant.
- VAT-002 (#724, false positive). A revoked API token never authenticates: the store's
  `api_token` lookup selects only `revoked_at IS NULL`, which its port documents; dismissed with a
  narrow entry.
- VAT-003 (#726, false positive). The setup token is 128 random bits, accepted only while no
  superadmin exists, compared as SHA-256 hex in constant time, and nothing expensive (Argon2) runs
  before it matches; dismissed with a narrow entry.
- VAT-004 (#734, false positive). `GitHubAppConfig`'s secrets never leave the server in clear:
  `Debug` prints only the id and slug, it is serialized only to be sealed, and the wiring wraps
  each secret in `yulius-github`'s `Secret` at once; dismissed with a narrow entry.
- VAT-005 (#837, false positive). The remote `cd_deploy` sends no confirmation, so the deploy
  itself refuses every production-stage environment (`may_deploy`), and an environment whose stage
  cannot be resolved is refused as not defined; the outer check only words the refusal. Dismissed
  with a narrow entry.

## Plan

- [x] Spec SPEC-392 with the triage of all five findings
- [ ] Device approval claims the grant and stores its token in one transaction (#725)
- [ ] Narrow [[ignore]] entries for the dismissed wordings (#724 #726 #734 #837)
- [ ] Gates, docs and changes entry
- [ ] Independent review answered
- [ ] Dismissed issues closed with their evidence (#724 #726 #734 #837)

## Acceptance criteria

- `http_auth::a_second_device_approval_leaves_no_token` fails on `dev` (the second approval adds a
  live `yulius CLI` token) and passes: the second approval, by the same person or another, answers
  409 and the token count stays as after the first.
- Two approvals of one grant started at the same time end with exactly one success and one new
  token, whatever their interleaving.
- `.config/vidoc.toml` has one narrow entry per dismissed wording, each with its reason.
- Each issue ends closed: #725 by the merged pull request, the others as not planned with a
  comment giving the evidence.

## Test plan

- Integration tests in `crates/yulius/tests/http_auth.rs` (HTTP device flow against the SQLite
  store): sequential second approvals (same person, another person) and two concurrent ones.
- A store unit test in `crates/yulius-store` if the module has them for device grants.
- `scripts/dev ci` (the full gate) and `scripts/dev security`.

## Decisions

- #725 is fixed rather than dismissed. The orphan token's plaintext is never handed to anyone, so
  it cannot be used, but it is a live credential row listed in the person's tokens that nobody
  asked for, and it does not even need a race: a second click on Approve makes one. Claiming and
  inserting in one transaction is the fix the finding asks for; a status check before issuing
  alone would still race, and a compensating revoke after a lost claim leaves the row when the
  revoke fails.
- #724, #726, #734 and #837 are dismissed (reasons above). For #734, wrapping the fields in the
  app's `Secret` would need `Secret` to implement `Serialize`, which would make every other
  `Secret` serializable by accident: a weaker guarantee than today's.
- No new ADR: the fix keeps ADR-0047's device grant model and only makes one step atomic.

## Evidence

Filled in at the end.
