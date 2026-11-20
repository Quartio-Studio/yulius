---
yulius-task: e32112f0-5b55-42c0-8d9c-80782ff60ab5
status: in-progress
---

# SPEC-564 – Name the assistant in session transcripts

## Goal

People reading a transcript with assistive technology hear which assistant produced an entry. Fixes #1455.

## Context

On dev at `a561121e7`, the shared transcript renderer in `pages/sessions.js` writes Claude into every assistant entry's screen-reader text. The Sessions search results also use a fixed Claude role label. The watch view already knows the task's harness and each Sessions API response includes the document's harness. SPEC-525 normalized messages from all three agents. The earlier browser tests checked prompt labels and assistant content without checking the assistant's name.

## Requirements

- AN-1. In a watched task and on its Sessions transcript, assistant entries identify Claude, Codex or Antigravity according to the harness. Subagent entries retain the “(subagent)” suffix.
- AN-2. Assistant hits in Sessions search use the matching document's assistant name. Prompt and other role labels retain their meanings.

## Plan

- [x] Reproduce the assistant name on dev and specify the correction
- [x] Exercise assistant names in both transcript views
- [ ] Render and verify the session’s assistant name
- [ ] Obtain independent approval and record delivery evidence

## Acceptance criteria

- AC-1 (AN-1). Open each harness's watched task and its session document. The screen-reader prefix on an answer matches that harness's short name in each view.
- AC-2 (AN-2). Search for an assistant answer. Each hit identifies the assistant of its own session, including when an imported Claude session and a task session match together.

## Test plan

Exercise the three harnesses through the Chromium watch, search and saved-transcript routes with synthetic normalized messages. Check a prompt and a subagent entry as controls. Run against the original renderer first, then verify the corrected renderer and the web suite.

## Decisions

Use the existing harness name lookup and its Claude fallback for older metadata. Watch uses the task's lifetime harness and Sessions uses the returned document's harness. The correction needs no new API or storage field.

## Evidence

- The new Chromium cases failed on dev `a561121e7` for Codex and Antigravity. Both reported Claude in the watch view, in search and on the saved transcript. Claude passed as the control.
- `npx e2e run tests/watching.e2e.ts --grep 'watch and Sessions identify'` passed all three harness cases, including replacing a live tool call when its result arrives and searching alongside an imported Claude session.
- A separate Playwright `ariaSnapshot()` of the first assistant entry contained the expected name in Chromium for each of the three harnesses in each view (six checks). The inputs were synthetic. Native screen-reader audio was not exercised.
