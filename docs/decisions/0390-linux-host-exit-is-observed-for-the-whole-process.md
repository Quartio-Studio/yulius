# 0390. Linux host exit is observed for the whole process

## Status

Accepted for SPEC-550. Refines the exit proof in ADR-0050.

## Context

A detached host can linger after it reaps its child and publishes the child's exit record. On Linux its main thread can also become a zombie while another thread still runs. Either observation can precede exit of the host process.

## Decision

Explicit termination reads the process identities in the status record. Linux liveness checks use a process PID file descriptor to observe exit of the last thread. The existing identity checks guard signal delivery and the existing two-second deadline bounds the wait.

## Alternatives

The child's exit record describes the agent's outcome. Checking the main thread's state covers a single-threaded zombie but can miss a surviving worker. A restarted server cannot reap a host that another process spawned.

## Consequences

A fully exited host can be recognized before its parent reaps it. If Linux cannot confirm exit, a signalable process continues to count as live and a surviving target fails termination. Linux added process PID file descriptors in kernel 5.3. [Linux pidfd_open(2)](https://man7.org/linux/man-pages/man2/pidfd_open.2.html)
