---
yulius-task: 1d49455f-17af-43a4-a853-03087bcdbfcb
status: done
---

# SPEC-583 — CLI and Superagent tasks inherit the Codex launch defaults

[SPEC-607](SPEC-607-codex-tasks-run-at-high-effort-unless-th.md) supersedes the inherited effort used in this release.

## Goal

A person can run `yulius task create` or ask either Superagent to start a task and get GPT-6.2 Astra at extra high effort, with Ultrafast on eligible subscriptions.

## Context

The CLI forwards the prompt to remote MCP. It shares the `task_create` handler with sandbox and Global Superagents. Cross-sandbox approval invokes the same handler in the target. [SPEC-520](SPEC-520-i-dont-know-why-but-i-dont-see-random-ha.md) draws a random harness for these callers when both harness and model are omitted. Once Codex is selected, [SPEC-561](SPEC-561-default-astra-6-2-to-xhigh-for-new-work.md) already supplies Astra and extra high effort, and [SPEC-523](SPEC-523-make-ultrafast-default-speed-in-codex-if.md) selects the tier from the assigned subscription.

## Requirements

- SPA-001. Default direct CLI, sandbox Superagent, Global Superagent and approved cross-sandbox task creation to the Codex harness when harness and model are omitted. Persist the normal optional model and effort overrides and apply the regular Codex subscription and launch rules.
- SPA-002. Honor named harnesses, model-implied harnesses, explicit effort and explicit Random. A follow-up created by a running task still inherits its parent harness. Automation selection and the HTTP task composer keep their existing policies.
- SPA-003. The task creation tools and operator documentation describe the defaults and subscription eligibility accurately.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Default CLI and Superagent task creation to Codex
- [x] Describe the inherited defaults in task creation tools and project documentation
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated
- [x] Verify deferred Codex startup and messaging after cross-sandbox approval

## Acceptance criteria

- With harness and model omitted, remote and Superagent creation select Codex with chosen provenance. Cross-sandbox creation does so after approval. Missing Codex capacity follows the ordinary Codex admission rules.
- Explicit Random still draws, an explicit model selects its provider, and a running Antigravity parent still creates an Antigravity follow-up. An explicit effort survives creation.
- The inherited Codex launch uses Astra at xhigh, and selects Ultrafast for ChatGPT Pro 500. Other accounts use their existing tier. Both task creation schemas explain what omission means. The browser mock models the same approved cross-sandbox choice.

## Test plan

Exercise the shared MCP creation path and the remote, Global Superagent and cross-sandbox entry points with synthetic accounts. Run the existing Codex launch tests for effort, model and tier, including ineligible subscriptions. Exercise cross-sandbox approval in Chromium. Verify tool descriptors, run repository quick checks and submit the reviewed change to Yulius CI.

## Decisions

Apply [ADR-0422](../../docs/decisions/0422-cli-and-superagent-task-creation-defaults-to-codex.md) at the common task creation boundary. Let the existing launch planner resolve model, effort and tier from task and subscription settings. The request changes direct task creation only. Scheduled automation runs and task-agent follow-ups have separate documented selection rules.

## Evidence

- On the original handler the CLI started Claude with only a Claude subscription connected. The sandbox and Global Superagent tests observed Claude, and the shared handler reported a random harness instead of Codex with chosen provenance.
- The original cross-sandbox mock approval produced Claude with random provenance. The corrected approval passed in Chromium with omitted settings, explicit High, a model override, a Gemini model and explicit Random.
- `cd e2e && npm ci --ignore-scripts && npx e2e run` passed 1,298 tests. The optional agent-driven editor test skipped because it requires a model credential.
- The MCP default description test failed on the previous schema, which still described a random draw for a caller without a task.
- Scoped `cargo nextest run` across `yulius`, `yulius-app` and `yulius-harness` passed all 101 selected tests. The real CLI reported the missing Codex subscription, then created a Codex task whose details showed inherited Astra and xhigh. Both Superagents and approved cross-sandbox creation recorded chosen Codex. The launch tests verified effort for new and resumed processes and Ultrafast eligibility against synthetic subscription credentials.
- Independent review approved `634be7942` with no findings after rerunning nine scoped cases. Repository quick checks passed. `scripts/dev security` reported Vidoc disabled for the project.
- Scoped Clippy passed with warnings denied. `scripts/dev mutants` reported 9 listed, 3 caught, 0 missed, 0 timed out, 6 unviable, 0 not reached. Hosted CI and merge evidence are recorded by Yulius for the delivered revision. No live provider session was started for verification.
- Hosted test shard 2 on `638e8505d` and a local reproduction both failed when the live cross-sandbox test started a prepared Codex task with only a Claude account connected. The fixture now checks that approval selected Codex before connecting its synthetic Codex account. All 41 selected creation and cross-sandbox tests passed, including start, message delivery and audit through the fake agent process.
