---
yulius-task: 1b0c5dd5-ae03-44a8-9ac1-9518a90a3f28
status: done
---

# SPEC-428: The two Superagents are tabs of one Superagent panel

Status: done
Date: 2026-11-06
Owner: Yulius task #410
Scope authorization: task #410 (`/goal`): "this 2-superagents-thingy should be somewhere here [the
sandbox Superagent panel] as a section like here [the Notes tabs Team / This sandbox / Me]. The
example task will be: check out what sandboxes needs between each other and solve it".
Changes the web UI of SPEC-427 GSA-006; decided in
[ADR-0272](../../docs/decisions/0272-the-two-superagents-are-tabs-of-one-panel.md). The Global
message box is removed by [SPEC-430](SPEC-430-this-should-be-exactly-like-normal-super.md); its example
moved into the Global note.

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
- SAT-002: in a sandbox the panel shows the tab the person picked last (a click, the keys or the
  note's button, remembered per browser, `yulius.superagentTab`, This sandbox until the person picks
  one; a tab shown for them, as in SAT-005, is not remembered), and the shown tab is the same in
  every sandbox: the page holds one tab in front, read from storage once at load and then set by
  whichever sandbox's panel shows a tab, so a panel shown again (a new page, another sandbox, the
  rail, a sandbox's remembered tool) shows it, whatever another browser tab stored meanwhile. The
  tab keys with Alt, Ctrl or Meta held do nothing. A session starts only when its tab is first shown:
  picking All sandboxes in a sandbox starts no sandbox Superagent.
- SAT-003: outside a sandbox (All sandboxes, Settings, `/superagent`) the panel shows All sandboxes;
  This sandbox is disabled there ("Open a sandbox first") and the remembered choice is not changed.
- SAT-004: one Global Superagent view serves the whole page: whichever sandbox's panel (or the one
  outside) shows the All sandboxes tab holds its terminal and message box, so the session is
  attached once and keeps what was typed in its box from one sandbox to the next.
- SAT-005: a sandbox entered while the panel shows the Global Superagent (from outside a sandbox,
  or from another sandbox on All sandboxes) keeps the panel open on All sandboxes, also when that
  sandbox had another tool open last (it wins over SPEC-113 KST-005), and so does leaving the
  sandboxes; no sandbox Superagent starts and the keyboard stays with the page (replaces GSA-006's
  "closes the panel instead"). `/s/<slug>/superagent` opens the This sandbox tab.
- SAT-006: the sandbox Superagent's note points to the other tab: "Work across sandboxes: the
  Global Superagent, in **All sandboxes**." where All sandboxes is a button that selects that tab.
- SAT-007: the Global Superagent's message box suggests the example: placeholder "For example:
  check what my sandboxes need from each other and solve it".
- SAT-008: while the sandbox Superagent asks to start a task (SPEC-335), the This sandbox tab shows
  a dot and is described "Your Superagent asks to start a task", also when All sandboxes is in front
  and This sandbox was never shown in the page: the sandbox view is made with the panel (its
  requests read), its session still starts only when its tab first shows (review round 3).

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] The Superagent panel shows This sandbox and All sandboxes as tabs
- [x] One Global Superagent view follows the tabs across sandboxes and pages
- [x] The sandbox note selects the All sandboxes tab and the box suggests the example
- [x] ADR, architecture, product and changes describe the tabs
- [x] Review round 1 fixed: the tab in front kept, only picked tabs remembered, more paths tested
- [x] Review round 2 fixed: one tab in front per page, keys with a modifier ignored
- [x] Review round 3 fixed: a hidden request marks the This sandbox tab
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

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

## Evidence

At `78f8bd3d` on 2026-11-06 (`origin/dev` had no newer commit to merge):

- `scripts/dev ci`: exit 0. It covers the repository contracts (docs links, unique decision
  numbers, design tokens), format, clippy, docs, macOS targets and deps, and
  `cargo nextest run --workspace --profile ci`: 3164 tests run, 3164 passed, 16 skipped. It also
  passed at `458527ae` and `873b59a1`.
- `cd e2e && npx e2e run` (the whole suite, 33 min): 858 passed, 1 skipped, 1 failed.
  - Skipped: `agent.e2e.ts`, which needs a model.
  - Failed: `editor.e2e.ts` "a folder chosen while the worktrees are read switches to it once
    the question is answered". This is the known load flake #1087, filed before this change, in
    the Editor's folder picker, which this change does not touch. Run alone right after, the file
    passed 32 of 32.
- The Superagent suites after the last fix: `global-superagent`, `agent-panel`,
  `superagent-requests`, `places` and `notes`, 113 of 113 passed. The round-4 reviewer also ran
  `rail`, `consistency` and `sessions`: 41 passed.
- New and changed tests:
  - `e2e/tests/global-superagent.e2e.ts` (11 tests):
    - the tabs in a sandbox, with the note, the terminal, the example placeholder and axe;
    - outside a sandbox, This sandbox disabled and skipped by the keys;
    - the note's button and the keys (←/→, Home, End, and a modifier doing nothing);
    - All sandboxes kept across sandboxes, Settings and a reload, with one Global session, its
      box's text kept and no sandbox session started (mock `globalOpens`, `cockpitStarts`);
    - a sandbox entered with it open keeps it, the keyboard staying on the page and nothing
      stored;
    - a choice another browser tab stored changes nothing;
    - it wins over a sandbox's remembered tool;
    - a panel shown again shows the tab picked last elsewhere;
    - `/s/<slug>/superagent` opens This sandbox.
  - `superagent-requests.e2e.ts`: a request while All sandboxes is in front marks the This
    sandbox tab, starting no session.
  - `agent-panel.e2e.ts` and `places.e2e.ts` had their selectors updated for the panes.
- Mutation checks. Each change below made a test fail, and the code was restored afterwards:
  - a Global view per panel instead of one shared view;
  - a re-show re-reading storage instead of the panel's tab;
  - deciding `globalOpen` from storage;
  - a per-panel tab instead of the page's tab;
  - no request count passed to the tab.
- Independent review in fresh sessions, 4 rounds. Every finding in this change's own code was
  fixed test-first:
  - Round 1: a re-show re-read the stored choice, which another browser tab shares, and started
    a sandbox Superagent unasked; tabs were stored without the person picking one; untested
    paths; `destroy()` created the Global view; sign-out re-showed it.
  - Round 2: a closed panel came back on its own stale tab; modifier keys switched tabs.
  - Round 3 (code approved): a request card loaded hidden behind All sandboxes.
  - Round 4: **APPROVE** at `78f8bd3d`. Its minor (no dot for a sandbox whose panel was never
    made in this page) is as SAT-008 states: on `dev`, requests showed only inside the open
    panel. Its nits do no harm: one more link read per resync for a kept panel, and a card made
    while hidden is armed by the time it shows.
- Pre-existing, filed: #1089. Playground guests are offered the Superagent, which the server
  refuses them; this was already so for the sandbox Superagent on `dev`.
- Not run: `yulius security-scan` reports "Security scan disabled". Vidoc is turned off for this
  project in Yulius, so nothing was scanned; this is not a pass.
- Not tested against a live server or model: the panel was driven only against the mock. The
  sessions and endpoints are SPEC-427's and are unchanged.
