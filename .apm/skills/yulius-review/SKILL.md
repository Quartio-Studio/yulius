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

For the independent review of a task's change: follow the task briefing for the reviewer and model of the active harness.
A reviewer does not ask for changes because of CI alone: `delivery_merge` merges
only a head whose own Yulius CI run passed. When the review finds nothing to fix
and the head's run is still going, or failed in a way the change does not cause,
the verdict is approve and the report says what CI still has to do. A failure
the change causes (its own test, format or lint, or a contract its files break)
is a blocker.

Deliver findings ordered by severity, each with path/location, concrete trigger,
impact and suggested correction, marked own code (code or tests the change added
or modified) or pre-existing (on dev before the change, with the evidence). If no
actionable issue is found, say so and name validation limits. Do not automatically
edit code during a read-only review or post reviews/comments externally without
task authorization.

An own-code finding is fixed in the same PR, test-first, and never becomes a
follow-up issue: a test that cannot fail, a weak or missing assertion, a race or a
missing regression test included. Re-review until approval under the task's existing
authorization. Split unwieldy work within scope. Only a pre-existing bug, a flaky
test the change did not cause, or a cosmetic nit left after 3 rounds becomes
an issue, saying why it is outside the change ("present on dev before this change",
with the evidence). The PR answers every finding; deferring an own-code finding
makes your verdict request changes (`docs/engineering/workflow.md`, Review output).
Before asking for another round, answer each finding of the last report with the
test that fails without its fix (or why no test can), check every finding of
that report against the new head, and run the project's mutation command on the
lines of the fix where it can take them. Here: `scripts/dev mutants --base
<reviewed head>`.
After an approval, fix the remaining nits within 20 product lines and merge
without another round: `delivery_merge` with the approved head counts them
(tests and documents do not count). More than that, or a file leaving the
change, needs a new round. A usage limit can cut a reviewer off before its report: resume that same reviewer by its agent id, never start a new one for the same round. A reviewer runs at high effort, never max.

Mutation testing runs beside the review: when you ask for a pull request's first review, start the project's mutation command in the background and let the reviewer read meanwhile; when it ends, put its summary line in the spec's Evidence and fix each missed mutant test-first before the merge. After a later round's fix, run it on that fix's lines alone. A project without one undoes by hand the few lines its new tests are meant to hold and says what failed. The reviewer may undo a single line it doubts and runs no campaign of its own. Nobody writes a mutation script by hand. A test that cannot fail is still fixed in the same pull request (ADR-0141). Here the command is `scripts/dev mutants`.

Default to changed lines, one worker and 10 minutes. At 16 listed changed-line mutants, or for an authorization, persistence or process lifecycle change, allow touched-file scope, up to two workers and 30 minutes. Cap each mutant at 300 seconds and respect CPU and memory limits. Declare the risk and run its affected integration tests. Record the base, scope, trigger, budgets, baseline and every outcome, including misses, timeouts and unreached mutants. Use run-owned disk scratch for workspace copies and disposable build targets, and remove them when the command ends.
