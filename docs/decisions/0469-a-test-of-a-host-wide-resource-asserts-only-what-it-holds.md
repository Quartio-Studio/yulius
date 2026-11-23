# 0469. A test of a host-wide resource asserts only what it holds

## Status

Accepted for SPEC-637 (#1620).

## Context

`a_loopback_listener_keeps_a_port_reserved` bound an ephemeral loopback port, dropped the
listener, and asserted that `loopback_ports_idle` saw the port free on both `127.0.0.1` and
`[::1]`. On a shared host anyone can take a freed ephemeral port, and the family the test never
bound may be held all along. Under `nextest --stress-count 200` it failed 1 time in 200 alone,
139 times beside a process holding `[::1]` ports and 25 times beside one churning `127.0.0.1`
port-0 binds. Neither the product nor the test leaked a socket.

## Decision

A test that touches a resource shared by the whole host (loopback ports, and the same way PIDs,
`/tmp` names, the clock) asserts only what the test itself holds: that a resource it holds is
reported as taken. The free side and every other branch of the decision run against an injected
fake (`loopback_ports_idle_with` takes its `bind`). No retry, sleep or quarantine stands in for
that.

## Alternatives

- Retry the free check, or sleep before it: lowers the failure rate and keeps a test that can
  fail for reasons outside the code.
- Probe a fixed port below the ephemeral range: other tasks, gates and dev servers use those
  ranges too, so the same race remains.
- A private network namespace per test: real on Linux with the right privileges only, not on
  macOS (ADR-0169), and it tests the kernel rather than our rule.

## Consequences

The decision of the loopback probe is covered per address family without sockets, and the
real-socket test can no longer be falsified by a neighbour. A defect that only real sockets
show on the free side (the real binder keeping its listener) is not caught by a test; the
binder is one line that drops the listener at once.
