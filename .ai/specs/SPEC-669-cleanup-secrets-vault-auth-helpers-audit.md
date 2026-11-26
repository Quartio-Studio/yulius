---
yulius-task: 8bb13626-56f5-427f-9a6d-8a2b592cd63e
status: in-progress
---

# SPEC-669: Cleanup of the secrets, vault and auth helpers (audit L4)

## Goal

Carry out findings F2, F5, F6, F7, F8 and F3 of the November 2026 L4 audit (secrets, identity, key
custody) as behaviour-preserving cleanups, one pull request each, delivered in that order. Less
code that does the same thing; sealing, injection, redaction and authorisation behave exactly as
before and their existing tests pass unchanged.

## Context

- The audit report and the cleanup charter are attachments of the task (`audit-2026-11-25/`). The
  charter asks chores for no spec; this file exists only because delivery opens no pull request
  without one, and it holds no design.
- Paths of this task: `yulius-app/src/vault/**`, `accounts.rs`, `auth.rs`, `scopes.rs`,
  `admin.rs`, `access.rs`, `yulius-egress/**`, `yulius-domain/src/vault/**`,
  `yulius-domain/src/redact.rs`, `yulius-harness/src/scrub_client.rs` and `secret.rs`. Parallel
  tasks hold `supervisor.rs` (#650), `mcp.rs` and `ports/**` (#656), `yulius-git/**` and
  `yulius-github/**` (#651), `ci/**` (#652).
- Out of scope by the request: F1, F4, F9 and Q5.

## Requirements

- R1. Every refusal text, audit entry and decision stays as it is.
- R2. No test is weakened or deleted except tests of deleted code; folded tests keep every case.
- R3. A file held by a parallel task is left and named under "Not done" in the pull request.
- R4. The two possible authorisation gaps of the report's Q3 are checked first and, when real,
  reported to the owner instead of fixed.

## Plan

- [x] F2: one named-agent-secret lookup and one personal/production refusal in the vault tools
- [x] F2: one argument parser and one scrub-file planner in the vault tools
- [x] F2: independent review of the vault-preamble PR
- [x] F5: Q3 authorisation gaps checked (agent_git_credential, handoffs owner check)
- [x] F5: one instance-admin rule (UserRecord::is_admin) and one token-scope rule (token_allows)
- [x] F5: independent review of the auth-helper PR
- [x] F6: one zeroizing Secret type shared by every crate
- [x] F6: constructor shim removed, call sites moved to Secret::new
- [ ] F6: independent review of the Secret PR
- [ ] F7: one JSON string walker for redaction
- [ ] F7: one redact_for_sandbox and shared credential-shape detectors
- [ ] F7: independent review of the redaction PR
- [ ] F8: pure egress decision in yulius-domain with unit tests
- [ ] F8: egress_apply calls the pure decision
- [ ] F8: independent review of the egress PR
- [ ] F3: one no-follow bounded reader for login files
- [ ] F3: provide_cli and the shared credential constants
- [ ] F3: independent review of the credential-reader PR

## Acceptance criteria

- AC1. Each delivered finding's pull request merged into `dev` on green Yulius CI, or the finding
  listed under "Not done" with the reason.
- AC2. The touched test binaries pass with their existing assertions unchanged.

## Test plan

- `cargo test -p yulius-app --lib` and the whole `yulius` integration binary (`--test main`)
  for every vault and auth change; the domain, harness and egress crates' own tests when touched.

## Decisions

- The vault's `session_hash.is_none()` checks stay: the two-factor unlock is stored per session
  hash, and they equal `!is_browser()` only through how `auth.rs` builds contexts.
- F5: one `UserRecord::is_admin` (not on `AuthContext`): the event fan-out, notices and the
  login-terminal recheck hold a `UserRecord`, so one helper covers every site. The gating sites
  keep their own refusal texts, so none is routed through `require_instance_admin` or
  `require_superadmin` (their texts differ). `AuthContext::token_allows` folds the three copies of
  "a token's scope narrows, a session's does not". The disabled-user rechecks after the extractor
  stay: `App` methods are also called with contexts built outside the HTTP extractor, and no test
  proves each of them refuses a disabled person first.
- F6: the one `Secret` lives in `yulius-contracts`, not `yulius-domain` as the audit proposed:
  the domain crate may use only `thiserror` and `toml`, and any other dependency (`zeroize`)
  needs a boundary decision (`scripts/check_repository.py`), which a chore does not take. Every
  crate that held a copy already depends on `yulius-contracts`. `new` takes `impl Into<String>`
  as the GitHub and import copies did; a credential file's bytes use `Secret::bytes`.
- F6: the GitHub installation flag stays on `model::Secret`, which now wraps the shared type.
  Moving it to a GitHub port bearer type changes the token parameter of 23 port methods: 77
  compile errors in 28 app files, most held by #651 (delivery) and #652 (ci), plus the wiring
  adapter and two test fakes. That is beyond this chore's size and lanes.
- F6: the GitHub copy printed `Secret([redacted])` under `Debug` and had a `Display`; the shared
  type prints `Secret(<redacted>)` like the other three, and nothing formatted the GitHub one
  with `Display`. Neither ever printed the value.
- Helpers stay inside the vault module where the copies are all there (`parse_args`, `named`),
  so the parallel MCP-argument cleanup (#656) is not raced.
