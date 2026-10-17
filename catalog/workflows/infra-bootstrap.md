---
name: Infra bootstrap
description: A project's engineering foundations, laid once like a senior engineer would - tests, lint, a local CI gate on our own machines, CD (deployment), docs, labels.
when_to_use: A fresh or newly onboarded project lacks its foundations - tests, lint, a CI gate, CD (deployment), agent docs - its GitHub Actions must move to Yulius CI, or its CD must be set up or changed.
spec: required
tests: required
order: 60
---

## Steps

1. Survey the stack, tests, CI and deploy, then write the spec and its ADRs
2. One local gate with format, lint, unit and integration tests
3. GitHub Actions translated into Yulius CI, then deleted
4. CD dev and production defined with deploy and verify scripts
5. AGENTS.md, architecture and product docs, labels and templates

## Instructions

Follow `infra-bootstrap` for the content and `delivery-pipeline` for the delivery.

- Respect what exists: map existing scripts, tests and docs instead of replacing them, and pick
  what is state of the art for this project's own stack. Every choice gets an ADR.
- One gate runs everything a merge depends on, the same way locally, in the sandbox and on Yulius
  CI: `[commands] gate` or `[[ci.jobs]]` in `yulius.toml`. Translate every
  `.github/workflows/*` job into it (`ci_setup` lists them), run it, then delete the workflows in
  the same pull request. Never add GitHub Actions or anything that needs a paid GitHub plan.
- CD is `[cd.dev]` and `[cd.prod]` in `yulius.toml`, both explicit (empty with an `about` when
  nothing deploys yet), with idempotent deploy and verify scripts under `scripts/cd/` and secrets
  named in `secrets`. Move what deploys today there; ask with `needs_you` for what only a person
  gives. A merge deploys nothing; production is a person's. App keys live in an ignored `.env`
  (`secret_copy_to_env`).
- Security scan: with a `## Security scan` briefing section, Yulius CI already runs the built-in
  `security` job (`yulius security-scan --ci`): add no job, only the rule to scan before delivery;
  else note: no Vidoc key on this instance.
- Keep each step a small pull request (chain the next task). Afterwards the foundations stay
  stable: change them only when the product needs it, with a new ADR.
