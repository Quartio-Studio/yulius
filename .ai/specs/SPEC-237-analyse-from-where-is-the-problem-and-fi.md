---
yulius-task: d77f03a9-06c5-44af-9462-fc05cf467807
status: in-progress
---

# SPEC-237: Production CD says a secret agents use is never for production, not "waiting for a person"

Status: in-progress
Date: 2026-10-19
Owner: task #223
Scope authorization: task #223: "analyse from where is the problem and fix" (two screenshots: the
CI/CD page lists every production secret as "waiting for a person to allow it", the Secrets page
offers no Allow; and a garbled terminal answer). Fixes #704. Extends SPEC-134 (CD, CDR-002, round 2
M-B: a production secret is exclusive of agents' uses) and SPEC-204 (the Secrets page offers no
production Allow for a secret agents use).

## Goal

The CI/CD page and a refused production run tell a person what is actually possible: a secret
that agents use (its `.env` copies or proxy hosts) never goes to production, so production needs a
secret of its own. Nobody is told to allow something that the server refuses.

## Context

- **Symptom (screenshot 1).** The project's dev CD has 7 secrets, all `allowed` (their `.env`
  copies are allowed, ADR-0105). Production names the same 7: each shows "waiting for a person to
  allow it" with a Secrets link. On the Secrets page each one reads "Not for production" with no
  button (`agentCdProd()` in `pages/secrets.js`, SPEC-204).
- **Root cause.** `App::cd_secret_states` (`crates/yulius-app/src/vault/env_files.rs`) maps every
  existing secret that is neither `cd_prod` nor `auto_allow` to `NotAllowed` for production. It
  never checks whether agents use the secret, which `answer_cd_prod` refuses with 409 ("agents can
  use this secret", SPEC-134 round 2 M-B). `cd_secret_values` likewise refuses the run with "a
  person who manages X allows it for production deploys on the Secrets page".
- **Why the tests missed it.** `cd::production_secrets_need_their_own_allowance` asserts
  `not_allowed` for exactly a secret whose `.env` copies are allowed, and only checks that the
  refusal contains "for production deploys". The e2e mock has no production secret.
- **Second defect, out of scope (#705).** In screenshot 2, the agent's answer is unreadable: the
  secret `PIPEDREAM_PROJECT_ENVIRONMENT` has the value `production`, so exact-value redaction
  (SPEC-002 ADR-20, ≥ 8-byte needles) turns every "production" into
  `[secret:PIPEDREAM_PROJECT_ENVIRONMENT]`, and the longer text breaks the TUI's cursor-addressed
  redraws. Changing what counts as a secret is a redaction-policy change with its own spec and ADR,
  so it is filed as #705, not mixed into this fix.

## Requirements

- **CAU-001** For production, a secret that exists and that agents use (`env_files = allowed` or
  any bound host), and that is not allowed for production (`cd_prod`) or set up by agents
  (`auto_allow`), has the state `agents_use`, not `not_allowed`. `ready`, `missing`, `production`
  (dev) and dev's `not_allowed` are unchanged.
- **CAU-002** A production run naming such a secret is refused (`conflict`, nothing runs) with a
  message that says agents use it, so production never gets it, and that production names a
  separate secret (or its hosts and `.env` copies are revoked first). It no longer tells a person
  to allow it.
- **CAU-003** The CI/CD page shows `agents_use` as "agents use it: production never gets it" and,
  once under the list, what to do instead. The badge "waiting for a person to allow it" is kept
  for `not_allowed` only.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Both defects reported as issues
- [ ] Server reports a secret agents use as never for production (state + refusal), test-first
- [ ] CD page shows that state with what to do instead of a dead Secrets link
- [ ] Spec, docs and changes kept true; gates green
- [ ] Independent review answered and merged into dev

## Acceptance criteria

- `cd::production_secrets_need_their_own_allowance`: with `.env` copies allowed, the production
  state is `agents_use` and the refused run's message says agents use it, with no "allows it for
  production"; once the copies are revoked the state is `not_allowed`, then `ready` when allowed.
  A secret with a bound host is `agents_use` too. Fails on `dev`.
- `cd.e2e.ts`: the mock's production names a secret agents use; the Production region shows
  "agents use it: production never gets it" and the hint, and no "waiting for a person to allow
  it". Fails on `dev`.

## Test plan

Rust integration test in `crates/yulius/tests/cd.rs` (the HTTP API end to end), e2e test against
the mock for the page. Full repository gate (`scripts/dev ci`), e2e suite and security scan before
delivery.

## Decisions

- A new state rather than reusing `production`: `production` means "dev names a production
  secret", the opposite case; one name per meaning keeps the API readable. Additive to the
  `state` string set of `CdSecret` (clients show unknown states as `missing`, the safe fallback).
- The page keeps the Secrets link on the row: adding a separate secret for production happens
  there. No ADR: this reports a decision already taken (SPEC-134 round 2 M-B) truthfully.
