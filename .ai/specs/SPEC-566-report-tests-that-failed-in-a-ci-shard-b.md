---
yulius-task: b3816b60-8bc9-472d-ba69-3ca74df8ffc4
status: in-progress
---

# SPEC-566 Report test failures observed before CI stopped a sibling

## Goal

Fixes #1506.

An agent reading a failed PR or merge-candidate notice can act on test failures already observed in sibling jobs stopped by fail-fast. The CI run details provide the stored diagnostics for every reported job.

## Context

The supplied audit found five quartio-v2 runs with 4, 1, 38, 4 and 1 distinct test names observed in stopped siblings. PR #1472 reported phone resume/signout in the terminal shard and an onboarding checkbox in a cancelled sibling. The counts describe observed names. SPEC-145 stores bounded, sanitized blocks on job completion. SPEC-444 selects exact runs and deduplicates agent notices. Its formatter currently selects terminal failed or errored jobs and links the first one. Task #555 supplies durable delivery evidence for verification.

## Requirements

- OBS-001 Include partial evidence from a cancelled job in the selected failed run when its stored reason identifies a sibling job failure and its retained blocks contain parsed failures. Exclude other cancellation causes and output-tail guesses.
- OBS-002 Keep the job verdict visible. Summarize up to five distinct retained names per job and account for further names and the scanner's omission count. Keep assertion output in details.
- OBS-003 Lead the agent to the exact run's details and the full retained blocks by job, including after the aggregate size limit. Preserve run selection, event identities and delivery instructions.

## Plan

- [x] Specify which stopped siblings have reportable evidence
- [x] Report partial observations in the red-run notification
- [x] Expose the stored blocks through CI run details
- [ ] Verify the fixture and deliver the reviewed change

## Acceptance criteria

- AC1 A PR #1472-shaped SQLite fixture reaches the real PR watch as one message naming both tests with their respective job outcomes. Another pass produces no duplicate.
- AC2 A stopped sibling with 38 distinct names shows five and counts 33 more. A candidate retains its instructions. Jobs without structured evidence and other cancellation reasons add no observation.
- AC3 Run detail identifies each eligible sibling and offers job-specific access to its full retained blocks when the aggregate failure budget or the MCP response limit is exhausted. A cancelled run does not volunteer partial sibling failures.
- AC4 The reviewed change reaches dev through ordinary green CI delivery.

## Test plan

Exercise the existing SQLite and fake-agent PR-watch fixture, the red-run and candidate formatters, and the MCP text renderer including named-job detail. Run focused Rust tests, repository quick checks, mutation verification and the normal exact-head CI gate.

## Decisions

Use the runner's persisted sibling-stop reason and the existing failure-block parsers. Reading retained records also covers runs created before this change. Changing scheduling, storage or notification identity would introduce a second mechanism for evidence the runner already captured.

## Evidence

- On the original implementation, the production watch, bounded-name and MCP detail regressions failed. The unrelated-cancellation control passed. The real watch message contained only the primary test.
- `cargo nextest run --no-fail-fast --locked -p yulius-app -p yulius -E 'test(/^pr_watch::/) | test(/^mcp_text::/)'` passed all 39 selected tests. The SQLite fixture delivered one notice and kept the next pass quiet. Its separate red run with an empty stopped sibling reported only the terminal job.
- The representative fixture's notification includes “failed: `quality` exited 100 (failing tests: phone::resume_signout); `onboarding` cancelled (partial failing tests: onboarding::checkbox). Read `ci_status` with run_id r1. Set job to a listed name for its full details.”
- The 38-name fixture verifies five distinct names and 33 more for the head and candidate. Additional blocks omitted at capture are counted separately. The run-detail fixture exhausts the aggregate budget and recovers the stored assertion through named-job lookup.
- Independent review found that preceding job tails could push the sibling lookup past MCP's 256 KiB response limit. The 16-tail regression failed on the reviewed commit. The fix puts every partial job's lookup before output. All 19 MCP renderer tests then passed, including named retrieval and both jobs' lookup instructions within the response prefix.
