---
yulius-task: 7a705d0c-6cb1-4209-afd2-75b62d40be59
status: in-progress
---

# SPEC-113: screens keep their state when you switch away and back

Status: in-progress
Date: 2026-10-14
Owner: task #98
Scope authorization: the person's request (task #98): "When I switch between screens, I want to
keep the state. For example: I went to studio sandbox, clicked claude section, went back to
yulius, clicked something, then went to studio. In studio I had default screen instead the one I
left. Find all places when such situation may happen and fix them all." Issue #435.

## Goal

Coming back to a sandbox, a page or a tool shows it as it was left: the same screen with its
filters, the same tool panel, and the tool's own state (the Superagent session, the browser tab,
the editor's open files with unsaved text, the terminals).

## Context

- **Symptom.** In studio open a task, a page or the Superagent ("Claude"); switch to another
  sandbox with its tab; switch back with studio's tab: studio shows New task (the composer), and
  the tool panel shows whatever the other sandbox had open, or nothing.
- **Root cause (four places).**
  1. Every way into a sandbox links to `/s/<slug>`, the composer: the sandbox tabs and the phone
     sandbox picker (`layout.js` `createTabRow`), the tab menu's Open, and Home's sandbox cards
     (`pages/home.js`). Nothing remembers where a sandbox was left.
  2. The sidebar's page links (`layout.js` `renderNav`) and the back links of a task's overview
     (`pages/taskinfo.js`) and Git changes (`pages/gittask.js`) point at the bare page path, so
     state the page keeps in its address (Tasks filters, the Sessions search, an open CI run, the
     Secrets section) is lost on return.
  3. The panel host destroys every tool view when the sandbox changes (`panels.js` `setContext` →
     `destroyWhere(() => true)`), and the open tool is one value for every sandbox. The editor's
     unsaved text is lost with it.
  4. Two pages keep a filter only in memory: MCP's scope (`pages/mcp.js`) and the Dashboard's
     period (`pages/dashboard.js`), so even a remembered address cannot bring them back.
- **Why tests missed it.** Every sandbox-switch test (`navigation.e2e.ts`, `agent-panel.e2e.ts`,
  `terminals.e2e.ts`) went from one sandbox to a sandbox not visited before, whose composer is the
  right answer; none went back.
- **What else the cause touches.** Notifications, terminal links and New task navigate to exact
  addresses and stay exact. The composer draft (SPEC-026), Notes' tab and the Shell's active
  terminal were already kept in storage.
- **Prior art.** Pages already keep state in their address with `replaceQuietly` (Tasks, Sessions,
  CI, Secrets, Git changes); the router records it. SPEC-026 keeps per-sandbox drafts.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| KST-001 | The router remembers, in this browser tab, the last address (path and query) shown inside each sandbox and on each sandbox page, including changes a page makes with `replaceQuietly`. At most 200 places are kept; sign-out and an expired session forget them, and a sign-in as another person (in another tab of the browser) drops the places left in this tab. | `places.e2e.ts` "signing out forgets …" |
