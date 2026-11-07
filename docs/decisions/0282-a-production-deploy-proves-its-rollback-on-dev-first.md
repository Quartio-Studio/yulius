# 0282. A production deploy proves its rollback on dev first

## Status

Accepted. Specified in [SPEC-438](../../.ai/specs/SPEC-438-something-is-fucked-up-with-deployment-f.md);
task #420, issue #1115. Amends [ADR-0263](0263-rollback-redeploys-a-kept-release-and-a-dev-drill-proves-it-before-production.md)
(the production gate itself is unchanged).

## Context

ADR-0263 refuses a production deploy until the newest drill of its drill environment (`dev` by
default) passed at a commit with the tree it deploys. With `promote = true`, production deploys
dev's head. Merges deploy nothing (ADR-0110) and agents merge into dev all day, so dev's newest
drill is almost always at an older commit. The refusal told the person to deploy dev, wait for the
deploy and its drill, and deploy production again before the next merge landed. On a busy project
the person kept losing that race (screenshot of 2026-11-07: "rollback is not proven for this
commit: the last rollback drill of dev ran at e83c627; deploy dev at this commit first").

## Decision

- **A person's production deploy that only lacks the proof starts it.** The gate refuses as before.
  When a dev deploy can supply what is missing, the request deploys the drill environment instead,
  as the same person with the same checks. "Can supply" means: rollback is configured; the newest
  drill is missing or at another tree, and none is running; the drill environment has `deploy` and
  `rollback` at the integration head; and that head has the tree production would deploy. The
  answer is that dev run, whose note says what follows.
- **Production waits in the server and continues by itself.** Once that deploy passed and the drill
  it started passed, Yulius starts the production deploy as the person who asked. It checks their
  access again and passes the commit they confirmed as `expect`, so a dev that moved meanwhile is
  refused as always. The continuation never starts another dev deploy. An agent's request never
  does either: agents never deploy production.
- **A wait that ends without production says so.** A failed or cancelled dev deploy, a drill that
  fails, cannot start or does not follow, or a refused continuation each record an ended production
  run in state `error`. Its reason starts "production did not deploy:", with a `cd.refused` audit
  entry. The overview shows the wait on production's rollback, and another production deploy is
  refused while one waits.
- **The wait is in memory.** A restart ends the dev deploy as an error, as any run left by a
  restart, and production stays undeployed. The person deploys again.

## Alternatives

- **Promote the drilled commit instead of dev's head**: promotion moves the integration branch's
  head (fast-forward or a pull request), so an older commit cannot be promoted without a new
  mechanism. Production would also stop being what dev runs. Rejected.
- **Accept any earlier passed drill**: ADR-0263 already rejected it; untested rollback scripts
  would reach production.
- **Deploy dev automatically after every merge**: it costs a deploy and a drill per merge and
  changes ADR-0110's "merges deploy nothing" for every project. Rejected.
- **Keep the refusal and add a "Deploy dev" button beside it**: the person still waits and races
  the next merge. Rejected.
- **Store the wait in the database**: it would only resume a deploy whose proof a restart already
  ended as an error. Not needed now; reversible.

## Consequences

- One click on "Deploy production" now covers dev's deploy, its drill and production. It takes as
  long as all three together, and dev ends on the released commit.
- Dev is deployed on a person's production request. That deploy is theirs and audited as theirs.
- The production card's last run tells why production did not deploy when the proof failed.
