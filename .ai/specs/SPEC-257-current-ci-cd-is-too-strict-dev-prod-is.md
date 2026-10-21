---
yulius-task: 74141208-b642-43e4-a1d9-5cb21fd10398
status: in-progress
---

# SPEC-257: CD environments beyond dev and production

Status: in-progress
Date: 2026-10-21
Owner: task #243
Scope authorization: task #243. The operator: "Current CI/CD is too strict. Dev/prod is default
but we can [have a] different prod in case we're building something for self-serve but also the
platform for serving in the same repo, and cases like that. Make CI/CD more general but let it
work as it is as well."

## Goal

A repository that ships more than one thing (a self-hosted product released from `main` and the
hosted platform that serves it, a staging and a preview machine) defines one CD environment per
target, each deployed and verified on its own, with the same trust rules as today: dev-stage
environments are the agents' ground, production-stage environments are a person's. A project that
defines only `[cd.dev]` and `[cd.prod]` keeps working unchanged.

## Context

Confirmed against `dev` at aebad335. SPEC-134 (task #113, ADR-0109, ADR-0110) made CD: exactly two
tables, `[cd.dev]` and `[cd.prod]`; any other `[cd.<name>]` is a parse error ("CD has two
environments"). The two names are a Rust enum (`yulius_domain::cd::CdEnv`) that carries both the
name and every rule: the branch a deploy reads (integration or production), who may deploy
(`may_deploy`), the containment (dev inside the sandbox, production beside it), the secret
allowance (dev: outside the proxy; production: an allowance of its own) and the cache directory.
`cd_runs.environment` stores `dev` or `prod`; the CD page and `cd_status` show exactly two cards.

CI is already general: `[[ci.jobs]]` names any number of jobs with their own commands, matrices,
budgets and secrets (SPEC-142). What is strict is CD. The delivery profile's
`integration_branch`/`production_branch` are configurable already; one integration and one
production branch stay.

## Requirements

- **CDN-001** `yulius.toml` may define any number (at most 16) of CD environments as
  `[cd.<name>]` tables. A name is 1-32 characters of lowercase letters, digits and `-`, starting
  with a letter. Each table has the keys of SPEC-134 (`deploy`, `verify`, `secrets`, `about`, and
  `promote` for production) and `stage = "dev" | "prod"`.
- **CDN-002** `dev` and `prod` are the default environments: their stage is their name (`stage`
  may be omitted, and a contradicting one is an error). Every other table must name its `stage`
  (no silent default for what decides who may deploy). `promote` is valid only in a
  production-stage table.
- **CDN-003** The stage carries every rule SPEC-134 gave the two environments: a dev-stage
  environment deploys the integration branch head, inside the sandbox, by a person with write
  access or an agent, with secrets allowed outside the proxy; a production-stage environment deploys
  the production branch (after its own promotion when its integration-head table says `promote`),
  beside the sandbox, only by an administrator who types the project's slug, with secrets allowed
  for production deploys. Agents never deploy a production-stage environment.
- **CDN-004** A name's stage is read from verified sources: `dev`/`prod` are fixed; another name
  takes the stage its integration-head table gives it, else (no such table, or an invalid
  `yulius.toml` there) a production-stage table at the production head. A table read at the
  commit a deploy runs whose stage differs from the resolved one counts as not defined.
- **CDN-005** CD is defined when at least one dev-stage and one production-stage environment are
  defined (empty counts); the briefing's gap and the stop hook use this rule.
- **CDN-006** A run records its environment's name and stage; one run per environment at a time;
  each environment has its own cache; the run gets `YULIUS_CD_ENV=<name>` and
  `YULIUS_CD_STAGE=<stage>`. Cancelling a production-stage run needs administrator access.
  Existing runs keep their environment (`dev`, `prod`) as their stage.
- **CDN-007** `GET /api/sandboxes/{sid}/cd` lists every environment with its `stage`: the
  dev-stage ones first, then the production-stage ones, the default name first within each and the
  others in file order. When no environment of a stage is defined, its default (`dev`, `prod`) is
  listed as not defined, as today. `POST /api/sandboxes/{sid}/cd/{env}/deploy` and MCP
  `cd_deploy {environment}` take any name (`production` still means `prod`).
- **CDN-008** The CD page shows one card per environment; production-stage cards keep the
  administrator-only deploy with the typed slug. `cd_status` lists them all.
- **CDN-009** Documentation (AGENTS.md, architecture, the `infra-bootstrap` and
  `delivery-pipeline` skills, the briefing and onboarding prompt) describes `[cd.dev]` and
  `[cd.prod]` as the default and named environments as the way to deploy more than one target.

## Plan

- [x] Survey CD and CI, write the spec and the ADR
- [ ] Named environments in the domain and the yulius.toml parser
- [ ] Runs record their environment and stage
- [ ] Deploys of a named environment from verified sources
- [ ] CD page and cd_status show every environment
- [ ] Docs, skills, briefing and onboarding describe named environments
- [ ] Independent review, gates and delivery

## Acceptance criteria

- AC1 (CDN-001, CDN-002): `[cd.platform]\nstage = "prod"` parses as a production-stage
  environment; `[cd.platform]` without `stage`, `[cd.dev]\nstage = "prod"`, `promote` in a
  dev-stage table, a bad name and a 17th environment are refused with their line.
- AC2 (CDN-003, CDN-004): an agent's deploy of a production-stage named environment is refused
  (`ProductionIsAPersons`); its deploy of a dev-stage named environment runs that table's commands
  from the integration head; an administrator's deploy of a production-stage named environment
  runs from the production head; a name the deployed commit gives another stage is not defined.
- AC3 (CDN-005): `[cd.staging] stage=dev` with `[cd.platform] stage=prod` is defined; only
  dev-stage tables are not.
- AC4 (CDN-006): a run of `platform` stores `platform`/`prod`, a second `platform` run is refused
  while it runs and a `prod` run is not; rows written before the migration read back with their
  stage.
- AC5 (CDN-007, CDN-008): the overview of a project with `[cd.dev]`, `[cd.prod]`,
  `[cd.selfserve]` and `[cd.platform]` lists them in that order with stages; the CD page shows
  the four cards and production's dialog for each production-stage one.
- AC6: Yulius's own `yulius.toml` (only `[cd.dev]` and `[cd.prod]`) behaves exactly as before:
  existing CD tests pass unchanged in meaning.

## Test plan

- Domain unit tests (`yulius-domain` `cd.rs`, `spec_format/config.rs`): parsing, stages,
  `authorize`, `is_defined`.
- Store tests (`yulius-store` `cd.rs`): name and stage round trip, one running run per name,
  migration of old rows.
- Integration tests (`crates/yulius/tests/cd.rs`): named dev-stage deploy by an agent, refusal of a
  named production-stage deploy by an agent, administrator deploy of a named production-stage
  environment, overview order.
- e2e (`e2e/tests/cd.e2e.ts`): a named production-stage card with its dialog.
- Gates: `scripts/dev ci`, the e2e suite, `scripts/dev security`.

## Decisions

- **One table per environment with a `stage` key** ([ADR-0195](../../docs/decisions/0195-cd-environments-are-named-and-take-their-rules-from-a-stage.md)),
  not stage-scoped table names (`[cd.prod.platform]`) nor a branch per environment: the stage is
  the one thing that carries trust, and a branch per environment would need its own protection,
  verification and promotion path.
- **`stage` required for new names**: what decides who may deploy is never implied.
- **Stage resolved at the integration head first**: an agent who relabels a production
  environment as dev only gets a dev environment (integration branch, dev secrets, inside the
  sandbox); nothing of production's trust moves with the name.
- **Secrets stay allowed per stage**: a person allows a secret for production deploys, which any
  production-stage environment, always started by a person, may name.
- **CI unchanged**: `[[ci.jobs]]` already expresses any set of jobs.
