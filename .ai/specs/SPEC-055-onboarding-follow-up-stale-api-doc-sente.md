---
yulius-task: 7999422f-d2da-45f1-a320-9116aab792b8
status: in-progress
---

# SPEC-055: onboarding follow-up, SPEC-051's stale API sentence and review nits

Status: in progress
Date: 2026-10-13
Owner: Yulius task #46
Scope authorization: user request: "Follow-up to task #43 (SPEC-051, ADR-0038, PR #312, merged):
the independent review left small docs/test items that were not merged ... Apply them ... as one
small `docs`/`test` change, and deliver to dev on green Yulius CI".

## Goal

SPEC-051's documents, comments and tests say what its code does: an import never starts
onboarding, a `pending` sandbox waits for **Start onboarding** in the browser, and the import test
asserts the pending onboarding summary the real server returns. No runtime behavior changes.

## Context

- SPEC-051 (task #43, PR #312) removed `start_onboarding` from `apply_import`
  (`crates/yulius-app/src/imports.rs`); `docs/api.md` still said the preparation task starts once
  the import is applied.
- The independent review of PR #312 listed wording nits in ADR-0038, `AuthContext::is_browser`
  (`crates/yulius-app/src/auth.rs`) and the web mock (`crates/yulius-server/web/assets/mock.js`),
  an unasserted pending summary in `crates/yulius/tests/import_server.rs`, and 0038 out of order
  in `docs/decisions/README.md`. They were left out so as not to supersede PR #312's green CI run.
- The pending summary comes from `crates/yulius-app/src/onboarding.rs`
  ("Not ready yet: {label} waits for someone to start onboarding.").

## Requirements

- FUP-001: `docs/api.md`'s import "Applying:" paragraph says the import never starts onboarding and
  that a `pending` sandbox waits for Start onboarding in the browser.
- FUP-002: ADR-0038's per-token scope alternative says a cookie session (the web UI's sign-in,
  CSRF-checked) is what the browser has and an API token is not.
- FUP-003: the `AuthContext::is_browser` doc comment says the caller signed in through the web UI
  rather than with an API token.
- FUP-004: the mock explains why its import onboarding stays `pending` while the real server
  starts a browser import's onboarding at once.
- FUP-005: `import_resumes_applies_and_reruns_idempotently` asserts that
  `onboarding_status.summary` contains "waits for someone to start onboarding".
- FUP-006: `docs/decisions/README.md` lists the decisions in numeric order.

## Plan

- [x] Docs match the code: api.md import paragraph, ADR-0038 alternative, decisions index order
- [x] Comments explain is_browser and the mock's pending onboarding
- [x] Import test asserts the real pending onboarding summary
- [ ] Gates green, independent review approves the head
- [ ] Merged into dev on green Yulius CI

## Acceptance criteria

- The import test fails when the pending summary does not contain the expected phrase and passes
  against the real server.
- `docs/api.md` no longer says the preparation task starts after an import.
- `scripts/dev ci` passes; Yulius CI is green on the head; an independent review approved it.

## Test plan

- `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^import_server::import_resumes_applies_and_reruns_idempotently$/)'`
  with the expected phrase mutated (must fail), then unchanged (must pass).
- `scripts/dev ci` for every gate.

## Decisions

- Optional item 7 (use `AuthContext::is_browser()` in `crates/yulius-app/src/vault/two_factor.rs`)
  is not done: `unlock_vault` needs the session hash value itself (for `vault_set_reauth`), and
  `vault/team.rs` reads `session_hash` the same way, so a boolean check would not give one
  definition. It would only add a second check. No ADR: nothing lasting is decided.
- A spec is written even though the request said none was needed, because Yulius requires one for
  any change beyond documentation (here a test and Rust comments).
