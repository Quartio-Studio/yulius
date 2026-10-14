---
yulius-task: f5d3d8ad-96de-495e-bfe2-254c87d8590d
status: in-progress
---

# SPEC-107: the update helper accepts GitHub's stateless installation tokens

Status: in progress
Date: 2026-10-14
Owner: Yulius task #92
Scope authorization: task request "fix" with a screenshot of the Updates page: "The last update
failed — The request was refused: the token is malformed." Tracked by issue #423.

## Goal

"Update now" works again when the update check ran via the GitHub App: the root update helper
accepts the App's installation token and fetches `main` with it, as before.

## Context

- ADR-18 (SPEC-002 §2.5): the server writes `<data>/update/request.json` with a short-lived read
  token; `yulius update-helper` (`crates/yulius/src/update_helper.rs`) checks the untrusted request
  strictly in `parse_request` and sends the token to Git only as an `http.extraHeader`.
- `valid_token` accepted 1–255 bytes of `[A-Za-z0-9_.-]`. Since 2026-10-02 GitHub mints every
  installation token in the stateless `ghs_<APPID>_<JWT>` format, about 520 characters long and
  variable in length
  (https://github.blog/changelog/2026-10-02-stateless-github-app-installation-tokens-rolled-out/).
  The characters still match (base64url JWT segments, `.`, `_`), but the length does not, so every
  App-token build request is refused as "the token is malformed".
- The tests used only short synthetic tokens (`ghs_A1b2.c-d_e`, `ghs_SyntheticToken123`), the old
  40-character shape, so nothing exercised a real token length.

## Requirements

- R1: `parse_request` accepts a token of `[A-Za-z0-9_.-]` up to `MAX_TOKEN_BYTES` = 4096 bytes,
  which covers a stateless installation token with ample room for its variable claims.
- R2: a token that is empty, longer than the bound, or holds any other byte (CR, LF, space, `:`)
  is still refused, so it cannot inject header content.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] Fix the cause until the regression test and every gate pass
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- A synthetic `ghs_<appid>_<header>.<payload>.<signature>` token of 520, 1024 and 4096 bytes is
  accepted; one of 4097 bytes and one with `\r\n` header injection are refused.
- The existing strict-parsing cases (`requests_are_parsed_strictly`) still pass unchanged.

## Test plan

- Regression: `update_helper::tests::stateless_installation_tokens_are_accepted` (failed before the
  fix with "a 520-byte installation token is refused").
- Existing: the whole `update_helper::tests` module, then the full gate (`scripts/dev ci`).
- Not testable here: a real update on the production instance (needs the root helper, systemd and
  a GitHub App token); the next "Update now" there is the live check.

## Decisions

- Bound of 4096 bytes rather than removing the bound: the request file is untrusted and the
  token becomes an HTTP header, so it stays bounded. 4096 is about 8× today's token and far below
  common header limits (8 KiB) after base64 for Basic auth (~5.5 KiB); the request itself is
  capped at 16 KiB. Not exactly GitHub's current length, which varies with the token's claims.
- Character set unchanged: the stateless format still fits `[A-Za-z0-9_.-]`, and widening it
  would weaken the header-injection guard for no gain.
- The same format change breaks secret redaction of these tokens (`yulius-domain` and
  `yulius-git` redactors). That is a separate concern with security risk, filed as #424, not
  widened into this fix.

## What else the cause could break

Anything that assumed the old 40-character token shape: the redactors (#424). Storage keeps
tokens as unbounded strings (SQLite `TEXT`, Rust `String`), and the Git credential helper accepts
any printable token, so neither is affected.

## Evidence

- Before the fix (commit f43b9b26): `cargo test -p yulius --lib update_helper::tests::` → 12 passed,
  1 failed: `stateless_installation_tokens_are_accepted` panicked "a 520-byte installation token is
  refused".
- After the fix: the same command → 13 passed, 0 failed.
- `scripts/dev ci` → exit 0: repository check, script tests, `cargo check`, `fmt --check`,
  `clippy -D warnings`, nextest 1580 passed / 16 skipped, doctests, `cargo deny`, `cargo shear`,
  `cargo doc`.
- No web UI change, so the e2e suite does not apply.
