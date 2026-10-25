---
yulius-task: f5a111b6-c57c-47d2-a140-63a929b440eb
status: in-progress
---

# SPEC-314: A production-stage deploy refusal names the environment's own table

Status: in-progress
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

- [ ] Spec SPEC-314 written and validated
- [ ] Failing regression test: a named production-stage refusal names [cd.platform], prod's names [cd.prod]
- [ ] The refusal names the environment's own table (server)
- [ ] Mock refusal says the server's whole sentence
- [ ] Docs, changes entry and gates green
- [ ] Independent review answered and PR merged on green

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
