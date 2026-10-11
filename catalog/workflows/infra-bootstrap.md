---
name: Infra bootstrap
description: A project's engineering foundations, laid once like a senior engineer would - tests, lint, a local CI gate on our own machines, environments, docs, labels.
when_to_use: A fresh or newly onboarded project lacks its foundations - tests, lint, a CI gate, environments or deploy, agent docs - or its GitHub Actions must move to Yulius CI.
spec: required
tests: required
order: 60
---

## Steps

1. Survey the stack, tests, CI and deploy, then write the spec and its ADRs
2. One local gate with format, lint, unit and integration tests
3. GitHub Actions translated into Yulius CI, then deleted
4. Dev and production environments with deploy and verification
5. AGENTS.md, architecture and product docs, labels and templates

## Instructions

Follow `infra-bootstrap` for the content and `delivery-pipeline` for the delivery.

- Respect what exists: map existing scripts, tests and docs instead of replacing them, and pick
  what is state of the art for this project's own stack. Every choice gets an ADR.
- One gate runs everything a merge depends on, the same way locally, in the sandbox and on Yulius
  CI: `[commands] gate` or `[[ci.jobs]]` in `yulius.toml`. Translate every
  `.github/workflows/*` job into it (`ci_setup` lists them), run it, then delete the workflows in
  the same pull request. Never add GitHub Actions or anything that needs a paid GitHub plan.
- Environments are `[[delivery.environments]]` with an idempotent deploy command and
  `verify = "command"` smoke tests; secrets come from the project's agent secrets.
- Keep each step a small pull request when it grows (chain the next task). Afterwards the
  foundations stay stable: change them only when the product needs it, with a new ADR.
