# 0014. Envelope-encrypted secrets vault with an egress proxy

## Status

Accepted. Recorded retroactively from SPEC-002 (ADR-20); the full analysis is in
[SPEC-002 section 2](../../.ai/specs/SPEC-002-yulius-architecture.md).

## Context

Agents need credentials but must never see them.

## Decision

XChaCha20-Poly1305 envelope encryption with a KEK from the master key; team reveals need re-authentication and are audited; agents use secrets through a credential-injecting egress proxy and `browser_fill_secret`; values are redacted from every agent-readable channel.

## Alternatives

`age`; SQLCipher; an external vault; secrets in the environment.

## Consequences

Redaction is an oracle for weak values (8-character minimum); remaining threats are listed in SPEC-002 section 2.6.
Since [ADR-0040](0040-personal-two-factor-guards-the-vault.md), reveals need a personal two-factor code instead of a recent sign-in, and the people who manage an agent secret can read it back.
