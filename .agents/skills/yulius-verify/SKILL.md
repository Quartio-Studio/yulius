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

For every change, after committing and before the review, run `scripts/dev security`. Vidoc scans
the branch through the agent secret `VIDOC_API_KEY` (ADR-0104). If it reports findings, fix them
test-first. A false positive is ignored in Vidoc with the reason only after the independent
reviewer agrees, and it is listed in the spec's Evidence. Exit 2 means unrun: if `secrets_list`
shows no bound host, request `api.vidoc.dev` with `secret_request_hosts` and report the scan as
unrun, never as passed. When `.config/vidoc.toml` sets `enabled = false` (ADR-0107), the gate
prints "Security scan disabled" and exits 0: report the scan as disabled, never as passed.
Yulius CI's `security` job runs the same gate on every pull request head (ADR-0116): its findings
fail the check. When a scan cannot run (#470) its exit 2 ends the job **skipped** ("security scan
skipped", ADR-0133): read the job's output, and report that scan as unrun, never as passed.

If a command fails, distinguish missing prerequisite/network failure from product
or test defects. Record the exact failing command and output location. Repair
within the task scope and rerun affected gates; do not ignore failures, add blind
retries or broaden policies. Missing tools mean unverified, not success.

Before completion inspect the diff, verify acceptance against actual results,
and update evidence/state. Review findings are own code or pre-existing
(`docs/engineering/workflow.md`, Review output): every finding in code or tests the
change added or modified is fixed and proven test-first in the same PR and never
becomes a follow-up issue, whatever the round limit (keep fixing, split the change or
ask a person), and the PR answers every finding; only a
pre-existing bug, a flaky test the change did not cause or a cosmetic nit left at
the round limit becomes an issue that says why it is outside the change. An
own-code finding deferred to an issue is unresolved: the review verdict is request
changes and the change is not done. Report passed, failed and unrun checks separately.
Yulius CI is unrun until its run for the exact head exists; local equivalence is not
a claim that `yulius/ci` passed. Leave product acceptance open if its evidence
is absent, even when the harness passes.

For published delivery, also verify the required PR check and the `yulius/ci` status
of the exact post-merge dev commit per `docs/engineering/git.md`. Local green checks do
not replace that post-merge gate.
