# 0055. A sandbox's counts are the sidebar's groups, decided by one domain rule

## Status

Accepted. Extends the "needs you" rule of #147/#167 (SPEC-001 UI-050); specified in
[SPEC-076](../../.ai/specs/SPEC-076-fix-170-home-cards-working-and-queued-co.md).

## Context

Home's cards and the sandbox tabs show three numbers per sandbox: needs you, working and queued.
#167 made "needs you" follow the sidebar (`taskInfo` in the web UI, `AttentionFacts::needs_you` on
the server). "Working" still counted live processes (`TaskStatus::is_live`) and "queued" only
`queued` tasks, while the sidebar's Working group leaves out "Your turn" and Done tasks and holds
paused and chained ones. A card could say "5 working" beside a sidebar "Working 6" (#170). The
server also counted a task being stopped as needing you when a prompt was held, which the sidebar
lists with the ended tasks.

## Decision

The domain decides a task's sidebar group in one function, `AttentionFacts::group`, returning a
`TaskGroup`: Needs you, Working, Queued (the Working group's queued and chained tasks), Done or
Ended. It mirrors `taskInfo`, and `needs_you` is `group() == NeedsYou`. The sandbox view counts
`needs_attention`, `running` and `queued` from it, so `running` + `queued` is the sidebar's Working
count. The web UI's mock counts with `taskInfo` and `counter` in `status.js`. The wire names stay.

## Alternatives

- Rename `counts.running` to `working`: clearer, but breaks any reader of the field for no user
  gain; the documentation says what it counts and the status page's `live_sessions` counts
  processes.
- Have the web UI count from its own task lists instead of the server: Home does not load every
  sandbox's tasks and pull requests, and the window title and tabs need the numbers at once.
- A second app-level function for working and queued beside `needs_you`: two places to keep in step
  with `taskInfo`, the disagreement this decision removes.

## Consequences

- A change to the sidebar's grouping changes `taskInfo` and `AttentionFacts::group` together; the
  e2e test in `consistency.e2e.ts` and the integration test in `delivery.rs` catch a drift.
- "Working" on a card no longer means "a process is alive": a task waiting for you or Done is not
  counted there, and a paused or chained one is.
- The pull request facts come from the last Git overview read, as for "needs you": until it is read
  after a start, a finished turn whose checks run counts as needing you rather than working.
