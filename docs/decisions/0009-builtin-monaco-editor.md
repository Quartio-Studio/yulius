# 0009. Built-in Monaco editor over a file API

## Status

Accepted. Recorded retroactively from SPEC-002 (ADR-10); the full analysis is in
[SPEC-002 section 2](../../.ai/specs/SPEC-002-yulius-architecture.md).

## Context

Per-user openvscode-server was heavy, outside containment and too complex.

## Decision

Vendored Monaco in the web UI over a small file API on the sandbox repository or a task worktree.

## Alternatives

openvscode-server per sandbox or per user; CodeMirror.

## Consequences

No editor process on the VM; no extensions or debugger; 14 MB of static assets.
