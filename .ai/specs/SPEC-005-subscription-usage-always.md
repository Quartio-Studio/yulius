---
yulius-task: 8b2cc0ca-a8e0-4db1-b809-4ef25e745c79
status: done
---

# SPEC-005: Subscriptions show their usage all the time

Status: implemented (real-subscription check pending, see Decisions)
Date: 2026-10-11
Owner: Yulius task #4
Scope authorization: user request ("In subscriptions I see 'Shows once a task runs' - I want to
see usage of subscriptions all the time")

## Goal

The Subscriptions page and the Status panel show each signed-in subscription's usage (5-hour
window, weekly window) at all times, not only after a task ran on it.

## Context

- Claude's usage endpoint needs the OAuth scope `user:profile`; `claude setup-token`, the
  sign-in Yulius stores, lacks it, so the probe got HTTP 403 ("not shared") and usage came only
  from running sessions (ADR-17, ROT-009). An idle subscription showed "Shows once a task runs".
- Claude Code reads the same numbers from the `anthropic-ratelimit-unified-*` headers of its own
  API responses.
- Task #7 and PR #195 change server-side placement and usage rounds; this task stays within
  reading idle usage and showing it.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| USG-001 | When the usage endpoint refuses a sign-in (403), the probe reads the 5-hour and weekly windows from the `anthropic-ratelimit-unified-5h/7d` headers of a one-token message on the cheapest model. | `wiring::usage::tests::a_refused_sign_in_is_read_from_the_headers`, `rate_limit_headers_become_percent_windows` |
| USG-002 | Subscriptions with a live task are not probed: their sessions already report usage. | `accounts.rs::refresh_usage_round` |
| USG-003 | The page no longer says usage "shows once a task runs"; an unread window says "Reading usage…" and the footer says usage is read every few minutes. | `layout.js` |

## Plan

- [x] Idle subscriptions get a usage reading from rate-limit headers
- [x] The page says usage is being read instead of waiting for a task
- [x] The fallback path is covered by a test; clippy and fmt are green
- [x] ADR-17 and the changes log are updated

## Acceptance criteria

- An idle subscription whose usage endpoint answers 403 gets primary and weekly readings after
  one usage round.
- No text on the page says usage waits for a task.

## Test plan

Unit tests for the header parser and a mock-server test of the 403 fallback; the existing
`account_usage` integration tests; clippy and `cargo fmt --check`.

## Decisions

- Utilization headers are read as fractions (0.37 = 37 %); acceptance of a `setup-token` on
  `/v1/messages` and that scale are verified on the deployed instance (coordinator), not here.
- The probe starts the 5-hour window of an idle subscription; accepted for always-visible usage.
