# 0387. Bind Claude refusals to the launch credential

## Status

Accepted for SPEC-539.

## Context

A Claude run can deliver a refusal after the administrator signs its subscription in again, including after the server reattaches the watcher.

## Decision

Before spawning Claude, write the SHA-256 digest of the selected account's sealed credential to the attempt's private run directory. Under the existing subscription usage and sign-in lock, apply the refusal only if the current sealed credential has that digest.

## Alternatives

An in-memory snapshot would be lost when the server restarts. Keeping another copy of the sealed credential would retain a recoverable secret for the lifetime of the run directory. An account timestamp also changes for unrelated updates, including usage readings.

## Consequences

A refusal still interrupts its run. When an older run has no digest, the supervisor can move or pause the task but cannot attribute the refusal to the subscription's current sign-in.
