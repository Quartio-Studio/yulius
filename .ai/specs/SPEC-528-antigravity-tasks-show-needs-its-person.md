---
yulius-task: b3a235a3-dc30-4af5-a9c0-8685ad64cae9
status: in-progress
---

# SPEC-528 Antigravity activity across background commands

## Goal

Show Working while an Antigravity session waits on its own command or starts another model turn. When a turn genuinely stops, show what needs the person's attention.

## Context

Tasks #501 and #502 showed running (waiting) with unfinished steps and empty request fields while their terminals showed running commands. Fixes #1444. The agy 1.5.10 probes in [SPEC-505](SPEC-505-implement-antigravity-cli-next-to-claude.md) observed a Stop with `fullyIdle: false` and a subsequent first PreInvocation after a system message.

The hook adapter discards the idle bit and forwards only typed first invocations. Antigravity has no Yulius-managed goal evaluator (`follows_goal`). A clean Stop therefore sets Waiting. The process probe recognizes Claude's shell wrapper. With no observed background shell, `AttentionFacts::group` puts Waiting with unfinished steps in NeedsYou. This is a derived group, so there is no stored NeedsAttention reason or question. A subsequent model transcript line can restore Working, but arrives after the model starts producing output. Task #507 (SPEC-525) translates transcript pages for viewers. This fix uses the native hooks and has no dependency on that reader.

## Requirements

- AGW-001 A successful Stop reporting outstanding background work keeps the task Working. The CLI owns its wake-up. Subsequent idle or failed Stops settle the turn, and ending the run still ends the task.
- AGW-002 A first autonomous invocation restores activity without resubmitting previous user input, clearing a real request or reopening a turn from an older hook. The matching SYSTEM_MESSAGE step identifies the wake-up. An unreadable transcript or an unknown preceding step supplies no activity signal. Typed invocations keep their existing prompt checks.
- AGW-003 An idle Stop with unfinished work retains the existing person's-turn rule and its task brief explains the wait. A real request retains its own text and priority. Missing idle metadata keeps the historical Stop behavior.

## Plan

- [x] Trace the false wait and specify Antigravity goal activity
- [x] Cover autonomous invocations and idle stops with regression tests
- [x] Correct Antigravity goal activity and preserve actionable requests
- [ ] Verify the fix and deliver the independently reviewed change

## Acceptance criteria

- AC-1 (AGW-001, AGW-002) A goal task with open steps reports Working across a busy Stop and autonomous PreInvocation. A first autonomous invocation after an idle Stop restores Working before any model output. An invocation sent before the idle Stop and delivered afterward leaves it Waiting. An idle Stop delayed until after a newer invocation leaves it Working.
- AC-2 (AGW-003) A fully idle Stop with open steps shows why it needs a person. A real needs_you request is visible with its action during agent activity. A finished answer settles normally.

## Test plan

Use synthetic Antigravity 1.5.10 hook payloads with a temporary transcript for the adapter and authenticated task hooks with open steps for the application. Check the task_get classification, reason, and activity after each event. Exercise old and error payloads in the existing hook suites. Run repository quick checks, the changed-file security scan and Yulius CI.

## Decisions

Use the CLI's explicit idle bit and first-invocation hook as lifecycle evidence. Screen text and assistant prose can be stale. Retain SPEC-505's goal launch, compatibility and provider boundaries. The existing turn-order lock also records the latest Antigravity work hook's send time. Idle settlement checks that time and holds the lock through the write so a delayed Stop cannot overwrite a newer invocation. No additional transcript parser, timer, or persisted flag is needed.

## Evidence

- On the original application, the busy-Stop regression returned `activity: waiting`, `shows: needs_you`, one unfinished step and no request. In an isolated archive of baseline `328f94363`, both application regressions failed. The idle Stop with an open step had no wait reason. The original adapter tests observed a missing idle value and no HTTP request at the autonomous invocation.
- `cargo test -p yulius --test main agent_hooks::` passed all 36 tests, including the busy and idle sequences. All 23 hook transport tests passed with `cargo test -p yulius-harness --test hook_client`. The tests use a synthetic Google session and a controlled clock.
- `scripts/dev quick` passed. The local `scripts/dev security` scan was unrun because Vidoc refused the configured key (HTTP 401). No live Gemini model call was made.
- Independent review of `28177bd64` identified a delayed Stop and an ambiguous explanation for a request dismissed during a list read. The gated Stop test failed with Waiting after a newer wake. The request test failed with a browser attribution. The fixes order the state writes and use source-neutral wording for a request snapshot. An ordinary MCP call also checks the reason in the default text.
