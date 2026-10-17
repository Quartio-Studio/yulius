---
yulius-task: 0cbba338-520f-462f-8c85-f4509cf397bf
status: draft
---

# SPEC-204: The Secrets page offers the production allowance only where it can be given

Status: draft
Date: 2026-10-17
Owner: task #191
Scope authorization: task #191 (requested by MrFishPL): "i couldnt set allow this for production
deploys, fix", with a screenshot of VIDOC_API_KEY (bound to `model.vidocsecurity.com`) and the
toast "Couldn't change it: agents can use this secret (copies into .env files or hosts the proxy
sends it to): revoke those, or add a separate secret for production deploys". Fixes #621. Applies
ADR-0109 and SPEC-134 CDR-002 as they stand.

## Goal

A person is never offered a secret action that the server will refuse. A secret agents use has
no "Allow for production deploys" button. Its card says why and what to do instead. A production
secret has no Allow for agents' requests.

## Context

- **Symptom.** Every agent secret a person manages shows a warning "Production" badge, the line
  "Production deploys (CD) do not get it until someone who manages it allows it here" and the
  button **Allow for production deploys**. For VIDOC_API_KEY, bound to `model.vidocsecurity.com`
  for the CI `security` job, the button fails every time with the 409 above. This project's
  `[cd.prod]` names no secrets, so production never asked for it. The warning badge suggested
  that something was missing when nothing was.
- **Root cause.** `agentCdProd` (`crates/yulius-server/web/assets/pages/secrets.js`) renders
  the allowance whatever the secret's state. `answer_cd_prod`
  (`crates/yulius-app/src/vault/env_files.rs`) correctly refuses a secret with bound hosts or
  `env_files = allowed`: a production secret is exclusive (ADR-0109, review M-B). The web mock's
  `/cd-prod` route does not apply that rule. The only e2e test used a secret without hosts, so
  the refusal was never exercised.
- **Same cause, other direction.** The server refuses to allow hosts (`answer_request`) or
  `.env` copies (`answer_env_files`) for a production secret (`PRODUCTION_SECRET`). A request
  can be left waiting from before the secret became a production secret, and the page still
  offers Allow for it.

## Requirements

- PXA-001. A secret agents can use (bound hosts, or `.env` copies allowed) shows no "Allow for
  production deploys" button and no warning badge. It shows a neutral note: production deploys
  never get a secret agents use, so add a separate secret for them.
- PXA-002. A production secret (`cd_prod`) whose host or `.env` request still waits offers
  Dismiss but not Allow. It says that production secrets are never given to agents.
- PXA-003. The web mock refuses, with the server's messages, what the server refuses: the
  production allowance for a secret agents use, and the host and `.env` allowances for a
  production secret.
- PXA-004. Nothing changes on the server. The exclusivity rule and its messages stay as they
  are (ADR-0109).

## Plan

- [x] Reproduce the defect and find its root cause
- [ ] Write the fix spec and the regression test that fails
- [ ] Fix the cause until the regression test and every gate pass
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- On the mock Secrets page, STRIPE_KEY (bound to `api.stripe.com`) has no button named
  "Let production deploys (CD) get STRIPE_KEY". Its card says production deploys need a
  separate secret.
- VIDOC_API_KEY (no hosts) can still be allowed for production deploys and revoked.
- Once VIDOC_API_KEY is a production secret, its waiting host request has Dismiss but no Allow.
- The mock's `/cd-prod`, `/request` and `/env` routes answer 409 where the server does.
- No WCAG violations on the page.

## Test plan

- `e2e/tests/secrets.e2e.ts`: a regression test for PXA-001 and PXA-002. It fails before the
  fix: the button is there and the toast shows. The existing production allowance test keeps
  passing.
- The server rule is already covered by `crates/yulius/tests/cd.rs` and `secrets.rs`, and does
  not change.
- Gates: `scripts/dev ci` (fmt, clippy, tests, docs integrity) and the e2e suite.

## Decisions

- **Keep the exclusivity, fix the page.** Relaxing the rule would let any agent use a production
  credential through the proxy without a person's production deploy, which ADR-0109 rejected
  (independent review H1, M-B). The request is satisfied by not offering what cannot be given,
  and by saying what to do instead. No new ADR: this applies ADR-0109.
- **The page derives the state from the view it has.** `hosts` and `env_files` are already in
  `AgentSecretView`, and the rule is one line. A new contract field would change the API for a
  presentation decision. The server stays the authority and still refuses.
- **The note is neutral, not a warning.** Most agent secrets are not for production, so a
  warning on each would read as a fault where there is none.
