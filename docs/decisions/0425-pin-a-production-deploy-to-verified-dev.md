# 0425. Pin a production deploy to verified dev

## Status

Accepted for SPEC-586. Revises the production CD rule in ADR-0411.

## Context

Dev can receive another merge before a person releases the revision that passed verification. Using the dev branch as the sole promotion source makes that release wait.

## Decision

A production deploy can select an earlier verified revision from the protected integration branch's first-parent history. Its SHA identifies the confirmation, promotion source and rollback evidence. A merge-commit profile opens its release pull request from a revision-specific ref when the branch has advanced. Fast-forward uses the selected SHA directly. The resulting production tree must match the selected tree for a pinned release.

Branch CI supplies the nearest verified commit on the first-parent history. An explicit dev run takes precedence. Its recorded jobs must cover the candidate’s own gate, including path-scoped jobs, even when the rerun used newer configuration. The same rule applies when the CI scheduler shares an ordinary run for that SHA. A job that returned the CI skip exit code retains that verdict through copies. An ambiguous historical skip cannot supply branch proof. An intermediate train step supplies proof only for the combined tree that actually ran. Rerunning an older commit changes that commit’s verdict. If that candidate required merge-first staging, its staging evidence can lead back to an earlier verified revision. CD validates that revision’s own configuration and place in the protected branch history.

## Alternatives

Waiting for the current head prevents a release while newer work is being checked. Deploying an arbitrary commit would lose the connection with protected dev verification.

## Consequences

Newer tasks keep their integration state and continue verification. A confirmed release can be retried against the same ref. Production access, release checks and the person's confirmation still apply.

If GitHub landed the release before a read failed, a retry checks the required verdicts, verifies the existing production tree and finishes recording the included tasks.

GitHub's pull-request merge accepts an expected head SHA. Yulius also checks the production base immediately before merging and verifies the resulting commit and tree before recording a release. An outside writer can still move the base during GitHub's merge call. If the result differs from the confirmed tree, CD returns a conflict, leaves the tasks on dev and starts no deployment scripts. [GitHub merge API](https://docs.github.com/en/rest/pulls/pulls#merge-a-pull-request)
