# 0006. One internal MCP server and hook protocol for coordination

## Status

Accepted. Recorded retroactively from SPEC-002 (ADR-07); the full analysis is in
[SPEC-002 section 2](../../.ai/specs/SPEC-002-yulius-architecture.md).

## Context

Coordination (steps, specs, CI, delivery) must work the same for every harness and be least-privilege.

## Decision

Yulius MCP (Streamable HTTP, JSON-RPC) on a loopback listener with per-run bearer tokens; hooks reach the same listener through `yulius hook`.

## Alternatives

Per-harness plugins; injecting text into the TUI only.

## Consequences

Task tokens reach only task-scoped tools; one protocol to test.
