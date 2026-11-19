---
yulius-task: 06a14f9f-537f-4675-8110-e05490d4329f
status: done
---

# SPEC-521 — Select a Codex or Gemini model and its effort

## Goal

Pick any user-selectable Codex or Gemini reasoning model for a task and see the effort levels it supports. The closed composer still needs only a model and an effort chip.

## Context

The attached composer screenshot shows one Codex choice and Gemini 4 Carbon. Settings shares that catalog. Both surfaces currently use Claude's effort list. The native launch plans already pass explicit model and effort values to their respective CLIs (SPEC-505 and SPEC-511). Task #501 owns the separate request to change Google's default. Task #502 owns random harness selection.

## Requirements

- **SEL-001** Offer every visible Codex model compatible with the pinned CLI in OpenAI's current stable CLI catalog, and all individual Gemini reasoning models in Google's Antigravity model selector. Each explicit choice submits the model ID and harness. Account access depends on the provider's plan.
- **SEL-002** Group models by provider, support filtering by name, and reveal the effort choices for the active model on demand. Default effort uses the server's high default. Hide effort for Haiku.
- **SEL-003** Both Settings and the composer keep a supported effort on a model change and clear an incompatible effort to Default. Normalize a stale saved pair before display or submission. Preserve the existing Codex default preference and harness-only request.

- **SEL-004** The pinned Antigravity CLI accepts the offered Gemini model and effort pairs. Pin the first verified release available from Google’s manifests for both supported Linux architectures.

- **SEL-005** The pinned Codex client bundles the selected models and their effort metadata so API-key accounts also get the model-specific Ultra behavior.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Expose the supported Codex and Gemini models in the searchable picker
- [x] Use model-specific effort tiers for task creation and saved preferences
- [x] Pin the Antigravity client that launches the full Gemini catalog
- [x] Pin the Codex client with GPT-6.2 Ultra delegation metadata
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated
- [x] Verify withheld-screen attention after the opening turn finishes

## Acceptance criteria

- **AC-1 (SEL-001)** Searching Sol or Flash finds explicit models. Starting a task with one sends that exact model ID and matching harness. The Codex default still follows the server.
- **AC-2 (SEL-002)** With the menu closed only model and effort chips are shown. Opening the model menu reveals named provider groups. Opening effort shows only the current model’s tiers, and selecting Haiku hides effort. GPT-6.2 offers Ultra, GPT-6 Luna stops at Max, and GPT-5.5 stops at Extra high. Gemini Carbon stops at High, Hydrogen and Neon offer Extra high, and Gemini 3.1 Pro offers Low and High.
- **AC-3 (SEL-003)** Moving from Ultra to Carbon shows Default and sends no override. A compatible choice survives switching and reload. Settings and the composer agree on restored choices.

- **AC-4 (SEL-004)** A real pinned CLI sends a Neon request with Extra high to a synthetic Gemini endpoint and honors the selected tiers for the other seven Gemini models.

- **AC-5 (SEL-005)** Codex loads GPT-6.2 catalog metadata on API-key launches. In Ultra it applies the model’s multi-agent reasoning level, verified against a synthetic endpoint.

## Test plan

Exercise explicit task creation, filtering, cross-model switches, incompatible stored values and Settings in browser tests. Check the full catalog and tiers against the provider metadata. Run the existing default-choice and launch contracts, the repository quick checks and the browser suite.

## Decisions

Catalog maintenance and default compatibility follow [ADR-0372](../../docs/decisions/0372-model-pickers-use-provider-reasoning-levels.md). Sources are the visible entries in the stable Codex 0.170.0 binary and Google's [Antigravity model guide](https://antigravity.google/docs/models). The Google scope is the eight individual Gemini models. Orchestrator is a multi-model execution mode.

## Evidence

- The browser tests for explicit Sol and Hydrogen failed on the original picker because neither menu item existed. After the change, all four tests in `e2e/tests/model-choices.e2e.ts` passed, including every offered Codex and Gemini tier and Settings preference recovery.
- Compared the 11 visible entries and their tiers in the stable Codex 0.170.0 binary with the eight individual Gemini models and tier buttons in Google's model guide on 2026-11-19. The CLI smoke used synthetic responses and local, isolated homes.
- Antigravity 1.5.10 rejected `--model gemini-4-neon --effort xhigh`. All 25 selectable model/effort pairs reached the fixture endpoint on 1.5.12. Gemini 3.1 Pro uses its preview API ID and thinking budget. The other models conveyed the selected thinking level.
- Codex 0.169.0 sent a GPT-6.2 Ultra turn at medium on fallback metadata. On 0.170.0 all 11 selected model IDs reached the local endpoint at their highest tier. Ultra runs used their catalog's multi-agent reasoning level (extra high or max). Luna's Max and GPT-5.5's Extra high were sent directly.
- `cargo test -p yulius-harness --test launch` passed 26 tests. The domain release-pin tests failed on their previous versions before the pin updates.
- Live subscription entitlements and model-generated answers were outside the synthetic checks.
- Review round 1 found that the browser test could confuse an omitted effort with explicit High. It now checks the raw request before the mock resolves defaults. Injecting an explicit High made the stale-preference assertion fail. All four browser tests passed on the fix, and round 2 approved commit `e0d4acfe3`.
- The two domain pin contracts, 12 UI contracts, 11 Codex installer tests and three Antigravity installer tests passed. Both network installer tests also passed on Linux x64, fetching, verifying and running the real releases. ARM64 digests were checked against publisher metadata.
- Local `scripts/dev quick` passed. Local `scripts/dev security` was unrun because Vidoc rejected the key with HTTP 401.
- Across all 92 browser test files, 1,119 tests passed and the credential-dependent agent test was skipped. The full local command terminated after 85 files reported. The seven remaining files passed in a separate invocation (196 tests). The tightened request assertions passed in their focused run.
- `scripts/dev mutants` reported 8 listed, 6 caught, 0 missed, 0 timed out, 2 unviable, 0 not reached. The review fix had no Rust mutations. Its browser mutation failed on the previously uncovered assertion.
- The hosted check on the submitted head is enforced at merge by Yulius's delivery queue.
- PR #1439’s first CI run failed in an existing Claude terminal test. The fake agent prints its answer before its Stop hook settles the opening turn. Under eight concurrent workers, 2 of 32 runs lost the attention flag. Waiting for that run’s answered Stop before starting the withheld-screen scenario preserved every original assertion and passed the focused test and all 32 concurrent runs. [Issue #1445](https://github.com/Quartio-Studio/yulius/issues/1445)
