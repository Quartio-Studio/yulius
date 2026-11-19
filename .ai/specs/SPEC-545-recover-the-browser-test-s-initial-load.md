---
yulius-task: 981593dc-56cc-4a0c-9ff6-d8ffacb5e012
status: in-progress
---

# SPEC-545 Recover the browser test’s initial load after a network change

## Goal

Let a mock browser test start when Chromium loses its initial page load to a network configuration change on the CI host.

## Context

Issue #1331 records failed script requests in runs `2e3cac12` and `f897cb92`. Chromium reported `net::ERR_NETWORK_CHANGED` and the mock never installed. The shared harness in `e2e/tests/support.ts` waits for installation after a single navigation. The slow-load cases in `mock-ready.e2e.ts` exercise delayed scripts that eventually arrive.

## Requirements

- **LOAD-001** Recover an initial mock page load interrupted by Chromium's exact network-change error for the app document or its startup scripts. Identify the recovery in run output.
- **LOAD-002** Make at most one additional navigation, preserving the original route and mock options. Return once the mock is installed.
- **LOAD-003** Surface other failures and a failed recovery. Keep later test steps, explicit reloads and the runner's zero test retries intact.

## Plan

- [x] Trace the interrupted initial asset load
- [x] Prove interrupted opens and ordinary failures in the harness
- [ ] Recover the initial mock load once
- [ ] Review the recovery and merge on green CI

## Acceptance criteria

- An interrupted initial asset load prevents the original harness from reaching mock readiness. With recovery, the same test reaches the requested page with the mock installed.
- A second interruption surfaces after two loads. An ordinary failure surfaces after one. The test body executes once.

## Test plan

Use the archived CI trace and an isolated Chromium network namespace to exercise the host event. Add deterministic coverage of the qualifying browser error, the one-recovery limit and ordinary failures. Run the mock-ready suite, affected browser flows, quick checks, security command and Yulius CI.

## Decisions

Observe Chromium's request failure during mock startup. The exact failure code identifies the interrupted load where the readiness timeout alone cannot distinguish a broken app from a host network change.
