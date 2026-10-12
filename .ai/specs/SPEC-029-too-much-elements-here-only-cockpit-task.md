---
yulius-task: 2c556d99-9a01-42d5-ace8-2b92733b99ae
status: in-progress
---

# SPEC-029: a shorter sandbox navigation with a "More" group

Status: draft
Date: 2026-10-12
Owner: Yulius task #27
Scope authorization: user request (task #27): "Too much elements here. Only Cockpit, Tasks, Git,
CI (and CD in the future), Secrets and Dashboard should be here. Other should be to be expanded",
with a screenshot of the sidebar's eleven navigation items.

## Goal

The sandbox navigation shows six pages: Cockpit, Tasks, Git, CI, Secrets and Dashboard. The rest
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
| NAV-001 | The sandbox navigation shows, in this order, Cockpit, Tasks, Git, CI, Secrets and Dashboard as links, then a "More" button. | `navigation.e2e.ts` |
| NAV-002 | "More" is a disclosure button (`aria-expanded`, `aria-controls`): collapsed by default it hides Sessions, Workflows, Skills, Automations and MCP; pressed it shows them below it, in that order, and pressed again hides them. | `navigation.e2e.ts` |
| NAV-003 | The choice is remembered in this browser (`yulius.sidebarMoreOpen`) and survives a reload. | `navigation.e2e.ts` |
| NAV-004 | Opening one of the More pages (a link, a deep link, a reload, the not-found page's links) expands the group, so the current page is always visible and marked as current. | `navigation.e2e.ts` |
| NAV-005 | No WCAG 2.x A/AA violation in the sidebar either way; the toggle works from the keyboard like any button. | `navigation.e2e.ts` (axe) |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] Sidebar shows six primary pages and a collapsible More group (UI, e2e)
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- A user sees only Cockpit, Tasks, Git, CI, Secrets, Dashboard and More in the sandbox
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
- **A More page opens the group** instead of marking the closed More button as current: the
  current page should never be hidden. The user can still close it; it then stays closed until
  they open another More page.
- **No CD entry yet**: there is no CD page to link to; it belongs in the primary list when it
  exists.
