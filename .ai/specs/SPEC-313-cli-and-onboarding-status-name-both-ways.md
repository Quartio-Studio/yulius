---
yulius-task: e74724e5-ee8a-495a-8a0e-6337d51297f7
status: done
---

# SPEC-313: The CLI and the onboarding status name both ways to start onboarding

Status: done
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
- What else the cause left stale: the "CLI never starts it" wording in SPEC-051 (OBD-002,
  OBD-003, the implementation notes), SPEC-001 (IMP-010, AC-IMP-4), SPEC-002 (§6.5, §6.6),
  ADR-0038's decision, README.md, the module docs of `onboarding.rs` and `run.rs`, doc comments
  and comments in `onboarding.rs`, `projects.rs`, `lifecycle.rs` and `imports.rs`, and comments
  in the tests. The web UI does not show the pending
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
- [x] Regression tests ask for both ways (failing)
- [x] CLI fact, status item and refusal name both ways
- [x] Specs, ADR-0038 and comments match ADR-0188
- [x] Gates green, review answered and delivered on dev

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

## Evidence

- Red (commit `768e4e5b`, tests only, on `dev` 9e1a2870):
  `cargo nextest run -p yulius-import -p yulius -E 'binary_id(yulius-import::import) |
  (binary_id(yulius::main) & test(/^import_server::/))'`: 72 run, 4 failed:
  `import_server::onboarding_starts_only_from_the_browser` (the continue refusal),
  `import_server::import_resumes_applies_and_reruns_idempotently` (the pending item),
  `end_to_end::onboards_everything_then_sends_nothing_new_on_rerun` and
  `end_to_end::dry_runs_status_and_declined_confirmations_upload_nothing` (the CLI fact).
- Green (fix `5ecb4494`): the same filter plus `projects::` and `remote`: 131 run, 131 passed.
  The approve refusal's old `contains("in the browser")` in
  `onboarding_starts_only_from_the_browser` now asks for both ways too.
- Independent review (fresh session, `om-code-review`) of `9e1a2870..35c154cb`: approve, with
  minors and nits, all answered in this change: a 112-character doc line and the stale `# Errors`
  of `approve_onboarding` (own code, fixed); stale comments in `lifecycle.rs` and `imports.rs`
  and SPEC-002 §6.5/§6.6 (missed by the sweep, fixed); this spec's ticks and Evidence (added);
  long test literals (wrapped); SPEC-051's "rewords both" (made precise); the test's doc comment
  (fixed); README.md's onboarding line (pre-existing, fixed since it is the same wording).
- Review round 2 of `35c154cb..162450e9`: approve; SPEC-002 §6.6's continue clause (fixed: a
  browser session or an `operate` token, the import stays browser-only), long Markdown lines
  (rewrapped) and a ragged `# Errors` line (joined).
- Review round 3 of `162450e9..6fe40624`: approve. Left as is: three Markdown paragraphs (SPEC-002
  §6.5/§6.6, README.md) still hold lines over 100 characters, as they did before this change; no
  gate checks Markdown line length.
- `scripts/dev ci` at `6fe40624`: exit 0 (repository check, script tests 143 OK, check, fmt,
  clippy, nextest 2615 passed / 16 skipped, doctests, deny, shear, doc).
- `scripts/dev security` at `6fe40624`: exit 2, not run: Vidoc's key is out of quota (HTTP 429
  "This API key has used its 50,000,000 tokens"); the 9 changed code files were not scanned. The
  change only rewords messages, comments and docs.
