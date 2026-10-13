---
yulius-task: 2c556d99-9a01-42d5-ace8-2b92733b99ae
status: done
---

# SPEC-029: a shorter sandbox navigation with a "More" group

Status: implemented
Date: 2026-10-12
Owner: Yulius task #27
Scope authorization: user request (task #27): "Too much elements here. Only Cockpit, Tasks, Git,
CI (and CD in the future), Secrets and Dashboard should be here. Other should be to be expanded",
with a screenshot of the sidebar's eleven navigation items; then, during delivery: "in CI in
brackets put 'tests'".

## Goal

The sandbox navigation shows six pages: Cockpit, Tasks, Git, CI (tests), Secrets and Dashboard. The rest
(Sessions, Workflows, Skills, Automations, MCP) is behind one "More" toggle that expands them in
place, so the sidebar leaves room for the task list and the pages used every day stay one click
away.

## Context

- `crates/yulius-server/web/assets/layout.js` `NAV` lists the eleven pages and `renderNav` builds
  the links once and updates them in place, so a focused link keeps focus across routes.
- The sidebar's "Stopped" task group is already a collapsible head (`button.task-group-head`,
  `aria-expanded`, remembered in `localStorage` with `readJson`/`writeJson`); the More toggle
  follows the same pattern.
- SPEC-001 UI-004 and the sidebar sketch list the navigation; SPEC-021 put Cockpit first.
- There is no CD page yet ("CD in the future"): nothing to add now; a CD page would join the
  primary list next to CI.
- e2e: `navigation.e2e.ts` clicks every page from the navigation and `sessions.e2e.ts` opens
  Sessions from it.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| NAV-001 | The sandbox navigation shows, in this order, Cockpit, Tasks, Git, "CI (tests)" (the CI page, whose title stays "CI"), Secrets and Dashboard as links, then a "More" button. The Cockpit has since moved to the right panel as the Agent (SPEC-035, ADR-0033). | `navigation.e2e.ts` |
| NAV-002 | "More" is a disclosure button (`aria-expanded`, `aria-controls`): collapsed by default it hides Sessions, Workflows, Skills, Automations and MCP; pressed it shows them below it, in that order, and pressed again hides them. | `navigation.e2e.ts` |
| NAV-003 | The choice is remembered in this browser (`yulius.sidebarMoreOpen`) and survives a reload. | `navigation.e2e.ts` |
| NAV-004 | Arriving at one of the More pages (a deep link, a reload, a move inside the app) shows the group expanded with that page marked current, for as long as that page is in view, without changing the remembered choice; closed there, it stays closed until another More page is opened, and More is marked as holding the current page (highlighted, `aria-current="true"`). When the group hides on leaving that page while focus is in it, focus moves to More. | `navigation.e2e.ts` |
| NAV-005 | No WCAG 2.x A/AA violation in the sidebar either way; the toggle works from the keyboard (Enter, Space). | `navigation.e2e.ts` (axe) |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Sidebar shows six primary pages and a collapsible More group (UI, e2e)
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- A user sees only Cockpit, Tasks, Git, CI (tests), Secrets, Dashboard and More in the sandbox
  navigation; More reveals the other five pages and hides them again.
- After a reload the group is as the user left it.
- On a More page (for example `/s/<slug>/mcp`) the group is open with that page marked current.

## Test plan

- e2e (mock): `navigation.e2e.ts` — the primary links in order and the five hidden; More expands
  and collapses (aria-expanded), every page opens from the navigation; the choice survives a
  reload; a deep link to a More page shows it expanded and current; axe clean.
- `sessions.e2e.ts` opens Sessions through More.
- The rest of the e2e suite (responsive, phone, quality) keeps passing.

## Decisions

- **Disclosure in place, not a popover menu** ([ADR-0025](../../docs/decisions/0025-sandbox-navigation-more-group.md)):
  the request says "expanded"; links in the sidebar keep the same look, focus order and route
  marking as the primary ones, work on the phone drawer unchanged, and need no menu layer.
- **Collapsed by default, remembered per browser**, like the Stopped group: a layout preference,
  not account data.
- **A More page opens the group for itself** instead of marking the closed More button as
  current: the current page should not be hidden on arrival. It is shown for that page only and
  never written to the remembered choice, so one visit does not undo the short sidebar. The user
  can still close it there; it then stays closed until they open another More page (arriving from
  a primary page counts as another).
- **No CD entry yet**: there is no CD page to link to; it belongs in the primary list when it
  exists.
- **Review round 1** (fresh session, APPROVE at 69aee79 with minors): opening a More page wrote
  "open" to the remembered choice, so one deep link undid the short sidebar; it is now shown for
  that page only (test: deep link, then Tasks and a reload stay collapsed). Tests now cover an
  in-app move to a More page, closing it there from the keyboard (Enter, Space) and staying closed
  within that page; the web README's `layout.js` row mentions More.
- **Review round 2** (fresh session, APPROVE at a18ce08 with minors): focus inside a group that
  hides on leaving its page moves to More instead of the top of the document, and More is marked
  active when it is closed on one of its pages, so the current page is always marked (tests in
  `navigation.e2e.ts`, failing before). Line wraps of the docs fixed.
- **Review round 3** (fresh session, APPROVE at 8a3c567 with minors): the closed More now also
  says it holds the current page to screen readers (`aria-current="true"`; "page" would claim the
  button is the page), NAV-004 and the e2e README record the focus move and the mark, and the test
  checks that More follows Dashboard. Not changed, by decision after three rounds: the same More
  page in another sandbox does not reopen a group closed on it (it is the same page; the choice
  carries), and reopening More on a More page saves "open" (it is the user's own action, like the
  toggle anywhere else).
- **"CI (tests)"** (request during delivery): only the navigation label says what CI is; the CI
  page keeps its title "CI", as the request names the sidebar entry and the page header has its
  own subtitle (`runs on this Yulius machine`).

## Evidence

- e2e (`cd e2e && npx e2e run`, mock) at e11feba: exit 0, 32 files passed, 1 skipped (the
  live-model `agent.e2e.ts`); 157 tests passed. `navigation.e2e.ts` (5 tests) covers NAV-001..005;
  each new assertion failed before its change (no More button; a deep link saved "open"; no focus
  move or mark; `aria-current` absent; the label "CI").
- `scripts/dev ci` (repository contracts, script tests, check, fmt, clippy `-D warnings`, nextest,
  doctests, deny, machete, doc): exit 0, 1166 tests run, 1166 passed, 16 skipped (no Rust change in
  this task); `python3 scripts/check_repository.py` at e11feba: passed.
- Seen in the browser (mock, 1280x800): the six pages and a collapsed More; More expands Sessions,
  Workflows, Skills, Automations and MCP below it.
- Not verified here: a real deployed instance (the UI is static and served the same way).