| KST-002 | A sandbox tab, the phone sandbox picker, the tab menu's Open and Home's sandbox cards return to the sandbox's remembered address; without one, or when that sandbox is the one in view, they open its composer as before. | `places.e2e.ts` "a sandbox tab returns …", "the tab of the sandbox in view …", "Home's card …" |
| KST-003 | A sidebar page link and the back links of a task's overview (Tasks), a task's changes (Git), a session (Sessions), an MCP server, a skill, a new skill, a workflow and an automation return to the list's remembered address (its filters), also after their "Leave without saving?" question; a link to the page in view opens it plain, as before. Deleting an MCP server returns to the list's filters too. | `places.e2e.ts` "a page link …", "the back links of a session and an MCP server …", "the Tasks back link …" |
| KST-004 | A page that says something is not there (a task, skill, workflow, MCP server, automation, session or sandbox; `notFound`) is not remembered as the sandbox's place: the next visit opens the composer. A CI run closed because its page went does not rewrite the new address. | `places.e2e.ts` "a missing task …", "a deleted skill …", "closing a CI run …" |
| KST-005 | Each sandbox keeps its open tool, or none, recorded when the person leaves it: coming back opens the tool it had (the rail shown for it, not saved as the person's Tools choice) or closes the panel. A sandbox not visited yet keeps the tool in view, as before. | `places.e2e.ts` "each sandbox comes back …", "a sandbox left without a tool …" |
| KST-006 | The Superagent, Browser, Editor and Shell views survive a sandbox switch for the 3 sandboxes used last (the current one included); an older sandbox's views are destroyed, except an editor holding unsaved changes. A removed sandbox, or one no longer visible, loses its views at once. Notes is rebuilt as before: its Team and Me notepads are the same in every sandbox, and two live copies in one tab would edit them against each other (its text is saved on the server and its tab is remembered). A sandbox's views are hidden when it leaves (the browser stream stops) and the editor of a background sandbox disposes its Monaco models (so they feed no other sandbox's suggestions) but keeps each open file's text, unsaved changes, dirty mark and cursor; a file opened, saved, reloaded or closed while it is in the background lands in that kept state. | `places.e2e.ts` "the editor keeps unsaved text …", "a file opened just before a sandbox switch …", "a removed sandbox's tool views …", `agent-panel.e2e.ts` |
| KST-007 | MCP's scope filter and the Dashboard's period are kept in the address (`?scope=`, `?days=`), so they come back with the page. | `places.e2e.ts` "MCP's scope and the Dashboard's period …" |

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression tests that fail
- [x] Sandbox tabs and page links return to where each was left
- [x] Each sandbox keeps its open tool and tool views across a switch
- [x] MCP scope and Dashboard period kept in the address
- [x] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green with docs, ADR and evidence

## Acceptance criteria

- In the mock: open studio's task #12, switch to product with its tab, then back with studio's
  tab: task #12 shows. The same with Tasks filtered to "Working", and with Home's card.
- On product, clicking product's own tab opens its composer.
- On studio's Tasks filtered, open Git, then Tasks in the sidebar: the filter is still on.
- With the Superagent open in studio and the panel closed in product, studio comes back with the
  Superagent open and product with no panel.
- Unsaved text typed in studio's editor is still there, marked modified, after a visit to product.
- MCP filtered to "Every sandbox" and the Dashboard on 7 days come back after another page.

## Test plan

- New `e2e/tests/places.e2e.ts` (mock, Chromium) with the cases above; six failed before the fix.
  "The tab of the sandbox in view opens its composer" and "a missing task is not remembered" passed
  before it: they guard the behaviour the fix must keep (the second fails if `forgetPlace` is
  removed, since the router now remembers every address in a sandbox).
- `agent-panel.e2e.ts` reads the shown Superagent only: the one of the sandbox left stays, hidden.
- Review round 1 cases, each failing before its fix: "a sandbox left without a tool …", "the back
  links of a session and an MCP server …" (MCP part; checked again by removing the fix), "a deleted
  skill …", "closing a CI run …", "a removed sandbox's tool views …". "The Tasks back link and the
  tab menu's Open …", "signing out forgets …" and "a file opened just before a sandbox switch …"
  cover behaviour of the first fix that had no test.
- Existing sandbox-switch tests (`navigation`, `agent-panel`, `terminals`, `notes`, `editor`)
  stay green.
- Full gate: `scripts/dev ci` and the whole e2e suite.

## Decisions

- [ADR-0092](../../docs/decisions/0092-screens-remember-where-they-were-left.md): remembered places
  live in the router (per browser tab, `sessionStorage`), opt-in per link (`data-restore`); tool
  views are kept for the 3 sandboxes used last, the editor suspending its Monaco models in the
  background. Alternatives (every link restoring, keeping every sandbox's views, keeping the
  places in `localStorage`) are in the ADR.
- Review round 1 (fresh session, `om-code-review`): request changes with 2 major findings (a sandbox
  left without a tool inherited the other's; the back links of sessions, MCP, skills, workflows and
  automations dropped the list's filters), all fixed test-first, as were the minor ones: the editor's
  save, reload, close and file-link paths while suspended, other "not found" pages, views of a
  removed sandbox, the CI dialog rewriting the next address. Nits taken: an editor with unsaved text
  is never evicted, places are bound to the signed-in person. Left as is: the Shell takes the
  keyboard when shown for a new context (it did before this change, #439).
- Not tested end to end: the 3-sandbox bound (the mock has 3 sandboxes), and the editor's 409
  choices while suspended (the mock cannot hold a save); both are guarded in code (`textOf`,
  `refreshFromDisk` on kept text).
