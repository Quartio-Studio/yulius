# 0025. The Cockpit is shown as the Agent, a tool in the right panel

## Status

Accepted. Specified in [SPEC-035](../../.ai/specs/SPEC-035-move-cockpit-to-the-right-and-change-coc.md);
changes the UI part of [0020](0020-cockpit-as-a-plain-shell-session.md) (a full page first in the
sidebar).

## Context

People asked to "move cockpit to the right", call it "Agent", and say plainly that it is for quick
work (setting up automations, quick questions) while anything more complex, such as writing code,
belongs in a task. The Cockpit was a full page, first in the left sidebar, that replaced the page
in view. The right of the screen is the tools rail (Browser, Editor, Shell) and its resizable
panel, which stays beside the page.

## Decision

The web UI shows the Cockpit as **Agent**, the first tool of the right rail, opened in the tool
panel beside the page; a top-bar Agent button opens or closes it even while the rail is hidden.
The panel starts the session the first time it is shown and keeps it while hidden, like the other
tools. Opened from the top bar or a link while the rail is hidden, it shows the rail only until the
panel closes, so the person's Tools choice stays. A notice above the session says what it is for and links to New task. The left navigation
no longer lists it; `/s/<slug>/agent` and the old `/s/<slug>/cockpit` open the sandbox with the
panel, as `/s/<slug>/browser` does. The server, its API (`/api/sandboxes/{sid}/cockpit`), the shell
kind `cockpit` and stored records keep their names.

## Alternatives

- **A full page reached from the right of the top bar**: still replaces the page in view, so a
  quick question about the task on screen means leaving it.
- **A fixed right column of its own**: a second panel system next to the tools rail (widths,
  phones, maximize) for one view.
- **Renaming the API and shell kind to `agent`**: changes a public contract and persisted records
  for no visible gain; "Agent" is a UI name.

## Consequences

The Agent shares the panel with Browser, Editor and Shell: one of them is open at a time. The
session is not started by visiting a sandbox, only by opening the panel. Old bookmarks keep
working. Code and API documents still say "Cockpit" for the same session.
