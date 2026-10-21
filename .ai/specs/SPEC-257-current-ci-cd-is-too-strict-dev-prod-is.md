---
yulius-task: 74141208-b642-43e4-a1d9-5cb21fd10398
status: done
---

# SPEC-257: CD environments beyond dev and production

Status: done
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
  with a letter; not `production`, which requests use for `prod`. Each table has the keys of SPEC-134 (`deploy`, `verify`, `secrets`, `about`, and
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
  each environment has its own cache per stage (a relabelled name never shares production's
  cache), and what is deployed is the last passed run of that name and stage; the run gets `YULIUS_CD_ENV=<name>` and
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
- **CDN-010** Delivery figures (Dashboard, `GET …/stages` `delivery`) are per stage: every
  environment of a stage counts as its deployments (a run's span carries `detail.stage`).

## Plan

- [x] Survey CD and CI, write the spec and the ADR
- [x] Named environments in the domain and the yulius.toml parser
- [x] Runs record their environment and stage
- [x] Deploys of a named environment from verified sources
- [x] CD page and cd_status show every environment
- [x] Docs, skills, briefing and onboarding describe named environments
- [x] Review findings fixed: cache, deployed run and figures per stage
- [x] Independent review approved and the gate green
- [x] Evidence recorded and merged into dev on green CI

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
- **Review H1, a cache per name and stage** (`cache/<name>` for the defaults, `cache/<stage>.<name>`
  otherwise): a dev-stage run of a relabelled name could otherwise leave files a production-stage
  run of that name later uses. Existing `cache/dev` and `cache/prod` stay where they are.
- **Review M1, `production` is reserved**: it is how requests name `prod`, so a table of that
  name could never be deployed itself and would deploy `[cd.prod]` in its place.
- **Review L2, the stage of a named environment is known after the verified reads**: an agent's
  request for a named production-stage environment is refused after Yulius read the verified
  heads, before any secret or promotion. `dev`/`prod` are still refused before anything is read.
  Refusals are audited (`cd.refused`) when the resolved stage is production.
- **Remote MCP `cd_deploy`** refuses an environment the page shows as production-stage, and
  without a typed slug the deploy refuses any production-stage one anyway.

## Evidence

- `scripts/dev ci` at 17038ab6: exit 0. Format, lint, `scripts/dev macos`, deps, docs and
  nextest: 2404 tests passed, 17 skipped. The first run, at 4b989384, failed one test,
  `workflows::catalog::tests::the_shipped_catalog_is_valid`: the infra-bootstrap workflow went
  past its 1,444-byte budget. The workflow text was reverted (17038ab6); the skill carries the
  guidance.
- New tests, each of which failed before its fix:
  - domain (`cd.rs`, `spec_format/config.rs`): named stages, `resolve_stage`,
    `environments_of`, the refusals (missing or contradicting `stage`, `promote` in a dev-stage
    table, bad names, `[cd.production]`, a 17th table);
  - store: name and stage round trip, rows from before the migration, the span's `detail.stage`,
    the deployed run per stage;
  - integration (`crates/yulius/tests/cd.rs`):
    - `named_environments_deploy_by_their_stage`;
    - `a_relabelled_environment_takes_the_integration_heads_stage`, which covers the review H1
      cache isolation and fails with the cache keyed by name only;
    - `a_named_production_run_is_cancelled_by_an_administrator`;
  - stages: `delivery_figures_count_every_environment_of_a_stage`;
  - e2e (`cd.e2e.ts`): `named environments show their stage and deploy by its rules`. All 18
    CD e2e tests pass locally; the whole suite runs as Yulius CI's `e2e` job on the pull request
    (a local full run was stopped after its first files passed, as a duplicate of that job).
- Existing CD tests are unchanged in meaning (AC6). Two assertions were updated:
  - a valid but undefined name (`staging`) is now 409 "add [cd.staging]", and `Staging` is 422;
  - `cd_status`'s closing line now names the stages.
- Independent review (fresh session, om-code-review):
  - round 1, REQUEST CHANGES: H1 a cache shared across stages, M1 `[cd.production]`, L1 Dashboard
    figures, L2 audit and refusal order, L3 tests;
  - all fixed in 4b989384;
  - round 2, APPROVE on 4b989384.
- `scripts/dev security` (Vidoc vidoc-1, change scan at 4b989384, 22 files): no critical, high or
  medium finding, and no low finding in the change.
- Not tested:
  - remote MCP `cd_deploy` for a named environment, because the remote harness has no cloned
    repository. The deploy refuses any production-stage environment without a typed slug, which
    the remote path never sends; that rule is covered by the domain tests;
  - a real deployment to a real machine (Yulius's own CD has none).

