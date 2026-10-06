---
name: yulius-review
description: Review Yulius specifications or code for requirement fidelity, Rust boundaries and orchestration failure modes. Use for an explicit review or the final review of an implemented slice.
---

# Review a Yulius change

Read `AGENTS.md`, the applicable spec and `docs/engineering/workflow.md`. Load
Rust rules for code and runtime contracts only for affected process/state/I/O work.

Establish the intended behavior from the request/spec, then inspect the diff and
callers. For existing mechanisms, identify the guarantee being preserved under
new default settings. Examine cancellation/completion races, stale ownership,
replay, bounded work, path/ref injection and secret exposure where applicable.
Check that tests cover failure triggers rather than only the implementation's
happy path. Follow shared contract construction sites and all resumed/new paths.

For a spec-only change review testability, state exits, compatibility, unresolved
choices and simplicity; do not demand nonexistent implementation tests. For code,
check its requirement evidence and run scoped verification when useful. Do not
claim an unexecuted test passed.

Deliver findings ordered by severity, each with path/location, concrete trigger,
impact and suggested correction. If no actionable issue is found, say so and name
validation limits. Do not automatically edit code during a read-only review or
post reviews/comments externally without task authorization.
