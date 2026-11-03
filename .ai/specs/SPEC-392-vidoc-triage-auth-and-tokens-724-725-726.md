---
yulius-task: e51a6bac-83ec-4617-82b5-809f74b2b193
status: done
---

# SPEC-392: Vidoc triage of the auth and token findings (#724 #725 #726 #734 #837)

Status: done
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
- [x] Device approval claims the grant and stores its token in one transaction (#725)
- [x] Narrow [[ignore]] entries for the dismissed wordings (#724 #726 #734 #837)
- [x] Gates, docs and changes entry
- [x] Independent review answered
- [x] Dismissed issues closed with their evidence (#724 #726 #734 #837)

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

- Integration test in `crates/yulius/tests/http_auth.rs` calling `App::device_approve` against the
  SQLite store of the HTTP harness: second approvals one after the other (same person, another
  person, a denial) and two concurrent ones; the CLI's poll still gets the first token.
- Store tests in `crates/yulius-store/tests/store.rs`: `approve_device_grant` on a pending,
  approved, denied and unknown grant; revoked tokens are not found by hash (the #724 guard).
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
- The `[[ignore]]` entries are title-only, like every existing entry, although AGENTS.md mentions a
  `function`: under ADR-0202 a `function` makes an entry accept any later finding in that function
  (`authenticate_bearer`, `complete_setup`, the remote dispatch), which is broader than these
  dismissals justify.
- `resolve_device_grant` is kept as it is (now only the deny path uses it in production); narrowing
  it to a deny-only port method is a refactor of an unchanged contract, outside this fix.
- No new ADR: the fix keeps ADR-0047's device grant model and only makes one step atomic.

## Evidence

- Triage on `dev` (f416b400, rechecked at c1043d15):
  - #724: `SqliteStore::api_token` selects `… AND revoked_at IS NULL` (`yulius-store/src/sessions.rs:133`),
    its port says "(not revoked)" (`ports/store.rs:111`); `revoke_user_credentials` sets
    `revoked_at` on every token of the user (`sessions.rs:107`); `authenticate_bearer`
    (`auth.rs:431`) is its only caller. Pinned by the new `store::revoked_api_tokens_are_not_found`
    and `remote_mcp::tokens_are_made_in_the_browser_listed_and_revoked` (revoked token 401).
  - #726: 16 random bytes (`auth.rs:227`), refused once setup completed (`auth.rs:265`), SHA-256
    hex compared in constant time (`auth.rs:272`), Argon2 only after (`auth.rs:290`). Pinned by
    `http_auth::setup_requires_token_confirmation_and_happens_once`.
  - #734: redacting `Debug` (`ports/github.rs:28`), serialized only to be sealed
    (`github_admin.rs:176`), wrapped in `GhSecret` at the client (`wiring/github_client.rs:54-57`);
    no view embeds it.
  - #837: the remote call passes `confirm: None` (`remote.rs:879`); `cd_prepare` refuses an
    unresolved stage (`ci/cd.rs:955`) and runs `may_deploy` (`ci/cd.rs:958`), which needs the typed
    slug for production (`yulius-domain/src/cd.rs:377`). Pinned by `remote_mcp` (prod refused,
    `:638`), `cd::production_is_deployed_by_an_administrator_who_types_the_slug` and
    `cd::named_environments_deploy_by_their_stage` (`:1673`, no slug: 422).
  - #725: real, fixed.
- Regression test `http_auth::a_second_device_approval_leaves_no_token` seen failing before the
  fix ("a second approval issued no token", left 2, right 1) and passing after; store test
  `approving_a_device_grant_stores_its_token_only_when_pending` passes.
- `scripts/dev ci` at 66f6ca9e (after merging origin/dev c1043d15): exit 0, nextest 2966 passed,
  16 skipped (clippy, fmt, doctests, macOS compile, deps and docs included).
- `yulius security-scan`: "Security scan disabled: Vidoc is turned off in Yulius" (exit 0):
  disabled, not passed; nothing was scanned, so the entries were not checked against a live scan.
- `[[ignore]]` entries (the independent reviewer agreed to each): `auth.rs` "authenticate_bearer
  does not check token revocation" and "No rate limiting on setup token verification";
  `ports/github.rs` "GitHub App secrets stored as plain strings rather than Secret wrapper";
  `remote.rs` "Production deployment check fails open on transient errors".
- Independent review (fresh session): approved 66f6ca9e, agreeing with all four dismissals.
  Answered: title-only entries and `resolve_device_grant` kept (Decisions), Plan, Evidence and
  Test plan updated. Pre-existing, left as is and noted in the pull request: the store's grant
  claim does not re-check `expires_at` (the app checks it just before); revoking one API token
  does not bump the access epoch, so a terminal opened with it keeps write access until the next
  re-check (at most 30 s, `GRANT_REFRESH_MS`).
- #724, #726, #734 and #837 closed as not planned with their file:line evidence on `dev`; #725
  closes with the pull request (`Fixes`).
