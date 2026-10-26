---
yulius-task: ce3233f6-ace6-4451-acea-b51b41a3a832
status: done
---

# SPEC-327: A Needs you request reads What happened, Problem, To do

Status: done
Date: 2026-10-26
Owner: Yulius task #311
Scope authorization: task #311: "Can we make it a bit more clearly with clear to do for a user
[…] I propose: what happened, problem, to do for a user or decision for a user."

## Goal

A `needs_you` request (SPEC-134 CDT-004, ADR-0111) reads as three short labelled parts in the
task's attention bar: **What happened**, **Problem** and **To do** (or **Decide** when the person
has a choice to make), so a person sees at a glance what they must do and why, instead of one
paragraph that starts with the action and runs into its reasons.

## Context

- Today `needs_you {action, why?, link?}` renders as one run of text in the attention bar
  (`taskview.js`): "**Needs you:** <action> · <why>". In the screenshot of the request the action
  (a Caddy command) and a 300-character `why` mixing what happened, the problem and a reassurance
  read as one paragraph; the to-do is the least visible part.
- The sidebar row and the notification center read `Needs you: <action>` (`status.js`,
  `notifications.js`); its detail is `why`.
- The request is stored as JSON in `tasks.needs_you` (`NeedsYouRecord`) and shown through
  `NeedsYouView` (`yulius-contracts`). Rows written before this change hold `why`.
- Agents in sessions started before the release keep the old tool schema until they restart; a
  call with `why` must keep working.

## Requirements

- R1. `needs_you` takes `happened` (what happened, at most 300 characters), `problem` (what is
  wrong or blocked because of it, at most 500), `action` (required: what the person should do or
  decide, at most 200) and `decision` (boolean, default false: the person chooses rather than
  does). `why` stays accepted as the older name of `problem`; giving both is refused.
- R2. The stored record and the API view carry `happened`, `problem` and `decision`; a row stored
  with `why` reads as its `problem`. `NeedsYouView.why` is gone.
- R3. The attention bar shows a heading "Needs you" (or "Needs your decision") and, below it, a
  labelled list: What happened, Problem, To do (or Decide), each only when given, the to-do last
  and emphasised. Backticked text renders as code. Open and Reply stay beside it. At phone width
  the labels sit above their text and nothing scrolls sideways.
- R4. The sidebar label and the notification read "Needs you: <action>" or "Needs your decision:
  <action>"; their detail is the problem.
- R5. The tool answer gives the agent the request as text to print in the session in the same
  three parts; the tool description and briefing line ask for the three parts.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] needs_you takes and stores what happened, the problem and a decision flag
- [x] The attention bar shows What happened, Problem and To do as a labelled list
- [x] Sidebar, notifications, tool text and docs name the to-do or decision
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Gates green, evidence recorded and delivered

## Acceptance criteria

- A1. A call with `happened`, `problem`, `action` stores all three; with `why` only, `why` becomes
  `problem`; with both `why` and `problem` it is refused; `happened` over 300 characters is refused.
- A2. A stored JSON row with `why` deserializes with that text as `problem`.
- A3. In the e2e mock a task with all parts shows three labelled rows in order, the to-do last; a
  decision shows "Decide" and "Needs your decision"; only an action shows just the To do row.
- A4. At 360 px the bar has no horizontal overflow.
- A5. The sidebar row's label reads "Needs your decision: …" for a decision.
- A6 (review). At 360 px the longest request (300 + 500 + 200 characters) keeps its to-do and its
  buttons in view: the context scrolls inside the list with the to-do held at its foot (opaque in a
  stacked bar too), the buttons wrap below, the terminal's corner notices start below the bar, and
  the list keeps its scroll and keyboard across task updates; it is a tab stop only while it scrolls.

## Test plan

- Unit tests in `needs_you.rs`: argument parsing (A1) and record compatibility (A2).
- Integration test through the MCP tool where one exists for `needs_you` (`tests/cd.rs`).
- e2e in `cd.e2e.ts`: A3, A4, A5; `reply.e2e.ts` fixtures move from `why` to `problem`.

## Decisions

- [ADR-0221](../../docs/decisions/0221-a-needs-you-request-reads-what-happened-problem-to-do.md):
  three named fields rather than one formatted `why`, `why` kept as an input alias, `decision` a
  flag rather than a list of options.

## Evidence

- Red first: `cd::a_needs_you_request_reads_what_happened_problem_and_to_do` failed with "unknown
  field `happened`"; the two new e2e tests in `cd.e2e.ts` failed before the bar changed; the plain
  sidebar label assertion failed with the backticks left in; the review tests (longest request at
  360 px, scroll/focus across updates, stacked to-do background) each failed before their fix.
- `cargo nextest run -p yulius -p yulius-app -E '… needs_you | task_message:: …'`: 17 passed.
- `npx e2e run tests/cd.e2e.ts tests/reply.e2e.ts`: 28 passed (twice in a row after the review
  round 1 fixes).
- `scripts/dev ci` at c3819d3f: exit 0; nextest 2624 tests run, 2624 passed, 16 skipped; lint,
  docs, deny ("advisories ok, bans ok, licenses ok, sources ok"), macOS compile and repository
  contracts passed. A first run failed on `clippy::expect_used` in the new unit test (fixed in
  c3819d3f).
- Full e2e suite (`npx e2e run`) at c3819d3f: exit 0; 66 files passed, 1 skipped; 718 tests passed, 1 skipped.
- `yulius security-scan`: **not run** (exit 2): Vidoc answered HTTP 429, "This API key has used its
  50,000,000 tokens"; 12 files not scanned. Unrun, not passed.
- Screenshot check (a failing throwaway e2e test's trace, removed): the request from the user's
  screenshot reads Needs you / What happened / Problem / To do with the Caddy commands as code, at
  1440 px and at 360 px (context scrolled, to-do pinned).
- Independent review (fresh session, `yulius-review` skill; `om-code-review` is not in this
  worktree): round 1 REQUEST CHANGES (to-do off screen on a phone, `docs/api.md`, link error naming
  `why`, corner notices under the bar; nits: unpaired backtick, `dl` in a `span`, icon alignment)
  all fixed test-first; round 2 REQUEST CHANGES (scroll and focus lost on task updates, see-through
  to-do in a stacked bar; nits: class on another bar, empty tab stop) all fixed test-first; round 3
  APPROVE at eed0abac. Left by decision: `{"problem": "", "why": "x"}` and `decision: null` are
  refused like `resolved` is (no agent sends them); a re-asked request reuses the old scroll
  position (clamped, harmless); a focused list that stops scrolling keeps its tab stop until the
  next refit. Pre-existing, outside this change: `cd.e2e.ts`'s `.task-row` `{ hasText }` check
  passes whatever the text (the harness ignores `hasText`); the new title assertions cover it.
