---
yulius-task: 6869077e-3347-40ab-88bc-151dc3aea411
status: done
---

# SPEC-415: Rollback is part of every project's CD, drilled on dev before production

Status: done
Date: 2026-11-04
Owner: Yulius task #397
Scope authorization: task #397, asked by quartio-v2's owner: "Create rollback mechanism. If
something breaks in the infra with the new update, we should be able to roll back the older
version quickly ... it should be a part of the ci/cd and it should be forced to be implemented as
tests and deployment". Decision record: [ADR-0263](../../docs/decisions/0263-rollback-redeploys-a-kept-release-and-a-dev-drill-proves-it-before-production.md).

## Goal

Every project deployed with Yulius CD can go back to an earlier good deploy in one click, without
a rebuild, and production cannot be deployed until that rollback is configured and has been proven
on dev by a drill that Yulius runs by itself.

## Context

- CD today (SPEC-134, SPEC-257, ADR-0109, ADR-0110, ADR-0195): `[cd.<name>]` tables of
  `yulius.toml`, read only from protected branches, each with a stage (`dev` or `prod`), a
  `deploy` and a `verify` command. A run (`cd_runs`) runs them from Yulius's verified mirror;
  production is a person's (administrator, slug typed); agents deploy dev with `cd_deploy`. Runs
  are pruned after 14 days, so they cannot be the list of what can be rolled back to.
- There is no rollback: going back means reverting on the branch and deploying again, which
  rebuilds, takes long, and is untested.
- quartio-v2 (dev `f883d01`) deploys two parts from one environment: `scripts/cd/run deploy dev`
  ships the engine (a signed release channel that customer VMs install from) and/or the infra
  (the operator platform on platform-dev) "whose files changed since each part's deployed
  commit"; `[cd.dev-both]`/`[cd.prod-both]` ship both. So one environment can deploy several
  parts, each with its own release history.
- A deploy's output already carries markers Yulius reads (`::yulius-stage`, `::yulius-measure`).

## Requirements

Contract (`yulius.toml`, every `[cd.<name>]`):

- RBK-001. `rollback`: a command that redeploys a release Yulius kept, never building. It reads the
  release to go to from `YULIUS_CD_RELEASE`, the part from `YULIUS_CD_PART` (empty for an
  environment of one part), the commit that release was deployed from from
  `YULIUS_CD_RELEASE_SHA`, and gets `YULIUS_CD_ROLLBACK=1`. The environment's `verify` runs after
  it with the same variables, exactly as after a deploy.
- RBK-002. `migrations`: one line for people saying how schema and data changes stay compatible
  with the previous release (or why there are none). `rollback` without `migrations` is invalid.
- RBK-003. `parts`: the names of parts the environment deploys separately (at most 8, environment
  name syntax). Without it the environment is one part.
- RBK-004. `drill` (production stage only): the dev-stage environment whose drill proves this
  environment's rollback; default `dev`. A `drill` naming no dev-stage table of the file is
  invalid.

Releases:

- RBK-010. A passed deploy records a release per part it shipped: `::yulius-release <id>` (one
  part) or `::yulius-release <part>=<id>` lines of its output; a one-part environment whose deploy
  printed none records its commit. A release id is 1-128 characters of letters, digits and
  `._+:@/-`; an invalid id, one holding an agent secret, or an unknown part is ignored and the
  output says so. Markers are read from the run's regular log file only (never through a link,
  never a FIFO), at most 256 MiB, the last 256 kept.
- RBK-011. `::yulius-rollback-barrier <reason>` (or `<part>: <reason>`) marks the release as one
  that cannot be rolled back past (an irreversible migration). Yulius keeps the reason, redacted
  of the sandbox's agent secrets.
- RBK-012. Yulius keeps the releases per environment, stage and part (the last 20 records, plus the
  newest deploy of each of the last 10 distinct releases deployed and of the current release),
  with commit, time and who. The current release is the newest one recorded by a passed deploy, or
  the last release a rollback or a failed drill rolled a part to (the wrapper prints
  `::yulius-rolled-back <step>` after each rollback command that succeeded), verified or not.
- RBK-013. "Previous" is the newest release deployed before the current one's deploy that differs
  from it, so repeated rollbacks walk back.

Rollback:

- RBK-020. A rollback of an environment's part to a kept release runs the `rollback` and then the
  `verify` command of the environment's table at its branch's verified head (dev: the integration
  head; production: the production head, never promoting), with the secrets, variables and
  containment of a deploy of that stage. Its release defaults to the previous one.
- RBK-021. Who may roll back is who may deploy (`dom::may_deploy`): dev-stage people with write
  access and agents (`cd_rollback`); production only an administrator who typed the project's
  slug, never an agent.
