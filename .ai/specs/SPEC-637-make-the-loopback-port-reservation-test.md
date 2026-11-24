---
yulius-task: 0eada8c4-d80b-4c06-9fc3-dffdbe96dd01
status: done
---

# SPEC-637 The loopback port reservation test asserts only what it holds

## Goal

`wiring::session::tests::a_loopback_listener_keeps_a_port_reserved` (#1620) no longer fails
when another loopback user on the shared host takes or holds the port it just released. The
probe's decision is tested for every address family without real sockets, and the real-socket
test checks only the direction no other process can change.

## Context

Symptom (#1620): during a sequential warm-worktree `scripts/dev test` at `7fe27a3d6`, the test
failed at its check "free again after the drop" (then `session.rs:463`, on dev b1b50218f
`crates/yulius/src/wiring/session.rs:530`); 5,751 of 5,752 tests passed and the paired cold
run passed. The test came with eef33c931 (PR #1488) and has not changed since; nextest runs
with `retries = 0`.

### Root cause

A defect of the test, not of the product:

1. The test binds port 0, so the kernel gives it a port from the ephemeral range shared by the
   whole host (`ip_local_port_range` 32768-60999 here). Once it drops the listener, any process
   may take that port: another nextest process, another worktree's gate, an outgoing connection.
2. `PtySessionHost::loopback_ports_idle` binds each port on both `127.0.0.1` and `[::1]`. The
   test reserved only one of them, so the other address may be held by someone else the whole
   time, before and after the drop.

So "free again after the drop" claims something about the host that the test does not own.
Neither the test's listener nor the probe's own short-lived binds leak.

### Reproduction

`target/issue-1620/repro.sh` (not committed: a reproduction aid) runs the test under
`nextest --stress-count 200 --no-fail-fast` alone and beside each of two competitors: a process
holding 10,000 `[::1]` ports from port-0 binds (the family the test does not hold), and one
binding `127.0.0.1` port 0 in a loop while keeping the last 2,000 (a same-family taker after the
drop). On dev b1b50218f, failures at `session.rs:530` per 200 iterations: alone 1, beside the
`[::1]` holder 139, beside the `127.0.0.1` churn 25. The single failure alone shows that this
sandbox's ordinary loopback traffic (the Yulius server on `127.0.0.1:8081`, Claude Code's
listeners) is enough.

### Product reach

The product calls the probe only for a removed sandbox's retired port range
(`crates/yulius-app/src/projects.rs:751`), and those ranges start at `PORT_RANGE_START` = 20000,
below the ephemeral range; unknown keeps the range reserved (the trait's default is `false`). A
port that someone else holds is correctly not idle. The product behaviour is right and stays as
it is.

## Requirements

- LPR-001 The probe's decision is one function over an injected bind: for each port of the range
  it binds `127.0.0.1` and then `[::1]`; the range is idle only when every IPv4 bind succeeds and
  every IPv6 bind succeeds or fails with `AddrNotAvailable` or `Unsupported` (a host without
  IPv6 loopback). Any other failure keeps the range reserved.
- LPR-002 Unit tests drive that function with a fake binder: every port free → idle, with each
  port bound on both families in order; IPv4 busy → reserved; IPv6 busy → reserved; IPv6
  `AddrNotAvailable` or `Unsupported` → idle; another IPv6 error → reserved; an IPv4 error of
  any kind → reserved; one busy port anywhere in a range → reserved.
- LPR-003 The real-socket test checks only that a listener the test holds, on either loopback
  family, makes the probe report the port reserved. It skips IPv6 where the host has no IPv6
  loopback, as before. No retries, no sleeps.
- LPR-004 `PtySessionHost::loopback_ports_idle` keeps its behaviour; it is the decision function
  with a binder that binds a `TcpListener` and drops it at once. A real-socket test holds its
  free side without a port anyone else can take: `0..=0` asks the kernel for any free port on
  each family, so it reads idle (review round 1).

## Plan

- [x] Spec the loopback probe flake with its root cause
- [x] Reproduce the failure beside a competing loopback port holder
- [x] Probe decision takes an injected binder, unit-tested per address family
- [x] Real-socket test checks only the held direction, proven under stress
- [x] ADR, changes entry and spec Evidence
- [x] Independent review approves the exact head

## Acceptance criteria

- The old test is shown to fail beside a competing loopback user (Reproduction above).
- The new unit tests and the real-socket test pass under `nextest --stress-count` alone and
  beside both competitors.
- Each new decision test fails when the line it holds is undone (mutation by hand).
- `crates/yulius`'s lib test binary passes in full; Yulius CI is green on the pull request head.

## Test plan

- `scripts/cargo nextest run -p yulius --lib -E 'test(/wiring::session::tests::/)'` and the whole
  lib binary.
- `target/issue-1620/repro.sh new 'test(/loopback/)' 200`: alone, beside the `[::1]` holder,
  beside the `127.0.0.1` churn.
- Mutations by hand: accept any IPv6 error; ignore IPv6; ignore IPv4; check only the range's
  first port.

## Decisions

- Test the decision through an injected binder instead of real sockets in a namespace of its own:
  a private network namespace would make the release direction real too, but is Linux-only
  (macOS is a supported development host, ADR-0169), needs privileges the test process may lack,
  and would test the kernel, not our rule. Retries or a sleep before the second check would only lower the rate.
- Keep the real-socket test for the held direction: it proves the real binder reaches the real
  sockets, which a fake cannot, and no other process can free a port the test holds.
- Recorded as [ADR-0469](../../docs/decisions/0469-a-test-of-a-host-wide-resource-asserts-only-what-it-holds.md): a test of a host-wide resource asserts only what the test itself holds.

## Evidence

Host: this sandbox, `ip_local_port_range` 32768-60999, 128 CPUs. Logs in the worktree's ignored
`target/issue-1620/`.

- Reproduction on dev b1b50218f (before the fix): `target/issue-1620/repro.sh old
  'test(a_loopback_listener_keeps_a_port_reserved)' 200` (nextest `--stress-count 200
  --no-fail-fast`). Failed iterations, all at `session.rs:530`: alone 1/200, beside the `[::1]`
  holder 139/200, beside the `127.0.0.1` churn 25/200, alone again afterwards 1/200. An earlier
  run right after killing a 20,000-port holder failed 200/200 while that process was still
  closing its sockets, a further sign of the same cause (not counted).
- After the fix: `target/issue-1620/repro.sh new
  'test(/wiring::session::tests::(a_loopback|a_range|a_busy|a_host_without|another_ipv6|any_ipv4)/)' 200`
  (the 7 probe tests per iteration): alone 200/200 passed, beside the `[::1]` holder 200/200,
  beside the `127.0.0.1` churn 200/200, afterwards 200/200.
- `scripts/cargo nextest run -p yulius --lib`: 702 tests run, 702 passed.
- Mutations in `loopback_ports_idle_with`, each restored with `git checkout` and `touch`. These
  five were applied by a small ignored helper (`target/issue-1620/mutate.py`, one exact-string
  replacement each), not by hand as AGENTS.md asks; the review read their logs. Failing tests
  per mutation:
  - any IPv6 error counts as free: `another_ipv6_failure_keeps_the_range_reserved`,
    `a_busy_ipv6_port_keeps_the_range_reserved`, `a_loopback_listener_keeps_a_port_reserved`;
  - a missing IPv6 loopback counts as busy: `a_host_without_ipv6_loopback_decides_on_ipv4`;
  - IPv4 ignored: `a_busy_ipv4_port_keeps_the_range_reserved`,
    `any_ipv4_failure_keeps_the_range_reserved`, `a_host_without_ipv6_loopback_decides_on_ipv4`,
    `a_loopback_listener_keeps_a_port_reserved`;
  - IPv6 ignored: `a_busy_ipv6_port_keeps_the_range_reserved`,
    `a_range_free_on_both_loopback_addresses_is_idle`,
    `another_ipv6_failure_keeps_the_range_reserved`, `a_loopback_listener_keeps_a_port_reserved`;
  - only the range's first port checked: `a_busy_ipv4_port_keeps_the_range_reserved`,
    `a_busy_ipv6_port_keeps_the_range_reserved`, `a_host_without_ipv6_loopback_decides_on_ipv4`,
    `a_range_free_on_both_loopback_addresses_is_idle`.
- Review round 1 (head 698d34cd2, request changes): after the change, no test held the real
  probe's free side. The reviewer's mutant, a binder that never succeeds (`.and_then(|_|
  Err(AddrInUse.into()))`), passed all 702 lib tests. Added
  `the_real_binder_reports_a_free_loopback_port_idle` (`loopback_ports_idle(0..=0)`). With the
  mutant applied by hand: 12 session tests run, 11 passed, that one failed. Restored by hand:
  all pass. The nit (a closure binding that shadowed `failing`) is renamed to `held`.
- After round 1: `target/issue-1620/repro.sh r1` over the 8 probe tests, 200 iterations each,
  alone, beside the `[::1]` holder, beside the `127.0.0.1` churn, and afterwards: 200/200
  passed in all four. `scripts/cargo nextest run -p yulius --lib`: 703 passed.
- Not tested: on real sockets, that the binder releases a port it bound (a dropped listener
  frees a port that anyone may take at once; ADR-0469). Also not on real sockets: a host
  without IPv6 loopback, because this host has `[::1]`; the fake host covers both error kinds.
- Review round 2 (yulius-reviewer, head 8643dda05): approve. The reviewer reapplied the round-1
  mutant by hand and the lib binary gave 703 run, 1 failed, exactly the new test; restored, 703
  passed. The two real-socket tests passed 300/300 under `--stress-count 300`. Its one nit, the
  outdated counts in the PR body, is fixed in the body.
