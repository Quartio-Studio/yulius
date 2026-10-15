---
yulius-task: a0bcb221-6c4a-4e7d-9e4d-bd4100dc436f
status: done
---

# SPEC-156: Reply in the attention bar opens a reply field

Status: implemented
Date: 2026-10-15
Owner: Yulius task #147
Scope authorization: task #147, "when i click reply, nothing happens. Fix." (screenshot: a
"Needs you: Add a GitHub ruleset…" bar with its Reply button). Fixes #516.

## Goal

When a person clicks **Reply** in a task's attention bar on a desktop, something visibly happens.
A reply field opens in the bar with the keyboard in it. The person types an answer and sends it
(Enter or Send), and the agent gets it as a typed prompt. Escape or Cancel closes the field and
sends nothing.

## Context

- Symptom: on a desktop, clicking Reply in "Needs you: …" (SPEC-134 CDT-004) or "The agent is
  waiting for your reply." changes nothing on screen.
- Root cause: `crates/yulius-server/web/assets/taskview.js` `reply()` only calls
  `terminal.focus()` (phones focus the quick input). Focus does reach xterm's hidden textarea; an
  e2e probe on the mock showed `document.activeElement` is `.xterm-helper-textarea` after the
  click. Claude Code hides the terminal cursor and draws its own, though, so the bar stays as it
  was and nothing hints where to type. No test clicked Reply: `cd.e2e.ts` only checks the bar's
  words and its Open link.
- A person's typed prompt ends the `needs_you` request (`hooks.rs` `hook_prompt_submit`: a typed
  prompt after recent keystrokes on the terminal socket). Text sent with `terminal.sendKeys` goes
  over that same socket, as the phone quick input's text does, so it counts as typed.
- `renderAttention()` rebuilds the bar's children on every task update. An `input` inside the bar
  loses the keyboard, an IME composition and a screen reader's place when it is removed and put
  back.
- `needs_you` stays set while the agent goes on (`status.js` `taskInfo`), including while Claude
  Code shows a permission menu (`activity: needs_attention`).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| RPL-001 | On a desktop, Reply in the "Needs you" and "waiting for your reply" bars opens a reply field ("Reply to the agent") with Send and Cancel, and the keyboard is in it. | `reply.e2e.ts` |
| RPL-002 | Enter or Send types the text into the task's terminal and submits it (text, then a separate Enter, as the quick input does); the field closes and empties. Empty text sends nothing. | `reply.e2e.ts` |
| RPL-003 | Escape or Cancel closes the field and sends nothing; the text typed so far is kept for the next Reply. | `reply.e2e.ts` |
| RPL-004 | A task update that re-renders the bar while the person types keeps the field, its text, the keyboard and the caret: the field is never taken out of the bar. A field that closes while someone types (or on Escape) leaves the keyboard on the bar, which ignores keys, never on the terminal; the draft is kept. | `reply.e2e.ts` |
| RPL-005 | Phones keep their quick input: Reply focuses it and no field opens. | `reply.e2e.ts` "Reply on a phone…" |
| RPL-006 | While the agent asks something in the terminal (`needs_attention`, alone or under a Needs you request), Reply focuses the terminal and no field opens, so no typed text picks a menu option unseen. | `reply.e2e.ts` "…while the agent asks in the terminal…" |

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] Fix the cause until the regression test and every gate pass
- [x] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- Clicking Reply on "Needs you: Add a ruleset" shows a focused "Reply to the agent" field. Typing
  "Ruleset added" and pressing Enter shows `> Ruleset added` in the task's terminal and closes
  the field.
- Escape closes it without anything reaching the session.
- A task update while typing keeps the text and the keyboard in the field.
- The page has no axe violations with the field open.

## Test plan

- Regression: `e2e/tests/reply.e2e.ts` fails on the unchanged view (no "Reply to the agent"
  textbox after the click) and passes with the fix. It waits on conditions: the textbox, the
  terminal text, and the mock session's input. It never waits on time.
- Gates: `scripts/dev check`, `scripts/dev lint`, `scripts/dev test`, the e2e suite,
  `scripts/dev security`.

## Decisions

- **A reply field in the bar, not a visible focus effect.** Options: (a) keep focusing the
  terminal and add a highlight or hide the bar; (b) open a field in the bar that sends through the
  terminal. (a) still leaves the person to find Claude Code's prompt line, possibly below a
  running turn's output. (b) gives the click a visible result on any screen and reuses the quick
  input's way of typing into the session, so the server needs no change. A person's reply is
  still a typed prompt, so it still ends `needs_you`. Chosen: (b). This is local to the view and
  easy to reverse, so it needs no ADR.
