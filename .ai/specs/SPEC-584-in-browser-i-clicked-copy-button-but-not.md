---
yulius-task: 94533bcb-3c26-400e-accf-9972ce2e54ae
status: done
---

# SPEC-584. Copy text from a webpage in the sandbox Browser

## Goal

A person can click a webpage’s Copy button in the Browser panel and paste its text on their device.

## Context

The panel streams another Chromium process. Copying a selection with the keyboard asks for the selected text over the viewer socket. A webpage’s button can write different text directly to the sandbox clipboard. A pointer event currently has no path for returning that text. The existing live test selects a heading before pressing the copy shortcut. In the two-browser reproduction the page acknowledged its write and the viewer timed out reading the expected text. [Issue #1587](https://github.com/Quartio-Studio/yulius/issues/1587)

The separate Codex terminal menu is covered by [SPEC-582](SPEC-582-fix-clipboard-on-codex.md).

## Requirements

- **BCP-001** A fresh clipboard write following a click on the sandbox’s virtual display offers **Copy to my clipboard** to that viewer. Accepting transfers the text to their device. Preserve multiline and Unicode text.
- **BCP-002** An ordinary click leaves the viewer’s clipboard alone. Limit transfers to fresh text, the initiating viewer and the existing clipboard size limit. The offer carries no text. Read the selection only after explicit acceptance on the originating socket. Expire replaced and unused offers.
- **BCP-003** Keep keyboard selection copy working. When the viewer’s browser refuses the accepted write, give the person a way to complete the copy.

## Plan

- [x] Reproduce the browser Copy failure
- [x] Capture the clipboard regression in a spec and test
- [x] Make Copy deliver the requested text
- [x] Verify, review and merge the fix into dev

## Acceptance criteria

- **AC-1 (BCP-001)** With no selection, click Copy on a fixture that writes a known multiline string. Verify the device clipboard stays unchanged until the viewer accepts, then read the exact string.
- **AC-2 (BCP-002)** An ordinary click with an older value on the sandbox clipboard transfers nothing. Another viewer cannot accept the offer. A consumed offer cannot be replayed.
- **AC-3 (BCP-003)** The selection shortcut still copies the heading. A refused local write offers an explicit copy gesture and reports success only when it works.

## Test plan

Exercise the button through the streamed panel using separate sandbox and viewer browsers. Cover idle viewers and refusal in targeted tests. Run repository quick checks, web end-to-end tests and Yulius CI.

## Decisions

The person confirmed the Copy button was on a webpage inside the Yulius Browser panel. The regression uses a synthetic page with a native plain-text clipboard write. Restrict clipboard access to that sandbox’s private virtual display. X reports a change of clipboard owner without attributing it to a webpage or person. The local action explicitly requests a transfer from the shared sandbox clipboard.

[ADR-0423](../../docs/decisions/0423-a-webpage-copy-returns-through-the-clicking-viewers-connection.md) records the display transfer and its lifetime.

## Evidence

- The two-Chromium regression failed on the original implementation after the fixture acknowledged its write. The viewer timed out waiting for the exact multiline UTF-8 text. The corrected keyboard assertion also awaits the actual clipboard read.
- The browser E2E regression failed before the Browser panel handled the offer. The focused suite now passes, including a refused asynchronous write followed by a native copy gesture.
- The two clipboard decoding unit tests pass. They cover ordinary UTF-8, invalid or incomplete data, characters split across incremental chunks and the combined byte limit.
- Independent review found four issues. The shared release-input predicate initially failed the idle-observer regression. An observer closing its socket now releases held input while the initiating viewer can still accept. All 64 browser crate unit tests pass.
- A simulated deadline between acceptance and its reply initially lost the reply. The focused browser suite now verifies a separate reply deadline, native retry, and an expired action that retains keyboard focus. Both the offer and retry erase usable clipboard state on expiry.
- The real-browser fixture now seeds the device clipboard before the first click and checks that exact value before acceptance. This assertion also fails if a clipboard read is denied.
- `scripts/dev security` reported the project’s Vidoc scan disabled.
- On the disposable Linux display, two Chromium processes exercised a page button with no DOM selection, the exact pre-existing device value before acceptance, the multiline UTF-8 result after acceptance, and keyboard copy. The stream used JPEG fallback. All 39 browser socket integration tests passed. The full Chromium UI suite passed 1,300 tests and skipped one opt-in agent test.
- The private Xvfb selection-owner fixture passed. It exercises the native event path with an existing selection, verifies there is no selection request before confirmation, and checks wrong, consumed and replaced offers.
- Review of the Xvfb fixture added server round trips before both negative assertions. A synthetic request queued on the fixture’s own display made the check fail with “selection requested before acceptance”. Removing the synthetic request restored the passing test.
- The review-fix mutation run listed 6 mutants, caught 4, missed 1 and found 1 unviable. The new private-display fixture catches the missed no-op X command. A package-scoped rerun of that exact mutant reported “1 mutant tested in 13s: 1 caught”.
- The first whole-change mutation run overlapped edits and some copies mixed source revisions. Its results were discarded. A detached checkout of the corrected implementation supplied the repeat run.
- Focused mutation testing of native clipboard handling initially missed 15 changes to expiry, cancellation and late X events. A real private-display fixture now checks expired and abandoned requests, wrong windows and properties, and a complete incremental transfer. The repeat covered all 56 native clipboard mutants: 50 caught, 6 unviable, 0 missed. All 66 browser crate unit tests passed.
- The stable whole-change mutation run reported “mutants: 105 listed, 70 caught, 15 missed, 0 timed out, 20 unviable, 0 not reached (bound 30 min, took 1326 s)”. Every one of the 15 names appears in the caught list of the strengthened native tests.
- Independent review approved commit `62c1fba53`. The subsequent changes add mutation regression tests and evidence. Following a clean merge of `dev` at `a643b6906`, the real two-Chromium regression and all 39 Browser integration tests passed again. The final PR checks and merge SHA are recorded in task #568’s delivery evidence.
