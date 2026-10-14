# 0106. The Superagent is opened from the tools rail, not a top-bar button

## Status

Accepted. Specified in [SPEC-132](../../.ai/specs/SPEC-132-delete-superagent-button-from-here-as-it.md);
amends [0036](0036-agent-in-the-right-panel.md), which gave the Superagent (then the Agent) a
top-bar button besides its place in the rail.

## Context

The top bar's right end showed Status, notifications, a **Superagent** button and the tools toggle.
The Superagent is also the first tool of the rail, which the toggle shows right under itself, so
the same panel had two controls a few pixels apart. A person asked to delete the top-bar one "as
it's already right under the button".

## Decision

The top bar has no Superagent button. The Superagent is opened from the tools rail: the toggle
(now "Show tools (Superagent, browser, editor, shell, notes)") shows the rail, and the rail's Superagent
opens and closes the panel. The links `/s/<slug>/superagent`, `/agent` and `/cockpit` still open it,
and a panel opened while the rail is hidden (by a link, or a sandbox returning to its tool) still
shows the rail only until the panel closes, so the person's Tools choice stays.

## Alternatives

- **Keep the button and drop the Superagent from the rail**: the rail is where every tool of the
  panel lives; one tool outside it would be the exception.
- **Show the button only while the rail is hidden**: still two controls for one panel, and one
  that comes and goes.
- **Keep both**: what the person asked to change.

## Consequences

Opening the Superagent from a hidden rail takes two clicks instead of one; once the rail is shown
(the choice is remembered) it is one. The top bar is shorter, which leaves the sandbox tabs more
room, also on phones. The code that kept the top-bar button in step with the panel
(`ctx.toggleAgent`, `ctx.onPanelChange`) is gone.
