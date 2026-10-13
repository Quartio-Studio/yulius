# 0053. Shell and Cockpit starts run detached from their request

## Status

Accepted. Specified in
[SPEC-077](../../.ai/specs/SPEC-077-fix-250-cockpit-and-shell-starts-survive.md) (issue #250);
amends [0020](0020-cockpit-as-a-plain-shell-session.md) and SPEC-021's start lock.

## Context

A terminal start (`POST /api/sandboxes/{sid}/shells`) and a Cockpit start
(`POST /api/sandboxes/{sid}/cockpit`) spawn a detached ptyhost, attach to it, save the shell's
record and register it, all inside the request's future. Axum drops that future when the client
disconnects. Dropped after the spawn, a Cockpit's process kept running with its run directory but
was never registered, so the next open started a second Cockpit on the same conversation and a
restart adopted both; a terminal stayed in the registry with no process attached, listed but
unusable. The per-(user, sandbox) Cockpit start-lock map also kept one entry per pair forever.

## Decision

- The section of a start that can leave a half-started shell runs as a task spawned by the
  request and awaited by it (`App::run_detached`). A dropped request drops only the `JoinHandle`;
  the start ends registered with its pump, or discarded with its run directory, on the paths that
  already existed.
- A Cockpit's detached section begins at its start lock, so a second open waits for an in-flight
  start and then returns that Cockpit. A terminal's checks (name, capacity, folder) stay in the
  request; its detached section begins after its registration.
- The start lock is held through a guard that, when dropped, removes the map entry if no other
  start holds or waits for that lock (checked under the map's lock).

## Alternatives

- **A drop guard that discards an unregistered shell**: also bounded, but it throws away a start
  the user asked for and has to terminate a process synchronously from `Drop`; a reconnecting
  client would start again from nothing.
- **Register before the spawn (as terminals did)**: a dropped start still leaves an entry without
  a process; it moves the problem, it does not end the start.
- **Leave the lock map growing**: bounded by users × sandboxes, but needless state; the guard is
  a few lines.

## Consequences

- A start always reaches its end, also during a server shutdown, where the shell is then adopted
  after the restart like any running shell.
- One spawned task per start; a panic in it answers 500 instead of tearing down the connection.
- Tests hold a start between its spawn and its attach with a gated session host
  (`tests/support/gated.rs`) to drop the request or to prove other users are not delayed.
