---
yulius-task: 3af0ab1d-b26b-4f9b-95fe-1e30eb87d458
status: in-progress
---

# SPEC-424: a click on the attention bar is never lost to a task update

Status: in progress
Date: 2026-11-05
Owner: Yulius task #406
Scope authorization: task request: "sometimes i cant click reply", with a screenshot of the task
view's attention bar ("The agent needs your attention. Its question is in the terminal below." and
its Reply button).

## Goal

Reply, and every other button of a task's attention bar (UX-03), does what it says on the first
click, also when a task update arrives while the person is clicking; the keyboard stays on the
bar's Reply across updates.

## Context

`renderAttention` in `crates/yulius-server/web/assets/taskview.js` rebuilds every child of the bar
but the reply field on each render, and the task view renders on every task update (`update`),
detail read, steps event, pull request and browser request. A running task is updated often: in
the mock its ambient activity touches #12 every 16 s, in production the server's activity, usage
and step updates do.

Root cause: the bar's "Reply" for a question in the terminal (`needs_attention`, and Needs you while
the agent asks in the terminal) is a new `button` on each render; the Needs you / waiting "Reply"
(`replyButton`, SPEC-156) is kept but taken out of the page and put back. A browser fires `click` on
the nearest common ancestor of the press and the release targets: when the pressed button is
replaced (or detached) between `pointerdown` and `pointerup`, the click lands on the bar, which does
nothing. So a click that spans an update is lost: "sometimes". The same rebuild drops the keyboard
from the terminal Reply to `body` (the `kept` focus guard of #506 only restores a node that is still
in the page), so Enter after an update does nothing either.

Reproduced red on dev with three e2e tests in `e2e/tests/reply.e2e.ts` (press Reply, deliver a task
update, release): the terminal is not focused, the reply field does not open, the keyboard is on
`body`.

What else the cause could break: every button the bar rebuilds (Start now, Cancel, Hand over,
Resume, Open browser tab, Request control) loses a click the same way; the fix is at the bar's
level, so it covers them. The held-prompt card and the queue card render only on a key change and
are not affected.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| RPL-001 | While a pointer is pressed inside the attention bar, a render of the bar is held back; it runs once the press ends (release or cancel, after its click), so the pressed button is the one released and its click acts. | e2e `reply.e2e.ts` (#406 tests) |
| RPL-002 | The update held back during a press shows once the press ends. | e2e |
| RPL-003 | The terminal Reply (agent asks in the terminal) is one button kept across renders, like the field Reply, so the keyboard stays on it across task updates. | e2e |
| RPL-004 | A press that never ends inside the page (released outside the window, the window losing focus) does not freeze the bar: a lost window focus or a pointer cancel ends it too. | e2e (window blur during a press) |

## Plan

- [x] Spec with the root cause of the lost Reply click
- [x] A click on the attention bar survives a task update and the keyboard stays on Reply
- [x] Docs, ADR and changes record the held render
- [ ] Independent review approves the head
- [ ] Every gate green with dev merged

## Acceptance criteria

- AC-1: with `activity: needs_attention`, the mouse pressed on Reply, a task update shown in the
  strip, then released: the terminal has the keyboard.
- AC-2: with a Needs you request, the mouse pressed on Reply across a task update that changes the
  request, then released: the reply field is open with the caret, and the bar shows the changed
  request.
- AC-3: the keyboard on the terminal Reply stays on it across a task update, and Enter focuses the
  terminal.

## Test plan

- e2e `reply.e2e.ts`: three new tests for AC-1 to AC-3; all three fail on dev (terminal not
  focused, field not opened, focus on `body`).
- The existing reply, watching, pause and needs-you e2e tests keep passing (the bar's rebuild on
  updates, focus kept on Reply after Escape, the request list's scroll).
- The whole e2e suite, as every web change.

## Decisions

- Hold the bar's render during a press rather than diff its children or keep every button as a
  persistent node: the bar has a dozen buttons over seven states, built from closures over the
  current task; a reconciler or per-button nodes would be a rewrite of the bar for the same
  guarantee. Holding the render covers every button at once and delays an update by a click's
  length only (ADR-0268).
- The terminal Reply also becomes a kept node (like `replyButton`): holding renders fixes the
  pointer, not the keyboard, which the existing focus guard keeps only for nodes that survive.
- The held render runs on a task after the release (`setTimeout 0`), so the click, dispatched after
  `pointerup` in the same task, reaches the pressed button first.
