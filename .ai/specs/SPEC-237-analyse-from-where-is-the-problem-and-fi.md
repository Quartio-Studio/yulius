---
yulius-task: d77f03a9-06c5-44af-9462-fc05cf467807
status: done
---

# SPEC-237: Production CD says a secret agents use is never for production, not "waiting for a person"

Status: done
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
  to allow it. Other secrets waiting beside it are named in the same refusal, so one answer says
  everything a person must do. MCP `cd_status` reads "NAME agents use it (production never gets
  it: name a separate secret)".
- **CAU-003** The CI/CD page shows `agents_use` as "agents use it: production never gets it" and,
  once under the list, what to do instead. The badge "waiting for a person to allow it" is kept
  for `not_allowed` only.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Both defects reported as issues
- [x] Server reports a secret agents use as never for production (state + refusal), test-first
- [x] CD page shows that state with what to do instead of a dead Secrets link
- [x] Docs and changes kept true
- [x] Review round 1 findings fixed test-first
- [x] Gates green and spec evidence recorded
- [x] Independent review answered and merged into dev

## Acceptance criteria

- `cd::production_secrets_need_their_own_allowance`: with `.env` copies allowed, the production
  state is `agents_use` and the refused run's message says agents use it, with no "allows it for
  production"; once the copies are revoked the state is `not_allowed`, then `ready` when allowed.
  A secret with a bound host is `agents_use` too, while dev still reads it `not_allowed`, and a
  plain secret named beside it appears in the refusal. Fails on `dev`.
- `ci::cd::tests::agents_read_that_production_never_gets_a_secret_agents_use`: the `cd_status`
  text for `agents_use`.
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
- The mock (`assets/mock.js`) refuses a production deploy for an `agents_use` secret with the
  server's first sentence only; it does not add the other waiting secrets, as no e2e test drives
  that mixed case (the Rust test covers the server's message). Review round 2 nit, accepted.
- The page keeps the Secrets link on the row: adding a separate secret for production happens
  there. No ADR: this reports a decision already taken (SPEC-134 round 2 M-B) truthfully.

## Evidence

- Red: `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^cd::production_/)'` at
  f7f45f66 (tests only): `production_never_gets_a_secret_bound_to_hosts` failed (`not_allowed`,
  expected `agents_use`), and `production_secrets_need_their_own_allowance` failed on the refusal
  "a person who manages DEV_TOKEN allows it for production deploys on the Secrets page".
  `npx e2e run tests/cd.e2e.ts` with dev's `cd.js`: the new test failed, with no "agents use it"
  badge (the unknown state fell back to "not in Secrets yet").
- Review round 1 red at 6118f620: the mixed refusal lacked PROD_TOKEN, and
  `agents_read_that_production_never_gets_a_secret_agents_use` failed. Green at 38185fb2.
- Green at 38185fb2: `scripts/dev ci` exit 0, with nextest 2262 tests run: 2262 passed (8 slow),
  17 skipped; deny "advisories ok, bans ok, licenses ok, sources ok"; docs built.
  `npx e2e run`: 60 files passed and 1 skipped, 629 tests passed and 1 skipped.
  `npx e2e run tests/cd.e2e.ts`: 14 passed.
- `scripts/dev security` (Vidoc vidoc-1, change scan at 38185fb2) exit 0: no open critical or
  high finding, and no medium finding in the change. Three findings sit in lines this branch did
  not touch (`ci/cd.rs:1522` production run output through `cd_status run_id`, `ci/cd.rs:809`
  one-run TOCTOU, `env_files.rs:131` `.git` folder); Yulius CI files each as an issue.
- An earlier `secrets.e2e.ts` run failed once on an axe target-size check of a Close button. That
  is the known flake #640, in code this change does not touch; the file then passed 20/20 twice.
- Independent review (fresh session, om-code-review): round 1 APPROVE with 3 low and 4 nit
  findings (6 own code, 1 pre-existing: the `cd_status` wording). All seven were fixed test-first in
  6118f620 and 38185fb2. Round 2 APPROVE
  at 38185fb2 with two nits: the spec lines (added) and the mock's mixed-case wording (accepted,
  see Decisions).
- Not tested: the real page against a live instance with the reported project's secrets. The mock
  and the HTTP API tests cover the same states.
