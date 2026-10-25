---
yulius-task: f5a111b6-c57c-47d2-a140-63a929b440eb
status: done
---

# SPEC-314: A production-stage deploy refusal names the environment's own table

Status: done
Date: 2026-10-25
Owner: task #300
Scope authorization: task #300, an issue-triage automation: "Solve GitHub issue #788 … in one
pull request whose description says `Fixes #788`". Fixes #788. Builds on
[SPEC-237](SPEC-237-analyse-from-where-is-the-problem-and-fi.md) (CAU-002, the refusal) and
SPEC-257 (named CD environments, ADR-0195).

## Goal

When a production-stage deploy names a secret agents use, the refusal tells the person to name a
separate secret in the deploying environment's own table: `[cd.platform]` for `[cd.platform]`,
`[cd.prod]` for `prod`.

## Context

- **Symptom.** Deploying `[cd.platform]` (`stage = "prod"`) with `secrets = "DEV_TOKEN"`, where
  agents use `DEV_TOKEN` (its `.env` copies or hosts), answers 409 "… Production names a separate
  secret of its own in [cd.prod] secrets of yulius.toml …". The person would edit a table that is
  not the one deploying.
- **Root cause.** `cd_secret_values` (`crates/yulius-app/src/vault/env_files.rs`) receives only the
  stage (`CdStage`) and writes the literal `[cd.prod]`. The literal predates named environments
  (193b8e81, #712); SPEC-257 (#779) and #796 left it alone. Both callers in
  `crates/yulius-app/src/ci/cd.rs` (the deploy and the deploy after a promotion) have the
  environment's name.
- **Why the tests missed it.** `production_secrets_need_their_own_allowance` checks only "agents
  use DEV_TOKEN" and "separate secret"; no test pins the table or covers a named production-stage
  environment with a secret.
- **The mock.** `mock.js` already says `[cd.${name}]`, but stops before the server's tail (the
  Secrets page link and "or a person first revokes the .env copies and hosts of …").
- **Same cause elsewhere.** No other message in `crates/*/src` writes a literal `[cd.prod]` for a
  deploying environment: the run's variables note already writes `[cd.{name}]` (`ci/cd.rs`), and
  the page (`pages/cd.js`) writes `[cd.${env.name}]`. The dev-stage refusal ("dev names a secret
  of its own") names the stage, not a table, and stays.

## Requirements

- **RNE-001** A production-stage deploy refused because agents use a secret it names says
  "Production names a separate secret of its own in [cd.<name>] secrets of yulius.toml", with
  `<name>` the deploying environment (`prod` for production).
- **RNE-002** The mock's refusal is the same sentence as the server's, including the Secrets page
  link, the revoke alternative and the other secrets waiting for a person ("Also, …").

## Plan

- [x] Spec SPEC-314 written and validated
- [x] Failing regression test: a named production-stage refusal names [cd.platform], prod's names [cd.prod]
- [x] The refusal names the environment's own table (server)
- [x] Mock refusal says the server's whole sentence
- [x] Docs, changes entry and gates green
- [x] Independent review answered and PR merged on green

## Acceptance criteria

- **AC1** Deploying `[cd.platform]` (`stage = "prod"`) naming a secret agents use is refused
  naming `[cd.platform]` and not `[cd.prod]`.
- **AC2** Deploying `prod` in the same case is refused naming `[cd.prod]`.
- **AC3** The mock answers the same sentence for the same case.

## Test plan

- `crates/yulius/tests/cd.rs`: a new test deploys `[cd.platform]` of `named_cd()` with
  `secrets = "DEV_TOKEN"` after its `.env` copies are allowed, and asserts the table in the
  refusal (AC1); `production_secrets_need_their_own_allowance` pins `[cd.prod]` (AC2). The new
  test fails before the fix.
- `e2e/tests/cd.e2e.ts`: the existing refusal check (`agents use PAY_KEY`) also asserts
  `[cd.prod]` and the revoke alternative, and a new test adds a secret waiting for a person and
  asserts the "Also, …" tail (AC3).

## Decisions

- Pass the environment's name to `cd_secret_values` beside its stage rather than the whole
  target: the stage still picks the rules (SPEC-257), and the name is only text. No ADR: no lasting
  design choice, a message fix.
- The mock takes the server's tail too, so the two say the same thing (issue #788's third
  criterion), rather than trimming the server's useful link.

## Evidence

- **Regression test first.** At d7c35ef6 (test only), `cargo nextest run -p yulius -E
  'binary_id(yulius::main) & (test(=cd::a_named_production_refusal_names_its_own_table) |
  test(=cd::production_secrets_need_their_own_allowance))'` failed the new test: `[cd.platform]`'s
  refusal said "… in [cd.prod] secrets of yulius.toml …". With the fix (141064c1) all 23 `cd::`
  tests passed.
- **Mock, test first.** `npx e2e run tests/cd.e2e.ts` failed the new `[cd.prod]` and revoke
  assertion before 6509167c and passed 18/18 after it. After review round 1, the new "Also, …"
  test failed (no node matched) before 33e3b9d0 and the file passed 19/19 after it.
- **Gate.** `scripts/dev ci` at ecdcdace: exit 0, 2616 tests passed, 16 skipped. Run again at
  33e3b9d0, after the review fixes: exit 0, 2616 tests passed, 16 skipped. The commit after it
  changes only this spec.
- **Security.** `yulius security-scan` at ecdcdace: exit 2, not run. Vidoc answered HTTP 429: the
  instance key has used its 50,000,000 tokens. This is unrun, not passed; Yulius CI's `security`
  job repeats it.
- **Review.** Round 1 (fresh session, om-code-review) approved ecdcdace with these findings,
  answered in 33e3b9d0:
  1. Low, own code: the mock left out the server's "Also, a person who manages X allows it for
     production deploys". Fixed test-first; RNE-002 now names it.
  2. Nit, own code: the Plan was unchecked. Checked here.
  3. Nit, own code: `cd_secret_values` took `environment` and `env` (the stage). The stage is now
     `stage`.
  4. Nit, pre-existing: the mock's production refusal for a secret that only waits for a person
     ("allows using it outside the proxy") differs from the server's ("allows it for production
     deploys"). That line is untouched here. The task may not create issues, so it is reported on
     #788.
- **Not tested.** No deploy against a real `[cd.platform]` on the live instance: the integration
  test drives the same HTTP route with a real Git origin and the real refusal path.
