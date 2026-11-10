# 0005. Claude Code only; accounts are org secrets with per-sandbox homes

## Status

Accepted. Recorded retroactively from SPEC-002 (ADR-05, ADR-06, ADR-17); the full analysis is in
[SPEC-002 section 2](../../.ai/specs/SPEC-002-yulius-architecture.md). Its "Claude Code only" is
proposed to be superseded for Codex by
[ADR-0290](0290-codex-returns-as-a-second-harness-with-a-pinned-cli.md) (SPEC-451), keeping the
per-sandbox homes and the absence of provider switching.

## Context

Founders decided on 2026-10-08 to support only Claude. Subscriptions should need no management.

## Decision

Claude OAuth tokens (from `claude setup-token`) are org secrets passed by environment; provider homes are per sandbox. Cross-provider transcript translation was withdrawn. Subscriptions form one self-managing pool: use-it-or-lose-it placement, proactive switch at 98 percent by native resume, and a gentle invisible pause held by the hook protocol when nothing is left.

## Alternatives

A global home per account; manual priorities; SIGSTOP; letting the CLI hit its limit.

## Consequences

No knobs for users; agents never see switches or pauses. Relies on Claude Code honouring a long hook timeout. A returning second harness needs its own decision.
