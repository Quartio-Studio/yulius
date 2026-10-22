# 0195. CD environments are named and take their rules from a stage

## Status

Accepted. Specified in [SPEC-257](../../.ai/specs/SPEC-257-current-ci-cd-is-too-strict-dev-prod-is.md);
task #243. Extends [0109](0109-cd-runs-verified-commits-out-of-the-agents-reach.md) (CD from
protected branches) and keeps [0110](0110-only-a-person-deploys-production.md) (only a person
deploys production) for every production-stage environment.

## Context

ADR-0109 gave CD exactly two tables, `[cd.dev]` and `[cd.prod]`. "Current CI/CD is too strict.
Dev/prod is default but we can [have a] different prod in case we're building something for
self-serve but also the platform for serving in the same repo." One repository can ship a
self-hosted product (released from `main`) and the hosted platform (deployed to machines), or need
a staging and a preview machine next to dev. Each target needs its own deploy and verify scripts,
secrets, runs and "what is deployed", which one `[cd.prod]` table cannot express without one
script deploying everything at once. CI already expresses any set of jobs (`[[ci.jobs]]`).

## Decision

- **Named environments.** `yulius.toml` may define up to 16 `[cd.<name>]` tables (lowercase
  letters, digits and `-`, starting with a letter, at most 32 characters), each with the keys of
  ADR-0109 and `stage = "dev" | "prod"`.
- **The stage carries the trust.** Branch read, who may deploy, containment, secret allowance and
  promotion follow the stage exactly as they followed `dev` and `prod`: a dev-stage environment is
  the integration branch, inside the sandbox, agents and people with write access, secrets allowed
  outside the proxy; a production-stage environment is the production branch (with its own
  `promote`), beside the sandbox, only an administrator who types the slug, secrets allowed for
  production deploys.
- **`dev` and `prod` stay the defaults.** Their stage is their name; `stage` may be omitted and
  cannot contradict it. Every other table must state its stage. A project with only the two tables
  works as before.
- **The stage of a name is resolved from verified commits.** At the integration head first; else
  a production-stage table at the production head. The table read where the deploy runs must have
  the same stage, or the environment is not defined there.
- **CD is defined** with at least one environment of each stage.
- **Runs keep name and stage** (`cd_runs.stage`, migration 0041): one run per environment, a cache
  per environment, `YULIUS_CD_ENV` (name) and `YULIUS_CD_STAGE` in the run.

## Alternatives

- **Stage in the table name** (`[cd.prod.platform]`): the stage would be fixed by syntax, but two
  spellings for the defaults (`[cd.prod]` and `[cd.prod.x]`) and nested TOML tables the strict
  parser does not support otherwise.
- **A branch per environment**: each would need its own protection, verified mirror ref and
  promotion path; one integration and one production branch cover the cases asked for.
- **Default unknown names to production**: safe, but a typo in `stage` would silently move an
  environment; requiring it says what decides the trust.
- **One `[cd.prod]` whose script deploys every target**: one run, one outcome and one set of
  secrets for unrelated targets; a failure in one blocks the other.

## Consequences

- An agent who relabels a production environment as dev in a pull request only gets a dev
  environment: integration branch, dev secrets, inside the sandbox. No production trust moves with
  a name.
- A secret allowed for production deploys may be named by every production-stage environment;
  each is still started by a person.
- The CD page, `cd_status` and the API list every environment with its stage; clients that keyed
  on the name `prod` read the stage instead.
