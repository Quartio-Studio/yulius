# 0002. A detached ptyhost process owns every agent run

## Status

Accepted. Recorded retroactively from SPEC-002 (ADR-02); the full analysis is in
[SPEC-002 section 2](../../.ai/specs/SPEC-002-yulius-architecture.md).

## Context

Agent sessions must survive a restart of the web server (NFR-001).

## Decision

Each run is owned by a detached `yulius ptyhost` process speaking a Unix-socket protocol, with vt100 screen state and exit record files. The server reattaches after restart.

## Alternatives

tmux/dtach; PTYs inside the server process.

## Consequences

Sessions outlive deploys; ownership and identity are explicit; more code to test (covered by `tools/fake-agent`). The systemd unit needs `KillMode=process`.
