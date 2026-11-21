# 0425. Pin a production deploy to verified dev

## Status

Accepted for SPEC-586. Revises the production CD rule in ADR-0411.

## Context

Dev can receive another merge before a person releases the revision that passed verification. Using the dev branch as the sole promotion source makes that release wait.

## Decision

A production deploy can select an earlier verified revision from the protected integration branch's first-parent history. Its SHA identifies the confirmation, promotion source and rollback evidence. A merge-commit profile opens its release pull request from a revision-specific ref when the branch has advanced. Fast-forward uses the selected SHA directly. The resulting production tree must match the selected tree for a pinned release.

## Alternatives

Waiting for the current head prevents a release while newer work is being checked. Deploying an arbitrary commit would lose the connection with protected dev verification.

## Consequences

Newer tasks keep their integration state and continue verification. A confirmed release can be retried against the same ref. Production access, release checks and the person's confirmation still apply.
