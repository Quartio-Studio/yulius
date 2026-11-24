# 0474. A test pause hook belongs to the runtime that registered it

## Status

Accepted for SPEC-645 (#1644).

## Context

Test pause points (`yulius_app::race`, ADR-0174, SPEC-250) keep their hooks in one list for the
whole process, matched by point and by a key: a path, or a task id for about 30 points. Path keys
are each test's own temporary folders, but task ids repeat: every `agent_hooks.rs` fixture names
its task `t-main`, other files use `t1`. The integration tests are one binary (ADR-0028). nextest
runs each test in its own process; libtest (`cargo test`) runs them as threads of one process.
There, a test's parked read of its own `t-main` took another test's `ParkedStatusRead` hook and
was held, while that test's read ran unpaused, so
`parked_mcp_explains_a_hold_dismissed_during_the_read` and
`parked_mcp_reads_classify_the_plan_they_return_and_honor_a_new_decision` failed beside a busy run.

## Decision

`race::on` records the tokio runtime it is called on (`Handle::id`), and while that runtime lives,
`race::reached` runs the hook only for a point reached on it, a worker or that runtime's blocking
pool. Each `#[tokio::test]` builds its own runtime, so a running test's hooks are its own whatever
its keys. Whether the runtime lives is read from a task `race::on` spawns on it, which never ends
and holds a token: the runtime's shutdown drops it. A hook whose runtime shut down matches by
point and key alone, so a test that restarts its server on a new runtime (the `task_done.rs`
restarts: one runtime per server life, the first shut down before the second starts) still meets
the hooks it registered before the restart. A hook registered outside a runtime, or a point
reached outside one (a plain `std::thread`), matches by point and key alone too, as before.

## Alternatives

- A unique task id per fixture: fixes only the fixtures changed, every new fixture can repeat the
  mistake, and many tests assert on the literal ids.
- Key the points by an app instance (its data folder plus the task id): changes about 30 reach
  sites and every test that holds them, for the same effect.
- Scope strictly to the runtime: the restart tests' hooks were never run on the second runtime
  (`task_done::a_worker_asking_during_reattachment_keeps_its_flag` failed).
- Run the binary only under nextest: AGENTS.md and agents still run `cargo test` filters, and the
  failure there looks like a product bug.

## Consequences

Running tests of one process cannot take each other's pause hooks through the same key. A test
that keeps two runtimes alive at once and reaches a hook on the one that did not register it
(only the #1644 neighbour tests do, on purpose) must register it there or outside any runtime. A
point reached on a plain thread, and the leftover hook of a finished test, can still run with any
test's key, as before. Each registration inside a runtime leaves one idle task on it until it
shuts down.
