# 0264. Yulius's own production rolls back through its update, not a CD command; environments that deploy nothing need no rollback

## Status

Accepted. Specified in [SPEC-418](../../.ai/specs/SPEC-418-configure-rollback-in-the-cd.md);
task #400 ("configure rollback in the CD"). Builds on
[ADR-0013](0013-updates-from-main.md) and
[ADR-0263](0263-rollback-redeploys-a-kept-release-and-a-dev-drill-proves-it-before-production.md).

## Context

ADR-0263 made `rollback` part of every `[cd.<name>]` table that deploys. Yulius's own tables run
no deploy command: `[cd.dev]` is empty (no dev machine), and `[cd.prod]` only promotes `dev` into
`main`; each instance then builds and installs `main` when a superadmin chooses Update now. The CD
page and `cd_status` still said "Rollback not configured … production deploys need a proven
rollback" for both, and invited adding a `rollback` command, although RBK-032 does not gate such
an environment and there is nothing Yulius CD deployed that a command could redeploy.

What can undo a Yulius release today:

- The update helper backs up the binary and the database before an install and restores both when
  the new build fails its check (ADR-0013): an automatic rollback of a broken install.
- The helper refuses to install a commit older than the installed one ("updates never go back"),
  and the store refuses a database whose schema is newer than the build (`StoreError` "newer than
  supported"): an older build cannot open the database a newer one migrated.
- A release that runs but misbehaves is undone by reverting the change on `dev` and promoting
  again: the instance updates forward to a tree without it.

## Decision

- **Yulius's own CD gets no `rollback` command.** Its production rolls back in two ways, both
  outside CD: automatically, by the update helper when the new build fails its check; on purpose,
  by a revert on `dev` promoted to `main` and installed with Update now. `[cd.prod]`'s `about` says
  so, and the product and architecture documents describe it.
- **An environment that runs no deploy command needs no rollback.** The CD page shows its rollback
  as "Not needed" (not a warning) with why: a promotion-only production says that promoting is the
  release and how to undo one; an empty environment says rollback comes with its deploy command.
  `cd_status` says the same. A table that defines `rollback` anyway is shown as configured, as
  before. The production gate is unchanged (RBK-031/032).

## Alternatives

- **A `rollback` command that commits the target release's tree onto `main` and pushes it.** It
  would make instances update "forward" to the old tree, but it needs a push credential for `main`
  in CD, puts a commit on `main` that `dev` lacks (the next promotion keeps the revert silently or
  conflicts), and goes around the merge-commit promotion. Rejected.
- **A `rollback` command that moves `main` back.** A force push rewrites the production branch.
  Rejected.
- **Let the update helper install an older commit, or restore its latest backup on request.** This
  is the only true one-click instance rollback, but an older build cannot open a migrated database,
  so it means restoring the database backup and losing everything written since the update; and
  the helper's protocol (build or install, never back) is deliberately narrow because it runs as
  root on the service user's request. A separate, security-reviewed feature if a person asks for
  it; not configuration.
- **Keep the warning and a placeholder `rollback` (`true`).** It would turn the badge green while
  rolling nothing back: a false signal. Rejected.

## Consequences

- Yulius's CD page no longer shows a warning for its own environments; it says where Yulius's
  rollback lives.
- Every project whose production only promotes (libraries, self-updating products) sees the same
  "Not needed" instead of an instruction it cannot follow.
- Undoing a Yulius release that passed its check stays a revert promoted by a person; a release
  that added a migration cannot be undone by installing an older build.
