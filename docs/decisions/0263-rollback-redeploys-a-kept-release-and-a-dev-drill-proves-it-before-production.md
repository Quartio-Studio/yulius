# 0263. Rollback redeploys a kept release, and a dev drill proves it before production

## Status

Accepted; amended by [ADR-0282](0282-a-production-deploy-proves-its-rollback-on-dev-first.md)
(a person's production deploy that only lacks the drill deploys dev first). Specified in
[SPEC-415](../../.ai/specs/SPEC-415-global-rollback-in-ci-cd-one-click-roll.md);
task #397, asked by quartio-v2's owner. Builds on
[ADR-0109](0109-cd-runs-verified-commits-out-of-the-agents-reach.md),
[ADR-0110](0110-only-a-person-deploys-production.md) and
[ADR-0195](0195-cd-environments-are-named-and-take-their-rules-from-a-stage.md).

## Context

A deploy that breaks something could only be undone by reverting on the branch and deploying
again: a rebuild, slow, and never tested. The owner wants rollback to be a global part of CD,
quick, and forced on every project by tests and deployment. quartio-v2 deploys two parts (engine
releases to customer VMs through signed channels; the infra binary on the operator platform) from
one environment, shipping only the parts that changed.

## Decision

- **The contract is four keys of `[cd.<name>]`.** `rollback` (a command that redeploys the release
  in `YULIUS_CD_RELEASE`, part in `YULIUS_CD_PART`, never building), `migrations` (one line: how
  data changes stay compatible with the previous release; required with `rollback`), `parts`
  (parts deployed separately; default one) and, on production-stage tables, `drill` (the
  dev-stage environment that proves it; default `dev`). The environment's `verify` runs after a
  rollback as after a deploy.
- **Yulius keeps the releases, the project keeps the artifacts.** A deploy names what it shipped
  with `::yulius-release [<part>=]<id>` output lines (a one-part deploy that names none ships its
  commit); Yulius records them per environment, stage and part in `cd_releases` (beyond the
  14-day run retention) and offers the last 10 as rollback targets. The project keeps those
  artifacts (images, binaries, channel releases) for at least as many deploys.
- **A rollback runs the scripts of the branch's verified head**, not of the target's commit:
  the same verified source as a deploy (ADR-0109), and the scripts the drill proved. The target's
  commit is `YULIUS_CD_RELEASE_SHA`. Who may roll back is who may deploy (ADR-0110). Production
  rollbacks never promote.
- **Data safety is declared twice.** `migrations` says the policy, shown before the button; a
  deploy that makes rolling back unsafe prints `::yulius-rollback-barrier [<part>:] <reason>`, and
  a rollback to a release before it is refused with that reason. Rolling forward is not blocked.
  A release deployed again keeps the barrier of its earlier deploy (its migration ran then).
- **The drill is automatic and dev-only.** After a dev-stage deploy passes, Yulius rolls each
  part that has an earlier release (shipped by that deploy or not) back to it, verifies, rolls
  forward to the release it ran with the same `rollback` command and verifies (behind a barrier:
  redeploys that release only). It is a `cd_runs` row of kind `drill`, requested by Yulius,
  audited; one that cannot start is recorded as an error saying why. A rollback or drill that
  fails part-way records the release each part was last rolled to, from the wrapper's own
  per-step markers.
- **Production is gated on it.** A production-stage deploy that runs a deploy command is refused
  until its table has `rollback` and the newest drill of its `drill` environment passed at a
  commit with the tree being deployed. A promotion-only production (nothing Yulius deploys) is not
  gated.

## Alternatives

- **Parts as separate environments** (`[cd.engine-dev]`, `[cd.infra-dev]`): no new key, but
  quartio-v2 ships both parts from one run and only what changed; splitting would double its
  deploys and its tables. Rejected.
- **Rollback by redeploying the old commit** (check out the target, run its `deploy`): rebuilds,
  is as slow as a deploy, and runs scripts that may predate the contract. Rejected: the owner asked
  for quick rollback of a known-good artifact.
- **Running the target commit's own rollback script**: the old commit may have none, and the
  drill would prove a different script than a real rollback runs. Rejected.
- **A GitHub required status for the drill**: Yulius already decides production deploys itself
  (ADR-0110); a commit status on `dev` would not stop a person's deploy and needs another GitHub
  permission. The gate is checked where production deploys start.
- **Gating on "a drill passed at some earlier commit"**: weaker, it lets untested rollback scripts
  reach production. The tree must match; dev is deployed (and drilled) first.
- **No gate, a warning only**: the owner asked for rollback to be forced. Rejected.

## Consequences

- Every project with a production deploy command must add `rollback` and `migrations` and deploy
  dev twice before its next production deploy; until then its production deploys are refused with
  what to add. Yulius itself (promotion only) is unaffected.
- A dev deploy takes longer: its drill follows it (two rollbacks and two verifies per part).
- Projects keep artifacts for their last 10 deploys and test their rollback command in their own
  gate (the infra-bootstrap skill says how).
- A failed drill can leave dev on the earlier release; the page says so, and the next dev deploy
  or a rollback moves it on.
