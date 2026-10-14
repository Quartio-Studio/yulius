# 0092. Screens remember where they were left

## Status

Accepted. Specified in
[SPEC-113](../../.ai/specs/SPEC-113-when-i-switch-between-screens-i-want-to.md).

## Context

Task #98 (issue #435): back in a sandbox after a visit to another one, the web UI showed that
sandbox's default screen (New task), not the one left. Every way into a sandbox linked to
`/s/<slug>`; the sidebar's page links dropped the filters the pages keep in their address; the tool
panel destroyed every tool view on a sandbox switch (the Superagent's view, the browser's tab, the
editor's open files with unsaved text, the terminals), and the open tool was one value for all
sandboxes. MCP's scope and the Dashboard's period were never in the address.

## Decision

- **Places in the router.** `router.js` remembers the last address (path and query) shown inside
  each sandbox and on each sandbox page, on every route change and every `replaceQuietly`. They
  live in `sessionStorage` (this browser tab), at most 200, and sign-out or an expired session
  forgets them, as does a sign-in as someone else. A page that says something is not there
  (`notFound`) is not kept as a place.
- **Restoring is opt-in per link.** A link marked `data-restore="sandbox"` (sandbox tabs, Home's
  cards, the phone picker and the tab menu's Open) or `"page"` (the sidebar's pages, the Tasks and
  Git back links) goes to the remembered address. A link to where the person already is opens the
  plain path, so the current sandbox's tab still opens its composer and a page link clears its
  filters. Other links (notifications, New task, terminal links) stay exact.
- **Tool views per sandbox.** The panel host remembers each sandbox's open tool (or none) and
  restores it on return; a sandbox not visited yet keeps the tool in view, as before. The
  Superagent, Browser, Editor and Shell views of the 3 sandboxes used last stay alive, hidden (the
  browser stream stops); older ones are destroyed, except an editor holding unsaved text, and a
  removed sandbox's go at once. The editor of a sandbox in the background
  disposes its Monaco models and keeps each file's text, unsaved mark and cursor. Notes is still
  rebuilt: its Team and Me notepads are shared by every sandbox and need one live copy per tab.
- MCP's scope and the Dashboard's period are kept in the address (`?scope=`, `?days=`).

## Alternatives

- **Every link restores.** Links that name a place (a notification about task #12, New task) must
  go exactly there; a per-link opt-in keeps them exact.
- **Places in `localStorage`.** They would follow the person into new tabs and past sign-in on a
  shared browser; a tab's places are what "where I left it" means, and a reload keeps them.
- **Keep every sandbox's views.** Each kept Superagent and Shell holds a WebSocket and a terminal;
  three sandboxes cover switching back and forth, and stay bounded.
- **Keep the editor's Monaco models in the background.** Monaco's models are global: another
  sandbox's files would feed the open sandbox's suggestions and language services (the reason the
  views were destroyed before).
- **A full in-memory cache of centre views.** Pages subscribe to the store and run timers; keeping
  them mounted while hidden would multiply reads and listeners. Their state is in their address.

## Consequences

Returning to a sandbox or a page shows it as it was left. A sandbox's Superagent and terminals keep
their screens across switches, at the cost of up to three sandboxes' sockets. The editor's undo
history does not survive a switch (its text does). A remembered task deleted while the
person was elsewhere shows "Task not found" once, with its way back; the next visit opens the
composer.