- A one-line `input`, as the quick input: a newline would submit early in Claude Code, and a
  longer answer can still be typed in the terminal.
- "The agent needs your attention" keeps focusing the terminal. There the answer is usually a
  choice in Claude Code's menu (arrow keys), not text. The same holds under a Needs you request
  (RPL-006).
- Review round 1 (fresh session, `17050ad9`, request changes) fixed test-first:
  - major: the field could answer a permission menu the person could not see (RPL-006);
  - the field is now kept in the bar (hidden when closed) rather than restored after each rebuild,
    for IME compositions and screen readers;
  - Safari's IME Enter (`keyCode 229`) is ignored, as elsewhere in the UI;
  - a field closing under typing moves the keyboard away from it;
  - the phone path and "empty Send types nothing" are tested.
  Not done: clearing a kept draft when a new `needs_you` arrives. The person sees the draft and
  can change it before sending.
- Review round 2 (fresh session, `cf907627`, request changes) fixed test-first:
  - major: handing the keyboard to the terminal when the field closes under typing sent the rest
    of the typing there, possibly into a permission menu (Enter approves, a digit picks). The
    keyboard now rests on the bar (`tabindex=-1`), which ignores keys. This also covers focus on
    Send/Cancel, and Escape no longer passes through the terminal on its way to Reply.
  - the re-render test now waits on a visible change of the bar before it checks focus and caret.
  Not done (not regressions):
  - with an Open link, the link wraps to its own row while replying;
  - read-only viewers can open the field, and the terminal refuses the send with its notice, as it
    did for the Reply button and the quick input.
- Review round 3 (fresh session, `f97be33a`) approved, with no blocker or major. Its cheap
  minors are done here:
  - a closed field never sends (`sendReply` checks `replying`);
  - the bar shows a focus ring when it holds the keyboard;
  - tests assert the keyboard returns to Reply on Escape, and that Enter after a close sends
    nothing;
  - the docs name the menu, close-under-typing and phone cases.
  Left as they were (not regressions): a focused Reply button drops the keyboard when the bar
  updates, as every button in the bar always did; a window narrowed to phone width keeps an open
  field.
  A final review in a fresh session checks this last delta.

## Evidence

- Reproduction (read-only probe, not committed): after clicking Reply on a Needs you bar in the
  mock, `document.activeElement` was xterm's `.xterm-helper-textarea`. Focus moved, but nothing
  changed on screen.
- Regression first:
  - `f4e932eb`: `npx e2e run tests/reply.e2e.ts` failed 3/3. There was no "Reply to the agent"
    textbox, and focus stayed on "Terminal input".
  - `fb311c08` (round 1 tests): 2 failed, the needs_attention case and the close-under-typing case.
  - `42aa7a41` (round 2 test): 1 failed, the keyboard was not on the bar.
  - Each passed with the fix that followed.
  - A mutation check (focus restore removed at `17050ad9`) made the re-render test fail.
- `reply.e2e.ts` with `cd.e2e.ts`, `task-done.e2e.ts`, `phone.e2e.ts`, `supertask.e2e.ts` and
  `prepared.e2e.ts`: passed.
- Full e2e at `f97be33a` (after merging `origin/dev`), host load about 50-66: 466 passed,
  1 skipped, 2 failed. Both failures were in `notes.e2e.ts` (tab order and grip polls), a file
  this change does not touch, and are the known load flakes #503 and #510. `notes.e2e.ts` alone
  then passed 35/35.
- Load flakes met on the way:
  - `ownership.e2e.ts` "hands the task over" failed under load on this branch and on `origin/dev`
    (scratch worktree), and passed alone.
  - One timeout in the new reply test (an attention-bar text wait at load ~66) did not reproduce
    in the next 5 runs (30 tests).
  - Both are recorded on #506; no retries were added.
- `scripts/dev check` and `scripts/dev lint` passed after the dev merge.
- `scripts/dev test`: not run here, because `cargo-nextest` is not installed in this sandbox. No
  Rust changed. Yulius CI runs it on the pull request.
- `scripts/dev security`: exit 2, not run. Vidoc answers 401 to the stored key (#470, ADR-0104).
  It counts as unrun, not passed.
- Visual check in the Yulius browser (mock, port 20107):
  - Reply on "The agent is waiting for your reply" opened the field under the words, with Send
    and Cancel.
  - Enter typed "Ruleset added" into the session, the agent answered, and the bar closed.
