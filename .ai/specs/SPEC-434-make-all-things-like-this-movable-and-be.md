---
yulius-task: 9f702770-1042-4644-a3d5-e4d93a5a0a47
status: done
---

# SPEC-434: every tab strip of the tool panel moves like the sandbox tabs

Status: implemented
Date: 2026-11-06
Owner: Yulius task #416
Scope authorization: user request: "make all things like this movable and behave like this"
(screenshot 1: the Superagent panel's tabs, This sandbox and All sandboxes; screenshot 2: the top
bar's sandbox tabs, each with its grip, yulius, quartio-v2, product, studio).

## Goal

The sandbox tabs in the top bar show a grip and move by drag, by Alt+←/→ and by their menu
(SPEC-167, SPEC-336). The Notes tabs do the same (SPEC-112, SPEC-136). The other tab strips of the
tool panel look alike but do not move: the Superagent's This sandbox and All sandboxes, the Shell's
terminals and the Browser's tabs. Each of them now moves as the sandbox tabs do, shows the same
grip and hint, and keeps the order the person gave it. The Editor's file tabs already move but show
no grip; they get it.

## Context

- `assets/tabdrag.js` (`createTabDrag`, `movedBy`) is the one drag for a tab strip: a press past
  4 px drags (mouse and pen; touch scrolls), the other tabs slide aside, the tab settles, Escape puts
  it back, and the click ending a drag chooses nothing (`clickConsumed`). Notes, the Editor and the
  sandbox tabs use it; a strip redrawn during a press waits (`dragging`) and redraws on release.
- The cue is `icon('grip')` in a decorative span before the name, faint at rest and in the tab's
  colour when it is hovered, chosen or dragged, hidden on a device without a mouse or pen, with the
  title `DRAG_TAB_HINT` ("Click to open, drag to reorder", `dom.js`). Its CSS exists twice:
  `.tab-grip` (top bar) and `.notes-tab-grip` (Notes).
- Keys: Alt+←/→ moves the focused tab (`aria-keyshortcuts`); the context menu's Move left / Move
  right moves it without dragging (WCAG 2.5.7). Orders are per browser in `localStorage`
  (`yulius.notesOrder`, the sandbox order in `store.js`).
- `assets/agent.js` draws the Superagent tabs (`term-tabs superagent-tabs`, two fixed tabs; arrows
  move between them in the fixed `TABS` order; outside a sandbox This sandbox is disabled).
- `assets/panels.js` `createShellView` draws the terminals (`term-tabs`, in the server's order;
  double-click or F2 renames, × or Delete closes, arrows by `closableTabKeys`).
- `assets/browser.js` draws the Browser's tabs (`bp-tablist`, the order the sandbox Chrome sends,
  redrawn on every tab list or title change; × or Delete closes).
- `assets/editorview.js` draws the file tabs (`ed-tabs`), already dragged with `createTabDrag`.
- Out of scope: the CI page's job tabs and the task review's Git tabs are sections of a page in a
  meaningful order, not things a person keeps open; nothing there is to arrange.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| MTB-001 | The Superagent's two tabs move by drag, Alt+←/→ and the context menu (Move left / Move right), as the Notes tabs. The order is kept per browser for every sandbox and page (`yulius.superagentOrder`); a stored value that is not the two tabs is ignored. The arrow, Home and End keys follow the order shown; outside a sandbox they still skip the disabled This sandbox. A press or drag selects nothing; a disabled tab moves too. | e2e `global-superagent.e2e.ts` "the Superagent tabs move by drag, keyboard and menu and stay so" |
| MTB-002 | The Shell's terminal tabs move by drag, Alt+←/→ and the context menu. The order is kept per browser and sandbox (`yulius.shellOrder.<sandbox id>`); a new terminal goes last, a closed one leaves the order. A press on × closes and drags nothing; a tab being renamed does not drag. The list refreshed during a press is drawn once the tab is let go. | e2e `terminals.e2e.ts` "terminal tabs move by drag, keyboard and menu and stay so" |
| MTB-003 | The Browser's tabs move by drag, Alt+←/→ and the context menu. The order is kept per browser and sandbox (`yulius.browserTabOrder.<sandbox id>`); a tab the sandbox Chrome opens goes last. A press on × closes and drags nothing; a tab list arriving during a press is drawn once the tab is let go. | e2e `browser.e2e.ts` "browser tabs move by drag, keyboard and menu and stay so" |
| MTB-004 | Superagent, terminal, Browser and Editor tabs show the grip before their name, as the Notes and sandbox tabs (decorative, no text; faint at rest; the tab's colour when it is hovered, chosen or dragged; `grab` cursor, `grabbing` while dragging; hidden on a device without a mouse or pen), and their title ends with "Click to open, drag to reorder" (after the tab's own title where it has one). One grip style serves every strip. | e2e "… show the grip" cases in each file; `notes.e2e.ts` grip test on the shared rule |

## Acceptance criteria

- AC-434-1: in a sandbox, opening the Superagent shows a grip before This sandbox and All sandboxes;
  dragging All sandboxes before This sandbox swaps them, and they stay so after a reload and in
  another sandbox.
- AC-434-2: with three terminals open, dragging the third first, then Alt+→ and the menu, reorder
  them; the order stays after a reload; a new terminal comes last.
