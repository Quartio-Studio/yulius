# 0006. One internal MCP server and hook protocol for coordination

## Status

Accepted. Recorded retroactively from SPEC-002 (ADR-07); the full analysis is in
[SPEC-002 section 2](../../.ai/specs/SPEC-002-yulius-architecture.md). Amended by
[0029](0029-cockpit-manages-through-mcp.md) (the Cockpit's `ck_` bearer and the project-management
tools acting for a person) and [0244](0244-task-runs-reach-yulius-through-a-stdio-bridge-and-are-woken-after-an-outage.md)
(task runs reach the listener through the `yulius mcp` stdio bridge).

## Context

Coordination (steps, specs, CI, delivery) must work the same for every harness and be least-privilege.

## Decision

Yulius MCP (Streamable HTTP, JSON-RPC) on a loopback listener with per-run bearer tokens; hooks reach the same listener through `yulius hook`.

## Alternatives

Per-harness plugins; injecting text into the TUI only.

## Consequences

Task tokens reach only task-scoped tools; one protocol to test.
