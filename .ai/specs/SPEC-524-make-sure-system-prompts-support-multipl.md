---
yulius-task: 3258204f-01dc-4b52-91c2-512a6c19e1aa
status: in-progress
---

# SPEC-524 Harness-specific system prompts for independent review

## Goal

Every task receives review instructions it can follow in its own harness. Codex reviews run on GPT-6.2 Astra, Antigravity reviews on Gemini 4 Neon, and Claude Code keeps its existing round policy.

## Context

The standard briefing, delivery workflows, skills and request text prescribe Claude Code's reviewer for every harness. Only Claude Code installs that reviewer and applies the Fable allowance and round hooks. The current attempt's harness already reaches the briefing builder. Task #501 scoped the same Neon default but paused before implementing it. Task #503 verified the 1.5.12 CLI pin and model catalog. This change reuses its pinned artifacts and delivers the default. Task #505 owns Codex speed.

## Requirements

- HR-1. Direct each review, including repeat rounds and playground reviews, to a fresh read-only reviewer in the current harness. Claude uses its round-selected reviewer, Codex requests Astra at high effort and Antigravity requests Neon at high effort.
- HR-2. Shared instructions refer to the active briefing for the review mechanism or qualify advice by harness. The independent verdict and delivery conditions still apply.
- HR-3. Make Gemini 4 Neon the default while preserving explicit task model selections. Enable a Gemini inference speed setting only if the shipped Antigravity CLI supports one.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Give each harness its own independent review instructions
- [ ] Verify Neon defaults and Antigravity fast-mode support
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-1. Both repository delivery profiles generate the correct review request for all three harnesses, including when the author runs another model. A non-Claude review receives no Claude agent or round-selection directive.
- AC-2. Shipped shared skills and workflow templates agree with the briefing. Existing Claude round selection continues to pass its tests.
- AC-3. An unspecified Antigravity task launches Neon, with the CLI's supported speed behavior established against the CLI reference and executable.

## Test plan

Exercise the generated briefing for each harness and profile, validate shared prompt contracts and inspect the pinned CLI help, command catalog and settings. Verify default launch arguments, explicit selection, the task view and the composer. Run quick checks, change scan, mutation checks and Yulius CI.

## Decisions

Use the attempt harness in the briefing and leave the author's explicit model choice intact. Keep shared delivery text independent of harness configuration. See [ADR-0376](../../docs/decisions/0376-review-instructions-follow-the-active-harness.md).

## Evidence

The old standard Codex briefing failed the policy matrix by naming the Claude reviewer. The updated launch test failed on the original Carbon argv.

Antigravity 1.5.12 on Linux x64, downloaded from Google (sha512 starting 26de25fc), reports in its bundled changelog that 1.1.0 removed `/fast`. Its help and the [CLI reference](https://antigravity.google/docs/cli/reference) expose no inference-speed flag. A synthetic Gemini API request confirmed Neon at high effort and the native `swarm` tool supporting a fresh worker pinned to a model. The `swarm models` response from the actual CLI includes `gemini-4-neon-high`. The ordinary `invoke_subagent` tool only accepts tiers.

- Real CLI 1.5.12 with a synthetic API: a Carbon parent called `swarm add` with `model=gemini-4-neon-high`. The tool returned the worker name and that model, and the worker sent its request to the Neon endpoint.
- Round 1, head 6fd7d10ab, requested changes (both own code). The new matrix assertion reproduced the incorrect promise that a Codex review opens the PR. Only Claude now gets that instruction, and the shared skill allows a code verdict before the first PR starts CI. The matrix checks the native tool, model, effort and fresh context for each profile.
- APM install and audit passed, and the repository quick checks passed. The local change scan was unrun because Vidoc returned HTTP 401.
