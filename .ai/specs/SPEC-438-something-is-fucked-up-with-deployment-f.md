---
yulius-task: 2e134745-5dd3-413f-a53e-cb585d7515c3
status: in-progress
---

# SPEC-438: A production deploy proves its rollback on dev by itself

Status: in-progress
Date: 2026-11-07
Owner: Yulius task #420
Scope authorization: task #420, asked by the instance's owner with a screenshot of the refusal
"Couldn't deploy production: nothing ran: rollback is not proven for this commit: the last rollback
drill of dev ran at e83c627; deploy dev at this commit first: its rollback drill follows by itself".
Issue [#1115](https://github.com/Quartio-Studio/yulius/issues/1115). Decision record:
[ADR-0282](../../docs/decisions/0282-a-production-deploy-proves-its-rollback-on-dev-first.md).

## Goal

"Deploy production" works when the only thing missing is a rollback drill at the commit it deploys.
Yulius deploys dev at that commit, its drill follows, and production deploys once the drill passed.
The person no longer races agents' merges into dev.

## Context

- Symptom: a person's production deploy is refused because dev's newest drill (RBK-031, ADR-0263)
  ran at another commit.
- Root cause: with `promote = true` (the infra-bootstrap default) production deploys dev's head.
  Merges deploy nothing (ADR-0110) and agents merge into dev all day, so dev's newest drill is
  almost always at an older commit. The refusal asks the person to deploy dev by hand, wait for the
  deploy and its drill, and deploy production again before the next merge. The gate is right; the
  flow around it is broken. `production_deploys_wait_for_a_proven_rollback` already shows the
  refusal ("Dev moved on since its drill") but tests it as the intended end state.
- Code path: `cd_prepare` (`crates/yulius-app/src/ci/cd.rs`) → `cd_rollback_gate` →
  `production_gate` (`crates/yulius-domain/src/rollback.rs`). A passed dev deploy starts
  `cd_start_drill` (`crates/yulius-app/src/ci/rollback.rs`) at the end of `cd_execute`.

## Requirements

- PDF-001. A person's production deploy refused only for its rollback proof, where a dev deploy can
  supply that proof, starts a deploy of the drill environment instead of failing. "Can supply" means
  all of these hold: the table has `rollback`; the newest drill is missing, or ran at another tree;
  no drill is running; the drill environment is a dev-stage table at the integration head with
  `deploy` and `rollback`; and the integration head has the tree production would deploy. The dev
  deploy is the person's own: same checks, secrets and audit as a dev deploy. Its note says
  production deploys at `<sha>` once its drill passes. The request answers with that dev run.
- PDF-002. Production waits in the server. Once the dev deploy passed and the drill it started
  passed, Yulius starts the production deploy as that person, with their access checked again and
  the commit they confirmed as `expect`. If dev moved, production refuses as it always does
  ("moved since you looked"). Only a person's request does this: an agent's never does, and neither
  does the continuation itself.
- PDF-003. If the dev deploy does not pass, its drill does not start, does not pass, or no drill
  follows it, production does not deploy. The same holds when the continued deploy is refused.
  Yulius then records an ended production run in state error whose reason starts "production did
  not deploy:" and says why, with its audit entry (`cd.refused`).
- PDF-004. While production waits, the CD overview says so on production's rollback
  (`waiting`: "Production deploys at <sha7> once the rollback drill of dev passes (asked by
  <login>)"). The page shows it. Another production deploy is then refused with that sentence. A
  restart forgets the wait: the dev deploy ends as an error (as any run left by a restart), and
  production is not deployed.
- PDF-005. Refusals that no dev deploy can fix stay refusals as before: rollback not configured, a
  failed drill at the same tree, a drill running, a dev that cannot deploy that tree, and every
  refusal that is not about the rollback.

## Plan

- [x] Reproduce the defect and find its root cause
- [ ] Spec, ADR and the regression test that fails
- [ ] Production deploy proves its rollback on dev first and continues by itself
- [ ] Waiting and not-deployed production shown on the CD page and in cd_status
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- AC1 (PDF-001, PDF-002): dev pushed past its drill; Bob deploys production; the answer is a dev
  run; dev deploys, drills and passes; production then deploys (and promotes) at that commit.
- AC2 (PDF-003): with a failing rollback, the drill fails and production records an error run
  "production did not deploy: the rollback drill of dev at <sha7> failed"; `main` does not move.
- AC3 (PDF-005): "not configured" and a failed drill at the same tree are refused as before.
- AC4 (PDF-004): while it waits, the overview's production rollback has `waiting`, and a second
  production deploy is refused with it.

## Test plan

- Domain unit tests: `dev_can_prove` for each gate outcome.
- Integration (`crates/yulius/tests/cd_rollback.rs`, real Git and ptyhost): AC1-AC4, with the
  existing `production_deploys_wait_for_a_proven_rollback` updated where it asserted the old
  refusal.
- e2e (`e2e/tests/cd.e2e.ts`): the waiting line on production's rollback.

## Decisions

- Deploy dev for the person rather than promote the drilled commit: promotion moves the
  integration branch's head (a pull request or fast-forward), not an arbitrary older commit. And
  production must stay what dev runs. See ADR-0282.
- The wait lives in memory, not in the database. A restart already ends the dev deploy as an error,
  so a stored wait would only resume a deploy whose proof never finished. Reversible: a table can
  be added later if waits need to survive restarts.
