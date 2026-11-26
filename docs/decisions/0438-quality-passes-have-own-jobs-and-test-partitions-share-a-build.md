# 0438. Quality passes have separate CI jobs and test partitions share a build

## Status

Accepted for the Yulius project. [SPEC-598](../../.ai/specs/SPEC-598-shorten-a-yulius-ci-run-parallel-quality.md)
The separate quality jobs stand. The shared test build is superseded: Since 2026-11-26 (audit L3 F1) one `test` job with 42 CPUs runs this repository's whole suite and then the doctests: the archive handoff is gone. Unpacking the 2.3 GB archive in each shard filled a bounded temporary directory (ENOSPC on `main`), and the split shortened no run while `e2e` was the full suite's longest job.

## Context

The production sample attributed about 154 seconds each to check and macOS, 115 seconds to lint and 71–275 seconds to docs, all on the critical path of one job. Two matrix checkouts compiled identical test binaries for about 195 seconds apiece. CI already owns per-job CPU budgets, checkouts, logs and cancellation.

## Decision

Give check, lint, macOS, docs and dependency analysis their own PR jobs. Keep the checker tests alongside native compilation. Keep both test matrix entries and e2e after merge. The first admitted test shard creates a locked, all-feature nextest archive. Both shards run that archive against their own checkout, with independent CPU budgets, cancellation and live diagnostics. Shard 1 runs doctests.

The two test checkouts of each CI lane share a lock and one archive slot in the sandbox's CI home. The protected job and a candidate definition have separate slots. The recorded run ID and SHA identify the archive. The producer publishes the finished archive by rename. Each consumer takes a hard link in its own cache before releasing the lock. The second completion frees the shared archive. If a build fails, its waiting peer skips. An infrastructure retry that succeeds after that skip runs the entire suite, including doctests, before reporting success. A subsequent run replaces an interrupted one’s files within the same bounded slot. The CI job timeout and process-group cancellation bound both the lock wait and the build.

## Alternatives

Parallel Cargo commands in one checkout contend for the same target lock. Giving each quality pass its own target directory would also require dividing a single job budget and combining concurrent output. Running both test partitions successively from one binary list saves compilation, but adds their execution times. One physical baseline shard spent 148 seconds executing its tests. A nextest archive keeps both shards runnable at the same time and carries the exact executable and metadata set across checkouts.

## Consequences

PR dashboards show each quality verdict. The test build occupies one existing shard slot. The other shard waits only for publication and can build first when admitted first, including on an instance with one slot. The quarantine listing and partition run extract the archive in the job’s disk-backed build scratch. A runner that supplies only its private job cache uses that directory for extraction. The gate removes the extracted files when it returns. On a cold tools cache, bootstrap also builds each pinned Cargo tool in a temporary target on the job’s disk or under the tools cache. Local tests and older branch checkouts still use the ordinary test command.
