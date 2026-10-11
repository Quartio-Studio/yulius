# 0011. Superadmin recovery by a six-digit code

## Status

Accepted. Recorded retroactively from SPEC-002 (ADR-13); the full analysis is in
[SPEC-002 section 2](../../.ai/specs/SPEC-002-yulius-architecture.md).

## Context

Recovery is needed without email.

## Decision

Six random digits, Argon2id hash, persistent failure counter; three failures clear the hash and lock recovery permanently; regeneration needs a superadmin session plus the password.

## Alternatives

No recovery; email reset.

## Consequences

Guessing is bounded to 3e-6; an attacker can deliberately lock recovery, which the founders accepted.
