# 0449. Use Codex quota headroom and preserve idle moves

## Status

Accepted for [SPEC-611](../../.ai/specs/SPEC-611-a-moved-task-is-not-shown-as-working-and.md). Refines SPEC-001 ROT-004 and [ADR-0288](0288-a-person-moves-a-task-between-subscriptions-and-removing-one-moves-its-tasks.md) for Codex.

## Context

The weekly reset score can put a Codex session on an account with 7% left while an account with 98% left is available. Codex warns about approaching rate limits when less than 10% remains. A live Goal-mode Codex task also loses its finished activity when a move reopens its idle conversation. Codex uses Yulius's shared goal gate and receives the original request as a prompt.

## Decision

Rank eligible Codex subscriptions by the remaining percentage in their tightest known binding window. Known quota of at least 10% comes first, followed by unknown readings and then known quota below 10%. Use the existing score and tie-breakers for equal headroom. Apply this ranking whenever the pool places a Codex session. Eligibility, explicit subscription choice and whether an existing session needs to move continue to follow the pool's existing rules.

When a subscription move reopens a live idle Codex conversation, carry its saved activity into the replacement launch regardless of the task's mode. The final launch mode determines whether work starts so a resumed reviewer or continuation still shows Working.

## Alternatives

A warning threshold alone would still give reset urgency priority within each group. A hard 10% eligibility threshold would leave work waiting when the pool still has usable quota. Inferring idle state from the terminal would require recognizing Codex's presentation and could lag behind the actual launch.

## Consequences

Codex may leave capacity unused when a tight subscription resets. Unknown usage remains eligible. The replacement run can settle at Waiting or Done without depending on another turn to clear the switch's working timer.
