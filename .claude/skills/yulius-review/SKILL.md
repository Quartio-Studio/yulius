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
impact and suggested correction, marked own code (code or tests the change added
or modified) or pre-existing (on dev before the change, with the evidence). If no
actionable issue is found, say so and name validation limits. Do not automatically
edit code during a read-only review or post reviews/comments externally without
task authorization.

An own-code finding is fixed in the same PR, test-first, and never becomes a
follow-up issue: a test that cannot fail, a weak or missing assertion, a race or a
missing regression test included. The round limit does not excuse it; the author
keeps fixing, splits the change or asks a person. Only a pre-existing bug, a flaky
test the change did not cause, or a cosmetic nit left at the round limit becomes
an issue, saying why it is outside the change ("present on dev before this change",
with the evidence). The PR answers every finding; deferring an own-code finding
makes your verdict request changes (`docs/engineering/workflow.md`, Review output).
