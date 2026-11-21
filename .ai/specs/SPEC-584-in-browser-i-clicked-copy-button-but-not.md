---
yulius-task: 94533bcb-3c26-400e-accf-9972ce2e54ae
status: draft
---

# SPEC-584. Copy text from a webpage in the sandbox Browser

## Goal

A person can click a webpage’s Copy button in the Browser panel and paste its text on their device.

## Context

The panel streams another Chromium process. Copying a selection with the keyboard asks for the selected text over the viewer socket. A webpage’s button can write different text directly to the sandbox clipboard. A pointer event currently has no path for returning that text. The existing live test selects a heading before pressing the copy shortcut. The separate Codex terminal menu is covered by [SPEC-582](SPEC-582-fix-clipboard-on-codex.md).

## Requirements

- **BCP-001** A text copy initiated by clicking a webpage on the sandbox’s virtual display reaches the clicking viewer. Preserve multiline and Unicode text.
- **BCP-002** An ordinary click leaves the viewer’s clipboard alone. Limit transfers to fresh text, the initiating viewer and the existing clipboard size limit. Cancel outstanding work when the input is superseded or the viewer leaves.
- **BCP-003** Keep keyboard selection copy working. When the viewer’s browser refuses an automatic write, give the person a way to complete the copy.

## Plan

- [ ] Reproduce the browser Copy failure
- [ ] Capture the clipboard regression in a spec and test
- [ ] Make Copy deliver the requested text
- [ ] Verify, review and merge the fix into dev

## Acceptance criteria

- **AC-1 (BCP-001)** With no selection, click Copy on a fixture that writes a known multiline string. Read that exact string from the viewer’s clipboard.
- **AC-2 (BCP-002)** An ordinary click with an older value on the sandbox clipboard transfers nothing. Another viewer receives no text.
- **AC-3 (BCP-003)** The selection shortcut still copies the heading. A refused local write offers an explicit copy gesture and reports success only when it works.

## Test plan

Exercise the button through the streamed panel using separate sandbox and viewer browsers. Cover idle viewers and refusal in targeted tests. Run repository quick checks, web end-to-end tests and Yulius CI.

## Decisions

The report names the browser without identifying a site. Use a synthetic page in the sandbox Browser panel with a native plain-text clipboard write. Restrict display clipboard access to that sandbox’s private virtual display.