- RBK-022. A rollback to a release older than a barrier recorded at or before the current release
  is refused with the barrier's reason. Rolling forward is never blocked.
- RBK-023. A target not in the kept list, an environment without `rollback`, and a part it does not
  have are refused with a plain reason, nothing run.
- RBK-024. Every rollback is audited (`cd.rollback` before it starts, `cd.finished` at its end;
  refused production rollbacks `cd.refused`). A passed rollback records its release as current.

Drill and the production gate:

- RBK-030. After a dev-stage deploy passes and its table has `rollback`, Yulius starts a drill on
  that environment, by itself: for each part with a release before the one it runs (shipped by this
  deploy or not), roll back to that release and verify, then roll back to the current release and
  verify. When a barrier on any deploy of the current release forbids going back, the drill
  redeploys the current release through `rollback` and verifies it. A part with no earlier release
  is not drilled. A drill never runs on a production-stage environment. A drill that cannot start
  is recorded as an error saying why, and audited.
- RBK-031. A production-stage deploy whose table has a `deploy` command is refused unless its
  table has `rollback` ("rollback not configured") and the newest finished drill of its `drill`
  environment passed at a commit with the same tree as the commit being deployed (with
  `promote`: the integration commit it promotes). The refusal says which and what to do.
- RBK-032. A production environment that runs no deploy command (promotion only) deploys nothing
  Yulius could roll back and is not gated.

Surfaces:

- RBK-040. The CD page shows per environment its rollback state (not configured, configured,
  drill passed/failed, production proven or not and why) and the `migrations` line; per part the
  kept releases (release, commit, time, who; the current one marked) with "Roll back to this" on
  each other one and "Roll back to previous" as the one-click default. The confirmation shows the
  migrations line and, for production, takes the typed slug. A blocked target says why instead of
  a button. Runs show whether they deployed, rolled back or drilled.
- RBK-041. `cd_rollback` lets an agent roll back a dev-stage environment (part, release optional);
  `cd_status` shows rollback, releases and drills.
