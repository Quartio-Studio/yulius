# 0383. A merge candidate reuses its tree's failed run

## Status

Accepted. Extends [ADR-0357](0357-a-merge-candidate-is-decided-by-its-own-run.md) for issue #1384.

## Context

After the first half of a failed candidate lands, its remaining members can rebuild the
original tree on a different base. Running it again can let a flaky failure through.

## Decision

During candidate formation, consult failed candidate runs in the sandbox's recent CI history
before looking for a passed verdict. When the tree matches, attach the original run ID.
An infrastructure error or cancellation leaves the tree eligible for testing.

## Alternatives

Remembering the omitted members works when bisection leaves exactly one. Looking up the
tree also works across different member lists and after a restart. Copying the failure to
a new run would duplicate evidence and obscure the original job logs.

## Consequences

The queue can bisect the remaining members using the original failure. CI retention and
the bounded candidate lookup determine which historical evidence is available.
