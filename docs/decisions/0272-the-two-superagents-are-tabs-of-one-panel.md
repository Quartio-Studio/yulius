# 0272. The two Superagents are tabs of one Superagent panel

## Status

Accepted. Specified in [SPEC-428](../../.ai/specs/SPEC-428-this-2-superagents-thingy-should-be-some.md);
task #410 ("this 2-superagents-thingy should be somewhere here as a section like here", the
Superagent panel and the Notes tabs). Changes the web UI of
[0271](0271-a-global-superagent-acts-in-every-sandbox-of-its-person-through-yulius-tools-only.md)
(SPEC-427 GSA-006); the sessions, endpoints and tools stay as they are.

The Global message box is removed by
[0274](0274-the-global-superagent-takes-its-persons-keys.md).

## Context

The Global Superagent (ADR-0271) opened only from the rail on the pages outside a sandbox, and a
sandbox Superagent's note linked to it by leaving the sandbox (`/superagent`). Entering a sandbox
while it was open closed the panel, so that no sandbox session started by itself. A person working
in a sandbox who wanted the Superagent that sees every sandbox had to leave the sandbox for it.
Notes already shows several notepads of different reach as tabs of one panel (Team, This sandbox,
Me). The Global Superagent is one session per person; its read-only terminal sends its size, so
two live views of it in one page would size it against each other.

## Decision

- The Superagent panel has two tabs, **This sandbox** (the sandbox Superagent) and **All
  sandboxes** (the Global Superagent), in the Notes tabs' strip and keys. Outside a sandbox only
  All sandboxes is enabled.
- The tab chosen in a sandbox is remembered per browser and is the same in every sandbox; a
  session starts only when its tab first shows.
- The page holds one Global Superagent view; the panel showing All sandboxes holds it, so its
  session is attached once and its message box keeps its text from one sandbox to the next.
- With All sandboxes in front, the panel stays open when the person enters, leaves or switches a
  sandbox, and no sandbox Superagent starts; `/s/<slug>/superagent` asks for This sandbox.
- The sandbox note's link becomes a button that selects All sandboxes; the Global box's
  placeholder gives the example "check what my sandboxes need from each other and solve it".

## Alternatives

- **A second rail tool** ("Global Superagent" beside "Superagent"): splits one concept over two
  buttons and widens the rail; rejected for the section the person asked for.
- **A Global view per sandbox panel**: simpler to wire, but attaches the one session several times,
  sizes it against itself and loses typed text on a switch; rejected.
- **Draggable tabs as Notes**: two fixed tabs, local first, need no order; rejected.
- **A tab per sandbox panel, not shared**: the Global session is the same everywhere, so a person
  watching it would lose it on every sandbox switch; rejected.

## Consequences

- The Global Superagent is reachable from every page; GSA-006's "closes the panel instead" no
  longer holds and SPEC-427's e2e test was changed with it.
- `panels.js` owns the shared view (`sharedGlobal`) and the page's tab in front (`agentTab`, read
  from storage once at load), and destroys the view at sign-out; a panel only detaches it when its
  sandbox's views go. Only a tab the person picks is stored, so another browser tab's choice never
  switches this page's panel.
- The panel's title is "Superagent" everywhere; the tab names which one is shown.
