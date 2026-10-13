# 0040. CI waits for disk under the sandbox's shared quota

## Status

Accepted. Specified in [SPEC-059](../../.ai/specs/SPEC-059-fix-240-256-258-245-ci-waits-for-disk-in.md).
It extends [0017](0017-sandbox-disk-upkeep.md) (disk upkeep) and SPEC-008.

## Context

A sandbox has one soft disk quota (100 GB on the VM). It covers the task worktrees, the CI
checkouts, the home and the system layer. On 2026-10-12, six or seven tasks building Rust (9–35 GB
of `target/` each) kept the `yulius` sandbox at 100–151 GB. The quota check that guards new
sessions also refused every CI job, so each run ended `error` within seconds, was not retried once
space freed, and dropped armed merges (#240, #256, #258). Upkeep had also removed a working task's
`target/` while its background gate ran, because an unanswered shell probe read as "no shells"
(#245).

## Decision

- The quota stays one shared, soft budget per sandbox, and still refuses new sessions over it.
- A CI run over the quota waits in the queue instead of ending `error`. While it waits, Yulius
  prunes the caches of tasks that do not work (largest first), plus the CI checkouts, and measures
  again, at most once a minute. The run starts by itself once usage is under the quota. Its reason
  and commit status say it waits for disk, and name the largest consumers. A job refused after
  its run started puts the run back in the queue.
- Upkeep removes a running task's caches only after a fresh probe says that no background shell
  and no background subagent runs. An unanswered probe keeps them.

## Alternatives

- **A larger quota sized for N parallel Rust tasks.** About 20 GB per building task plus CI and the
  home: 200 GB or more for eight tasks. The host has the space, so a person can raise
  `--sandbox-disk-gb` (`YULIUS_SANDBOX_DISK_GB`) at any time. It needs no code, but a fixed size still overflows at N+1 tasks.
  It does not remove the failure mode, so it is an operations choice, not this decision.
- **CI outside the quota, with its own budget.** That would make CI immune to the agents. But the
  quota is the one measure of a sandbox's disk, and checkouts that grow unbounded beside it would
  fill the host instead. CI's own checkouts are already capped (a quarter of the quota per job).
- **One shared `CARGO_TARGET_DIR` per sandbox.** Cargo locks the build directory, so parallel
  tasks would wait for each other's builds. Branches would also thrash each other's artifacts.
  This is Rust-only, and Yulius manages projects in any language.
- **Erroring at once and retrying.** That is what failed: retries land while other tasks rebuild,
  and each error drops the armed merge.

## Consequences

- A pull request's CI can wait for a long time when live tasks hold the space. The wait is visible
  (run reason, `ci_status`, the commit's description) and names who holds the space. It ends by
  itself, through supersede or through a cancel.
- Idle tasks lose their build caches sooner under pressure and rebuild them cold. Working tasks
  and idle tasks with background work never lose them, so the quota can still be exceeded by
  working tasks alone. CI then waits for them.
