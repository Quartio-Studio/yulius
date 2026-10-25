---
yulius-task: e74724e5-ee8a-495a-8a0e-6337d51297f7
status: in-progress
---

# SPEC-313: The CLI and the onboarding status name both ways to start onboarding

Status: in-progress
Date: 2026-10-25
Owner: task #299
Scope authorization: task #299 (solve issue #790 in one pull request, `Fixes #790`; no new
issues).

## Goal

Every message that tells a person or their agent how a `pending` sandbox's onboarding starts names
both ways: **Start onboarding** on the sandbox page, or `yulius project onboard`
(`onboarding_start`) with an `operate` token the person made for their own agent.

## Context

- Symptom (#790): a person's agent following [remote-control.md](../../docs/remote-control.md)
  runs `yulius onboard . --yes`, which prints "Onboarding ● waiting for Start onboarding in the
  browser" and the pending item "someone clicks Start onboarding on the sandbox page: …". The
  guide's next step is `yulius project onboard --notes …`, which the output never mentions. A
  `yulius login` (`cli`) token that tries to start or continue it is refused with "Start
  onboarding on the sandbox page in the browser; the CLI only uploads", which does not say an
  operate token can.
- Root cause: the three messages were written for ADR-0038 (onboarding starts only in the
  browser). ADR-0188 (SPEC-247 RMT-021) let an `operate` token start it too
  (`AuthContext::acts_for_person`, `crates/yulius-app/src/auth.rs`) and updated the doc comments
  of `ensure_browser`, docs/api.md, docs/operations.md and docs/product.md, but not the strings:
  - `show_onboarding` (`crates/yulius-import/src/run.rs`): the `pending` fact, with the comment
    "The CLI never starts onboarding".
  - `App::onboarding_status` (`crates/yulius-app/src/onboarding.rs`): the `pending` item, with
    the comment "never the CLI".
  - `ensure_browser` (`crates/yulius-app/src/onboarding.rs`): the 403 text.
- Why the tests missed it: the tests pin the old strings (`end_to_end.rs`, the import test
  server's stub status, `import_server.rs` with `contains("Start onboarding")`), so they kept
  passing; none asks for the CLI way.
- What else the cause left stale: the "CLI never starts it" wording in SPEC-051 (OBD-003, the
  implementation notes), SPEC-001 (IMP-010, AC-IMP-4), ADR-0038's decision, the module doc of
  `onboarding.rs` and `run.rs`, and comments in the tests. The web UI does not show the pending
  items (it has its own gate, ADR-0076), so it is unaffected.

## Requirements

- NBW-001: `yulius onboard` (and `--status`) shows a `pending` sandbox as waiting to start, naming
  Start onboarding on the sandbox page and `yulius project onboard` with an operate token.
- NBW-002: the onboarding status of a `pending` sandbox (`OnboardingStatus.pending`, read by the
  CLI, `project_get` and the import result) names both ways, with the sandbox page's URL, and
  `onboarding_start` for an agent on the remote MCP.
- NBW-003: the 403 for a caller that may not start onboarding (a `yulius login` token, at
  approve or at continuing the resting onboarding task) names both ways and says the `yulius
  login` token cannot.

## Plan

- [x] Spec and root cause of the browser-only messages
- [ ] Regression tests ask for both ways (failing)
- [ ] CLI fact, status item and refusal name both ways
- [ ] Specs, ADR-0038 and comments match ADR-0188
- [ ] Gates green, review answered and delivered on dev

## Acceptance criteria

- `yulius onboard` against a `pending` sandbox prints "Onboarding ● waiting to start: Start
  onboarding on the sandbox page, or `yulius project onboard` with an operate token" (NBW-001).
- The import result's `onboarding_status.pending` for a `pending` sandbox contains "Start
  onboarding on the sandbox page", the sandbox URL, "`yulius project onboard`" and
  "`onboarding_start`" (NBW-002).
- `POST /api/tasks/{prep}/continue` of a `pending` sandbox's onboarding task with a `cli` token is
  403 and the message contains "Start onboarding on the sandbox page" and "`yulius project
  onboard` with an operate token" (NBW-003).

## Test plan

- `crates/yulius-import/tests/import/end_to_end.rs`: the full run and `--status` assert the new
  fact line on the rendered output; the test server's stub status returns the new pending item,
  which the rendered output shows.
- `crates/yulius/tests/import_server.rs`: the upload's `onboarding_status.pending` and the `cli`
  token's continue refusal assert both ways. Both fail on `dev` before the fix.

## Decisions

- The CLI fact keeps both ways in the line itself rather than relying on the server's pending
  item below it: an older server's item still says only the browser.
- The pending item names the CLI way first and ends with the sandbox URL, so the URL is the last
  word of the line and a terminal's link detection does not take a trailing comma with it.
- No new ADR: ADR-0188 already decided the operate token starts onboarding; ADR-0038 gets an
  amendment note pointing to it, as SPEC-051 OBD-002 already has.
