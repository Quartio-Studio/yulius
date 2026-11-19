---
yulius-task: e9bfc4ce-4926-40ed-b8d3-0f6160bc7b70
status: in-progress
---

# SPEC-522 Compare harnesses on the sandbox Dashboard

## Goal

A sandbox operator can compare how much work each harness handled, how long its runs lasted and how much token usage it reported in the same period.

## Context

The Overview already reads the bounded, permission-scoped analytics summary for tasks created in an inclusive UTC date range. Its model and subscription charts do not expose the task and run harness breakdowns. Each task has a current harness and each attempt records its actual harness.

## Requirements

- HDA-001 Extend the existing per-harness run breakdown with a finished-attempt count, median and nearest-rank p90 wall milliseconds. Exclude live attempts from this distribution and retain their workload and reported usage. An empty distribution has null percentiles.
- HDA-002 Add a Harnesses tab to the sandbox Dashboard. Compare assigned tasks, attempts, finished attempts and their elapsed wall time side by side. Include harnesses appearing only in tasks or only in attempts.
- HDA-003 Compare known input, output and cache tokens separately, with the number of attempts reporting any usage out of all attempts. Distinguish a measured zero from missing telemetry.
- HDA-004 Support bookmarkable 7-, 30- and 90-day periods, scoped to this sandbox, refresh and retry. Show an empty cohort and flag a truncated cohort.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Measure finished run durations by executing harness
- [x] Compare harness workload, run times and token coverage on the Dashboard
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

1. Two ended attempts of the same task on one harness count separately. A live attempt does not move its percentiles. A different harness with only live runs has no percentiles (HDA-001).
2. From Overview the operator opens Harnesses and compares the named harnesses, including assigned tasks without runs. A run's tokens and elapsed time belong to its executing harness (HDA-002, HDA-003).
3. A partially reporting cohort shows coverage, separate token categories and a dash for unknowns. Empty and capped cohorts have explicit states. Changing the range and reopening its address restores the selection (HDA-003, HDA-004).

## Test plan

Use synthetic run records in the summary test and browser tests through the real Dashboard against the web mock. Exercise navigation, period selection, missing and zero values, empty and truncated data. Run repository quick checks, web e2e, security scan and Yulius CI.

## Decisions

Reuse the analytics summary and its existing authorization, work bound and creation-day cohort. Compare durations of ended attempts so a live run does not skew the sample. Wall time includes idle time. See [ADR-0374](../../docs/decisions/0374-compare-harnesses-by-executing-attempt.md).

## Evidence

- Before implementation, the Rust assertion for the finished count received null. Both browser scenarios failed to find the Harnesses tab.
- `cargo test --locked -p yulius-app --lib analytics::tests` passed all seven tests. Two closed Codex attempts of 0 and 400 ms produced a median of 200 ms and a p90 of 400 ms. Two live attempts across two harnesses contributed no duration samples.
- `npx e2e run tests/harness-comparison.e2e.ts` passed all three browser scenarios, including date restoration, missing and measured-zero telemetry, empty and capped cohorts, retry, 360 px layout and an accessibility check.
- Independent review round 1 found that two unknowns in one row could hide a swapped column. An added row reports 17 input, 23 output, 42 cache-read and 64 cache-write tokens. In an isolated web copy the test failed when Output and Cache read were swapped. On the feature branch all three scenarios passed.
- `scripts/dev quick` passed. `scripts/dev security` exited 2 because Vidoc refused the instance key (HTTP 401).
