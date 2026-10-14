---
yulius-task: 38a7ee87-dd89-4f68-bc9f-1ef0938d7f16
status: done
---

# SPEC-132: the top bar has no Superagent button; the tools rail opens it

Status: implemented
Date: 2026-10-14
Owner: Yulius task #120
Scope authorization: task #120, "delete superagent button from here as it's already right under the
button", with a screenshot of the top bar's right end: Status, notifications, a **Superagent**
button and the tools toggle.

## Goal

The top bar no longer shows a Superagent button. The Superagent is opened where it already sits:
first in the tools rail, right under the tools toggle. Its links (`/s/<slug>/superagent`, `/agent`,
`/cockpit`) keep opening it, and nothing else about the panel changes.

## Context

- `crates/yulius-server/web/assets/layout.js`, `createTabRow()`: the tab row's right end is the
  update banner, "Not isolated", Status, notifications, the Superagent button (`.agent-toggle`,
  Show/Hide Superagent, SPEC-035 AGT-005, SPEC-074 SUP-007) and the tools toggle. The button calls
  `ctx.toggleAgent` (`app.js`), which opens the panel with `persistRail: false`, and follows the
  panel through `ctx.onPanelChange`, which nothing else sets.
- `panels.js`: the rail lists Superagent first (`KINDS`). A panel opened while the rail is hidden
  shows the rail only until the panel closes (`railForPanel`); besides the top-bar button, the
  Superagent links (`app.js` route handling) and the return to a sandbox's tool (SPEC-113 KST-005)
  open panels that way, so that behaviour stays.
- `styles.css`: `.btn-agent`, `.agent-toggle` (also hidden off sandbox pages) and the phone rule
  that shrank the button to an icon.
- e2e: `agent-panel.e2e.ts`, `places.e2e.ts` and `sessions.e2e.ts` open the Superagent with the
  top-bar button; `rail.e2e.ts` already asserts the rail order (Superagent first).
- Docs describing the button: ADR-0036, SPEC-035 AGT-005, SPEC-074 SUP-007, `docs/product.md`,
  `crates/yulius-server/web/README.md`, `e2e/README.md`.

## Requirements

| ID | Requirement | Verification |
| --- | --- | --- |
| NSB-001 | The tab row has no Superagent control on any page or width; its right end is Status, notifications and the tools toggle (plus the update banner and "Not isolated" when they apply). | e2e `agent-panel.e2e.ts` |
| NSB-002 | The rail's Superagent button opens and closes the Superagent beside the page, with the same session, notice and keyboard behaviour as before; `/s/<slug>/superagent`, `/agent` and `/cockpit` still open it, showing the rail only while the panel is open. | e2e `agent-panel.e2e.ts`, `places.e2e.ts` |
| NSB-003 | No code, style or document is left for the removed button (`toggleAgent`, `onPanelChange`, `.agent-toggle`, `.btn-agent`); ADR-0106 records the change and amends ADR-0036; SPEC-035, SPEC-074, the product summary and the UI READMEs say how the Superagent is opened now. | review |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] The top bar has no Superagent button; the tools rail opens it, and the e2e suite proves it
- [x] ADR, specs, product and UI docs describe the rail as the Superagent's way in
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- On any sandbox page the top bar shows Status, notifications and the tools toggle, and no
  Superagent button (also at 390 px).
- Show tools, then Superagent in the rail, opens the Superagent beside the page; clicking it again
  closes it; its session survives hiding, other tools and other pages.
- `/s/studio/agent` opens the sandbox with the Superagent and the rail, and closing it hides the
  rail again when the person had it hidden.

## Test plan

- e2e (mock): a new test that the tab row has no Superagent control at desktop and phone widths
  (fails before the change); every test that clicked "Show/Hide Superagent" opens it from the rail
  instead; the transient-rail tests that started from the top-bar button start from the `/agent`
  link, which keeps that behaviour.
- `scripts/dev check`, `scripts/dev lint`, `scripts/dev test`, `scripts/dev security`, the e2e suite.

## Decisions

- **Remove the button, keep the rail entry** (ADR-0106): the person asked for it; the rail's
  Superagent is one click from the toggle right above it, and one way in is clearer than two.
  Alternatives: keep the button and drop the rail entry (the rail is where every tool lives, and
  rail.e2e asserts the order), or show the button only while the rail is hidden (two controls
  that move around).
- **Keep the transient rail for links and sandbox returns**: those still open a panel while the
  rail is hidden, and the person's Tools choice must survive them.

## Evidence

Run on the head after merging `origin/dev` (4929ee47), 2026-10-14:

| Command | Result |
| --- | --- |
| `npx e2e run tests/agent-panel.e2e.ts --grep "top bar has no"` before the change | 2 failed: the tab row held "Show Superagent" (1280 and 390 px), as NSB-001 expects |
| same test with `panels.js:341` set to `{ context: true }` (review round 1 test) | failed: the rail stayed after the returning tool closed; passes with the code as merged |
| `scripts/dev check` | pass |
| `scripts/dev lint` | pass |
| `scripts/dev test` | pass: 1666 tests passed, 16 skipped |
| `scripts/dev deps` | pass: cargo-deny and cargo-shear found no issues |
| `cd e2e && npx e2e run` | pass: 401 passed, 1 skipped (`agent.e2e.ts`, the live-model test) |
| `scripts/dev security` | not run (exit 2): Vidoc answers HTTP 401 "Invalid or expired token" for the bound `VIDOC_API_KEY`; tracked in #470 |

Independent review round 1 (fresh session, `om-code-review`, head 5416a7ab): approved with two
minors and three nits. Fixed: the sandbox-return path of the transient rail got its own e2e test
(proved failing without the behaviour); the spec's Plan and status; the toggle label names Notes
too; dev merged with the ADR index and changes log checked. Accepted: opening the Superagent from
a hidden rail takes two clicks (ADR-0106 Consequences).

