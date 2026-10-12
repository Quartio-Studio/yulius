# 0018. Tasks start by a logical rule

## Status

Accepted. Extends [0012](0012-task-intake-and-chains.md); specified in
[SPEC-010](../../.ai/specs/SPEC-010-task-start-rules.md).

## Context

A chained task waited for exactly one predecessor to be Done. People want "start when X and Y are
both finished", "when either is", "when X failed", and so on.

## Decision

A chained task may carry a **start rule**: text in a small boolean language over `#N [done|merged|
ended]` atoms with `and`, `or`, `not` and parentheses. The rule is parsed and evaluated by a pure
domain module (`yulius_domain::rule`) in three-valued logic: a task that has not ended and is not
in the asked state is unknown. True starts the task, unknown keeps it waiting, false (it can never
become true) puts it under Needs you. The rule is stored as canonical text in `task_chains.rule`;
`after_task_id` stays the first task it names (branch base, briefing, links). Whenever a task
changes, the waiting rules of its sandbox are evaluated again (at most 32 per sandbox), and on the
background tick. A plain "run after #N" chain is unchanged and means `#N done`.

## Alternatives

- A rule table with one row per dependency, indexed by task: faster lookup, but more schema and
  cleanup for at most 32 waiting tasks per sandbox.
- JSON AST from the client: no parser, but unreadable on the wire, in the database and in the UI.
- A general expression language (comparisons, time, counts): more power than any request so far.

## Consequences

One small grammar to learn, shown back in canonical form. Rules can only look at tasks of the same
sandbox by number. New states or operators extend the domain module without a schema change.
