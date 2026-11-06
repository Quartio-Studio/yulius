---
yulius-task: 1b0c5dd5-ae03-44a8-9ac1-9518a90a3f28
status: in-progress
---

# SPEC-428: The two Superagents are tabs of one Superagent panel

Status: in-progress
Date: 2026-11-06
Owner: Yulius task #410
Scope authorization: task #410 (`/goal`): "this 2-superagents-thingy should be somewhere here [the
sandbox Superagent panel] as a section like here [the Notes tabs Team / This sandbox / Me]. The
example task will be: check out what sandboxes needs between each other and solve it".
Changes the web UI of SPEC-427 GSA-006; decided in
[ADR-0272](../../docs/decisions/0272-the-two-superagents-are-tabs-of-one-panel.md).

## Goal

The Superagent panel shows both Superagents as tabs, as the Notes panel shows its notepads:
**This sandbox** (the sandbox Superagent) and **All sandboxes** (the person's Global Superagent).
Inside a sandbox the Global Superagent is one click away instead of only on the pages outside a
sandbox, and its message box suggests the example of what it is for: "check what my sandboxes need
from each other and solve it".

## Context

- SPEC-427 GSA-006 (task #409, merged 2026-11-06): outside a sandbox the rail's Superagent button
  opens the Global Superagent (title "Global Superagent"); inside a sandbox it opens that sandbox's
  Superagent, whose note links to `/superagent` (All sandboxes). Entering a sandbox while the Global
  Superagent is open closes the panel, so no sandbox session starts by itself.
- Notes (`notes.js`, SPEC-041/SPEC-103/SPEC-112) is a `term-tabs` tab strip (role `tablist`) above
  its panes; its choice is remembered per browser (`yulius.notesTab`).
- `panels.js` keeps one view per (tool, sandbox) (`agent:<id>`, `agent:none` outside), hiding the
  views of the sandbox left and keeping up to 3 sandboxes' views (SPEC-113).
- The Global Superagent is one session per person; its terminal sends resizes (review round 3 of
  SPEC-427), so two live terminal views of it in one page would size it against each other.
- Nothing on the server changes: the endpoints, the session and its instructions stay as SPEC-427.

## Requirements

- SAT-001: the Superagent panel (title "Superagent") has a tab strip "Superagents" with two tabs,
  **This sandbox** and **All sandboxes**, styled and keyed as the Notes tabs (click; ←/→, Home and
  End move between enabled tabs). This sandbox holds the sandbox Superagent as before (its note,
  connection banner, task requests, terminal); All sandboxes holds the Global Superagent as before
  (its note, read-only terminal, message box).
- SAT-002: in a sandbox the panel shows the tab chosen last (remembered per browser,
  `yulius.superagentTab`, This sandbox until the person picks one), and the shown tab is the same in
  every sandbox. A session starts only when its tab is first shown: picking All sandboxes in a
  sandbox starts no sandbox Superagent.
- SAT-003: outside a sandbox (All sandboxes, Settings, `/superagent`) the panel shows All sandboxes;
  This sandbox is disabled there ("Open a sandbox first") and the remembered choice is not changed.
- SAT-004: one Global Superagent view serves the whole page: whichever sandbox's panel (or the one
  outside) shows the All sandboxes tab holds its terminal and message box, so the session is
  attached once and keeps what was typed in its box from one sandbox to the next.
- SAT-005: a sandbox entered while the panel shows the Global Superagent (from outside a sandbox,
  or from another sandbox on All sandboxes) keeps the panel open on All sandboxes; no sandbox
  Superagent starts (replaces GSA-006's "closes the panel instead"). `/s/<slug>/superagent`
  opens the This sandbox tab.
- SAT-006: the sandbox Superagent's note points to the other tab: "Work across sandboxes: the
  Global Superagent, in **All sandboxes**." where All sandboxes is a button that selects that tab.
- SAT-007: the Global Superagent's message box suggests the example: placeholder "For example:
  check what my sandboxes need from each other and solve it".

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] The Superagent panel shows This sandbox and All sandboxes as tabs
- [ ] One Global Superagent view follows the tabs across sandboxes and pages
- [ ] The sandbox note selects the All sandboxes tab and the box suggests the example
- [ ] ADR, architecture, product and changes describe the tabs
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-SAT-1 (e2e): in `studio` the Superagent panel shows the tabs This sandbox (selected) and All
  sandboxes; All sandboxes shows the Global Superagent's note, terminal and the example
  placeholder; This sandbox shows the sandbox's terminal again; axe finds no violation.
- AC-SAT-2 (e2e): with All sandboxes chosen, a reload and another sandbox show All sandboxes, no
  sandbox Superagent was started, and the Global session was opened once in the page.
- AC-SAT-3 (e2e): on All sandboxes (the page) the panel shows All sandboxes with This sandbox
  disabled; entering a sandbox keeps it open on All sandboxes and starts no sandbox Superagent.
- AC-SAT-4 (e2e): the sandbox note's All sandboxes button selects that tab; ←/→ move between tabs.
- AC-SAT-5 (e2e): text typed in the Global box in one sandbox is still there in another.

## Test plan

- e2e against the mock: `e2e/tests/global-superagent.e2e.ts` updated (AC-SAT-1 to AC-SAT-5);
  `agent-panel.e2e.ts` and `superagent-requests.e2e.ts` still pass with the This sandbox tab default.
- No Rust change; `scripts/dev ci` for the repository contracts and docs.

## Decisions

- **Tabs in the one panel, not a second rail tool.** The person asked for a section like Notes';
  a second rail button would split one concept in two places and leave the rail's labels unclear.
- **One shared Global view, moved between panels.** A view per sandbox would attach the one session
  several times and size it against itself; moving the element keeps one terminal and one box.
- **No dragging of the two tabs.** Notes' order is a preference among three notepads; two fixed
  tabs, local first, need none.
- **The choice is shared by every sandbox.** All sandboxes is the same session everywhere, so a
  person watching it while moving between sandboxes keeps watching it.
- **The example as the box's placeholder**, where the person types, rather than a button that sends
  it: it is the person's to send.
