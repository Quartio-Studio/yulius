---
yulius-task: a4a6603c-af95-4f93-8c03-fee246e93ef4
status: in-progress
---

# SPEC-610 Run repository file readers before merge

## Goal

Fixes #1652.

Catch stale assertions about repository content on the pull request and its merge train while keeping the full Rust and browser suites on dev.

## Context

PR #1645 changed the composer mock. The merge train passed and dev revision 13a632c40 failed one of 2,986 tests because the Antigravity composer assertion still expected the old line. The owner measured about 2.8 minutes for the train. [SPEC-598](SPEC-598-shorten-a-yulius-ci-run-parallel-quality.md) records the independent CI jobs. Rust contract tests open repository paths at runtime or embed them at compile time. Several tests share a directory reader.

## Requirements

- R1 Run all UI contract cases for any web tree change. Run the Rust text contracts for catalog and documentation changes, including readers in other crates.
- R2 Derive or verify coverage from test inputs. Cover edits, additions, deletions and both sides of renames. Keep unrelated full-suite work after merge.
- R3 Tell task agents to run every test in each test binary they touch.
- R4 Measure focused execution and the actual PR gate against the reported train. Request staging once after green delivery.

## Plan

- [x] Identify repository file readers and specify the pre-merge contract
- [x] Run the affected contract tests in the pull request gate
- [ ] Require whole touched test binaries in task briefings
- [ ] Verify reader coverage, timing and delivery evidence
- [ ] Approve the change for delivery to dev

## Acceptance criteria

A web-only mock edit executes the composer assertion and its siblings. A catalog or documentation edit executes its text readers. A newly introduced reader is selected or makes the coverage check fail. An unrelated edit avoids the Rust reader build. A rendered briefing includes the whole-binary instruction.

## Test plan

Exercise native job declarations and source discovery with real paths and synthetic readers. Exercise failure propagation with the reported stale composer assertion. Time the focused checks, run the touched binaries and quick checks, and record the CI and merge verdicts.

## Decisions

Use the existing path-filtered PR gate. It owns the checkout, timeout, exit status and candidate coverage, including outside PRs. Adding execution to delivery_merge would duplicate those responsibilities and miss independent CI runs. Group readers of shared inputs conservatively. No persistence or API changes are needed.

## Evidence

- On base 13a632c40, the initial web-path selection ran all 12 ui_contract tests. Eleven passed and the reported Antigravity assertion failed. Nextest reported 0.473 s of test execution. The first build in this worktree took 3m10s. PR #1651 then corrected the assertion on dev at 1f9314339.
- The Python regressions first failed on the absent job and selector. A new source-only reader, a workspace-root helper, a concatenated compile-time path and chained directory joins exercise discovery without adding test names to the implementation. A separate test-only helper with unknown callers fails the coverage check. A real Git fixture covers removal, rename and an unavailable base. The native path check compares the job globs with discovered tracked inputs. All seven cases passed.
- An unrelated Rust edit returned CI's skip code 78 in 0.43 s, before bootstrap or Cargo.
- The briefing assertion failed on the old rendered instructions. Running the entire domain test binary also caught three section-budget regressions in the first wording. The shorter discipline paragraph preserves the existing budget.
