# 0439. Delivery checks mutations by hand and requested campaigns build on disk

## Status

Accepted on 2026-11-21 for SPEC-600. Supersedes the per-pull-request and between-review campaign defaults of ADR-0347. ADR-0141's test-strength requirement continues to apply.

## Context

Mutation campaigns accounted for 105 minutes across 22 sessions. Workspace copies and alternate Cargo targets in private tmpfs mounts occupied 231 GiB across three sessions. The owner installed a by-hand policy on the instance and asked that updates preserve it.

## Decision

For every delivery undo the few lines the tests should hold, watch the affected tests fail, restore the lines and record what failed. A reviewer may undo a doubtful line. Fix a test that cannot fail in the same pull request. Run a project mutation command only when the task asks for it by name. Ship the instance wording in the catalog and bootstrap it into new projects. After a delivery merges on green Yulius CI, request staging once and end the task. A reported gate, deployment or verification failure calls for a fix forward.

Give each contained agent and CI job private scratch on its sandbox disk. Use separate command-owned build trees and reclaim them after completion or cancellation. Cap the session's ordinary tmpfs at 1 GiB.

## Alternatives

Change-size thresholds still launch workspace builds for deliveries. Shared writable targets can mix the baseline and mutant binaries. Increasing the permitted tmpfs size would keep build storage competing with session memory.

## Consequences

A write beyond tmpfs capacity returns ENOSPC. By-hand evidence identifies the behavior and the failing test. Requested campaigns retain the changed-line tool, its matched baseline and its explicit outcome accounting. Existing external repositories adopt the policy through their own tasks.