- AC-434-3: the Browser's tabs reorder the same way and keep their order after a reload.
- AC-434-4: the Editor's file tabs show the grip; their drag, keys and menu behave as before.
- AC-434-5: every strip has no axe violations; the existing Notes, Superagent, Shell, Browser and
  Editor tests still pass.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Superagent tabs move by drag, keys and menu with the shared grip
- [x] Terminal tabs move by drag, keys and menu and keep their order per sandbox
- [x] Browser tabs move by drag, keys and menu and keep their order per sandbox
- [x] Editor tabs show the grip and the click-or-drag hint
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Design

- `tabdrag.js` gains `inOrder(items, order, keyOf)`: the items in the stored order, those it does
  not name after them in their own order (the rule `store.js` applies to sandboxes). Each strip
  reads its stored order once, applies it on every draw, and writes the shown keys on a move.
- Each strip creates its `createTabDrag` with `keyOf` its `data-` key; tabs call `drag.press` on
  `pointerdown` (not from ×, not while renaming), their click first asks `clickConsumed()`, and
  redraws during a press are held (`dragging`) and done in `cancelled` or `commit`.
- Alt+←/→ (no Ctrl or Meta) uses `movedBy`, keeps the keyboard on the moved tab and is announced in
  `aria-keyshortcuts` with the strip's other keys. The context menu adds Move left / Move right,
  disabled at the ends (the terminals' menu also keeps its own items: Rename and Close).
- CSS: one `.tab-grip` rule for every strip (`.tab .tab-grip` keeps the top bar's margins); the
  `.notes-tabs` drag rules become `.term-tabs` rules; `.bp-tablist` gets the same
  reordering/dragging/settling rules.

## Test plan

- e2e (fail before): per strip, drag a tab past another with the mouse and read the order; Alt+← and
  Alt+→ at both ends; the context menu's Move left disabled on the first tab; reload and read the
  order again; a new terminal / Browser tab goes last; a press on × closes without moving; the grip
  is `aria-hidden`, first, `grab`, and the title ends with the hint; axe finds nothing.
- Superagent: the arrow keys follow the moved order; outside a sandbox the moved order still skips
  the disabled tab.
- Existing tests: `notes.e2e.ts` (grip rule renamed to `.tab-grip`), `layout.e2e.ts` (sandbox
  tabs), `editor.e2e.ts`, `terminals.e2e.ts`, `browser.e2e.ts`, `global-superagent.e2e.ts`.
- Not covered: a real touch screen (the media rule is checked as CSS, as SPEC-136 does).

## Decisions

- Every tab strip of the tool panel moves the same way (drag, Alt+←/→, menu) and keeps its order
  per browser (ADR-0278), as the Notes and sandbox tabs already do: the request names them all,
  and one behaviour is what makes them predictable. No server order: terminals and Browser tabs
  belong to one person in one sandbox, a per-browser order needs no API or migration, and it is
  what the sandbox order already does.
- The page tabs of the CI and Git pages stay fixed (see Context).
- One shared `.tab-grip` style instead of a copy per strip: four more copies would drift.

## Evidence

- Tests written first, each failing before its change: `global-superagent.e2e.ts` "the Superagent
  tabs move by drag, keyboard and menu and stay so" (no grip/hint), `terminals.e2e.ts` "terminal
  tabs move by drag, keyboard and menu and stay so", `browser.e2e.ts` "browser tabs move by drag,
  keyboard and menu and stay so", and the grip/hint assertion in `editor.e2e.ts` "the editor’s file
  tabs move by drag, Alt+arrows and the menu". The Notes grip test now reads the shared `.tab-grip`.
- Visual check in the Yulius browser (mock on port 20100): the Superagent, Browser and Shell strips
  show the grip as the top bar does.
- Independent review (fresh session, `om-code-review`), 3 rounds, approved on `e917b2ff`:
  - Round 1 (own code): the Superagent's other tab kept its slide offset after a drag (fixed in
    `tabdrag.js` `end()`, test `tabsInPlace`, which failed before); dragged/settling rules lost to
    the strips' `.active` rules (selector raised; a mutation reverting it fails the Superagent test);
    × presses, a rename and redraws during a press were not proven (tests added: `pressAndMove`,
    rename input, `sendTabs()` and a rename ending mid-press); the grip's 4 px gap is asserted and the
    tab's other parts keep 6 px.
  - Round 2: the mid-press redraw tests could pass without the `drag.dragging` guards; they now
    assert mid-press that the pressed tab is still the strip's (the reviewer's guard-removal
    mutation fails both, 4/4 runs; real code passes 10/10 twice).
  - Not changed: the per-sandbox order keys stay after a sandbox goes (pre-existing pattern of
    `yulius.shellActive.<id>`, bounded to the tabs of the last write); the nit to wait a mock tick
    after `sendTabs()` would need a fixed sleep, which the test rules forbid, and the next
    `evaluate` round-trip already follows it (4/4 runs held).
- `python3 scripts/bootstrap.py` then `scripts/dev ci`: exit 0, nextest 3165 passed, 16 skipped.
- `cd e2e && npx e2e run`: 75 files passed, 1 skipped; 863 tests passed, 1 skipped.
- `yulius security-scan`: "Security scan disabled" (Vidoc is off in Yulius): not run, not passed.
- Not covered: a real touch screen (the coarse-pointer rule is checked as CSS, as SPEC-136 does).
