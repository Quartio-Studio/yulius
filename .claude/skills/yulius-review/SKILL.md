---
name: yulius-review
description: Review Yulius specifications or code for requirement fidelity, Rust boundaries and orchestration failure modes. Use for an explicit review or the final review of an implemented slice.
---

# Review a Yulius change

Follow the bundled skill `om-code-review` for the procedure, severities, the own-code or
pre-existing mark and the verdict. This skill adds what to look for in Yulius.

Read `AGENTS.md` and the applicable spec. Load `docs/engineering/rust.md` for code, and
`docs/engineering/runtime-contracts.md` only for affected process, state or I/O work.

Establish the intended behavior from the request or spec, then inspect the diff and callers. For
existing mechanisms, identify the guarantee being preserved under new default settings. Examine
cancellation and completion races, stale ownership, replay, bounded work, path and ref injection
and secret exposure where applicable. Check that tests cover failure triggers rather than only
the happy path. Follow shared contract construction sites and all resumed and new paths.

For a spec-only change, review testability, state exits, compatibility, unresolved choices and
simplicity; do not demand nonexistent implementation tests. Do not claim an unexecuted test
passed, edit code during a read-only review, or post reviews or comments externally without task
authorization.
