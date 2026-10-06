---
name: yulius-verify
description: Verify Yulius changes against requirements and run the repository Rust and integrity gates. Use for completion checks, CI failures or validation; reports missing evidence without faking green.
---

# Verify a change

Read `AGENTS.md`, the affected spec and `docs/engineering/development.md`.

Derive a requirement/evidence checklist from the user's scope before examining
passing tests. Inspect current Git state and code. Map each behavior to direct
proof; a green compilation cannot prove cancellation, recovery or a UI flow.

For Rust/tooling changes run `scripts/dev ci`. For isolated prose changes run
`python3 scripts/check_repository.py`. Run additional integration, property,
protocol or UI checks only when the change's behavior requires them. Ordinary
checks must not need model credentials or production infrastructure.

If a command fails, distinguish missing prerequisite/network failure from product
or test defects. Record the exact failing command and output location. Repair
within the task scope and rerun affected gates; do not ignore failures, add blind
retries or broaden policies. Missing tools mean unverified, not success.

Before completion inspect the diff, verify acceptance against actual results,
and update evidence/state. Report passed, failed and unrun checks separately.
Hosted CI is unrun until a real workflow result exists; local equivalence is not
a claim that GitHub checks passed. Leave product acceptance open if its evidence
is absent, even when the harness passes.

For published delivery, also verify the required PR checks and exact post-merge
push runs on dev and main per `docs/engineering/git.md`. Local green checks do
not replace either post-merge gate.
