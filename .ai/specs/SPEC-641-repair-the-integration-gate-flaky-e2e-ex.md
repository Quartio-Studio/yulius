---
yulius-task: 58d32986-54df-4a2b-8030-be560a652c54
status: done
---

# SPEC-641: A terminal shown late leaves the keyboard on the control the person moved to

Status: done
Date: 2026-11-24
Owner: Yulius task #625 (started by task #616's open-issues run; no issue may be created)
Scope authorization: task #625: "make the e2e test `tests/browser.e2e.ts › expired clipboard
actions preserve focus and explain how to copy again` deterministic. Fix its root cause, in the
web UI if the product loses focus wrongly. Deliver it in ONE pull request into dev."

## Goal

The integration gate on dev 320803966 (Yulius CI run be25f19b-f494-4bec-abc3-59eed7c34e11, job
`e2e`) failed one test, `browser.e2e.ts:131`: "Copy to my clipboard", focused by the test, was not
focused after the offer expired, and disappeared about 4 s later. The test passes deterministically
on any host speed, and the product no longer takes the keyboard from a control the person moved to
while a task's terminal was still loading.

## Context

- **Symptom (trace of the failed run).** Step 9 focused the toast's "Copy to my clipboard" while
  the page was still filling in (164 changes since step 5). From the first read after the expiry,
  the button showed no focus (39 reads), then at 4056 ms it was gone.
- **Root cause.** The task view asks its terminal for the keyboard when it opens
  (`terminal-core.js` `takeFocus`, SPEC-056 WUI-102). A request made before the terminal showed
  is kept and lands in `reveal()` once the first state and screen are drawn, unless another
  terminal was asked for since, or the person is typing in a text field (SPEC-052). Buttons do not
  hold it back ("Buttons and tabs do not hold it back"). On a loaded host the mock task terminal's
  first screen arrived after the test focused the toast button: `reveal()` called `term.focus()`
  and moved the keyboard to the terminal's `xterm-helper-textarea`. The toast's `focusout` then
  started its 4 s close timer (`dom.js` `toast`, ADR-0237), which explains the disappearance.
  The expiry handler, the "Needs you" region and `controlCopyExpiry` are not involved. Probes:
  firing the sign-in request (`askBrowserSignIn`) before or after the focus kept the focus; holding
  the terminal's first screen (`__yuliusHoldTerminal`) until after the focus reproduced the CI
  signature exactly (default state 39 reads, no node at 4071 ms). The focus-out stack is
  `reveal (terminal-core.js:303) → term.focus`.
- **Why the tests missed it.** On an idle host the terminal shows before the test reaches the
  Browser tool, so the deferred focus has already landed (on the rail button) before the toast
  exists. Only a slow first screen orders it after the person's own focus.
- **Same cause elsewhere.** Any control the person reaches before a task's terminal shows (a toast
  action, a tools rail button, a header button, a menu) lost the keyboard to the terminal. That is
  unexpected for keyboard and screen reader users (WCAG 3.2.1, 2.4.3), not only a test race.
- **Review round 1 (cdcbf595e) and SPEC-052.** The first fix compared the focused node at the
  request with the one at the reveal. SPEC-052 had tried and dropped that rule ("remember where the
  keyboard was when focus was asked for") because the UI re-creates focused controls: choosing a
  Shell tab rebuilds the strip and focuses the new tab node (`panels.js` `renderTabs`), and a count
  change repaints the sandbox tab row through `keepFocus` (`dom.js`). Round 1 found both again,
  plus a request made before `terminal-core.js` loaded: the wrapper in `terminal.js` replays
  `view.focus()` when the core arrives, so the snapshot was taken too late. A key on the focused
  Shell tab can close the terminal (`closableTabKeys`), so the first two were real regressions.
- **Review round 2 (6e96aedb3).** The counted moves held for round 1's cases. Round 2 found:
  - The clipboard test's "first screen" variant checked `.term-pending`, which exists only once
    the core has loaded, so a late core could fail it.
  - No test held the terminal-input exclusion (WUI-102, #323).
  - The first move after the control that opened the view left the page was not counted. That
    control leaves with its page for the composer's prompt (Ctrl+Enter), Go to task, a chain chip
    or a toast's "Open #N". A new task from the composer is the commonest slow terminal, and there
    the terminal still took the keyboard from Show tools.

## Requirements

| ID | Requirement | Verification |
| --- | --- | --- |
| TFK-001 | A terminal focus request made before the terminal showed does not land when the keyboard moved to another control since the request (not the page itself), also when the request came before the terminal's code loaded. | `browser.e2e.ts` "expired clipboard actions preserve focus…" (first screen late) and "the clipboard action keeps the focus when the terminal code loads after it" |
| TFK-002 | A control that a re-render re-created and focused again (its predecessor left the page) is not a move: the terminal still takes the keyboard. Focus moving into a terminal's own input is not a move either. | `terminals.e2e.ts` "a Shell tab chosen while its terminal still loads…", `terminal.e2e.ts` "a task terminal shown late takes the keyboard from its sandbox tab repainted meanwhile", `queued-messages.e2e.ts` "the Shell asked for last takes the keyboard from the task terminal that took it from the page" |
| TFK-005 | When the control that opened the task view left with its page and the keyboard is on the page at the request, the person's next focus counts as a move. | `terminal.e2e.ts` "a task opened from the composer / from Go to task leaves the keyboard on the control focused before its terminal shows" |
| TFK-003 | The WUI-102 and SPEC-052 rules stay: the keyboard on the page body takes the deferred focus; a later request of another terminal wins; typing in a text field holds it back. A request made after the person's move (the view opening later) still takes the keyboard, as the terminal asked for last. | `queued-messages.e2e.ts`, `terminals.e2e.ts`, `fork.e2e.ts` "a fork from the sidebar row menu gets the keyboard…" (the keyboard is on the page when the fork's view asks), full e2e suite |
| TFK-004 | The clipboard tests order the terminal explicitly (held, focus, released, then waited for with `terminalShown`), so they no longer depend on host speed. | the same tests, run repeatedly |

## Plan

- [x] Reproduce the focus loss deterministically by holding the terminal's first screen
- [x] Regression check: the clipboard focus test holds the terminal and fails without the fix
- [x] Deferred terminal focus no longer takes the keyboard from a control the person moved to
- [x] Terminal, queued-messages and browser e2e specs pass repeatedly; full e2e suite passes
- [x] Decision record and changes entry (the architecture document does not describe this rule)
- [x] Independent review round 1 (requested changes)
- [x] Round 1 answered: keyboard moves counted, re-rendered tabs and a late terminal core covered by tests
- [x] Round 2 review of the reworked rule
- [x] Round 2 answered: the first move after the opener left counts; WUI-102 terminal input, composer, Go to task and fork tests
- [x] Round 3 review

## Acceptance criteria

- The changed test fails on dev's `terminal-core.js` with the CI signature and passes with the fix;
  the three round-1 tests fail at cdcbf595e and pass with the reworked rule.
- `browser.e2e.ts` passes in full, repeatedly; the terminal focus specs pass.
- The full e2e suite passes locally once and on Yulius CI.

## Test plan

- `npx e2e run tests/browser.e2e.ts:131` with and without the fix (restore `terminal-core.js`
  from dev for the failing run).
- `npx e2e run tests/browser.e2e.ts` five times in a row; `terminal.e2e.ts`,
  `queued-messages.e2e.ts` and `editor.e2e.ts` in full.
- `npx e2e run` (whole suite) once.

## Decisions

- **Fix the product, not only the test.** Making the test wait for the terminal before focusing
  would hide a real focus theft. Recorded in an ADR.
- **"Moved on" is a counted keyboard move since the request** (round 1 replaced the node
  comparison). `dom.js` counts focus reaching another control while the control before it is still
  on the page; a control focused after its predecessor left the page is a re-render, the same place
  for the person. Terminals' inputs are not counted. A request stores the count (the `terminal.js`
  wrapper when the core has not loaded yet) and the reveal compares it. Alternatives: compare nodes
  (SPEC-052's dropped rule, round 1's regressions); mark every app restoration (`keepFocus`, each
  tab strip) by hand (a missed site is the same regression); count only moves after a pointer or key
  event (the e2e harness and assistive tools focus without one); hold back only inside toasts and
  dialogs (menus and rail buttons lose it the same way).
  A request reads the count with a control that already left the page forgotten when the keyboard
  is on the page (round 2): the opener of a task gone with the composer or the details page, so the
  person's next focus counts.
  Known gap: if the focused control leaves the page after the request (re-rendered away to the
  page body, with nothing focused in its place) and the person then focuses something else before
  the terminal shows, that move is not counted and the terminal takes the keyboard, as on dev.
  Recorded as [ADR-0472](../../docs/decisions/0472-a-terminal-shown-late-leaves-the-keyboard-where-the-person-moved-it.md).
- **A request after the person's move still wins.** Round 1's cold-load probe (focus "Show tools"
  as soon as it shows) logged the move before the task view's request (`request view=false
  moves=1`): the view opened after the focus, and the terminal asked for last takes the keyboard
  (WUI-102). Holding back there would need to tell a person's earlier move apart from the link
  click that opened the view; that is outside this repair.
- **No architecture change.** The fix is one rule in `dom.js` and the terminal; `docs/architecture.md`
  does not describe deferred terminal focus, so it stays as it is.

## Evidence

- Root cause probes (`target/run625/zprobe.e2e.ts.txt`, not committed): firing the mock's sign-in
  request (`askBrowserSignIn`) before the offer, between offer and focus, or after the focus kept
  the focus (5 load points, all passed). Holding the task terminal's first screen until after the
  focus reproduced the CI signature; the `focusout` stack was
  `reveal (terminal-core.js:303) → Dl.focus → yn.focus` (xterm).
- Regression check without the fix (commit dba6ca199, dev's `terminal-core.js`):
  `npx e2e run tests/browser.e2e.ts:133` failed: `expect.toBeFocused … default states (1 match)
  x38 -> at 3967ms no node (0 matches) x10`, as in CI run be25f19b (x39, 4056 ms).
- With the fix (34f1b1dc4): the same test passed (1.89 s).
  `npx e2e run tests/terminal.e2e.ts tests/queued-messages.e2e.ts tests/editor.e2e.ts
  tests/browser.e2e.ts`: 4 files, 96 tests passed.
- `npx e2e run tests/browser.e2e.ts` five times in a row: 7 passed each time.
- Whole suite `npx e2e run`: 103 files passed, 1 skipped; 1363 tests passed, 1 skipped
  (`agent.e2e.ts`, needs a model key); 3 m 29 s.
- `scripts/dev security`: "Security scan disabled: Vidoc is turned off in Yulius, for this project
  or the whole instance; nothing was scanned." (exit 0, unrun).
- Review round 1 (cdcbf595e) requested changes; its findings and answers are under Context and
  Decisions.
- Round 1 regression tests (commit 91f656193) at cdcbf595e's rule: "the clipboard action keeps the
  focus when the terminal code loads after it" failed (`toBeFocused`, no node); "a Shell tab chosen
  while its terminal still loads…" failed (`toBeFocused`, default states: the keyboard stayed on
  the tab); "a task terminal shown late takes the keyboard from its sandbox tab repainted
  meanwhile" failed (`toBeFocused`, default states).
- With the counted moves (a18c07f89): `npx e2e run tests/browser.e2e.ts tests/terminals.e2e.ts
  tests/terminal.e2e.ts tests/queued-messages.e2e.ts tests/editor.e2e.ts`: 5 files, 114 tests
  passed. The five focus tests (both clipboard variants, the two round 1 tests, "a Shell tab whose
  terminal has not opened yet…") passed five runs in a row. Whole suite `npx e2e run`: 103 files
  passed, 1 skipped; 1366 tests passed, 1 skipped (`agent.e2e.ts`); 3 m 47 s.
- The reviewer's probes at a18c07f89: a fork's new task, a sandbox tab restoring a task view, and a
  sidebar link with a repaint meanwhile all end with the keyboard in the terminal. The cold-load
  probe ends there too, because its focus came before the view's request (logged order:
  `focusin tools-toggle moves=1`, then `request view=false moves=1`). The sidebar-menu fork probe
  failed on its own selector (`.task-kebab` not found), not on focus.
- Round 2 tests (commit caeb715d2) at 6e96aedb3's rule: "a task opened from the composer…" and
  "a task opened from Go to task…" failed (`toBeFocused`, default states: the terminal took the
  keyboard). The WUI-102 test passed there and failed with the terminal-input clause removed from
  `dom.js` (`toBeFocused`, default states). The sidebar-menu fork test passes before and after: it
  guards that a fork's view, asking while the keyboard is on the page, takes it when it shows.
- With the round 2 fix (f7fc217f1): browser, terminals, terminal, queued-messages, editor and fork
  specs: 6 files, 122 tests passed.
  The eight focus tests (both clipboard variants, the Shell, sandbox tab, WUI-102, composer, Go to
  task and fork tests) passed five runs in a row. Whole suite `npx e2e run`: 103 files passed,
  1 skipped; 1370 tests passed, 1 skipped (`agent.e2e.ts`); 3 m 28 s.
- Review round 3 (6795f80d2, Opus 5.5) approved. It undid each part of the fix in turn, and a test
  failed for each. It reported one minor finding, the fork test's wording (fixed here), and one nit:
  the reset in `keyboardMoveCount()` does not treat `documentElement` as the page, though
  `keyboardOnPage()` does. That nit is left: Chromium never focuses `documentElement` on an HTML
  page.
- Not tested: real hosts' screen readers; the rule is checked in headless Chromium with the mock.
