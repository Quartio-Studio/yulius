# 0278. Every tool panel tab strip moves like the sandbox tabs

## Status

Accepted. Specified in [SPEC-434](../../.ai/specs/SPEC-434-make-all-things-like-this-movable-and-be.md);
task #416. Extends SPEC-112, SPEC-136, SPEC-167 and SPEC-336 (the Notes and sandbox tabs).

## Context

The sandbox tabs in the top bar and the Notes tabs show a grip and move by drag, Alt+←/→ and their
menu, through one drag (`tabdrag.js`). The other tab strips of the tool panel look alike but did
not move: the Superagent's This sandbox and All sandboxes, the Shell's terminals and the Browser's
tabs; the Editor's file tabs moved but showed no grip. A person asked for all of them to move and
behave like the sandbox tabs.

## Decision

Every tab strip of the tool panel (Notes, Superagent, Shell, Browser, Editor) moves the same way:
`createTabDrag` for the pointer, Alt+←/→ for the focused tab, Move left / Move right in its context
menu, and the same grip (one `.tab-grip` style, shared with the top bar) and "Click to open, drag to
reorder" hint. Each strip keeps its order per browser in `localStorage`, as the Notes and sandbox
orders already are: `yulius.superagentOrder` (every sandbox), `yulius.shellOrder.<sandbox>` and
`yulius.browserTabOrder.<sandbox>`. Tabs the stored order does not name (opened since) come after
it in their own order (`inOrder`). The CI page's job tabs and the task review's Git tabs are page
sections in a meaningful order and stay fixed.

## Alternatives

- A server-kept order for terminals and Browser tabs: an API field and a migration for an order
  that only its one person sees, and the sandbox and Notes orders are per browser already.
- Moving only the Superagent tabs (the screenshot): the request says "all things like this", and
  strips that look alike but behave differently are the inconsistency it complains about.
- A copy of the grip style per strip: four more copies that would drift; one rule set serves all.

## Consequences

- A person arranges every strip the same way, with a mouse, the keyboard or the menu.
- The order does not follow the person to another browser or device.
- A new tab strip in the tool panel is expected to use `createTabDrag`, `inOrder` and `.tab-grip`.
