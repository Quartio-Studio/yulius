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
- [x] Recover the initial mock load once
- [ ] Review the recovery and merge on green CI

## Acceptance criteria

- An interrupted initial asset load prevents the original harness from reaching mock readiness. With recovery, the same test reaches the requested page with the mock installed.
- A second interruption surfaces after two loads. An ordinary failure surfaces after one. The test body executes once.

## Test plan

Use the archived CI trace and an isolated Chromium network namespace to exercise the host event. Add deterministic coverage of the qualifying browser error, the one-recovery limit and ordinary failures. Run the mock-ready suite, affected browser flows, quick checks, security command and Yulius CI.

## Decisions

Observe Chromium's request failure during mock startup ([ADR-0385](../../docs/decisions/0385-recover-an-interrupted-initial-mock-load-once.md)). The exact failure code identifies the interrupted load where the readiness timeout alone cannot distinguish a broken app from a host network change.

## Evidence

- On dev `8d05c25bf`, a disposable Linux network namespace served the real UI on loopback. The server held six script requests, then added `192.0.2.42/24` to its dummy interface. The test timed out after 30 seconds waiting for the mock. Its trace recorded `net::ERR_NETWORK_CHANGED` for startup modules including `/assets/pages/pipeline.js`. With the fix, the same probe logged one recovery and passed in 1.46 seconds.
- The test-first commit covers intercepted entry-script and document requests. Before recovery, the interrupted script left the mock missing, repeated interruption made only one request, and navigation to the interrupted document rejected. The fixtures abort real browser requests and supply the Chromium network-change code on the Playwright request because interception cannot select that particular abort reason.
- During implementation, the document case caught Chromium's error-page commit cancelling a newly issued navigation. Recovery now waits for that commit before reopening the original URL.
- `npx e2e run tests/mock-ready.e2e.ts` (in a focused run) passed all three readiness checks. The two originally failing cases in `drafts.e2e.ts` and `filedrop.e2e.ts` passed.
- The repeated-interruption case exposed a runner evaluation still recorded as pending when the last readiness sample met the poll deadline. The bounded poll now reads the page directly through the worker's Playwright surface.
- `npx e2e run tests/initial-load.e2e.ts tests/mock-ready.e2e.ts` passed all eight tests. The five recovery cases cover the initial document, the entry script, the one-recovery cap, an ordinary reset, and subsequent navigation and reload. The real-interface probe also passed with this version in 2.10 seconds.
- `scripts/dev quick` passed the repository contracts and Rust format check.
