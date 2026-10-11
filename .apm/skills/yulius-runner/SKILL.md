---
name: yulius-runner
description: Design, implement or debug Yulius coding-agent adapters, subprocess supervision, cancellation and event replay. Use when crossing provider or operating-system execution boundaries.
---

# Work on a provider or supervisor boundary

Read `AGENTS.md`, the affected spec, `docs/engineering/rust.md` and
`docs/engineering/runtime-contracts.md`. This skill applies to future runtime
work; the current checkout has only engineering tooling.

Establish a provider/version capability table from primary wire documentation or
redacted real fixtures before defining a mapper. Distinguish simulated fixtures
from observed transcripts. Record absent capabilities; never fill missing usage,
completion or approval signals with invented values.

Keep the provider wire parser at the adapter boundary. Define frame/output bounds,
normalized sequence/correlation IDs, delta-versus-snapshot semantics and durable
replay. Test malformed/partial/unknown frames, duplicates, disconnect and resume.

For processes, draw ownership from spawn through cleanup: argv/cwd/environment,
pipe readers, task handles, cancel signal, grace deadline, escalation, reaping,
lease release. Identify every terminal/parked state and its actual exit trigger.
A timeout or delivered signal does not prove exit; dropping a future does not
clean a process tree. Use a deterministic fake executable for failure scenarios.

Test spawn failure, ignored termination, descendants, output flooding, races and
restart reconciliation. Confirm no shell interpolation, secret leakage, host
execution disguised as containment, or deletion of unowned worktrees.

Run targeted contract tests and `scripts/dev ci`. Report exactly which provider
versions/transports were verified and which need a separately authorized live
smoke test. Updating a mapper without verified wire evidence is not full parity.
