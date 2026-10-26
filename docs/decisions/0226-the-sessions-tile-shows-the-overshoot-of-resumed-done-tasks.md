# 0226. The Sessions tile shows the overshoot of resumed Done tasks

## Status

Accepted. Specified in [SPEC-334](../../.ai/specs/SPEC-334-bruh-9-8-fix.md); task #318, fixes #928.
Keeps [ADR-0040](0040-done-tasks-release-their-subscription.md) and SPEC-050 SES-101.

## Context

The status panel's Sessions tile read "9 / 8" with 6 tasks queued, and the person asked "Bruh 9/8?
Fix". A Done task keeps its idle session for follow-ups but holds no session slot (SES-101), so the
queue starts another task in its place. When the Done task works again (a person types a follow-up
into its terminal, or its own background work wakes the agent), it counts again without passing the
start door. Typed input in a live terminal cannot be queued. The count can therefore pass the limit
until those turns end. The start door and the queue already start nothing while the count is at or
above the limit. The tile printed the raw `live / limit`, so the limit looked broken.

## Decision

- **Over the limit, the tile shows the limit full and marks the overshoot**: `8 / 8` with a `+1`
  mark, a full bar, and the detail `1 over the limit · 6 queued`.
- **It says why**: on hover (the tile's title) and to screen readers (hidden text): "1 Done task is
  working again, beyond the limit of 8 sessions. Queued tasks start once fewer than 8 run."
- Within the limit the tile is unchanged; the server's status view is unchanged.

## Alternatives

- **Enforce the limit on resumed Done tasks**: means refusing or holding a person's keystrokes in a
  live terminal, or stopping an agent mid-turn. SES-101 chose to let a follow-up run; the overshoot
  ends with its turn.
- **Count Done tasks' idle sessions again**: reverts SES-101. Idle sessions would keep queued tasks
  waiting for nothing.
- **Cap the number silently (`8 / 8`)**: hides sessions that really run.
- **Keep `9 / 8` and add a note**: still reads as a broken limit at a glance.

## Consequences

- The tile's value never shows a count above the limit; the overshoot is the `+N` mark.
- The mock takes `?sessionlimit=N` (default 8), so the e2e suite shows the tile both within and
  over its limit.
