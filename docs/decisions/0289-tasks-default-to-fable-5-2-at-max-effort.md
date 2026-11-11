# 0289. Tasks default to Fable 5.2 at max effort

## Status

Accepted (2026-11-10); its effort (`max`) is superseded by
[ADR-0295](0295-the-default-effort-is-high-again.md) (`high`), the rest stands. Supersedes the model and effort of
[ADR-0024](0024-default-model-and-effort.md); specified in
[SPEC-023](../../.ai/specs/SPEC-023-make-sure-the-default-model-is-always-op.md).

## Context

ADR-0024 pinned the default to Opus 5.5 at high effort. Fable 5.2 (`claude-fable-5-2`) now leads
Opus 5.5 at every effort level, at about 1.5 to 2.6 times the cost per task. The person asked:
"make fable 5.2 effort max default model".

## Decision

`DEFAULT_CLAUDE_MODEL = "claude-fable-5-2"` and `DEFAULT_CLAUDE_EFFORT = "max"`. Everything else in
ADR-0024 stands: the full id is pinned, the defaults are applied at every launch and the Cockpit,
a task record keeps `None` for "Default", and Haiku gets no `--effort`. `ModelFamily::of` reads a
Fable id as `Any`, so only the 5-hour and weekly windows choose a default task's subscription, no
longer the weekly Opus cap. The price table gains Fable 5.2 ($8 in, $10 cache write, $0.20 cache
read, $40 out per MTok) and the composer lists it.

## Alternatives

- Fable 5.2 at medium effort: close to Opus 5.5 at xhigh, cheaper and faster, but not what was
  asked.
- Opus 5.5 at max: cheaper, and the weekly Opus cap would still bind, but it scores below Fable 5.2
  at every level.
- A Fable family with its own weekly cap: Yulius reads no such window from a subscription's usage
  today; add one when the usage report names it.

## Consequences

Default tasks use more of each subscription and take longer. A subscription without Fable access
cannot run a default task; people pick Opus 5.5 explicitly there. Claude Code 2.1.311 logs
`unrecognized_model` for the id but passes it on.
