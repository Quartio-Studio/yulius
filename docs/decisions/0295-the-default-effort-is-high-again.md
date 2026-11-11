# 0295. The default effort is high again; the default model stays Fable 5.2

## Status

Accepted (2026-11-11). Supersedes the effort of
[ADR-0289](0289-tasks-default-to-fable-5-2-at-max-effort.md); its model, price and model-family
decisions stand. Specified in
[SPEC-462](../../.ai/specs/SPEC-462-default-effort-is-high-the-default-model.md) and
[SPEC-023](../../.ai/specs/SPEC-023-make-sure-the-default-model-is-always-op.md).

## Context

ADR-0289 moved the default from Opus 5.5 at `high` to Fable 5.2 at `max` on 2026-11-10. A day
later the person decided: "the default effort is **high**. Not max, not xhigh. The default model
stays Fable 5.2." At `max` every task, Cockpit and Superagent that names no effort uses more of
each subscription and takes longer, whatever the work is.

The MCP tool `task_create` still told agents "Model override (default Opus)" and "Effort override
(default high)": retyped wording that nothing held to the constants.

## Decision

`DEFAULT_CLAUDE_EFFORT = "high"`; `DEFAULT_CLAUDE_MODEL` stays `"claude-fable-5-2"`. As before
(ADR-0024), the defaults are applied at every launch and a task record keeps `None` for "Default":
no stored data changes, a session that names no effort takes `high` at its next start, and one
that chose an effort (also `max`) keeps it. Haiku still gets no `--effort`.

Wording that states the default is derived or checked: `task_create` builds its descriptions from
the two constants, and a test (`ui_contract.rs`) holds the composer's and Settings' "Default"
labels in `models.js` and `taskview.js` to them.

## Alternatives

- `xhigh`: between the two (the effort menu calls it "best for most coding"). Refused by the
  person in so many words.
- An instance setting (Settings, Instance) instead of a constant: the default changed twice in two
  days, and a setting would change it without a release, a promotion and an update. Not built
  here: the person asked for a change small enough to ship the same day. It needs a settings
  record, an API, a card, validation against the effort list and the labels read from the server
  instead of `models.js`. Worth its own task if the default moves again.
- Keep `max` and let people pick High in Settings: Settings is per browser, and the Cockpit and
  the Superagents take no choice from it.

## Consequences

Default sessions use less of each subscription and finish sooner than on 2026-11-10; work that
wants the deepest reasoning names `max` or `xhigh` in the composer, in Settings (per browser) or
with `/effort` in a Cockpit. Running tasks and Cockpits started at `max` keep it until their next
start. Production gets the change with the next promotion to `main` and update.
