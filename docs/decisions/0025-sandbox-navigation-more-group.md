# 0025. Six sandbox pages in the sidebar, the rest under "More"

## Status

Accepted. Specified in [SPEC-029](../../.ai/specs/SPEC-029-too-much-elements-here-only-cockpit-task.md).
The Cockpit has since left `NAV` for the right panel as the Agent ([0026](0026-agent-in-the-right-panel.md)).

## Context

The sandbox navigation had grown to eleven pages and pushed the active task list down the
sidebar. People asked for "only Cockpit, Tasks, Git, CI (and CD in the future), Secrets and
Dashboard", with the other pages "to be expanded" (task #27).

## Decision

`layout.js` keeps two lists: `NAV` (Cockpit, Tasks, Git, CI labelled "CI (tests)", Secrets, Dashboard) and `NAV_MORE`
(Sessions, Workflows, Skills, Automations, MCP). After `NAV` comes a "More" disclosure button
(`aria-expanded`, `aria-controls`) that shows the `NAV_MORE` links in place below it. It starts
collapsed; its state is a per-browser preference (`yulius.sidebarMoreOpen`), like the Stopped
task group. Arriving at a More page shows the group expanded for that page only, without writing
the preference, so the current page is not hidden and one visit does not undo the choice. A CD page
would join `NAV` next to CI.

## Alternatives

- A popover menu behind "More": a second layer with its own focus handling and phone behaviour,
  and the current page would be hidden inside it; the request says "expanded".
- Moving the secondary pages to the user menu or Settings: they are sandbox pages, not account
  or instance settings, and would leave the sandbox context.
- Removing pages: they stay reachable by link and deep link anyway; the request only shortens
  what shows at once.

## Consequences

Tests and people reach Sessions, Workflows, Skills, Automations and MCP with one more click when
the group is closed. A new sandbox page has to choose a list; pages used every day go in `NAV`.