- RBK-042. Onboarding (infra-bootstrap skill and workflow, the CD page's set-up prompt), the
  briefing's delivery rules and the docs make rollback part of CD from day one, including the
  project gate test that the rollback command exists and accepts a release.

## Plan

- [x] Spec and ADR: rollback contract, releases, drill and production gate
- [x] Contract keys and release rules in the domain, with unit tests
- [x] Releases and run kinds stored (migration 0047), with store tests
- [x] Deploys record releases; a person rolls back from the API, audited
- [x] Dev drill runs after a deploy and gates production deploys
- [x] Agents roll back dev with cd_rollback; cd_status shows rollback
- [x] CD page: releases with one-click roll back, rollback state, e2e
- [x] Onboarding, briefing rules and docs teach rollback from day one
- [x] Independent review answered: three rounds, approved
- [x] Gates green and evidence recorded

## Acceptance criteria

- AC1. With `[cd.dev]` defining `rollback`, two passed dev deploys record two releases; the CD
  overview lists both with the newest current; "roll back to previous" runs the rollback command
  with the first release in `YULIUS_CD_RELEASE`, then verify, and the first release is current.
- AC2. The second dev deploy is followed by a drill run (kind `drill`, requested by Yulius) that
  rolls back to the first release, verifies, rolls forward to the second, verifies, and passes.
- AC3. A production deploy is refused while `[cd.prod]` has no `rollback`, refused while the drill
  of the commit it would deploy has not passed, and runs once it has.
- AC4. An agent rolls back dev with `cd_rollback`; an agent's production rollback is refused; a
  person without administrator access or the typed slug cannot roll back production.
- AC5. A rollback past a barrier is refused with its reason; `rollback` without `migrations` is a
  configuration problem; a production deploy never starts a drill.
- AC6. The CD page lists the releases and rolls back to a chosen one in one click after its
  confirmation (e2e on the mock).

## Test plan

- Domain unit tests: parsing of the new keys and their errors; release and barrier markers; the
  previous release, the barrier check and the drill steps over histories; the production gate.
- Store tests: run kinds and steps round-trip; releases are kept and pruned per part.
- Integration (`crates/yulius/tests/cd_rollback.rs`, real Git, real ptyhost, real scripts): a test
  project deploys dev twice, the drill runs, a person rolls back and forward, production is gated
  and then deploys, agents roll back dev only, barriers block, multi-part releases. This is the live
  drill on a test project.
- e2e (`e2e/tests/cd.e2e.ts`): releases and one-click roll back on the mock.

## Decisions

- Parts live inside an environment (`parts`), not as more environments: quartio-v2 deploys engine
  and infra from one run and ships only what changed. Details and alternatives in ADR-0263.
- A rollback runs the scripts of the branch's verified head, not of the release's commit: one
  verified source as for deploys, and the drill proves exactly the scripts a real rollback runs
  (the old commit may predate `rollback`). The release's commit is passed as
  `YULIUS_CD_RELEASE_SHA` for a verify that depends on it.
- The drill rolls forward with `rollback` to the new release, not with a new deploy: no rebuild,
  and it proves the command takes any kept release.
- The gate compares trees: a production head that is a merge of the drilled integration commit has
  its tree, and the drill proves those files' rollback.
- Review round 1: the drill covers every part with an earlier release, not only the parts a
  deploy shipped, so a deploy that ships nothing (quartio-v2's "only what changed") still proves
  its commit; a failed rollback or drill records where it left each part (from the wrapper's
  per-step markers, not the script's output alone) so "Roll back to this" works afterwards.
- Review round 2: a release deployed again inherits the barrier of its earlier deploy, so retention
  (which keeps a release's newest deploy) never keeps an older release behind a dropped barrier. The
  wrapper's `::yulius-rolled-back` marker shares the run's log, so the project's own scripts could
  print it; it only names steps of the run's own record, and the scripts are the project's,
  reviewed and merged like its code, so this is accepted (as `::yulius-release` is).
- `cd_rollback` is not added to the remote MCP of people's laptops (`remote.rs`) nor to the
  briefing's tool list (whose byte budget is full; the delivery rule names it): RBK-041 asks for a
  task agent's tool, which `tools/list` serves with its description.

## Evidence

- `scripts/dev ci` at 6193fe39: passed (repository checks, scripts' unit tests, check, fmt,
  clippy `-D warnings`, nextest 3099 passed / 16 skipped, doctests, cargo deny, cargo shear, doc).
- The live drill on a test project, `crates/yulius/tests/cd_rollback.rs` (real Git, the real
  detached ptyhost, real deploy/verify/rollback scripts, the fake GitHub), 7 tests passed:
  - AC1/AC2: two dev deploys record v1 and v2; the drill runs by itself (rollback v1, verify,
    rollback v2, verify; `YULIUS_CD_RELEASE`, `YULIUS_CD_RELEASE_SHA`, `YULIUS_CD_DRILL` reach the
    script); one request rolls back to the previous release without rebuilding and verifies it,
    another rolls forward to a chosen one; audit entries `cd.rollback`, `cd.drill`, `cd.finished`.
  - AC3/AC5: a production deploy is refused without `rollback`, without a drill, after a failed
    drill, and for a commit whose tree was not drilled; it runs once the drill of its commit
    passed; production is never drilled.
  - AC4: an agent rolls dev back with `cd_rollback` and is refused production (audited); a
    writer, and an administrator without the typed slug, cannot roll back production; an
    administrator can, on `main`, with no promotion.
  - Parts and barriers: engine and infra keep their own releases; a barrier makes the drill only
    redeploy and refuses a rollback past it with its reason; a barrier holding a secret is
    redacted; a deploy that ships no part still drills; a failed drill says where it left dev and
    rolling forward works; a drill that cannot start is recorded and audited; a redeployed
    release keeps its barrier.
- Existing CD tests (`crates/yulius/tests/cd.rs`) updated for the gate (production tables have a
  rollback and a seeded passed drill) and passing; 228 CD, stages, delivery, MCP and related
  integration tests passed before the review fixes, all again in the gate.
- Unit tests: `yulius_domain::rollback` (markers, history, previous, barriers, drill steps,
  retention, production gate), config parsing of the new keys and refusals, store kinds, steps
  and retention, `ci::rollback` (log reading never follows a link or waits on a FIFO or
  `/dev/zero`; last markers kept; reached steps), drill phase spans, briefing budget.
- Mutation checks: with the failed-drill recording, the redaction of barriers or the drilling of
  unshipped parts removed, the corresponding integration tests fail.
- e2e: `tests/cd.e2e.ts` 24/24 and `tests/pipeline.e2e.ts` passed (one-click roll back on the
  mock, production with the slug typed, no WCAG violations). Full suite: 819 passed, 1 skipped,
  1 failed: `editor.e2e.ts` "unsaved text is kept when the disk moves on…", which passed 32/32
  alone and lies outside this change (the live editor of #566); filed as #1054.
- Security scan: `yulius security-scan` reports "Security scan disabled" (Vidoc is turned off
  for this project): disabled, not passed.
- Independent review in a fresh session: round 1 requested changes (2 high, 4 medium, lows), round
  2 found one regression of a fix (barriers lost to retention), round 3 approved; every finding
  was fixed test-first or recorded in Decisions.
- Not tested here: a drill on a real Yulius instance (instances run `main`, and promoting is a
  person's) and quartio-v2's own rollback scripts (its follow-up task); macOS runs only the
  gate's cross-compilation.
