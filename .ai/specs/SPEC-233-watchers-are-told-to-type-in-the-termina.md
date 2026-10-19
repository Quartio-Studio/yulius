---
yulius-task: d5037714-ba92-4ff7-973f-095c6dc6a614
status: in-progress
---

# SPEC-233: A watcher is told the agent waits for the owner, not to type in a terminal

Status: in-progress
Date: 2026-10-19
Owner: task #219
Scope authorization: task #219: "Fix GitHub issue #576". Fixes #576, found by task #170. Extends
SPEC-179 (someone else's task is watched as its live transcript) and SPEC-070 (only the owner
changes a task).

## Goal

A person watching someone else's waiting task reads that the agent waits for the owner
("The agent is waiting for @anna-q's reply.", "The agent needs @anna-q's attention."), with no
instruction to type in a terminal they do not have. The owner's texts stay as they are.

## Context

Confirmed against `dev` at b9080ea4.

- **Symptom.** Mock `/s/studio/t/12` handed to anna-q, activity `waiting`: the control card of the
  watcher (mkarp) reads "…Request control The agent is waiting for your reply. Type it in the
  terminal below."; with `needs_attention` it reads "The agent needs your attention. Its question
  is in the terminal below."
- **Root cause.** `renderAttention()` in `crates/yulius-server/web/assets/taskview.js` builds both
  texts as constants. `owner()` (`otherOwner()` in `status.js`: another person's login for a
  watcher, null for one's own task) gates only the Reply button there, never the words. A watcher
  has no terminal since SPEC-179 (they read the live transcript) and cannot type: the reply is the
  owner's.
- **Why the tests missed it.** `reply.e2e.ts` and `task-done.e2e.ts` check the texts as the owner
  only; `watching.e2e.ts` never set a watched task to `waiting` or `needs_attention`.

## Requirements

- **WAT-001** For a watcher (`owner()` set) of a running task waiting for a reply, the attention
  bar says "The agent is waiting for @<owner>'s reply." and nothing about typing or a terminal.
- **WAT-002** For a watcher of a running task that needs attention, it says "The agent needs
  @<owner>'s attention." and nothing about a terminal.
- **WAT-003** The owner's texts and Reply buttons are unchanged.

## Plan

- [x] Spec records the root cause of #576
- [ ] Regression e2e test fails on the watcher's texts
- [ ] Watchers are told the agent waits for the owner
- [ ] Independent review approved
- [ ] Gates green and merged into dev

## Acceptance criteria

- `watching.e2e.ts`: a watcher of studio #12 (owner anna-q) sees "@anna-q's reply" when it waits
  and "@anna-q's attention" when it needs attention, and no "terminal below" or "your reply"; handed
  back to mkarp, the owner reads the unchanged texts. Fails on `dev`.
- `reply.e2e.ts` and `task-done.e2e.ts` (the owner's texts) stay green.

## Test plan

One e2e test in `e2e/tests/watching.e2e.ts` drives both activities as a watcher, then as the
owner, against the mock (`ambient=0` so nothing moves under it). The full e2e suite and the
repository gate run before delivery.

## Decisions

- The watcher's text names the owner with `@login`, as the watch footer does ("@anna-q is at the
  keyboard · you are watching"); no instruction is given, since the watcher's only action, Request
  control, is already on the card. No ADR: a wording fix within SPEC-179.
- The sidebar labels "Your turn" / "Needs your action" (`statusInfo()` in `status.js`) are shared
  by every view of a task and are not this change; noted on #576.
