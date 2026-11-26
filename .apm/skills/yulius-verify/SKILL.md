---
name: yulius-verify
description: Verify Yulius changes against requirements and run the repository Rust and integrity gates. Use for completion checks, CI failures or validation; reports missing evidence without faking green.
---

# Verify a change

Read `AGENTS.md`, the affected spec and `docs/engineering/development.md` (commands, the security
scan and its exit codes). Which checks to run when is the bundled skill `delivery-pipeline`'s.

Derive a requirement and evidence checklist from the user's scope before examining passing tests.
Map each behavior to direct proof; a green compilation cannot prove cancellation, recovery or a UI
flow. For isolated prose changes run `python3 scripts/check_repository.py`. Ordinary checks must
not need model credentials or production infrastructure.

If a command fails, distinguish a missing prerequisite or network failure from product or test
defects. Record the exact failing command and output location. Repair within the task scope and
rerun the affected checks; do not ignore failures, add blind retries or broaden policies. Missing
tools mean unverified, not success.

Report passed, failed and unrun checks separately: a scan that ended unrun, incomplete or disabled
is not passed, and Yulius CI is unrun until its run for the exact head exists. Before completion
inspect the diff, verify acceptance against actual results and update the spec's evidence. Leave
product acceptance open when its evidence is absent, even when the harness passes.
