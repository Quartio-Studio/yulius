---
yulius-task: bdf4e0d3-9aa9-43ec-92a1-336758489cf5
status: done
---

# SPEC-523. Default to Ultrafast on a Codex Pro 500 subscription

## Goal

Task #505 asks for Ultrafast by default when a Codex task runs on the user's Ultra plan.

## Context

SPEC-511 sets the model and reasoning effort. Speed is a separate Codex setting. Yulius already
writes the selected account's credential and Codex home configuration on each launch. Codex
0.169.0 reads the plan from the ID token and maps `promax` to Pro 500. OpenAI lists Pro 500 as
the individual plan with Astra Ultrafast. See [Codex's plan labels](https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/tui/src/subscription.rs)
and [OpenAI's Pro tiers](https://help.openai.com/en/articles/9793128-about-chatgpt-pro-tiers).

## Requirements

- **SPD-001** Supply the native Ultrafast default on new and resumed Codex launches whose assigned
  ChatGPT sign-in identifies Pro 500. Recompute it after an account move or credential refresh.
- **SPD-002** Leave the speed to Codex for other plans, API keys, and missing or unreadable plan
  information. Preserve model and reasoning effort choices. Plan detection requires no network
  call, exposes no credential data and adds no failure to launching.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Select Ultrafast from the assigned Codex subscription on each launch
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- **AC-1 (SPD-001)** A new Pro 500 task and a resume on that subscription have the top-level
  `service_tier = "ultrafast"` in the generated home configuration. A refreshed sign-in that gains Pro 500 gets the same default.
- **AC-2 (SPD-002)** Replanning with a lower-tier account, API key, or unreadable claim omits the
  setting and still produces a launch plan. Model and effort follow SPEC-511.

## Test plan

Exercise the public launch planner with synthetic credentials, checking the generated configuration
for new sessions and resumes, including changes of subscription. Run the harness suite, quick
checks, mutation checks, security scan and Yulius CI.

## Decisions

Use the selected subscription's namespaced ID-token plan claim. The top individual plan that
Codex recognizes is Pro 500. Write the default to the managed home configuration so Codex can
apply project overrides, session choices, model capabilities and feature settings.

## Evidence

- The public launch test failed on the Pro 500 upgrade before implementation and on a truncated
  token before the envelope check. The complete `cargo test -p yulius-harness` suite then passed
  (240 tests). Both new and resumed launch plans were checked using synthetic sign-ins.
- The pinned Codex CLI 0.169.0 returned `service_tier: ultrafast` from `config/read` against a
  temporary home with the generated setting. Its service-tier resolver checks the model catalog
  and honors disabled features. See [the pinned resolver](https://github.com/openai/codex/blob/rust-v0.169.0/codex-rs/tui/src/service_tier_resolution.rs).
  The same CLI reported `promax` from `account/read` with a synthetic personal sign-in and
  reported `priority` when a trusted project's configuration selected it. No billable turn was sent.
- `scripts/dev quick` passed the repository contracts and formatting check.
- Independent review approved `00b7ccef` with one minor. The reviewer found that the plan detector accepted padded JWT payloads. The added
  launch assertion failed on the padded Pro 500 fixture. Plan detection now uses Codex's strict
  base64url decoding, while the existing credential refresh comparison keeps its prior behavior.
- Local Vidoc scan exited 2. Vidoc rejected the instance key (HTTP 401).
- A synthetic Responses endpoint observed the pinned CLI send `service_tier: ultrafast` for a
  GPT-6.2 Astra catalog fixture advertising Ultrafast and omit the field for the same model
  without that tier. This exercised the actual request without billing a provider account.
- `cargo clippy -p yulius-harness --all-targets -- -D warnings` passed after the review fix.
- Focused cargo-mutants run of the entire final harness change at `0cfad1ff0`, with the
  240 harness tests: 17 mutants tested, 16 caught, 1 unviable, 0 missed. This run isolates the
  feature from the workspace mutation copy's unrelated Vidoc test failure
  ([issue #1364](https://github.com/Quartio-Studio/yulius/issues/1364)).
- Initial `scripts/dev mutants`: 17 listed, 16 caught, 0 missed, 0 timed out, 1 unviable,
  0 not reached (bound 30 min, took 721 s). The harness-only result above verifies the viable
  mutations independently of issue #1364.
- Review-fix `scripts/dev mutants --base 00b7ccef9`: 5 listed, 5 caught, 0 missed, 0 timed
  out, 0 unviable, 0 not reached (bound 30 min, took 531 s).
- Yulius CI run `0e97e2bf-3e5b-4f0c-9c27-066bbcc03783` passed on the reviewed fix
  `0cfad1ff0` in [PR #1436](https://github.com/Quartio-Studio/yulius/pull/1436).
  The security job was skipped. The merge queue checks the final documentation commit.
